//! The smooth power field: one sparse Hermitian solve (or, with nothing to
//! align to, the smoothest field by inverse iteration).

use faer::linalg::solvers::Solve;
use faer::sparse::{SparseColMat, Triplet};
use faer::{Mat, Side};
use glam::DVec3;

use super::frames::{Cx, DualEdge, FaceFrames, dual_edges};
use crate::control::{Control, Stage};
use crate::error::Error;

/// Symmetry of the field: a cross has 4 directions.
pub const ROSY: f64 = 4.0;
/// Most inverse-iteration steps for the smoothest (unconstrained) field.
const EIGEN_ITERATIONS: usize = 60;
/// Inverse iteration stops once the eigenvalue estimate changes by less than
/// this (relative) between steps.
const EIGEN_TOLERANCE: f64 = 1e-10;
/// Diagonal regularization, relative to the mean diagonal: keeps the system
/// positive definite (a flat region with no constraint has zero energy).
const REGULARIZATION: f64 = 1e-9;
/// The face count curvature alignment's strength is calibrated at: the soft
/// term is area-weighted so that, like the smoothness term, its total doesn't
/// change with resolution, and it is scaled to this many faces of the part.
const ALIGNMENT_REFERENCE_FACES: f64 = 10_000.0;

/// A face whose cross must follow `direction` exactly (features, borders).
#[derive(Clone, Copy, Debug)]
pub struct Hard {
    pub face: u32,
    pub direction: DVec3,
}

/// A face pulled towards `target` (a unit power-4 value in its frame) with
/// strength `weight` ∈ [0, 1] (curvature alignment).
#[derive(Clone, Copy, Debug)]
pub struct Soft {
    pub face: u32,
    pub target: Cx,
    pub weight: f64,
}

/// Per-face unit cross directions (one representative of each cross), and
/// their unit power values `z = e^{4iθ}` in each face's frame (1 on a
/// degenerate face, which has no frame).
#[derive(Clone, Debug)]
pub struct Field {
    pub directions: Vec<DVec3>,
    pub powers: Vec<Cx>,
}

/// Minimizes Σ over interior edges w_e |z_g − r_fg·z_f|² + α·Σ w_f·Â_f·|z_f − ẑ_f|²
/// with the hard faces fixed, where r_fg = e^{4i(φ_g − φ_f)} carries a cross
/// across the edge keeping its angle to the edge (φ: the edge's angle in each
/// face's frame), w_e = |e|²/(A_f + A_g), and Â_f is the face's share of the
/// part's area times [`ALIGNMENT_REFERENCE_FACES`]. The Hermitian system is
/// solved as its real symmetric 2n×2n form with faer's sparse Cholesky. With
/// no hard faces and no soft weight, the smoothest field is the eigenvector of
/// the smallest generalized eigenvalue (A·x = λ·M·x with the area mass matrix,
/// Knöppel et al. 2013), found by inverse iteration. Edges touching a
/// degenerate face are left out of the energy; such faces take a neighbour's
/// direction afterwards.
///
/// Errors are [`Error::cancelled`] when `control` is cancelled (checked after
/// assembly, after factorization and every iteration) and internal errors when
/// the system can't be built or factored.
pub fn smooth_field(
    positions: &[DVec3],
    triangles: &[[u32; 3]],
    hard: &[Hard],
    soft: &[Soft],
    alignment: f64,
    control: &Control,
) -> Result<Field, Error> {
    let stage = Stage::Field;
    let frames = FaceFrames::new(positions, triangles);
    let edges = dual_edges(triangles);
    let n = triangles.len();
    let mut fixed: Vec<Option<Cx>> = vec![None; n];
    for h in hard {
        if (h.face as usize) < n && !frames.is_degenerate(h.face) {
            fixed[h.face as usize] = Some(frames.power(h.face, h.direction, ROSY));
        }
    }
    let free: Vec<u32> = (0..n as u32).filter(|&f| fixed[f as usize].is_none()).collect();
    let mut slot = vec![usize::MAX; n];
    for (i, &f) in free.iter().enumerate() {
        slot[f as usize] = i;
    }
    let areas: Vec<f64> = triangles
        .iter()
        .map(|t| {
            let [a, b, c] = t.map(|v| positions[v as usize]);
            0.5 * (b - a).cross(c - a).length()
        })
        .collect();
    let problem = Problem {
        positions,
        frames: &frames,
        areas: &areas,
        fixed: &fixed,
        slot: &slot,
        unknowns: free.len(),
    };
    let unconstrained = hard.is_empty() && !soft.iter().any(|s| s.weight * alignment > 0.0);
    let system = problem.assemble(&edges, soft, alignment).map_err(|e| Error::internal(stage, e))?;
    control.checkpoint(stage)?;
    let solution =
        if unconstrained { system.smallest_eigenvector(control)? } else { system.solve(control)? };
    let mut powers: Vec<Cx> = (0..n).map(|f| fixed[f].unwrap_or(Cx::ONE)).collect();
    for (i, &f) in free.iter().enumerate() {
        let z = solution[i];
        powers[f as usize] = if z.norm() > 0.0 { z.scale(1.0 / z.norm()) } else { Cx::ONE };
    }
    for (f, z) in powers.iter_mut().enumerate() {
        if frames.is_degenerate(f as u32) {
            *z = Cx::ONE;
        }
    }
    let mut directions: Vec<DVec3> =
        (0..n as u32).map(|f| frames.direction(f, powers[f as usize].arg() / ROSY)).collect();
    fill_degenerate(&frames, &edges, &mut directions);
    Ok(Field { directions, powers })
}

