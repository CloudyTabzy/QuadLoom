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
use crate::trace::{self, TraceOutput};
use crate::validate::{ValidationReport, validate};

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
    /// Wall-clock time per stage, in pipeline order.
    pub timings_ms: Vec<(Stage, f64)>,
}

/// Runs the pipeline on flat `xyz` positions and flat triangle indices, with
/// feature hints in the same (input) vertex and triangle ids.
///
/// Currently: validate → repair → features → prep remesh (per part, in
/// parallel), then the remeshed triangles are re-emitted; the quad stages slot
/// in before the output.
pub fn run(
    positions: &[f32],
    indices: &[u32],
    feature_input: &FeatureInput,
    params: &Params,
    control: &Control,
) -> Result<RunOutput, Error> {
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
    let (repaired, components, repair_report) = repair(&mesh, &params.repair, control)?;
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
    timings_ms.push((Stage::Repair, elapsed_ms(started)));

    let started = Instant::now();
    control.checkpoint(Stage::Features)?;
    fault::inject(params, Stage::Features, control)?;
    let (feature_set, feature_report) = features::detect(&repaired, feature_input, &params.features);
    control.set_progress(Stage::Features, 1.0);
    timings_ms.push((Stage::Features, elapsed_ms(started)));

    let started = Instant::now();
    control.checkpoint(Stage::PrepRemesh)?;
    fault::inject(params, Stage::PrepRemesh, control)?;
    let feature_edges: Vec<[u32; 2]> = feature_set.edges.iter().map(|e| e.v).collect();
    let soft_edges: Vec<[u32; 2]> =
        feature_set.edges.iter().filter(|e| e.sources == features::source::ANGLE).map(|e| e.v).collect();
    let (positions, triangles, feature_edges, remesh_report) = if params.remesh.enabled {
        remesh_parts(&repaired, &components, &feature_edges, &soft_edges, params, control)?
    } else {
        let faces = repaired.triangles.len();
        let report = RemeshReport { input_faces: faces, output_faces: faces, ..RemeshReport::default() };
        (repaired.positions, repaired.triangles, feature_edges, report)
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
            let scales = frame.as_ref().map(|f| f.scales.as_slice());
            let built = quad::compute(layout, scales, base * params.quad.edge_scale, &params.quad, control)?;
            timings_ms.push((Stage::Quantize, elapsed_ms(started)));
            Some(built)
        }
        _ => None,
    };

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
        components: components.len(),
        remesh: remesh_report,
        field,
        frame,
        trace,
        quads,
        timings_ms,
    })
}

type Remeshed = (Vec<glam::DVec3>, Vec<[u32; 3]>, Vec<[u32; 2]>, RemeshReport);

/// Remeshes every part independently (in parallel) at one shared target
/// length, then concatenates the results in part order.
fn remesh_parts(
    repaired: &RepairedMesh,
    components: &[Component],
    feature_edges: &[[u32; 2]],
    soft_edges: &[[u32; 2]],
    params: &Params,
    control: &Control,
) -> Result<Remeshed, Error> {
    let stage = Stage::PrepRemesh;
    let release_angle = if params.features.use_angle { params.features.angle_deg } else { 0.0 };
    let target = remesh::target_length(&repaired.positions, &repaired.triangles, &params.remesh);
    let scale = remesh::Scale {
        target,
        tolerance: remesh::bbox_diagonal(&repaired.positions) * params.remesh.max_surface_distance_rel,
        face_limit: params.max_faces,
    };
    // Refuse before allocating anything if the target alone (adaptivity only
    // refines) would exceed the face limit: too much memory would crash the host.
    let area: f64 = repaired
        .triangles
        .iter()
        .map(|t| {
            let [a, b, c] = t.map(|v| repaired.positions[v as usize]);
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
    let results: Vec<Result<(remesh::RemeshedMesh, RemeshReport), Error>> = components
        .par_iter()
        .map(|component| {
            let mut vertices: Vec<u32> =
                component.faces.iter().flat_map(|&f| repaired.triangles[f as usize]).collect();
            vertices.sort_unstable();
            vertices.dedup();
            let local = |v: u32| vertices.binary_search(&v).ok().map(|i| i as u32);
            let positions: Vec<glam::DVec3> =
                vertices.iter().map(|&v| repaired.positions[v as usize]).collect();
            let triangles: Vec<[u32; 3]> = component
                .faces
                .iter()
                .map(|&f| repaired.triangles[f as usize].map(|v| local(v).unwrap_or(0)))
                .collect();
            let to_local = |edges: &[[u32; 2]]| -> Vec<[u32; 2]> {
                edges.iter().filter_map(|&[a, b]| Some([local(a)?, local(b)?])).collect()
            };
            let (features, soft) = (to_local(feature_edges), to_local(soft_edges));
            remesh::remesh(
                &positions,
                &triangles,
                &features,
                &soft,
                release_angle,
                scale,
                &params.remesh,
                control,
            )
        })
        .collect();

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
