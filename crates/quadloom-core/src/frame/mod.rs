//! Stage 3b: rectangular frames (decision 0007, after Corman & Crane's
//! *Rectangular Surface Parameterization*, 2025).
//!
//! The cross field becomes an exactly integrable rectangular frame: a rotation
//! θ per face plus log scale factors u (size) and v (stretch) per vertex. The
//! constraint is one equation per free interior edge (Cartan's first structure
//! equation, discretized over the dual edges), so the frame is integrable by
//! construction. Singularities are unchanged, because θ is a 0-form.
//!
//! Each connected part is solved on its own (in parallel). A part that is too
//! large, has nothing to solve, or does not converge keeps the plain field, and
//! that is reported, never an error. Only cancellation propagates.

pub mod curvature;
pub mod residual;
pub mod setup;
pub mod solve;

use glam::DVec3;
use rayon::prelude::*;

use crate::components;
use crate::control::{Control, Stage};
use crate::error::{Error, ErrorKind};
use crate::field::FieldOutput;

/// Frame settings. Defaults are the opt-in defaults of decision 0007.
#[derive(Clone, Debug, PartialEq)]
pub struct FrameParams {
    /// Off by default: the stage is not entered.
    pub enabled: bool,
    /// 0 = keep the stretch (v) free and penalize size (u); 1 = the reverse.
    pub balance: f64,
    /// Whether curvature sets a stretch target per face.
    pub curvature_stretch: bool,
    /// Largest stretch ratio the curvature target may ask for.
    pub max_stretch: f64,
    /// Weight of the curvature target against the balance.
    pub stretch_weight: f64,
    /// Weight of the rotation θ² (follow the field when > 0).
    pub align_weight: f64,
    /// Strength ε of the discrete smoothness energy on v.
    pub smoothness: f64,
    /// Newton iterations per part before falling back.
    pub max_iterations: usize,
    /// Parts with more faces than this keep the plain field.
    pub max_part_faces: usize,
}

impl Default for FrameParams {
    fn default() -> Self {
        Self {
            enabled: false,
            balance: 0.5,
            curvature_stretch: false,
            max_stretch: 3.0,
            stretch_weight: 1.0,
            align_weight: 0.0,
            smoothness: 0.01,
            max_iterations: 50,
            max_part_faces: 300_000,
        }
    }
}

/// Stage 3b totals.
#[derive(Clone, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct FrameReport {
    /// Connected parts.
    pub parts: usize,
    /// Parts solved to acceptance.
    pub solved_parts: usize,
    /// Parts whose solve did not converge, or failed; they keep the plain field.
    pub fallback_parts: usize,
    /// Parts above `max_part_faces`; they keep the plain field.
    pub skipped_parts: usize,
    /// Newton steps, maximum over parts.
    pub iterations: usize,
    /// Largest |F|∞ over solved parts.
    pub residual: f64,
    /// Largest rotation |θ| over solved faces, in degrees.
    pub max_rotation_deg: f64,
    /// 5th percentile of the relative quad size e^{−ū} over solved faces.
    pub size_p5: f64,
    /// 95th percentile of the relative quad size.
    pub size_p95: f64,
    /// Median aspect ratio e^{2|v̄|} over solved faces.
    pub stretch_p50: f64,
    /// 95th percentile of the aspect ratio.
    pub stretch_p95: f64,
    /// Whether curvature stretch targets were used.
    pub curvature_stretch: bool,
}

/// The rectangular frame of a mesh.
#[derive(Clone, Debug, Default)]
pub struct FrameOutput {
    /// Per face: X1 = cosθ·d + sinθ·(n×d); the field's own direction where not solved.
    pub directions: Vec<DVec3>,
    /// Per face rotation θ (0 where not solved).
    pub theta: Vec<f64>,
    /// Per vertex size log u, normalized per part so Σ_f A_f e^{2ū_f} = Σ_f A_f (0 where not solved).
    pub u: Vec<f64>,
    /// Per vertex stretch log v (0 where not solved).
    pub v: Vec<f64>,
    /// Per face corner sign of v (±1).
    pub corner_sign: Vec<[i8; 3]>,
    /// Per face [e^{ū+v̄}, e^{ū−v̄}] ([1, 1] where not solved).
    pub scales: Vec<[f64; 2]>,
    pub report: FrameReport,
}

