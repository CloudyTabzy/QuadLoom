//! The pipeline entry point.

use std::time::Instant;

use rayon::prelude::*;

use crate::attempts;
use crate::components::Component;
use crate::control::{Control, Stage};
use crate::error::Error;
use crate::fault;
use crate::features::{self, FeatureInput, FeatureReport};
use crate::field::{self, FieldOutput};
use crate::frame::{self, FrameOutput};
use crate::mesh::{InputMesh, PolyMesh};
use crate::params::Params;
use crate::quad::{self, QuadOutput};
use crate::remesh::{self, RemeshParams, RemeshReport};
use crate::repair::RepairedMesh;
use crate::repair::{RepairReport, repair};
use crate::symmetry::{self, SymmetryReport};
use crate::trace::{self, TraceOutput};
use crate::validate::{ValidationReport, validate};

/// Something the user should know about a run that still succeeded.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Warning {
    /// Stage name (see [`Stage::name`]).
    pub stage: &'static str,
    pub message: String,
    /// What the user can do about it.
    pub hint: Option<String>,
}

impl Warning {
    fn new(stage: Stage, message: impl Into<String>, hint: impl Into<String>) -> Self {
        let hint = hint.into();
        Self { stage: stage.name(), message: message.into(), hint: (!hint.is_empty()).then_some(hint) }
    }
}

/// Fallback patterns above this share of patches are worth a warning.
const FALLBACK_WARN_SHARE: f64 = 0.02;
/// A kept side with less than this share of the surface is worth a warning.
const SMALL_SIDE_SHARE: f64 = 0.25;

/// Everything a successful run produces.
#[derive(Clone, Debug)]
pub struct RunOutput {
    pub mesh: PolyMesh,
    pub validation: ValidationReport,
    pub repair: RepairReport,
    pub features: FeatureReport,
    /// Feature edges of `mesh`, as sorted vertex pairs.
    pub feature_edges: Vec<[u32; 2]>,
    /// Connected parts that will be remeshed independently.
    pub components: usize,
    /// Prep remesh totals over all parts (only face counts when disabled).
    pub remesh: RemeshReport,
    /// The cross field on `mesh` (one direction per triangle), if enabled.
    pub field: Option<FieldOutput>,
    /// The rectangular frame, if the frame stage ran (it refines `field`'s directions).
    pub frame: Option<FrameOutput>,
    /// The patch layout, if the field and tracing ran. It lives on its own
    /// copy of `mesh`, refined around singularities.
    pub trace: Option<TraceOutput>,
    /// The quad mesh built from the layout, if the quad stages ran.
    pub quads: Option<QuadOutput>,
    /// Why the quad stages produced nothing, if they failed (the run still
    /// succeeds, with the layout as its result).
    pub quad_error: Option<String>,
    /// What symmetry did, if it was requested. With symmetry the quad mesh
    /// is mirrored; the prep mesh and layout stay one side.
    pub symmetry: Option<SymmetryReport>,
    /// Things worth the user's attention, in pipeline order.
    pub warnings: Vec<Warning>,
    /// Wall-clock time per stage, in pipeline order.
    pub timings_ms: Vec<(Stage, f64)>,
}

/// Runs the pipeline on flat `xyz` positions and flat triangle indices, with
/// feature hints in the same (input) vertex and triangle ids: [`run_upstream`]
/// (validate through the patch layout), then [`finish`] (quads and output).
pub fn run(
    positions: &[f32],
    indices: &[u32],
    feature_input: &FeatureInput,
    params: &Params,
    control: &Control,
) -> Result<RunOutput, Error> {
    let upstream = run_upstream(positions, indices, feature_input, params, control)?;
    finish(upstream, params, control)
}

/// What a run computes before the quad stage. [`finish`] builds the quads
/// from it; a host that keeps a copy can re-run only the quad stage when
/// nothing but `params.quad` changed (a quick re-run).
#[derive(Clone, Debug)]
pub struct Upstream {
    /// The prep-remeshed surface (one side of the symmetry planes, if any).
    positions: Vec<glam::DVec3>,
    triangles: Vec<[u32; 3]>,
    feature_edges: Vec<[u32; 2]>,
    /// Per triangle, its part's edge length over the remesh report's target
    /// (see [`part_targets`]). Empty: every part has the target's.
    part_scale: Vec<f64>,
    validation: ValidationReport,
    repair: RepairReport,
    features: FeatureReport,
    components: usize,
    remesh: RemeshReport,
    field: Option<FieldOutput>,
    frame: Option<FrameOutput>,
    trace: Option<TraceOutput>,
    planes: Vec<symmetry::Plane>,
    symmetry: Option<SymmetryReport>,
    warnings: Vec<Warning>,
    timings_ms: Vec<(Stage, f64)>,
}

impl Upstream {
    /// Drops the stage timings, for a copy reused by a quick re-run (those
    /// stages didn't run again).
    pub fn without_timings(mut self) -> Self {
        self.timings_ms.clear();
        self
    }
}