/// What [`smooth_field`] assembles from.
struct Problem<'a> {
    positions: &'a [DVec3],
    frames: &'a FaceFrames,
    areas: &'a [f64],
    fixed: &'a [Option<Cx>],
    slot: &'a [usize],
    unknowns: usize,
}

/// The real form of the Hermitian system over the free faces: the lower
/// triangle as triplets (faer sums duplicates), the right-hand side, and the
/// per-unknown mass (face area over the mean, for the eigenvector branch).
struct System {
    unknowns: usize,
    triplets: Vec<Triplet<usize, usize, f64>>,
    diagonal: Vec<f64>,
    rhs: Vec<Cx>,
    mass: Vec<f64>,
}

impl Problem<'_> {
    fn assemble(&self, edges: &[DualEdge], soft: &[Soft], alignment: f64) -> Result<System, String> {
        let m = self.unknowns;
        let mut triplets = Vec::new();
        // At most 4 complex entries (each up to 3 lower real entries) per edge.
        triplets.try_reserve(12 * edges.len() + 2 * m).map_err(|_| "out of memory for the field system")?;
        let total_area: f64 = self.areas.iter().sum();
        let mean_area = if self.areas.is_empty() { 1.0 } else { total_area / self.areas.len() as f64 };
        let mut system = System {
            unknowns: m,
            triplets,
            diagonal: vec![0.0; 2 * m],
            rhs: vec![Cx::ZERO; m],
            mass: vec![1.0; m],
        };
        for (f, &i) in self.slot.iter().enumerate() {
            if i != usize::MAX && mean_area > 0.0 {
                system.mass[i] = (self.areas[f] / mean_area).max(1e-12);
            }
        }
        for e in edges {
            if self.frames.is_degenerate(e.f) || self.frames.is_degenerate(e.g) {
                continue;
            }
            let d = self.positions[e.b as usize] - self.positions[e.a as usize];
            let r = Cx::from_angle(ROSY * (self.frames.angle(e.g, d) - self.frames.angle(e.f, d)));
            // Dual-Laplacian weight: edge length over the distance between the faces.
            let w = d.length_squared() / (self.areas[e.f as usize] + self.areas[e.g as usize]);
            let (sf, sg) = (self.slot[e.f as usize], self.slot[e.g as usize]);
            match (sf != usize::MAX, sg != usize::MAX) {
                (true, true) => {
                    system.add(sf, sf, Cx::ONE.scale(w));
                    system.add(sg, sg, Cx::ONE.scale(w));
                    // One off-diagonal block; its mirror (conj) is the upper triangle.
                    let (row, col, v) =
                        if sg > sf { (sg, sf, r.scale(-w)) } else { (sf, sg, r.conj().scale(-w)) };
                    system.add(row, col, v);
                }
                (true, false) => {
                    system.add(sf, sf, Cx::ONE.scale(w));
                    let zg = self.fixed[e.g as usize].unwrap_or(Cx::ZERO);
                    system.rhs[sf] = system.rhs[sf] + (r.conj() * zg).scale(w);
                }
                (false, true) => {
                    system.add(sg, sg, Cx::ONE.scale(w));
                    let zf = self.fixed[e.f as usize].unwrap_or(Cx::ZERO);
                    system.rhs[sg] = system.rhs[sg] + (r * zf).scale(w);
                }
                (false, false) => {}
            }
        }
        let share = if total_area > 0.0 { ALIGNMENT_REFERENCE_FACES / total_area } else { 0.0 };
        for s in soft {
            let i = self.slot.get(s.face as usize).copied().unwrap_or(usize::MAX);
            if i == usize::MAX {
                continue;
            }
            let w = alignment * s.weight * self.areas[s.face as usize] * share;
            if w > 0.0 {
                system.add(i, i, Cx::ONE.scale(w));
                system.rhs[i] = system.rhs[i] + s.target.scale(w);
            }
        }
        Ok(system)
    }
}

