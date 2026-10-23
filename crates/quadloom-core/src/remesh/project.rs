//! Projection back onto the input surface and onto its feature lines.

use glam::DVec3;

use crate::spatial::TriangleBvh;

/// Input triangles an oriented query may land on must not face away from the
/// query's orientation: on a shell thinner than an edge the nearest point is
/// often on the other sheet, which faces the other way. Anything stricter
/// (e.g. within 60°) misplaces vertices on wrinkled cloth, whose folds turn
/// sharply between neighbouring triangles.
const MIN_FACING: f64 = 0.0;

/// Closest-point queries against the input surface and its feature segments.
#[derive(Clone, Debug)]
pub struct Projector {
    surface: TriangleBvh,
    /// Unit normal of each input triangle.
    normals: Vec<DVec3>,
    /// Unit normals of the input faces beside each feature segment (one is
    /// zero on a boundary).
    segment_sides: Vec<[DVec3; 2]>,
    /// Feature segments stored as degenerate triangles `(a, b, b)`; the BVH's
    /// closest-point routine handles them as segments.
    features: TriangleBvh,
    /// Largest allowed distance between the remeshed surface and the input
    /// (see `Pass::stays_close`).
    pub max_distance: f64,
    /// How much farther than the nearest input point an oriented query may
    /// land: the other sheet of a thin shell lies within it, an unrelated part
    /// of the model usually doesn't.
    sheet_window: f64,
}

impl Projector {
    pub fn new(
        positions: &[DVec3],
        triangles: &[[u32; 3]],
        feature_edges: &[[u32; 2]],
        max_distance: f64,
        sheet_window: f64,
    ) -> Self {
        let surface = TriangleBvh::new(positions, triangles);
        Self::with_surface(surface, positions, triangles, feature_edges, max_distance, sheet_window)
    }

    /// [`new`](Self::new) around an already built BVH of the same triangles
    /// (the remesher builds it before its edit mesh, so the BVH's build
    /// buffers are gone by the time the edit mesh exists).
    pub fn with_surface(
        surface: TriangleBvh,
        positions: &[DVec3],
        triangles: &[[u32; 3]],
        feature_edges: &[[u32; 2]],
        max_distance: f64,
        sheet_window: f64,
    ) -> Self {
        let segments: Vec<[u32; 3]> = feature_edges.iter().map(|&[a, b]| [a, b, b]).collect();
        let normals: Vec<DVec3> = triangles
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|v| positions[v as usize]);
                (b - a).cross(c - a).normalize_or_zero()
            })
            .collect();
        // The first two faces (in face order) beside each feature segment; only
        // the segments are mapped, not every edge.
        let mut beside: hashbrown::HashMap<(u32, u32), [u32; 2]> =
            feature_edges.iter().map(|&[a, b]| ((a.min(b), a.max(b)), [u32::MAX; 2])).collect();
        for (f, t) in triangles.iter().enumerate() {
            for i in 0..3 {
                let (a, b) = (t[i], t[(i + 1) % 3]);
                if let Some(faces) = beside.get_mut(&(a.min(b), a.max(b)))
                    && let Some(slot) = faces.iter_mut().find(|g| **g == u32::MAX)
                {
                    *slot = f as u32;
                }
            }
        }
        let segment_sides = feature_edges
            .iter()
            .map(|&[a, b]| {
                let faces = beside.get(&(a.min(b), a.max(b))).copied().unwrap_or([u32::MAX; 2]);
                faces.map(|f| normals.get(f as usize).copied().unwrap_or(DVec3::ZERO))
            })
            .collect();
        Self {
            surface,
            normals,
            segment_sides,
            features: TriangleBvh::new(positions, &segments),
            max_distance,
            sheet_window,
        }
    }

    /// The closest point on input triangles facing like `facing`, and its
    /// distance. The nearest point overall wins when `facing` is zero, when
    /// no triangle faces that way, or when those that do are more than
    /// `sheet_window` farther (then they're another part, not this sheet).
    fn closest_facing(&self, p: DVec3, facing: DVec3) -> Option<(DVec3, f64)> {
        let (q, d, f) = self.surface.closest_triangle_where(p, |_| true)?;
        let n = facing.normalize_or_zero();
        let faces_ok = |f: u32| self.normals[f as usize].dot(n) >= MIN_FACING;
        // Usually the nearest triangle already faces the right way: one query.
        if n == DVec3::ZERO || faces_ok(f) {
            return Some((q, d));
        }
        let facing_ok = self.surface.closest_point_where(p, faces_ok);
        Some(facing_ok.filter(|&(_, e)| e <= d + self.sheet_window).unwrap_or((q, d)))
    }

    /// [`onto_surface`](Self::onto_surface) restricted to input triangles
    /// facing like `facing` (the sheet the query belongs to).
    pub fn onto_surface_facing(&self, p: DVec3, facing: DVec3) -> DVec3 {
        self.closest_facing(p, facing).map_or(p, |(q, _)| q)
    }

    /// [`surface_distance`](Self::surface_distance) to the sheet facing like `facing`.
    pub fn surface_distance_facing(&self, p: DVec3, facing: DVec3) -> f64 {
        self.closest_facing(p, facing).map_or(f64::INFINITY, |(_, d)| d)
    }

    /// [`onto_features`](Self::onto_features) restricted to feature segments
    /// with an input face beside them facing like `facing`.
    pub fn onto_features_facing(&self, p: DVec3, facing: DVec3) -> DVec3 {
        let n = facing.normalize_or_zero();
        if n == DVec3::ZERO {
            return self.onto_features(p);
        }
        let Some((nearest, d, s)) = self.features.closest_triangle_where(p, |_| true) else {
            return self.onto_surface(p);
        };
        let sides_ok = |s: u32| self.segment_sides[s as usize].iter().any(|side| side.dot(n) >= MIN_FACING);
        if sides_ok(s) {
            return nearest;
        }
        self.features
            .closest_point_where(p, sides_ok)
            .filter(|&(_, e)| e <= d + self.sheet_window)
            .map_or(nearest, |(q, _)| q)
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
}