/// The stages before the quads: validate, repair, symmetry cut, features,
/// prep remesh (per part, in parallel), cross field, frame and patch layout.
pub fn run_upstream(
    positions: &[f32],
    indices: &[u32],
    feature_input: &FeatureInput,
    params: &Params,
    control: &Control,
) -> Result<Upstream, Error> {
    control.checkpoint(Stage::Validate)?;
    params.check()?;
    check_limits(positions.len() / 3, indices.len(), params)?;

    let mut timings_ms = Vec::new();
    let started = Instant::now();
    let mesh = InputMesh::from_buffers(positions, indices)?;
    fault::inject(params, Stage::Validate, control)?;
    let validation = validate(&mesh, control)?;
    check_groups(feature_input, mesh.face_count())?;
    timings_ms.push((Stage::Validate, elapsed_ms(started)));

    let started = Instant::now();
    fault::inject(params, Stage::Repair, control)?;
    let (mut repaired, mut components, repair_report) = repair(&mesh, &params.repair, control)?;
    if repaired.triangles.is_empty() {
        return Err(Error::invalid_input(
            Stage::Repair,
            format!(
                "nothing is left after cleanup ({} small parts with {} faces were skipped, {} degenerate faces removed)",
                repair_report.skipped_small_components,
                repair_report.skipped_small_component_faces,
                repair_report.removed_repeated + repair_report.removed_zero_area + repair_report.removed_duplicates,
            ),
        )
        .with_hint("lower 'Minimum Part Size' under Cleanup, or check the mesh for degenerate geometry"));
    }
    // Everything downstream works on the repaired copy.
    drop(mesh);
    timings_ms.push((Stage::Repair, elapsed_ms(started)));

    let started = Instant::now();
    control.checkpoint(Stage::Features)?;
    fault::inject(params, Stage::Features, control)?;
    // Symmetry: keep one side of the planes; the cut becomes a feature line.
    let mut warnings: Vec<Warning> = Vec::new();
    let (mut planes, mut seam) = (Vec::new(), Vec::new());
    let symmetry_report = params.symmetry.enabled().then(|| {
        let (found, mut report) =
            symmetry::planes(&repaired.positions, &repaired.triangles, &params.symmetry);
        if !found.is_empty() {
            let (share, touches) = symmetry::kept_share(&repaired, &found, params.symmetry.keep_negative);
            report.kept_share = share;
            report.touches_planes = touches;
            match symmetry::cut(&repaired, &found, params.symmetry.keep_negative) {
                Some((half, seam_edges)) => {
                    components = crate::components::split(&half.triangles, half.positions.len());
                    repaired = half;
                    report.seam_edges = seam_edges.len();
                    seam = seam_edges;
                    planes = found;
                }
                None => {
                    report.axes = [false; 3];
                    report.skipped = Some("nothing lies on the kept side of the plane".into());
                }
            }
        }
        symmetry_warnings(&report, &params.symmetry, &mut warnings);
        report
    });
    // Sizes are the whole model's, also when only one side of it is kept.
    let whole = symmetry::Whole::new(&repaired.positions, &planes);
    let (mut feature_set, feature_report) =
        features::detect_in(&repaired, feature_input, &params.features, whole);
    // The seam is always a feature line, whatever the feature settings.
    for &[a, b] in &seam {
        match feature_set.edges.binary_search_by_key(&[a, b], |e| e.v) {
            Ok(i) => feature_set.edges[i].sources |= features::source::BOUNDARY,
            Err(i) => feature_set.edges.insert(
                i,
                features::FeatureEdge { v: [a, b], sources: features::source::BOUNDARY, convex: true },
            ),
        }
    }
    control.set_progress(Stage::Features, 1.0);
    timings_ms.push((Stage::Features, elapsed_ms(started)));

    let started = Instant::now();
    control.checkpoint(Stage::PrepRemesh)?;
    fault::inject(params, Stage::PrepRemesh, control)?;
    let feature_edges: Vec<[u32; 2]> = feature_set.edges.iter().map(|e| e.v).collect();
    let soft_edges: Vec<[u32; 2]> =
        feature_set.edges.iter().filter(|e| e.sources == features::source::ANGLE).map(|e| e.v).collect();
    let component_count = components.len();
    // From here on only the geometry is needed: the repair's vertex and face
    // maps and the feature set are freed now, the geometry once the remesh
    // has its own copies.
    let RepairedMesh {
        positions: clean_positions,
        triangles: clean_triangles,
        vertex_map,
        vertex_copies,
        face_origin,
    } = repaired;
    // (A partial move would keep the rest alive to the end of the run.)
    drop((vertex_map, vertex_copies, face_origin, feature_set));
    let (positions, triangles, feature_edges, remesh_report, part_scale) = if params.remesh.enabled {
        let remeshed = remesh_parts(
            clean_positions,
            clean_triangles,
            &components,
            &feature_edges,
            &soft_edges,
            Side { whole, planes: &planes, seam: &seam },
            params,
            control,
        )?;
        drop((components, feature_edges, soft_edges));
        remeshed
    } else {
        drop((components, soft_edges));
        let faces = clean_triangles.len();
        let report = RemeshReport { input_faces: faces, output_faces: faces, ..RemeshReport::default() };
        (clean_positions, clean_triangles, feature_edges, report, Vec::new())
    };
    timings_ms.push((Stage::PrepRemesh, elapsed_ms(started)));

    let field = if params.field.enabled {
        let started = Instant::now();
        control.checkpoint(Stage::Field)?;
        fault::inject(params, Stage::Field, control)?;
        let field = field::compute(&positions, &triangles, &feature_edges, &params.field, control)?;
        timings_ms.push((Stage::Field, elapsed_ms(started)));
        Some(field)
    } else {
        None
    };

    let frame = match &field {
        Some(field) if params.frame.enabled => {
            let started = Instant::now();
            control.checkpoint(Stage::Frame)?;
            fault::inject(params, Stage::Frame, control)?;
            let computed =
                frame::compute(&positions, &triangles, &feature_edges, field, &params.frame, control)?;
            timings_ms.push((Stage::Frame, elapsed_ms(started)));
            Some(computed)
        }
        _ => None,
    };
    // Tracing follows the frame's directions when it ran. The singularities are
    // the field's: the rotation θ is a 0-form, so they do not change.
    let frame_field = match (&field, &frame) {
        (Some(field), Some(frame)) => Some(FieldOutput {
            directions: frame.directions.clone(),
            singularities: field.singularities.clone(),
            report: field.report.clone(),
        }),
        _ => None,
    };

    let trace = match frame_field.as_ref().or(field.as_ref()) {
        Some(field) if params.trace.enabled => {
            let started = Instant::now();
            control.checkpoint(Stage::Trace)?;
            fault::inject(params, Stage::Trace, control)?;
            let layout =
                trace::compute(&positions, &triangles, &feature_edges, field, &params.trace, control)?;
            timings_ms.push((Stage::Trace, elapsed_ms(started)));
            Some(layout)
        }
        _ => None,
    };

    Ok(Upstream {
        positions,
        triangles,
        feature_edges,
        part_scale,
        validation,
        repair: repair_report,
        features: feature_report,
        components: component_count,
        remesh: remesh_report,
        field,
        frame,
        trace,
        planes,
        symmetry: symmetry_report,
        warnings,
        timings_ms,
    })
}

