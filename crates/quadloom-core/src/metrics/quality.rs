//! Quad-mesh quality: element shape and vertex regularity.
//!
//! Shape metrics cover quads only; triangles and n-gons are counted but not
//! measured. Degenerate quads score as worst-case values, never NaN.

use crate::mesh::{DVec3, PolyMesh};

/// Aspect ratio reported for quads with a zero-length edge.
pub const MAX_ASPECT: f64 = 1e6;

/// Summary statistics over a set of per-element values.
#[derive(Clone, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Stats {
    pub min: Option<f64>,
    pub mean: Option<f64>,
    /// 5th percentile (nearest rank).
    pub p05: Option<f64>,
    /// 95th percentile (nearest rank).
    pub p95: Option<f64>,
    pub max: Option<f64>,
}

impl Stats {
    pub fn from_values(mut values: Vec<f64>) -> Self {
        if values.is_empty() {
            return Self::default();
        }
        values.sort_by(f64::total_cmp);
        let n = values.len();
        let rank = |q: f64| values[((q * n as f64).ceil() as usize).clamp(1, n) - 1];
        Self {
            min: values.first().copied(),
            mean: Some(values.iter().sum::<f64>() / n as f64),
            p05: Some(rank(0.05)),
            p95: Some(rank(0.95)),
            max: values.last().copied(),
        }
    }
}

/// Quality of a (mostly) quad mesh.
#[derive(Clone, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct QuadQuality {
    pub faces: usize,
    pub quads: usize,
    pub triangles: usize,
    pub ngons: usize,
    /// Every face is a quad (and there is at least one).
    pub pure_quad: bool,
    /// Vertices referenced by at least one face.
    pub vertices: usize,
    pub boundary_edges: usize,
    pub non_manifold_edges: usize,
    /// Interior vertices whose valence isn't 4.
    pub irregular_interior: usize,
    /// Boundary vertices whose valence isn't 2 (corner) or 3 (side).
    pub irregular_boundary: usize,
    pub singularities_per_1k_quads: Option<f64>,
    /// Minimum corner Jacobian per quad, in `[-1, 1]` (1 = perfect, ≤ 0 = inverted).
    pub scaled_jacobian: Stats,
    /// Mean absolute deviation of corner angles from 90°, per quad.
    pub angle_deviation_deg: Stats,
    /// Longest edge over shortest edge, per quad.
    pub aspect_ratio: Stats,
}

pub fn quad_quality(mesh: &PolyMesh) -> QuadQuality {
    let mut q = QuadQuality { faces: mesh.face_count(), ..QuadQuality::default() };
    let (mut jacobians, mut angles, mut aspects) = (Vec::new(), Vec::new(), Vec::new());
    for face in faces(mesh) {
        match face.len() {
            3 => q.triangles += 1,
            4 => {
                q.quads += 1;
                let corners = [0, 1, 2, 3].map(|i| mesh.positions[face[i] as usize]);
                let shape = quad_shape(corners);
                jacobians.push(shape.jacobian);
                angles.push(shape.angle_deviation_deg);
                aspects.push(shape.aspect);
            }
            _ => q.ngons += 1,
        }
    }
    q.pure_quad = q.quads > 0 && q.quads == q.faces;
    q.scaled_jacobian = Stats::from_values(jacobians);
    q.angle_deviation_deg = Stats::from_values(angles);
    q.aspect_ratio = Stats::from_values(aspects);

    let topology = Topology::of(mesh);
    q.vertices = topology.referenced;
    q.boundary_edges = topology.boundary_edges;
    q.non_manifold_edges = topology.non_manifold_edges;
    for (v, &valence) in topology.valence.iter().enumerate() {
        if valence == 0 {
            continue;
        }
        if topology.on_boundary[v] {
            q.irregular_boundary += usize::from(!(2..=3).contains(&valence));
        } else {
            q.irregular_interior += usize::from(valence != 4);
        }
    }
    q.singularities_per_1k_quads =
        (q.quads > 0).then(|| (q.irregular_interior + q.irregular_boundary) as f64 * 1000.0 / q.quads as f64);
    q
}

fn faces(mesh: &PolyMesh) -> impl Iterator<Item = &[u32]> {
    mesh.face_offsets
        .windows(2)
        .map(|w| mesh.face_indices.get(w[0] as usize..w[1] as usize).unwrap_or_default())
}

struct QuadShape {
    jacobian: f64,
    angle_deviation_deg: f64,
    aspect: f64,
}

