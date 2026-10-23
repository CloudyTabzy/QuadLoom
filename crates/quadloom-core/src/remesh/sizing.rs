//! Target edge length and the per-vertex sizing field.

use hashbrown::HashSet;

use glam::{DMat4, DVec3, DVec4};
use rayon::prelude::*;

use super::RemeshParams;
use crate::edit::{EditMesh, VertexKind};

/// The sphericity-based length may not drop below this fraction of the
/// "about 10,000 faces" length. Open surfaces have no volume, which would
/// otherwise drive the target to zero.
const OPEN_SURFACE_FLOOR: f64 = 0.25;
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
/// With adaptivity on, the length follows the allowed deviation `tolerance`:
/// an edge of length h on a surface of curvature radius r strays about
/// `h²/8r` from it, and Dunyach et al. (EG 2013) size edges as
/// `h = √(6·tolerance·r − 3·tolerance²)` so triangles stay within the tolerance.
/// r is the *maximum* curvature radius at each free vertex, from a local jet
/// fit (see [`Ball::curvature`]); h is clamped to `[min_mult, max_mult] ×
/// target`. Feature and corner vertices sit on creases, which are features
/// rather than curvature, so they take the mean of their free neighbours. The
/// field is then smoothed twice (decision 0004).
pub fn vertex_sizes(mesh: &EditMesh, target: f64, tolerance: f64, params: &RemeshParams) -> Vec<f64> {
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
            Some(error_bounded_length(k, tolerance).clamp(lo, hi))
        })
        .collect();
    let mut sizes: Vec<f64> = free_sizes.iter().map(|s| s.unwrap_or(target)).collect();
    let measured: Vec<bool> = free_sizes.iter().map(Option::is_some).collect();
    // Crease vertices borrow their free neighbours' sizes, but no more than
    // their own feature line's curvature allows: a long chord would cut a
    // curved crease just as it cuts a curved surface.
    for v in 0..slots as u32 {
        if !mesh.is_vertex_alive(v) || measured[v as usize] {
            continue;
        }
        let free: Vec<f64> =
            mesh.one_ring(v).filter(|&n| measured[n as usize]).map(|n| sizes[n as usize]).collect();
        let borrowed =
            if free.is_empty() { target.clamp(lo, hi) } else { free.iter().sum::<f64>() / free.len() as f64 };
        let line = match mesh.feature_neighbours(v).as_slice() {
            &[a, b] => {
                let reach = BALL_RADIUS * target;
                let k = menger_curvature(
                    mesh.walk_feature(v, a, reach),
                    mesh.position(v),
                    mesh.walk_feature(v, b, reach),
                );
                error_bounded_length(k, tolerance).clamp(lo, hi)
            }
            _ => hi,
        };
        sizes[v as usize] = borrowed.min(line);
    }
    for _ in 0..2 {
        let previous = sizes.clone();
        for v in 0..slots as u32 {
            if !mesh.is_vertex_alive(v) {
                continue;
            }
            let (sum, count) = mesh
                .one_ring(v)
                .fold((0.0, 0usize), |(sum, count), n| (sum + previous[n as usize], count + 1));
            if count == 0 {
                continue;
            }
            let neighbours = sum / count as f64;
            sizes[v as usize] = 0.5 * (previous[v as usize] + neighbours);
        }
    }
    sizes
}

/// Dunyach et al.'s edge length for curvature `k` and deviation `tolerance`
/// (infinite on flat surfaces; the tolerance itself where r < tolerance).
fn error_bounded_length(k: f64, tolerance: f64) -> f64 {
    if k <= 1e-12 {
        return f64::INFINITY;
    }
    let r = 1.0 / k;
    let squared = 6.0 * tolerance * r - 3.0 * tolerance * tolerance;
    if squared > 0.0 { squared.sqrt() } else { tolerance }
}