/// The quad stage, symmetry mirroring and output, from [`run_upstream`]'s
/// result and the quad settings in `params`.
pub fn finish(upstream: Upstream, params: &Params, control: &Control) -> Result<RunOutput, Error> {
    params.check()?;
    // Progress leaves the earlier stages at once, also when they were reused.
    control.set_progress(Stage::Quantize, 0.0);
    let Upstream {
        positions,
        triangles,
        feature_edges,
        part_scale,
        validation,
        repair: repair_report,
        features: feature_report,
        components: component_count,
        remesh: remesh_report,
        field,
        frame,
        trace,
        planes,
        symmetry: symmetry_report,
        mut warnings,
        mut timings_ms,
    } = upstream;
    let mut quad_error = None;
    // The layout of the quads, if other attempts changed it.
    let mut relaid = None;
    let quads = match &trace {
        Some(layout) if params.quad.enabled => {
            let started = Instant::now();
            control.checkpoint(Stage::Quantize)?;
            fault::inject(params, Stage::Quantize, control)?;
            // Quads are sized from the prep remesh's edge length (or the mean edge without it).
            let base = if remesh_report.target_edge_length > 0.0 {
                remesh_report.target_edge_length
            } else {
                mean_edge(&positions, &triangles)
            };
            // The length of a quad edge at size 1: asked for, or what the
            // target count comes to on this surface.
            let unit = if params.quad.target_quads > 0 {
                let area: f64 = triangles
                    .iter()
                    .filter_map(|t| {
                        let [a, b, c] = t.map(|v| positions.get(v as usize).copied());
                        Some(0.5 * (b? - a?).cross(c? - a?).length())
                    })
                    .sum();
                (area * (1usize << planes.len()) as f64 / params.quad.target_quads as f64).sqrt()
            } else {
                base * params.quad.edge_scale
            };
            let mut local = local_size(&positions, &triangles, base, params.quad.adaptivity, &part_scale);
            let thin = quad::turn::cap(&mut local, &positions, &triangles, &feature_edges, unit);
            // All ones is no size field at all.
            let local = (params.quad.adaptivity > 0.0 || !part_scale.is_empty() || thin > 0).then_some(local);
            let sizing =
                quad::Sizing { scales: frame.as_ref().map(|f| f.scales.as_slice()), local: local.as_deref() };
            let built = if params.quad.target_quads > 0 {
                quad::target::compute(layout, sizing, &params.quad, 1usize << planes.len(), control)
            } else {
                quad::compute(layout, sizing, base * params.quad.edge_scale, &params.quad, control)
            };
            timings_ms.push((Stage::Quantize, elapsed_ms(started)));
            // The field the layout follows: the frame's directions when it ran.
            let traced = match (&field, &frame) {
                (Some(field), Some(frame)) => Some(FieldOutput {
                    directions: frame.directions.clone(),
                    singularities: field.singularities.clone(),
                    report: field.report.clone(),
                }),
                _ => None,
            };
            match built {
                // More layouts of each part, and the best quads of each.
                Ok(q) if params.quad.attempts > 1 => {
                    let started = Instant::now();
                    let mirror = 1usize << planes.len();
                    // A run asked for a number of quads asks each later
                    // attempt for what the first gave the parts it tries,
                    // and keeps the first result if the best parts together
                    // miss the number.
                    // The later attempts search for counts on a shorter leash
                    // (see `attempts::LATER_EFFORT`).
                    let later = quad::QuadParams {
                        solver_effort: (params.quad.solver_effort * attempts::LATER_EFFORT).clamp(0.05, 20.0),
                        ..params.quad.clone()
                    };
                    let build = |layout: &TraceOutput, wanted: usize| {
                        if params.quad.target_quads > 0 {
                            let asked = quad::QuadParams { target_quads: wanted * mirror, ..later.clone() };
                            quad::target::compute(layout, sizing, &asked, mirror, control)
                        } else {
                            quad::compute(layout, sizing, base * params.quad.edge_scale, &later, control)
                        }
                    };
                    let fits = |quads: usize| {
                        params.quad.target_quads == 0
                            || quad::target::within_tolerance(params.quad.target_quads, quads * mirror)
                    };
                    match traced.as_ref().or(field.as_ref()) {
                        Some(followed) => {
                            let job = attempts::Job {
                                positions: &positions,
                                triangles: &triangles,
                                feature_edges: &feature_edges,
                                field: followed,
                                trace: &params.trace,
                                build: &build,
                                fits: &fits,
                                edge: unit,
                            };
                            let (layout, mut q, tally) =
                                attempts::improve(&job, layout, q, params.quad.attempts, control)?;
                            q.report.attempts = tally.attempts;
                            q.report.parts_retried = tally.retried;
                            q.report.parts_improved = tally.improved;
                            relaid = layout;
                            timings_ms.push((Stage::Attempts, elapsed_ms(started)));
                            Some(q)
                        }
                        None => Some(q),
                    }
                }
                Ok(q) => Some(q),
                Err(e) if e.kind == crate::error::ErrorKind::Cancelled => return Err(e),
                // Never fail a run over the quads: the layout is still a valid result.
                Err(e) => {
                    warnings.push(Warning::new(
                        Stage::Quantize,
                        format!(
                            "the quads could not be built ({}); the result shows the patch layout",
                            e.message
                        ),
                        "try another Density, or turn Prep Remesh on (Advanced) if it is off",
                    ));
                    quad_error = Some(e.message.clone());
                    None
                }
            }
        }
        _ => None,
    };

    let trace = relaid.or(trace);
    if let Some(q) = &quads {
        let patches = q.report.grid_patches
            + q.report.center_patches
            + q.report.split_patches
            + q.report.bridge_patches
            + q.report.fan_patches
            + q.report.kept_patches;
        let rough = q.report.fan_patches + q.report.kept_patches;
        if patches > 0 && rough as f64 > FALLBACK_WARN_SHARE * patches as f64 {
            warnings.push(Warning::new(
                Stage::Patterns,
                format!("{rough} of {patches} patches used a fallback pattern, which can look uneven"),
                "a higher Density or more Adaptive Detail gives the patterns more room",
            ));
        }
    }
    if params.quad.enabled && params.trace.enabled && params.field.enabled && trace.is_none() {
        warnings.push(Warning::new(Stage::Trace, "no patch layout was made, so no quads either", ""));
    }
    if !planes.is_empty() && quads.is_none() {
        warnings.push(Warning::new(
            Stage::Output,
            "symmetry mirrors the quad mesh, which wasn't built: the result is one side only",
            "turn Quads (and the Cross Field and Patch Layout it needs) on",
        ));
    }
    if let Some(frame) = &frame
        && frame.report.fallback_parts > 0
    {
        warnings.push(Warning::new(
            Stage::Frame,
            format!(
                "Quad Shape kept the regular field on {} part(s): it could not be made rectangular there",
                frame.report.fallback_parts
            ),
            "",
        ));
    }

    // Symmetry: mirror the quad mesh across the planes and weld the seam.
    let quads = match quads {
        Some(q) if !planes.is_empty() => Some(mirror_quads(q, &planes)),
        other => other,
    };
    if let Some(q) = &quads
        && q.report.target_quads > 0
        && !quad::target::within_tolerance(q.report.target_quads, q.report.quads)
    {
        warnings.push(Warning::new(
            Stage::Quantize,
            format!(
                "target requested {} quads; the result has {} ({} bounded sizing trials)",
                q.report.target_quads, q.report.quads, q.report.target_attempts
            ),
            "feature boundaries, fixed patches and integer subdivisions can limit the achievable count",
        ));
    }

    let started = Instant::now();
    control.checkpoint(Stage::Output)?;
    fault::inject(params, Stage::Output, control)?;
    let face_count = triangles.len() as u32;
    let mesh = PolyMesh {
        face_indices: triangles.iter().flatten().copied().collect(),
        face_offsets: (0..=face_count).map(|f| f * 3).collect(),
        positions,
    };
    control.set_progress(Stage::Output, 1.0);
    timings_ms.push((Stage::Output, elapsed_ms(started)));

    Ok(RunOutput {
        mesh,
        validation,
        repair: repair_report,
        features: feature_report,
        feature_edges,
        components: component_count,
        remesh: remesh_report,
        field,
        frame,
        trace,
        quads,
        quad_error,
        symmetry: symmetry_report,
        warnings,
        timings_ms,
    })
}

type Remeshed = (Vec<glam::DVec3>, Vec<[u32; 3]>, Vec<[u32; 2]>, RemeshReport, Vec<f64>);

/// What the mesh being remeshed is one side of: the whole model's scales,
/// the mirror planes, and the seam they cut (sorted vertex pairs). Without
/// Symmetry the whole is the mesh and there are no planes.
#[derive(Clone, Copy, Debug)]
struct Side<'a> {
    whole: symmetry::Whole,
    planes: &'a [symmetry::Plane],
    seam: &'a [[u32; 2]],
}

impl Side<'static> {
    /// A mesh that is all of its model.
    #[cfg(test)]
    fn alone(positions: &[glam::DVec3]) -> Self {
        Self { whole: symmetry::Whole::new(positions, &[]), planes: &[], seam: &[] }
    }
}

