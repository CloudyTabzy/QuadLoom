//! Pipeline stages, cancellation and progress reporting.
//!
//! [`Control`] is shared between the thread running the pipeline and whoever
//! started it (Blender's UI thread, the CLI, a test). It's lock-free: one atomic
//! flag for cancellation and one atomic word holding `(stage, fraction)`.

use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::error::Error;

/// A step of the remeshing pipeline. Errors and progress are tagged with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
#[non_exhaustive]
#[repr(u8)]
pub enum Stage {
    Validate,
    Repair,
    Features,
    PrepRemesh,
    Field,
    Trace,
    Layout,
    Quantize,
    Patterns,
    Map,
    Smooth,
    Output,
}

impl Stage {
    /// Every stage, in pipeline order.
    pub const ALL: [Stage; 12] = [
        Self::Validate,
        Self::Repair,
        Self::Features,
        Self::PrepRemesh,
        Self::Field,
        Self::Trace,
        Self::Layout,
        Self::Quantize,
        Self::Patterns,
        Self::Map,
        Self::Smooth,
        Self::Output,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Validate => "validate",
            Self::Repair => "repair",
            Self::Features => "features",
            Self::PrepRemesh => "prep_remesh",
            Self::Field => "field",
            Self::Trace => "trace",
            Self::Layout => "layout",
            Self::Quantize => "quantize",
            Self::Patterns => "patterns",
            Self::Map => "map",
            Self::Smooth => "smooth",
            Self::Output => "output",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.name() == name)
    }

    fn from_index(index: u32) -> Self {
        Self::ALL.get(index as usize).copied().unwrap_or(Self::Validate)
    }
}

/// Fraction resolution stored in the low 24 bits of the progress word.
const FRACTION_SCALE: f32 = 1_000_000.0;
/// How many [`Ticker::tick`] calls pass between cancel checks.
const TICK_INTERVAL: usize = 4096;

/// Cancellation flag plus progress, shared across threads.
#[derive(Debug, Default)]
pub struct Control {
    cancelled: AtomicBool,
    /// `(stage index) << 24 | (fraction * FRACTION_SCALE)`, so a reader never
    /// sees a stage from one update paired with a fraction from another.
    progress: AtomicU32,
}

impl Control {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    /// Records progress. `fraction` is clamped to `0..=1`; NaN counts as 0.
    pub fn set_progress(&self, stage: Stage, fraction: f32) {
        let fraction = if fraction.is_nan() { 0.0 } else { fraction.clamp(0.0, 1.0) };
        let packed = (stage as u32) << 24 | (fraction * FRACTION_SCALE) as u32;
        self.progress.store(packed, Ordering::Relaxed);
    }

    pub fn progress(&self) -> (Stage, f32) {
        let packed = self.progress.load(Ordering::Relaxed);
        let fraction = (packed & 0x00FF_FFFF) as f32 / FRACTION_SCALE;
        (Stage::from_index(packed >> 24), fraction)
    }

    /// Returns `Err(Cancelled)` if cancellation was requested.
    pub fn checkpoint(&self, stage: Stage) -> Result<(), Error> {
        if self.is_cancelled() { Err(Error::cancelled(stage)) } else { Ok(()) }
    }

    /// A cheap per-item progress reporter for hot loops over `total` items.
    pub fn ticker(&self, stage: Stage, total: usize) -> Ticker<'_> {
        self.set_progress(stage, 0.0);
        Ticker { control: self, stage, total: total.max(1), calls: 0 }
    }
}

/// Reports loop progress and checks cancellation every [`TICK_INTERVAL`] calls.
#[derive(Debug)]
pub struct Ticker<'a> {
    control: &'a Control,
    stage: Stage,
    total: usize,
    calls: usize,
}

impl Ticker<'_> {
    /// Call once per item with the number of items done so far.
    pub fn tick(&mut self, done: usize) -> Result<(), Error> {
        self.calls += 1;
        if self.calls % TICK_INTERVAL != 0 {
            return Ok(());
        }
        self.control.set_progress(self.stage, done as f32 / self.total as f32);
        self.control.checkpoint(self.stage)
    }
}

impl fmt::Display for Stage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorKind;

    #[test]
    fn cancel_is_observed_by_checkpoint() {
        let control = Control::default();
        assert!(control.checkpoint(Stage::Validate).is_ok());
        control.cancel();
        assert!(control.is_cancelled());
        let err = control.checkpoint(Stage::Validate).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Cancelled);
        assert_eq!(err.stage, Stage::Validate);
    }

    #[test]
    fn progress_is_clamped() {
        let control = Control::default();
        control.set_progress(Stage::Field, 1.7);
        assert_eq!(control.progress(), (Stage::Field, 1.0));
        control.set_progress(Stage::Trace, -0.2);
        assert_eq!(control.progress(), (Stage::Trace, 0.0));
        control.set_progress(Stage::Smooth, f32::NAN);
        assert_eq!(control.progress(), (Stage::Smooth, 0.0));
        control.set_progress(Stage::Quantize, 0.25);
        let (stage, fraction) = control.progress();
        assert_eq!(stage, Stage::Quantize);
        assert!((fraction - 0.25).abs() < 1e-6);
    }

    #[test]
    fn control_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Control>();
    }

    #[test]
    fn stage_names_are_snake_case() {
        assert_eq!(Stage::PrepRemesh.name(), "prep_remesh");
        assert_eq!(Stage::Validate.to_string(), "validate");
        for stage in Stage::ALL {
            assert_eq!(Stage::from_name(stage.name()), Some(stage));
        }
    }

    #[test]
    fn ticker_reports_cancel_from_another_thread() {
        let control = Control::default();
        let result: Result<(), crate::error::Error> = std::thread::scope(|s| {
            s.spawn(|| {
                std::thread::sleep(std::time::Duration::from_millis(5));
                control.cancel();
            });
            let mut ticker = control.ticker(Stage::Validate, usize::MAX);
            let mut i = 0usize;
            loop {
                ticker.tick(i)?;
                i += 1;
                if i % 4096 == 0 {
                    std::thread::yield_now();
                }
            }
        });
        let err: crate::error::Error = result.unwrap_err();
        assert_eq!(err.kind, ErrorKind::Cancelled);
    }
}
