//! The smooth power field: one sparse Hermitian solve (or, with nothing to
//! align to, the smoothest field by inverse iteration).

use std::collections::HashMap;

use faer::linalg::solvers::Solve;
use faer::sparse::{SparseColMat, Triplet};
use faer::{Mat, Side};
use glam::DVec3;

use super::frames::{Cx, DualEdge, FaceFrames, dual_edges};

/// Symmetry of the field: a cross has 4 directions.
pub const ROSY: f64 = 4.0;
/// Inverse-iteration steps for the smoothest (unconstrained) field.
const EIGEN_ITERATIONS: usize = 60;
/// Diagonal regularization, relative to the mean diagonal: keeps the system
/// positive definite (a flat region with no constraint has zero energy).
const REGULARIZATION: f64 = 1e-9;

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

/// Per-face unit cross directions (one representative of each cross), and the
/// power values `z = e^{4iθ}` they come from.
#[derive(Clone, Debug)]
pub struct Field {
    pub directions: Vec<DVec3>,
    pub powers: Vec<Cx>,
}

/// Minimizes Σ over interior edges |z_g − r_fg·z_f|² + α·Σ w_f |z_f − ẑ_f|²
/// with the hard faces fixed, where r_fg = e^{4i(φ_g − φ_f)} carries a cross
/// across the edge keeping its angle to the edge (φ: the edge's angle in each
/// face's frame). The Hermitian system is solved as its real symmetric 2n×2n
/// form with faer's sparse Cholesky. With no hard faces and no soft weight the
/// smoothest field is the eigenvector of the smallest eigenvalue (Knöppel et
/// al. 2013), found by inverse iteration. Degenerate faces take a neighbour's
/// direction.
pub fn smooth_field(
    positions: &[DVec3],
    triangles: &[[u32; 3]],
    hard: &[Hard],
    soft: &[Soft],
    alignment: f64,
) -> Result<Field, String> {
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
    let system = assemble(positions, triangles, &frames, &edges, &fixed, &slot, free.len(), soft, alignment);
    let solution = if hard.is_empty() && !soft.iter().any(|s| s.weight * alignment > 0.0) {
        system.smallest_eigenvector()?
    } else {
        system.solve()?
    };
    let mut powers: Vec<Cx> = (0..n).map(|f| fixed[f].unwrap_or(Cx::ZERO)).collect();
    for (i, &f) in free.iter().enumerate() {
        powers[f as usize] = solution[i];
    }
    let mut directions: Vec<DVec3> =
        (0..n as u32).map(|f| frames.direction(f, powers[f as usize].arg() / ROSY)).collect();
    fill_degenerate(&frames, &edges, &mut directions);
    Ok(Field { directions, powers })
}

/// The real form of the Hermitian system over the free faces.
struct System {
    unknowns: usize,
    entries: HashMap<(usize, usize), f64>,
    rhs: Vec<Cx>,
}

#[allow(clippy::too_many_arguments)]
fn assemble(
    positions: &[DVec3],
    triangles: &[[u32; 3]],
    frames: &FaceFrames,
    edges: &[DualEdge],
    fixed: &[Option<Cx>],
    slot: &[usize],
    unknowns: usize,
    soft: &[Soft],
    alignment: f64,
) -> System {
    let mut system = System { unknowns, entries: HashMap::new(), rhs: vec![Cx::ZERO; unknowns] };
    for e in edges {
        let d = positions[e.b as usize] - positions[e.a as usize];
        let r = Cx::from_angle(ROSY * (frames.angle(e.g, d) - frames.angle(e.f, d)));
        let area = |f: u32| {
            let [a, b, c] = triangles[f as usize].map(|v| positions[v as usize]);
            0.5 * (b - a).cross(c - a).length()
        };
        // Dual-Laplacian weight: edge length over the distance between the faces,
        // ~ |e|² / (A_f + A_g). Uniform weights make singularities cheap where
        // faces are small (the fans at a UV sphere's poles).
        let w = d.length_squared() / (area(e.f) + area(e.g)).max(f64::MIN_POSITIVE);
        let (sf, sg) = (slot[e.f as usize], slot[e.g as usize]);
        match (sf != usize::MAX, sg != usize::MAX) {
            (true, true) => {
                system.add(sf, sf, Cx::ONE.scale(w));
                system.add(sg, sg, Cx::ONE.scale(w));
                system.add(sg, sf, r.scale(-w));
                system.add(sf, sg, r.conj().scale(-w));
            }
            (true, false) => {
                system.add(sf, sf, Cx::ONE.scale(w));
                let zg = fixed[e.g as usize].unwrap_or(Cx::ZERO);
                system.rhs[sf] = system.rhs[sf] + (r.conj() * zg).scale(w);
            }
            (false, true) => {
                system.add(sg, sg, Cx::ONE.scale(w));
                let zf = fixed[e.f as usize].unwrap_or(Cx::ZERO);
                system.rhs[sg] = system.rhs[sg] + (r * zf).scale(w);
            }
            (false, false) => {}
        }
    }
    for s in soft {
        let i = slot.get(s.face as usize).copied().unwrap_or(usize::MAX);
        let w = alignment * s.weight;
        if i != usize::MAX && w > 0.0 {
            system.add(i, i, Cx::ONE.scale(w));
            system.rhs[i] = system.rhs[i] + s.target.scale(w);
        }
    }
    system
}

