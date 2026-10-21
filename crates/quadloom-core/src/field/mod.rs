//! Stage 3: a smooth per-face cross field (4-RoSy) on the prep mesh.
//!
//! Complex power representation (Knöppel, Crane, Pinkall & Schröder,
//! *Globally Optimal Direction Fields*, SIGGRAPH 2013) in dual, per-face form:
//! each face stores `z = e^{4iθ}` in its own frame, so the four directions of
//! a cross share one value, and smoothness is a quadratic energy over the
//! faces' shared edges. See `docs/superpowers/plans/2026-10-23-m4-cross-field.md`.

pub mod frames;
pub mod singular;
pub mod solve;

pub use frames::{Cx, FaceFrames};
pub use singular::vertex_indices;
pub use solve::{Field, Hard, ROSY, Soft, smooth_field};
