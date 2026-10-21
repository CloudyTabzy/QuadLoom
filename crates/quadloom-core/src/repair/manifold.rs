//! Making the mesh manifold: every edge on at most two faces, every vertex one fan.
//!
//! Edges used by more than two faces keep one pair attached (preferring a pair
//! with opposite winding, i.e. a consistently oriented pair) and the remaining
//! faces are detached. Then every vertex whose incident faces form more than one
//! edge-connected fan gets one copy per extra fan. Detaching an edge is just
//! "don't link across it": the vertex-fan step does the actual separation.

use std::collections::HashMap;

pub use glam::DVec3;

use crate::unionfind::UnionFind;

#[derive(Clone, Debug, PartialEq)]
pub struct ManifoldSplit {
    pub positions: Vec<DVec3>,
    pub triangles: Vec<[u32; 3]>,
    /// Edges that had more than two faces.
    pub split_edges: usize,
    /// Vertex copies created (one per extra fan).
    pub split_vertices: usize,
    /// `parent[v]` is the input vertex that output vertex `v` descends from.
    pub parent: Vec<u32>,
}

/// Rounds of "detach stubborn faces, re-split fans" before giving up.
const MAX_ROUNDS: usize = 8;

/// Splits non-manifold edges and vertices. Edges listed in `forced_cuts`
/// (undirected, input vertex ids, any order) are separated too, as if they were
/// boundaries.
///
/// Fan splitting alone can't free a detached face that is also fan-connected to
/// the kept faces around *both* endpoints of its non-manifold edge. Such faces
/// get their own copies of the two endpoints, and the fan pass repeats.
pub fn split(positions: &[DVec3], triangles: &[[u32; 3]], forced_cuts: &[(u32, u32)]) -> ManifoldSplit {
    let mut cuts: Vec<(u32, u32)> = forced_cuts.iter().map(|&(a, b)| (a.min(b), a.max(b))).collect();
    cuts.sort_unstable();
    let identity: Vec<u32> = (0..positions.len() as u32).collect();
    let mut result = split_once(positions, triangles, &cuts, &identity);
    for _ in 0..MAX_ROUNDS {
        let stubborn = non_manifold_groups(&result.triangles);
        if stubborn.is_empty() {
            break;
        }
        for group in stubborn {
            let (keep_a, keep_b) = choose_pair(&group);
            for &(_, _, f, _) in group.iter().filter(|u| u.2 != keep_a && u.2 != keep_b) {
                for endpoint in [group[0].0, group[0].1] {
                    let copy = result.positions.len() as u32;
                    result.positions.push(result.positions[endpoint as usize]);
                    result.parent.push(result.parent[endpoint as usize]);
                    result.split_vertices += 1;
                    for corner in result.triangles[f as usize].iter_mut().filter(|c| **c == endpoint) {
                        *corner = copy;
                    }
                }
            }
        }
        // Detaching may have disconnected fans around the old endpoints.
        let again = split_once(&result.positions, &result.triangles, &cuts, &result.parent);
        let parent = again.parent.iter().map(|&p| result.parent[p as usize]).collect();
        result = ManifoldSplit {
            positions: again.positions,
            triangles: again.triangles,
            split_edges: result.split_edges,
            split_vertices: result.split_vertices + again.split_vertices,
            parent,
        };
    }
    result
}

/// Edge uses `(min, max, face, from)` for every edge used by more than two faces.
fn non_manifold_groups(triangles: &[[u32; 3]]) -> Vec<Vec<(u32, u32, u32, u32)>> {
    let uses = edge_uses(triangles);
    uses.chunk_by(|x, y| (x.0, x.1) == (y.0, y.1)).filter(|g| g.len() > 2).map(<[_]>::to_vec).collect()
}

/// The face pair a non-manifold edge keeps: the first pair with opposite winding
/// (a consistently oriented pair), else the first two faces.
fn choose_pair(group: &[(u32, u32, u32, u32)]) -> (u32, u32) {
    let opposite = group
        .iter()
        .enumerate()
        .flat_map(|(i, x)| group[i + 1..].iter().map(move |y| (x, y)))
        .find(|(x, y)| x.3 != y.3);
    opposite.map_or((group[0].2, group[1].2), |(x, y)| (x.2, y.2))
}