impl System {
    /// Adds complex entry `v` at (`i`, `j`), `i ≥ j`, as the lower part of its
    /// 2×2 real block [[re, −im], [im, re]] and of the mirrored block.
    fn add(&mut self, i: usize, j: usize, v: Cx) {
        let m = self.unknowns;
        if i == j {
            // Diagonal entries of a Hermitian matrix are real.
            self.diagonal[i] += v.re;
            self.diagonal[i + m] += v.re;
            return;
        }
        // With i > j and i, j < m, the lower triangle of the two blocks
        // (i, j) = v and (j, i) = conj(v) holds exactly these four entries
        // (the other four are their mirrors in the upper triangle).
        self.triplets.push(Triplet::new(i, j, v.re));
        self.triplets.push(Triplet::new(i + m, j, v.im));
        self.triplets.push(Triplet::new(i + m, j + m, v.re));
        self.triplets.push(Triplet::new(j + m, i, -v.im));
    }

    /// Cholesky factor of the regularized (and optionally shifted by
    /// `shift` × mass) real matrix. Consumes the triplets.
    fn factor(&mut self, shift: f64) -> Result<faer::sparse::linalg::solvers::Llt<usize, f64>, String> {
        let m = self.unknowns;
        let size = 2 * m;
        let mean = if size > 0 { self.diagonal.iter().sum::<f64>() / size as f64 } else { 1.0 };
        let reg = (REGULARIZATION * mean).max(f64::MIN_POSITIVE);
        let mut triplets = std::mem::take(&mut self.triplets);
        triplets.try_reserve(size).map_err(|_| "out of memory for the field system")?;
        for k in 0..size {
            let mass = self.mass[k % m.max(1)];
            triplets.push(Triplet::new(k, k, self.diagonal[k] + reg + shift * mean * mass));
        }
        let matrix = SparseColMat::<usize, f64>::try_new_from_triplets(size, size, &triplets)
            .map_err(|e| format!("field matrix: {e:?}"))?;
        drop(triplets);
        matrix.sp_cholesky(Side::Lower).map_err(|e| format!("field factorization: {e:?}"))
    }

    fn solve(mut self, control: &Control) -> Result<Vec<Cx>, Error> {
        let m = self.unknowns;
        if m == 0 {
            return Ok(Vec::new());
        }
        let llt = self.factor(0.0).map_err(|e| Error::internal(Stage::Field, e))?;
        control.checkpoint(Stage::Field)?;
        let mut x =
            Mat::<f64>::from_fn(2 * m, 1, |k, _| if k < m { self.rhs[k].re } else { self.rhs[k - m].im });
        llt.solve_in_place(x.as_mut());
        Ok((0..m).map(|k| Cx { re: x[(k, 0)], im: x[(k + m, 0)] }).collect())
    }

    /// The eigenvector of the smallest generalized eigenvalue (A·x = λ·M·x),
    /// by inverse iteration from a fixed pseudo-random start, normalized in the
    /// mass norm. Stops when the eigenvalue estimate settles.
    fn smallest_eigenvector(mut self, control: &Control) -> Result<Vec<Cx>, Error> {
        let m = self.unknowns;
        if m == 0 {
            return Ok(Vec::new());
        }
        let llt = self.factor(1e-6).map_err(|e| Error::internal(Stage::Field, e))?;
        control.checkpoint(Stage::Field)?;
        let mass = |k: usize| self.mass[k % m];
        // A fixed pseudo-random start: a constant start in the face frames is
        // itself a structured field (on a UV sphere, the latitude field with
        // +1 poles) that inverse iteration is slow to leave.
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = || {
            state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            (state >> 11) as f64 / (1u64 << 53) as f64 - 0.5
        };
        let mut x = Mat::<f64>::from_fn(2 * m, 1, |_, _| next());
        let mut previous = f64::INFINITY;
        for _ in 0..EIGEN_ITERATIONS {
            for k in 0..2 * m {
                x[(k, 0)] *= mass(k);
            }
            llt.solve_in_place(x.as_mut());
            // ‖x‖_M after one step ≈ 1/λ for the normalized previous iterate.
            let norm = (0..2 * m).map(|k| x[(k, 0)] * x[(k, 0)] * mass(k)).sum::<f64>().sqrt();
            if norm > 0.0 {
                for k in 0..2 * m {
                    x[(k, 0)] /= norm;
                }
            }
            control.checkpoint(Stage::Field)?;
            if (norm - previous).abs() <= EIGEN_TOLERANCE * norm {
                break;
            }
            previous = norm;
        }
        Ok((0..m).map(|k| Cx { re: x[(k, 0)], im: x[(k + m, 0)] }).collect())
    }
}

