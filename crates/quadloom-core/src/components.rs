//! Connected components of a manifold triangle mesh (connected across shared edges).

use crate::unionfind::UnionFind;

/// One connected piece: indices into the mesh's triangles, ascending.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Component {
    pub faces: Vec<u32>,
}

/// Faces connected through edges shared by exactly two faces. Components are
/// ordered by their lowest face.
pub fn split(triangles: &[[u32; 3]], vertex_count: usize) -> Vec<Component> {
    debug_assert!(triangles.iter().flatten().all(|&v| (v as usize) < vertex_count));
    let mut edges: Vec<(u32, u32, u32)> = triangles
        .iter()
        .enumerate()
        .flat_map(|(f, t)| {
            (0..3).map(move |i| (t[i].min(t[(i + 1) % 3]), t[i].max(t[(i + 1) % 3]), f as u32))
        })
        .collect();
    edges.sort_unstable();
    let mut sets = UnionFind::new(triangles.len());
    for group in edges.chunk_by(|x, y| (x.0, x.1) == (y.0, y.1)) {
        if let [x, y] = group {
            sets.union(x.2, y.2);
        }
    }
    let mut components: Vec<Component> = Vec::new();
    let mut index_of_root = vec![usize::MAX; triangles.len()];
    for f in 0..triangles.len() as u32 {
        let root = sets.find(f) as usize;
        if index_of_root[root] == usize::MAX {
            index_of_root[root] = components.len();
            components.push(Component { faces: Vec::new() });
        }
        components[index_of_root[root]].faces.push(f);
    }
    components
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn components_split_two_patches() {
        let triangles = [[0, 1, 2], [1, 3, 2], [4, 5, 6]];
        let c = split(&triangles, 7);
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].faces, vec![0, 1]);
        assert_eq!(c[1].faces, vec![2]);
    }

    #[test]
    fn vertex_touching_faces_are_separate_components() {
        // Sharing only vertex 0 doesn't connect two faces.
        let c = split(&[[0, 1, 2], [0, 3, 4]], 5);
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn components_ordered_by_lowest_face() {
        let c = split(&[[4, 5, 6], [0, 1, 2], [1, 3, 2]], 7);
        assert_eq!(c[0].faces, vec![0]);
        assert_eq!(c[1].faces, vec![1, 2]);
    }
}
