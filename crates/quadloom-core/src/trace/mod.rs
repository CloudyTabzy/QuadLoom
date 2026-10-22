//! Stage 4: field tracing and patch layout.
//!
//! The prep mesh is cut along its feature lines, and field-aligned paths are
//! traced on a graph of (vertex, field direction) nodes until every patch is
//! a disk with 3 to `max_sides` convex corners (Pietroni et al., *Reliable
//! Feature-Line Driven Quad-Remeshing*, SIGGRAPH 2021, §5). See
//! `docs/superpowers/plans/2026-10-24-m5-trace-layout.md`.

pub mod cut;
pub mod graph;
pub mod patches;
pub mod refine;
pub mod search;
pub mod turns;

use glam::DVec3;

/// How a path's cost grows as it drifts from the field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum TraceCost {
    /// Anisotropic length √((d·f)² + drift²·(d·f⊥)²) of each arc's net
    /// displacement d against the field f (Campen, Heistermann & Kobbelt,
    /// *Practical Anisotropic Geodesy*, 2013).
    Anisotropic,
    /// The oracle's ℓ·(1 + drift·(α/45°)²), α the arc's angle to the field.
    AnglePenalty,
}

impl TraceCost {
    pub fn name(self) -> &'static str {
        match self {
            Self::Anisotropic => "anisotropic",
            Self::AnglePenalty => "angle_penalty",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        [Self::Anisotropic, Self::AnglePenalty].into_iter().find(|c| c.name() == name)
    }
}

/// The prep mesh and its field, as tracing sees them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TraceMesh {
    pub positions: Vec<DVec3>,
    pub triangles: Vec<[u32; 3]>,
    /// Sorted vertex pairs.
    pub feature_edges: Vec<[u32; 2]>,
    /// One unit direction per triangle.
    pub directions: Vec<DVec3>,
    /// `(vertex, index in quarters)`, by vertex id.
    pub singularities: Vec<(u32, i32)>,
}
