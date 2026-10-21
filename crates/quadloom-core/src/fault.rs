//! Test-only fault injection.
//!
//! With the `fault-injection` feature, [`Params::fault`](crate::Params) can make a
//! stage panic, fail, or stall, so the bindings and the Blender add-on can prove
//! they contain each case. Without the feature, [`inject`] compiles to nothing.

use crate::control::{Control, Stage};
use crate::error::Error;
use crate::params::Params;

/// A deliberate failure to trigger inside a stage.
#[cfg(feature = "fault-injection")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    /// Panic inside the named stage.
    Panic(Stage),
    /// Return an internal error from the named stage.
    Error(Stage),
    /// Stall the validate stage for this many milliseconds, staying cancellable.
    SlowMillis(u32),
}

/// Triggers the configured fault if it targets `stage`.
#[cfg(feature = "fault-injection")]
pub(crate) fn inject(params: &Params, stage: Stage, control: &Control) -> Result<(), Error> {
    use std::time::Duration;

    const STEP_MS: u32 = 10;
    match params.fault {
        // Fault injection exists to exercise panic containment, so this is the
        // one deliberate panic in the crate.
        #[allow(clippy::panic)]
        Some(Fault::Panic(s)) if s == stage => panic!("injected panic in stage {stage}"),
        Some(Fault::Error(s)) if s == stage => {
            Err(Error::internal(stage, format!("injected error in stage {stage}")))
        }
        Some(Fault::SlowMillis(ms)) if stage == Stage::Validate => {
            let steps = ms.div_ceil(STEP_MS).max(1);
            for step in 0..steps {
                control.set_progress(stage, step as f32 / steps as f32);
                control.checkpoint(stage)?;
                std::thread::sleep(Duration::from_millis(u64::from(STEP_MS)));
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

#[cfg(not(feature = "fault-injection"))]
#[inline(always)]
pub(crate) fn inject(_params: &Params, _stage: Stage, _control: &Control) -> Result<(), Error> {
    Ok(())
}
