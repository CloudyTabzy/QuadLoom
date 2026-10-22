//! Objective and Newton–KKT solve of the rectangular frame (decision 0007).
//!
//! Minimizes the quadratic Φ(x) = ½·xᵀHx + g0ᵀx over the unknowns, subject to
//! F(x) = 0 on the free edges. Each Newton step solves the KKT system
//!
//!   [H + Hf   Jᵀ ] [dx]   = −[∇Φ + Jᵀλ]
//!   [J      −δI ] [dλ]     [F]
//!
//! with a sparse LU from faer, then backtracks on the KKT residual norm.

use faer::Mat;
use faer::linalg::solvers::Solve;
use faer::sparse::{SparseColMat, Triplet};

use super::FrameParams;
use super::residual::{Unknowns, evaluate};
use super::setup::FrameMesh;
use crate::control::{Control, Stage};
use crate::error::Error;
use crate::trace::graph::CANCEL_EVERY;

/// Diagonal regularization of the constraint block (covers the one dependent
/// constraint of a closed featureless part).
const KKT_DELTA: f64 = 1e-10;
/// |F|∞ below which the iterate counts as converged.
const CONVERGED: f64 = 1e-8;
/// KKT residual norm below which the iterate counts as converged.
const KKT_CONVERGED: f64 = 1e-7;
/// |F|∞ a solution must reach to be used at all.
const ACCEPT: f64 = 1e-6;
/// Regularizes the size objective so that every u has a diagonal entry.
const GAUGE_U: f64 = 1e-6;
/// Regularizes the stretch objective; keeps v free when balance = 0.
const GAUGE_V: f64 = 1e-6;
/// Regularizes the rotation objective. Large enough to prefer the least
/// rotation (a global θ rotation is otherwise free), small enough not to pull
/// the solution away from the constraints.
const GAUGE_THETA: f64 = 1e-4;
/// Smallest backtracking step. A Newton direction that cannot reduce the merit
/// even at this step is stuck: the loop stops and the ACCEPT check decides.
const MIN_STEP: f64 = 1e-3;
/// Armijo factor for the KKT residual.
const ARMIJO: f64 = 1e-4;
/// Newton steps over which the merit must fall to count as progress.
const STALL_WINDOW: usize = 3;
/// After STALL_WINDOW steps, a merit above this fraction of its earlier value stops the loop.
const STALL_RATIO: f64 = 0.9;

/// The weights of the quadratic objective (see [`Weights::from_params`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Weights {
    /// Weight of u² (size).
    pub w_u: f64,
    /// Weight of (v' − t_f)² (stretch and its curvature target).
    pub w_v: f64,
    /// Weight of θ² (alignment to the field).
    pub w_theta: f64,
    /// Strength ε of the discrete Dirichlet energy on v.
    pub smoothness: f64,
}

impl Weights {
    /// The weights of `params`: w_u = 1 − balance, w_v = balance (plus
    /// `stretch_weight` with curvature stretch), w_θ = `align_weight`.
    pub fn from_params(params: &FrameParams) -> Self {
        let stretch = if params.curvature_stretch { params.stretch_weight } else { 0.0 };
        Self {
            w_u: 1.0 - params.balance,
            w_v: params.balance + stretch,
            w_theta: params.align_weight,
            smoothness: params.smoothness,
        }
    }
}

/// Sparse matrix entries (duplicates add up).
type Triplets = Vec<Triplet<usize, usize, f64>>;

/// A converged (or accepted) solution.
#[derive(Clone, Debug, PartialEq)]
pub struct Solution {
    /// The unknowns, in the layout of [`Unknowns`].
    pub x: Vec<f64>,
    /// Newton steps taken.
    pub iterations: usize,
    /// |F|∞ at `x`.
    pub residual: f64,
}

/// Solves the frame problem of one part. `targets`: per face v target t_f
/// (already blended; zeros when no curvature stretch); a missing entry is 0.
///
/// Returns `Ok(None)` when the solve did not reach [`ACCEPT`] (the caller
/// falls back to the plain field) and `Err` only for cancellation, which the
/// caller must propagate.
pub fn solve(
    mesh: &FrameMesh,
    unknowns: &Unknowns,
    weights: &Weights,
    targets: &[f64],
    max_iterations: usize,
    control: &Control,
) -> Result<Option<Solution>, Error> {
    Ok(run(mesh, unknowns, weights, targets, max_iterations, control)?.0)
}

