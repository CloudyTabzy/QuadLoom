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
    assert_eq!(json["faces"], 4);
    assert_eq!(json["boundary_edges"], 0);
}

#[test]
fn run_writes_obj_with_same_face_count() {
    let input = scratch("run_tet.obj", TETRAHEDRON_OBJ);
    let output = input.with_file_name("run_tet_out.obj");
    let out = quadloom(&["run", input.to_str().unwrap(), output.to_str().unwrap()]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let written = std::fs::read_to_string(&output).unwrap();
    assert_eq!(written.lines().filter(|l| l.starts_with("f ")).count(), 4);
    let stats: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(stats["output_faces"], 4);
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
fn usage_error_exits_64() {
    let out = quadloom(&["frobnicate"]);
    assert_eq!(out.status.code(), Some(64));
}
