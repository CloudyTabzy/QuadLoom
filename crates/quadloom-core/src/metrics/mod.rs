//! Measurements of remeshing results, used by the benchmark and for regression checks.

pub mod quality;

pub use quality::{MAX_ASPECT, QuadQuality, Stats, quad_quality};
