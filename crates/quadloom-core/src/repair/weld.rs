//! Merging vertices that sit (almost) on top of each other.
//!
//! Exporters routinely split vertices at UV seams and hard normals; to us those
//! are the same point, and leaving them split would turn every seam into a hole.

use std::collections::HashMap;

pub use glam::DVec3;

use crate::unionfind::UnionFind;

/// The welded mesh and how input vertices map onto it.
#[derive(Clone, Debug, PartialEq)]
pub struct Weld {
    pub positions: Vec<DVec3>,
    pub triangles: Vec<[u32; 3]>,
    /// Input vertices that were merged into another one.
    pub merged: usize,
    /// `remap[input_vertex]` is the welded vertex it became.
    pub remap: Vec<u32>,
}

/// Merges vertices closer than `epsilon` (absolute). Each cluster keeps the
/// position of its lowest-index vertex, so the result is order-independent.
/// `epsilon <= 0` merges exact duplicates only.
pub fn weld(positions: &[DVec3], triangles: &[[u32; 3]], epsilon: f64) -> Weld {
    let mut sets = UnionFind::new(positions.len());
    if epsilon > 0.0 {
        union_within(positions, epsilon, &mut sets);
    } else {
        union_exact(positions, &mut sets);
    }

    let mut new_index = vec![u32::MAX; positions.len()];
    let mut welded = Vec::new();
    for v in 0..positions.len() as u32 {
        let root = sets.find(v);
        if root == v {
            new_index[v as usize] = welded.len() as u32;
            welded.push(positions[v as usize]);
        }
    }
    let remap: Vec<u32> = (0..positions.len() as u32).map(|v| new_index[sets.find(v) as usize]).collect();
    let triangles = triangles.iter().map(|t| t.map(|i| remap[i as usize])).collect();
    Weld { merged: positions.len() - welded.len(), positions: welded, triangles, remap }
}

/// Grid hashing with cells of size `epsilon`: any pair within `epsilon` lies in
/// the same or a neighbouring cell, so 27 lookups per vertex suffice.
fn union_within(positions: &[DVec3], epsilon: f64, sets: &mut UnionFind) {
    // Float-to-int casts saturate, so absurd coordinates can't overflow.
    let cell = |p: DVec3| {
        [(p.x / epsilon).floor() as i64, (p.y / epsilon).floor() as i64, (p.z / epsilon).floor() as i64]
    };
    let mut grid: HashMap<[i64; 3], Vec<u32>> = HashMap::new();
    for (i, &p) in positions.iter().enumerate() {
        grid.entry(cell(p)).or_default().push(i as u32);
    }
    let eps2 = epsilon * epsilon;
    for (i, &p) in positions.iter().enumerate() {
        let [cx, cy, cz] = cell(p);
        for dx in -1..=1i64 {
            for dy in -1..=1i64 {
                for dz in -1..=1i64 {
                    let key = [cx.saturating_add(dx), cy.saturating_add(dy), cz.saturating_add(dz)];
                    for &j in grid.get(&key).map(Vec::as_slice).unwrap_or_default() {
                        if (j as usize) < i && (positions[j as usize] - p).length_squared() <= eps2 {
                            sets.union(i as u32, j);
                        }
                    }
                }
            }
        }
    }
}

fn union_exact(positions: &[DVec3], sets: &mut UnionFind) {
    let mut order: Vec<u32> = (0..positions.len() as u32).collect();
    let key = |i: u32| positions[i as usize].to_array().map(f64::to_bits);
    order.sort_unstable_by_key(|&i| (key(i), i));
    for pair in order.windows(2) {
        if key(pair[0]) == key(pair[1]) {
            sets.union(pair[0], pair[1]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f64, y: f64, z: f64) -> DVec3 {
        DVec3::new(x, y, z)
    }

    fn boundary_edges(triangles: &[[u32; 3]]) -> usize {
        let mut keys: Vec<(u32, u32)> = triangles
            .iter()
            .flat_map(|t| (0..3).map(move |i| (t[i].min(t[(i + 1) % 3]), t[i].max(t[(i + 1) % 3]))))
            .collect();
        keys.sort_unstable();
        keys.chunk_by(|a, b| a == b).filter(|g| g.len() == 1).count()
    }

    #[test]
    fn weld_joins_uv_split_quad() {
        // Two triangles of a unit square whose shared diagonal vertices are duplicated.
        let positions =
            [v(0., 0., 0.), v(1., 0., 0.), v(1., 1., 0.), v(0., 0., 0.), v(1., 1., 0.), v(0., 1., 0.)];
        let triangles = [[0, 1, 2], [3, 4, 5]];
        let w = weld(&positions, &triangles, 1e-9);
        assert_eq!(w.positions.len(), 4);
        assert_eq!(w.merged, 2);
        assert_eq!(boundary_edges(&w.triangles), 4);
        assert_eq!(w.remap, vec![0, 1, 2, 0, 2, 3]);
    }

    #[test]
    fn weld_keeps_separate_touching_parts() {
        let positions =
            [v(0., 0., 0.), v(1., 0., 0.), v(0., 1., 0.), v(0., 0., 1e-3), v(1., 0., 1e-3), v(0., 1., 1e-3)];
        let w = weld(&positions, &[[0, 1, 2], [3, 4, 5]], 1e-6);
        assert_eq!(w.positions.len(), 6);
        assert_eq!(w.merged, 0);
    }

    #[test]
    fn weld_merges_within_epsilon_across_cell_borders() {
        // 0.5e-6 apart but straddling a grid cell boundary.
        let positions = [v(0.999_999_9e-6, 0., 0.), v(1.000_000_4e-6, 0., 0.), v(1., 1., 1.)];
        let w = weld(&positions, &[[0, 1, 2]], 1e-6);
        assert_eq!(w.merged, 1);
    }

    #[test]
    fn weld_is_deterministic_under_permutation() {
        let a = [v(0., 0., 0.), v(1., 0., 0.), v(0., 0., 0.), v(0., 1., 0.)];
        let b = [v(0., 1., 0.), v(0., 0., 0.), v(1., 0., 0.), v(0., 0., 0.)];
        let wa = weld(&a, &[[0, 1, 3], [2, 3, 1]], 1e-9);
        let wb = weld(&b, &[[1, 2, 0], [3, 0, 2]], 1e-9);
        assert_eq!(wa.positions.len(), wb.positions.len());
        assert_eq!(wa.merged, wb.merged);
        // Same input → same output, every time.
        assert_eq!(weld(&a, &[[0, 1, 3], [2, 3, 1]], 1e-9).remap, wa.remap);
    }

    #[test]
    fn zero_epsilon_welds_exact_duplicates_only() {
        let positions = [v(0., 0., 0.), v(0., 0., 0.), v(1e-12, 0., 0.)];
        let w = weld(&positions, &[[0, 1, 2]], 0.0);
        assert_eq!(w.merged, 1);
        assert_eq!(w.positions.len(), 2);
    }
}
