// `Params { .., ..Params::default() }` is needed when the fault-injection feature
// adds a field, and "needless" without it.
#![allow(clippy::needless_update)]

use quadloom_core::features::FeatureInput;
use quadloom_core::{Control, ErrorKind, Params, Stage, run};

fn tetrahedron() -> (Vec<f32>, Vec<u32>) {
    let p = vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
    let i = vec![0, 2, 1, 0, 1, 3, 1, 2, 3, 2, 0, 3];
    (p, i)
}

fn no_remesh() -> Params {
    let mut params = Params::default();
    params.remesh.enabled = false;
    params
}

#[test]
fn run_passthrough_preserves_tetrahedron() {
    let (p, i) = tetrahedron();
    let out = run(&p, &i, &FeatureInput::default(), &no_remesh(), &Control::default()).unwrap();
    assert_eq!(out.mesh.face_count(), 4);
    assert_eq!(out.mesh.positions.len(), 4);
    assert_eq!(out.validation.faces, 4);
    assert_eq!(out.validation.vertices, 4);
    assert_eq!(out.validation.boundary_edges, 0);
    assert_eq!(out.validation.non_manifold_edges, 0);
    assert_eq!(out.validation.components, 1);
    assert!(out.timings_ms.iter().any(|(s, _)| *s == Stage::Validate));
}

#[test]
fn run_reports_two_components() {
    let p = [
        0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, //
        5.0, 0.0, 0.0, 6.0, 0.0, 0.0, 5.0, 1.0, 0.0,
    ];
    let i = [0, 1, 2, 3, 4, 5];
    let out = run(&p, &i, &FeatureInput::default(), &Params::default(), &Control::default()).unwrap();
    assert_eq!(out.validation.components, 2);
    assert_eq!(out.validation.boundary_edges, 6);
}

#[test]
fn run_counts_degenerate_faces() {
    // Face 0 is a good triangle, face 1 a zero-area sliver (collinear points),
    // face 2 repeats a vertex index.
    let p = [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 2.0, 0.0, 0.0];
    let i = [0, 1, 2, 0, 1, 3, 0, 0, 1];
    let out = run(&p, &i, &FeatureInput::default(), &Params::default(), &Control::default()).unwrap();
    assert_eq!(out.validation.degenerate_faces, 2);
}

#[test]
fn run_refuses_oversized_input_before_allocating() {
    let (p, i) = tetrahedron();
    let params = Params { max_faces: 1, ..Params::default() };
    let err = run(&p, &i[..6], &FeatureInput::default(), &params, &Control::default()).unwrap_err();
    assert_eq!(err.kind, ErrorKind::InvalidInput);
    assert!(err.message.contains("2 faces"), "{}", err.message);
    assert!(err.message.contains("limit 1"), "{}", err.message);
}

#[test]
fn run_refuses_too_many_vertices() {
    // One face within the limit, but more vertices than any mesh within the
    // face limit could use (3 per face): loose-vertex blowups are refused too.
    let p = [0.0f32; 12];
    let params = Params { max_faces: 1, ..Params::default() };
    let err = run(&p, &[0, 1, 2], &FeatureInput::default(), &params, &Control::default()).unwrap_err();
    assert_eq!(err.kind, ErrorKind::InvalidInput);
    assert!(err.message.contains("4 vertices"), "{}", err.message);
    assert!(err.message.contains("limit 3"), "{}", err.message);
}

#[test]
fn limits_are_exposed_for_bindings() {
    let params = Params { max_faces: 10, ..Params::default() };
    assert!(quadloom_core::check_limits(30, 30, &params).is_ok());
    assert!(quadloom_core::check_limits(31, 30, &params).is_err());
    assert!(quadloom_core::check_limits(30, 33, &params).is_err());
    // Index space: more than u32::MAX entries can't be addressed.
    let huge = Params { max_faces: usize::MAX, ..Params::default() };
    assert!(quadloom_core::check_limits(3, u32::MAX as usize + 3, &huge).is_err());
}

#[test]
fn run_honours_cancel() {
    let (p, i) = tetrahedron();
    let control = Control::default();
    control.cancel();
    let err = run(&p, &i, &FeatureInput::default(), &Params::default(), &control).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Cancelled);
}

#[cfg(feature = "fault-injection")]
mod faults {
    use super::*;
    use quadloom_core::Fault;
    use std::time::{Duration, Instant};

