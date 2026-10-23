//! Curvature aspect targets for the rectangular frame (decision 0007, point 6).
//!
//! A smoothed per-face curvature tensor (normal cycle: dihedral angle × edge
//! length × ê êᵀ, averaged at the vertices and smoothed) gives the normal
//! curvatures along a face's two frame axes. The target for v is the log half
//! aspect ratio of those curvatures, clamped, so that quads come out short
//! across high curvature and long along low curvature.

use glam::{DMat3, DVec3};

use crate::control::{Control, Stage};
use crate::error::Error;
use crate::trace::graph::CANCEL_EVERY;

/// Smoothing passes applied to the vertex curvature tensors.
pub const SMOOTHING_STEPS: usize = 10;

/// Floor added to the curvatures, relative to their median (keeps flat faces at 0).
const FLOOR_FRACTION: f64 = 0.05;
/// Absolute floor, so a fully flat mesh still has a defined ratio.
const FLOOR_ABSOLUTE: f64 = 1e-12;

/// Per-face target for v (log half aspect ratio along X1 = `directions[f]` vs
/// X2 = n × X1), clamped to ±½ln(max_stretch). Non-finite targets become 0.
///
/// Errors only when cancelled.
pub fn aspect_targets(
    positions: &[DVec3],
    triangles: &[[u32; 3]],
    directions: &[DVec3],
    max_stretch: f64,
    smoothing: usize,
    control: &Control,
) -> Result<Vec<f64>, Error> {
    let stage = Stage::Frame;
    control.checkpoint(stage)?;
    let nf = triangles.len();
    let nv = positions.len();
    if nf == 0 {
        return Ok(Vec::new());
    }
    let point = |v: u32| positions.get(v as usize).copied().unwrap_or(DVec3::ZERO);

    // Unit normals and areas per face.
    let mut normal = Vec::with_capacity(nf);
    let mut area = Vec::with_capacity(nf);
    for t in triangles {
        let [a, b, c] = t.map(point);
        let cross = (b - a).cross(c - a);
        normal.push(cross.normalize_or_zero());
        area.push(0.5 * cross.length());
    }

    // Each undirected edge with its two faces, in sorted order. `forward` is
    // true when the face traverses lo → hi.
    let mut halfedges: Vec<(u32, u32, u32, bool)> = Vec::with_capacity(3 * nf);
    for (f, t) in triangles.iter().enumerate() {
        for c in 0..3 {
            let (p, q) = (t[c], t[(c + 1) % 3]);
            halfedges.push((p.min(q), p.max(q), f as u32, p < q));
        }
    }
    halfedges.sort_unstable();

    // Vertex normal-cycle tensors: ½·dihedral·length·ê êᵀ from each interior edge.
    let mut tensor = vec![DMat3::ZERO; nv];
    let mut vertex_area = vec![0.0; nv];
    for (f, t) in triangles.iter().enumerate() {
        for &v in t {
            if let (Some(slot), Some(a)) = (vertex_area.get_mut(v as usize), area.get(f)) {
                *slot += a / 3.0;
            }
        }
    }
    let mut i = 0;
    while i < halfedges.len() {
        tick(i, control)?;
        let (lo, hi) = (halfedges[i].0, halfedges[i].1);
        let mut j = i;
        while j < halfedges.len() && halfedges[j].0 == lo && halfedges[j].1 == hi {
            j += 1;
        }
        let group = &halfedges[i..j];
        i = j;
        // Interior manifold edges only: exactly one face on each side.
        let [first, second] = group else { continue };
        if first.3 == second.3 {
            continue;
        }
        let (fwd, bwd) = if first.3 { (first.2, second.2) } else { (second.2, first.2) };
        let (Some(&n_f), Some(&n_g)) = (normal.get(fwd as usize), normal.get(bwd as usize)) else {
            continue;
        };
        let (pa, pb) = (point(lo), point(hi));
        let len = (pb - pa).length();
        if len == 0.0 {
            continue;
        }
        let e = (pb - pa) / len;
        let beta = n_f.cross(n_g).dot(e).atan2(n_f.dot(n_g));
        let m = DMat3::from_cols(e * e.x, e * e.y, e * e.z) * (0.5 * beta * len);
        for v in [lo, hi] {
            if let Some(slot) = tensor.get_mut(v as usize) {
                *slot += m;
            }
        }
    }
    // Divide by the vertex area (the tensor is then a density).
    for (t, a) in tensor.iter_mut().zip(&vertex_area) {
        *t = if *a > 0.0 { *t * (1.0 / a) } else { DMat3::ZERO };
    }

    // One-ring Jacobi smoothing.
    let mut neighbours: Vec<Vec<u32>> = vec![Vec::new(); nv];
    let mut edges: Vec<(u32, u32)> = halfedges.iter().map(|h| (h.0, h.1)).collect();
    edges.dedup();
    for &(a, b) in &edges {
        if let Some(list) = neighbours.get_mut(a as usize) {
            list.push(b);
        }
        if let Some(list) = neighbours.get_mut(b as usize) {
            list.push(a);
        }
    }
    for _ in 0..smoothing {
        control.checkpoint(stage)?;
        let mut next = Vec::with_capacity(nv);
        for (v, list) in neighbours.iter().enumerate() {
            let mut acc = tensor.get(v).copied().unwrap_or(DMat3::ZERO);
            for &n in list {
                acc += tensor.get(n as usize).copied().unwrap_or(DMat3::ZERO);
            }
            next.push(acc * (1.0 / (1 + list.len()) as f64));
        }
        tensor = next;
    }

    // Face tensor = mean of its corners; normal curvatures along X1 and X2.
    let mut kappas = Vec::with_capacity(nf);
    for (f, t) in triangles.iter().enumerate() {
        tick(f, control)?;
        let mean = t.iter().map(|&v| tensor.get(v as usize).copied().unwrap_or(DMat3::ZERO)).sum::<DMat3>()
            * (1.0 / 3.0);
        let n = normal.get(f).copied().unwrap_or(DVec3::ZERO);
        let x1 = directions.get(f).copied().unwrap_or(DVec3::ZERO);
        let x2 = n.cross(x1);
        // The tensor measures bending across an edge, so the curvature along
        // X_i is read from the tangent n × X_i.
        let curvature = |x: DVec3| {
            let w = n.cross(x);
            (w.dot(mean * w)).abs()
        };
        kappas.push((curvature(x1), curvature(x2)));
    }

    // Floor from the median mean curvature, then the log ratio, clamped.
    let mut mean_curvatures: Vec<f64> = kappas.iter().map(|(a, b)| 0.5 * (a + b)).collect();
    mean_curvatures.sort_unstable_by(f64::total_cmp);
    let median = mean_curvatures.get(nf / 2).copied().unwrap_or(0.0);
    let floor = FLOOR_FRACTION * median + FLOOR_ABSOLUTE;
    let limit = 0.5 * max_stretch.max(1.0).ln();
    let targets = kappas
        .iter()
        .map(|&(k1, k2)| {
            let raw = 0.5 * ((k1 + floor) / (k2 + floor)).ln();
            let clamped = raw.max(-limit).min(limit);
            if clamped.is_finite() { clamped } else { 0.0 }
        })
        .collect();
    Ok(targets)
}

