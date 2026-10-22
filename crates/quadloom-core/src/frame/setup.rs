//! Frame setup: unit-area geometry, the transport between neighbouring faces,
//! held faces, corner signs and odd singularities (decision 0007).
//!
//! Notation (shared with `residual.rs`): face `f = [i, j, k]` is
//! counter-clockwise; corner `c` is vertex `t[c]`, and halfedge `c` runs
//! `t[c] → t[c+1]`. The field's angle in a face is measured from that face's
//! first edge, as in [`FaceFrames`].

use std::collections::{HashMap, HashSet};
use std::f64::consts::{FRAC_PI_2, PI};

use glam::DVec3;

use super::residual::halfedge;
use crate::control::{Control, Stage};
use crate::error::Error;
use crate::field::frames::FaceFrames;
use crate::trace::graph::CANCEL_EVERY;

/// A face whose area (after unit-area scaling) is below this is degenerate.
const DEGENERATE_AREA: f64 = 1e-12;
/// A face with a corner angle below this (radians) is degenerate.
const DEGENERATE_ANGLE: f64 = 1e-4;

/// An interior edge `{a, b}` with `a < b`, seen from its two faces: `f`
/// traverses `a → b` as its corner `cf`, and `g` traverses `b → a` as its
/// corner `cg`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameEdge {
    pub a: u32,
    pub b: u32,
    pub f: u32,
    pub cf: u8,
    pub g: u32,
    pub cg: u8,
    /// Transport angle left after the nearest whole quarter turns, in [−π/4, π/4].
    pub beta: f64,
    /// Quarter turns mod 4. An odd value flips the sign of v across the edge.
    pub jump: u8,
    /// Whether `{a, b}` is a feature edge.
    pub feature: bool,
}

/// Unit-area geometry and combinatorics of one connected part, ready for the
/// residual. Positions are scaled so that the total area is 1; every angle and
/// cotangent is unchanged by that.
#[derive(Clone, Debug)]
pub struct FrameMesh {
    /// Vertex positions, scaled to unit total area.
    pub positions: Vec<DVec3>,
    pub triangles: Vec<[u32; 3]>,
    /// Face areas after scaling.
    pub area: Vec<f64>,
    /// Interior angle at each corner (radians).
    pub alpha: Vec<[f64; 3]>,
    /// Cotangent of each corner angle. Zero on degenerate faces.
    pub cot: Vec<[f64; 3]>,
    /// The field's angle φ of each face's direction, in its frame.
    pub phi: Vec<f64>,
    /// Angle of halfedge `c` from halfedge 0, before adding φ and θ: η = φ + θ − off[c].
    pub off: Vec<[f64; 3]>,
    /// Unit face normals (zero on degenerate faces).
    pub normal: Vec<DVec3>,
    /// Every interior edge, sorted by `(a, b)`. Feature edges are included.
    pub edges: Vec<FrameEdge>,
    /// Indices into `edges` of the edges that carry a constraint.
    pub free: Vec<usize>,
    /// Faces whose θ is fixed at 0 (touching a feature or border edge, or degenerate).
    pub held: Vec<bool>,
    pub degenerate: Vec<bool>,
    /// Sign of v at each corner (±1).
    pub corner_sign: Vec<[i8; 3]>,
    /// Vertices with an odd index: their v is pinned to 0.
    pub odd: Vec<bool>,
}

