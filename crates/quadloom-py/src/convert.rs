//! Conversions between Python buffers and core types.
//!
//! Inputs are copied out of the Python buffer with `PyBuffer::to_vec`, the safe
//! path: no borrowed views into Python memory outlive the call.

use pyo3::buffer::{Element, PyUntypedBuffer};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict};
use quadloom_core::features::FeatureReport;
use quadloom_core::{
    FieldOutput, PatchKind, RemeshReport, RepairReport, RunOutput, TraceOutput, ValidationReport,
};

use crate::errors::invalid_argument;

/// Number of elements in a buffer argument, without copying it.
pub fn item_count(py: Python<'_>, obj: &Bound<'_, PyAny>, name: &str) -> PyResult<usize> {
    PyUntypedBuffer::get(obj)
        .map(|buffer| buffer.item_count())
        .map_err(|e| invalid_argument(py, format!("{name} must be a buffer (e.g. array.array): {e}")))
}

/// Copies a buffer of `T` out of Python. Empty buffers are accepted as long as
/// their format matches: an empty `array.array` has no meaningfully aligned
/// data pointer, which PyO3's typed view would otherwise reject.
fn read<T: Element>(py: Python<'_>, obj: &Bound<'_, PyAny>) -> PyResult<Vec<T>> {
    let raw = PyUntypedBuffer::get(obj)?;
    if raw.item_count() == 0 && T::is_compatible_format(raw.format()) {
        return Ok(Vec::new());
    }
    raw.into_typed::<T>()?.to_vec(py)
}

/// Reads `positions` as a buffer of `float32`.
pub fn positions(py: Python<'_>, obj: &Bound<'_, PyAny>) -> PyResult<Vec<f32>> {
    read::<f32>(py, obj).map_err(|e| {
        invalid_argument(py, format!("positions must be a float32 buffer (e.g. array('f')): {e}"))
    })
}

/// Reads an index buffer argument as `uint32`, or `int32` with every value >= 0.
pub fn indices(py: Python<'_>, obj: &Bound<'_, PyAny>, name: &str) -> PyResult<Vec<u32>> {
    if let Ok(unsigned) = read::<u32>(py, obj) {
        return Ok(unsigned);
    }
    let signed = read::<i32>(py, obj).map_err(|e| {
        invalid_argument(py, format!("{name} must be a uint32 or int32 buffer (e.g. array('I')): {e}"))
    })?;
    signed
        .into_iter()
        .enumerate()
        .map(|(n, v)| {
            u32::try_from(v).map_err(|_| invalid_argument(py, format!("{name}[{n}] is negative ({v})")))
        })
        .collect()
}

/// Reads an optional buffer of vertex pairs (`[a0, b0, a1, b1, ...]`).
pub fn edge_pairs(py: Python<'_>, obj: Option<&Bound<'_, PyAny>>, name: &str) -> PyResult<Vec<[u32; 2]>> {
    let Some(obj) = obj else { return Ok(Vec::new()) };
    let flat = indices(py, obj, name)?;
    if flat.len() % 2 != 0 {
        return Err(invalid_argument(
            py,
            format!("{name} must hold vertex pairs (got {} values)", flat.len()),
        ));
    }
    Ok(flat.chunks_exact(2).map(|p| [p[0], p[1]]).collect())
}

/// Reads an optional per-triangle `int32` group buffer (material, face set).
pub fn groups(py: Python<'_>, obj: Option<&Bound<'_, PyAny>>, name: &str) -> PyResult<Option<Vec<i32>>> {
    let Some(obj) = obj else { return Ok(None) };
    read::<i32>(py, obj)
        .map(Some)
        .map_err(|e| invalid_argument(py, format!("{name} must be an int32 buffer (e.g. array('i')): {e}")))
}

/// Packs a run's output into the dict the add-on consumes.
pub fn run_output<'py>(py: Python<'py>, output: &RunOutput) -> PyResult<Bound<'py, PyDict>> {
    let mesh = &output.mesh;
    let positions: Vec<u8> = mesh
        .positions
        .iter()
        .flat_map(|p| [p.x as f32, p.y as f32, p.z as f32])
        .flat_map(f32::to_le_bytes)
        .collect();
    let le_u32 = |values: &[u32]| values.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<u8>>();

    let dict = PyDict::new(py);
    dict.set_item("positions", PyBytes::new(py, &positions))?;
    dict.set_item("face_offsets", PyBytes::new(py, &le_u32(&mesh.face_offsets)))?;
    dict.set_item("face_indices", PyBytes::new(py, &le_u32(&mesh.face_indices)))?;
    dict.set_item("validation", validation(py, &output.validation)?)?;
    dict.set_item("repair", repair(py, &output.repair)?)?;
    dict.set_item("features", features(py, &output.features)?)?;
    dict.set_item("feature_edges", PyBytes::new(py, &le_u32(output.feature_edges.as_flattened())))?;
    dict.set_item("components", output.components)?;
    dict.set_item("remesh", remesh(py, &output.remesh)?)?;
    match &output.field {
        Some(f) => dict.set_item("field", field(py, f)?)?,
        None => dict.set_item("field", py.None())?,
    }
    match &output.trace {
        Some(t) => dict.set_item("trace", trace(py, t)?)?,
        None => dict.set_item("trace", py.None())?,
    }
    let timings: Vec<(&str, f64)> = output.timings_ms.iter().map(|(s, ms)| (s.name(), *ms)).collect();
    dict.set_item("timings_ms", timings)?;
    Ok(dict)
}

