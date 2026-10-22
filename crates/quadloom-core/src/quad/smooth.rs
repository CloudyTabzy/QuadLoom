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
    untangle(mesh, fixed, &neighbours, &patches, trace, &surface);
}

/// Rounds of untangling after smoothing.
const UNTANGLE_ROUNDS: usize = 30;

/// Quads with a corner that turns the wrong way against the surface (reflex
/// or flipped) get their free vertices relaxed with full Laplacian steps (projected back onto their patches)
/// until none is inverted or the rounds run out.
fn untangle(
    mesh: &mut PolyMesh,
    fixed: &[bool],
    neighbours: &[Vec<u32>],
    patches: &[Vec<u32>],
    trace: &TraceOutput,
    surface: &TriangleBvh,
) {
    let tm = &trace.mesh;
    let face_normal: Vec<DVec3> = tm
        .triangles
        .iter()
        .map(|t| {
            let [a, b, c] = t.map(|v| tm.positions.get(v as usize).copied().unwrap_or(DVec3::ZERO));
            (b - a).cross(c - a).normalize_or_zero()
        })
        .collect();
    let project = |v: usize, goal: DVec3| -> Option<(DVec3, DVec3)> {
        let allowed: HashSet<u32> = patches[v].iter().copied().collect();
        let (q, _, t) = surface.closest_triangle_where(goal, |t| {
            trace.patch_of_face.get(t as usize).is_some_and(|p| allowed.contains(p))
        })?;
        Some((q, face_normal.get(t as usize).copied().unwrap_or(DVec3::ZERO)))
    };
    let faces: Vec<&[u32]> = mesh
        .face_offsets
        .windows(2)
        .map(|w| mesh.face_indices.get(w[0] as usize..w[1] as usize).unwrap_or_default())
        .collect();
    for _ in 0..UNTANGLE_ROUNDS {
        let positions = &mesh.positions;
        let normals: Vec<DVec3> = (0..positions.len())
            .into_par_iter()
            .map(|v| project(v, positions[v]).map_or(DVec3::ZERO, |(_, n)| n))
            .collect();
        let mut tangled = vec![false; positions.len()];
        let mut any = false;
        for face in &faces {
            if face.len() != 4 {
                continue;
            }
            let q = [face[0], face[1], face[2], face[3]].map(|v| positions[v as usize]);
            let normal = face.iter().map(|&v| normals[v as usize]).sum::<DVec3>();
            // Tangled: a corner turns the wrong way against the surface (a reflex
            // or flipped corner).
            let reflex = (0..4).any(|k| {
                let (prev, here, next) = (q[(k + 3) % 4], q[k], q[(k + 1) % 4]);
                (next - here).cross(prev - here).dot(normal) <= 0.0
            });
            if reflex {
                for &v in face.iter() {
                    if !fixed.get(v as usize).copied().unwrap_or(true) {
                        tangled[v as usize] = true;
                        any = true;
                    }
                }
            }
        }
        if !any {
            break;
        }
        let next: Vec<DVec3> = (0..positions.len())
            .into_par_iter()
            .map(|v| {
                let p = positions[v];
                if !tangled[v] || neighbours[v].is_empty() {
                    return p;
                }
                let average = neighbours[v].iter().map(|&u| positions[u as usize]).sum::<DVec3>()
                    / neighbours[v].len() as f64;
                match project(v, average) {
                    Some((q, _)) if q.is_finite() => q,
                    _ => p,
                }
            })
            .collect();
        mesh.positions = next;
    }
}
