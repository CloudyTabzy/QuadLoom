//! The Python exception hierarchy and the mapping from core errors onto it.
//!
//! Every exception is an `Exception` subclass (never `BaseException`), so a
//! plain `except Exception` in the add-on catches all of them, panics included.

use pyo3::exceptions::PyException;
use pyo3::prelude::*;
use quadloom_core::{Error, ErrorKind, Stage};

pyo3::create_exception!(
    quadloom_native,
    QuadLoomError,
    PyException,
    "Base class of every QuadLoom failure. Carries `stage` and `hint` attributes."
);
pyo3::create_exception!(
    quadloom_native,
    InvalidInputError,
    QuadLoomError,
    "The input can't be processed as given; the message says why."
);
pyo3::create_exception!(quadloom_native, CancelledError, QuadLoomError, "The run was cancelled.");
pyo3::create_exception!(
    quadloom_native,
    InternalError,
    QuadLoomError,
    "A bug inside QuadLoom (including contained panics). Please report it."
);

/// Builds the Python exception for a core error, with `stage` and `hint` set.
pub fn to_py_err(py: Python<'_>, error: &Error) -> PyErr {
    let err = match error.kind {
        ErrorKind::InvalidInput => InvalidInputError::new_err(error.to_string()),
        ErrorKind::Cancelled => CancelledError::new_err(error.to_string()),
        ErrorKind::Internal => InternalError::new_err(error.to_string()),
    };
    annotate(py, err, error.stage, error.hint.as_deref())
}

/// An `InvalidInputError` about one argument of a Python call.
pub fn invalid_argument(py: Python<'_>, message: impl Into<String>) -> PyErr {
    let message = format!("{}: {}", Stage::Validate, message.into());
    annotate(py, InvalidInputError::new_err(message), Stage::Validate, None)
}

/// An `InternalError` for a panic caught at the boundary.
pub fn from_panic(py: Python<'_>, payload: &(dyn std::any::Any + Send), stage: Stage) -> PyErr {
    let detail = payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic payload".to_owned());
    let message = format!("{stage}: internal error (please report): {detail}");
    annotate(
        py,
        InternalError::new_err(message),
        stage,
        Some("this is a QuadLoom bug, not a problem with your mesh"),
    )
}

fn annotate(py: Python<'_>, err: PyErr, stage: Stage, hint: Option<&str>) -> PyErr {
    let value = err.value(py);
    // Setting attributes on a fresh exception instance can't fail in practice;
    // if it ever did, the exception is still raised, just without the extras.
    let _ = value.setattr("stage", stage.name());
    let _ = value.setattr("hint", hint);
    err
}