/// Checks for cancellation every `CANCEL_EVERY` items.
fn tick(i: usize, control: &Control) -> Result<(), Error> {
    if i.is_multiple_of(CANCEL_EVERY as usize) { control.checkpoint(Stage::Frame) } else { Ok(()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::field::FaceFrames;
    use crate::testmesh;

    /// Vertices on an open boundary (used by one face only).
    fn rim_vertices(triangles: &[[u32; 3]], nv: usize) -> Vec<bool> {
        let mut edges: Vec<(u32, u32)> = triangles
            .iter()
            .flat_map(|t| (0..3).map(move |c| (t[c].min(t[(c + 1) % 3]), t[c].max(t[(c + 1) % 3]))))
            .collect();
        edges.sort_unstable();
        let mut rim = vec![false; nv];
        let mut i = 0;
        while i < edges.len() {
            let mut j = i;
            while j < edges.len() && edges[j] == edges[i] {
                j += 1;
            }
            if j - i == 1 {
                for v in [edges[i].0, edges[i].1] {
                    if let Some(r) = rim.get_mut(v as usize) {
                        *r = true;
                    }
                }
            }
            i = j;
        }
        rim
    }

    #[test]
    fn cylinder_targets_hit_the_clamp() {
        let (p, t) = testmesh::cylinder(8, 32);
        let axis = vec![DVec3::Z; t.len()];
        let targets = aspect_targets(&p, &t, &axis, 3.0, SMOOTHING_STEPS, &Control::default()).unwrap();
        let rim = rim_vertices(&t, p.len());
        let limit = 0.5 * 3.0_f64.ln();
        let mut checked = 0;
        for (f, tri) in t.iter().enumerate() {
            if tri.iter().any(|&v| rim.get(v as usize).copied().unwrap_or(true)) {
                continue;
            }
            checked += 1;
            let target = targets[f];
            assert!((target + limit).abs() < 1e-3, "face {f}: target {target}, expected {}", -limit);
        }
        assert!(checked > 0, "no interior faces on the cylinder");
    }

    #[test]
    fn sphere_targets_are_near_zero() {
        let (p, t) = testmesh::icosphere(3);
        let dirs = FaceFrames::new(&p, &t).e1;
        let targets = aspect_targets(&p, &t, &dirs, 3.0, SMOOTHING_STEPS, &Control::default()).unwrap();
        for (f, &target) in targets.iter().enumerate() {
            assert!(target.abs() < 0.1, "face {f}: target {target}");
        }
    }

    #[test]
    fn flat_grid_targets_are_zero() {
        let (p, t) = testmesh::grid(6, 6);
        let dirs = FaceFrames::new(&p, &t).e1;
        let targets = aspect_targets(&p, &t, &dirs, 3.0, SMOOTHING_STEPS, &Control::default()).unwrap();
        assert!(targets.iter().all(|&target| target == 0.0), "{targets:?}");
    }
}
