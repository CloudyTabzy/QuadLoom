//! The integrability residual F and its exact derivatives (decision 0007).
//!
//! For a free interior edge e = (a, b), with f holding a→b and g holding b→a:
//!
//!   F_e = ρ_f(a→b) − ρ_g(b→a) + θ_g − θ_f + β_e
//!
//! and for halfedge p→q of a counter-clockwise triangle (p, q, r) with corner
//! angles α, and η = φ + θ − off (the angle of X1 from the halfedge):
//!
//!   ρ(p→q) = ½·cot α_r · ( −(u_q − u_p) + cos2η·Δv + sin2η·N )
//!
//! where v' is v times the corner sign, Δv = v'_q − v'_p, and
//! N = cot α_p (v'_r − v'_q) + cot α_q (v'_r − v'_p).

use faer::sparse::Triplet;

use super::setup::FrameMesh;
use crate::control::{Control, Stage};
use crate::error::Error;
use crate::trace::graph::CANCEL_EVERY;

/// The unknown vector x = [u (one per vertex) | v (one per non-odd vertex) | θ (one per non-held face)].
#[derive(Clone, Debug)]
pub struct Unknowns {
    /// Number of u unknowns (one per vertex).
    pub nu: usize,
    /// Number of v unknowns.
    pub nvv: usize,
    /// Number of θ unknowns.
    pub nt: usize,
    /// Position of each vertex's v in the v block, if it has one.
    pub v_index: Vec<Option<usize>>,
    /// Position of each face's θ in the θ block, if it has one.
    pub t_index: Vec<Option<usize>>,
}

/// Assigns consecutive indices to the entries that are not excluded.
fn number(excluded: impl Iterator<Item = bool>) -> (Vec<Option<usize>>, usize) {
    let mut count = 0;
    let index = excluded
        .map(|skip| {
            if skip {
                None
            } else {
                count += 1;
                Some(count - 1)
            }
        })
        .collect();
    (index, count)
}

impl Unknowns {
    pub fn new(mesh: &FrameMesh) -> Self {
        let (v_index, nvv) = number(mesh.odd.iter().copied());
        let (t_index, nt) = number(mesh.held.iter().copied());
        Self { nu: mesh.positions.len(), nvv, nt, v_index, t_index }
    }

    /// Total number of unknowns.
    pub fn len(&self) -> usize {
        self.nu + self.nvv + self.nt
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Splits x into per-vertex u, per-vertex v (zero at odd vertices) and per-face θ
    /// (zero at held faces). `nf` is the face count.
    pub fn unpack(&self, x: &[f64], nf: usize) -> (Vec<f64>, Vec<f64>, Vec<f64>) {
        let at = |k: usize| x.get(k).copied().unwrap_or(0.0);
        let u: Vec<f64> = (0..self.nu).map(at).collect();
        let mut v = vec![0.0; self.nu];
        for (m, k) in self.v_index.iter().enumerate() {
            if let (Some(k), Some(slot)) = (k, v.get_mut(m)) {
                *slot = at(self.nu + k);
            }
        }
        let mut theta = vec![0.0; nf];
        for (f, k) in self.t_index.iter().enumerate() {
            if let (Some(k), Some(slot)) = (k, theta.get_mut(f)) {
                *slot = at(self.nu + self.nvv + k);
            }
        }
        (u, v, theta)
    }
}

/// F on the free edges, its Jacobian and the Hessian of λ·F.
#[derive(Clone, Debug)]
pub struct Evaluation {
    /// F on each free edge, in the order of `mesh.free`.
    pub f: Vec<f64>,
    /// ∂F/∂x. Row = position in `mesh.free`. Duplicate entries add up.
    pub jacobian: Vec<Triplet<usize, usize, f64>>,
    /// ∂²(λ·F)/∂x² (both triangles, duplicates add up). Its entries are always
    /// emitted, zeros included, so the sparsity is the same on every call.
    pub hessian: Vec<Triplet<usize, usize, f64>>,
}

/// One halfedge's contribution ρ and its derivatives. Vertex indices are
/// the mesh's vertex ids; a face's own θ is what `dtheta` differentiates.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Halfedge {
    pub rho: f64,
    /// (vertex, ∂ρ/∂u) for p and q.
    pub du: [(usize, f64); 2],
    /// (vertex, ∂ρ/∂v) for p, q and r.
    pub dv: [(usize, f64); 3],
    /// ∂ρ/∂θ_f.
    pub dtheta: f64,
    /// ∂²ρ/∂θ_f².
    pub d2theta: f64,
    /// (vertex, ∂²ρ/∂θ_f∂v) for p, q and r.
    pub dtheta_v: [(usize, f64); 3],
}

