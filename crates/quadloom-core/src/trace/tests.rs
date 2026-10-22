//! Layouts of whole shapes.

use glam::DVec3;

use super::*;
use crate::field::{FieldParams, compute as field_compute};
use crate::remesh::tests::sphere_mesh;
use crate::testmesh::{cube, cylinder, grid, icosphere, torus};

fn layout(positions: &[DVec3], triangles: &[[u32; 3]], features: &[[u32; 2]]) -> TraceOutput {
    layout_with(positions, triangles, features, &TraceParams::default())
}

fn layout_with(
    positions: &[DVec3],
    triangles: &[[u32; 3]],
    features: &[[u32; 2]],
    params: &TraceParams,
) -> TraceOutput {
    let control = Control::default();
    let field = field_compute(positions, triangles, features, &FieldParams::default(), &control).unwrap();
    compute(positions, triangles, features, &field, params, &control).unwrap()
}

fn summary(out: &TraceOutput) -> String {
    format!("{:?}", out.report)
}

fn assert_valid(out: &TraceOutput) {
    for (k, p) in out.patches.iter().enumerate() {
        assert!(p.kind.is_valid(), "patch {k} is {:?} ({} sides): {}", p.kind, p.sides.len(), summary(out));
    }
    assert_eq!(out.report.unsolved, 0, "{}", summary(out));
    assert_eq!(out.patch_of_face.len(), out.mesh.triangles.len());
}

#[test]
fn cube_layout_is_six_quads() {
    let (p, t, f) = cube(4);
    let out = layout(&p, &t, &f);
    assert_eq!(out.patches.len(), 6, "{}", summary(&out));
    for patch in &out.patches {
        assert_eq!(patch.sides.len(), 4);
        assert_eq!(patch.kind, PatchKind::Ok);
    }
    assert_eq!(out.report.kept_paths, 0);
}

#[test]
fn l_shape_concave_corner_gets_traced() {
    let (p, t) = grid(8, 8);
    let t: Vec<[u32; 3]> = t
        .into_iter()
        .filter(|tri| !tri.iter().all(|&v| p[v as usize].x >= 4.0 && p[v as usize].y >= 4.0))
        .collect();
    let out = layout(&p, &t, &[]);
    assert_valid(&out);
    assert!(out.report.kept_paths >= 1);
    assert!(out.patches.iter().all(|p| (3..=5).contains(&p.sides.len())));
}

#[test]
fn dangling_feature_is_extended() {
    let (p, t) = grid(10, 10);
    let id = |i: u32, j: u32| j * 11 + i;
    let features: Vec<[u32; 2]> = (3..6).map(|i| [id(i, 5), id(i + 1, 5)]).collect();
    let out = layout(&p, &t, &features);
    assert_valid(&out);
    assert!(out.report.kept_paths >= 2, "both ends are extended: {}", summary(&out));
}

#[test]
fn sphere_layout_is_valid() {
    let (p, t) = icosphere(4);
    let out = layout(&p, &t, &[]);
    assert_valid(&out);
    for patch in &out.patches {
        assert!(patch.singularities.len() <= 1, "{patch:?}");
    }
    let ok = out.report.ok as f64 / out.patches.len() as f64;
    assert!(ok >= 0.5, "ok fraction {ok}: {}", summary(&out));
}

#[test]
fn latitude_field_sphere_degrades_gracefully() {
    // A UV sphere's field has a +1 singularity (4 quarters) at each pole,
    // which no single quad-patch singularity can match and no path can cross.
    let (p, t) = sphere_mesh(16, 32);
    let out = layout(&p, &t, &[]);
    assert_eq!(out.report.non_disk, 0, "{}", summary(&out));
    assert_eq!(out.patch_of_face.len(), out.mesh.triangles.len());
    let invalid = out.patches.iter().filter(|p| !p.kind.is_valid()).count();
    assert_eq!(out.report.unsolved, invalid, "every invalid patch is reported");
}

#[test]
fn torus_layout_is_all_disks() {
    let (p, t) = torus(32, 16);
    let out = layout(&p, &t, &[]);
    assert_valid(&out);
}

#[test]
fn open_cylinder_is_cut_into_disks() {
    let (p, t) = cylinder(10, 32);
    let out = layout(&p, &t, &[]);
    assert_valid(&out);
    assert!(out.report.kept_paths >= 1);
}

#[test]
fn thin_strip_layout_reports_or_solves() {
    let (p, t) = grid(40, 1);
    let out = layout(&p, &t, &[]);
    for patch in &out.patches {
        assert!(patch.kind.is_valid() || out.report.unsolved > 0, "{patch:?}");
    }
}

#[test]
fn layout_is_deterministic() {
    let (mut p, t) = sphere_mesh(14, 28);
    for (k, q) in p.iter_mut().enumerate() {
        *q *= 1.0 + 0.05 * ((k as f64) * 0.7).sin();
    }
    let a = layout(&p, &t, &[]);
    let b = layout(&p, &t, &[]);
    assert_eq!(a.patch_of_face, b.patch_of_face);
    assert_eq!(a.paths, b.paths);
}

#[test]
fn cancel_stops_layout() {
    let (p, t) = sphere_mesh(10, 20);
    let control = Control::default();
    let field = field_compute(&p, &t, &[], &FieldParams::default(), &control).unwrap();
    control.cancel();
    let result = compute(&p, &t, &[], &field, &TraceParams::default(), &control);
    assert_eq!(result.unwrap_err().kind, crate::error::ErrorKind::Cancelled);
}

#[test]
fn both_costs_give_valid_layouts() {
    let (p, t, f) = cube(6);
    for (cost, drift) in [(TraceCost::Anisotropic, 10.0), (TraceCost::AnglePenalty, 100.0)] {
        let params = TraceParams { cost, drift, ..TraceParams::default() };
        let out = layout_with(&p, &t, &f, &params);
        assert_valid(&out);
    }
}

#[test]
#[ignore = "performance check; run in release"]
fn perf_layout_200k_faces() {
    let (p, t) = icosphere(7);
    let started = std::time::Instant::now();
    let out = layout(&p, &t, &[]);
    let seconds = started.elapsed().as_secs_f64();
    eprintln!("{} faces, {seconds:.1} s, {}", t.len(), summary(&out));
    assert!(seconds < 20.0);
}