/// The Newton loop behind [`solve`].
fn run(
    mesh: &FrameMesh,
    unknowns: &Unknowns,
    weights: &Weights,
    targets: &[f64],
    max_iterations: usize,
    control: &Control,
) -> Result<(Option<Solution>, usize), Error> {
    let stage = Stage::Frame;
    let n = unknowns.len();
    let m = mesh.free.len();
    if n == 0 || m == 0 {
        return Ok((None, 0));
    }
    let (h, g0) = objective(mesh, unknowns, weights, targets, control)?;

    let mut x = vec![0.0; n];
    let mut lambda = vec![0.0; m];
    let mut iterations = 0;
    // Merit of every iterate so far; the stall test compares against it.
    let mut merits: Vec<f64> = Vec::new();
    // |F|∞ at the current x; the loop always leaves it describing x.
    let mut residual;
    loop {
        control.checkpoint(stage)?;
        let at = evaluate(mesh, unknowns, &x, Some(&lambda), control)?;
        let grad = gradient(&h, &g0, &at.jacobian, &x, &lambda, n);
        residual = inf_norm(&at.f);
        let merit = norm2(&at.f).hypot(norm2(&grad));
        if !merit.is_finite() {
            return Ok((None, iterations));
        }
        if residual < CONVERGED && merit < KKT_CONVERGED {
            break;
        }
        // Stall: the merit fell by under 10% over the last STALL_WINDOW steps, so
        // more steps are unlikely to converge. The ACCEPT check below decides.
        merits.push(merit);
        let k = merits.len() - 1;
        let stalled = k >= STALL_WINDOW
            && merits.get(k - STALL_WINDOW).is_some_and(|&earlier| merit > STALL_RATIO * earlier);
        if iterations >= max_iterations || stalled {
            break;
        }

        // K = [[H + Hf, Jᵀ], [J, −δI]], rhs = −[g; F].
        let mut triplets = Vec::with_capacity(h.len() + at.hessian.len() + 2 * at.jacobian.len() + m);
        triplets.extend_from_slice(&h);
        triplets.extend_from_slice(&at.hessian);
        for t in &at.jacobian {
            triplets.push(Triplet::new(n + t.row, t.col, t.val));
            triplets.push(Triplet::new(t.col, n + t.row, t.val));
        }
        for k in 0..m {
            triplets.push(Triplet::new(n + k, n + k, -KKT_DELTA));
        }
        let Ok(matrix) = SparseColMat::<usize, f64>::try_new_from_triplets(n + m, n + m, &triplets) else {
            return Ok((None, iterations));
        };
        drop(triplets);
        let Ok(lu) = matrix.sp_lu() else { return Ok((None, iterations)) };
        let mut rhs = Mat::<f64>::from_fn(n + m, 1, |k, _| {
            -if k < n { grad[k] } else { at.f.get(k - n).copied().unwrap_or(0.0) }
        });
        lu.solve_in_place(rhs.as_mut());
        let step: Vec<f64> = (0..n + m).map(|k| rhs[(k, 0)]).collect();
        if !step.iter().all(|s| s.is_finite()) {
            return Ok((None, iterations));
        }
        let (dx, dl) = step.split_at(n);

        // Backtrack until the KKT residual norm decreases (Armijo).
        let mut t = 1.0;
        let mut accepted = None;
        while t >= MIN_STEP {
            control.checkpoint(stage)?;
            let xt: Vec<f64> = x.iter().zip(dx).map(|(a, b)| a + t * b).collect();
            let lt: Vec<f64> = lambda.iter().zip(dl).map(|(a, b)| a + t * b).collect();
            let trial = evaluate(mesh, unknowns, &xt, None, control)?;
            let gt = gradient(&h, &g0, &trial.jacobian, &xt, &lt, n);
            let merit_t = norm2(&trial.f).hypot(norm2(&gt));
            if merit_t < (1.0 - ARMIJO * t) * merit {
                accepted = Some((xt, lt));
                break;
            }
            t *= 0.5;
        }
        let Some((xt, lt)) = accepted else { break };
        x = xt;
        lambda = lt;
        iterations += 1;
    }

    let accepted = residual.is_finite() && residual <= ACCEPT && x.iter().all(|v| v.is_finite());
    let solution = accepted.then_some(Solution { x, iterations, residual });
    Ok((solution, iterations))
}

