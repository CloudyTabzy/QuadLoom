//! Two-sided surface distance between a result and the mesh it was made from.

use rayon::prelude::*;

use crate::mesh::{DVec3, InputMesh, PolyMesh};
use crate::spatial::TriangleBvh;

/// Distances relative to the reference's bounding-box diagonal.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct SurfaceDistance {
    /// max(result→reference, reference→result).
    pub hausdorff: f64,
    /// Mean over all samples of both directions.
    pub mean: f64,
    pub result_to_reference_max: f64,
    pub reference_to_result_max: f64,
}

/// Samples each mesh at its vertices and face centroids and measures the
/// distance to the other mesh's surface. Infinite if either side is empty.
pub fn surface_distance(result: &PolyMesh, reference: &InputMesh) -> SurfaceDistance {
    let result_tris = fan_triangles(result);
    let result_bvh = TriangleBvh::new(&result.positions, &result_tris);
    let reference_bvh = TriangleBvh::new(reference.positions(), reference.triangles());

    let result_samples = samples(&result.positions, polygon_centroids(result));
    let reference_centroids = reference
        .triangles()
        .iter()
        .map(|t| t.iter().map(|&i| reference.positions()[i as usize]).sum::<DVec3>() / 3.0)
        .collect();
    let reference_samples = samples(reference.positions(), reference_centroids);

    let forward = distances(&reference_bvh, &result_samples);
    let backward = distances(&result_bvh, &reference_samples);

    let scale = reference_bvh.bounds().map(|(lo, hi)| (hi - lo).length()).filter(|d| *d > 0.0).unwrap_or(1.0);
    let max = |d: &[f64]| d.iter().copied().fold(if d.is_empty() { f64::INFINITY } else { 0.0 }, f64::max);
    let (fwd_max, bwd_max) = (max(&forward), max(&backward));
    let count = forward.len() + backward.len();
    let mean = if count == 0 || forward.is_empty() || backward.is_empty() {
        f64::INFINITY
    } else {
        forward.iter().chain(&backward).sum::<f64>() / count as f64
    };
    SurfaceDistance {
        hausdorff: fwd_max.max(bwd_max) / scale,
        mean: mean / scale,
        result_to_reference_max: fwd_max / scale,
        reference_to_result_max: bwd_max / scale,
    }
}

fn samples(vertices: &[DVec3], centroids: Vec<DVec3>) -> Vec<DVec3> {
    let mut all = vertices.to_vec();
    all.extend(centroids);
    all
}

/// Distance from each sample to `bvh`, computed in parallel. `INFINITY` if the BVH is empty.
fn distances(bvh: &TriangleBvh, samples: &[DVec3]) -> Vec<f64> {
    samples.par_iter().map(|&p| bvh.closest_point(p).map_or(f64::INFINITY, |(_, d)| d)).collect()
}

fn polygons(mesh: &PolyMesh) -> impl Iterator<Item = &[u32]> {
    mesh.face_offsets
        .windows(2)
        .map(|w| mesh.face_indices.get(w[0] as usize..w[1] as usize).unwrap_or_default())
}

fn polygon_centroids(mesh: &PolyMesh) -> Vec<DVec3> {
    polygons(mesh)
        .filter(|f| !f.is_empty())
        .map(|f| f.iter().map(|&i| mesh.positions[i as usize]).sum::<DVec3>() / f.len() as f64)
        .collect()
}

fn fan_triangles(mesh: &PolyMesh) -> Vec<[u32; 3]> {
    polygons(mesh).flat_map(|f| (1..f.len().saturating_sub(1)).map(move |i| [f[0], f[i], f[i + 1]])).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::{InputMesh, PolyMesh};

    /// A `n`×`n` grid of unit cells on the plane z = `z`, as triangles.
    fn plane(n: u32, z: f32) -> InputMesh {
        let mut p = Vec::new();
        for y in 0..=n {
            for x in 0..=n {
                p.extend([x as f32, y as f32, z]);
            }
        }
        let w = n + 1;
        let mut i = Vec::new();
        for y in 0..n {
            for x in 0..n {
                let a = y * w + x;
                i.extend([a, a + 1, a + w + 1, a, a + w + 1, a + w]);
            }
        }
        InputMesh::from_buffers(&p, &i).unwrap()
    }

    #[test]
    fn identical_meshes_have_zero_distance() {
        let m = plane(4, 0.0);
        let d = surface_distance(&PolyMesh::from_triangles(&m), &m);
        assert!(d.hausdorff < 1e-12, "{d:?}");
        assert!(d.mean < 1e-12, "{d:?}");
    }

    #[test]
    fn offset_plane_distance() {
        let reference = plane(4, 0.0);
        // Diagonal of the 4x4 reference plane is 4√2; offset by 0.1 of that.
        let offset = 0.1 * 4.0 * std::f32::consts::SQRT_2;
        let result = PolyMesh::from_triangles(&plane(4, offset));
        let d = surface_distance(&result, &reference);
        assert!((d.hausdorff - 0.1).abs() < 1e-6, "{d:?}");
        assert!((d.mean - 0.1).abs() < 1e-6, "{d:?}");
        assert!((d.result_to_reference_max - d.reference_to_result_max).abs() < 1e-6);
    }

    #[test]
    fn empty_result_has_no_distance() {
        let d = surface_distance(&PolyMesh::default(), &plane(2, 0.0));
        assert!(d.hausdorff.is_infinite());
    }

    #[test]
    #[ignore = "performance check; run with --release -- --ignored"]
    fn perf_100k_queries_under_2s() {
        let reference = plane(224, 0.0); // ~100k triangles
        let result = PolyMesh::from_triangles(&plane(160, 0.01));
        let started = std::time::Instant::now();
        let _ = surface_distance(&result, &reference);
        assert!(started.elapsed().as_secs_f64() < 2.0, "{:?}", started.elapsed());
    }
}
