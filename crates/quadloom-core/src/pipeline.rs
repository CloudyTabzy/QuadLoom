//! The pipeline entry point.

use std::time::Instant;

use rayon::prelude::*;

use crate::components::Component;
use crate::control::{Control, Stage};
use crate::error::Error;
use crate::fault;
use crate::features::{self, FeatureInput, FeatureReport};
use crate::mesh::{InputMesh, PolyMesh};
use crate::params::Params;
use crate::remesh::{self, RemeshReport};
use crate::repair::RepairedMesh;
use crate::repair::{RepairReport, repair};
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
    let release_angle = if params.features.use_angle { params.features.angle_deg } else { 0.0 };
    let target = remesh::target_length(&repaired.positions, &repaired.triangles, &params.remesh);
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
                target,
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

fn elapsed_ms(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1000.0
}
