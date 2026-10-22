//! The mesh cut open along its feature lines.
//!
//! Every barrier (feature) edge becomes a boundary edge: a vertex gets one
//! copy per fan sector between barrier edges, so each original feature edge
//! leaves two boundary edges, one per side. Faces keep their order; only
//! their vertex ids change. Corner `h = 3f + i` (vertex `i` of face `f`) is
//! also the half-edge leaving that vertex in `f`.

use hashbrown::HashSet;

use glam::DVec3;

use crate::mesh::halfedge::{HalfEdgeId, HalfEdges};
use crate::unionfind::UnionFind;

const NONE: u32 = u32::MAX;

#[derive(Clone, Debug)]
pub struct CutMesh {
    /// Per cut vertex.
    pub positions: Vec<DVec3>,
    /// Same faces as the input, in cut vertex ids.
    pub triangles: Vec<[u32; 3]>,
    /// Cut vertex → input vertex.
    pub original: Vec<u32>,
    /// The other copies of each cut vertex's input vertex.
    pub twins: Vec<Vec<u32>>,
    /// Whether a cut vertex lies on a boundary (mesh border or cut).
    pub boundary: Vec<bool>,
    /// Opposite half-edge, or `NONE` on boundary and barrier edges.
    twin: Vec<u32>,
    /// Each cut vertex's outgoing half-edges, counter-clockwise; a boundary
    /// vertex's fan starts at its outgoing boundary half-edge.
    fan_offsets: Vec<u32>,
    fans: Vec<u32>,
}

impl CutMesh {
    /// Cuts `triangles` (a manifold mesh) along `barrier` edges.
    pub fn new(positions: &[DVec3], triangles: &[[u32; 3]], barrier: &[[u32; 2]]) -> Self {
        let barrier: HashSet<(u32, u32)> = barrier.iter().map(|&[a, b]| (a.min(b), a.max(b))).collect();
        let he = HalfEdges::build(triangles, positions.len());
        let tail = |h: u32| triangles[(h / 3) as usize][(h % 3) as usize];
        let next = |h: u32| h - h % 3 + (h + 1) % 3;
        let mut twin = vec![NONE; 3 * triangles.len()];
        let mut sectors = UnionFind::new(3 * triangles.len());
        for h in 0..twin.len() as u32 {
            let (a, b) = (tail(h), tail(next(h)));
            if barrier.contains(&(a.min(b), a.max(b))) {
                continue;
            }
            if let Some(t) = he.twin(HalfEdgeId(h)) {
                twin[h as usize] = t.0;
                // Corners at a: h and the corner after t; at b: next(h) and t.
                sectors.union(h, next(t.0));
                sectors.union(next(h), t.0);
            }
        }
        // The lowest-corner sector of each vertex keeps its id; others are
        // appended in corner order.
        let mut cut_positions = positions.to_vec();
        let mut original: Vec<u32> = (0..positions.len() as u32).collect();
        let mut id_of_root = vec![NONE; twin.len()];
        let mut seen = vec![false; positions.len()];
        let mut corner_id = vec![NONE; twin.len()];
        for h in 0..twin.len() as u32 {
            let root = sectors.find(h) as usize;
            if id_of_root[root] == NONE {
                let v = tail(h);
                id_of_root[root] = if seen[v as usize] {
                    cut_positions.push(positions[v as usize]);
                    original.push(v);
                    (cut_positions.len() - 1) as u32
                } else {
                    seen[v as usize] = true;
                    v
                };
            }
            corner_id[h as usize] = id_of_root[root];
        }
        let cut_triangles: Vec<[u32; 3]> =
            (0..triangles.len()).map(|f| [0, 1, 2].map(|i| corner_id[3 * f + i])).collect();
        let n = cut_positions.len();
        let mut copies: Vec<Vec<u32>> = vec![Vec::new(); positions.len()];
        for (v, &o) in original.iter().enumerate() {
            copies[o as usize].push(v as u32);
        }
        let twins = original
            .iter()
            .enumerate()
            .map(|(v, &o)| copies[o as usize].iter().copied().filter(|&c| c != v as u32).collect())
            .collect();

        // Fans: walk clockwise to the outgoing boundary half-edge (if any),
        // then collect counter-clockwise.
        let mut first_corner = vec![NONE; n];
        for (h, &v) in corner_id.iter().enumerate() {
            if first_corner[v as usize] == NONE {
                first_corner[v as usize] = h as u32;
            }
        }
        let prev = |h: u32| h - h % 3 + (h + 2) % 3;
        let mut boundary = vec![false; n];
        let (mut fan_offsets, mut fans) = (Vec::with_capacity(n + 1), Vec::with_capacity(twin.len()));
        fan_offsets.push(0);
        for v in 0..n {
            let h0 = first_corner[v];
            if h0 != NONE {
                // Clockwise from h is next(twin(h)); stop at a boundary or back at h0.
                let (mut start, mut closed) = (h0, false);
                for _ in 0..twin.len() {
                    let t = twin[start as usize];
                    if t == NONE {
                        break;
                    }
                    start = next(t);
                    if start == h0 {
                        closed = true;
                        break;
                    }
                }
                let ring_start = fans.len();
                let mut h = start;
                for _ in 0..twin.len() {
                    fans.push(h);
                    let t = twin[prev(h) as usize];
                    if t == NONE || t == start {
                        break;
                    }
                    h = t;
                }
                if closed {
                    // Interior: start at the lowest corner, for determinism.
                    let ring = &mut fans[ring_start..];
                    if let Some(k) = ring.iter().enumerate().min_by_key(|&(_, &c)| c).map(|(k, _)| k) {
                        ring.rotate_left(k);
                    }
                } else {
                    boundary[v] = true;
                }
            }
            fan_offsets.push(fans.len() as u32);
        }
        Self {
            positions: cut_positions,
            triangles: cut_triangles,
            original,
            twins,
            boundary,
            twin,
            fan_offsets,
            fans,
        }
    }