/// What one part came to.
enum Outcome {
    Solved(Box<PartSolution>),
    /// Above `max_part_faces`: plain field.
    Skipped,
    /// The setup or solve failed or did not converge: plain field.
    Fallback,
    /// Nothing to solve (no free edges or no unknowns): plain field, not counted.
    Untouched,
}

/// The frame of one solved part, in the part's own face and vertex numbering.
struct PartSolution {
    theta: Vec<f64>,
    directions: Vec<DVec3>,
    u: Vec<f64>,
    v: Vec<f64>,
    corner_sign: Vec<[i8; 3]>,
    scales: Vec<[f64; 2]>,
    iterations: usize,
    residual: f64,
}

/// Everything a part needs from the whole mesh.
struct Context<'a> {
    positions: &'a [DVec3],
    triangles: &'a [[u32; 3]],
    directions: &'a [DVec3],
    params: &'a FrameParams,
    weights: solve::Weights,
    control: &'a Control,
}

/// Solves the rectangular frame of every connected part of `triangles`, given
/// the cross field. Parts run in parallel and each solve is sequential, so the
/// result is deterministic. Errors only when cancelled or when the input is
/// invalid (field and mesh sizes disagree).
pub fn compute(
    positions: &[DVec3],
    triangles: &[[u32; 3]],
    feature_edges: &[[u32; 2]],
    field: &FieldOutput,
    params: &FrameParams,
    control: &Control,
) -> Result<FrameOutput, Error> {
    let stage = Stage::Frame;
    control.checkpoint(stage)?;
    let (nv, nf) = (positions.len(), triangles.len());
    if field.directions.len() != nf {
        return Err(Error::invalid_input(stage, "the frame needs one field direction per face"));
    }
    let parts = components::split(triangles, nv);
    let part_vertices: Vec<Vec<u32>> = parts
        .iter()
        .map(|part| {
            let mut vs: Vec<u32> = part.faces.iter().flat_map(|&f| triangles[f as usize]).collect();
            vs.sort_unstable();
            vs.dedup();
            vs
        })
        .collect();
    // Each feature edge goes to every part that holds both of its endpoints.
    let mut parts_of_vertex: Vec<Vec<usize>> = vec![Vec::new(); nv];
    for (k, vs) in part_vertices.iter().enumerate() {
        for &v in vs {
            if let Some(list) = parts_of_vertex.get_mut(v as usize) {
                list.push(k);
            }
        }
    }
    let mut part_features: Vec<Vec<[u32; 2]>> = vec![Vec::new(); parts.len()];
    for &[a, b] in feature_edges {
        let holders = parts_of_vertex.get(a as usize).map(Vec::as_slice).unwrap_or(&[]);
        for &k in holders {
            if part_vertices.get(k).is_some_and(|vs| vs.binary_search(&b).is_ok()) {
                if let Some(bucket) = part_features.get_mut(k) {
                    bucket.push([a, b]);
                }
            }
        }
    }

    let ctx = Context {
        positions,
        triangles,
        directions: &field.directions,
        params,
        weights: solve::Weights::from_params(params),
        control,
    };
    let done = std::sync::atomic::AtomicUsize::new(0);
    let results: Vec<Result<Outcome, Error>> = parts
        .par_iter()
        .zip(part_vertices.par_iter())
        .zip(part_features.par_iter())
        .map(|((part, vertices), features)| {
            let outcome = solve_part(&ctx, &part.faces, vertices, features)?;
            let finished = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            control.set_progress(stage, finished as f32 / parts.len().max(1) as f32);
            Ok(outcome)
        })
        .collect();

    let mut out = FrameOutput {
        directions: field.directions.clone(),
        theta: vec![0.0; nf],
        u: vec![0.0; nv],
        v: vec![0.0; nv],
        corner_sign: vec![[1; 3]; nf],
        scales: vec![[1.0; 2]; nf],
        report: FrameReport {
            parts: parts.len(),
            curvature_stretch: params.curvature_stretch,
            ..FrameReport::default()
        },
    };
    let mut sizes = Vec::new();
    let mut stretches = Vec::new();
    for ((part, vertices), result) in parts.iter().zip(&part_vertices).zip(results) {
        match result? {
            Outcome::Solved(sol) => {
                let report = &mut out.report;
                report.solved_parts += 1;
                report.iterations = report.iterations.max(sol.iterations);
                report.residual = report.residual.max(sol.residual);
                for (k, &f) in part.faces.iter().enumerate() {
                    let fi = f as usize;
                    let (Some(&theta), Some(&dir), Some(&sign), Some(&scale)) =
                        (sol.theta.get(k), sol.directions.get(k), sol.corner_sign.get(k), sol.scales.get(k))
                    else {
                        continue;
                    };
                    if let Some(slot) = out.theta.get_mut(fi) {
                        *slot = theta;
                    }
                    if let Some(slot) = out.directions.get_mut(fi) {
                        *slot = dir;
                    }
                    if let Some(slot) = out.corner_sign.get_mut(fi) {
                        *slot = sign;
                    }
                    if let Some(slot) = out.scales.get_mut(fi) {
                        *slot = scale;
                    }
                    report.max_rotation_deg = report.max_rotation_deg.max(theta.abs().to_degrees());
                    let ub = 0.5 * (scale[0].ln() + scale[1].ln());
                    let vb = 0.5 * (scale[0].ln() - scale[1].ln());
                    sizes.push((-ub).exp());
                    stretches.push((2.0 * vb.abs()).exp());
                }
                for (k, &m) in vertices.iter().enumerate() {
                    if let Some(slot) = out.u.get_mut(m as usize) {
                        *slot = sol.u.get(k).copied().unwrap_or(0.0);
                    }
                    if let Some(slot) = out.v.get_mut(m as usize) {
                        *slot = sol.v.get(k).copied().unwrap_or(0.0);
                    }
                }
            }
            Outcome::Skipped => out.report.skipped_parts += 1,
            Outcome::Fallback => out.report.fallback_parts += 1,
            Outcome::Untouched => {}
        }
    }
    sizes.sort_unstable_by(f64::total_cmp);
    stretches.sort_unstable_by(f64::total_cmp);
    let report = &mut out.report;
    report.size_p5 = percentile(&sizes, 0.05);
    report.size_p95 = percentile(&sizes, 0.95);
    report.stretch_p50 = percentile(&stretches, 0.5);
    report.stretch_p95 = percentile(&stretches, 0.95);
    control.set_progress(stage, 1.0);
    Ok(out)
}

