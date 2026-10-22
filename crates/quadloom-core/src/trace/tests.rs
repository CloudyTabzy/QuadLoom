//! Layouts of whole shapes.

use glam::DVec3;

use hashbrown::HashMap;

use super::*;
use crate::field::{FaceFrames, FieldParams, ROSY, compute as field_compute, vertex_indices};
use crate::remesh::tests::sphere_mesh;
use crate::testmesh::{cube, cylinder, double_torus, grid, icosphere, torus};

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
    // Review of M5: the old assertion could not fail. A strip one face wide
    // is a single quad patch.
    let (p, t) = grid(40, 1);
    let out = layout(&p, &t, &[]);
    assert_valid(&out);
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

/// Positions, triangles, per-face directions and feature edges.
type OracleCase = (Vec<DVec3>, Vec<[u32; 3]>, Vec<DVec3>, Vec<[u32; 2]>);

/// Loads QuadWild's `_rem` stage files (`<prefix>.obj`, `.rosy`, `.sharp`).
fn load_oracle_case(prefix: &str) -> OracleCase {
    let text = std::fs::read_to_string(format!("{prefix}.obj")).unwrap();
    let (mut p, mut t) = (Vec::new(), Vec::new());
    for line in text.lines() {
        let mut f = line.split_whitespace();
        match f.next() {
            Some("v") => {
                let c: Vec<f64> = f.take(3).map(|x| x.parse().unwrap()).collect();
                p.push(DVec3::new(c[0], c[1], c[2]));
            }
            Some("f") => {
                let ids: Vec<u32> =
                    f.map(|x| x.split('/').next().unwrap().parse::<u32>().unwrap() - 1).collect();
                for k in 1..ids.len() - 1 {
                    t.push([ids[0], ids[k], ids[k + 1]]);
                }
            }
            _ => {}
        }
    }
    let rosy = std::fs::read_to_string(format!("{prefix}.rosy")).unwrap();
    let nums: Vec<f64> = rosy.split_whitespace().skip(2).map(|x| x.parse().unwrap()).collect();
    let d: Vec<DVec3> = nums.chunks(3).map(|c| DVec3::new(c[0], c[1], c[2])).collect();
    let sharp = std::fs::read_to_string(format!("{prefix}.sharp")).unwrap();
    let mut features: Vec<[u32; 2]> = sharp
        .lines()
        .skip(1)
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let v: Vec<usize> = l.split(',').map(|x| x.trim().parse().unwrap()).collect();
            let tri = t[v[1]];
            let (a, b) = (tri[v[2]], tri[(v[2] + 1) % 3]);
            [a.min(b), a.max(b)]
        })
        .collect();
    features.sort_unstable();
    features.dedup();
    (p, t, d, features)
}

#[test]
#[ignore = "diagnostic: QL_CASE=<prefix of QuadWild _rem files>"]
fn diagnose_oracle_case() {
    let Some(prefix) = std::env::var_os("QL_CASE") else { return };
    let (p, t, d, features) = load_oracle_case(prefix.to_str().unwrap());
    let frames = FaceFrames::new(&p, &t);
    let powers: Vec<_> = (0..t.len() as u32).map(|f| frames.power(f, d[f as usize], ROSY)).collect();
    let singularities: Vec<(u32, i32)> = vertex_indices(&p, &t, &powers)
        .into_iter()
        .enumerate()
        .filter(|&(_, k)| k != 0)
        .map(|(v, k)| (v as u32, k))
        .collect();
    let part =
        TraceMesh { positions: p, triangles: t, feature_edges: features, directions: d, singularities };
    let (mesh, _) = refine::for_tracing(&part, &Control::default()).unwrap();
    let mut singular = vec![0i32; mesh.positions.len()];
    for &(v, k) in &mesh.singularities {
        singular[v as usize] = k;
    }
    let settings = TraceParams::default().settings();
    let labels = HashMap::new();
    let region = select::Region::new(
        select::RegionMesh {
            positions: &mesh.positions,
            triangles: &mesh.triangles,
            barrier: &mesh.feature_edges,
            directions: mesh.directions.clone(),
            singular: &singular,
            labels: &labels,
            tolerance: settings.cc_tolerance * mean_edge(&mesh),
            merge_id: None,
        },
        &settings,
        &Control::default(),
    )
    .unwrap();
    let mut class_counts: std::collections::BTreeMap<String, usize> = Default::default();
    for c in region.classes.iter().flatten() {
        *class_counts.entry(format!("{c:?}")).or_default() += 1;
    }
    eprintln!("boundary classes {class_counts:?}");
    let control = Control::default();
    let mut layout = select::Layout::new(region, &settings, &control);
    layout.run(&control).unwrap();
    let mut ends: HashMap<u32, usize> = HashMap::new();
    let mut through: HashMap<u32, usize> = HashMap::new();
    for c in layout.chosen.iter().filter(|c| !c.removed) {
        let n = c.path.nodes.len();
        for (i, node) in c.path.nodes.iter().enumerate() {
            if !c.path.closed && (i == 0 || i + 1 == n) {
                *ends.entry(node.vertex()).or_default() += 1;
            } else {
                *through.entry(node.vertex()).or_default() += 1;
            }
        }
    }
    let partition = layout.partition().clone();
    let region = &layout.region;
    let mut cats: std::collections::BTreeMap<String, usize> = Default::default();
    let mut kinds: std::collections::BTreeMap<String, usize> = Default::default();
    for (_, info) in partition.patches() {
        *kinds.entry(format!("{:?}/{}", info.kind, info.sides.len())).or_default() += 1;
        if info.kind != PatchKind::NotConvex {
            continue;
        }
        for &(v, class) in &info.not_convex {
            let key = format!(
                "{class:?} region_class={:?} ends={} through={} singular={}",
                region.classes[v as usize],
                ends.get(&v).copied().unwrap_or(0),
                through.get(&v).copied().unwrap_or(0),
                region.geometric_turns[v as usize]
            );
            *cats.entry(key).or_default() += 1;
        }
    }
    eprintln!("kinds {kinds:?}");
    for (k, n) in &cats {
        eprintln!("{n:5}  {k}");
    }
    eprintln!("tally {:?}", layout.tally);
}