fn quad_shape(p: [DVec3; 4]) -> QuadShape {
    let edges = [0, 1, 2, 3].map(|i| p[(i + 1) % 4] - p[i]);
    let lengths = edges.map(DVec3::length);
    let (shortest, longest) =
        lengths.iter().fold((f64::INFINITY, 0.0f64), |(lo, hi), &l| (lo.min(l), hi.max(l)));
    let aspect = if shortest > 0.0 { (longest / shortest).min(MAX_ASPECT) } else { MAX_ASPECT };

    let normal = (p[2] - p[0]).cross(p[3] - p[1]).normalize_or_zero();
    let mut jacobian = f64::INFINITY;
    let mut deviation_sum = 0.0;
    for i in 0..4 {
        let (next, prev) = (edges[i], -edges[(i + 3) % 4]);
        let scale = lengths[i] * lengths[(i + 3) % 4];
        if scale <= 0.0 || normal == DVec3::ZERO {
            jacobian = -1.0;
            deviation_sum += 90.0;
            continue;
        }
        jacobian = jacobian.min(next.cross(prev).dot(normal) / scale);
        let cos = (next.dot(prev) / scale).clamp(-1.0, 1.0);
        deviation_sum += (cos.acos().to_degrees() - 90.0).abs();
    }
    QuadShape { jacobian: jacobian.clamp(-1.0, 1.0), angle_deviation_deg: deviation_sum / 4.0, aspect }
}

/// Edge incidence and vertex valence, from sorted edge keys (deterministic).
struct Topology {
    valence: Vec<usize>,
    on_boundary: Vec<bool>,
    referenced: usize,
    boundary_edges: usize,
    non_manifold_edges: usize,
}