/// The edge length each part is remeshed at, and the length the report and
/// the quad size go by.
///
/// With a length the user set, that is one length for all. With the
/// automatic length each part gets its own: the nominal length for the whole
/// object's area, shortened for that part if it is thin and closed
/// ([`remesh::target_length`] has the measure) or if its feature lines lie
/// close together ([`remesh::feature_density`]), whichever asks for more,
/// and never so long that the part is left with a handful of triangles
/// ([`remesh::longest_length`]).
///
/// One length for all parts was the mean of what they wanted, which serves
/// none of them: a pair of glasses with thin closed arms and open lenses got
/// a length between the two, too long for the arms (their quads sheared
/// round a rod a few triangles thick) and shorter than the lenses needed.
/// The length reported is the nominal one, which no part exceeds.
///
/// With Symmetry the mesh is one side of the model, and every measure here
/// is the whole model's ([`Side`]): the area the triangles are shared out
/// over, and each part as the whole part it is half (or a quarter) of.
fn part_targets(
    positions: &[glam::DVec3],
    triangles: &[[u32; 3]],
    components: &[Component],
    feature_edges: &[[u32; 2]],
    side: Side<'_>,
    params: &RemeshParams,
) -> (f64, Vec<f64>) {
    let shared = |target: f64| (target, vec![target; components.len()]);
    // A length the user set is a share of the whole model's diagonal.
    let set = || match side.planes {
        [] => remesh::target_length(positions, triangles, params),
        _ => params.target_edge_rel * side.whole.diagonal,
    };
    if params.target_edge_rel > 0.0 {
        return shared(set());
    }
    // Each part's vertices, to tell which part an edge belongs to.
    let mut part_of = vec![u32::MAX; positions.len()];
    for (part, component) in components.iter().enumerate() {
        for &f in &component.faces {
            for v in triangles.get(f as usize).copied().unwrap_or_default() {
                if let Some(slot) = part_of.get_mut(v as usize) {
                    *slot = part as u32;
                }
            }
        }
    }
    let part_of_edge = |a: u32| part_of.get(a as usize).copied().unwrap_or(u32::MAX) as usize;
    let at = |v: u32| positions.get(v as usize).copied();
    // A part the planes cut is one of several copies that make a whole part:
    // two for each plane its seam lies on.
    let is_seam = |a: u32, b: u32| side.seam.binary_search(&[a.min(b), a.max(b)]).is_ok();
    let mut origin = glam::DVec3::ZERO;
    for plane in side.planes {
        origin[plane.axis] = plane.offset;
    }
    let mut cut_by = vec![0u8; components.len()];
    for &[a, b] in side.seam {
        let (Some(p), Some(q), Some(flags)) = (at(a), at(b), cut_by.get_mut(part_of_edge(a))) else {
            continue;
        };
        for (i, plane) in side.planes.iter().enumerate() {
            // The cut puts its vertices on the plane exactly.
            if p[plane.axis] == plane.offset && q[plane.axis] == plane.offset {
                *flags |= 1 << i;
            }
        }
    }
    let copies: Vec<f64> = cut_by.iter().map(|flags| f64::from(1u32 << flags.count_ones())).collect();
    let measured: Vec<remesh::PartMeasure> = components
        .iter()
        .zip(&copies)
        .map(|(c, &copies)| {
            let faces = c.faces.iter().filter_map(|&f| triangles.get(f as usize).copied());
            remesh::side_measure(positions, faces, origin, copies, is_seam)
        })
        .collect();
    // The whole model's area: every part of this side, once for each copy of the side.
    let area: f64 =
        side.whole.copies * measured.iter().zip(&copies).map(|(m, &copies)| m.area / copies).sum::<f64>();
    if area.is_nan() || area <= 0.0 {
        return shared(set());
    }
    // Each part's feature length: every feature edge belongs to the part its
    // ends are in. The seam is no feature of the model's.
    let mut feature_length = vec![0.0f64; components.len()];
    for &[a, b] in feature_edges {
        if is_seam(a, b) {
            continue;
        }
        let part = part_of_edge(a);
        if let (Some(length), Some(p), Some(q)) = (feature_length.get_mut(part), at(a), at(b)) {
            *length += (p - q).length() * copies.get(part).copied().unwrap_or(1.0);
        }
    }
    let nominal = remesh::nominal_length(area);
    let lengths = measured
        .iter()
        .zip(&feature_length)
        .map(|(part, &features)| {
            let close = remesh::feature_density(part.area, features + part.open_length, nominal);
            let longest = remesh::longest_length(part.area, part.faces, nominal);
            (nominal / part.thin.max(close).sqrt()).min(longest)
        })
        .collect();
    (nominal, lengths)
}

