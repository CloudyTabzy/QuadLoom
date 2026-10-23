//! `quadloom_native`: the Python face of the QuadLoom core.
//!
//! This crate only converts and contains. Every call copies its inputs out of
//! Python, releases the GIL, runs the core inside `catch_unwind`, and turns any
//! failure (panics included) into a `QuadLoomError` subclass. Nothing from
//! the core can unwind into Blender.

#![forbid(unsafe_code)]

mod convert;
mod errors;
mod settings;

use std::hash::{BuildHasher, Hash, Hasher, RandomState};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Version of the settings and result layout shared with the add-on (`bridge.API_VERSION`).
/// Bump it whenever either side changes.
pub const API_VERSION: u32 = 7;

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

/// One run's work before the quad stage, and what it was computed from.
struct Kept {
    /// Hash of the input buffers and feature hints.
    input: u64,
    /// The run's settings with `quad` reset (only `quad` may differ on reuse).
    params: quadloom_core::Params,
    upstream: quadloom_core::Upstream,
}

struct CacheInner {
    kept: Mutex<Option<Kept>>,
    hasher: RandomState,
    /// Whether the run in progress (or the last one) reuses the kept work.
    reusing: AtomicBool,
}

impl CacheInner {
    fn input_key(
        &self,
        positions: &[f32],
        indices: &[u32],
        features: &quadloom_core::features::FeatureInput,
    ) -> u64 {
        let mut h = self.hasher.build_hasher();
        positions.len().hash(&mut h);
        for x in positions {
            h.write_u32(x.to_bits());
        }
        indices.hash(&mut h);
        features.hash(&mut h);
        h.finish()
    }
}

/// The settings that decide the stages before the quads.
fn upstream_params(params: &quadloom_core::Params) -> quadloom_core::Params {
    quadloom_core::Params { quad: quadloom_core::QuadParams::default(), ..params.clone() }
}

/// Keeps the last run's work before the quad stage (cleanup, features, prep
/// remesh, field, layout). A run given this cache, on the same mesh with the
/// same settings outside `params["quad"]`, starts at the quad stage. Safe to
/// share between threads; holds one run at a time.
#[pyclass(frozen, module = "quadloom_native")]
struct Cache {
    inner: Arc<CacheInner>,
}

#[pymethods]
impl Cache {
    #[new]
    fn new() -> Self {
        Self {
            inner: Arc::new(CacheInner {
                kept: Mutex::new(None),
                hasher: RandomState::new(),
                reusing: AtomicBool::new(false),
            }),
        }
    }

    /// Forgets the kept run (frees its memory).
    fn clear(&self) {
        if let Ok(mut kept) = self.inner.kept.lock() {
            *kept = None;
        }
    }

    /// Whether the run in progress reuses the kept work (meaningful once its
    /// progress has left the first stage; the last run's answer before that).
    fn reusing(&self) -> bool {
        self.inner.reusing.load(Ordering::SeqCst)
    }

    /// Whether a run is kept.
    fn filled(&self) -> bool {
        self.inner.kept.lock().map(|kept| kept.is_some()).unwrap_or(false)
    }
}

/// The pipeline, starting at the quad stage when `cache` holds this input's
/// upstream stages. Returns the output and whether the cache was used.
fn run_cached(
    positions: &[f32],
    indices: &[u32],
    features: &quadloom_core::features::FeatureInput,
    params: &quadloom_core::Params,
    control: &quadloom_core::Control,
    cache: Option<&CacheInner>,
) -> Result<(quadloom_core::RunOutput, bool), quadloom_core::Error> {
    let Some(cache) = cache else {
        return quadloom_core::run(positions, indices, features, params, control).map(|out| (out, false));
    };
    let input = cache.input_key(positions, indices, features);
    let wanted = upstream_params(params);
    let hit = cache.kept.lock().ok().and_then(|mut kept| {
        let reusable = kept.as_ref().is_some_and(|k| k.input == input && k.params == wanted);
        if !reusable {
            // Free the old run before computing a new one.
            *kept = None;
        }
        kept.as_ref().filter(|_| reusable).map(|k| k.upstream.clone())
    });
    // Set before any progress moves on, so a progress reader can rely on it.
    cache.reusing.store(hit.is_some(), Ordering::SeqCst);
    if let Some(upstream) = hit {
        return quadloom_core::finish(upstream.without_timings(), params, control).map(|out| (out, true));
    }
    let upstream = quadloom_core::run_upstream(positions, indices, features, params, control)?;
    if let Ok(mut kept) = cache.kept.lock() {
        *kept = Some(Kept { input, params: wanted, upstream: upstream.clone() });
    }
    quadloom_core::finish(upstream, params, control).map(|out| (out, false))
}

