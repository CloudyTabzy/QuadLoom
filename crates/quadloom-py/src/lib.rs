//! `quadloom_native`: the Python face of the QuadLoom core.
//!
//! This crate only converts and contains. Every call copies its inputs out of
//! Python, releases the GIL, runs the core inside `catch_unwind`, and turns any
//! failure (panics included) into a `QuadLoomError` subclass. Nothing from
//! the core can unwind into Blender.

#![forbid(unsafe_code)]

mod convert;
mod errors;

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use pyo3::prelude::*;
use pyo3::types::PyDict;

/// Shared cancel flag and progress for one run. Safe to use from any thread.
#[pyclass(frozen, module = "quadloom_native")]
struct Control {
    inner: Arc<quadloom_core::Control>,
}

#[pymethods]
impl Control {
    #[new]
    fn new() -> Self {
        Self { inner: Arc::default() }
    }

    /// Requests cancellation; the run stops at its next checkpoint.
    fn cancel(&self) {
        self.inner.cancel();
    }

    fn is_cancelled(&self) -> bool {
        self.inner.is_cancelled()
    }

    /// `(stage_name, fraction)` of the most recent progress update.
    fn progress(&self) -> (&'static str, f32) {
        let (stage, fraction) = self.inner.progress();
        (stage.name(), fraction)
    }
}

#[pymodule]
mod quadloom_native {
    use super::*;

    #[pymodule_export]
    use crate::errors::{CancelledError, InternalError, InvalidInputError, QuadLoomError};

    #[pymodule_export]
    use super::Control;

    /// The crate version, e.g. `"0.1.0"`.
    #[pyfunction]
    fn version() -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    /// Build details, for the add-on's diagnostics box and bug reports.
    #[pyfunction]
    fn build_info(py: Python<'_>) -> PyResult<Bound<'_, PyDict>> {
        let dict = PyDict::new(py);
        dict.set_item("version", env!("CARGO_PKG_VERSION"))?;
        dict.set_item("fault_injection", cfg!(feature = "fault-injection"))?;
        dict.set_item("profile", if cfg!(debug_assertions) { "debug" } else { "release" })?;
        Ok(dict)
    }

    /// Runs the pipeline. See the add-on's `bridge.py` for the result layout.
    #[pyfunction]
    #[pyo3(signature = (positions, indices, control, *, max_faces = quadloom_core::params::DEFAULT_MAX_FACES, fault = None))]
    fn run<'py>(
        py: Python<'py>,
        positions: &Bound<'py, PyAny>,
        indices: &Bound<'py, PyAny>,
        control: &Bound<'py, Control>,
        max_faces: usize,
        fault: Option<&str>,
    ) -> PyResult<Bound<'py, PyDict>> {
        let positions = convert::positions(py, positions)?;
        let indices = convert::indices(py, indices)?;
        let params = params(py, max_faces, fault)?;
        let control = Arc::clone(&control.get().inner);

        let outcome = py.detach(move || {
            catch_unwind(AssertUnwindSafe(|| {
                quadloom_core::run(&positions, &indices, &params, &control)
            }))
            .map_err(|payload| (payload, control.progress().0))
        });

        match outcome {
            Ok(Ok(output)) => convert::run_output(py, &output),
            Ok(Err(error)) => Err(errors::to_py_err(py, &error)),
            Err((payload, stage)) => Err(errors::from_panic(py, payload.as_ref(), stage)),
        }
    }
}

#[cfg(feature = "fault-injection")]
fn params(py: Python<'_>, max_faces: usize, fault: Option<&str>) -> PyResult<quadloom_core::Params> {
    let fault = fault.map(|spec| parse_fault(spec).ok_or_else(|| {
        errors::invalid_argument(py, format!("unknown fault {spec:?} (expected panic:<stage>, error:<stage> or slow:<ms>)"))
    }));
    Ok(quadloom_core::Params { max_faces, fault: fault.transpose()? })
}

#[cfg(not(feature = "fault-injection"))]
fn params(py: Python<'_>, max_faces: usize, fault: Option<&str>) -> PyResult<quadloom_core::Params> {
    if fault.is_some() {
        return Err(errors::invalid_argument(py, "fault injection not available in this build"));
    }
    Ok(quadloom_core::Params { max_faces })
}

#[cfg(feature = "fault-injection")]
fn parse_fault(spec: &str) -> Option<quadloom_core::Fault> {
    use quadloom_core::{Fault, Stage};
    let (kind, arg) = spec.split_once(':')?;
    match kind {
        "panic" => Stage::from_name(arg).map(Fault::Panic),
        "error" => Stage::from_name(arg).map(Fault::Error),
        "slow" => arg.parse().ok().map(Fault::SlowMillis),
        _ => None,
    }
}
