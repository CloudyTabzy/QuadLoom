//! Index-based half-edge topology over a triangle soup.
//!
//! Half-edge `3f + i` runs from `tri[i]` to `tri[(i + 1) % 3]` of face `f`, so
//! `next` and `face` are arithmetic and only twins need storage. Twins are
//! matched by sorting undirected edge keys, which keeps the build deterministic.

macro_rules! id_type {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        #[repr(transparent)]
        pub struct $name(pub u32);

        impl $name {
            pub fn index(self) -> usize {
                self.0 as usize
            }
        }
    };
}

id_type!(
    /// A vertex of the input mesh.
    VertexId
);
id_type!(
    /// A directed edge inside one face.
    HalfEdgeId
);
id_type!(
    /// A triangle of the input mesh.
    FaceId
);

const NO_TWIN: u32 = u32::MAX;

/// Half-edge connectivity for a triangle mesh.
#[derive(Clone, Debug)]
pub struct HalfEdges {
    /// `twin[h]` is the opposite half-edge, or `NO_TWIN` on boundary and non-manifold edges.
    twin: Vec<u32>,
    boundary_edges: usize,
    non_manifold_edges: usize,
}

impl HalfEdges {
    /// Builds connectivity for `triangles`. Edges shared by exactly two faces are
    /// twinned (regardless of their winding, which a later orientation pass fixes);
    /// edges used by one face are boundary; edges used by three or more are
    /// non-manifold and left untwinned.
    pub fn build(triangles: &[[u32; 3]], vertex_count: usize) -> Self {
        debug_assert!(triangles.iter().flatten().all(|&v| (v as usize) < vertex_count));
        // Sized up front: collecting a `flat_map` would grow by doubling.
        let mut keyed: Vec<(u32, u32, u32)> = Vec::with_capacity(3 * triangles.len());
        keyed.extend(triangles.iter().enumerate().flat_map(|(f, t)| {
            (0..3).map(move |i| {
                let (a, b) = (t[i], t[(i + 1) % 3]);
                (a.min(b), a.max(b), (3 * f + i) as u32)
            })
        }));
        keyed.sort_unstable();

        let mut twin = vec![NO_TWIN; keyed.len()];
        let (mut boundary_edges, mut non_manifold_edges) = (0, 0);
        for group in keyed.chunk_by(|x, y| (x.0, x.1) == (y.0, y.1)) {
            match group {
                [_] => boundary_edges += 1,
                [(_, _, h0), (_, _, h1)] => {
                    twin[*h0 as usize] = *h1;
                    twin[*h1 as usize] = *h0;
                }
                _ => non_manifold_edges += 1,
            }
        }
        Self { twin, boundary_edges, non_manifold_edges }
    }

    /// Number of half-edges (three per face).
    pub fn len(&self) -> usize {
        self.twin.len()
    }

    pub fn is_empty(&self) -> bool {
        self.twin.is_empty()
    }

    pub fn twin(&self, h: HalfEdgeId) -> Option<HalfEdgeId> {
        match self.twin[h.index()] {
            NO_TWIN => None,
            t => Some(HalfEdgeId(t)),
        }
    }

    pub fn next(&self, h: HalfEdgeId) -> HalfEdgeId {
        let base = h.0 - h.0 % 3;
        HalfEdgeId(base + (h.0 + 1) % 3)
    }

    pub fn face(&self, h: HalfEdgeId) -> FaceId {
        FaceId(h.0 / 3)
    }

    /// Undirected edges used by exactly one face.
    pub fn boundary_edge_count(&self) -> usize {
        self.boundary_edges
    }

    /// Undirected edges used by three or more faces (each counted once).
    pub fn non_manifold_edge_count(&self) -> usize {
        self.non_manifold_edges
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn tetrahedron() -> Vec<[u32; 3]> {
        vec![[0, 2, 1], [0, 1, 3], [1, 2, 3], [2, 0, 3]]
    }

    fn all_halfedges(he: &HalfEdges) -> impl Iterator<Item = HalfEdgeId> {
        (0..he.len() as u32).map(HalfEdgeId)
    }

    #[test]
    fn closed_tetrahedron_has_no_boundary() {
        let he = HalfEdges::build(&tetrahedron(), 4);
        assert_eq!(he.boundary_edge_count(), 0);
        assert_eq!(he.non_manifold_edge_count(), 0);
        for h in all_halfedges(&he) {
            let t = he.twin(h).expect("closed mesh: every half-edge has a twin");
            assert_eq!(he.twin(t), Some(h));
        }
    }

    #[test]
    fn single_triangle_has_three_boundary_edges() {
        let he = HalfEdges::build(&[[0, 1, 2]], 3);
        assert_eq!(he.boundary_edge_count(), 3);
        assert_eq!(he.non_manifold_edge_count(), 0);
    }

    #[test]
    fn three_triangles_on_one_edge_is_non_manifold() {
        let he = HalfEdges::build(&[[0, 1, 2], [1, 0, 3], [0, 1, 4]], 5);
        assert_eq!(he.non_manifold_edge_count(), 1);
    }

    #[test]
    fn next_cycles_within_face() {
        let he = HalfEdges::build(&tetrahedron(), 4);
        for h in all_halfedges(&he) {
            assert_eq!(he.next(he.next(he.next(h))), h);
            assert_eq!(he.face(he.next(h)), he.face(h));
        }
    }

    fn soup() -> impl Strategy<Value = (usize, Vec<[u32; 3]>)> {
        (3usize..40).prop_flat_map(|n| {
            let tri = (0..n as u32, 0..n as u32, 0..n as u32).prop_map(|(a, b, c)| [a, b, c]);
            (Just(n), proptest::collection::vec(tri, 1..60))
        })
    }

    proptest! {
        #[test]
        fn proptest_twin_is_involution((n, tris) in soup()) {
            let he = HalfEdges::build(&tris, n);
            for h in all_halfedges(&he) {
                if let Some(t) = he.twin(h) {
                    prop_assert_eq!(he.twin(t), Some(h));
                }
            }
        }
    }
}
