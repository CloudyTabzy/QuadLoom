//! Smoothing of the quad mesh on the surface.
//!
//! Patterns are placed patch by patch through a map of each patch onto its
//! polygon, so quads can kink where patches meet. A few rounds of Laplacian
//! smoothing (each free vertex to the average of its neighbours) even this
//! out. Each round is projected back onto the triangles of the patches the
//! vertex belongs to, so it stays on the surface and can't jump across thin
//! parts. Vertices on feature lines and borders don't move.

use std::collections::HashSet;

use glam::DVec3;
use rayon::prelude::*;

use crate::mesh::PolyMesh;
use crate::spatial::TriangleBvh;
use crate::trace::TraceOutput;

/// Smooths `mesh` in place: `fixed` vertices stay, `patch_of_face` names the
/// layout patch of each output face (to restrict projection).
pub fn smooth(
    mesh: &mut PolyMesh,
    fixed: &[bool],
    patch_of_face: &[u32],
    trace: &TraceOutput,
    iterations: usize,
) {
    if iterations == 0 || mesh.positions.is_empty() {
        return;
    }
    let n = mesh.positions.len();
    let mut neighbours: Vec<Vec<u32>> = vec![Vec::new(); n];
    let mut patches: Vec<Vec<u32>> = vec![Vec::new(); n];
    for (f, w) in mesh.face_offsets.windows(2).enumerate() {
        let face = mesh.face_indices.get(w[0] as usize..w[1] as usize).unwrap_or_default();
        let patch = patch_of_face.get(f).copied().unwrap_or(u32::MAX);
        for (i, &v) in face.iter().enumerate() {
            let next = face[(i + 1) % face.len()];
            let prev = face[(i + face.len() - 1) % face.len()];
            if let Some(list) = neighbours.get_mut(v as usize) {
                for u in [next, prev] {
                    if !list.contains(&u) {
                        list.push(u);
                    }
                }
            }
            if let Some(list) = patches.get_mut(v as usize)
                && !list.contains(&patch)
            {
                list.push(patch);
            }
        }
    }
    let surface = TriangleBvh::new(&trace.mesh.positions, &trace.mesh.triangles);
    for _ in 0..iterations {
        let current = &mesh.positions;
        let next: Vec<DVec3> = (0..n)
            .into_par_iter()
            .map(|v| {
                let p = current[v];
                if fixed.get(v).copied().unwrap_or(true) || neighbours[v].is_empty() {
                    return p;
                }
                let average = neighbours[v].iter().map(|&u| current[u as usize]).sum::<DVec3>()
                    / neighbours[v].len() as f64;
                let allowed: HashSet<u32> = patches[v].iter().copied().collect();
                let projected = surface.closest_point_where(average, |t| {
                    trace.patch_of_face.get(t as usize).is_some_and(|q| allowed.contains(q))
                });
                match projected {
                    Some((q, _)) if q.is_finite() => q,
                    _ => p,
                }
            })
            .collect();
        mesh.positions = next;
    }
}
