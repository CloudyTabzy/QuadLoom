//! QuadLoom core: feature-aligned quad remeshing in safe Rust.
//!
//! The crate is a plain library with no Python dependency. Every stage of the
//! pipeline lives in its own module, takes typed parameters, reports progress
//! through [`Control`], and returns typed errors instead of panicking.

#![forbid(unsafe_code)]

pub mod components;
pub mod control;
pub mod edit;
pub mod error;
pub mod fault;
pub mod features;
pub mod field;
pub mod frame;
pub mod mesh;
pub mod metrics;
pub mod params;
pub mod pipeline;
pub mod quad;
pub mod remesh;
pub mod repair;
pub mod spatial;
pub mod symmetry;
#[cfg(test)]
mod testmesh;
pub mod trace;
mod unionfind;
pub mod validate;

pub use components::Component;
pub use control::{Control, Stage, Ticker};
pub use error::{Error, ErrorKind};
#[cfg(feature = "fault-injection")]
pub use fault::Fault;
pub use field::{CurvatureAlignment, FieldOutput, FieldParams, FieldReport};
pub use frame::{FrameOutput, FrameParams, FrameReport};
pub use mesh::{InputMesh, PolyMesh};
pub use params::Params;
pub use pipeline::{RunOutput, Warning, check_limits, run};
pub use quad::{QuadOutput, QuadParams, QuadReport};
pub use remesh::{RemeshParams, RemeshReport};
pub use repair::{RepairParams, RepairReport, RepairedMesh, repair};
pub use symmetry::{SymmetryParams, SymmetryReport};
pub use trace::{PatchKind, TraceCost, TraceOutput, TraceParams, TraceReport};
pub use validate::ValidationReport;