/// ρ for halfedge `c` of face `f`, given per-vertex `u` and `v` (one entry per
/// vertex) and the face's `theta`.
pub(crate) fn halfedge(mesh: &FrameMesh, f: usize, c: usize, u: &[f64], v: &[f64], theta: f64) -> Halfedge {
    let t = mesh.triangles[f];
    let (cp, cq, cr) = (c, (c + 1) % 3, (c + 2) % 3);
    let (p, q, r) = (t[cp] as usize, t[cq] as usize, t[cr] as usize);
    let cot = mesh.cot[f];
    let sg = mesh.corner_sign[f];
    let k = 0.5 * cot[cr];
    let eta = mesh.phi[f] + theta - mesh.off[f][c];
    let (cs, sn) = ((2.0 * eta).cos(), (2.0 * eta).sin());
    let (sp, sq, sr) = (f64::from(sg[cp]), f64::from(sg[cq]), f64::from(sg[cr]));
    let (vp, vq, vr) = (sp * v[p], sq * v[q], sr * v[r]);
    let dv = vq - vp;
    let nvv = cot[cp] * (vr - vq) + cot[cq] * (vr - vp);
    let rho = k * (-(u[q] - u[p]) + cs * dv + sn * nvv);
    // Coefficients of Δv and N with respect to v_p, v_q, v_r (chained through the corner signs).
    let a = [-sp, sq, 0.0];
    let b = [-cot[cq] * sp, -cot[cp] * sq, (cot[cp] + cot[cq]) * sr];
    let dv_coef: [f64; 3] = std::array::from_fn(|i| k * (cs * a[i] + sn * b[i]));
    let dtv: [f64; 3] = std::array::from_fn(|i| k * (-2.0 * sn * a[i] + 2.0 * cs * b[i]));
    Halfedge {
        rho,
        du: [(p, k), (q, -k)],
        dv: [(p, dv_coef[0]), (q, dv_coef[1]), (r, dv_coef[2])],
        dtheta: k * (-2.0 * sn * dv + 2.0 * cs * nvv),
        d2theta: k * (-4.0 * cs * dv - 4.0 * sn * nvv),
        dtheta_v: [(p, dtv[0]), (q, dtv[1]), (r, dtv[2])],
    }
}