/// Nearest-rank percentile of sorted values; 1 when there are none.
fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 1.0;
    }
    let rank = ((p * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len());
    sorted.get(rank - 1).copied().unwrap_or(1.0)
}

/// One part: its own numbering, mesh, solve and outputs. A failed or
/// unconverged setup or solve falls back to the plain field; cancellation is
/// passed on.
fn solve_part(
    ctx: &Context<'_>,
    faces: &[u32],
    vertices: &[u32],
    feature_edges: &[[u32; 2]],
) -> Result<Outcome, Error> {
    if faces.len() > ctx.params.max_part_faces {
        return Ok(Outcome::Skipped);
    }
    let local = |v: u32| vertices.binary_search(&v).ok().map_or(0, |i| i as u32);
    let part_positions: Vec<DVec3> =
        vertices.iter().map(|&v| ctx.positions.get(v as usize).copied().unwrap_or(DVec3::ZERO)).collect();
    let part_triangles: Vec<[u32; 3]> =
        faces.iter().map(|&f| ctx.triangles.get(f as usize).copied().unwrap_or([0; 3]).map(local)).collect();
    let part_directions: Vec<DVec3> =
        faces.iter().map(|&f| ctx.directions.get(f as usize).copied().unwrap_or(DVec3::X)).collect();
    let part_features: Vec<[u32; 2]> = feature_edges.iter().map(|&[a, b]| [local(a), local(b)]).collect();

    let control = ctx.control;
    let mesh = match setup::FrameMesh::new(
        &part_positions,
        &part_triangles,
        &part_features,
        &part_directions,
        control,
    ) {
        Ok(mesh) => mesh,
        Err(e) if e.kind == ErrorKind::Cancelled => return Err(e),
        Err(_) => return Ok(Outcome::Fallback),
    };
    let unknowns = residual::Unknowns::new(&mesh);
    if unknowns.is_empty() || mesh.free.is_empty() {
        return Ok(Outcome::Untouched);
    }
    // Curvature targets, blended so that w_v (v − t)² matches the stretch
    // term stretch_weight·(v − raw)²: t = stretch_weight·raw / w_v.
    let targets = if ctx.params.curvature_stretch {
        let raw = match curvature::aspect_targets(
            &mesh.positions,
            &mesh.triangles,
            &part_directions,
            ctx.params.max_stretch,
            curvature::SMOOTHING_STEPS,
            control,
        ) {
            Ok(raw) => raw,
            Err(e) if e.kind == ErrorKind::Cancelled => return Err(e),
            // A target that cannot be computed is zero: the frame never fails a run.
            Err(_) => vec![0.0; faces.len()],
        };
        let scale = if ctx.weights.w_v > 0.0 { ctx.params.stretch_weight / ctx.weights.w_v } else { 0.0 };
        raw.iter()
            .map(|&r| {
                let t = scale * r;
                if t.is_finite() { t } else { 0.0 }
            })
            .collect()
    } else {
        vec![0.0; faces.len()]
    };
    let solution =
        match solve::solve(&mesh, &unknowns, &ctx.weights, &targets, ctx.params.max_iterations, control) {
            Ok(Some(solution)) => solution,
            Ok(None) => return Ok(Outcome::Fallback),
            Err(e) if e.kind == ErrorKind::Cancelled => return Err(e),
            Err(_) => return Ok(Outcome::Fallback),
        };

    let nf = part_triangles.len();
    let (mut u, v, theta) = unknowns.unpack(&solution.x, nf);
    // Normalize the size per part: Σ_f A_f e^{2ū_f} = Σ_f A_f (the part has unit area).
    let mean_u = |t: &[u32; 3], u: &[f64]| {
        t.iter().map(|&m| u.get(m as usize).copied().unwrap_or(0.0)).sum::<f64>() / 3.0
    };
    let total_area: f64 = mesh.area.iter().sum();
    let weighted: f64 =
        part_triangles.iter().zip(&mesh.area).map(|(t, a)| a * (2.0 * mean_u(t, &u)).exp()).sum();
    if weighted > 0.0 && weighted.is_finite() && total_area > 0.0 {
        let shift = -0.5 * (weighted / total_area).ln();
        for value in &mut u {
            *value += shift;
        }
    }

    let mut directions = Vec::with_capacity(nf);
    let mut scales = Vec::with_capacity(nf);
    for (f, t) in part_triangles.iter().enumerate() {
        let th = theta.get(f).copied().unwrap_or(0.0);
        let d = part_directions.get(f).copied().unwrap_or(DVec3::X);
        // θ = 0 (held and degenerate faces) keeps the field's direction exactly.
        let dir = if th == 0.0 {
            d
        } else {
            let n = mesh.normal.get(f).copied().unwrap_or(DVec3::ZERO);
            d * th.cos() + n.cross(d) * th.sin()
        };
        directions.push(dir);
        let ub = mean_u(t, &u);
        let vb = t
            .iter()
            .enumerate()
            .map(|(c, &m)| {
                let sign = f64::from(mesh.corner_sign.get(f).map_or(1, |s| s[c]));
                sign * v.get(m as usize).copied().unwrap_or(0.0)
            })
            .sum::<f64>()
            / 3.0;
        scales.push([(ub + vb).exp(), (ub - vb).exp()]);
    }
    Ok(Outcome::Solved(Box::new(PartSolution {
        theta,
        directions,
        u,
        v,
        corner_sign: mesh.corner_sign,
        scales,
        iterations: solution.iterations,
        residual: solution.residual,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::field::{
        CurvatureAlignment, Cx, FaceFrames, FieldParams, ROSY, compute as field_compute, vertex_indices,
    };
    use crate::testmesh;

    /// The frame settings with the stage turned on.
    fn on() -> FrameParams {
        FrameParams { enabled: true, ..FrameParams::default() }
    }

    fn field_of(p: &[DVec3], t: &[[u32; 3]], f: &[[u32; 2]], params: &FieldParams) -> FieldOutput {
        field_compute(p, t, f, params, &Control::default()).unwrap()
    }

    fn powers(p: &[DVec3], t: &[[u32; 3]], directions: &[DVec3]) -> Vec<Cx> {
        let frames = FaceFrames::new(p, t);
        directions.iter().enumerate().map(|(f, d)| frames.power(f as u32, *d, ROSY)).collect()
    }

    #[test]
    fn cube_keeps_feature_alignment() {
        let (p, t, features) = testmesh::cube(4);
        let field = field_of(&p, &t, &features, &FieldParams::default());
        let out = compute(&p, &t, &features, &field, &on(), &Control::default()).unwrap();
        let mesh = setup::FrameMesh::new(&p, &t, &features, &field.directions, &Control::default()).unwrap();
        assert_eq!(out.report.solved_parts, 1);
        for (f, &held) in mesh.held.iter().enumerate() {
            if held {
                assert_eq!(out.theta[f], 0.0, "held face {f} rotated");
                assert_eq!(out.directions[f], field.directions[f], "held face {f} moved");
            }
        }
    }

    #[test]
    fn singularities_are_preserved() {
        let (p, t) = testmesh::icosphere(2);
        let field = field_of(&p, &t, &[], &FieldParams::default());
        let out = compute(&p, &t, &[], &field, &on(), &Control::default()).unwrap();
        assert_eq!(out.report.solved_parts, 1, "{:?}", out.report);
        let before = vertex_indices(&p, &t, &powers(&p, &t, &field.directions));
        let after = vertex_indices(&p, &t, &powers(&p, &t, &out.directions));
        assert_eq!(before, after);
    }

    #[test]
    fn balance_trades_size_for_shape() {
        let (p, t) = testmesh::icosphere(2);
        let aligned = FieldParams { curvature_alignment: CurvatureAlignment::On, ..FieldParams::default() };
        let field = field_of(&p, &t, &[], &aligned);
        let run = |balance: f64| {
            let params = FrameParams { balance, ..on() };
            compute(&p, &t, &[], &field, &params, &Control::default()).unwrap()
        };
        // Balance 0 (no stretch penalty) stalls on the icosphere and falls back
        // (see the report); 0.5 is the default and converges.
        let (keep_shape, keep_size) = (run(1.0), run(0.5));
        assert_eq!(keep_shape.report.solved_parts, 1, "{:?}", keep_shape.report);
        assert_eq!(keep_size.report.solved_parts, 1, "{:?}", keep_size.report);
        // Area-weighted stats of (ū, v̄) from the per-face scales.
        let stats = |out: &FrameOutput| {
            let (mut w, mut abs_v, mut mean_u, mut sq_u) = (0.0, 0.0, 0.0, 0.0);
            for (f, s) in out.scales.iter().enumerate() {
                let a = {
                    let [x, y, z] = t[f].map(|i| p[i as usize]);
                    0.5 * (y - x).cross(z - x).length()
                };
                let (ln_a, ln_b) = (s[0].ln(), s[1].ln());
                let (ub, vb) = (0.5 * (ln_a + ln_b), 0.5 * (ln_a - ln_b));
                w += a;
                abs_v += a * vb.abs();
                mean_u += a * ub;
                sq_u += a * ub * ub;
            }
            let mu = mean_u / w;
            (abs_v / w, sq_u / w - mu * mu)
        };
        let (v_shape, _) = stats(&keep_shape);
        let (_, u_var_size) = stats(&keep_size);
        let (_, u_var_shape) = stats(&keep_shape);
        let (v_size, _) = stats(&keep_size);
        assert!(v_shape < v_size, "mean |v| {v_shape:e} at balance 1, {v_size:e} at balance 0");
        assert!(u_var_size < u_var_shape, "var u {u_var_size:e} at balance 0, {u_var_shape:e} at balance 1");
    }

    #[test]
    fn unconverged_part_falls_back() {
        let (p, t) = testmesh::icosphere(2);
        let field = field_of(&p, &t, &[], &FieldParams::default());
        let params = FrameParams { max_iterations: 1, ..on() };
        let out = compute(&p, &t, &[], &field, &params, &Control::default()).unwrap();
        assert_eq!(out.report.fallback_parts, 1, "{:?}", out.report);
        assert_eq!(out.directions, field.directions);
    }

    #[test]
    fn all_held_part_is_untouched() {
        let p = vec![DVec3::X, DVec3::Y, DVec3::Z];
        let t = vec![[0, 1, 2]];
        let field = field_of(&p, &t, &[], &FieldParams::default());
        let out = compute(&p, &t, &[], &field, &on(), &Control::default()).unwrap();
        assert_eq!(out.report.solved_parts, 0);
        assert_eq!(out.directions, field.directions);
    }

    #[test]
    fn cancel_stops_the_solve() {
        let (p, t) = testmesh::icosphere(2);
        let field = field_of(&p, &t, &[], &FieldParams::default());
        let control = Control::default();
        control.cancel();
        let err = compute(&p, &t, &[], &field, &on(), &control).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Cancelled);
    }

    #[test]
    fn curvature_stretch_elongates_cylinder_quads() {
        let (p, t) = testmesh::cylinder(8, 32);
        let field = field_of(&p, &t, &[], &FieldParams::default());
        let params = FrameParams {
            curvature_stretch: true,
            max_stretch: 3.0,
            balance: 0.5,
            stretch_weight: 1.0,
            ..on()
        };
        let out = compute(&p, &t, &[], &field, &params, &Control::default()).unwrap();
        assert_eq!(out.report.solved_parts, 1, "{:?}", out.report);
        let mut ratios: Vec<f64> = out.scales.iter().map(|s| s[0].max(s[1]) / s[0].min(s[1])).collect();
        ratios.sort_unstable_by(f64::total_cmp);
        let median = ratios[ratios.len() / 2];
        assert!(median >= 1.9, "median stretch {median:.3}");
    }

    #[test]
    fn plain_grid_needs_no_rotation() {
        let (p, t) = testmesh::grid(8, 8);
        let field = field_of(&p, &t, &[], &FieldParams::default());
        let out = compute(&p, &t, &[], &field, &on(), &Control::default()).unwrap();
        assert_eq!(out.report.solved_parts, 1, "{:?}", out.report);
        assert!(out.theta.iter().all(|th| th.abs() < 1e-9));
        assert!(out.scales.iter().all(|s| (s[0] - 1.0).abs() < 1e-9 && (s[1] - 1.0).abs() < 1e-9));
    }

    #[test]
    fn large_part_is_skipped() {
        let (p, t) = testmesh::icosphere(3);
        let field = field_of(&p, &t, &[], &FieldParams::default());
        let params = FrameParams { max_part_faces: 1000, ..on() };
        let out = compute(&p, &t, &[], &field, &params, &Control::default()).unwrap();
        assert_eq!(out.report.skipped_parts, 1, "{:?}", out.report);
        assert_eq!(out.report.solved_parts, 0);
        assert_eq!(out.directions, field.directions);
    }
}
