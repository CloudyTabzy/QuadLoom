//! QuadLoom core: feature-aligned quad remeshing in safe Rust.
//!
//! The crate is a plain library with no Python dependency. Every stage of the
//! pipeline lives in its own module, takes typed parameters, reports progress
//! through [`Control`], and returns typed errors instead of panicking.

#![forbid(unsafe_code)]