fn edge_uses(triangles: &[[u32; 3]]) -> Vec<(u32, u32, u32, u32)> {
    let mut uses: Vec<(u32, u32, u32, u32)> = triangles
        .iter()
        .enumerate()
        .flat_map(|(f, t)| {
            (0..3).map(move |i| {
                let (a, b) = (t[i], t[(i + 1) % 3]);
                (a.min(b), a.max(b), f as u32, a)
            })
        })
        .filter(|u| u.0 != u.1)
        .collect();
    uses.sort_unstable();
    uses
}

/// One pass: link faces across manifold edges (and one chosen pair per
/// non-manifold edge), then give every extra fan around a vertex its own copy.
/// `origin` maps current vertex ids to the ids `cuts` is expressed in.
fn split_once(
    positions: &[DVec3],
    triangles: &[[u32; 3]],
    cuts: &[(u32, u32)],
    origin: &[u32],
) -> ManifoldSplit {
    let uses = edge_uses(triangles);

    let mut links: HashMap<(u32, u32, u32), u32> = HashMap::new();
    let mut split_edges = 0;
    for group in uses.chunk_by(|x, y| (x.0, x.1) == (y.0, y.1)) {
        let key = (group[0].0, group[0].1);
        let (oa, ob) = (origin[key.0 as usize], origin[key.1 as usize]);
        if cuts.binary_search(&(oa.min(ob), oa.max(ob))).is_ok() {
            continue;
        }
        let pair = match group {
            [x, y] => Some((x.2, y.2)),
            [_] => None,
            _ => {
                split_edges += 1;
                Some(choose_pair(group))
            }
        };
        if let Some((f, g)) = pair {
            links.insert((f, key.0, key.1), g);
            links.insert((g, key.0, key.1), f);
        }
    }

    // Faces around each vertex, in face order.
    let mut incident: Vec<Vec<u32>> = vec![Vec::new(); positions.len()];
    for (f, t) in triangles.iter().enumerate() {
        for &v in t {
            if incident[v as usize].last() != Some(&(f as u32)) {
                incident[v as usize].push(f as u32);
            }
        }
    }

    let mut out_positions = positions.to_vec();
    let mut parent: Vec<u32> = (0..positions.len() as u32).collect();
    let mut out_triangles = triangles.to_vec();
    let mut split_vertices = 0;
    for (v, faces) in incident.iter().enumerate() {
        if faces.len() < 2 {
            continue;
        }
        let v = v as u32;
        let mut fans = UnionFind::new(faces.len());
        for (local, &f) in faces.iter().enumerate() {
            for &w in triangles[f as usize].iter().filter(|&&w| w != v) {
                let partner = links.get(&(f, v.min(w), v.max(w)));
                if let Some(other) = partner.and_then(|g| faces.binary_search(g).ok()) {
                    fans.union(local as u32, other as u32);
                }
            }
        }
        // The fan holding the lowest face keeps `v`; every other fan gets a copy.
        let mut copy_of_root: HashMap<u32, u32> = HashMap::new();
        for (local, &f) in faces.iter().enumerate() {
            let root = fans.find(local as u32);
            if root == 0 {
                continue;
            }
            let copy = *copy_of_root.entry(root).or_insert_with(|| {
                out_positions.push(positions[v as usize]);
                parent.push(v);
                split_vertices += 1;
                (out_positions.len() - 1) as u32
            });
            for corner in out_triangles[f as usize].iter_mut().filter(|c| **c == v) {
                *corner = copy;
            }
        }
    }

    ManifoldSplit { positions: out_positions, triangles: out_triangles, split_edges, split_vertices, parent }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::halfedge::HalfEdges;

    fn v(x: f64, y: f64, z: f64) -> DVec3 {
        DVec3::new(x, y, z)
    }

    #[test]
    fn three_faces_on_edge_split_to_manifold() {
        // A "book" of three pages sharing the spine 0-1.
        let positions = [v(0., 0., 0.), v(0., 0., 1.), v(1., 0., 0.), v(-1., 0., 0.), v(0., 1., 0.)];
        let triangles = [[0, 1, 2], [1, 0, 3], [0, 1, 4]];
        let s = split(&positions, &triangles, &[]);
        let he = HalfEdges::build(&s.triangles, s.positions.len());
        assert_eq!(he.non_manifold_edge_count(), 0);
        assert_eq!(s.split_edges, 1);
        assert_eq!(s.positions.len(), 7, "the detached page gets its own copy of both spine vertices");
        assert_eq!(s.parent.len(), s.positions.len());
        assert_eq!(&s.parent[5..], &[0, 1]);
    }

    #[test]
    fn opposite_winding_pair_is_kept_together() {
        // Faces 0 and 2 traverse the spine the same way (0→1); face 1 opposes them.
        // The consistent pair (0, 1) stays attached; face 2 is detached.
        let positions = [v(0., 0., 0.), v(0., 0., 1.), v(1., 0., 0.), v(-1., 0., 0.), v(0., 1., 0.)];
        let s = split(&positions, &[[0, 1, 2], [1, 0, 3], [0, 1, 4]], &[]);
        assert_eq!(&s.triangles[0][..2], &[0, 1]);
        assert_eq!(&s.triangles[1][..2], &[1, 0]);
        assert_ne!(&s.triangles[2][..2], &[0, 1]);
    }

    #[test]
    fn bowtie_vertex_split() {
        // Two triangles touching only at vertex 0.
        let positions = [v(0., 0., 0.), v(1., 0., 0.), v(1., 1., 0.), v(-1., 0., 0.), v(-1., -1., 0.)];
        let s = split(&positions, &[[0, 1, 2], [0, 3, 4]], &[]);
        assert_eq!(s.split_vertices, 1);
        assert_eq!(s.positions.len(), 6);
        assert_ne!(s.triangles[0][0], s.triangles[1][0]);
    }

    #[test]
    fn manifold_mesh_is_untouched() {
        let positions = [v(0., 0., 0.), v(1., 0., 0.), v(0., 1., 0.), v(1., 1., 0.)];
        let triangles = [[0, 1, 2], [1, 3, 2]];
        let s = split(&positions, &triangles, &[]);
        assert_eq!(s.triangles, triangles.to_vec());
        assert_eq!((s.split_edges, s.split_vertices), (0, 0));
    }

    #[test]
    fn detached_face_connected_at_both_ends_is_separated() {
        // Proptest's shrunk case: face 2 hangs on edge 0-1 next to faces 0 and 1,
        // and is also fan-connected to them around both 0 and 1 through other
        // edges, so splitting vertices by fan alone can't free it.
        let positions =
            [v(0., 0., 0.), v(0., 0., 1.), v(0., 0., 0.5), v(0., 1., 0.), v(1., 0., 0.), v(0., 0., -1.)];
        let flat: Vec<u32> = vec![
            0, 2, 1, 1, 4, 3, 0, 2, 1, 3, 5, 4, 5, 1, 0, 4, 1, 5, 3, 4, 0, 2, 2, 5, 0, 3, 1, 0, 1, 1, 5, 3,
            0, 1, 0, 5, 0, 5, 3, 4, 2, 2, 5, 5, 3, 2, 1, 1, 0, 2, 1, 1, 0, 0, 4, 0, 4,
        ];
        let triangles: Vec<[u32; 3]> = flat
            .chunks_exact(3)
            .map(|t| [t[0], t[1], t[2]])
            .filter(|t| t[0] != t[1] && t[1] != t[2] && t[0] != t[2])
            .collect();
        let s = split(&positions, &triangles, &[]);
        let he = HalfEdges::build(&s.triangles, s.positions.len());
        assert_eq!(he.non_manifold_edge_count(), 0);
        assert!(s.parent.iter().all(|&p| (p as usize) < positions.len()));
    }

    #[test]
    fn forced_cut_separates_an_edge() {
        let positions = [v(0., 0., 0.), v(1., 0., 0.), v(0., 1., 0.), v(1., 1., 0.)];
        let s = split(&positions, &[[0, 1, 2], [1, 3, 2]], &[(1, 2)]);
        let he = HalfEdges::build(&s.triangles, s.positions.len());
        assert_eq!(he.boundary_edge_count(), 6, "cutting the diagonal leaves two separate triangles");
    }
}