/// `directions`: float32 × 3 per triangle; `singular_vertices` (uint32) and
/// `singular_quarters` (int32): each singular vertex and its index × 4.
fn field<'py>(py: Python<'py>, f: &FieldOutput) -> PyResult<Bound<'py, PyDict>> {
    let dict = PyDict::new(py);
    let directions: Vec<u8> = f
        .directions
        .iter()
        .flat_map(|d| [d.x as f32, d.y as f32, d.z as f32])
        .flat_map(f32::to_le_bytes)
        .collect();
    dict.set_item("directions", PyBytes::new(py, &directions))?;
    let vertices: Vec<u8> = f.singularities.iter().flat_map(|&(v, _)| v.to_le_bytes()).collect();
    let quarters: Vec<u8> = f.singularities.iter().flat_map(|&(_, k)| k.to_le_bytes()).collect();
    dict.set_item("singular_vertices", PyBytes::new(py, &vertices))?;
    dict.set_item("singular_quarters", PyBytes::new(py, &quarters))?;
    let report = PyDict::new(py);
    let r = &f.report;
    report.set_item("singularities", r.singularities)?;
    report.set_item("positive_quarters", r.positive_quarters)?;
    report.set_item("negative_quarters", r.negative_quarters)?;
    report.set_item("hard_faces", r.hard_faces)?;
    report.set_item("curvature_aligned", r.curvature_aligned)?;
    report.set_item("fallback_parts", r.fallback_parts)?;
    dict.set_item("report", report)?;
    Ok(dict)
}

/// The patch layout, on its own (refined) copy of the mesh:
/// - `positions` (float32 × 3) and `triangles` (uint32 × 3): that mesh
/// - `patch_of_face` (uint32): one patch id per triangle
/// - `feature_edges` (uint32 pairs): the feature lines on that mesh
/// - `corner_offsets` (uint32, patches + 1) into `corners` (uint32), and
///   `side_lengths` (float64, one per corner: the side starting there)
/// - `kinds` (uint8 per patch, an index into `kind_names`)
/// - `path_offsets` (uint32) into `path_vertices` (uint32)
/// - `report`
fn trace<'py>(py: Python<'py>, t: &TraceOutput) -> PyResult<Bound<'py, PyDict>> {
    let le_u32 =
        |values: &mut dyn Iterator<Item = u32>| values.flat_map(u32::to_le_bytes).collect::<Vec<u8>>();
    let dict = PyDict::new(py);
    let positions: Vec<u8> = t
        .mesh
        .positions
        .iter()
        .flat_map(|p| [p.x as f32, p.y as f32, p.z as f32])
        .flat_map(f32::to_le_bytes)
        .collect();
    dict.set_item("positions", PyBytes::new(py, &positions))?;
    dict.set_item("triangles", PyBytes::new(py, &le_u32(&mut t.mesh.triangles.iter().flatten().copied())))?;
    dict.set_item("patch_of_face", PyBytes::new(py, &le_u32(&mut t.patch_of_face.iter().copied())))?;
    dict.set_item(
        "feature_edges",
        PyBytes::new(py, &le_u32(&mut t.mesh.feature_edges.iter().flatten().copied())),
    )?;
    let mut offsets = vec![0u32];
    for p in &t.patches {
        offsets.push(offsets[offsets.len() - 1] + p.corners.len() as u32);
    }
    dict.set_item("corner_offsets", PyBytes::new(py, &le_u32(&mut offsets.into_iter())))?;
    dict.set_item(
        "corners",
        PyBytes::new(py, &le_u32(&mut t.patches.iter().flat_map(|p| p.corners.iter().copied()))),
    )?;
    let lengths: Vec<u8> =
        t.patches.iter().flat_map(|p| p.sides.iter().map(|s| s.length)).flat_map(f64::to_le_bytes).collect();
    dict.set_item("side_lengths", PyBytes::new(py, &lengths))?;
    let names = [
        PatchKind::NonDisk,
        PatchKind::NotConvex,
        PatchKind::LowSides,
        PatchKind::HighSides,
        PatchKind::NotCcAble,
        PatchKind::ValenceMismatch,
        PatchKind::Ok,
    ];
    let kinds: Vec<u8> =
        t.patches.iter().map(|p| names.iter().position(|&k| k == p.kind).unwrap_or(0) as u8).collect();
    dict.set_item("kinds", PyBytes::new(py, &kinds))?;
    dict.set_item("kind_names", names.iter().map(|k| k.name()).collect::<Vec<_>>())?;
    let mut path_offsets = vec![0u32];
    for path in &t.paths {
        path_offsets.push(path_offsets[path_offsets.len() - 1] + path.len() as u32);
    }
    dict.set_item("path_offsets", PyBytes::new(py, &le_u32(&mut path_offsets.into_iter())))?;
    dict.set_item("path_vertices", PyBytes::new(py, &le_u32(&mut t.paths.iter().flatten().copied())))?;
    let report = PyDict::new(py);
    let r = &t.report;
    for (key, value) in [
        ("parts", r.parts),
        ("patches", r.patches),
        ("non_disk", r.non_disk),
        ("not_convex", r.not_convex),
        ("low_sides", r.low_sides),
        ("high_sides", r.high_sides),
        ("not_cc_able", r.not_cc_able),
        ("valence_mismatch", r.valence_mismatch),
        ("ok", r.ok),
        ("candidates", r.candidates),
        ("kept_paths", r.kept_paths),
        ("removed_paths", r.removed_paths),
        ("sub_passes_run", r.sub_passes_run),
        ("relaxed_patches", r.relaxed_patches),
        ("seams", r.seams),
        ("unsolved", r.unsolved),
    ] {
        report.set_item(key, value)?;
    }
    report.set_item("sides_histogram", r.sides_histogram.to_vec())?;
    dict.set_item("report", report)?;
    Ok(dict)
}

