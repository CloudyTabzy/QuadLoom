//! Smoothing of the quad mesh on the surface.
//!
//! Patterns are placed patch by patch through a map of each patch onto its
//! polygon, so quads can kink where patches meet. A few rounds of smoothing
//! even this out: each free vertex moves to a blend of its neighbours'
//! average and, per incident quad, the corner that would make that quad a
//! parallelogram (which keeps quads square around irregular vertices). Each round is projected back onto the triangles of the patches the
//! vertex belongs to, so it stays on the surface and can't jump across thin
//! parts. Vertices on feature lines and borders don't move.

use std::collections::HashSet;

use glam::DVec3;
use rayon::prelude::*;

use crate::mesh::PolyMesh;
use crate::spatial::TriangleBvh;
use crate::trace::TraceOutput;

/// Blend of the parallelogram target into the Laplacian average. The
/// Laplacian alone skews quads around irregular vertices; the parallelogram
/// target alone can oscillate.
const PARALLELOGRAM_WEIGHT: f64 = 0.3;
/// Fraction of the way each round moves toward the target (under-relaxation:
/// a full step lets a checkerboard wobble survive or grow).
const STEP: f64 = 0.6;

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
    // Per vertex, each incident quad as (next, opposite, previous) corners.
    let mut quads: Vec<Vec<[u32; 3]>> = vec![Vec::new(); n];
    for (f, w) in mesh.face_offsets.windows(2).enumerate() {
        let face = mesh.face_indices.get(w[0] as usize..w[1] as usize).unwrap_or_default();
        let patch = patch_of_face.get(f).copied().unwrap_or(u32::MAX);
        for (i, &v) in face.iter().enumerate() {
            let next = face[(i + 1) % face.len()];
            let prev = face[(i + face.len() - 1) % face.len()];
            if face.len() == 4
                && let Some(list) = quads.get_mut(v as usize)
            {
                list.push([next, face[(i + 2) % 4], prev]);
            }
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
                let laplacian = neighbours[v].iter().map(|&u| current[u as usize]).sum::<DVec3>()
                    / neighbours[v].len() as f64;
                // Where each incident quad would want v to be a parallelogram corner.
                let average = if quads[v].is_empty() {
                    laplacian
                } else {
                    let parallelogram = quads[v]
                        .iter()
                        .map(|&[a, o, b]| current[a as usize] + current[b as usize] - current[o as usize])
                        .sum::<DVec3>()
                        / quads[v].len() as f64;
                    laplacian.lerp(parallelogram, PARALLELOGRAM_WEIGHT)
                };
                let allowed: HashSet<u32> = patches[v].iter().copied().collect();
                let goal = p.lerp(average, STEP);
                let projected = surface.closest_point_where(goal, |t| {
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