/// F on the free edges with its Jacobian, and (when `lambda` is given) the
/// Hessian of λ·F. Checkpoints every [`CANCEL_EVERY`] edges.
///
/// Errors: `Cancelled` when `control` is cancelled; `Internal` if `x` or
/// `lambda` doesn't match the mesh and unknowns.
pub fn evaluate(
    mesh: &FrameMesh,
    unknowns: &Unknowns,
    x: &[f64],
    lambda: Option<&[f64]>,
    control: &Control,
) -> Result<Evaluation, Error> {
    let stage = Stage::Frame;
    control.checkpoint(stage)?;
    if x.len() != unknowns.len() || lambda.is_some_and(|l| l.len() != mesh.free.len()) {
        return Err(Error::internal(stage, "frame evaluation: vector sizes do not match the mesh"));
    }
    let nf = mesh.triangles.len();
    let (u, v, theta) = unknowns.unpack(x, nf);
    let nu = unknowns.nu;
    let v_col = |m: usize| unknowns.v_index.get(m).copied().flatten().map(|k| nu + k);
    let theta_col = |f: usize| unknowns.t_index.get(f).copied().flatten().map(|k| nu + unknowns.nvv + k);
    let theta_of = |f: usize| theta.get(f).copied().unwrap_or(0.0);

    let mut out =
        Evaluation { f: Vec::with_capacity(mesh.free.len()), jacobian: Vec::new(), hessian: Vec::new() };
    for (row, &e) in mesh.free.iter().enumerate() {
        if row % (CANCEL_EVERY as usize) == 0 {
            control.checkpoint(stage)?;
        }
        let edge = &mesh.edges[e];
        let (fa, ga) = (edge.f as usize, edge.g as usize);
        let mut value = edge.beta + theta_of(ga) - theta_of(fa);
        for (face, corner, sign) in [(fa, edge.cf, 1.0), (ga, edge.cg, -1.0)] {
            let h = halfedge(mesh, face, corner as usize, &u, &v, theta_of(face));
            value += sign * h.rho;
            for (m, d) in h.du {
                out.jacobian.push(Triplet::new(row, m, sign * d));
            }
            for (m, d) in h.dv {
                if let Some(c) = v_col(m) {
                    out.jacobian.push(Triplet::new(row, c, sign * d));
                }
            }
            if let Some(tc) = theta_col(face) {
                out.jacobian.push(Triplet::new(row, tc, sign * h.dtheta));
                // λ·F carries the halfedge's sign, so the Hessian does too.
                let w = lambda.and_then(|l| l.get(row)).map_or(0.0, |l| l * sign);
                out.hessian.push(Triplet::new(tc, tc, w * h.d2theta));
                for (m, d) in h.dtheta_v {
                    if let Some(vc) = v_col(m) {
                        out.hessian.push(Triplet::new(tc, vc, w * d));
                        out.hessian.push(Triplet::new(vc, tc, w * d));
                    }
                }
            }
        }
        if let Some(tc) = theta_col(ga) {
            out.jacobian.push(Triplet::new(row, tc, 1.0));
        }
        if let Some(tc) = theta_col(fa) {
            out.jacobian.push(Triplet::new(row, tc, -1.0));
        }
        out.f.push(value);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use glam::DVec3;

    use super::*;
    use crate::field::{FieldParams, compute};
    use crate::testmesh;

    /// Deterministic values in [−1, 1) (64-bit LCG).
    struct Lcg(u64);

    impl Lcg {
        fn signed(&mut self) -> f64 {
            self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            2.0 * ((self.0 >> 11) as f64 / (1u64 << 53) as f64) - 1.0
        }
    }

    /// y = J·d for a triplet matrix with `rows` rows.
    fn apply(triplets: &[Triplet<usize, usize, f64>], d: &[f64], rows: usize) -> Vec<f64> {
        let mut y = vec![0.0; rows];
        for t in triplets {
            y[t.row] += t.val * d[t.col];
        }
        y
    }

    /// y = Jᵀ·w for a triplet matrix with `cols` columns.
    fn apply_t(triplets: &[Triplet<usize, usize, f64>], w: &[f64], cols: usize) -> Vec<f64> {
        let mut y = vec![0.0; cols];
        for t in triplets {
            y[t.col] += t.val * w[t.row];
        }
        y
    }

    fn norm(v: &[f64]) -> f64 {
        v.iter().map(|x| x * x).sum::<f64>().sqrt()
    }

    #[test]
    fn jacobian_matches_finite_differences() {
        let (p, t) = testmesh::icosphere(2);
        let control = Control::default();
        let field = compute(&p, &t, &[], &FieldParams::default(), &control).unwrap();
        let mesh = FrameMesh::new(&p, &t, &[], &field.directions, &vec![false; p.len()], &control).unwrap();
        let unknowns = Unknowns::new(&mesh);
        let n = unknowns.len();
        let m = mesh.free.len();
        let mut rng = Lcg(3);
        let x: Vec<f64> = (0..n).map(|_| 0.3 * rng.signed()).collect();
        let lambda: Vec<f64> = (0..m).map(|_| rng.signed()).collect();
        let h = 1e-6;

        let at = evaluate(&mesh, &unknowns, &x, Some(&lambda), &control).unwrap();
        for _ in 0..6 {
            let d: Vec<f64> = (0..n).map(|_| rng.signed()).collect();
            let shifted = |s: f64| -> Vec<f64> { x.iter().zip(&d).map(|(a, b)| a + s * b).collect() };
            let plus = evaluate(&mesh, &unknowns, &shifted(h), Some(&lambda), &control).unwrap();
            let minus = evaluate(&mesh, &unknowns, &shifted(-h), Some(&lambda), &control).unwrap();

            // J·d against the central difference of F.
            let jd = apply(&at.jacobian, &d, m);
            let fd: Vec<f64> = plus.f.iter().zip(&minus.f).map(|(a, b)| (a - b) / (2.0 * h)).collect();
            let err =
                norm(&jd.iter().zip(&fd).map(|(a, b)| a - b).collect::<Vec<_>>()) / norm(&fd).max(1e-12);
            assert!(err < 1e-6, "J·d relative error {err:e}");

            // H·d against the central difference of Jᵀλ.
            let hd = apply(&at.hessian, &d, n);
            let gp = apply_t(&plus.jacobian, &lambda, n);
            let gm = apply_t(&minus.jacobian, &lambda, n);
            let fd: Vec<f64> = gp.iter().zip(&gm).map(|(a, b)| (a - b) / (2.0 * h)).collect();
            let err =
                norm(&hd.iter().zip(&fd).map(|(a, b)| a - b).collect::<Vec<_>>()) / norm(&fd).max(1e-12);
            assert!(err < 1e-6, "H·d relative error {err:e}");
        }
    }

    #[test]
    fn zero_field_on_flat_grid_has_zero_residual() {
        let (p, t) = testmesh::grid(4, 4);
        let directions = vec![DVec3::X; t.len()];
        let mesh =
            FrameMesh::new(&p, &t, &[], &directions, &vec![false; p.len()], &Control::default()).unwrap();
        let n = p.len();
        let r = mesh.residuals(&vec![0.0; n], &vec![0.0; n], &vec![0.0; t.len()]);
        assert!(!r.is_empty());
        assert!(r.iter().fold(0.0f64, |m, x| m.max(x.abs())) < 1e-14);
    }
}
