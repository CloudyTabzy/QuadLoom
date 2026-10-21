//! Target edge length and the per-vertex sizing field.

use glam::DVec3;

use super::RemeshParams;
use crate::edit::{EditMesh, VertexKind};

/// The sphericity-based length may not drop below this fraction of the
/// "about 10,000 faces" length. Open surfaces have no volume, which would
/// otherwise drive the target to zero.
const OPEN_SURFACE_FLOOR: f64 = 0.25;
/// Normal-deviation angle (radians) that one target-length edge may span
/// before adaptivity shortens edges.
const ANGLE_PER_EDGE: f64 = 0.3;

/// The global target edge length: explicit (`target_edge_rel` × diagonal) or
/// the oracle's `ExpectedEdgeL`: `min(√(A/2000·S²·2.309), √(A·2.309/10000))`
/// where `S` is the sphericity, with the first term floored.
pub fn target_length(positions: &[DVec3], triangles: &[[u32; 3]], params: &RemeshParams) -> f64 {
    if params.target_edge_rel > 0.0 {
        return params.target_edge_rel * super::bbox_diagonal(positions);
    }
    let (mut area, mut volume6) = (0.0, 0.0);
    for &[a, b, c] in triangles {
        let [pa, pb, pc] = [a, b, c].map(|v| positions[v as usize]);
        area += 0.5 * (pb - pa).cross(pc - pa).length();
        volume6 += pa.dot(pb.cross(pc));
    }
    if area <= 0.0 {
        return super::bbox_diagonal(positions) * 0.01;
    }
    let volume = volume6.abs() / 6.0;
    let sphericity = std::f64::consts::PI.cbrt() * (6.0 * volume).powf(2.0 / 3.0) / area;
    let l0 = (area / 2000.0 * sphericity * sphericity * 2.309).sqrt();
    let l1 = (area * 2.309 / 10_000.0).sqrt();
    l0.max(OPEN_SURFACE_FLOOR * l1).min(l1)
}

/// Desired edge length at every vertex slot of `mesh` (dead slots get `target`).
///
/// With adaptivity on, the length shrinks where the surface curves. A free
/// vertex whose incident face normals spread by angle θ (from their mean) over
/// edges of mean length ℓ has curvature κ ≈ 2θ/ℓ; the desired length is
/// `ANGLE_PER_EDGE / κ`, clamped to `[min_mult, max_mult] × target`. Feature
/// and corner vertices sit on creases, which are features rather than
/// curvature, so they take the mean of their free neighbours. The field is
/// then smoothed twice. This is QuadLoom's choice (decision 0004); the oracle
/// adapts to triangle quality instead.
pub fn vertex_sizes(mesh: &EditMesh, target: f64, params: &RemeshParams) -> Vec<f64> {
    let slots = mesh.vertex_slots();
    if !params.adaptive {
        return vec![target; slots];
    }
    let (lo, hi) = (params.min_mult * target, params.max_mult * target);
    let mut sizes = vec![target; slots];
    let mut measured = vec![false; slots];
    for v in 0..slots as u32 {
        if !mesh.is_vertex_alive(v) || mesh.kind(v) != VertexKind::Free {
            continue;
        }
        measured[v as usize] = true;
        let faces = mesh.faces_around(v);
        let normals: Vec<DVec3> = faces.iter().map(|&f| mesh.face_normal(f)).collect();
        let mean = normals.iter().copied().sum::<DVec3>().normalize_or_zero();
        let spread = normals.iter().map(|n| n.dot(mean).clamp(-1.0, 1.0).acos()).fold(0.0, f64::max);
        let ring: Vec<u32> = mesh.one_ring(v).collect();
        let p = mesh.position(v);
        let mean_len =
            ring.iter().map(|&n| (mesh.position(n) - p).length()).sum::<f64>() / ring.len().max(1) as f64;
        sizes[v as usize] = if spread > 1e-9 && mean_len > 0.0 {
            (ANGLE_PER_EDGE * mean_len / (2.0 * spread)).clamp(lo, hi)
        } else {
            hi
        };
    }
    // Crease vertices borrow their free neighbours' sizes.
    for v in 0..slots as u32 {
        if !mesh.is_vertex_alive(v) || measured[v as usize] {
            continue;
        }
        let free: Vec<f64> =
            mesh.one_ring(v).filter(|&n| measured[n as usize]).map(|n| sizes[n as usize]).collect();
        sizes[v as usize] =
            if free.is_empty() { target.clamp(lo, hi) } else { free.iter().sum::<f64>() / free.len() as f64 };
    }
    for _ in 0..2 {
        let previous = sizes.clone();
        for v in 0..slots as u32 {
            if !mesh.is_vertex_alive(v) {
                continue;
            }
            let ring: Vec<u32> = mesh.one_ring(v).collect();
            if ring.is_empty() {
                continue;
            }
            let neighbours = ring.iter().map(|&n| previous[n as usize]).sum::<f64>() / ring.len() as f64;
            sizes[v as usize] = 0.5 * (previous[v as usize] + neighbours);
        }
    }
    sizes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sphere(radius: f64) -> EditMesh {
        let (p, t) = crate::remesh::tests::sphere_mesh(8, 16);
        let p: Vec<DVec3> = p.into_iter().map(|q| q * radius).collect();
        EditMesh::from_triangles(&p, &t, &[]).unwrap()
    }

    #[test]
    fn adaptive_sizes_shrink_on_curved_regions() {
        let params = RemeshParams::default();
        let mean = |m: &EditMesh, target: f64| {
            let s = vertex_sizes(m, target, &params);
            s.iter().sum::<f64>() / s.len() as f64
        };
        // Same target, tighter curvature → shorter edges.
        let (tight, loose) = (sphere(1.0), sphere(4.0));
        assert!(mean(&tight, 1.0) < mean(&loose, 1.0));
        let off = RemeshParams { adaptive: false, ..params.clone() };
        assert!(vertex_sizes(&tight, 1.0, &off).iter().all(|&s| s == 1.0));
    }
}