impl System {
    /// Adds complex entry `v` at (`i`, `j`) as its 2×2 real block
    /// [[re, −im], [im, re]].
    fn add(&mut self, i: usize, j: usize, v: Cx) {
        let m = self.unknowns;
        for (row, col, x) in [(i, j, v.re), (i, j + m, -v.im), (i + m, j, v.im), (i + m, j + m, v.re)] {
            if x != 0.0 {
                *self.entries.entry((row, col)).or_default() += x;
            }
        }
    }

    /// Cholesky factor of the (regularized) real matrix.
    fn factor(&self, shift: f64) -> Result<faer::sparse::linalg::solvers::Llt<usize, f64>, String> {
        let size = 2 * self.unknowns;
        let diagonal: f64 = (0..size).map(|k| self.entries.get(&(k, k)).copied().unwrap_or(0.0)).sum();
        let mean = if size > 0 { diagonal / size as f64 } else { 1.0 };
        let reg = (REGULARIZATION * mean).max(f64::MIN_POSITIVE) + shift * mean;
        let mut keys: Vec<(usize, usize)> = self.entries.keys().copied().collect();
        keys.sort_unstable();
        let mut triplets: Vec<Triplet<usize, usize, f64>> = keys
            .into_iter()
            .filter(|&(r, c)| r >= c) // lower triangle is enough for Side::Lower
            .map(|(r, c)| Triplet::new(r, c, self.entries[&(r, c)]))
            .collect();
        triplets.extend((0..size).map(|k| Triplet::new(k, k, reg)));
        let matrix = SparseColMat::<usize, f64>::try_new_from_triplets(size, size, &triplets)
            .map_err(|e| format!("field matrix: {e:?}"))?;
        matrix.sp_cholesky(Side::Lower).map_err(|e| format!("field factorization: {e:?}"))
    }

    fn solve(&self) -> Result<Vec<Cx>, String> {
        let m = self.unknowns;
        if m == 0 {
            return Ok(Vec::new());
        }
        let llt = self.factor(0.0)?;
        let mut x =
            Mat::<f64>::from_fn(2 * m, 1, |k, _| if k < m { self.rhs[k].re } else { self.rhs[k - m].im });
        llt.solve_in_place(x.as_mut());
        Ok((0..m).map(|k| Cx { re: x[(k, 0)], im: x[(k + m, 0)] }).collect())
    }

    /// The eigenvector of the smallest eigenvalue, by inverse iteration from
    /// the constant field (deterministic start), normalized per face.
    fn smallest_eigenvector(&self) -> Result<Vec<Cx>, String> {
        let m = self.unknowns;
        if m == 0 {
            return Ok(Vec::new());
        }
        let llt = self.factor(1e-6)?;
        // A fixed pseudo-random start: a constant start in the face frames is
        // itself a structured field (on a UV sphere, the latitude field with
        // +1 poles) that inverse iteration is slow to leave.
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = || {
            state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            (state >> 11) as f64 / (1u64 << 53) as f64 - 0.5
        };
        let mut x = Mat::<f64>::from_fn(2 * m, 1, |_, _| next());
        for _ in 0..EIGEN_ITERATIONS {
            llt.solve_in_place(x.as_mut());
            let norm = (0..2 * m).map(|k| x[(k, 0)] * x[(k, 0)]).sum::<f64>().sqrt();
            if norm > 0.0 {
                for k in 0..2 * m {
                    x[(k, 0)] /= norm;
                }
            }
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
        let field = smooth_field(&p, &t, &[Hard { face: 0, direction: d }], &[], 0.0).unwrap();
        for dir in &field.directions {
            assert!((cross_angle(*dir) - cross_angle(d)).norm() < 1e-6, "{dir:?}");
            assert!((dir.length() - 1.0).abs() < 1e-9);
        }
    }

    #[test]
    fn rotation_by_90_is_the_same_cross() {
        let (p, t) = grid(6);
        let d = DVec3::new(1.0, 0.3, 0.0).normalize();
        let a = smooth_field(&p, &t, &[Hard { face: 5, direction: d }], &[], 0.0).unwrap();
        let b = smooth_field(&p, &t, &[Hard { face: 5, direction: DVec3::Z.cross(d) }], &[], 0.0).unwrap();
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
        let field = smooth_field(&p, &t, &[], &soft, 0.3).unwrap();
        for dir in &field.directions {
            assert!((cross_angle(*dir) - cross_angle(d)).norm() < 1e-6, "{dir:?}");
        }
    }

    #[test]
    fn featureless_sphere_gives_a_unit_field() {
        let (p, t) = crate::remesh::tests::sphere_mesh(12, 24);
        let field = smooth_field(&p, &t, &[], &[], 0.0).unwrap();
        for (f, dir) in field.directions.iter().enumerate() {
            assert!((dir.length() - 1.0).abs() < 1e-9, "face {f}: {dir:?}");
        }
    }

    #[test]
    fn degenerate_face_does_not_nan() {
        let (mut p, t) = grid(4);
        // Collapse one face by putting its third corner on its first edge.
        let [a, b, c] = t[7];
        p[c as usize] = 0.5 * (p[a as usize] + p[b as usize]);
        let field = smooth_field(&p, &t, &[Hard { face: 0, direction: DVec3::X }], &[], 0.0).unwrap();
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
        let field = smooth_field(&p, &t, &[], &[], 0.0).unwrap();
        let elapsed = started.elapsed();
        eprintln!("{} faces in {elapsed:?}", field.directions.len());
        assert!(elapsed.as_secs_f64() < 5.0, "{elapsed:?}");
    }
}
