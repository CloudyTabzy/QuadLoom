//! Stage 0b: the repair ladder. Each step is a pure function over index buffers
//! and reports exactly what it changed (design §6.1: never repair silently).

pub mod degenerate;
pub mod manifold;
pub mod orient;
pub mod weld;

use glam::DVec3;

use crate::components::{self, Component};
use crate::control::{Control, Stage};
use crate::error::Error;
use crate::mesh::InputMesh;

/// Relative area below which a triangle counts as zero-area (× squared bbox diagonal).
const ZERO_AREA_RATIO: f64 = 1e-12;
/// How many cut-and-reorient rounds a non-orientable surface gets.
const MAX_ORIENT_ROUNDS: usize = 4;

#[derive(Clone, Debug, PartialEq)]
pub struct RepairParams {
    /// Weld distance as a fraction of the bounding-box diagonal.
    pub weld_epsilon_rel: f64,
    pub remove_degenerate: bool,
    pub split_non_manifold: bool,
    pub orient: bool,
    /// Components with fewer faces are skipped (and reported).
    pub min_component_faces: usize,
}

impl Default for RepairParams {
    fn default() -> Self {
        Self {
            weld_epsilon_rel: 1e-6,
            remove_degenerate: true,
            split_non_manifold: true,
            orient: true,
            min_component_faces: 8,
        }
    }
}

/// Every change the repair ladder made.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct RepairReport {
    pub welded_vertices: usize,
    pub removed_repeated: usize,
    pub removed_zero_area: usize,
    pub removed_duplicates: usize,
    pub split_non_manifold_edges: usize,
    pub split_non_manifold_vertices: usize,
    pub flipped_faces: usize,
    /// Edges cut to make non-orientable surfaces orientable.
    pub cut_edges: usize,
    /// Vertices dropped because no surviving face uses them (includes skipped parts).
    pub removed_isolated_vertices: usize,
    pub components: usize,
    pub skipped_small_components: usize,
    pub skipped_small_component_faces: usize,
}

/// A clean triangle mesh plus the map from the input's vertices onto it.
#[derive(Clone, Debug, PartialEq)]
pub struct RepairedMesh {
    pub positions: Vec<DVec3>,
    pub triangles: Vec<[u32; 3]>,
    /// `vertex_map[input_vertex]`: its first copy in the repaired mesh, if it survived.
    pub vertex_map: Vec<Option<u32>>,
    /// Every repaired vertex descending from each input vertex (several after a split).
    pub vertex_copies: Vec<Vec<u32>>,
}