/// Curvature of the circle through three points (0 if they're collinear).
fn menger_curvature(a: DVec3, b: DVec3, c: DVec3) -> f64 {
    let product = (b - a).length() * (c - b).length() * (c - a).length();
    if product <= 0.0 { 0.0 } else { 2.0 * (b - a).cross(c - a).length() / product }
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
    offsets: Vec<DVec3>,
}

impl Ball {
    /// Maximum absolute curvature at free vertex `v`, judged over the free
    /// surface within `radius` (flooding through free vertices only, so it
    /// never crosses a crease; at least the one-ring, at most [`MAX_BALL`]
    /// samples).
    ///
    /// The samples' heights over the tangent plane (ball-mean normal) are fitted
    /// by least squares with `h = a·x² + b·xy + c·y² + d`, a degree-2 osculating
    /// jet (Cazals & Pouget, SGP 2003) plus a constant. The principal
    /// curvatures are the eigenvalues of `[[2a, b], [b, 2c]]`. Zero-mean noise
    /// finer than the ball doesn't correlate with the quadratic terms, so it
    /// ends up in `d` rather than in the curvature. When the samples can't
    /// determine a quadric (e.g. all on a line) the sagitta estimate
    /// `2·Σ|height| / Σ d²` is used instead.
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
        self.offsets.clear();
        self.offsets.extend(self.samples.iter().map(|&u| mesh.position(u) - p));
        jet_curvature(&self.offsets, n).unwrap_or_else(|| {
            let (height, d2) = self
                .offsets
                .iter()
                .fold((0.0, 0.0), |(h, d2), e| (h + e.dot(n).abs(), d2 + e.length_squared()));
            if d2 > 0.0 { 2.0 * height / d2 } else { 0.0 }
        })
    }
}