impl FrameMesh {
    /// Builds the frame mesh of one part. `directions` holds one unit tangent
    /// per face (the cross field); `feature_edges` are vertex pairs in any order.
    ///
    /// Errors: `InvalidInput` for mismatched lengths, non-finite positions,
    /// repeated or out-of-range triangle vertices, a part without area, or a
    /// directed edge shared by two faces (non-manifold or misoriented input).
    pub fn new(
        positions: &[DVec3],
        triangles: &[[u32; 3]],
        feature_edges: &[[u32; 2]],
        directions: &[DVec3],
        control: &Control,
    ) -> Result<Self, Error> {
        let stage = Stage::Field; // Task 2: Stage::Frame
        control.checkpoint(stage)?;
        let invalid = |message: &str| Error::invalid_input(stage, message);
        let nv = positions.len();
        let nf = triangles.len();
        if directions.len() != nf {
            return Err(invalid("the frame needs one direction per face"));
        }
        if positions.iter().any(|p| !p.is_finite()) {
            return Err(invalid("non-finite vertex position"));
        }
        for t in triangles {
            let distinct = t[0] != t[1] && t[1] != t[2] && t[0] != t[2];
            if !distinct || t.iter().any(|&v| v as usize >= nv) {
                return Err(invalid("triangle with repeated or out-of-range vertices"));
            }
        }

        let raw_area: Vec<f64> = triangles.iter().map(|t| triangle_area(positions, t)).collect();
        let total: f64 = raw_area.iter().sum();
        if !(total.is_finite() && total > 0.0) {
            return Err(invalid("the part has no area"));
        }
        // Unit total area: objective weights then mean the same on any model.
        let scale = 1.0 / total.sqrt();
        let pos: Vec<DVec3> = positions.iter().map(|&p| p * scale).collect();
        let frames = FaceFrames::new(&pos, triangles);
        let area: Vec<f64> = raw_area.iter().map(|a| a * scale * scale).collect();

        let mut alpha = Vec::with_capacity(nf);
        let mut cot = Vec::with_capacity(nf);
        let mut off = Vec::with_capacity(nf);
        let mut degenerate = Vec::with_capacity(nf);
        let mut phi = Vec::with_capacity(nf);
        for (f, t) in triangles.iter().enumerate() {
            poll(control, stage, f)?;
            let angles: [f64; 3] = std::array::from_fn(|c| {
                let p = pos[t[c] as usize];
                let q = pos[t[(c + 1) % 3] as usize];
                let r = pos[t[(c + 2) % 3] as usize];
                (q - p).cross(r - p).length().atan2((q - p).dot(r - p))
            });
            let bad = area[f] < DEGENERATE_AREA || angles.iter().any(|&x| x < DEGENERATE_ANGLE);
            // A degenerate face takes no part in the residual; zero keeps every value finite.
            cot.push(angles.map(|x| if bad { 0.0 } else { 1.0 / x.tan() }));
            off.push([0.0, PI - angles[1], 2.0 * PI - angles[1] - angles[2]]);
            phi.push(frames.angle(f as u32, directions[f]));
            alpha.push(angles);
            degenerate.push(bad);
        }

        let mut halfedge_of: HashMap<(u32, u32), (u32, u8)> = HashMap::with_capacity(3 * nf);
        for (f, t) in triangles.iter().enumerate() {
            for c in 0..3usize {
                let key = (t[c], t[(c + 1) % 3]);
                if halfedge_of.insert(key, (f as u32, c as u8)).is_some() {
                    return Err(invalid(
                        "an edge is shared by more than two faces, or the faces are misoriented",
                    ));
                }
            }
        }

        let features: HashSet<(u32, u32)> =
            feature_edges.iter().map(|&[a, b]| (a.min(b), a.max(b))).collect();
        let mut edges = Vec::new();
        for (f, t) in triangles.iter().enumerate() {
            poll(control, stage, f)?;
            for c in 0..3usize {
                let (a, b) = (t[c], t[(c + 1) % 3]);
                if a > b {
                    continue; // recorded from the lower-to-higher halfedge
                }
                let Some(&(g, cg)) = halfedge_of.get(&(b, a)) else { continue };
                let ev = pos[b as usize] - pos[a as usize];
                // The field's angle across the edge: f's direction measured from ev, then
                // re-expressed in g's frame.
                let af = phi[f] - frames.angle(f as u32, ev);
                let eg = frames.angle(g, ev);
                let raw = phi[g as usize] - (eg + af);
                let k = (raw / FRAC_PI_2).round();
                edges.push(FrameEdge {
                    a,
                    b,
                    f: f as u32,
                    cf: c as u8,
                    g,
                    cg,
                    beta: raw - k * FRAC_PI_2,
                    jump: (k as i64).rem_euclid(4) as u8,
                    feature: features.contains(&(a, b)),
                });
            }
        }
        edges.sort_unstable_by_key(|e| (e.a, e.b));
        let edge_index: HashMap<(u32, u32), usize> =
            edges.iter().enumerate().map(|(i, e)| ((e.a, e.b), i)).collect();

        // A face is held if one of its edges is a feature or a border edge (used by one face).
        let mut held = degenerate.clone();
        for (f, t) in triangles.iter().enumerate() {
            for c in 0..3usize {
                let (p, q) = (t[c], t[(c + 1) % 3]);
                if features.contains(&(p.min(q), p.max(q))) || !halfedge_of.contains_key(&(q, p)) {
                    held[f] = true;
                }
            }
        }

        let (corner_sign, odd) = corner_signs(triangles, nv, &edges, &edge_index, &halfedge_of, control)?;
        let free: Vec<usize> = edges
            .iter()
            .enumerate()
            .filter(|(_, e)| !e.feature && !degenerate[e.f as usize] && !degenerate[e.g as usize])
            .map(|(i, _)| i)
            .collect();
        control.set_progress(stage, 1.0);
        Ok(Self {
            positions: pos,
            triangles: triangles.to_vec(),
            area,
            alpha,
            cot,
            phi,
            off,
            normal: frames.normal,
            edges,
            free,
            held,
            degenerate,
            corner_sign,
            odd,
        })
    }

