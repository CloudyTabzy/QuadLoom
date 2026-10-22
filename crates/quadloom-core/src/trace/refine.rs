//! Refinement before tracing.
//!
//! Paths never pass through singular vertices and run only along mesh edges,
//! so a singularity's one-ring, or two adjacent singularities, can block every
//! path that should separate them. As in the oracle
//! (`SplitAdjacentSingularities`), every edge at a singular vertex is split at
//! its midpoint, and so is every edge whose two ends each lie on exactly one
//! (non-border) feature edge: an isolated feature edge then gets an interior
//! vertex, so cutting along it actually opens the mesh.

use std::collections::HashMap;

use glam::DVec3;

use super::TraceMesh;
use super::graph::CANCEL_EVERY;
use crate::control::{Control, Stage};
use crate::error::Error;
use crate::field::{FaceFrames, ROSY, vertex_indices};

/// `mesh` with the edges described above split (1-to-2, 1-to-3 or 1-to-4 per
/// face, so the result stays conforming). Child faces keep their parent's
/// direction, split feature edges stay features, and singularities are
/// recomputed (they stay on the same vertices: children of one face share its
/// direction, so no turn appears across the new edges).
pub fn for_tracing(mesh: &TraceMesh, control: &Control) -> Result<TraceMesh, Error> {
    let n = mesh.positions.len();
    let mut singular = vec![false; n];
    for &(v, k) in &mesh.singularities {
        if k != 0 {
            singular[v as usize] = true;
        }
    }
    let mut uses: HashMap<(u32, u32), u32> = HashMap::new();
    for t in &mesh.triangles {
        for i in 0..3 {
            let (a, b) = (t[i], t[(i + 1) % 3]);
            *uses.entry((a.min(b), a.max(b))).or_default() += 1;
        }
    }
    let mut degree = vec![0u32; n];
    for &[a, b] in &mesh.feature_edges {
        if uses.get(&(a.min(b), a.max(b))) == Some(&2) {
            degree[a as usize] += 1;
            degree[b as usize] += 1;
        }
    }
    split_edges(
        mesh,
        |a, b| {
            let (a, b) = (a as usize, b as usize);
            singular[a] || singular[b] || (degree[a] == 1 && degree[b] == 1)
        },
        control,
    )
}