/// Remeshes every part independently (in parallel) at one shared target
/// length, then concatenates the results in part order.
/// Remeshes each part, consuming the cleaned mesh (freed as soon as the parts
/// have their own copies).
#[allow(clippy::too_many_arguments)]
fn remesh_parts(
    positions: Vec<glam::DVec3>,
    triangles: Vec<[u32; 3]>,
    components: &[Component],
    feature_edges: &[[u32; 2]],
    soft_edges: &[[u32; 2]],
    side: Side<'_>,
    params: &Params,
    control: &Control,
) -> Result<Remeshed, Error> {
    let stage = Stage::PrepRemesh;
    let release_angle = if params.features.use_angle { params.features.angle_deg } else { 0.0 };
    let (target, targets) =
        part_targets(&positions, &triangles, components, feature_edges, side, &params.remesh);
    let diagonal = match side.planes {
        [] => remesh::bbox_diagonal(&positions),
        _ => side.whole.diagonal,
    };
    let scale = remesh::Scale {
        target,
        tolerance: diagonal * params.remesh.max_surface_distance_rel,
        face_limit: params.max_faces,
    };
    // Refuse before allocating anything if the targets alone (adaptivity only
    // refines) would exceed the face limit: too much memory would crash the host.
    let expected: f64 = components
        .iter()
        .zip(&targets)
        .map(|(component, &length)| {
            let area: f64 = component
                .faces
                .iter()
                .map(|&f| {
                    let [a, b, c] = triangles[f as usize].map(|v| positions[v as usize]);
                    0.5 * (b - a).cross(c - a).length()
                })
                .sum();
            area / (0.433 * length * length).max(f64::MIN_POSITIVE)
        })
        .sum();
    if expected > params.max_faces as f64 {
        return Err(Error::invalid_input(
            stage,
            format!(
                "the remesh would make about {expected:.0} triangles, above the limit {}",
                params.max_faces
            ),
        )
        .with_hint("raise Triangle Size, or set it to 0 for an automatic size"));
    }
    let whole = components.len() == 1
        && components[0].faces.len() == triangles.len()
        && uses_every_vertex(&triangles, positions.len());
    let results: Vec<Result<(remesh::RemeshedMesh, RemeshReport), Error>> = if whole {
        // One part that is the whole mesh: no copy, and the input goes as
        // soon as the remesh has set up its own.
        let scale = remesh::Scale { target: targets.first().copied().unwrap_or(target), ..scale };
        let prepared = remesh::prepare(
            &positions,
            &triangles,
            feature_edges,
            soft_edges,
            scale,
            &params.remesh,
            control,
        );
        drop((positions, triangles));
        vec![prepared.and_then(|p| p.run(release_angle, &params.remesh, control))]
    } else {
        components
            .par_iter()
            .zip(targets.par_iter())
            .map(|(component, &length)| {
                let scale = remesh::Scale { target: length, ..scale };
                let mut vertices: Vec<u32> =
                    component.faces.iter().flat_map(|&f| triangles[f as usize]).collect();
                vertices.sort_unstable();
                vertices.dedup();
                let local = |v: u32| vertices.binary_search(&v).ok().map(|i| i as u32);
                let part_positions: Vec<glam::DVec3> =
                    vertices.iter().map(|&v| positions[v as usize]).collect();
                let part_triangles: Vec<[u32; 3]> = component
                    .faces
                    .iter()
                    .map(|&f| triangles[f as usize].map(|v| local(v).unwrap_or(0)))
                    .collect();
                let to_local = |edges: &[[u32; 2]]| -> Vec<[u32; 2]> {
                    edges.iter().filter_map(|&[a, b]| Some([local(a)?, local(b)?])).collect()
                };
                let (features, soft) = (to_local(feature_edges), to_local(soft_edges));
                let prepared = remesh::prepare(
                    &part_positions,
                    &part_triangles,
                    &features,
                    &soft,
                    scale,
                    &params.remesh,
                    control,
                );
                drop((vertices, part_positions, part_triangles, features, soft));
                prepared.and_then(|p| p.run(release_angle, &params.remesh, control))
            })
            .collect()
    };

    let (mut positions, mut triangles, mut features) = (Vec::new(), Vec::new(), Vec::new());
    let mut total =
        RemeshReport { target_edge_length: target, min_angle_deg: 180.0, ..RemeshReport::default() };
    let mut ratio_weight = 0.0;
    let mut part_scale: Vec<f64> = Vec::new();
    for (result, &length) in results.into_iter().zip(&targets) {
        let (part, report) = result?;
        part_scale.resize(part_scale.len() + part.triangles.len(), length / target);
        let offset = positions.len() as u32;
        positions.extend(part.positions);
        triangles.extend(part.triangles.iter().map(|t| t.map(|v| v + offset)));
        features.extend(part.feature_edges.iter().map(|e| e.map(|v| v + offset)));
        total.input_faces += report.input_faces;
        total.output_faces += report.output_faces;
        total.splits += report.splits;
        total.collapses += report.collapses;
        total.flips += report.flips;
        total.released_features += report.released_features;
        total.max_surface_distance = total.max_surface_distance.max(report.max_surface_distance);
        total.min_angle_deg = total.min_angle_deg.min(report.min_angle_deg);
        ratio_weight += report.mean_radius_ratio * report.output_faces as f64;
    }
    total.mean_radius_ratio =
        if total.output_faces > 0 { ratio_weight / total.output_faces as f64 } else { 0.0 };
    if total.output_faces > params.max_faces {
        return Err(Error::invalid_input(
            stage,
            format!("the remesh made {} triangles, above the limit {}", total.output_faces, params.max_faces),
        )
        .with_hint("raise Triangle Size or Max Deviation, or Finest"));
    }
    // Nothing to keep if every part has the report's length.
    if part_scale.iter().all(|&scale| scale == 1.0) {
        part_scale = Vec::new();
    }
    Ok((positions, triangles, features, total, part_scale))
}

/// Per-triangle group arrays must match the triangle count exactly.
fn check_groups(input: &FeatureInput, faces: usize) -> Result<(), Error> {
    for (name, groups) in [("material", &input.material), ("face_set", &input.face_set)] {
        if let Some(groups) = groups
            && groups.len() != faces
        {
            return Err(Error::invalid_input(
                Stage::Validate,
                format!("{name} has {} entries but the mesh has {faces} triangles", groups.len()),
            ));
        }
    }
    Ok(())
}

/// Refuses inputs that are too large, using only their sizes, so callers (the
/// Python binding, the add-on) can check *before* copying any data.
///
/// Whether every vertex is a corner of some triangle.
fn uses_every_vertex(triangles: &[[u32; 3]], vertex_count: usize) -> bool {
    let mut used = vec![false; vertex_count];
    for &v in triangles.iter().flatten() {
        if let Some(u) = used.get_mut(v as usize) {
            *u = true;
        }
    }
    used.iter().all(|&u| u)
}

/// The vertex limit is three per allowed face: no mesh within the face limit
/// can use more, so anything beyond that is loose-vertex bloat.
pub fn check_limits(vertex_count: usize, index_count: usize, params: &Params) -> Result<(), Error> {
    let stage = Stage::Validate;
    let hint = "decimate the mesh first, or raise the face limit under Advanced";
    let faces = index_count / 3;
    if faces > params.max_faces {
        return Err(Error::invalid_input(
            stage,
            format!("mesh has {faces} faces, above the limit {}", params.max_faces),
        )
        .with_hint(hint));
    }
    let max_vertices = params.max_faces.saturating_mul(3);
    if vertex_count > max_vertices {
        return Err(Error::invalid_input(
            stage,
            format!("mesh has {vertex_count} vertices, above the limit {max_vertices}"),
        )
        .with_hint("remove loose vertices, or raise the face limit under Advanced"));
    }
    let addressable = u32::MAX as usize;
    if vertex_count > addressable || index_count > addressable {
        return Err(Error::invalid_input(stage, "mesh is too large to index with 32-bit indices"));
    }
    Ok(())
}

/// Warnings about a symmetry request that doesn't fit the mesh.
fn symmetry_warnings(
    report: &SymmetryReport,
    params: &crate::symmetry::SymmetryParams,
    out: &mut Vec<Warning>,
) {
    let stage = Stage::Repair;
    let names = ["X", "Y", "Z"];
    if let Some(reason) = &report.skipped {
        let hint = if reason.contains("kept side") {
            "the mesh lies on the other side: try Keep Negative Side, or move the object's origin to its middle"
        } else if params.auto_detect {
            "the mesh isn't symmetric within the tolerance: raise Tolerance, or pick the axes yourself"
        } else {
            ""
        };
        out.push(Warning::new(stage, format!("symmetry skipped: {reason}"), hint));
        return;
    }
    if !params.auto_detect {
        for (axis, name) in names.iter().enumerate() {
            let a = report.asymmetry[axis];
            let one_sided = report.worst[axis] > params.tolerance.max(1e-6);
            if report.axes[axis] && a <= params.tolerance.max(1e-6) && one_sided {
                out.push(Warning::new(
                    stage,
                    format!(
                        "the mesh has something on one side of {} only (the sides differ by {:.1}% of its size in one place): the other side was replaced by a mirror of this one",
                        name,
                        100.0 * report.worst[axis]
                    ),
                    "if that isn't wanted, turn this axis off, or use Keep Negative Side to keep the other side's",
                ));
            }
            if report.axes[axis] && a > params.tolerance.max(1e-6) {
                out.push(Warning::new(
                    stage,
                    format!(
                        "the mesh isn't symmetric across {} ({:.1}% of its size): the other side was replaced by a mirror of this one",
                        name,
                        100.0 * a
                    ),
                    "if that isn't wanted, turn this axis off, or use Auto Detect",
                ));
            }
        }
    }
    if report.kept_share < SMALL_SIDE_SHARE {
        out.push(Warning::new(
            stage,
            format!("only {:.0}% of the surface is on the kept side of the plane", 100.0 * report.kept_share),
            "Keep Negative Side keeps the larger part",
        ));
    }
    if !report.touches_planes {
        out.push(Warning::new(
            stage,
            "the mesh doesn't reach the mirror plane: the mirrored copy is a separate piece",
            "move the object's origin, or use Auto Detect (it also tries the object's center)",
        ));
    }
}