/// The quadratic objective: its Hessian as triplets (duplicates add up) and
/// its linear term g0 (the curvature targets). Built once per part.
fn objective(
    mesh: &FrameMesh,
    unknowns: &Unknowns,
    weights: &Weights,
    targets: &[f64],
    control: &Control,
) -> Result<(Triplets, Vec<f64>), Error> {
    let stage = Stage::Frame;
    let nu = unknowns.nu;
    // Column of the first θ unknown.
    let theta_base = unknowns.nu + unknowns.nvv;
    let mut h = Vec::new();
    let mut g0 = vec![0.0; unknowns.len()];
    for f in 0..mesh.triangles.len() {
        if f % CANCEL_EVERY as usize == 0 {
            control.checkpoint(stage)?;
        }
        let t = mesh.triangles[f];
        let area = mesh.area[f];
        let target = targets.get(f).copied().unwrap_or(0.0);
        for (c, &vertex) in t.iter().enumerate() {
            let m = vertex as usize;
            let share = area / 3.0;
            h.push(Triplet::new(m, m, 2.0 * (weights.w_u + GAUGE_U) * share));
            if let Some(k) = unknowns.v_index.get(m).copied().flatten() {
                let s = f64::from(mesh.corner_sign[f][c]);
                h.push(Triplet::new(nu + k, nu + k, 2.0 * (weights.w_v + GAUGE_V) * share));
                // (s·v − t)² has linear term −2·s·t·v.
                if let Some(slot) = g0.get_mut(nu + k) {
                    *slot -= 2.0 * weights.w_v * share * s * target;
                }
            }
        }
        // A degenerate face has no smoothness energy (its cotangents are zero).
        if !mesh.degenerate[f] {
            for c in 0..3 {
                let (p, q) = (t[c] as usize, t[(c + 1) % 3] as usize);
                let w = 0.5 * mesh.cot[f][(c + 2) % 3] * weights.smoothness;
                let (kp, kq) =
                    (unknowns.v_index.get(p).copied().flatten(), unknowns.v_index.get(q).copied().flatten());
                let (sp, sq) =
                    (f64::from(mesh.corner_sign[f][c]), f64::from(mesh.corner_sign[f][(c + 1) % 3]));
                // The energy is w·(s_q·v_q − s_p·v_p)²: Hessian 2w·aaᵀ with a = (−s_p, s_q).
                let a = [(kp, -sp), (kq, sq)];
                for &(ia, ca) in &a {
                    for &(ib, cb) in &a {
                        if let (Some(ia), Some(ib)) = (ia, ib) {
                            h.push(Triplet::new(nu + ia, nu + ib, 2.0 * w * ca * cb));
                        }
                    }
                }
            }
        }
        if let Some(k) = unknowns.t_index.get(f).copied().flatten() {
            h.push(Triplet::new(
                theta_base + k,
                theta_base + k,
                2.0 * (weights.w_theta + GAUGE_THETA) * area,
            ));
        }
    }
    Ok((h, g0))
}

/// ∇Φ + Jᵀλ = H·x + g0 + Jᵀλ.
fn gradient(
    h: &[Triplet<usize, usize, f64>],
    g0: &[f64],
    jacobian: &[Triplet<usize, usize, f64>],
    x: &[f64],
    lambda: &[f64],
    n: usize,
) -> Vec<f64> {
    let mut g = g0.to_vec();
    g.resize(n, 0.0);
    for t in h {
        if let (Some(slot), Some(xc)) = (g.get_mut(t.row), x.get(t.col)) {
            *slot += t.val * xc;
        }
    }
    for t in jacobian {
        if let (Some(slot), Some(lc)) = (g.get_mut(t.col), lambda.get(t.row)) {
            *slot += t.val * lc;
        }
    }
    g
}