/// `mesh` with every edge `(a, b)` for which `split(a, b)` holds split at its
/// midpoint. New vertices are numbered after the existing ones, in order of
/// first use (faces in order, then corners).
pub(crate) fn split_edges(
    mesh: &TraceMesh,
    split: impl Fn(u32, u32) -> bool,
    control: &Control,
) -> Result<TraceMesh, Error> {
    let mut positions = mesh.positions.clone();
    let mut mid: HashMap<(u32, u32), u32> = HashMap::new();
    let mut midpoint = |a: u32, b: u32, positions: &mut Vec<DVec3>| -> u32 {
        *mid.entry((a.min(b), a.max(b))).or_insert_with(|| {
            positions.push(0.5 * (positions[a as usize] + positions[b as usize]));
            (positions.len() - 1) as u32
        })
    };
    let mut triangles = Vec::with_capacity(mesh.triangles.len());
    let mut directions = Vec::with_capacity(mesh.triangles.len());
    for (f, &t) in mesh.triangles.iter().enumerate() {
        if f as u32 % CANCEL_EVERY == 0 {
            control.checkpoint(Stage::Trace)?;
        }
        let marked = [0, 1, 2].map(|i| split(t[i], t[(i + 1) % 3]));
        let children: Vec<[u32; 3]> = match marked.iter().filter(|&&m| m).count() {
            0 => vec![t],
            1 => {
                // Rotate so the split edge is a → b.
                let i = marked.iter().position(|&m| m).unwrap_or(0);
                let [a, b, c] = [t[i], t[(i + 1) % 3], t[(i + 2) % 3]];
                let m = midpoint(a, b, &mut positions);
                vec![[a, m, c], [m, b, c]]
            }
            2 => {
                // Rotate so the unsplit edge is c → a.
                let i = (marked.iter().position(|&m| !m).unwrap_or(0) + 1) % 3;
                let [a, b, c] = [t[i], t[(i + 1) % 3], t[(i + 2) % 3]];
                let (mab, mbc) = (midpoint(a, b, &mut positions), midpoint(b, c, &mut positions));
                let p = |v: u32| positions[v as usize];
                // The quad a, mab, mbc, c is split along its shorter diagonal.
                if (p(a) - p(mbc)).length() <= (p(mab) - p(c)).length() {
                    vec![[mab, b, mbc], [a, mab, mbc], [a, mbc, c]]
                } else {
                    vec![[mab, b, mbc], [a, mab, c], [mab, mbc, c]]
                }
            }
            _ => {
                let [a, b, c] = t;
                let (mab, mbc, mca) = (
                    midpoint(a, b, &mut positions),
                    midpoint(b, c, &mut positions),
                    midpoint(c, a, &mut positions),
                );
                vec![[a, mab, mca], [mab, b, mbc], [mca, mbc, c], [mab, mbc, mca]]
            }
        };
        let direction = mesh.directions.get(f).copied().unwrap_or(DVec3::ZERO);
        directions.extend(std::iter::repeat_n(direction, children.len()));
        triangles.extend(children);
    }
    let mut feature_edges: Vec<[u32; 2]> = Vec::with_capacity(mesh.feature_edges.len());
    for &[a, b] in &mesh.feature_edges {
        match mid.get(&(a.min(b), a.max(b))) {
            Some(&m) => feature_edges.extend([[a.min(m), a.max(m)], [b.min(m), b.max(m)]]),
            None => feature_edges.push([a.min(b), a.max(b)]),
        }
    }
    feature_edges.sort_unstable();
    feature_edges.dedup();
    control.checkpoint(Stage::Trace)?;
    let frames = FaceFrames::new(&positions, &triangles);
    let powers: Vec<_> =
        (0..triangles.len() as u32).map(|f| frames.power(f, directions[f as usize], ROSY)).collect();
    let singularities = vertex_indices(&positions, &triangles, &powers)
        .into_iter()
        .enumerate()
        .filter(|&(_, k)| k != 0)
        .map(|(v, k)| (v as u32, k))
        .collect();
    control.checkpoint(Stage::Trace)?;
    Ok(TraceMesh { positions, triangles, feature_edges, directions, singularities })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::field::{FieldParams, compute};
    use crate::mesh::halfedge::HalfEdges;
    use crate::remesh::tests::sphere_mesh;

    fn sphere_with_field() -> TraceMesh {
        let (positions, triangles) = sphere_mesh(12, 24);
        let field =
            compute(&positions, &triangles, &[], &FieldParams::default(), &Control::default()).unwrap();
        TraceMesh {
            positions,
            triangles,
            feature_edges: Vec::new(),
            directions: field.directions,
            singularities: field.singularities,
        }
    }

    #[test]
    fn refine_splits_edges_at_singular_vertices() {
        let mesh = sphere_with_field();
        assert_eq!(mesh.singularities.iter().map(|s| s.1).sum::<i32>(), 8);
        let refined = for_tracing(&mesh, &Control::default()).unwrap();
        let old = mesh.positions.len() as u32;
        assert!(refined.positions.len() > mesh.positions.len());
        assert_eq!(refined.singularities.iter().map(|s| s.1).sum::<i32>(), 8);
        assert_eq!(refined.singularities, mesh.singularities, "singularities stay on the same vertices");
        for &(s, _) in &refined.singularities {
            for t in refined.triangles.iter().filter(|t| t.contains(&s)) {
                for &v in t.iter().filter(|&&v| v != s) {
                    assert!(v >= old, "singular vertex {s} still has an unsplit edge to {v}");
                }
            }
        }
    }

    #[test]
    fn refine_is_conforming() {
        let refined = for_tracing(&sphere_with_field(), &Control::default()).unwrap();
        let he = HalfEdges::build(&refined.triangles, refined.positions.len());
        assert_eq!(he.boundary_edge_count(), 0);
        assert_eq!(he.non_manifold_edge_count(), 0);
        assert_eq!(refined.directions.len(), refined.triangles.len());
    }

    #[test]
    fn isolated_feature_edge_gets_an_interior_vertex() {
        let (positions, triangles) = crate::testmesh::grid(4, 4);
        let directions = vec![DVec3::X; triangles.len()];
        // Vertex (2, 2) to (3, 2): both ends lie on exactly one feature edge.
        let mesh = TraceMesh {
            positions,
            triangles,
            feature_edges: vec![[12, 13]],
            directions,
            ..TraceMesh::default()
        };
        let refined = for_tracing(&mesh, &Control::default()).unwrap();
        assert_eq!(refined.feature_edges, vec![[12, 25], [13, 25]]);
        assert_eq!(refined.positions[25], DVec3::new(2.5, 2.0, 0.0));
    }
}
