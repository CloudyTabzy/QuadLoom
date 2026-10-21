//! Target edge length and the per-vertex sizing field.

use std::collections::HashSet;

use glam::DVec3;
use rayon::prelude::*;

use super::RemeshParams;
use crate::edit::{EditMesh, VertexKind};

/// The sphericity-based length may not drop below this fraction of the
/// "about 10,000 faces" length. Open surfaces have no volume, which would
/// otherwise drive the target to zero.
const OPEN_SURFACE_FLOOR: f64 = 0.25;
/// Normal-deviation angle (radians) that one target-length edge may span
/// before adaptivity shortens edges.
const ANGLE_PER_EDGE: f64 = 0.3;
/// Curvature is measured over vertices within this fraction of the target
/// length, so tessellation noise finer than the target averages out.
const BALL_RADIUS: f64 = 0.5;
/// At most this many samples per curvature ball: bounds the cost on input
/// much denser than the target (later, coarser iterations see the full ball).
const MAX_BALL: usize = 64;
/// Below this agreement (|Σ normals| / Σ |normals|) the ball isn't a height
/// field over its mean plane (the surface wraps within it), so the one-ring
/// estimate is used instead.
const MIN_COHERENCE: f64 = 0.5;

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
/// With adaptivity on, the length shrinks where the surface curves: the
/// desired length is `ANGLE_PER_EDGE / κ`, clamped to `[min_mult, max_mult] ×
/// target`, with κ measured at each free vertex over the nearby free surface
/// (see [`Ball::curvature`]). Feature and corner vertices sit on creases, which
/// are features rather than curvature, so they take the mean of their free
/// neighbours. The field is then smoothed twice. This is QuadLoom's choice
/// (decision 0004); the oracle adapts to triangle quality instead.
pub fn vertex_sizes(mesh: &EditMesh, target: f64, params: &RemeshParams) -> Vec<f64> {
    let slots = mesh.vertex_slots();
    if !params.adaptive {
        return vec![target; slots];
    }
    let (lo, hi) = (params.min_mult * target, params.max_mult * target);
    let normals = vertex_normals(mesh);
    let free_sizes: Vec<Option<f64>> = (0..slots as u32)
        .into_par_iter()
        .map_init(Ball::default, |ball, v| {
            if !mesh.is_vertex_alive(v) || mesh.kind(v) != VertexKind::Free {
                return None;
            }
            let k = ball.curvature(mesh, &normals, v, BALL_RADIUS * target);
            Some(if k > 1e-12 { (ANGLE_PER_EDGE / k).clamp(lo, hi) } else { hi })
        })
        .collect();
    let mut sizes: Vec<f64> = free_sizes.iter().map(|s| s.unwrap_or(target)).collect();
    let measured: Vec<bool> = free_sizes.iter().map(Option::is_some).collect();
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

/// Area-weighted (unnormalized) vertex normals.
fn vertex_normals(mesh: &EditMesh) -> Vec<DVec3> {
    let mut normals = vec![DVec3::ZERO; mesh.vertex_slots()];
    for f in mesh.faces() {
        let corners = mesh.face_corners(f);
        let [a, b, c] = corners.map(|v| mesh.position(v));
        let n = (b - a).cross(c - a);
        for v in corners {
            normals[v as usize] += n;
        }
    }
    normals
}

/// Reusable scratch for [`Ball::curvature`].
#[derive(Default)]
struct Ball {
    seen: HashSet<u32>,
    queue: Vec<u32>,
    samples: Vec<u32>,
}

impl Ball {
    /// Curvature at free vertex `v`, judged over the free surface within
    /// `radius` (flooding through free vertices only, so it never crosses a
    /// crease; at least the one-ring, at most [`MAX_BALL`] samples).
    ///
    /// Against the plane through `v` with the ball's mean normal, a sample at
    /// distance d on a sphere of radius R sits d²/2R off the plane, so
    /// κ = 2·Σ height / Σ d²: exact on spheres at any sampling, while noise much
    /// finer than the ball only adds its own (small) amplitude to the heights.
    fn curvature(&mut self, mesh: &EditMesh, normals: &[DVec3], v: u32, radius: f64) -> f64 {
        let p = mesh.position(v);
        self.seen.clear();
        self.queue.clear();
        self.samples.clear();
        self.seen.insert(v);
        self.queue.push(v);
        let (mut normal, mut magnitude) = (DVec3::ZERO, 0.0);
        let mut head = 0;
        while let Some(&u) = self.queue.get(head) {
            head += 1;
            normal += normals[u as usize];
            magnitude += normals[u as usize].length();
            for n in mesh.one_ring(u) {
                if self.samples.len() >= MAX_BALL {
                    break;
                }
                if !self.seen.insert(n) {
                    continue;
                }
                self.samples.push(n);
                if mesh.kind(n) == VertexKind::Free && (mesh.position(n) - p).length() < radius {
                    self.queue.push(n);
                }
            }
        }
        if magnitude <= 0.0 || normal.length() < MIN_COHERENCE * magnitude {
            return ring_curvature(mesh, v);
        }
        let n = normal.normalize();
        let (height, d2) = self.samples.iter().fold((0.0, 0.0), |(h, d2), &u| {
            let e = mesh.position(u) - p;
            (h + e.dot(n).abs(), d2 + e.length_squared())
        });
        if d2 > 0.0 { 2.0 * height / d2 } else { 0.0 }
    }
}

/// One-ring curvature: a vertex whose incident face normals spread by angle θ
/// (from their mean) over edges of mean length ℓ has κ ≈ 2θ/ℓ.
fn ring_curvature(mesh: &EditMesh, v: u32) -> f64 {
    let normals: Vec<DVec3> = mesh.faces_around(v).iter().map(|&f| mesh.face_normal(f)).collect();
    let mean = normals.iter().copied().sum::<DVec3>().normalize_or_zero();
    let spread = normals.iter().map(|n| n.dot(mean).clamp(-1.0, 1.0).acos()).fold(0.0, f64::max);
    let p = mesh.position(v);
    let ring: Vec<u32> = mesh.one_ring(v).collect();
    let mean_len =
        ring.iter().map(|&n| (mesh.position(n) - p).length()).sum::<f64>() / ring.len().max(1) as f64;
    if mean_len > 0.0 { 2.0 * spread / mean_len } else { 0.0 }
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

    #[test]
    fn ball_curvature_is_exact_on_a_sphere() {
        for radius in [0.5, 2.0] {
            let mesh = sphere(radius);
            let normals = vertex_normals(&mesh);
            let mut ball = Ball::default();
            // A vertex away from the poles; the ball spans a few rings.
            let k = ball.curvature(&mesh, &normals, 40, 0.6 * radius);
            assert!((k * radius - 1.0).abs() < 0.1, "κ·R = {} at R = {radius}", k * radius);
        }
    }

    #[test]
    fn fine_corrugation_is_not_curvature() {
        // A flat 1×1 grid of 100×100 cells whose rows alternate z = ±0.001:
        // neighbouring faces meet at a sharp angle, yet at a 0.1 target the
        // surface is flat, so sizes stay above the target.
        let n = 100u32;
        let mut p = Vec::new();
        for y in 0..=n {
            for x in 0..=n {
                let z = if y % 2 == 0 { 0.001 } else { -0.001 };
                p.push(DVec3::new(f64::from(x) / f64::from(n), f64::from(y) / f64::from(n), z));
            }
        }
        let w = n + 1;
        let mut t = Vec::new();
        for y in 0..n {
            for x in 0..n {
                let a = y * w + x;
                t.extend([[a, a + 1, a + w + 1], [a, a + w + 1, a + w]]);
            }
        }
        let mesh = EditMesh::from_triangles(&p, &t, &[]).unwrap();
        let params = RemeshParams { max_mult: 3.0, ..RemeshParams::default() };
        let sizes = vertex_sizes(&mesh, 0.1, &params);
        let centre = (n / 2) * w + n / 2;
        assert!(sizes[centre as usize] > 0.1, "noise shrank the size to {}", sizes[centre as usize]);
        assert!(ring_curvature(&mesh, centre) > 10.0, "the one-ring alone sees the noise as curvature");
    }
}
