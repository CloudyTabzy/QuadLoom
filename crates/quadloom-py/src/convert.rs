//! Conversions between Python buffers and core types.
//!
//! Inputs are copied out of the Python buffer with `PyBuffer::to_vec`, the safe
//! path: no borrowed views into Python memory outlive the call.

use pyo3::buffer::PyBuffer;
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict};
use quadloom_core::{RunOutput, ValidationReport};

use crate::errors::invalid_argument;

/// Reads `positions` as a buffer of `float32`.
pub fn positions(py: Python<'_>, obj: &Bound<'_, PyAny>) -> PyResult<Vec<f32>> {
    let buffer = PyBuffer::<f32>::get(obj).map_err(|e| {
        invalid_argument(py, format!("positions must be a float32 buffer (e.g. array('f')): {e}"))
    })?;
    buffer.to_vec(py).map_err(|e| invalid_argument(py, format!("positions: {e}")))
}

/// Reads `indices` as a buffer of `uint32`, or `int32` with every value ≥ 0.
pub fn indices(py: Python<'_>, obj: &Bound<'_, PyAny>) -> PyResult<Vec<u32>> {
    if let Ok(buffer) = PyBuffer::<u32>::get(obj) {
        return buffer.to_vec(py).map_err(|e| invalid_argument(py, format!("indices: {e}")));
    }
    let buffer = PyBuffer::<i32>::get(obj).map_err(|e| {
        invalid_argument(py, format!("indices must be a uint32 or int32 buffer (e.g. array('I')): {e}"))
    })?;
    let signed = buffer.to_vec(py).map_err(|e| invalid_argument(py, format!("indices: {e}")))?;
    signed
        .into_iter()
        .enumerate()
        .map(|(n, v)| {
            u32::try_from(v).map_err(|_| invalid_argument(py, format!("indices[{n}] is negative ({v})")))
        })
        .collect()
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
    let timings: Vec<(&str, f64)> = output.timings_ms.iter().map(|(s, ms)| (s.name(), *ms)).collect();
    dict.set_item("timings_ms", timings)?;
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