    /// The residual F on every interior edge (features included), in the order
    /// of `edges`, for per-vertex `u` and `v` and per-face `theta`. Test helper.
    pub fn residuals(&self, u: &[f64], v: &[f64], theta: &[f64]) -> Vec<f64> {
        let th = |k: usize| theta.get(k).copied().unwrap_or(0.0);
        self.edges
            .iter()
            .map(|e| {
                let (f, g) = (e.f as usize, e.g as usize);
                let rho_f = halfedge(self, f, e.cf as usize, u, v, th(f)).rho;
                let rho_g = halfedge(self, g, e.cg as usize, u, v, th(g)).rho;
                rho_f - rho_g + th(g) - th(f) + e.beta
            })
            .collect()
    }
}

fn triangle_area(positions: &[DVec3], t: &[u32; 3]) -> f64 {
    let [a, b, c] = t.map(|v| positions[v as usize]);
    0.5 * (b - a).cross(c - a).length()
}

/// Checks for cancellation every [`CANCEL_EVERY`] items.
fn poll(control: &Control, stage: Stage, i: usize) -> Result<(), Error> {
    if i % CANCEL_EVERY as usize == 0 { control.checkpoint(stage) } else { Ok(()) }
}

/// The position of vertex `v` in triangle `t`.
fn corner_of(t: &[u32; 3], v: u32) -> Option<usize> {
    t.iter().position(|&x| x == v)
}

/// The sign of v at every corner, and the odd vertices (decision 0007).
///
/// Around each vertex m, a sign spreads across the fan: an interior edge {m, x}
/// with an odd quarter-turn jump flips it. If two routes disagree, m is odd.
/// Each fan is walked with an explicit stack, from its lowest face, so the
/// result does not depend on hash order. Faces the walk doesn't reach keep +1.
fn corner_signs(
    triangles: &[[u32; 3]],
    nv: usize,
    edges: &[FrameEdge],
    edge_index: &HashMap<(u32, u32), usize>,
    halfedge_of: &HashMap<(u32, u32), (u32, u8)>,
    control: &Control,
) -> Result<(Vec<[i8; 3]>, Vec<bool>), Error> {
    let stage = Stage::Field; // Task 2: Stage::Frame
    let nf = triangles.len();
    let mut fans: Vec<Vec<u32>> = vec![Vec::new(); nv];
    for (f, t) in triangles.iter().enumerate() {
        for &v in t {
            fans[v as usize].push(f as u32);
        }
    }
    let mut sign = vec![[1i8; 3]; nf];
    let mut odd = vec![false; nv];
    // seen[f]: the sign of face f in the current fan (0 = not reached yet).
    let mut seen = vec![0i8; nf];
    let mut touched: Vec<u32> = Vec::new();
    let mut stack: Vec<u32> = Vec::new();
    for (m, fan) in fans.iter().enumerate() {
        poll(control, stage, m)?;
        let Some(&start) = fan.first() else { continue };
        let vertex = m as u32;
        seen[start as usize] = 1;
        touched.push(start);
        stack.push(start);
        while let Some(f) = stack.pop() {
            let t = triangles[f as usize];
            let Some(c) = corner_of(&t, vertex) else { continue };
            for x in [t[(c + 1) % 3], t[(c + 2) % 3]] {
                // An interior edge {m, x} has its two halfedges in f and in the other face g.
                let (Some(&(h1, _)), Some(&(h2, _))) =
                    (halfedge_of.get(&(vertex, x)), halfedge_of.get(&(x, vertex)))
                else {
                    continue;
                };
                let g = if h1 == f { h2 } else { h1 };
                let Some(&ei) = edge_index.get(&(vertex.min(x), vertex.max(x))) else { continue };
                let flips = edges[ei].jump % 2 == 1;
                let s = if flips { -seen[f as usize] } else { seen[f as usize] };
                let gi = g as usize;
                if seen[gi] == 0 {
                    seen[gi] = s;
                    touched.push(g);
                    stack.push(g);
                } else if seen[gi] != s {
                    odd[m] = true;
                }
            }
        }
        for &f in fan {
            let fi = f as usize;
            if seen[fi] != 0 {
                if let Some(c) = corner_of(&triangles[fi], vertex) {
                    sign[fi][c] = seen[fi];
                }
            }
        }
        for &f in &touched {
            seen[f as usize] = 0;
        }
        touched.clear();
        stack.clear();
    }
    Ok((sign, odd))
}

