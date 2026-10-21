//! User-facing parameters for a pipeline run.

use crate::control::Stage;
use crate::error::Error;
#[cfg(feature = "fault-injection")]
use crate::fault::Fault;
use crate::features::FeatureParams;
use crate::repair::RepairParams;

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
        Ok(())
    }
}