/// Degenerate faces have no frame: give each the direction of a neighbour
/// that has one (breadth-first, in face order).
fn fill_degenerate(frames: &FaceFrames, edges: &[DualEdge], directions: &mut [DVec3]) {
    let n = directions.len();
    let mut neighbours: Vec<Vec<u32>> = vec![Vec::new(); n];
    for e in edges {
        neighbours[e.f as usize].push(e.g);
        neighbours[e.g as usize].push(e.f);
    }
    let mut known: Vec<bool> = (0..n as u32).map(|f| !frames.is_degenerate(f)).collect();
    let mut queue: std::collections::VecDeque<u32> = (0..n as u32).filter(|&f| known[f as usize]).collect();
    while let Some(f) = queue.pop_front() {
        for &g in &neighbours[f as usize] {
            if !known[g as usize] {
                known[g as usize] = true;
                directions[g as usize] = directions[f as usize];
                queue.push_back(g);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::Control;

    /// A flat `n`×`n` grid on z = 0.
    fn grid(n: u32) -> (Vec<DVec3>, Vec<[u32; 3]>) {
        let w = n + 1;
        let p =
            (0..=n).flat_map(|y| (0..=n).map(move |x| DVec3::new(f64::from(x), f64::from(y), 0.0))).collect();
        let mut t = Vec::new();
        for y in 0..n {
            for x in 0..n {
                let a = y * w + x;
                t.extend([[a, a + 1, a + w + 1], [a, a + w + 1, a + w]]);
            }
        }
        (p, t)
    }

    /// The cross of `d` as a 3D power: angle of `d` about +z, times 4.
    fn cross_angle(d: DVec3) -> Cx {
        Cx::from_angle(ROSY * d.y.atan2(d.x))
    }

    #[test]
    fn constant_field_on_plane() {
        let (p, t) = grid(10);
        let d = DVec3::new(1.0, 0.3, 0.0).normalize();
        let field = smooth_field(
            &p,
            &t,
            &[Hard { face: 0, direction: d }],
            &[],
            0.0,
            &crate::control::Control::default(),
        )
        .unwrap();
        for dir in &field.directions {
            assert!((cross_angle(*dir) - cross_angle(d)).norm() < 1e-6, "{dir:?}");
            assert!((dir.length() - 1.0).abs() < 1e-9);
        }
    }

    #[test]
    fn rotation_by_90_is_the_same_cross() {
        let (p, t) = grid(6);
        let d = DVec3::new(1.0, 0.3, 0.0).normalize();
        let a = smooth_field(
            &p,
            &t,
            &[Hard { face: 5, direction: d }],
            &[],
            0.0,
            &crate::control::Control::default(),
        )
        .unwrap();
        let b = smooth_field(
            &p,
            &t,
            &[Hard { face: 5, direction: DVec3::Z.cross(d) }],
            &[],
            0.0,
            &crate::control::Control::default(),
        )
        .unwrap();
        for (x, y) in a.directions.iter().zip(&b.directions) {
            assert!((cross_angle(*x) - cross_angle(*y)).norm() < 1e-6);
        }
    }

    #[test]
    fn soft_alignment_pulls_the_field() {
        let (p, t) = grid(6);
        let frames = FaceFrames::new(&p, &t);
        let d = DVec3::new(0.5, 3f64.sqrt() / 2.0, 0.0); // 60°
        let soft: Vec<Soft> = (0..t.len() as u32)
            .map(|f| Soft { face: f, target: frames.power(f, d, ROSY), weight: 1.0 })
            .collect();
        let field = smooth_field(&p, &t, &[], &soft, 0.3, &Control::default()).unwrap();
        for dir in &field.directions {
            assert!((cross_angle(*dir) - cross_angle(d)).norm() < 1e-6, "{dir:?}");
        }
    }

    #[test]
    fn featureless_sphere_gives_a_unit_field() {
        let (p, t) = crate::remesh::tests::sphere_mesh(12, 24);
        let field = smooth_field(&p, &t, &[], &[], 0.0, &Control::default()).unwrap();
        for (f, dir) in field.directions.iter().enumerate() {
            assert!((dir.length() - 1.0).abs() < 1e-9, "face {f}: {dir:?}");
        }
    }

    #[test]
    fn adjacent_degenerate_faces_keep_the_system_definite() {
        // Regression (M4 review): two zero-area faces sharing an edge gave that
        // edge an infinite weight and the factorization failed for the whole part.
        let (mut p, t) = grid(4);
        for v in [6u32, 7, 8] {
            p[v as usize].y = p[1].y; // collapse the second row onto the first
        }
        let field =
            smooth_field(&p, &t, &[Hard { face: 0, direction: DVec3::X }], &[], 0.0, &Control::default());
        let field = field.unwrap();
        assert!(field.directions.iter().all(|d| d.is_finite() && (d.length() - 1.0).abs() < 1e-9));
    }

    #[test]
    fn alignment_strength_does_not_depend_on_resolution() {
        // Regression (M4 review): the soft term had no area weight, so its pull
        // against smoothness grew with the face count.
        let total = |n: u32| {
            let (p, t) = grid(n);
            let p: Vec<DVec3> = p.into_iter().map(|q| q / f64::from(n)).collect();
            let frames = FaceFrames::new(&p, &t);
            let areas: Vec<f64> = t
                .iter()
                .map(|tri| {
                    let [a, b, c] = tri.map(|v| p[v as usize]);
                    0.5 * (b - a).cross(c - a).length()
                })
                .collect();
            let fixed = vec![None; t.len()];
            let slot: Vec<usize> = (0..t.len()).collect();
            let soft: Vec<Soft> =
                (0..t.len() as u32).map(|f| Soft { face: f, target: Cx::ONE, weight: 1.0 }).collect();
            let problem = Problem {
                positions: &p,
                frames: &frames,
                areas: &areas,
                fixed: &fixed,
                slot: &slot,
                unknowns: t.len(),
            };
            let with = problem.assemble(&[], &soft, 0.3).unwrap().diagonal.iter().sum::<f64>();
            let without = problem.assemble(&[], &[], 0.3).unwrap().diagonal.iter().sum::<f64>();
            with - without
        };
        let (coarse, fine) = (total(8), total(32));
        assert!((coarse - fine).abs() < 1e-9 * coarse, "{coarse} vs {fine}");
    }

    #[test]
    fn cancel_stops_the_solve() {
        let (p, t) = grid(10);
        let control = Control::default();
        control.cancel();
        let err =
            smooth_field(&p, &t, &[Hard { face: 0, direction: DVec3::X }], &[], 0.0, &control).unwrap_err();
        assert_eq!(err.kind, crate::error::ErrorKind::Cancelled);
    }

    #[test]
    fn degenerate_face_does_not_nan() {
        let (mut p, t) = grid(4);
        // Collapse one face by putting its third corner on its first edge.
        let [a, b, c] = t[7];
        p[c as usize] = 0.5 * (p[a as usize] + p[b as usize]);
        let field = smooth_field(
            &p,
            &t,
            &[Hard { face: 0, direction: DVec3::X }],
            &[],
            0.0,
            &crate::control::Control::default(),
        )
        .unwrap();
        assert!(field.directions.iter().all(|d| d.is_finite() && (d.length() - 1.0).abs() < 1e-9));
    }
}

#[cfg(test)]
mod perf {
    use super::*;

    #[test]
    #[ignore = "performance check; run with --release -- --ignored"]
    fn perf_field_200k_faces() {
        // A featureless sphere: the slowest path (inverse iteration, no constraints).
        let (p, t) = crate::remesh::tests::sphere_mesh(220, 460);
        let started = std::time::Instant::now();
        let field = smooth_field(&p, &t, &[], &[], 0.0, &Control::default()).unwrap();
        let elapsed = started.elapsed();
        eprintln!("{} faces in {elapsed:?}", field.directions.len());
        assert!(elapsed.as_secs_f64() < 5.0, "{elapsed:?}");
    }
}