#[cfg(test)]
mod tests {
    use std::f64::consts::{FRAC_PI_2, PI};

    use glam::DVec3;

    use super::*;
    use crate::field::{FieldParams, compute};
    use crate::testmesh;

    /// Deterministic uniform numbers in [0, 1) (64-bit LCG).
    struct Lcg(u64);

    impl Lcg {
        fn unit(&mut self) -> f64 {
            self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 11) as f64 / (1u64 << 53) as f64
        }
    }

    /// Cells on [0.1, 1] × [0.1, 0.9] (z = 0), alternating diagonals (CCW), interior
    /// vertices jittered by `jitter` cell widths from a fixed LCG (seed 1).
    fn jittered_grid(n: u32, jitter: f64) -> (Vec<DVec3>, Vec<[u32; 3]>) {
        let mut rng = Lcg(1);
        let h = 0.9 / f64::from(n);
        let mut p = Vec::new();
        for j in 0..=n {
            for i in 0..=n {
                let mut q =
                    [0.1 + 0.9 * f64::from(i) / f64::from(n), 0.1 + 0.8 * f64::from(j) / f64::from(n)];
                if i > 0 && i < n && j > 0 && j < n {
                    q[0] += jitter * h * (rng.unit() - 0.5);
                    q[1] += jitter * h * (rng.unit() - 0.5);
                }
                p.push(DVec3::new(q[0], q[1], 0.0));
            }
        }
        let id = |i: u32, j: u32| j * (n + 1) + i;
        let mut t = Vec::new();
        for j in 0..n {
            for i in 0..n {
                let (a, b, c, d) = (id(i, j), id(i + 1, j), id(i + 1, j + 1), id(i, j + 1));
                if (i + j) % 2 == 1 {
                    t.extend([[a, b, c], [a, c, d]]);
                } else {
                    t.extend([[a, b, d], [b, c, d]]);
                }
            }
        }
        (p, t)
    }

    /// Circumcenter of a triangle's (x, y) positions.
    fn circumcenter(t: [DVec3; 3]) -> (f64, f64) {
        let [a, b, c] = t;
        let d = 2.0 * (a.x * (b.y - c.y) + b.x * (c.y - a.y) + c.x * (a.y - b.y));
        let (a2, b2, c2) = (a.x * a.x + a.y * a.y, b.x * b.x + b.y * b.y, c.x * c.x + c.y * c.y);
        let ux = (a2 * (b.y - c.y) + b2 * (c.y - a.y) + c2 * (a.y - b.y)) / d;
        let uy = (a2 * (c.x - b.x) + b2 * (a.x - c.x) + c2 * (b.x - a.x)) / d;
        (ux, uy)
    }

    /// The circumcenter-based conformal θ of every face (θ = −circumcenter y).
    fn conformal_theta(p: &[DVec3], t: &[[u32; 3]]) -> Vec<f64> {
        t.iter().map(|tri| -circumcenter(tri.map(|i| p[i as usize])).1).collect()
    }

    fn rms(values: &[f64]) -> f64 {
        (values.iter().map(|x| x * x).sum::<f64>() / values.len() as f64).sqrt()
    }

    fn max_abs(values: &[f64]) -> f64 {
        values.iter().fold(0.0, |m, x| m.max(x.abs()))
    }

    /// The rectangular map of the test: a(x) = 1/(2√x), b(y) = (π/2) sin(πy), so
    /// u = ½ ln(ab) and v = ½ ln(a/b) at every vertex.
    fn rect_uv(p: &[DVec3]) -> (Vec<f64>, Vec<f64>) {
        let (mut u, mut v) = (Vec::new(), Vec::new());
        for q in p {
            let a = 1.0 / (2.0 * q.x.sqrt());
            let b = FRAC_PI_2 * (PI * q.y).sin();
            u.push(0.5 * (a * b).ln());
            v.push(0.5 * (a / b).ln());
        }
        (u, v)
    }

    #[test]
    fn jittered_grid_rectangular_map_converges_second_order() {
        let gamma: f64 = 0.4;
        let (c, s) = (gamma.cos(), gamma.sin());
        let mut rms_at = Vec::new();
        for n in [16u32, 32] {
            let (p0, t) = jittered_grid(n, 0.3);
            let p: Vec<DVec3> =
                p0.iter().map(|q| DVec3::new(c * q.x - s * q.y, s * q.x + c * q.y, 0.0)).collect();
            let directions = vec![DVec3::new(c, s, 0.0); t.len()];
            // The exact map is written in the unrotated grid coordinates.
            let (u, v) = rect_uv(&p0);
            let mesh = FrameMesh::new(&p, &t, &[], &directions, &Control::default()).unwrap();
            let r = mesh.residuals(&u, &v, &vec![0.0; t.len()]);
            rms_at.push(rms(&r));
        }
        assert!(rms_at[0] < 1.5e-3, "rms at n=16 is {:e}", rms_at[0]);
        assert!(rms_at[1] < rms_at[0] / 3.0, "rms at n=32 is {:e}, n=16 is {:e}", rms_at[1], rms_at[0]);
        assert!(rms_at[1] > 0.0);
    }

    #[test]
    fn conformal_exp_map_is_exact_with_circumcenter_theta() {
        let (p, t) = jittered_grid(16, 0.3);
        let directions = vec![DVec3::X; t.len()];
        let u: Vec<f64> = p.iter().map(|q| q.x).collect();
        let v = vec![0.0; p.len()];
        let theta = conformal_theta(&p, &t);
        let mesh = FrameMesh::new(&p, &t, &[], &directions, &Control::default()).unwrap();
        let r = mesh.residuals(&u, &v, &theta);
        assert!(max_abs(&r) < 1e-12, "max |F| is {:e}", max_abs(&r));
    }

    #[test]
    fn rotating_the_reference_frame_changes_nothing() {
        let (p, t) = jittered_grid(12, 0.3);
        let control = Control::default();
        let mut rng = Lcg(7);
        let gammas: Vec<f64> = (0..t.len()).map(|_| 0.6 * (rng.unit() - 0.5)).collect();

        // (a) Conformal case (v = 0). A quarter turn on every third face changes the
        // representation (quarter jumps and sign bits), not the constraint.
        let u: Vec<f64> = p.iter().map(|q| q.x).collect();
        let v = vec![0.0; p.len()];
        let theta = conformal_theta(&p, &t);
        let directions: Vec<DVec3> = (0..t.len())
            .map(|f| {
                let quarter = if f % 3 == 2 { FRAC_PI_2 } else { 0.0 };
                let a = gammas[f] + quarter;
                DVec3::new(a.cos(), a.sin(), 0.0)
            })
            .collect();
        let theta_rot: Vec<f64> = theta.iter().zip(&gammas).map(|(th, g)| th - g).collect();
        let rotated = FrameMesh::new(&p, &t, &[], &directions, &control).unwrap();
        let f_rot = rotated.residuals(&u, &v, &theta_rot);
        assert!(max_abs(&f_rot) < 1e-12, "conformal rotated max |F| is {:e}", max_abs(&f_rot));

        // (b) Rectangular case (v ≠ 0). A small rotation only: F must match elementwise.
        let (u, v) = rect_uv(&p);
        let zero = vec![0.0; t.len()];
        let unrotated = FrameMesh::new(&p, &t, &[], &vec![DVec3::X; t.len()], &control).unwrap();
        let f_unrot = unrotated.residuals(&u, &v, &zero);
        let directions: Vec<DVec3> = gammas.iter().map(|g| DVec3::new(g.cos(), g.sin(), 0.0)).collect();
        let theta_rot: Vec<f64> = gammas.iter().map(|g| -g).collect();
        let rotated = FrameMesh::new(&p, &t, &[], &directions, &control).unwrap();
        let f_rot = rotated.residuals(&u, &v, &theta_rot);
        assert_eq!(f_unrot.len(), f_rot.len());
        for (k, (a, b)) in f_unrot.iter().zip(&f_rot).enumerate() {
            assert!((a - b).abs() < 1e-12, "edge {k}: unrotated {a:e}, rotated {b:e}");
        }
    }

    #[test]
    fn cube_held_faces_and_features() {
        let (p, t, features) = testmesh::cube(4);
        let control = Control::default();
        let field = compute(&p, &t, &features, &FieldParams::default(), &control).unwrap();
        let mesh = FrameMesh::new(&p, &t, &features, &field.directions, &control).unwrap();

        for &[a, b] in &features {
            for (f, tri) in t.iter().enumerate() {
                if tri.contains(&a) && tri.contains(&b) {
                    assert!(mesh.held[f], "face {f} touches a feature edge but is free");
                }
            }
        }
        assert!(mesh.free.iter().all(|&e| !mesh.edges[e].feature), "a feature edge is free");

        let mut expected: Vec<u32> =
            field.singularities.iter().filter(|(_, k)| k.rem_euclid(2) == 1).map(|&(v, _)| v).collect();
        expected.sort_unstable();
        let actual: Vec<u32> = (0..p.len() as u32).filter(|&m| mesh.odd[m as usize]).collect();
        assert_eq!(actual, expected);
        assert_eq!(actual.len(), 8, "the cube's 8 corners are the odd singularities");

        assert!(mesh.corner_sign.iter().flatten().all(|&s| s == 1 || s == -1));
    }

    #[test]
    fn icosphere_odd_vertices_are_the_singularities() {
        let (p, t) = testmesh::icosphere(2);
        let control = Control::default();
        let field = compute(&p, &t, &[], &FieldParams::default(), &control).unwrap();
        let mesh = FrameMesh::new(&p, &t, &[], &field.directions, &control).unwrap();
        let mut expected: Vec<u32> =
            field.singularities.iter().filter(|(_, k)| k.rem_euclid(2) == 1).map(|&(v, _)| v).collect();
        expected.sort_unstable();
        let actual: Vec<u32> = (0..p.len() as u32).filter(|&m| mesh.odd[m as usize]).collect();
        assert_eq!(actual, expected);
    }

    #[test]
    fn degenerate_faces_are_held() {
        let (mut p, t) = testmesh::grid(3, 3);
        // Vertex 5 onto its neighbour 4: every face containing both collapses.
        p[5] = p[4];
        let directions = vec![DVec3::X; t.len()];
        let mesh = FrameMesh::new(&p, &t, &[], &directions, &Control::default()).unwrap();

        let collapsed: Vec<usize> = t
            .iter()
            .enumerate()
            .filter(|(_, tri)| tri.contains(&4) && tri.contains(&5))
            .map(|(f, _)| f)
            .collect();
        assert!(!collapsed.is_empty());
        for &f in &collapsed {
            assert!(mesh.degenerate[f], "face {f} should be degenerate");
            assert!(mesh.held[f], "face {f} should be held");
        }
        assert!(
            mesh.free.iter().all(|&e| {
                let edge = &mesh.edges[e];
                !mesh.degenerate[edge.f as usize] && !mesh.degenerate[edge.g as usize]
            }),
            "a free edge touches a degenerate face"
        );
        let n = p.len();
        let r = mesh.residuals(&vec![0.0; n], &vec![0.0; n], &vec![0.0; t.len()]);
        assert!(r.iter().all(|x| x.is_finite()));
    }
}
