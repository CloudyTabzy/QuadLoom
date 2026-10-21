//! User-facing parameters for a pipeline run.

#[cfg(feature = "fault-injection")]
use crate::fault::Fault;

/// Default refusal threshold: inputs above this many triangles are rejected up front.
pub const DEFAULT_MAX_FACES: usize = 2_000_000;

/// Parameters for [`crate::run`]. M1 only carries safety limits; every later
/// stage adds its own section.
#[derive(Clone, Debug, PartialEq)]
pub struct Params {
    /// Inputs with more triangles than this are refused before any per-face work.
    pub max_faces: usize,
    /// Test-only fault to inject (see [`crate::fault`]).
    #[cfg(feature = "fault-injection")]
    pub fault: Option<Fault>,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            max_faces: DEFAULT_MAX_FACES,
            #[cfg(feature = "fault-injection")]
            fault: None,
        }
    }
}