    #[test]
    fn fault_error_is_returned() {
        let (p, i) = tetrahedron();
        let params = Params { fault: Some(Fault::Error(Stage::Validate)), ..Params::default() };
        let err = run(&p, &i, &FeatureInput::default(), &params, &Control::default()).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Internal);
        assert_eq!(err.stage, Stage::Validate);
    }

    #[test]
    fn fault_panic_panics() {
        let (p, i) = tetrahedron();
        let params = Params { fault: Some(Fault::Panic(Stage::Validate)), ..Params::default() };
        let caught =
            std::panic::catch_unwind(|| run(&p, &i, &FeatureInput::default(), &params, &Control::default()));
        assert!(caught.is_err(), "the injected panic must unwind out of run()");
    }

    #[test]
    fn slow_fault_is_cancellable() {
        let (p, i) = tetrahedron();
        let params = Params { fault: Some(Fault::SlowMillis(5_000)), ..Params::default() };
        let control = Control::default();
        let started = Instant::now();
        let result = std::thread::scope(|s| {
            s.spawn(|| {
                std::thread::sleep(Duration::from_millis(50));
                control.cancel();
            });
            run(&p, &i, &FeatureInput::default(), &params, &control)
        });
        assert_eq!(result.unwrap_err().kind, ErrorKind::Cancelled);
        assert!(started.elapsed() < Duration::from_secs(1), "{:?}", started.elapsed());
    }
}

