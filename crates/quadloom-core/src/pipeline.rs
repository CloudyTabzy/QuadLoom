//! The pipeline entry point.

use std::time::Instant;

use crate::control::{Control, Stage};
use crate::error::Error;
use crate::fault;
use crate::mesh::{InputMesh, PolyMesh};
use crate::params::Params;
use crate::validate::{ValidationReport, validate};

/// Everything a successful run produces.
#[derive(Clone, Debug)]
pub struct RunOutput {
    pub mesh: PolyMesh,
    pub validation: ValidationReport,
    /// Wall-clock time per stage, in pipeline order.
    pub timings_ms: Vec<(Stage, f64)>,
}

/// Runs the pipeline on flat `xyz` positions and flat triangle indices.
///
/// M1 validates the input and re-emits it as polygons; later milestones insert
/// the remeshing stages between the two.
pub fn run(
    positions: &[f32],
    indices: &[u32],
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
    timings_ms.push((Stage::Validate, elapsed_ms(started)));

    let started = Instant::now();
    control.checkpoint(Stage::Output)?;
    fault::inject(params, Stage::Output, control)?;
    let mesh = PolyMesh::from_triangles(&mesh);
    control.set_progress(Stage::Output, 1.0);
    timings_ms.push((Stage::Output, elapsed_ms(started)));

    Ok(RunOutput { mesh, validation, timings_ms })
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