/// Maximum absolute principal curvature of the quadric `a·x² + b·xy + c·y² + d`
/// fitted to `offsets` (sample − centre) over the plane with normal `n`, or
/// `None` if the samples don't determine it. Coordinates are scaled to the
/// sample spread so the normal equations stay well conditioned.
fn jet_curvature(offsets: &[DVec3], n: DVec3) -> Option<f64> {
    if offsets.len() < 5 {
        return None;
    }
    let (t1, t2) = n.any_orthonormal_pair();
    let scale = offsets.iter().map(|e| e.length()).fold(0.0, f64::max);
    if scale <= 0.0 {
        return None;
    }
    let (mut m, mut rhs) = (DMat4::ZERO, DVec4::ZERO);
    for e in offsets {
        // Tangent direction stretched to the chord length: on a sphere a chord
        // d long rises exactly d²/2R, so the quadric is exact at any ball size
        // (the plain projection is only exact to second order).
        let (px, py) = (e.dot(t1), e.dot(t2));
        let stretch = e.length() / px.hypot(py).max(f64::MIN_POSITIVE);
        let (x, y, h) = (px * stretch / scale, py * stretch / scale, e.dot(n) / scale);
        let row = DVec4::new(x * x, x * y, y * y, 1.0);
        m += DMat4::from_cols(row * row.x, row * row.y, row * row.z, row * row.w);
        rhs += row * h;
    }
    // Determinant of the (scaled, so O(1)-sized) normal matrix: near zero when
    // the samples don't span both tangent directions.
    if m.determinant().abs() < 1e-9 * (offsets.len() as f64).powi(4) {
        return None;
    }
    let [a, b, c, _] = (m.inverse() * rhs).to_array();
    let (mean, half_gap) = (a + c, ((a - c) * (a - c) + b * b).sqrt());
    Some(((mean + half_gap).abs().max((mean - half_gap).abs())) / scale)
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
            let s = vertex_sizes(m, target, 0.01, &params);
            s.iter().sum::<f64>() / s.len() as f64
        };
        // Same target, tighter curvature → shorter edges.
        let (tight, loose) = (sphere(1.0), sphere(4.0));
        assert!(mean(&tight, 1.0) < mean(&loose, 1.0));
        let off = RemeshParams { adaptive: false, ..params.clone() };
        assert!(vertex_sizes(&tight, 1.0, 0.01, &off).iter().all(|&s| s == 1.0));
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
    fn ball_curvature_finds_the_maximum_on_a_cylinder() {
        // Radius 0.5, length 2: principal curvatures 2 (around) and 0 (along).
        let (rings, segments) = (40u32, 48u32);
        let mut p = Vec::new();
        for i in 0..=rings {
            for j in 0..segments {
                let a = std::f64::consts::TAU * f64::from(j) / f64::from(segments);
                p.push(DVec3::new(0.5 * a.cos(), 0.5 * a.sin(), 2.0 * f64::from(i) / f64::from(rings)));
            }
        }
        let mut t = Vec::new();
        for i in 0..rings {
            for j in 0..segments {
                let (a, b) = (i * segments + j, i * segments + (j + 1) % segments);
                t.extend([[a, b, b + segments], [a, b + segments, a + segments]]);
            }
        }
        let mesh = EditMesh::from_triangles(&p, &t, &[]).unwrap();
        let normals = vertex_normals(&mesh);
        let k = Ball::default().curvature(&mesh, &normals, (rings / 2) * segments, 0.3);
        assert!((k - 2.0).abs() < 0.2, "κ_max = {k}, expected 2");
    }

    #[test]
    fn menger_curvature_of_a_circle() {
        let on = |a: f64| DVec3::new(2.0 * a.cos(), 2.0 * a.sin(), 0.0);
        assert!((menger_curvature(on(0.0), on(0.3), on(1.1)) - 0.5).abs() < 1e-12);
        assert_eq!(menger_curvature(DVec3::ZERO, DVec3::X, 2.0 * DVec3::X), 0.0);
    }

    #[test]
    fn crease_vertices_follow_their_line_curvature() {
        // A cylinder's rim circle (radius 0.5) is a feature line on a surface
        // that is flat along the rim's band only when far from it; sizes at the
        // rim may not exceed what the circle's curvature allows.
        let (rings, segments) = (6u32, 96u32);
        let mut p = Vec::new();
        for i in 0..=rings {
            for j in 0..segments {
                let a = std::f64::consts::TAU * f64::from(j) / f64::from(segments);
                p.push(DVec3::new(0.5 * a.cos(), 0.5 * a.sin(), 0.05 * f64::from(i)));
            }
        }
        let mut t = Vec::new();
        for i in 0..rings {
            for j in 0..segments {
                let (a, b) = (i * segments + j, i * segments + (j + 1) % segments);
                t.extend([[a, b, b + segments], [a, b + segments, a + segments]]);
            }
        }
        let mesh = EditMesh::from_triangles(&p, &t, &[]).unwrap();
        let params = RemeshParams { max_mult: 10.0, min_mult: 0.01, ..RemeshParams::default() };
        let tolerance = 0.001;
        let sizes = vertex_sizes(&mesh, 1.0, tolerance, &params);
        let allowed = error_bounded_length(2.0, tolerance);
        assert!(sizes[0] <= 1.05 * allowed, "rim size {} vs {allowed}", sizes[0]);
    }

    #[test]
    fn error_bounded_length_follows_dunyach() {
        // r = 1, ε = 0.01: h = √(0.06 − 0.0003).
        assert!((error_bounded_length(1.0, 0.01) - 0.0597_f64.sqrt()).abs() < 1e-12);
        assert_eq!(error_bounded_length(0.0, 0.01), f64::INFINITY);
        assert_eq!(error_bounded_length(1000.0, 0.01), 0.01, "r below the tolerance");
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
        let sizes = vertex_sizes(&mesh, 0.1, 1.4 / 2500.0, &params);
        let centre = (n / 2) * w + n / 2;
        assert!(sizes[centre as usize] > 0.1, "noise shrank the size to {}", sizes[centre as usize]);
        assert!(ring_curvature(&mesh, centre) > 10.0, "the one-ring alone sees the noise as curvature");
    }
}
