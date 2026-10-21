//! QuadLoom core: feature-aligned quad remeshing in safe Rust.
//!
//! The crate is a plain library with no Python dependency. Every stage of the
//! pipeline lives in its own module, takes typed parameters, reports progress
//! through [`Control`], and returns typed errors instead of panicking.

#![forbid(unsafe_code)]

pub mod components;
pub mod control;
pub mod error;
pub mod fault;
pub mod mesh;
pub mod metrics;
pub mod params;
pub mod pipeline;
pub mod repair;
pub mod spatial;
mod unionfind;
pub mod validate;

pub use components::Component;
pub use control::{Control, Stage, Ticker};
pub use error::{Error, ErrorKind};
#[cfg(feature = "fault-injection")]
pub use fault::Fault;
pub use mesh::{InputMesh, PolyMesh};
pub use params::Params;
pub use pipeline::{RunOutput, check_limits, run};
pub use repair::{RepairParams, RepairReport, RepairedMesh, repair};
pub use validate::ValidationReport;
