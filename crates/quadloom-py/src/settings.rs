//! Strict parsing of the `params` dict: `{"repair": {...}, "features": {...}}`.
//!
//! Every key must name a real setting with the right type. Typos and wrong
//! types fail loudly with the exact key path, instead of being silently ignored.

use pyo3::prelude::*;
use pyo3::types::{PyBool, PyDict, PyString};
use quadloom_core::Params;

use crate::errors::invalid_argument;

/// A mutable reference to one setting, typed.
enum Field<'a> {
    F64(&'a mut f64),
    Bool(&'a mut bool),
    Usize(&'a mut usize),
}

impl Field<'_> {
    fn type_name(&self) -> &'static str {
        match self {
            Self::F64(_) => "finite number",
            Self::Bool(_) => "bool",
            Self::Usize(_) => "non-negative integer",
        }
    }

    fn set(&mut self, value: &Bound<'_, PyAny>) -> bool {
        match self {
            Self::F64(slot) if !value.is_instance_of::<PyBool>() => {
                value.extract().map(|v| **slot = v).is_ok()
            }
            Self::Bool(slot) if value.is_instance_of::<PyBool>() => {
                value.extract().map(|v| **slot = v).is_ok()
            }
            Self::Usize(slot) if !value.is_instance_of::<PyBool>() => {
                value.extract().map(|v| **slot = v).is_ok()
            }
            _ => false,
        }
    }
}

/// Applies a `params` dict on top of `params`.
pub fn apply(py: Python<'_>, params: &mut Params, dict: Option<&Bound<'_, PyDict>>) -> PyResult<()> {
    let Some(dict) = dict else { return Ok(()) };
    for (key, value) in dict.iter() {
        let section = key_str(py, &key, "params")?;
        let value = value
            .cast_into::<PyDict>()
            .map_err(|_| invalid_argument(py, format!("params.{section} must be a dict")))?;
        let r = &mut params.repair;
        let f = &mut params.features;
        let m = &mut params.remesh;
        let mut fields: Vec<(&str, Field<'_>)> = match section.as_str() {
            "repair" => vec![
                ("weld_epsilon_rel", Field::F64(&mut r.weld_epsilon_rel)),
                ("remove_degenerate", Field::Bool(&mut r.remove_degenerate)),
                ("split_non_manifold", Field::Bool(&mut r.split_non_manifold)),
                ("orient", Field::Bool(&mut r.orient)),
                ("min_component_faces", Field::Usize(&mut r.min_component_faces)),
            ],
            "features" => vec![
                ("angle_deg", Field::F64(&mut f.angle_deg)),
                ("use_angle", Field::Bool(&mut f.use_angle)),
                ("use_marked_sharp", Field::Bool(&mut f.use_marked_sharp)),
                ("use_seams", Field::Bool(&mut f.use_seams)),
                ("use_material_boundaries", Field::Bool(&mut f.use_material_boundaries)),
                ("use_face_set_boundaries", Field::Bool(&mut f.use_face_set_boundaries)),
                ("use_open_boundaries", Field::Bool(&mut f.use_open_boundaries)),
                ("prune_steps", Field::Usize(&mut f.prune_steps)),
            ],
            "remesh" => vec![
                ("enabled", Field::Bool(&mut m.enabled)),
                ("target_edge_rel", Field::F64(&mut m.target_edge_rel)),
                ("iterations", Field::Usize(&mut m.iterations)),
                ("adaptive", Field::Bool(&mut m.adaptive)),
                ("min_mult", Field::F64(&mut m.min_mult)),
                ("max_mult", Field::F64(&mut m.max_mult)),
                ("max_surface_distance_rel", Field::F64(&mut m.max_surface_distance_rel)),
            ],
            other => {
                return Err(invalid_argument(
                    py,
                    format!("params.{other} is not a known section (known: repair, features, remesh)"),
                ));
            }
        };
        apply_section(py, &section, &value, &mut fields)?;
    }
    Ok(())
}

fn apply_section(
    py: Python<'_>,
    section: &str,
    dict: &Bound<'_, PyDict>,
    fields: &mut [(&str, Field<'_>)],
) -> PyResult<()> {
    for (key, value) in dict.iter() {
        let key = key_str(py, &key, &format!("params.{section}"))?;
        let Some((_, field)) = fields.iter_mut().find(|(name, _)| *name == key) else {
            let known: Vec<&str> = fields.iter().map(|(name, _)| *name).collect();
            return Err(invalid_argument(
                py,
                format!("params.{section}.{key} is not a known setting (known: {})", known.join(", ")),
            ));
        };
        if !field.set(&value) {
            return Err(invalid_argument(
                py,
                format!("params.{section}.{key} must be a {}", field.type_name()),
            ));
        }
    }
    Ok(())
}

fn key_str(py: Python<'_>, key: &Bound<'_, PyAny>, path: &str) -> PyResult<String> {
    key.cast::<PyString>()
        .map(|s| s.to_string_lossy().into_owned())
        .map_err(|_| invalid_argument(py, format!("{path} keys must be strings")))
}