/// The quad mesh mirrored across `planes`, seam vertices welded.
fn mirror_quads(q: QuadOutput, planes: &[symmetry::Plane]) -> QuadOutput {
    let m = symmetry::mirror(
        &q.mesh.positions,
        &q.mesh.face_offsets,
        &q.mesh.face_indices,
        &q.feature_edges,
        planes,
    );
    let patch_of_face =
        m.source.iter().map(|&f| q.patch_of_face.get(f as usize).copied().unwrap_or(0)).collect();
    let mut report = q.report;
    report.quads = m.offsets.len().saturating_sub(1);
    QuadOutput {
        mesh: PolyMesh { positions: m.positions, face_offsets: m.offsets, face_indices: m.indices },
        patch_of_face,
        feature_edges: m.edges,
        patches: q.patches,
        report,
    }
}

/// Per-face quad-size multiplier: the face's part's own size (`part_scale`,
/// over `base`; empty for 1 everywhere), times what the prep remesh's local
/// detail adds: the mean edge length around the face's corners over its
/// part's size, raised to `adaptivity` (0 = uniform, 1 = follow the remesh fully).
fn local_size(
    positions: &[glam::DVec3],
    triangles: &[[u32; 3]],
    base: f64,
    adaptivity: f64,
    part_scale: &[f64],
) -> Vec<f64> {
    let mut sum = vec![0.0; positions.len()];
    let mut count = vec![0u32; positions.len()];
    for t in triangles {
        for i in 0..3 {
            let (a, b) = (t[i] as usize, t[(i + 1) % 3] as usize);
            if let (Some(&p), Some(&q)) = (positions.get(a), positions.get(b)) {
                let l = (p - q).length();
                for v in [a, b] {
                    sum[v] += l;
                    count[v] += 1;
                }
            }
        }
    }
    let vertex = |v: u32| {
        let v = v as usize;
        if count.get(v).copied().unwrap_or(0) > 0 { sum[v] / count[v] as f64 } else { base }
    };
    triangles
        .iter()
        .enumerate()
        .map(|(f, t)| {
            let part = part_scale.get(f).copied().filter(|s| s.is_finite() && *s > 0.0).unwrap_or(1.0);
            let h = t.iter().map(|&v| vertex(v)).sum::<f64>() / 3.0;
            let own = base * part;
            let ratio = if own > 0.0 && h.is_finite() && h > 0.0 { (h / own).clamp(0.05, 20.0) } else { 1.0 };
            part * ratio.powf(adaptivity)
        })
        .collect()
}

/// Mean edge length of a triangle mesh (each interior edge counted twice).
fn mean_edge(positions: &[glam::DVec3], triangles: &[[u32; 3]]) -> f64 {
    let (mut sum, mut count) = (0.0, 0usize);
    for t in triangles {
        for i in 0..3 {
            let (a, b) = (t[i] as usize, t[(i + 1) % 3] as usize);
            if let (Some(&p), Some(&q)) = (positions.get(a), positions.get(b)) {
                sum += (p - q).length();
                count += 1;
            }
        }
    }
    if count > 0 { sum / count as f64 } else { 0.0 }
}