#[test]
fn run_with_features_on_cube() {
    // A unit cube whose 24 corners are split per face, like a glTF export.
    let corners = [
        [0., 0., 0.],
        [1., 0., 0.],
        [1., 1., 0.],
        [0., 1., 0.],
        [0., 0., 1.],
        [1., 0., 1.],
        [1., 1., 1.],
        [0., 1., 1.],
    ];
    let quads = [[0, 3, 2, 1], [4, 5, 6, 7], [0, 1, 5, 4], [3, 7, 6, 2], [0, 4, 7, 3], [1, 2, 6, 5]];
    let (mut p, mut i) = (Vec::new(), Vec::new());
    for q in quads {
        let base = (p.len() / 3) as u32;
        for c in q {
            p.extend(corners[c].map(|x: f64| x as f32));
        }
        i.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    let out = run(&p, &i, &FeatureInput::default(), &no_remesh(), &Control::default()).unwrap();
    assert_eq!(out.repair.welded_vertices, 16);
    assert_eq!(out.components, 1);
    assert_eq!(out.mesh.positions.len(), 8);
    assert_eq!(out.features.edges, 12);
    assert_eq!(out.features.corners, 8);
    assert_eq!(out.feature_edges.len(), 12);
    assert!(out.timings_ms.iter().any(|(s, _)| *s == Stage::Repair));
    assert!(out.timings_ms.iter().any(|(s, _)| *s == Stage::Features));
}

#[test]
fn feature_groups_of_wrong_length_are_rejected() {
    let (p, i) = tetrahedron();
    let input = FeatureInput { material: Some(vec![0, 1]), ..FeatureInput::default() };
    let err = run(&p, &i, &input, &Params::default(), &Control::default()).unwrap_err();
    assert_eq!(err.kind, ErrorKind::InvalidInput);
    assert!(err.message.contains("material"), "{}", err.message);
}

#[test]
fn out_of_range_params_rejected() {
    let (p, i) = tetrahedron();
    let mut params = Params::default();
    params.repair.weld_epsilon_rel = f64::NAN;
    let err = run(&p, &i, &FeatureInput::default(), &params, &Control::default()).unwrap_err();
    assert!(err.message.contains("repair.weld_epsilon_rel"), "{}", err.message);
    params.repair.weld_epsilon_rel = 0.5;
    assert!(run(&p, &i, &FeatureInput::default(), &params, &Control::default()).is_err());
    let mut params = Params::default();
    params.features.angle_deg = 400.0;
    let err = run(&p, &i, &FeatureInput::default(), &params, &Control::default()).unwrap_err();
    assert!(err.message.contains("features.angle_deg"), "{}", err.message);
    let mut params = Params::default();
    params.repair.weld_epsilon_rel = -1.0; // negative disables welding: allowed
    assert!(run(&p, &i, &FeatureInput::default(), &params, &Control::default()).is_ok());
}

fn cube() -> (Vec<f32>, Vec<u32>) {
    let corners: [[f32; 3]; 8] = [
        [0., 0., 0.],
        [1., 0., 0.],
        [1., 1., 0.],
        [0., 1., 0.],
        [0., 0., 1.],
        [1., 0., 1.],
        [1., 1., 1.],
        [0., 1., 1.],
    ];
    let i = vec![
        0, 3, 2, 0, 2, 1, 4, 5, 6, 4, 6, 7, 0, 1, 5, 0, 5, 4, 3, 7, 6, 3, 6, 2, 0, 4, 7, 0, 7, 3, 1, 2, 6, 1,
        6, 5,
    ];
    (corners.iter().flatten().copied().collect(), i)
}

#[test]
fn remesh_refuses_a_target_that_would_exceed_the_face_limit() {
    // Regression (review of M3b): a 12-triangle cube at Triangle Size 1e-4
    // asked for ~4e8 faces, enough to run the host out of memory. It's refused
    // up front, as invalid input with a hint.
    let (p, i) = cube();
    let mut params = Params::default();
    params.remesh.target_edge_rel = 1e-4;
    let started = std::time::Instant::now();
    let err = run(&p, &i, &FeatureInput::default(), &params, &Control::default()).unwrap_err();
    assert_eq!(err.kind, ErrorKind::InvalidInput, "{err:?}");
    assert_eq!(err.stage, Stage::PrepRemesh);
    assert!(err.hint.is_some());
    assert!(started.elapsed().as_secs_f64() < 1.0, "refused only after {:?}", started.elapsed());
}

#[test]
fn remesh_of_a_non_manifold_part_is_invalid_input() {
    // Regression (review of M3b): with Fix Orientation off, a flipped face
    // reached the remesher and came back as an internal error.
    let (p, mut i) = cube();
    i.swap(0, 1);
    let mut params = Params::default();
    params.repair.orient = false;
    let err = run(&p, &i, &FeatureInput::default(), &params, &Control::default()).unwrap_err();
    assert_eq!(err.kind, ErrorKind::InvalidInput, "{err:?}");
    assert!(err.hint.as_deref().is_some_and(|h| h.contains("Fix Orientation")), "{err:?}");
}

#[test]
fn pipeline_remeshes_cube() {
    let corners = [
        [0., 0., 0.],
        [1., 0., 0.],
        [1., 1., 0.],
        [0., 1., 0.],
        [0., 0., 1.],
        [1., 0., 1.],
        [1., 1., 1.],
        [0., 1., 1.],
    ];
    let p: Vec<f32> = corners.iter().flat_map(|c: &[f64; 3]| c.map(|x| x as f32)).collect();
    let i = [
        0, 3, 2, 0, 2, 1, 4, 5, 6, 4, 6, 7, 0, 1, 5, 0, 5, 4, 3, 7, 6, 3, 6, 2, 0, 4, 7, 0, 7, 3, 1, 2, 6, 1,
        6, 5,
    ];
    // Uniform sizing so the face count is predictable; adaptivity would make
    // the flat cube faces up to 3× coarser.
    let mut params = Params::default();
    params.remesh.target_edge_rel = 0.1;
    params.remesh.adaptive = false;
    let out = run(&p, &i, &FeatureInput::default(), &params, &Control::default()).unwrap();
    assert!(out.mesh.face_count() >= 300, "{} faces", out.mesh.face_count());
    assert_eq!(out.remesh.input_faces, 12);
    assert_eq!(out.remesh.output_faces, out.mesh.face_count());
    assert!(out.feature_edges.len() > 12);
    for c in corners {
        let c = glam::DVec3::from_array(c);
        assert!(out.mesh.positions.iter().any(|q| (*q - c).length() < 1e-9), "corner {c:?} lost");
    }
    assert!(out.timings_ms.iter().any(|(s, _)| *s == Stage::PrepRemesh));
}

#[test]
fn remesh_runs_per_component() {
    // Two cubes: each remeshed on its own, merged in order.
    let mut p: Vec<f32> = Vec::new();
    let mut i: Vec<u32> = Vec::new();
    let tris = [
        0, 3, 2, 0, 2, 1, 4, 5, 6, 4, 6, 7, 0, 1, 5, 0, 5, 4, 3, 7, 6, 3, 6, 2, 0, 4, 7, 0, 7, 3, 1, 2, 6, 1,
        6, 5,
    ];
    for shift in [0.0f32, 3.0] {
        let base = (p.len() / 3) as u32;
        for c in [
            [0., 0., 0.],
            [1., 0., 0.],
            [1., 1., 0.],
            [0., 1., 0.],
            [0., 0., 1.],
            [1., 0., 1.],
            [1., 1., 1.],
            [0., 1., 1.],
        ] {
            p.extend([c[0] + shift, c[1], c[2]]);
        }
        i.extend(tris.iter().map(|&v| v + base));
    }
    let mut params = Params::default();
    params.remesh.target_edge_rel = 0.05;
    let a = run(&p, &i, &FeatureInput::default(), &params, &Control::default()).unwrap();
    let b = run(&p, &i, &FeatureInput::default(), &params, &Control::default()).unwrap();
    assert_eq!(a.components, 2);
    assert_eq!(a.mesh, b.mesh, "parallel per-component remeshing must be deterministic");
    let left = a.mesh.positions.iter().filter(|q| q.x < 2.0).count();
    assert!(left > 0 && left < a.mesh.positions.len());
}