    pub fn vertex_count(&self) -> usize {
        self.positions.len()
    }

    pub fn face_count(&self) -> usize {
        self.triangles.len()
    }

    /// The vertex half-edge `h` leaves.
    pub fn tail(&self, h: u32) -> u32 {
        self.triangles[(h / 3) as usize][(h % 3) as usize]
    }

    /// The vertex half-edge `h` arrives at.
    pub fn head(&self, h: u32) -> u32 {
        self.tail(Self::next(h))
    }

    pub fn next(h: u32) -> u32 {
        h - h % 3 + (h + 1) % 3
    }

    pub fn prev(h: u32) -> u32 {
        h - h % 3 + (h + 2) % 3
    }

    /// The opposite half-edge, or None on boundary (and cut) edges.
    pub fn twin(&self, h: u32) -> Option<u32> {
        match self.twin[h as usize] {
            NONE => None,
            t => Some(t),
        }
    }

    /// `v`'s outgoing half-edges, counter-clockwise (from its outgoing
    /// boundary half-edge when `v` is on a boundary).
    pub fn fan(&self, v: u32) -> &[u32] {
        let (a, b) = (self.fan_offsets[v as usize] as usize, self.fan_offsets[v as usize + 1] as usize);
        &self.fans[a..b]
    }

    /// `v`'s neighbours counter-clockwise (for a boundary vertex, from the
    /// end of its outgoing boundary edge to the start of its incoming one).
    pub fn neighbours(&self, v: u32) -> Vec<u32> {
        let fan = self.fan(v);
        let mut ring: Vec<u32> = fan.iter().map(|&h| self.head(h)).collect();
        if self.boundary[v as usize] {
            if let Some(&last) = fan.last() {
                ring.push(self.tail(Self::prev(last)));
            }
        }
        ring
    }