/// The mean edge length of a mesh (the CC-ability tolerance unit).
fn mean_edge(mesh: &TraceMesh) -> f64 {
    let lengths: Vec<f64> = mesh
        .triangles
        .iter()
        .flat_map(|t| (0..3).map(move |i| (t[i], t[(i + 1) % 3])))
        .map(|(a, b)| (mesh.positions[a as usize] - mesh.positions[b as usize]).length())
        .collect();
    lengths.iter().sum::<f64>() / lengths.len().max(1) as f64
}

#[test]
fn guided_loops_cross_feature_lines() {
    // Review of M5: a streamline stopped at the copy it reached through a
    // twin arc, so no guided loop ever crossed a feature.
    let (p, t) = cylinder(10, 32);
    let features: Vec<[u32; 2]> = (0..10).map(|i| [i * 32, (i + 1) * 32]).collect();
    let control = Control::default();
    let field = field_compute(&p, &t, &features, &FieldParams::default(), &control).unwrap();
    let settings = TraceParams::default().settings();
    let labels = HashMap::new();
    let singular = vec![0; p.len()];
    let region = select::Region::new(
        select::RegionMesh {
            positions: &p,
            triangles: &t,
            barrier: &features,
            directions: field.directions.clone(),
            singular: &singular,
            labels: &labels,
            tolerance: 0.1,
            merge_id: None,
        },
        &settings,
        &control,
    )
    .unwrap();
    let g = &region.graph;
    let mut search = search::Search::new(g);
    let v = 5 * 32 + 16;
    let around = DVec3::new(-(std::f64::consts::PI).sin(), (std::f64::consts::PI).cos(), 0.0);
    let start = graph::Node::new(v, g.closest_dir(v, around));
    let found = search.guided_loop(&region.cut, start, 100.0, 3, &control).unwrap();
    let path = found.expect("a loop around the cylinder, across the feature");
    assert!(path.closed);
}

#[test]
#[ignore = "timing; run in release"]
fn cancel_latency_is_bounded() {
    // Review of M5: setup (refinement, the graph, seeding) didn't check
    // cancel, so cancelling took seconds on large meshes.
    let (p, t) = icosphere(7);
    let field = field_compute(&p, &t, &[], &FieldParams::default(), &Control::default()).unwrap();
    for delay_ms in [50u64, 300, 800, 1500, 3000] {
        let control = std::sync::Arc::new(Control::default());
        let canceller = {
            let control = control.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(delay_ms));
                control.cancel();
                std::time::Instant::now()
            })
        };
        let result = compute(&p, &t, &[], &field, &TraceParams::default(), &control);
        let returned = std::time::Instant::now();
        let cancelled_at = canceller.join().unwrap();
        if let Err(e) = result {
            assert_eq!(e.kind, crate::error::ErrorKind::Cancelled);
            let latency = returned.saturating_duration_since(cancelled_at).as_secs_f64();
            eprintln!("cancel at {delay_ms} ms: returned {latency:.3} s later");
            assert!(latency < 0.25, "cancel at {delay_ms} ms took {latency:.3} s");
        }
    }
}

#[test]
fn double_torus_layout_is_all_disks() {
    // Review of M5: genus 2 with no features relies on loops and recovery.
    let (p, t) = double_torus();
    let he = crate::mesh::halfedge::HalfEdges::build(&t, p.len());
    assert_eq!(he.boundary_edge_count(), 0, "closed");
    assert_eq!(p.len() as i64 - (3 * t.len() / 2) as i64 + t.len() as i64, -2, "genus 2");
    let out = layout(&p, &t, &[]);
    assert_eq!(out.report.non_disk, 0, "{}", summary(&out));
    assert!(out.patches.iter().all(|patch| (3..=5).contains(&patch.sides.len())), "{}", summary(&out));
}

#[test]
fn source_face_maps_children_to_parents() {
    let (p, t) = icosphere(2);
    let out = layout(&p, &t, &[]);
    assert_eq!(out.source_face.len(), out.mesh.triangles.len());
    assert!(out.source_face.iter().all(|&f| (f as usize) < t.len()));
    let centroid = |c: [DVec3; 3]| (c[0] + c[1] + c[2]) / 3.0;
    for (k, tri) in out.mesh.triangles.iter().enumerate() {
        let parent = t[out.source_face[k] as usize];
        let corners = parent.map(|v| p[v as usize]);
        let center = centroid(corners);
        // A child lies inside its parent, so its centroid is within the parent's
        // largest corner distance from the parent's centroid.
        let radius = corners.iter().map(|c| (*c - center).length()).fold(0.0, f64::max);
        let child = centroid(tri.map(|v| out.mesh.positions[v as usize]));
        assert!((child - center).length() <= radius + 1e-9, "trace triangle {k} escapes its parent");
    }
}
