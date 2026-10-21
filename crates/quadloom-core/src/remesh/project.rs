//! Projection back onto the input surface and onto its feature lines.

use glam::DVec3;

use crate::spatial::TriangleBvh;

/// Closest-point queries against the input surface and its feature segments.
#[derive(Clone, Debug)]
pub struct Projector {
    surface: TriangleBvh,
    /// Feature segments stored as degenerate triangles `(a, b, b)`; the BVH's
    /// closest-point routine handles them as segments.
    features: TriangleBvh,
    /// Largest allowed distance from the input surface.
    pub max_distance: f64,
}

impl Projector {
    pub fn new(
        positions: &[DVec3],
        triangles: &[[u32; 3]],
        feature_edges: &[[u32; 2]],
        max_distance: f64,
    ) -> Self {
        let segments: Vec<[u32; 3]> = feature_edges.iter().map(|&[a, b]| [a, b, b]).collect();
        Self {
            surface: TriangleBvh::new(positions, triangles),
            features: TriangleBvh::new(positions, &segments),
            max_distance,
        }
    }

    pub fn onto_surface(&self, p: DVec3) -> DVec3 {
        self.surface.closest_point(p).map_or(p, |(q, _)| q)
    }

    /// The closest point on any input feature line (or the surface, if there are none).
    pub fn onto_features(&self, p: DVec3) -> DVec3 {
        self.features.closest_point(p).map_or_else(|| self.onto_surface(p), |(q, _)| q)
    }

    pub fn surface_distance(&self, p: DVec3) -> f64 {
        self.surface.closest_point(p).map_or(f64::INFINITY, |(_, d)| d)
    }

    pub fn within_bound(&self, p: DVec3) -> bool {
        self.surface_distance(p) <= self.max_distance
    }
}