    /// The half-edge `a → b`, if `a` and `b` share an edge (either way round
    /// a boundary edge counts only as `a → b` in its own face).
    pub fn half_edge(&self, a: u32, b: u32) -> Option<u32> {
        self.fan(a).iter().copied().find(|&h| self.head(h) == b)
    }

    /// Whether `a`–`b` is an edge with a face on both sides.
    pub fn is_interior_edge(&self, a: u32, b: u32) -> bool {
        self.half_edge(a, b).is_some_and(|h| self.twin(h).is_some())
    }

    /// Boundary half-edges, ascending; each leaves one boundary vertex.
    pub fn boundary_half_edges(&self) -> impl Iterator<Item = u32> + '_ {
        (0..self.twin.len() as u32).filter(|&h| self.twin[h as usize] == NONE)
    }

    /// The boundary half-edge after `h` along its loop (the patch on the left).
    pub fn next_on_boundary(&self, h: u32) -> u32 {
        let fan = self.fan(self.head(h));
        fan.first().copied().unwrap_or(h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testmesh::{cube, grid};

    fn loops(cut: &CutMesh) -> usize {
        let mut seen = vec![false; 3 * cut.face_count()];
        let mut count = 0;
        for h in cut.boundary_half_edges() {
            if seen[h as usize] {
                continue;
            }
            count += 1;
            let mut e = h;
            while !seen[e as usize] {
                seen[e as usize] = true;
                e = cut.next_on_boundary(e);
            }
        }
        count
    }

    #[test]
    fn cut_duplicates_vertices_along_features() {
        let (p, t, features) = cube(4);
        let cut = CutMesh::new(&p, &t, &features);
        let mut copies = vec![0; p.len()];
        for &o in &cut.original {
            copies[o as usize] += 1;
        }
        let on_edge =
            |q: DVec3| q.to_array().iter().filter(|&&c| c.abs() < 1e-9 || (c - 1.0).abs() < 1e-9).count();
        for (v, q) in p.iter().enumerate() {
            let expected = match on_edge(*q) {
                3 => 3,
                2 => 2,
                _ => 1,
            };
            assert_eq!(copies[v], expected, "vertex {v} at {q}");
        }
        assert_eq!(loops(&cut), 6);
        for (v, &o) in cut.original.iter().enumerate() {
            assert_eq!(cut.positions[v], p[o as usize]);
            assert_eq!(cut.boundary[v], on_edge(p[o as usize]) >= 2);
        }
    }

    #[test]
    fn uncut_mesh_keeps_its_ids() {
        let (p, t) = grid(3, 3);
        let cut = CutMesh::new(&p, &t, &[]);
        assert_eq!(cut.triangles, t);
        assert_eq!(cut.vertex_count(), p.len());
        assert_eq!(loops(&cut), 1);
        // The centre-ish vertex (1, 1) is interior with a closed fan of 6.
        assert!(!cut.boundary[5]);
        assert_eq!(cut.fan(5).len(), 6);
        // Corner (0, 0) is on the border; its fan starts at its outgoing border edge.
        let fan = cut.fan(0);
        assert!(cut.twin(fan[0]).is_none());
        assert!(cut.twin(CutMesh::prev(*fan.last().unwrap())).is_none());
    }

    #[test]
    fn fans_are_counter_clockwise() {
        let (p, t) = grid(3, 3);
        let cut = CutMesh::new(&p, &t, &[]);
        let v = 5u32;
        let fan = cut.fan(v);
        let centre = cut.positions[v as usize];
        let angle = |h: u32| {
            let d = cut.positions[cut.head(h) as usize] - centre;
            d.y.atan2(d.x)
        };
        let mut turn = 0.0;
        for k in 0..fan.len() {
            let (a, b) = (angle(fan[k]), angle(fan[(k + 1) % fan.len()]));
            let mut d = b - a;
            if d < 0.0 {
                d += std::f64::consts::TAU;
            }
            turn += d;
        }
        assert!((turn - std::f64::consts::TAU).abs() < 1e-9, "fan winds once CCW: {turn}");
    }
}