impl Topology {
    fn of(mesh: &PolyMesh) -> Self {
        let mut keys: Vec<(u32, u32)> = faces(mesh)
            .flat_map(|f| (0..f.len()).map(move |i| (f[i], f[(i + 1) % f.len()])))
            .filter(|(a, b)| a != b)
            .map(|(a, b)| (a.min(b), a.max(b)))
            .collect();
        keys.sort_unstable();

        let n = mesh.positions.len();
        let mut valence = vec![0usize; n];
        let mut on_boundary = vec![false; n];
        let (mut boundary_edges, mut non_manifold_edges) = (0, 0);
        for group in keys.chunk_by(|x, y| x == y) {
            let (a, b) = (group[0].0 as usize, group[0].1 as usize);
            if a >= n || b >= n {
                continue;
            }
            valence[a] += 1;
            valence[b] += 1;
            match group.len() {
                1 => {
                    boundary_edges += 1;
                    on_boundary[a] = true;
                    on_boundary[b] = true;
                }
                2 => {}
                _ => non_manifold_edges += 1,
            }
        }
        let referenced = valence.iter().filter(|&&v| v > 0).count();
        Self { valence, on_boundary, referenced, boundary_edges, non_manifold_edges }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::{DVec3, PolyMesh};

    fn poly(positions: &[[f64; 3]], faces: &[&[u32]]) -> PolyMesh {
        let mut offsets = vec![0];
        let mut indices = Vec::new();
        for f in faces {
            indices.extend_from_slice(f);
            offsets.push(indices.len() as u32);
        }
        PolyMesh {
            positions: positions.iter().map(|p| DVec3::from_array(*p)).collect(),
            face_offsets: offsets,
            face_indices: indices,
        }
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn unit_square_is_perfect() {
        let q =
            quad_quality(&poly(&[[0., 0., 0.], [1., 0., 0.], [1., 1., 0.], [0., 1., 0.]], &[&[0, 1, 2, 3]]));
        assert!(q.pure_quad);
        assert!(close(q.scaled_jacobian.min.unwrap(), 1.0));
        assert!(close(q.angle_deviation_deg.max.unwrap(), 0.0));
        assert!(close(q.aspect_ratio.max.unwrap(), 1.0));
    }

    #[test]
    fn rhombus_60deg_jacobian() {
        let (c, s) = (60f64.to_radians().cos(), 60f64.to_radians().sin());
        let q =
            quad_quality(&poly(&[[0., 0., 0.], [1., 0., 0.], [1. + c, s, 0.], [c, s, 0.]], &[&[0, 1, 2, 3]]));
        assert!((q.scaled_jacobian.min.unwrap() - s).abs() < 1e-9, "{:?}", q.scaled_jacobian);
        assert!(close(q.angle_deviation_deg.mean.unwrap(), 30.0));
    }

    #[test]
    fn concave_quad_negative_jacobian() {
        let q = quad_quality(&poly(
            &[[0., 0., 0.], [2., 0., 0.], [0.5, 0.5, 0.], [0., 2., 0.]],
            &[&[0, 1, 2, 3]],
        ));
        assert!(q.scaled_jacobian.min.unwrap() < 0.0, "{:?}", q.scaled_jacobian);
    }

    #[test]
    fn zero_length_edge_counts_as_worst() {
        let q =
            quad_quality(&poly(&[[0., 0., 0.], [1., 0., 0.], [1., 0., 0.], [0., 1., 0.]], &[&[0, 1, 2, 3]]));
        assert_eq!(q.scaled_jacobian.min, Some(-1.0));
        assert_eq!(q.aspect_ratio.max, Some(MAX_ASPECT));
        for stats in [&q.scaled_jacobian, &q.angle_deviation_deg, &q.aspect_ratio] {
            for v in [stats.min, stats.mean, stats.p05, stats.p95, stats.max].into_iter().flatten() {
                assert!(v.is_finite(), "{stats:?}");
            }
        }
    }

    #[test]
    fn grid_3x3_has_no_singularities() {
        let mut positions = Vec::new();
        for y in 0..4 {
            for x in 0..4 {
                positions.push([f64::from(x), f64::from(y), 0.0]);
            }
        }
        let faces: Vec<Vec<u32>> = (0..3u32)
            .flat_map(|y| {
                (0..3u32).map(move |x| vec![y * 4 + x, y * 4 + x + 1, (y + 1) * 4 + x + 1, (y + 1) * 4 + x])
            })
            .collect();
        let refs: Vec<&[u32]> = faces.iter().map(Vec::as_slice).collect();
        let q = quad_quality(&poly(&positions, &refs));
        assert_eq!(q.quads, 9);
        assert_eq!(q.boundary_edges, 12);
        assert_eq!(q.irregular_interior, 0);
        assert_eq!(q.irregular_boundary, 0);
        assert_eq!(q.singularities_per_1k_quads, Some(0.0));
    }

    #[test]
    fn cube_quads_have_8_valence3_singularities() {
        let p = [
            [0., 0., 0.],
            [1., 0., 0.],
            [1., 1., 0.],
            [0., 1., 0.],
            [0., 0., 1.],
            [1., 0., 1.],
            [1., 1., 1.],
            [0., 1., 1.],
        ];
        let faces: [&[u32]; 6] =
            [&[0, 3, 2, 1], &[4, 5, 6, 7], &[0, 1, 5, 4], &[1, 2, 6, 5], &[2, 3, 7, 6], &[3, 0, 4, 7]];
        let q = quad_quality(&poly(&p, &faces));
        assert_eq!(q.boundary_edges, 0);
        assert_eq!(q.non_manifold_edges, 0);
        assert_eq!(q.irregular_interior, 8);
        assert!(close(q.scaled_jacobian.min.unwrap(), 1.0));
    }

    #[test]
    fn mixed_faces_counted() {
        let p = [
            [0., 0., 0.],
            [1., 0., 0.],
            [1., 1., 0.],
            [0., 1., 0.],
            [2., 0., 0.],
            [2., 1., 0.],
            [3., 0.5, 0.],
        ];
        let q = quad_quality(&poly(&p, &[&[0, 1, 2], &[0, 2, 3, 3], &[1, 4, 6, 5, 2]]));
        assert_eq!((q.faces, q.quads, q.triangles, q.ngons), (3, 1, 1, 1));
        assert!(!q.pure_quad);
    }

    #[test]
    fn empty_mesh_stats_are_none() {
        let q = quad_quality(&PolyMesh::default());
        assert_eq!(q.faces, 0);
        assert!(!q.pure_quad);
        assert_eq!(q.scaled_jacobian.mean, None);
        assert_eq!(q.singularities_per_1k_quads, None);
    }

    #[test]
    fn stats_percentiles() {
        let s = Stats::from_values((1..=100).map(f64::from).collect());
        assert_eq!(s.min, Some(1.0));
        assert_eq!(s.max, Some(100.0));
        assert_eq!(s.mean, Some(50.5));
        assert_eq!(s.p05, Some(5.0));
        assert_eq!(s.p95, Some(95.0));
    }
}