/// Runs the repair ladder: weld → degenerate removal → manifold split →
/// orientation (cutting non-orientable edges) → components → compaction.
pub fn repair(
    input: &InputMesh,
    params: &RepairParams,
    control: &Control,
) -> Result<(RepairedMesh, Vec<Component>, RepairReport), Error> {
    let stage = Stage::Repair;
    let mut report = RepairReport::default();
    control.checkpoint(stage)?;
    let diagonal = bbox_diagonal(input.positions());

    let welded = weld::weld(input.positions(), input.triangles(), params.weld_epsilon_rel * diagonal);
    report.welded_vertices = welded.merged;
    let input_to_welded = welded.remap;
    let (mut positions, mut triangles) = (welded.positions, welded.triangles);
    // `ancestor[v]`: the welded vertex each current vertex descends from.
    let mut ancestor: Vec<u32> = (0..positions.len() as u32).collect();
    control.set_progress(stage, 0.2);
    control.checkpoint(stage)?;

    if params.remove_degenerate {
        let d = degenerate::remove(&positions, &triangles, ZERO_AREA_RATIO * diagonal * diagonal);
        report.removed_repeated = d.removed_repeated;
        report.removed_zero_area = d.removed_zero_area;
        report.removed_duplicates = d.removed_duplicates;
        triangles = d.triangles;
    }
    control.set_progress(stage, 0.35);
    control.checkpoint(stage)?;

    if params.split_non_manifold {
        let s = manifold::split(&positions, &triangles, &[]);
        report.split_non_manifold_edges = s.split_edges;
        report.split_non_manifold_vertices = s.split_vertices;
        ancestor = s.parent.iter().map(|&p| ancestor[p as usize]).collect();
        (positions, triangles) = (s.positions, s.triangles);
    }
    control.set_progress(stage, 0.55);
    control.checkpoint(stage)?;

    if params.orient {
        for _ in 0..MAX_ORIENT_ROUNDS {
            let o = orient::orient(&positions, &triangles);
            report.flipped_faces += o.flipped;
            triangles = o.triangles;
            if o.cut_edges.is_empty() {
                break;
            }
            report.cut_edges += o.cut_edges.len();
            let s = manifold::split(&positions, &triangles, &o.cut_edges);
            ancestor = s.parent.iter().map(|&p| ancestor[p as usize]).collect();
            (positions, triangles) = (s.positions, s.triangles);
            control.checkpoint(stage)?;
        }
    }
    control.set_progress(stage, 0.75);

    let mut kept = Vec::with_capacity(triangles.len());
    let mut components_out = Vec::new();
    for component in components::split(&triangles, positions.len()) {
        if component.faces.len() < params.min_component_faces {
            report.skipped_small_components += 1;
            report.skipped_small_component_faces += component.faces.len();
            continue;
        }
        let start = kept.len() as u32;
        kept.extend(component.faces.iter().map(|&f| triangles[f as usize]));
        components_out.push(Component { faces: (start..kept.len() as u32).collect() });
    }
    report.components = components_out.len();

    // Compact: drop vertices no surviving face uses.
    let mut new_index = vec![u32::MAX; positions.len()];
    for &v in kept.iter().flatten() {
        new_index[v as usize] = 0;
    }
    let mut compact_positions = Vec::new();
    let mut compact_ancestor = Vec::new();
    for (v, slot) in new_index.iter_mut().enumerate() {
        if *slot == 0 {
            *slot = compact_positions.len() as u32;
            compact_positions.push(positions[v]);
            compact_ancestor.push(ancestor[v]);
        }
    }
    report.removed_isolated_vertices = positions.len() - compact_positions.len();
    let triangles: Vec<[u32; 3]> = kept.iter().map(|t| t.map(|v| new_index[v as usize])).collect();

    let welded_count = input_to_welded.iter().copied().max().map_or(0, |m| m as usize + 1);
    let mut copies_of_welded: Vec<Vec<u32>> = vec![Vec::new(); welded_count];
    for (v, &w) in compact_ancestor.iter().enumerate() {
        copies_of_welded[w as usize].push(v as u32);
    }
    let vertex_copies: Vec<Vec<u32>> =
        input_to_welded.iter().map(|&w| copies_of_welded[w as usize].clone()).collect();
    let vertex_map = vertex_copies.iter().map(|c| c.first().copied()).collect();

    control.set_progress(stage, 1.0);
    Ok((
        RepairedMesh { positions: compact_positions, triangles, vertex_map, vertex_copies },
        components_out,
        report,
    ))
}

