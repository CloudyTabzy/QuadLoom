use quadloom_core::{Control, ErrorKind, Params, Stage, run};

fn tetrahedron() -> (Vec<f32>, Vec<u32>) {
    let p = vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
    let i = vec![0, 2, 1, 0, 1, 3, 1, 2, 3, 2, 0, 3];
    (p, i)
}

#[test]
fn run_passthrough_preserves_tetrahedron() {
    let (p, i) = tetrahedron();
    let out = run(&p, &i, &Params::default(), &Control::default()).unwrap();
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
    let out = run(&p, &i, &Params::default(), &Control::default()).unwrap();
    assert_eq!(out.validation.components, 2);
    assert_eq!(out.validation.boundary_edges, 6);
}

#[test]
fn run_counts_degenerate_faces() {
    // Face 0 is a good triangle, face 1 a zero-area sliver (collinear points),
    // face 2 repeats a vertex index.
    let p = [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 2.0, 0.0, 0.0];
    let i = [0, 1, 2, 0, 1, 3, 0, 0, 1];
    let out = run(&p, &i, &Params::default(), &Control::default()).unwrap();
    assert_eq!(out.validation.degenerate_faces, 2);
}

#[test]
#[allow(clippy::needless_update)] // `..` covers `fault` when fault-injection is on
fn run_refuses_oversized_input_before_allocating() {
    let (p, i) = tetrahedron();
    let params = Params { max_faces: 1, ..Params::default() };
    let err = run(&p, &i[..6], &params, &Control::default()).unwrap_err();
    assert_eq!(err.kind, ErrorKind::InvalidInput);
    assert!(err.message.contains("2 faces"), "{}", err.message);
    assert!(err.message.contains("limit 1"), "{}", err.message);
}

#[test]
fn run_honours_cancel() {
    let (p, i) = tetrahedron();
    let control = Control::default();
    control.cancel();
    let err = run(&p, &i, &Params::default(), &control).unwrap_err();
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
        let err = run(&p, &i, &params, &Control::default()).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Internal);
        assert_eq!(err.stage, Stage::Validate);
    }

    #[test]
    fn fault_panic_panics() {
        let (p, i) = tetrahedron();
        let params = Params { fault: Some(Fault::Panic(Stage::Validate)), ..Params::default() };
        let caught = std::panic::catch_unwind(|| run(&p, &i, &params, &Control::default()));
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
            run(&p, &i, &params, &control)
        });
        assert_eq!(result.unwrap_err().kind, ErrorKind::Cancelled);
        assert!(started.elapsed() < Duration::from_secs(1), "{:?}", started.elapsed());
    }
}
