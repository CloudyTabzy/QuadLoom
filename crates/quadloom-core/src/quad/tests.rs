//! End-to-end checks of the quad stages through the pipeline.

use std::collections::HashMap;

use glam::DVec3;

use crate::control::Control;
use crate::features::FeatureInput;
use crate::mesh::PolyMesh;
use crate::params::Params;
use crate::pipeline::{RunOutput, run};
use crate::testmesh::{cube, icosphere, torus};

fn run_mesh(p: &[DVec3], t: &[[u32; 3]], params: &Params) -> RunOutput {
    let positions: Vec<f32> = p.iter().flat_map(|v| [v.x as f32, v.y as f32, v.z as f32]).collect();
    let indices: Vec<u32> = t.iter().flatten().copied().collect();
    run(&positions, &indices, &FeatureInput::default(), params, &Control::default()).unwrap()
}

/// Faces per size, uses per undirected edge, and V − E + F.
type Topology = (HashMap<usize, usize>, HashMap<(u32, u32), usize>, i64);

fn topology(mesh: &PolyMesh) -> Topology {
    let mut sizes = HashMap::new();
    let mut edges: HashMap<(u32, u32), usize> = HashMap::new();
    for w in mesh.face_offsets.windows(2) {
        let face = &mesh.face_indices[w[0] as usize..w[1] as usize];
        *sizes.entry(face.len()).or_default() += 1;
        for i in 0..face.len() {
            let (a, b) = (face[i], face[(i + 1) % face.len()]);
            *edges.entry((a.min(b), a.max(b))).or_default() += 1;
        }
    }
    let faces = mesh.face_offsets.len() as i64 - 1;
    let euler = mesh.positions.len() as i64 - edges.len() as i64 + faces;
    (sizes, edges, euler)
}

/// Every directed edge appears once (consistent orientation) on a closed mesh.
fn oriented(mesh: &PolyMesh) -> bool {
    let mut directed: HashMap<(u32, u32), usize> = HashMap::new();
    for w in mesh.face_offsets.windows(2) {
        let face = &mesh.face_indices[w[0] as usize..w[1] as usize];
        for i in 0..face.len() {
            *directed.entry((face[i], face[(i + 1) % face.len()])).or_default() += 1;
        }
    }
    directed.iter().all(|(&(a, b), &n)| n == 1 && directed.get(&(b, a)) == Some(&1))
}

fn assert_closed_quads(out: &RunOutput, euler: i64) {
    let quads = out.quads.as_ref().expect("quads ran");
    let (sizes, edges, chi) = topology(&quads.mesh);
    assert_eq!(
        sizes.get(&4).copied().unwrap_or(0),
        quads.mesh.face_offsets.len() - 1,
        "all quads: {sizes:?} {:?}",
        quads.report
    );
    assert!(edges.values().all(|&n| n == 2), "closed and manifold: {:?}", quads.report);
    assert_eq!(chi, euler, "Euler characteristic: {:?}", quads.report);
    assert!(oriented(&quads.mesh), "consistently oriented");
    assert!(quads.mesh.positions.iter().all(|p| p.is_finite()));
    assert_eq!(quads.patch_of_face.len(), quads.mesh.face_offsets.len() - 1);
}

#[test]
fn cube_becomes_a_closed_quad_mesh() {
    let (p, t, _) = cube(6);
    let out = run_mesh(&p, &t, &Params::default());
    assert_closed_quads(&out, 2);
    let report = &out.quads.as_ref().unwrap().report;
    assert_eq!(report.kept_patches, 0);
}

#[test]
fn cube_feature_edges_are_quad_edges() {
    let (p, t, _) = cube(6);
    let out = run_mesh(&p, &t, &Params::default());
    let quads = out.quads.unwrap();
    let (_, edges, _) = topology(&quads.mesh);
    assert!(!quads.feature_edges.is_empty());
    assert!(quads.feature_edges.iter().all(|&[a, b]| edges.contains_key(&(a, b))));
    // The 12 cube edges, each subdivided: their total length is 12.
    let length: f64 = quads
        .feature_edges
        .iter()
        .map(|&[a, b]| (quads.mesh.positions[a as usize] - quads.mesh.positions[b as usize]).length())
        .sum();
    assert!((length - 12.0).abs() < 1e-6, "feature length {length}");
}

