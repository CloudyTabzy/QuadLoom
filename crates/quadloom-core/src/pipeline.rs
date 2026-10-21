//! The pipeline entry point.

use std::time::Instant;

use crate::control::{Control, Stage};
use crate::error::Error;
use crate::fault;
use crate::features::{self, FeatureInput, FeatureReport};
use crate::mesh::{InputMesh, PolyMesh};
use crate::params::Params;
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
    /// Wall-clock time per stage, in pipeline order.
    pub timings_ms: Vec<(Stage, f64)>,
}

/// Runs the pipeline on flat `xyz` positions and flat triangle indices, with
/// feature hints in the same (input) vertex and triangle ids.
///
/// Currently: validate → repair → features, then the repaired mesh is
/// re-emitted; the remeshing stages slot in before the output.
pub fn run(
    positions: &[f32],
    indices: &[u32],
    feature_input: &FeatureInput,
    params: &Params,
    control: &Control,
) -> Result<RunOutput, Error> {
    control.checkpoint(Stage::Validate)?;
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
    control.checkpoint(Stage::Output)?;
    fault::inject(params, Stage::Output, control)?;
    let face_count = repaired.triangles.len() as u32;
    let mesh = PolyMesh {
        face_indices: repaired.triangles.iter().flatten().copied().collect(),
        face_offsets: (0..=face_count).map(|f| f * 3).collect(),
        positions: repaired.positions,
    };
    control.set_progress(Stage::Output, 1.0);
    timings_ms.push((Stage::Output, elapsed_ms(started)));

    Ok(RunOutput {
        mesh,
        validation,
        repair: repair_report,
        features: feature_report,
        feature_edges: feature_set.edges.iter().map(|e| e.v).collect(),
        components: components.len(),
        timings_ms,
    })
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