fn elapsed_ms(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testmesh::torus;

    /// A torus as flat buffers, with the remesh off so only the quad stages run.
    fn torus_run(params: &Params) -> RunOutput {
        let (p, t) = torus(24, 16);
        let positions: Vec<f32> = p.iter().flat_map(|v| [v.x as f32, v.y as f32, v.z as f32]).collect();
        let indices: Vec<u32> = t.iter().flatten().copied().collect();
        run(&positions, &indices, &FeatureInput::default(), params, &Control::default()).unwrap()
    }

    fn no_remesh() -> Params {
        let mut params = Params::default();
        params.remesh.enabled = false;
        params
    }

    fn ran(out: &RunOutput, stage: Stage) -> bool {
        out.timings_ms.iter().any(|(s, _)| *s == stage)
    }

    /// Unit cubes scaled by `scales`, three apart along x, as one mesh.
    fn boxes(scales: &[glam::DVec3]) -> (Vec<glam::DVec3>, Vec<[u32; 3]>) {
        let (cube_p, cube_t, _) = crate::testmesh::cube(1);
        let (mut p, mut t) = (Vec::new(), Vec::new());
        for (k, &scale) in scales.iter().enumerate() {
            let base = p.len() as u32;
            p.extend(cube_p.iter().map(|&v| v * scale + glam::DVec3::new(3.0 * k as f64, 0.0, 0.0)));
            t.extend(cube_t.iter().map(|tri| tri.map(|v| v + base)));
        }
        (p, t)
    }

    #[test]
    fn each_part_is_remeshed_at_its_own_length() {
        // A cube and a sheet a thousand times thinner than wide.
        let (p, t) = boxes(&[glam::DVec3::ONE, glam::DVec3::new(1.0, 1.0, 0.001)]);
        let components = crate::components::split(&t, p.len());
        assert_eq!(components.len(), 2);
        let auto = RemeshParams::default();
        let (base, lengths) = part_targets(&p, &t, &components, &[], Side::alone(&p), &auto);
        // The nominal length for both parts' area; the cube gets it, the
        // sheet a quarter of it (sixteen times the faces, the most).
        let (cube_area, sheet_area) = (6.0, 2.0 + 4.0 * 0.001);
        let nominal = ((cube_area + sheet_area) * 2.309 / 10_000.0f64).sqrt();
        assert!((base - nominal).abs() < 1e-9, "{base} vs {nominal}");
        assert!(
            (lengths[0] - nominal).abs() < 1e-9 && (lengths[1] - nominal / 4.0).abs() < 1e-9,
            "{lengths:?}"
        );
        // One length for both was between the two: too long for the sheet,
        // shorter than the cube needs.
        let one = remesh::target_length(&p, &t, &auto);
        assert!(one < lengths[0] && one > lengths[1], "{one} {lengths:?}");
        // A length the user set is for every part; so is a single part's.
        let set = RemeshParams { target_edge_rel: 0.05, ..RemeshParams::default() };
        let (base, lengths) = part_targets(&p, &t, &components, &[], Side::alone(&p), &set);
        assert!(lengths.iter().all(|&l| l == base), "{base} {lengths:?}");
        // A part on its own gets the nominal length for its own area.
        let (base, lengths) = part_targets(&p, &t, &components[..1], &[], Side::alone(&p), &auto);
        assert_eq!(lengths, [base]);
        assert!((base - (6.0 * 2.309 / 10_000.0f64).sqrt()).abs() < 1e-9, "{base}");
    }

    #[test]
    fn one_side_of_a_mirror_plane_gets_the_lengths_of_the_whole() {
        // The cube and the thin sheet again, whole, and the half of each on
        // one side of the plane y = 0.5.
        let (p, t) = boxes(&[glam::DVec3::ONE, glam::DVec3::new(1.0, 1.0, 0.001)]);
        let components = crate::components::split(&t, p.len());
        let auto = RemeshParams::default();
        let (whole_base, whole_lengths) = part_targets(&p, &t, &components, &[], Side::alone(&p), &auto);
        let mesh = crate::repair::RepairedMesh {
            positions: p.clone(),
            triangles: t.clone(),
            vertex_map: Vec::new(),
            vertex_copies: Vec::new(),
            face_origin: (0..t.len() as u32).collect(),
        };
        let planes = [symmetry::Plane { axis: 1, offset: 0.5 }];
        let (half, seam) = symmetry::cut(&mesh, &planes, false).unwrap();
        let parts = crate::components::split(&half.triangles, half.positions.len());
        assert_eq!(parts.len(), 2);
        let side =
            Side { whole: symmetry::Whole::new(&half.positions, &planes), planes: &planes, seam: &seam };
        let (base, lengths) = part_targets(&half.positions, &half.triangles, &parts, &[], side, &auto);
        assert!((base - whole_base).abs() < 1e-9, "{base} vs {whole_base}");
        assert!(
            lengths.iter().zip(&whole_lengths).all(|(a, b)| (a - b).abs() < 1e-9),
            "{lengths:?} vs {whole_lengths:?}"
        );
        // Taken for all there is, the half got the whole's triangles on half
        // the area: edges 0.7 times as long, and twice the quads once mirrored.
        let (alone, _) =
            part_targets(&half.positions, &half.triangles, &parts, &[], Side::alone(&half.positions), &auto);
        assert!((alone - whole_base * 0.5f64.sqrt()).abs() < 1e-9, "{alone} vs {whole_base}");
        // A length the user set is a share of the whole's size.
        let set = RemeshParams { target_edge_rel: 0.05, ..RemeshParams::default() };
        let (whole_set, _) = part_targets(&p, &t, &components, &[], Side::alone(&p), &set);
        let (half_set, _) = part_targets(&half.positions, &half.triangles, &parts, &[], side, &set);
        assert!((half_set - whole_set).abs() < 1e-12, "{half_set} vs {whole_set}");
    }

    #[test]
    fn a_small_part_of_a_large_object_keeps_enough_triangles() {
        // A cube and another a twentieth its size: 1/400 of the area, which
        // at the nominal length is 25 triangles for a whole cube.
        let (p, t) = boxes(&[glam::DVec3::ONE, glam::DVec3::splat(0.05)]);
        let components = crate::components::split(&t, p.len());
        let (base, lengths) =
            part_targets(&p, &t, &components, &[], Side::alone(&p), &RemeshParams::default());
        assert_eq!(lengths[0], base);
        // It came with 12 triangles: the least, 100, on its 0.015 of area.
        let small = (6.0 * 0.0025 * 2.309 / 100.0f64).sqrt();
        assert!((lengths[1] - small).abs() < 1e-12 && small < 0.6 * base, "{base} {lengths:?}");
        // As many as it came with, up to 400.
        let area = 0.015;
        assert_eq!(remesh::longest_length(area, 250, base), (area * 2.309 / 250.0f64).sqrt());
        assert_eq!(remesh::longest_length(area, 5000, base), (area * 2.309 / 400.0f64).sqrt());
        // A speck gets no shorter than an eighth of the nominal length.
        assert_eq!(remesh::longest_length(1e-9, 5000, base), base / 8.0);
    }

    #[test]
    fn a_part_with_close_feature_lines_gets_shorter_edges() {
        // A unit cube whose twelve edges are features: 12 of feature length
        // on 6 of area, a spacing of half a unit, 13 nominal edges. Nothing to shorten.
        let (p, t, edges) = crate::testmesh::cube(1);
        let components = crate::components::split(&t, p.len());
        let auto = RemeshParams::default();
        let (base, lengths) = part_targets(&p, &t, &components, &edges, Side::alone(&p), &auto);
        assert_eq!(lengths, [base]);
        // The measure itself: spacing 3 nominal edges, nothing; 1.6, 2 / 1.6
        // squared; 0.5, sixteen times by the square, four at the most.
        let near = |a: f64, b: f64| (a - b).abs() < 1e-9;
        assert!(near(remesh::feature_density(3.0, 1.0, 1.0), 1.0));
        assert!(near(remesh::feature_density(1.6, 1.0, 1.0), 1.5625));
        assert!(near(remesh::feature_density(0.5, 1.0, 1.0), 4.0));
        assert!(near(remesh::feature_density(1.0, 0.0, 1.0), 1.0), "no features at all");
        // Open edges count as feature lines: a ribbon 10 long and 0.02 wide,
        // open all round, is 0.2 of area on 20.04 of border, a hundredth
        // apart, which is closer than two nominal edges (0.0136 for its area).
        let p = vec![
            glam::DVec3::ZERO,
            glam::DVec3::new(10.0, 0.0, 0.0),
            glam::DVec3::new(10.0, 0.02, 0.0),
            glam::DVec3::new(0.0, 0.02, 0.0),
        ];
        let t = vec![[0, 1, 2], [0, 2, 3]];
        let components = crate::components::split(&t, p.len());
        let (base, lengths) = part_targets(&p, &t, &components, &[], Side::alone(&p), &auto);
        let spacing = 0.2 / 20.04;
        let expected = (2.0 * base / spacing).powi(2).min(4.0);
        assert!(
            (lengths[0] - base / expected.sqrt()).abs() < 1e-9 && lengths[0] < base,
            "{base} {lengths:?}"
        );
    }

    #[test]
    fn quads_take_their_part_s_size() {
        // Two triangles with edges about 1, the second on a part at a quarter size.
        let p: Vec<glam::DVec3> = [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [5.0, 0.0], [5.25, 0.0], [5.0, 0.25]]
            .iter()
            .map(|&[x, y]| glam::DVec3::new(x, y, 0.0))
            .collect();
        let t = vec![[0, 1, 2], [3, 4, 5]];
        let mean = (2.0 + 2f64.sqrt()) / 3.0;
        // No parts, no adaptivity: nothing local (the caller skips the call), 1 if asked.
        assert_eq!(local_size(&p, &t, mean, 0.0, &[]), [1.0, 1.0]);
        // Parts only: each face its part's size, however fine its triangles.
        assert_eq!(local_size(&p, &t, mean, 0.0, &[1.0, 0.25]), [1.0, 0.25]);
        // With adaptivity, the triangles count against their own part's size:
        // the second part's are just as fine as its size says, so no finer still.
        let both = local_size(&p, &t, mean, 1.0, &[1.0, 0.25]);
        assert!((both[0] - 1.0).abs() < 1e-9 && (both[1] - 0.25).abs() < 1e-9, "{both:?}");
        // Without parts the same triangles are a quarter of the one size.
        let fine = local_size(&p, &t, mean, 1.0, &[]);
        assert!((fine[1] - 0.25).abs() < 1e-9, "{fine:?}");
    }

    #[test]
    fn finishing_a_kept_upstream_matches_a_full_run() {
        // A quick re-run: the upstream kept from one run, finished with other
        // quad settings, must give exactly what a full run with them gives.
        let (p, t) = torus(24, 16);
        let positions: Vec<f32> = p.iter().flat_map(|v| [v.x as f32, v.y as f32, v.z as f32]).collect();
        let indices: Vec<u32> = t.iter().flatten().copied().collect();
        let input = FeatureInput::default();
        let control = Control::default();
        let kept = run_upstream(&positions, &indices, &input, &Params::default(), &control).unwrap();
        for (edge_scale, regularity, smoothing) in [(0.75, 50.0, 20), (0.5, 100.0, 5), (1.2, 25.0, 0)] {
            let mut params = Params::default();
            params.quad.edge_scale = edge_scale;
            params.quad.regularity = regularity;
            params.quad.smoothing = smoothing;
            let quick = finish(kept.clone().without_timings(), &params, &control).unwrap();
            let full = run(&positions, &indices, &input, &params, &control).unwrap();
            assert_eq!(quick.quads, full.quads, "edge_scale {edge_scale}");
            assert_eq!(quick.mesh, full.mesh);
            assert_eq!(quick.warnings, full.warnings);
            assert!(!ran(&quick, Stage::Field) && ran(&quick, Stage::Quantize), "only the quad stage ran");
        }
    }

    #[test]
    fn six_quads_go_round_a_thin_part_at_any_density() {
        // A torus whose tube is 2.5 round, with quads asked 1 long and all
        // one size: two or three would go round the tube. The thin-part
        // rule gives it six, 0.42 long, so about fifteen rings of them:
        // some 90 quads where there were under 40.
        let (p, t) = torus(24, 16);
        let positions: Vec<f32> = p.iter().flat_map(|v| [v.x as f32, v.y as f32, v.z as f32]).collect();
        let indices: Vec<u32> = t.iter().flatten().copied().collect();
        let control = Control::default();
        let kept = run_upstream(&positions, &indices, &FeatureInput::default(), &Params::default(), &control)
            .unwrap();
        let mut params = Params::default();
        params.quad.edge_scale = 1.0 / kept.remesh.target_edge_length;
        params.quad.adaptivity = 0.0;
        let out = finish(kept.clone().without_timings(), &params, &control).unwrap();
        let quads = out.quads.as_ref().expect("quads").report.quads;
        assert!((60..=160).contains(&quads), "{quads} quads");
        // Asked for quads a quarter of that, it has eight round it and is left alone.
        let unit = 0.3;
        let mut local = vec![1.0; kept.triangles.len()];
        assert_eq!(
            quad::turn::cap(&mut local, &kept.positions, &kept.triangles, &kept.feature_edges, unit),
            0
        );
    }

    /// A torus and a smaller one beside it, as flat buffers.
    fn two_tori() -> (Vec<f32>, Vec<u32>) {
        let (mut p, mut t) = torus(24, 16);
        let base = p.len() as u32;
        let (q, u) = torus(20, 12);
        p.extend(q.iter().map(|&v| 0.6 * v + glam::DVec3::new(3.0, 0.0, 0.0)));
        t.extend(u.iter().map(|tri| tri.map(|v| v + base)));
        let positions = p.iter().flat_map(|v| [v.x as f32, v.y as f32, v.z as f32]).collect();
        (positions, t.iter().flatten().copied().collect())
    }

    #[test]
    fn more_attempts_give_a_whole_result_that_is_the_same_on_every_run() {
        let (positions, indices) = two_tori();
        let control = Control::default();
        let kept = run_upstream(&positions, &indices, &FeatureInput::default(), &Params::default(), &control)
            .unwrap();
        let mut params = Params::default();
        params.quad.attempts = 1;
        let once = finish(kept.clone().without_timings(), &params, &control).unwrap();
        assert!(!ran(&once, Stage::Attempts));
        assert_eq!(once.quads.as_ref().expect("quads").report.attempts, 1);

        params.quad.attempts = 4;
        let out = finish(kept.clone().without_timings(), &params, &control).unwrap();
        assert!(ran(&out, Stage::Attempts));
        // Progress is back with the pipeline once the attempts are done.
        assert_eq!(control.progress(), (Stage::Output, 1.0));
        let (quads, layout) = (out.quads.as_ref().expect("quads"), out.trace.as_ref().expect("layout"));
        assert_eq!(quads.report.attempts, 4);
        assert!(quads.report.parts_improved <= quads.report.parts_retried && quads.report.parts_retried <= 2);
        // The quads and the layout they are reported on belong together.
        let faces = quads.mesh.face_offsets.len() - 1;
        assert_eq!(quads.report.quads, faces);
        assert_eq!(quads.patch_of_face.len(), faces);
        assert_eq!(quads.patches.len(), layout.patches.len());
        assert!(quads.patch_of_face.iter().all(|&p| (p as usize) < layout.patches.len()));
        assert_eq!(layout.parts.len(), 2);
        assert!(quads.mesh.face_offsets.windows(2).all(|w| w[1] - w[0] == 4));
        // The attempts run side by side, and the result does not depend on which finished first.
        let again = finish(kept.without_timings(), &params, &control).unwrap();
        assert_eq!(again.quads, out.quads);
        assert_eq!(again.trace.as_ref().map(|t| &t.patches), out.trace.as_ref().map(|t| &t.patches));
    }

    #[test]
    fn a_run_asked_for_a_number_of_quads_still_has_it_after_more_attempts() {
        let (positions, indices) = two_tori();
        let control = Control::default();
        let mut params = Params::default();
        params.quad.target_quads = 1500;
        params.quad.attempts = 3;
        let out = run(&positions, &indices, &FeatureInput::default(), &params, &control).unwrap();
        let report = &out.quads.as_ref().expect("quads").report;
        assert_eq!(report.attempts, 3);
        assert_eq!(report.target_quads, 1500);
        assert!(quad::target::within_tolerance(1500, report.quads), "{report:?}");
    }

    #[test]
    fn the_number_of_attempts_is_range_checked() {
        for attempts in [0, attempts::MOST + 1] {
            let mut params = Params::default();
            params.quad.attempts = attempts;
            let error = params.check().expect_err("out of range");
            assert!(error.message.contains("quad.attempts"), "{}", error.message);
        }
        let mut params = Params::default();
        params.quad.attempts = attempts::MOST;
        assert!(params.check().is_ok());
    }

    #[test]
    fn pipeline_runs_frame_when_enabled() {
        let mut params = no_remesh();
        params.frame.enabled = true;
        let out = torus_run(&params);
        let frame = out.frame.as_ref().expect("the frame stage ran");
        assert_eq!(frame.report.solved_parts, 1, "{:?}", frame.report);
        assert!(ran(&out, Stage::Frame));
        assert!(out.trace.is_some());
    }

    #[test]
    fn pipeline_without_frame_is_unchanged() {
        let base = torus_run(&no_remesh());
        let mut edited = no_remesh();
        edited.frame.balance = 0.9;
        edited.frame.curvature_stretch = true;
        edited.frame.max_stretch = 5.0;
        let other = torus_run(&edited);
        for out in [&base, &other] {
            assert!(out.frame.is_none());
            assert!(!ran(out, Stage::Frame));
        }
        let (a, b) = (base.trace.as_ref().unwrap(), other.trace.as_ref().unwrap());
        assert_eq!(base.mesh, other.mesh);
        assert_eq!(a.patch_of_face, b.patch_of_face);
        assert_eq!(a.paths, b.paths);
        assert_eq!(a.report, b.report);
        assert_eq!(a.source_face, b.source_face);
    }
}