fn inf_norm(v: &[f64]) -> f64 {
    v.iter().fold(0.0, |m, x| m.max(x.abs()))
}

fn norm2(v: &[f64]) -> f64 {
    v.iter().map(|x| x * x).sum::<f64>().sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::field::{CurvatureAlignment, FieldParams, compute};
    use crate::testmesh;

    /// Builds the frame problem of a mesh from its cross field.
    fn problem(
        p: &[glam::DVec3],
        t: &[[u32; 3]],
        features: &[[u32; 2]],
        params: &FieldParams,
    ) -> (FrameMesh, Unknowns) {
        let control = Control::default();
        let field = compute(p, t, features, params, &control).unwrap();
        let mesh =
            FrameMesh::new(p, t, features, &field.directions, &vec![false; p.len()], &control).unwrap();
        let unknowns = Unknowns::new(&mesh);
        (mesh, unknowns)
    }

    #[test]
    fn newton_converges_on_icosphere() {
        let (p, t) = testmesh::icosphere(2);
        let (mesh, unknowns) = problem(&p, &t, &[], &FieldParams::default());
        let weights = Weights::from_params(&FrameParams::default());
        let zeros = vec![0.0; t.len()];
        let sol = solve(
            &mesh,
            &unknowns,
            &weights,
            &zeros,
            FrameParams::default().max_iterations,
            &Control::default(),
        )
        .unwrap()
        .expect("the icosphere solve should be accepted");
        assert!(sol.residual < CONVERGED, "|F|inf is {:e}", sol.residual);
        assert!(sol.iterations <= FrameParams::default().max_iterations, "{} Newton steps", sol.iterations);
    }

    #[test]
    fn newton_converges_on_torus() {
        let (p, t) = testmesh::torus(32, 16);
        let (mesh, unknowns) = problem(&p, &t, &[], &FieldParams::default());
        let weights = Weights::from_params(&FrameParams::default());
        let zeros = vec![0.0; t.len()];
        let sol = solve(
            &mesh,
            &unknowns,
            &weights,
            &zeros,
            FrameParams::default().max_iterations,
            &Control::default(),
        )
        .unwrap()
        .expect("the torus solve should be accepted");
        assert!(sol.residual < CONVERGED, "|F|inf is {:e}", sol.residual);
    }

    #[test]
    fn balance_weights_follow_the_settings() {
        let w = Weights::from_params(&FrameParams { balance: 0.25, ..FrameParams::default() });
        assert_eq!((w.w_u, w.w_v, w.w_theta), (0.75, 0.25, 0.0));
        let curved = FrameParams {
            balance: 0.5,
            curvature_stretch: true,
            stretch_weight: 2.0,
            ..FrameParams::default()
        };
        assert_eq!(Weights::from_params(&curved).w_v, 2.5);
    }

    #[test]
    fn all_held_problem_has_no_free_edges() {
        // A lone triangle: every edge is a border edge, so nothing is constrained.
        let p = vec![glam::DVec3::X, glam::DVec3::Y, glam::DVec3::Z];
        let t = vec![[0, 1, 2]];
        let (mesh, unknowns) = problem(&p, &t, &[], &FieldParams::default());
        assert!(mesh.free.is_empty());
        let _ = (CurvatureAlignment::On, unknowns);
    }

    #[test]
    fn stalled_solve_stops_early() {
        let (p, t) = testmesh::icosphere(2);
        let (mut mesh, unknowns) = problem(&p, &t, &[], &FieldParams::default());
        // Shift every free edge's transport angle by 0.7 rad: the constraints then ask for a
        // holonomy the mesh cannot supply, so the merit stalls short of zero.
        for &e in &mesh.free {
            mesh.edges[e].beta += 0.7;
        }
        let weights = Weights::from_params(&FrameParams::default());
        let zeros = vec![0.0; t.len()];
        let max = FrameParams::default().max_iterations;
        let (solution, iterations) =
            run(&mesh, &unknowns, &weights, &zeros, max, &Control::default()).unwrap();
        assert!(solution.is_none(), "the shifted problem should not be accepted");
        assert!(iterations < max, "{iterations} steps of {max}");
        assert!(iterations <= 10, "{iterations} steps before the stall stop");
    }
}
