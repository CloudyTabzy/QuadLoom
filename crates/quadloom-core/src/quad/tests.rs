//! End-to-end checks of the quad stages through the pipeline.

use hashbrown::HashMap;

use glam::DVec3;

use crate::control::Control;
use crate::features::FeatureInput;
use crate::mesh::PolyMesh;
use crate::params::Params;
use crate::pipeline::{RunOutput, run};
use crate::testmesh::{cube, icosphere, torus};

use super::{Prepared, QuadParams};

fn run_mesh(p: &[DVec3], t: &[[u32; 3]], params: &Params) -> RunOutput {
    let positions: Vec<f32> = p.iter().flat_map(|v| [v.x as f32, v.y as f32, v.z as f32]).collect();
    let indices: Vec<u32> = t.iter().flatten().copied().collect();
    run(&positions, &indices, &FeatureInput::default(), params, &Control::default())
        .unwrap_or_else(|e| panic!("the run failed: {e:?}"))
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
fn target_cube_count_preserves_features_and_closed_topology() {
    let (p, t, _) = cube(2);
    let mut params = Params::default();
    params.remesh.enabled = false;
    params.quad.target_quads = 600;
    let out = run_mesh(&p, &t, &params);
    assert_closed_quads(&out, 2);
    let q = out.quads.as_ref().unwrap();
    assert!(q.report.quads.abs_diff(600) <= 30, "{:?}", q.report);
    assert_eq!(q.report.target_quads, 600);
    assert!((1..=8).contains(&q.report.target_attempts));
    let feature_length: f64 = q
        .feature_edges
        .iter()
        .map(|&[a, b]| (q.mesh.positions[a as usize] - q.mesh.positions[b as usize]).length())
        .sum();
    assert!((feature_length - 12.0).abs() < 1e-6);
}

#[test]
fn target_request_limits_are_checked_up_front() {
    let mut params = Params::default();
    params.quad.target_quads = 2_000_001;
    let error = params.check().unwrap_err();
    assert!(error.message.contains("target_quads"));
}

#[test]
fn target_output_cap_includes_early_mapping_fallback_before_later_success() {
    use super::{
        Prepared, QuadParams, pattern,
        segments::{Segment, SegmentUse, Segments},
    };
    use crate::trace::{Patch, TraceOutput};
    let mut trace = TraceOutput::default();
    trace.mesh.positions = vec![
        DVec3::ZERO,
        DVec3::X,
        DVec3::Y,
        DVec3::new(3.0, 0.0, 0.0),
        DVec3::new(4.0, 0.0, 0.0),
        DVec3::new(4.0, 1.0, 0.0),
        DVec3::new(3.0, 1.0, 0.0),
    ];
    trace.mesh.triangles = vec![[0, 1, 2], [3, 4, 5], [3, 5, 6]];
    trace.patches = vec![
        Patch {
            corners: vec![],
            sides: vec![],
            singularities: vec![],
            kind: crate::PatchKind::Ok,
            cc_margin: 0.0,
        },
        Patch {
            corners: vec![3, 4, 5, 6],
            sides: vec![],
            singularities: vec![],
            kind: crate::PatchKind::Ok,
            cc_margin: 0.0,
        },
    ];
    let pairs = [[0, 1], [1, 2], [2, 0], [3, 4], [4, 5], [5, 6], [6, 3]];
    let segments = pairs
        .iter()
        .enumerate()
        .map(|(s, v)| Segment {
            vertices: v.to_vec(),
            uses: vec![SegmentUse {
                patch: if s < 3 { 0 } else { 1 },
                side: if s < 3 { s as u8 } else { (s - 3) as u8 },
                reversed: false,
            }],
            polyline: false,
        })
        .collect();
    let prepared = Prepared {
        faces_of: vec![vec![0], vec![1, 2]],
        segs: Segments {
            segments,
            sides: vec![
                Some((0..3).map(|s| vec![(s, false)]).collect()),
                Some((3..7).map(|s| vec![(s, false)]).collect()),
            ],
            twins: Vec::new(),
        },
    };
    let patterns = vec![pattern::build(&[1, 2, 1]), pattern::build(&[2; 4])];
    let counts = [1, 2, 1, 2, 2, 2, 2];
    let params = QuadParams { smoothing: 0, ..Default::default() };
    let assembled =
        prepared.assemble(&trace, &counts, 1.0, &params, Some((&patterns, 7)), &Control::default()).unwrap();
    let super::Assembled::Mesh(output) = assembled else { panic!("{assembled:?}") };
    assert_eq!(output.report.kept_patches, 1);
    assert_eq!(output.report.quads, 7); // first planned 1 becomes 3, then 4 succeeds.
    let error = prepared
        .assemble(&trace, &counts, 1.0, &params, Some((&patterns, 5)), &Control::default())
        .unwrap_err();
    assert!(error.message.contains("limit"));
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

/// A unit sphere with a finger: the cap within 16° of +z is drawn out into
/// a tube of radius 0.12 and length 1.6 with a round end, flaring into the
/// sphere at its base.
fn fingered_sphere() -> (Vec<DVec3>, Vec<[u32; 3]>) {
    let (mut p, t) = icosphere(5);
    let (cap, radius, length) = (16f64.to_radians(), 0.12, 1.6);
    let (rim, base) = (cap.sin(), cap.cos());
    for v in &mut p {
        let polar = v.z.clamp(-1.0, 1.0).acos();
        if polar >= cap {
            continue;
        }
        let around = DVec3::new(v.x, v.y, 0.0).normalize_or_zero();
        // 0 at the pole (the fingertip), 1 at the cap's rim (the finger's base).
        let u = polar / cap;
        *v = if u < 0.25 {
            // The round end: a quarter turn from the axis to the tube.
            let turn = u / 0.25 * std::f64::consts::FRAC_PI_2;
            around * radius * turn.sin() + DVec3::Z * (base + length + radius * turn.cos())
        } else {
            let down = (u - 0.25) / 0.75;
            let flare = down.powi(6);
            around * (radius + (rim - radius) * flare) + DVec3::Z * (base + length * (1.0 - down))
        };
    }
    (p, t)
}

/// The scaled Jacobian of every quad (the benchmark's measure: the sine of
/// the worst corner in the quad's own plane).
fn scaled_jacobians(mesh: &PolyMesh) -> Vec<f64> {
    mesh.face_offsets
        .windows(2)
        .filter_map(|w| {
            let &[a, b, c, d] = &mesh.face_indices[w[0] as usize..w[1] as usize] else { return None };
            let q = [a, b, c, d].map(|v| mesh.positions[v as usize]);
            let normal = (q[2] - q[0]).cross(q[3] - q[1]).normalize_or_zero();
            Some(
                (0..4)
                    .map(|k| {
                        let (next, prev) = (q[(k + 1) % 4] - q[k], q[(k + 3) % 4] - q[k]);
                        next.cross(prev).dot(normal) / (next.length() * prev.length()).max(1e-300)
                    })
                    .fold(f64::INFINITY, f64::min),
            )
        })
        .collect()
}

#[test]
fn relaxing_does_not_wear_down_a_finger() {
    // Averaging in space and snapping back to the surface slid the quads at
    // the end of a thin part down it a little every round, until the end was
    // bare (5% of the model on the benchmark's pig tail). Many rounds must
    // leave the surface as well covered as none. The quads are sized for the
    // sphere, so the finger is only a few of them round.
    let (p, t) = fingered_sphere();
    let positions: Vec<f32> = p.iter().flat_map(|v| [v.x as f32, v.y as f32, v.z as f32]).collect();
    let indices: Vec<u32> = t.iter().flatten().copied().collect();
    let reference = crate::InputMesh::from_buffers(&positions, &indices).unwrap();
    // How far the input is from the quads at its worst, and the mean both ways.
    let uncovered = |rounds: usize| -> (f64, f64) {
        let mut params = Params::default();
        params.quad.smoothing = rounds;
        params.quad.adaptivity = 0.0;
        params.quad.edge_scale = 2.0;
        let quads = run_mesh(&p, &t, &params).quads.unwrap();
        let distance = crate::metrics::surface_distance(&quads.mesh, &reference);
        (distance.reference_to_result_max, distance.mean)
    };
    let (unrelaxed, few, many) = (uncovered(0), uncovered(20), uncovered(100));
    println!(
        "input to quads, worst and mean: unrelaxed {unrelaxed:?}, 20 rounds {few:?}, 100 rounds {many:?}"
    );
    assert!(few.0 < unrelaxed.0 * 1.25 + 0.002, "20 rounds: {few:?}, unrelaxed {unrelaxed:?}");
    assert!(many.0 < unrelaxed.0 * 1.25 + 0.002, "100 rounds: {many:?}, unrelaxed {unrelaxed:?}");
}

#[test]
fn relaxing_keeps_feature_vertices_on_their_lines_and_corners_in_place() {
    let (p, t, _) = cube(6);
    let mut params = Params::default();
    params.quad.smoothing = 40;
    let quads = run_mesh(&p, &t, &params).quads.unwrap();
    // The cube spans [0, 1]³ or [−h, h]³: read its extent off the input.
    let (low, high) =
        p.iter().fold((DVec3::INFINITY, DVec3::NEG_INFINITY), |(l, h), v| (l.min(*v), h.max(*v)));
    let on_face = |x: f64, lo: f64, hi: f64| (x - lo).abs() < 1e-9 || (x - hi).abs() < 1e-9;
    let faces_touched = |q: &DVec3| {
        [on_face(q.x, low.x, high.x), on_face(q.y, low.y, high.y), on_face(q.z, low.z, high.z)]
            .iter()
            .filter(|&&b| b)
            .count()
    };
    let mut on_line = hashbrown::HashSet::new();
    for &[a, b] in &quads.feature_edges {
        on_line.extend([a, b]);
    }
    assert!(!on_line.is_empty());
    for &v in &on_line {
        let q = &quads.mesh.positions[v as usize];
        assert!(faces_touched(q) >= 2, "feature vertex {v} left its edge: {q}");
    }
    // All eight corners are still vertices.
    let corners = quads.mesh.positions.iter().filter(|q| faces_touched(q) == 3).count();
    assert_eq!(corners, 8);
    // And every vertex is on the cube.
    assert!(quads.mesh.positions.iter().all(|q| faces_touched(q) >= 1));
}

#[test]
fn simple_shapes_have_no_folded_quads() {
    let (cube_p, cube_t, _) = cube(6);
    let (sphere_p, sphere_t) = icosphere(3);
    let (torus_p, torus_t) = torus(48, 24);
    for (name, p, t) in
        [("cube", cube_p, cube_t), ("sphere", sphere_p, sphere_t), ("torus", torus_p, torus_t)]
    {
        let quads = run_mesh(&p, &t, &Params::default()).quads.unwrap();
        let jacobians = scaled_jacobians(&quads.mesh);
        let worst = jacobians.iter().copied().fold(f64::INFINITY, f64::min);
        let mean = jacobians.iter().sum::<f64>() / jacobians.len() as f64;
        assert!(worst > 0.3, "{name}: worst scaled Jacobian {worst}");
        assert!(mean > 0.9, "{name}: mean scaled Jacobian {mean}");
    }
}

/// An open tube of radius 0.1 and length 1 along z (five rings of eight
/// vertices), laid out as two patches, each half the way round: they meet
/// along two seams, at angle 0 and angle π.
fn two_patch_tube() -> crate::trace::TraceOutput {
    use crate::trace::{Patch, PatchKind, Side, TraceMesh, TraceOutput};
    let id = |ring: u32, k: u32| ring * 8 + k % 8;
    let positions: Vec<DVec3> = (0..5u32)
        .flat_map(|ring| {
            (0..8u32).map(move |k| {
                let a = std::f64::consts::TAU * f64::from(k) / 8.0;
                DVec3::new(0.1 * a.cos(), 0.1 * a.sin(), 0.25 * f64::from(ring))
            })
        })
        .collect();
    let mut triangles = Vec::new();
    let mut patch_of_face = Vec::new();
    for ring in 0..4 {
        for k in 0..8 {
            let (a, b, c, d) = (id(ring, k), id(ring, k + 1), id(ring + 1, k + 1), id(ring + 1, k));
            triangles.extend([[a, b, c], [a, c, d]]);
            patch_of_face.extend([k / 4; 2]);
        }
    }
    let side = |vertices: Vec<u32>| Side { length: (vertices.len() - 1) as f64, vertices };
    let half = |from: u32| -> Patch {
        let (near, far) = (from, from + 4);
        let sides = vec![
            side((near..=far).map(|k| id(0, k)).collect()),
            side((0..5).map(|ring| id(ring, far)).collect()),
            side((near..=far).rev().map(|k| id(4, k)).collect()),
            side((0..5).rev().map(|ring| id(ring, near)).collect()),
        ];
        Patch {
            corners: sides.iter().map(|s| s.vertices[0]).collect(),
            sides,
            singularities: Vec::new(),
            kind: PatchKind::Ok,
            cc_margin: 0.0,
        }
    };
    let faces = triangles.len() as u32;
    TraceOutput {
        mesh: TraceMesh {
            directions: vec![DVec3::Z; triangles.len()],
            positions,
            triangles,
            ..TraceMesh::default()
        },
        patch_of_face,
        source_face: (0..faces).collect(),
        patches: vec![half(0), half(4)],
        ..TraceOutput::default()
    }
}

#[test]
fn a_tube_two_quads_round_clashes_and_its_patches_keep_their_triangles() {
    // Each half-tube patch as a 1 × 3 grid: the "rungs" across it join a
    // sample of one seam to the same sample of the other, and the other
    // half's rungs join the same pairs. An edge in four faces: that failed
    // the whole run (the benchmark's chess set came back as its patch layout).
    let trace = two_patch_tube();
    let params = QuadParams { smoothing: 0, ..Default::default() };
    let prepared = Prepared::new(&trace, &[], &Control::default()).unwrap();
    // Segments in build order: bottom, seam, top, seam of the first half; bottom and top of the second.
    assert_eq!(prepared.segs.segments.len(), 6);
    let counts = [1, 3, 1, 3, 1, 1];
    let assembled = prepared.assemble(&trace, &counts, 0.3, &params, None, &Control::default()).unwrap();
    let super::Assembled::Clash(patches) = assembled else { panic!("expected a clash: {assembled:?}") };
    assert_eq!(patches, [0, 1]);
    // With both kept as triangles the mesh is a surface: 64 triangles, 3 quads each.
    let prepared = Prepared::new(&trace, &[true, true], &Control::default()).unwrap();
    let counts: Vec<u32> = prepared.segs.segments.iter().map(|s| 2 * (s.vertices.len() as u32 - 1)).collect();
    let assembled = prepared.assemble(&trace, &counts, 0.3, &params, None, &Control::default()).unwrap();
    let super::Assembled::Mesh(output) = assembled else { panic!("expected a mesh: {assembled:?}") };
    assert_eq!((output.report.kept_patches, output.report.quads), (2, 192));
    let (_, edges, _) = topology(&output.mesh);
    assert!(edges.values().all(|&n| n <= 2));
}

#[test]
fn the_quad_stage_survives_a_tube_two_quads_round() {
    // End to end, at sizes from one quad round to several: always a surface.
    let trace = two_patch_tube();
    for edge in [0.08, 0.15, 0.3, 0.6, 1.5] {
        let output = super::compute(
            &trace,
            super::Sizing::default(),
            edge,
            &QuadParams::default(),
            &Control::default(),
        )
        .unwrap_or_else(|e| panic!("edge {edge}: {e:?}"));
        let (sizes, edges, _) = topology(&output.mesh);
        assert_eq!(
            sizes.get(&4).copied().unwrap_or(0),
            output.mesh.face_offsets.len() - 1,
            "edge {edge}: all quads"
        );
        assert!(edges.values().all(|&n| n <= 2), "edge {edge}: an edge in more than two faces");
        // Open at both ends, so some edges have one face; the rest have two.
        assert!(edges.values().any(|&n| n == 1), "edge {edge}");
    }
}

#[test]
fn clashing_patches_are_kept_and_the_build_retried_a_few_times() {
    let trace = two_patch_tube();
    let mesh = || super::Assembled::Mesh(Box::default());
    // The second round is told to keep the patch that clashed in the first.
    let mut rounds = Vec::new();
    let result = super::retrying_clashes(&trace, |keep| {
        rounds.push(keep.to_vec());
        Ok(if keep[1] { mesh() } else { super::Assembled::Clash(vec![1]) })
    });
    assert!(result.is_ok());
    assert_eq!(rounds, [vec![false, false], vec![false, true]]);
    // A build that never stops clashing is given up on.
    let mut calls = 0;
    let error = super::retrying_clashes(&trace, |_| {
        calls += 1;
        Ok(super::Assembled::Clash(vec![0]))
    })
    .unwrap_err();
    assert_eq!(calls, super::CLASH_ROUNDS + 1);
    assert!(error.message.contains("share an edge"));
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

/// A sphere laid out as one of its triangles and everything else: a patch
/// with the whole sphere's area behind a boundary of three short edges.
fn sphere_behind_a_triangle() -> crate::trace::TraceOutput {
    use crate::trace::{Patch, PatchKind, Side, TraceMesh, TraceOutput, TraceReport};
    let (positions, triangles) = icosphere(2);
    let [a, b, c] = triangles[0];
    let side = |from: u32, to: u32| Side { vertices: vec![from, to], length: 1.0 };
    let patch = |corners: [u32; 3]| Patch {
        corners: corners.to_vec(),
        sides: (0..3).map(|i| side(corners[i], corners[(i + 1) % 3])).collect(),
        singularities: Vec::new(),
        kind: PatchKind::Ok,
        cc_margin: 0.0,
    };
    let faces = triangles.len();
    let mut patch_of_face = vec![1u32; faces];
    patch_of_face[0] = 0;
    TraceOutput {
        mesh: TraceMesh {
            directions: vec![DVec3::Z; faces],
            positions,
            triangles,
            feature_edges: Vec::new(),
            singularities: Vec::new(),
        },
        patch_of_face,
        source_face: (0..faces as u32).collect(),
        // The rest of the sphere runs round the same three edges the other way.
        patches: vec![patch([a, b, c]), patch([a, c, b])],
        paths: Vec::new(),
        parts: Vec::new(),
        report: TraceReport::default(),
    }
}

#[test]
fn a_patch_that_bulges_far_past_its_boundary_keeps_its_triangles() {
    // Quads are counted along a patch's boundary, so a pattern holds about
    // as many as fit in the boundary laid flat. A patch with far more area
    // than that (here a whole sphere behind one small triangle) gets a few
    // quads drawn flat across its mouth, and what bulged behind it is gone.
    // (BrainStem: a solid part 7% of the model long, 330 triangles of it in
    // one patch of 8 quads, the result 3.3% of the diagonal off the input.)
    let trace = sphere_behind_a_triangle();
    let prepared = Prepared::new(&trace, &[], &Control::default()).unwrap();
    assert!(prepared.segs.sides[0].is_some(), "the triangle is an ordinary patch");
    assert!(prepared.segs.sides[1].is_none(), "the rest of the sphere keeps its triangles");
    let quads =
        super::compute(&trace, super::Sizing::default(), 0.3, &QuadParams::default(), &Control::default())
            .unwrap();
    assert_eq!(quads.report.kept_patches, 1);
    // The sphere is still there: every vertex on it, and vertices all round it.
    let (mut low, mut high) = (DVec3::splat(f64::INFINITY), DVec3::splat(f64::NEG_INFINITY));
    for q in &quads.mesh.positions {
        assert!((q.length() - 1.0).abs() < 0.05, "off the sphere: {q}");
        low = low.min(*q);
        high = high.max(*q);
    }
    assert!(low.max_element() < -0.9 && high.min_element() > 0.9, "{low} {high}");
}

#[test]
fn a_patch_whose_long_edges_leave_the_surface_is_refused() {
    // On a unit sphere, at a quad size of 0.1: patch 0 is one quad of that
    // size; patch 1 one whose corners lie a quarter turn apart, so its edges
    // are chords 1.4 long passing 0.29 inside the sphere.
    let (positions, triangles) = icosphere(3);
    let faces = triangles.len();
    let trace = crate::trace::TraceOutput {
        mesh: crate::trace::TraceMesh {
            directions: vec![DVec3::Z; faces],
            positions,
            triangles,
            feature_edges: Vec::new(),
            singularities: Vec::new(),
        },
        patch_of_face: vec![0; faces],
        source_face: (0..faces as u32).collect(),
        ..crate::trace::TraceOutput::default()
    };
    let on_sphere = |x: f64, y: f64, z: f64| DVec3::new(x, y, z).normalize();
    let quads = vec![
        on_sphere(0.0, 0.0, 1.0),
        on_sphere(0.1, 0.0, 1.0),
        on_sphere(0.1, 0.1, 1.0),
        on_sphere(0.0, 0.1, 1.0),
        DVec3::X,
        DVec3::Y,
        DVec3::NEG_X,
        DVec3::NEG_Y,
    ];
    let (offsets, indices, patches) = (vec![0u32, 4, 8], vec![0u32, 1, 2, 3, 4, 5, 6, 7], vec![0u32, 1]);
    assert_eq!(super::astray_patches(&quads, &offsets, &indices, &patches, &trace, 0.1), [1]);
    // Asked for quads that size, the same edges are what was asked for: nothing to refuse.
    assert!(super::astray_patches(&quads, &offsets, &indices, &patches, &trace, 1.0).is_empty());
    // A long edge that stays on the surface is no fault: a flat sheet, one quad across it.
    let sheet = crate::trace::TraceOutput {
        mesh: crate::trace::TraceMesh {
            positions: vec![
                DVec3::ZERO,
                DVec3::new(4.0, 0.0, 0.0),
                DVec3::new(4.0, 4.0, 0.0),
                DVec3::new(0.0, 4.0, 0.0),
            ],
            triangles: vec![[0, 1, 2], [0, 2, 3]],
            directions: vec![DVec3::X; 2],
            feature_edges: Vec::new(),
            singularities: Vec::new(),
        },
        patch_of_face: vec![0; 2],
        source_face: vec![0, 1],
        ..crate::trace::TraceOutput::default()
    };
    let flat = sheet.mesh.positions.clone();
    assert!(super::astray_patches(&flat, &[0, 4], &[0, 1, 2, 3], &[0], &sheet, 0.1).is_empty());
}
