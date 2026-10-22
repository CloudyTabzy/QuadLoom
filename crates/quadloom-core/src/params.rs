//! User-facing parameters for a pipeline run.

use crate::control::Stage;
use crate::error::Error;
#[cfg(feature = "fault-injection")]
use crate::fault::Fault;
use crate::features::FeatureParams;
use crate::field::FieldParams;
use crate::remesh::RemeshParams;
use crate::repair::RepairParams;
use crate::trace::TraceParams;

/// Largest allowed weld distance, as a fraction of the bbox diagonal. Above
/// this, welding stops being "merge split seams" and starts destroying detail
/// (and degrades to quadratic time as most vertices share a grid cell).
pub const MAX_WELD_EPSILON_REL: f64 = 1e-2;

/// Default refusal threshold: inputs above this many triangles are rejected up front.
pub const DEFAULT_MAX_FACES: usize = 2_000_000;

/// Parameters for [`crate::run`]. M1 only carries safety limits; every later
/// stage adds its own section.
#[derive(Clone, Debug, PartialEq)]
pub struct Params {
    /// Inputs with more triangles than this are refused before any per-face work.
    pub max_faces: usize,
    /// Stage 0: cleanup.
    pub repair: RepairParams,
    /// Stage 1: feature lines.
    pub features: FeatureParams,
    /// Stage 2: isotropic prep remesh.
    pub remesh: RemeshParams,
    /// Stage 3: cross field.
    pub field: FieldParams,
    /// Stage 4: field tracing and patch layout.
    pub trace: TraceParams,
    /// Test-only fault to inject (see [`crate::fault`]).
    #[cfg(feature = "fault-injection")]
    pub fault: Option<Fault>,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            max_faces: DEFAULT_MAX_FACES,
            repair: RepairParams::default(),
            features: FeatureParams::default(),
            remesh: RemeshParams::default(),
            field: FieldParams::default(),
            trace: TraceParams::default(),
            #[cfg(feature = "fault-injection")]
            fault: None,
        }
    }
}

impl Params {
    /// Rejects settings that are out of range, naming the setting.
    pub fn check(&self) -> Result<(), Error> {
        let bad = |message: String| Err(Error::invalid_input(Stage::Validate, message));
        let weld = self.repair.weld_epsilon_rel;
        if !weld.is_finite() || weld > MAX_WELD_EPSILON_REL {
            return bad(format!(
                "repair.weld_epsilon_rel must be at most {MAX_WELD_EPSILON_REL} (got {weld})"
            ));
        }
        let angle = self.features.angle_deg;
        if !(0.0..=180.0).contains(&angle) {
            return bad(format!("features.angle_deg must be between 0 and 180 (got {angle})"));
        }
        let r = &self.remesh;
        if !(0.0..=0.5).contains(&r.target_edge_rel) {
            return bad(format!(
                "remesh.target_edge_rel must be between 0 (auto) and 0.5 (got {})",
                r.target_edge_rel
            ));
        }
        if r.iterations > 100 {
            return bad(format!("remesh.iterations must be at most 100 (got {})", r.iterations));
        }
        if !(r.min_mult > 0.0 && r.min_mult <= 1.0 && r.max_mult >= 1.0 && r.max_mult <= 10.0) {
            return bad(format!(
                "remesh.min_mult must be in (0, 1] and remesh.max_mult in [1, 10] (got {} and {})",
                r.min_mult, r.max_mult
            ));
        }
        if !(r.max_surface_distance_rel > 0.0 && r.max_surface_distance_rel <= 0.1) {
            return bad(format!(
                "remesh.max_surface_distance_rel must be in (0, 0.1] (got {})",
                r.max_surface_distance_rel
            ));
        }
        let f = &self.field;
        if !(0.0..=100.0).contains(&f.alignment_weight) {
            return bad(format!(
                "field.alignment_weight must be between 0 and 100 (got {})",
                f.alignment_weight
            ));
        }
        if !(0.0..=1000.0).contains(&f.sharp_factor) {
            return bad(format!("field.sharp_factor must be between 0 and 1000 (got {})", f.sharp_factor));
        }
        let t = &self.trace;
        if !(4..=6).contains(&t.max_sides) {
            return bad(format!("trace.max_sides must be between 4 and 6 (got {})", t.max_sides));
        }
        if !(0.0..=1000.0).contains(&t.drift) {
            return bad(format!("trace.drift must be between 0 and 1000 (got {})", t.drift));
        }
        if !(0.0..=10.0).contains(&t.cc_tolerance) {
            return bad(format!("trace.cc_tolerance must be between 0 and 10 (got {})", t.cc_tolerance));
        }
        if !(0.001..=0.1).contains(&t.loop_density) {
            return bad(format!("trace.loop_density must be between 0.001 and 0.1 (got {})", t.loop_density));
        }
        if t.sub_passes > 10 {
            return bad(format!("trace.sub_passes must be at most 10 (got {})", t.sub_passes));
        }
        Ok(())
    }
}
