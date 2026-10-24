//! The pipeline entry point.

use std::time::Instant;

use rayon::prelude::*;

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
use crate::remesh::{self, RemeshReport};
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
    let (mut feature_set, feature_report) = features::detect(&repaired, feature_input, &params.features);
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
    let (positions, triangles, feature_edges, remesh_report) = if params.remesh.enabled {
        let remeshed = remesh_parts(
            clean_positions,
            clean_triangles,
            &components,
            &feature_edges,
            &soft_edges,
            params,
            control,
        )?;
        drop((components, feature_edges, soft_edges));
        remeshed
    } else {
        drop((components, soft_edges));
        let faces = clean_triangles.len();
        let report = RemeshReport { input_faces: faces, output_faces: faces, ..RemeshReport::default() };
        (clean_positions, clean_triangles, feature_edges, report)
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
            let local = (params.quad.adaptivity > 0.0)
                .then(|| local_size(&positions, &triangles, base, params.quad.adaptivity));
            let sizing =
                quad::Sizing { scales: frame.as_ref().map(|f| f.scales.as_slice()), local: local.as_deref() };
            let built = if params.quad.target_quads > 0 {
                quad::target::compute(layout, sizing, &params.quad, 1usize << planes.len(), control)
            } else {
                quad::compute(layout, sizing, base * params.quad.edge_scale, &params.quad, control)
            };
            timings_ms.push((Stage::Quantize, elapsed_ms(started)));
            match built {
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

    if let Some(q) = &quads {
        let patches = q.report.grid_patches
            + q.report.center_patches
            + q.report.split_patches
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

type Remeshed = (Vec<glam::DVec3>, Vec<[u32; 3]>, Vec<[u32; 2]>, RemeshReport);

/// Remeshes every part independently (in parallel) at one shared target
/// length, then concatenates the results in part order.
/// Remeshes each part, consuming the cleaned mesh (freed as soon as the parts
/// have their own copies).
fn remesh_parts(
    positions: Vec<glam::DVec3>,
    triangles: Vec<[u32; 3]>,
    components: &[Component],
    feature_edges: &[[u32; 2]],
    soft_edges: &[[u32; 2]],
    params: &Params,
    control: &Control,
) -> Result<Remeshed, Error> {
    let stage = Stage::PrepRemesh;
    let release_angle = if params.features.use_angle { params.features.angle_deg } else { 0.0 };
    let target = remesh::target_length(&positions, &triangles, &params.remesh);
    let scale = remesh::Scale {
        target,
        tolerance: remesh::bbox_diagonal(&positions) * params.remesh.max_surface_distance_rel,
        face_limit: params.max_faces,
    };
    // Refuse before allocating anything if the target alone (adaptivity only
    // refines) would exceed the face limit: too much memory would crash the host.
    let area: f64 = triangles
        .iter()
        .map(|t| {
            let [a, b, c] = t.map(|v| positions[v as usize]);
            0.5 * (b - a).cross(c - a).length()
        })
        .sum();
    let expected = area / (0.433 * target * target).max(f64::MIN_POSITIVE);
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
            .map(|component| {
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
    for result in results {
        let (part, report) = result?;
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
    Ok((positions, triangles, features, total))
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
        report,
    }
}

/// Per-face quad-size multiplier from the prep remesh's local detail: the
/// mean edge length around each face's corners over the global size `base`,
/// raised to `adaptivity` (0 = uniform, 1 = follow the remesh fully).
fn local_size(positions: &[glam::DVec3], triangles: &[[u32; 3]], base: f64, adaptivity: f64) -> Vec<f64> {
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
        .map(|t| {
            let h = t.iter().map(|&v| vertex(v)).sum::<f64>() / 3.0;
            let ratio =
                if base > 0.0 && h.is_finite() && h > 0.0 { (h / base).clamp(0.05, 20.0) } else { 1.0 };
            ratio.powf(adaptivity)
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