#[pymodule]
mod quadloom_native {
    use super::*;

    #[pymodule_export]
    use crate::errors::{CancelledError, InternalError, InvalidInputError, QuadLoomError};

    #[pymodule_export]
    use super::{Cache, Control};

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
        // The add-on refuses a backend with another API (a stale one Blender kept loaded).
        dict.set_item("api", crate::API_VERSION)?;
        dict.set_item("fault_injection", cfg!(feature = "fault-injection"))?;
        dict.set_item("profile", if cfg!(debug_assertions) { "debug" } else { "release" })?;
        Ok(dict)
    }

    /// Runs the pipeline. See the add-on's `bridge.py` for the result layout.
    #[pyfunction]
    #[pyo3(signature = (
        positions, indices, control, *,
        sharp_edges = None, seam_edges = None, material = None, face_set = None, params = None,
        max_faces = quadloom_core::params::DEFAULT_MAX_FACES, fault = None, cache = None,
    ))]
    #[allow(clippy::too_many_arguments)]
    fn run<'py>(
        py: Python<'py>,
        positions: &Bound<'py, PyAny>,
        indices: &Bound<'py, PyAny>,
        control: &Bound<'py, Control>,
        sharp_edges: Option<&Bound<'py, PyAny>>,
        seam_edges: Option<&Bound<'py, PyAny>>,
        material: Option<&Bound<'py, PyAny>>,
        face_set: Option<&Bound<'py, PyAny>>,
        params: Option<&Bound<'py, PyDict>>,
        max_faces: usize,
        fault: Option<&str>,
        cache: Option<&Bound<'py, Cache>>,
    ) -> PyResult<Bound<'py, PyDict>> {
        let mut core_params = base_params(py, max_faces, fault)?;
        settings::apply(py, &mut core_params, params)?;
        let params = core_params;
        // Refuse oversized inputs from their sizes alone, before copying anything.
        let vertex_floats = convert::item_count(py, positions, "positions")?;
        let index_count = convert::item_count(py, indices, "indices")?;
        quadloom_core::check_limits(vertex_floats / 3, index_count, &params)
            .map_err(|e| errors::to_py_err(py, &e))?;
        let positions = convert::positions(py, positions)?;
        let indices = convert::indices(py, indices, "indices")?;
        let feature_input = quadloom_core::features::FeatureInput {
            sharp_edges: convert::edge_pairs(py, sharp_edges, "sharp_edges")?,
            seam_edges: convert::edge_pairs(py, seam_edges, "seam_edges")?,
            material: convert::groups(py, material, "material")?,
            face_set: convert::groups(py, face_set, "face_set")?,
        };
        let control = Arc::clone(&control.get().inner);
        let cache = cache.map(|c| Arc::clone(&c.get().inner));

        let outcome = py.detach(move || {
            catch_unwind(AssertUnwindSafe(|| {
                run_cached(&positions, &indices, &feature_input, &params, &control, cache.as_deref())
            }))
            .map_err(|payload| (payload, control.progress().0))
        });

        match outcome {
            Ok(Ok((output, reused))) => {
                let dict = convert::run_output(py, &output)?;
                dict.set_item("reused", reused)?;
                Ok(dict)
            }
            Ok(Err(error)) => Err(errors::to_py_err(py, &error)),
            Err((payload, stage)) => Err(errors::from_panic(py, payload.as_ref(), stage)),
        }
    }
}

#[cfg(feature = "fault-injection")]
fn base_params(py: Python<'_>, max_faces: usize, fault: Option<&str>) -> PyResult<quadloom_core::Params> {
    let fault = fault.map(|spec| {
        parse_fault(spec).ok_or_else(|| {
            errors::invalid_argument(
                py,
                format!("unknown fault {spec:?} (expected panic:<stage>, error:<stage> or slow:<ms>)"),
            )
        })
    });
    Ok(quadloom_core::Params { max_faces, fault: fault.transpose()?, ..quadloom_core::Params::default() })
}

#[cfg(not(feature = "fault-injection"))]
fn base_params(py: Python<'_>, max_faces: usize, fault: Option<&str>) -> PyResult<quadloom_core::Params> {
    if fault.is_some() {
        return Err(errors::invalid_argument(py, "fault injection not available in this build"));
    }
    Ok(quadloom_core::Params { max_faces, ..quadloom_core::Params::default() })
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