#[test]
fn torus_becomes_a_closed_quad_mesh() {
    let (p, t) = torus(48, 24);
    let out = run_mesh(&p, &t, &Params::default());
    assert_closed_quads(&out, 0);
}

#[test]
fn sphere_becomes_a_closed_quad_mesh() {
    let (p, t) = icosphere(3);
    let out = run_mesh(&p, &t, &Params::default());
    assert_closed_quads(&out, 2);
}

#[test]
fn quad_points_lie_on_the_surface() {
    let (p, t) = icosphere(3);
    let out = run_mesh(&p, &t, &Params::default());
    let quads = out.quads.unwrap();
    // The unit sphere's triangles sit inside it: points are within the chord sagitta.
    for q in &quads.mesh.positions {
        let r = q.length();
        assert!((0.95..=1.0 + 1e-9).contains(&r), "radius {r}");
    }
}

#[test]
fn quad_size_follows_the_setting() {
    let (p, t) = torus(48, 24);
    let mut params = Params::default();
    params.quad.edge_scale = 1.5;
    let fine = run_mesh(&p, &t, &params).quads.unwrap().report.quads;
    params.quad.edge_scale = 3.0;
    let coarse = run_mesh(&p, &t, &params).quads.unwrap().report.quads;
    // Twice the edge, about a quarter of the quads.
    let ratio = fine as f64 / coarse.max(1) as f64;
    assert!((2.5..6.0).contains(&ratio), "fine {fine} coarse {coarse}");
}

#[test]
fn adaptivity_spends_quads_on_detail() {
    // The prep remesh refines where curvature is high; adaptive quads follow it.
    let (p, t) = torus(48, 24);
    let mut params = Params::default();
    params.quad.adaptivity = 0.0;
    let uniform = run_mesh(&p, &t, &params).quads.unwrap();
    params.quad.adaptivity = 1.0;
    let adaptive = run_mesh(&p, &t, &params).quads.unwrap();
    assert!(
        adaptive.report.quads > uniform.report.quads,
        "adaptive {} uniform {}",
        adaptive.report.quads,
        uniform.report.quads
    );
    assert_closed_quads_mesh(&adaptive.mesh);
}

/// Closed, all-quad, manifold.
fn assert_closed_quads_mesh(mesh: &PolyMesh) {
    let (sizes, edges, _) = topology(mesh);
    assert_eq!(sizes.len(), 1, "{sizes:?}");
    assert!(sizes.contains_key(&4));
    assert!(edges.values().all(|&n| n == 2));
}

#[test]
fn output_guard_rejects_broken_meshes() {
    use glam::DVec3;
    let square = |z: f64| {
        [DVec3::new(0.0, 0.0, z), DVec3::new(1.0, 0.0, z), DVec3::new(1.0, 1.0, z), DVec3::new(0.0, 1.0, z)]
    };
    let mut positions: Vec<DVec3> = square(0.0).to_vec();
    positions.extend([DVec3::new(0.5, -1.0, 1.0), DVec3::new(0.5, -1.0, -1.0)]);
    let good =
        PolyMesh { positions: positions.clone(), face_offsets: vec![0, 4], face_indices: vec![0, 1, 2, 3] };
    assert!(super::check_output(&good).is_ok());
    // Edge (0,1) in three faces.
    let fan = PolyMesh {
        positions: positions.clone(),
        face_offsets: vec![0, 4, 7, 10],
        face_indices: vec![0, 1, 2, 3, 1, 0, 4, 1, 0, 5],
    };
    assert!(super::check_output(&fan).is_err());
    let repeated =
        PolyMesh { positions: positions.clone(), face_offsets: vec![0, 4], face_indices: vec![0, 1, 1, 3] };
    assert!(super::check_output(&repeated).is_err());
    let mut nan = good.clone();
    nan.positions[0] = DVec3::NAN;
    assert!(super::check_output(&nan).is_err());
}

#[test]
fn quads_off_leaves_no_quad_mesh() {
    let (p, t) = torus(24, 16);
    let mut params = Params::default();
    params.quad.enabled = false;
    assert!(run_mesh(&p, &t, &params).quads.is_none());
}