fn validation<'py>(py: Python<'py>, report: &ValidationReport) -> PyResult<Bound<'py, PyDict>> {
    let dict = PyDict::new(py);
    dict.set_item("vertices", report.vertices)?;
    dict.set_item("faces", report.faces)?;
    dict.set_item("boundary_edges", report.boundary_edges)?;
    dict.set_item("non_manifold_edges", report.non_manifold_edges)?;
    dict.set_item("degenerate_faces", report.degenerate_faces)?;
    dict.set_item("components", report.components)?;
    Ok(dict)
}

fn repair<'py>(py: Python<'py>, r: &RepairReport) -> PyResult<Bound<'py, PyDict>> {
    let dict = PyDict::new(py);
    for (key, value) in [
        ("welded_vertices", r.welded_vertices),
        ("removed_repeated", r.removed_repeated),
        ("removed_zero_area", r.removed_zero_area),
        ("removed_duplicates", r.removed_duplicates),
        ("split_non_manifold_edges", r.split_non_manifold_edges),
        ("split_non_manifold_vertices", r.split_non_manifold_vertices),
        ("flipped_faces", r.flipped_faces),
        ("cut_edges", r.cut_edges),
        ("removed_isolated_vertices", r.removed_isolated_vertices),
        ("components", r.components),
        ("skipped_small_components", r.skipped_small_components),
        ("skipped_small_component_faces", r.skipped_small_component_faces),
    ] {
        dict.set_item(key, value)?;
    }
    Ok(dict)
}

fn features<'py>(py: Python<'py>, f: &FeatureReport) -> PyResult<Bound<'py, PyDict>> {
    let dict = PyDict::new(py);
    dict.set_item("edges", f.edges)?;
    dict.set_item("by_source", f.by_source.to_vec())?;
    dict.set_item("pruned_edges", f.pruned_edges)?;
    dict.set_item("corners", f.corners)?;
    dict.set_item("length_ratio", f.length_ratio)?;
    Ok(dict)
}

fn remesh<'py>(py: Python<'py>, r: &RemeshReport) -> PyResult<Bound<'py, PyDict>> {
    let dict = PyDict::new(py);
    dict.set_item("input_faces", r.input_faces)?;
    dict.set_item("output_faces", r.output_faces)?;
    dict.set_item("target_edge_length", r.target_edge_length)?;
    dict.set_item("splits", r.splits)?;
    dict.set_item("collapses", r.collapses)?;
    dict.set_item("flips", r.flips)?;
    dict.set_item("max_surface_distance", r.max_surface_distance)?;
    dict.set_item("min_angle_deg", r.min_angle_deg)?;
    dict.set_item("mean_radius_ratio", r.mean_radius_ratio)?;
    dict.set_item("released_features", r.released_features)?;
    Ok(dict)
}