fn bbox_diagonal(positions: &[DVec3]) -> f64 {
    let mut points = positions.iter().copied();
    let Some(first) = points.next() else { return 1.0 };
    let (lo, hi) = points.fold((first, first), |(lo, hi), p| (lo.min(p), hi.max(p)));
    let d = (hi - lo).length();
    if d > 0.0 { d } else { 1.0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::Control;
    use crate::mesh::InputMesh;
    use crate::mesh::halfedge::HalfEdges;
    use proptest::prelude::*;

    /// A unit cube with every face's vertices duplicated (as a glTF exporter
    /// splitting at hard normals would), one face flipped, and one duplicate face.
    fn messy_cube() -> InputMesh {
        let corners = [
            [0., 0., 0.],
            [1., 0., 0.],
            [1., 1., 0.],
            [0., 1., 0.],
            [0., 0., 1.],
            [1., 0., 1.],
            [1., 1., 1.],
            [0., 1., 1.],
        ];
        let quads = [[0, 3, 2, 1], [4, 5, 6, 7], [0, 1, 5, 4], [3, 7, 6, 2], [0, 4, 7, 3], [1, 2, 6, 5]];
        let (mut p, mut i) = (Vec::new(), Vec::new());
        for q in quads {
            let base = (p.len() / 3) as u32;
            for c in q {
                p.extend(corners[c].map(|x: f64| x as f32));
            }
            i.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
        }
        i.swap(1, 2); // flip the first triangle
        i.extend_from_slice(&[4, 5, 6]); // duplicate of the third triangle
        InputMesh::from_buffers(&p, &i).unwrap()
    }

    #[test]
    fn repair_report_counts_match() {
        let (mesh, components, report) =
            repair(&messy_cube(), &RepairParams::default(), &Control::default()).unwrap();
        assert_eq!(report.welded_vertices, 24 - 8);
        assert_eq!(report.removed_duplicates, 1);
        assert_eq!(report.flipped_faces, 1);
        assert_eq!(report.components, 1);
        assert_eq!(components.len(), 1);
        assert_eq!(mesh.positions.len(), 8);
        assert_eq!(mesh.triangles.len(), 12);
        let he = HalfEdges::build(&mesh.triangles, mesh.positions.len());
        assert_eq!((he.boundary_edge_count(), he.non_manifold_edge_count()), (0, 0));
        assert!(orient::signed_volume(&mesh.positions, &mesh.triangles) > 0.0);
    }

    #[test]
    fn vertex_map_tracks_welds() {
        let input = messy_cube();
        let (mesh, _, _) = repair(&input, &RepairParams::default(), &Control::default()).unwrap();
        for (v, target) in mesh.vertex_map.iter().enumerate() {
            let target = target.expect("every input vertex survives in a closed cube");
            assert_eq!(mesh.positions[target as usize], input.positions()[v]);
            assert_eq!(mesh.vertex_copies[v], vec![target]);
        }
    }

    #[test]
    fn small_component_skipped_and_counted() {
        let mut p: Vec<f32> =
            messy_cube().positions().iter().flat_map(|q| [q.x as f32, q.y as f32, q.z as f32]).collect();
        let mut i: Vec<u32> = messy_cube().triangles().iter().flatten().copied().collect();
        let base = (p.len() / 3) as u32;
        p.extend([5.0, 0.0, 0.0, 6.0, 0.0, 0.0, 5.0, 1.0, 0.0]);
        i.extend([base, base + 1, base + 2]);
        let input = InputMesh::from_buffers(&p, &i).unwrap();
        let (mesh, components, report) =
            repair(&input, &RepairParams::default(), &Control::default()).unwrap();
        assert_eq!(report.components, 1);
        assert_eq!(report.skipped_small_components, 1);
        assert_eq!(report.skipped_small_component_faces, 1);
        assert_eq!(components.len(), 1);
        assert_eq!(mesh.triangles.len(), 12);
        assert_eq!(mesh.vertex_map[base as usize], None, "vertices of skipped parts map to nothing");
    }

    #[test]
    fn split_vertex_keeps_all_copies() {
        // Bow-tie: two triangles sharing vertex 0 only.
        let p = [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, -1.0, 0.0, 0.0, -1.0, -1.0, 0.0];
        let input = InputMesh::from_buffers(&p, &[0, 1, 2, 0, 3, 4]).unwrap();
        let params = RepairParams { min_component_faces: 1, ..RepairParams::default() };
        let (mesh, components, report) = repair(&input, &params, &Control::default()).unwrap();
        assert_eq!(report.split_non_manifold_vertices, 1);
        assert_eq!(components.len(), 2);
        assert_eq!(mesh.vertex_copies[0].len(), 2);
    }

    #[test]
    fn cancel_is_honoured() {
        let control = Control::default();
        control.cancel();
        assert!(repair(&messy_cube(), &RepairParams::default(), &control).is_err());
    }

    fn soup() -> impl Strategy<Value = (Vec<f32>, Vec<u32>)> {
        (4usize..24).prop_flat_map(|n| {
            let pos = proptest::collection::vec(-2i8..3, n * 3)
                .prop_map(|v| v.into_iter().map(f32::from).collect());
            let idx = proptest::collection::vec(0..n as u32, 3..60).prop_map(|mut v| {
                v.truncate(v.len() / 3 * 3);
                v
            });
            (pos, idx)
        })
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]
        #[test]
        fn repair_is_stable((p, i) in soup()) {
            let params = RepairParams { min_component_faces: 1, ..RepairParams::default() };
            let Ok(input) = InputMesh::from_buffers(&p, &i) else { return Ok(()) };
            let (first, _, _) = repair(&input, &params, &Control::default()).unwrap();
            let he = HalfEdges::build(&first.triangles, first.positions.len());
            prop_assert_eq!(he.non_manifold_edge_count(), 0);
            if first.triangles.is_empty() {
                return Ok(());
            }
            let flat: Vec<f32> = first.positions.iter().flat_map(|q| [q.x as f32, q.y as f32, q.z as f32]).collect();
            let again = InputMesh::from_buffers(&flat, &first.triangles.iter().flatten().copied().collect::<Vec<_>>()).unwrap();
            let (second, _, _) = repair(&again, &params, &Control::default()).unwrap();
            prop_assert_eq!(second.triangles.len(), first.triangles.len());
            prop_assert_eq!(second.positions.len(), first.positions.len());
        }
    }
}
