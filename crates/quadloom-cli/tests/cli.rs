#![allow(clippy::unwrap_used)] // test helpers outside #[test] fns

use std::path::PathBuf;
use std::process::{Command, Output};

const TETRAHEDRON_OBJ: &str = "v 0 0 0\nv 1 0 0\nv 0 1 0\nv 0 0 1\nf 1 3 2\nf 1 2 4\nf 2 3 4\nf 3 1 4\n";

fn scratch(name: &str, contents: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("quadloom-cli-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, contents).unwrap();
    path
}

fn quadloom(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_quadloom")).args(args).output().unwrap()
}

#[test]
fn info_prints_validation_json() {
    let input = scratch("info_tet.obj", TETRAHEDRON_OBJ);
    let out = quadloom(&["info", input.to_str().unwrap()]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["validation"]["faces"], 4);
    assert_eq!(json["validation"]["boundary_edges"], 0);
}

#[test]
fn info_reports_repair_and_features() {
    let input = scratch("info_tri_cube.obj", TRI_CUBE_OBJ);
    let out = quadloom(&["info", input.to_str().unwrap()]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["repair"]["components"], 1);
    assert_eq!(json["features"]["edges"], 12);
    assert_eq!(json["features"]["corners"], 8);
}

#[test]
fn run_writes_obj_matching_reported_faces() {
    let input = scratch("run_tet.obj", TETRAHEDRON_OBJ);
    let output = input.with_file_name("run_tet_out.obj");
    let out = quadloom(&["run", input.to_str().unwrap(), output.to_str().unwrap()]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let written = std::fs::read_to_string(&output).unwrap();
    let stats: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let faces = written.lines().filter(|l| l.starts_with("f ")).count();
    assert_eq!(stats["output_faces"], faces);
    // The written mesh is the quad mesh; the prep remesh's triangles are reported apart.
    assert_eq!(stats["remesh"]["output_faces"], stats["prep_faces"]);
    assert!(stats["prep_faces"].as_u64().unwrap() > 4, "the tetrahedron is refined by the prep remesh");
    let quads = written.lines().filter(|l| l.starts_with("f ") && l.split_whitespace().count() == 5).count();
    assert_eq!(quads, faces, "all quads");
    assert_eq!(stats["quads"]["quads"], faces);
}

#[test]
fn missing_file_exits_4() {
    let out = quadloom(&["info", "does/not/exist.obj"]);
    assert_eq!(out.status.code(), Some(4));
}

#[test]
fn out_of_range_index_exits_2() {
    let input = scratch("bad_index.obj", "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 9\n");
    let out = quadloom(&["info", input.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(2), "{}", String::from_utf8_lossy(&out.stderr));
    let err: serde_json::Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(err["error"]["kind"], "invalid_input");
}

#[test]
fn uv_seams_do_not_split_vertices() {
    // Two triangles sharing edge 2-3; vertex 3 uses a different UV in each face.
    let obj = "v 0 0 0\nv 1 0 0\nv 0 1 0\nv 1 1 0\nvt 0 0\nvt 1 0\nvt 0 1\nvt 1 1\nvt 0.5 0.5\n\
               f 1/1 2/2 4/4\nf 1/1 4/5 3/3\n";
    let input = scratch("uv_seam.obj", obj);
    let out = quadloom(&["info", input.to_str().unwrap()]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["validation"]["vertices"], 4);
    assert_eq!(json["validation"]["boundary_edges"], 4);
}

const QUAD_CUBE_OBJ: &str = "v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nv 0 0 1\nv 1 0 1\nv 1 1 1\nv 0 1 1\n\
f 1 4 3 2\nf 5 6 7 8\nf 1 2 6 5\nf 2 3 7 6\nf 3 4 8 7\nf 4 1 5 8\n";

const TRI_CUBE_OBJ: &str = "v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nv 0 0 1\nv 1 0 1\nv 1 1 1\nv 0 1 1\n\
f 1 4 3\nf 1 3 2\nf 5 6 7\nf 5 7 8\nf 1 2 6\nf 1 6 5\nf 2 3 7\nf 2 7 6\nf 3 4 8\nf 3 8 7\nf 4 1 5\nf 4 5 8\n";

#[test]
fn metrics_on_quad_cube() {
    let result = scratch("metrics_quad_cube.obj", QUAD_CUBE_OBJ);
    let reference = scratch("metrics_tri_cube.obj", TRI_CUBE_OBJ);
    let out = quadloom(&["metrics", result.to_str().unwrap(), "--reference", reference.to_str().unwrap()]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["quality"]["pure_quad"], true);
    assert_eq!(json["quality"]["quads"], 6);
    assert_eq!(json["quality"]["irregular_interior"], 8);
    assert!(json["distance"]["hausdorff"].as_f64().unwrap() < 1e-9, "{}", json["distance"]);
}

#[test]
fn metrics_without_reference() {
    let result = scratch("metrics_quad_cube_noref.obj", QUAD_CUBE_OBJ);
    let out = quadloom(&["metrics", result.to_str().unwrap()]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["quality"]["faces"], 6);
    assert!(json["distance"].is_null());
}

#[test]
fn usage_error_exits_64() {
    let out = quadloom(&["frobnicate"]);
    assert_eq!(out.status.code(), Some(64));
}

#[test]
fn layout_files_round_trip() {
    let input = scratch("layout_cube.obj", TRI_CUBE_OBJ);
    let output = input.with_file_name("layout_cube_out.obj");
    let prefix = input.with_file_name("layout_cube");
    let out = quadloom(&[
        "run",
        input.to_str().unwrap(),
        output.to_str().unwrap(),
        "--layout",
        prefix.to_str().unwrap(),
    ]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let run: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(run["trace"]["patches"], 6);
    let file = |ext: &str| input.with_file_name(format!("layout_cube_p0.{ext}"));
    for ext in ["obj", "patch", "corners", "feature", "c_feature"] {
        assert!(file(ext).exists(), "missing {ext}");
    }
    let stats = quadloom(&[
        "layout-stats",
        file("obj").to_str().unwrap(),
        file("patch").to_str().unwrap(),
        file("corners").to_str().unwrap(),
    ]);
    assert!(stats.status.success(), "{}", String::from_utf8_lossy(&stats.stderr));
    let stats: serde_json::Value = serde_json::from_slice(&stats.stdout).unwrap();
    assert_eq!(stats["patches"], 6);
    assert_eq!(stats["valid"], 6);
    assert_eq!(stats["sides_histogram"], run["trace"]["sides_histogram"]);
    assert_eq!(stats["unassigned_faces"], 0);
}

/// A torus as OBJ text (`rings` × `segments` grid, two triangles per cell).
fn torus_obj(rings: u32, segments: u32) -> String {
    let mut obj = String::new();
    for i in 0..rings {
        let a = std::f64::consts::TAU * f64::from(i) / f64::from(rings);
        for j in 0..segments {
            let b = std::f64::consts::TAU * f64::from(j) / f64::from(segments);
            let r = 1.0 + 0.35 * b.cos();
            obj += &format!("v {} {} {}\n", r * a.cos(), r * a.sin(), 0.35 * b.sin());
        }
    }
    let id = |i: u32, j: u32| (i % rings) * segments + (j % segments) + 1;
    for i in 0..rings {
        for j in 0..segments {
            let (a, b, c, d) = (id(i, j), id(i + 1, j), id(i + 1, j + 1), id(i, j + 1));
            obj += &format!("f {a} {b} {c}\nf {a} {c} {d}\n");
        }
    }
    obj
}

#[test]
fn run_with_frame_reports_frame() {
    let input = scratch("run_torus_frame.obj", &torus_obj(24, 16));
    let output = input.with_file_name("run_torus_frame_out.obj");
    let rosy = input.with_file_name("run_torus_frame.rosy");
    let out = quadloom(&[
        "run",
        input.to_str().unwrap(),
        output.to_str().unwrap(),
        "--frame",
        "{}",
        "--rosy",
        rosy.to_str().unwrap(),
    ]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(json["frame"]["solved_parts"].as_u64().unwrap() >= 1, "{}", json["frame"]);
    // The rosy file holds one direction per prep-mesh triangle.
    let faces = json["prep_faces"].as_u64().unwrap();
    let written = std::fs::read_to_string(&rosy).unwrap();
    assert_eq!(written.lines().count(), faces as usize + 2);
}

#[test]
fn run_frame_unknown_key_exits_2() {
    let input = scratch("run_frame_bad_key.obj", TETRAHEDRON_OBJ);
    let output = input.with_file_name("run_frame_bad_key_out.obj");
    let out =
        quadloom(&["run", input.to_str().unwrap(), output.to_str().unwrap(), "--frame", r#"{"bogus": 1}"#]);
    assert_eq!(out.status.code(), Some(2), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stderr).contains("params.frame.bogus"));
}
