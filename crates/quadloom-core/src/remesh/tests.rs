use super::*;
use crate::control::Control;

fn cube() -> (Vec<DVec3>, Vec<[u32; 3]>) {
    let p = [
        [0., 0., 0.],
        [1., 0., 0.],
        [1., 1., 0.],
        [0., 1., 0.],
        [0., 0., 1.],
        [1., 0., 1.],
        [1., 1., 1.],
        [0., 1., 1.],
    ]
    .map(DVec3::from_array)
    .to_vec();
    let t = vec![
        [0, 3, 2],
        [0, 2, 1],
        [4, 5, 6],
        [4, 6, 7],
        [0, 1, 5],
        [0, 5, 4],
        [3, 7, 6],
        [3, 6, 2],
        [0, 4, 7],
        [0, 7, 3],
        [1, 2, 6],
        [1, 6, 5],
    ];
    (p, t)
}

fn cube_features() -> Vec<[u32; 2]> {
    vec![[0, 1], [1, 2], [2, 3], [0, 3], [4, 5], [5, 6], [6, 7], [4, 7], [0, 4], [1, 5], [2, 6], [3, 7]]
}

/// A UV sphere of radius 1.
pub(crate) fn sphere_mesh(stacks: u32, slices: u32) -> (Vec<DVec3>, Vec<[u32; 3]>) {
    let mut p = vec![DVec3::Z];
    for i in 1..stacks {
        let phi = std::f64::consts::PI * f64::from(i) / f64::from(stacks);
        for j in 0..slices {
            let theta = std::f64::consts::TAU * f64::from(j) / f64::from(slices);
            p.push(DVec3::new(phi.sin() * theta.cos(), phi.sin() * theta.sin(), phi.cos()));
        }
    }
    p.push(DVec3::NEG_Z);
    let ring = |i: u32, j: u32| 1 + (i - 1) * slices + j % slices;
    let south = p.len() as u32 - 1;
    let mut t = Vec::new();
    for j in 0..slices {
        t.push([0, ring(1, j), ring(1, j + 1)]);
        t.push([south, ring(stacks - 1, j + 1), ring(stacks - 1, j)]);
    }
    for i in 1..stacks - 1 {
        for j in 0..slices {
            let (a, b, c, d) = (ring(i, j), ring(i, j + 1), ring(i + 1, j + 1), ring(i + 1, j));
            t.push([a, d, c]);
            t.push([a, c, b]);
        }
    }
    (p, t)
}

/// A closed box `1 × 1 × h`.
fn plate(h: f64) -> (Vec<DVec3>, Vec<[u32; 3]>) {
    let (p, t) = cube();
    (p.into_iter().map(|v| DVec3::new(v.x, v.y, v.z * h)).collect(), t)
}

/// Uniform sizing (adaptivity has its own test in `sizing`).
fn params(target_rel: f64) -> RemeshParams {
    RemeshParams { target_edge_rel: target_rel, adaptive: false, ..RemeshParams::default() }
}

fn run(p: &[DVec3], t: &[[u32; 3]], f: &[[u32; 2]], params: &RemeshParams) -> (RemeshedMesh, RemeshReport) {
    remesh(p, t, f, &[], 0.0, Scale::of(p, t, params, usize::MAX), params, &Control::default()).unwrap()
}

/// Like `run`, with every feature edge soft (angle-detected) and released below 35°.
fn run_soft(
    p: &[DVec3],
    t: &[[u32; 3]],
    f: &[[u32; 2]],
    params: &RemeshParams,
) -> (RemeshedMesh, RemeshReport) {
    remesh(p, t, f, f, 35.0, Scale::of(p, t, params, usize::MAX), params, &Control::default()).unwrap()
}

#[test]
fn auto_target_length_matches_oracle_formula_on_a_cube() {
    let (p, t) = cube();
    // A = 6, V = 1: sphericity S = π^(1/3)(6V)^(2/3)/A ≈ 0.806.
    // L0 = sqrt(A/2000·S²·2.309) ≈ 0.0671, L1 = sqrt(A·2.309/10000) ≈ 0.0372 → min = L1.
    let l = target_length(&p, &t, &RemeshParams::default());
    assert!((l - (6.0f64 * 2.309 / 10_000.0).sqrt()).abs() < 1e-9, "{l}");
}

#[test]
fn open_surface_target_is_floored() {
    // A flat square has no volume: the sphericity term would collapse to zero.
    let p = vec![DVec3::ZERO, DVec3::X, DVec3::new(1.0, 1.0, 0.0), DVec3::Y];
    let t = vec![[0, 1, 2], [0, 2, 3]];
    let l1 = (1.0f64 * 2.309 / 10_000.0).sqrt();
    let l = target_length(&p, &t, &RemeshParams::default());
    assert!((l - 0.25 * l1).abs() < 1e-12, "{l} vs floor {}", 0.25 * l1);
}

#[test]
fn coarse_cube_refined_to_target() {
    let (p, t) = cube();
    let (mesh, report) = run(&p, &t, &cube_features(), &params(0.1));
    assert!(mesh.triangles.len() >= 300, "{} faces", mesh.triangles.len());
    assert!(mesh.feature_edges.len() > 12);
    for corner in &p {
        assert!(mesh.positions.iter().any(|q| (*q - *corner).length() < 1e-12), "corner {corner:?} lost");
    }
    // Every feature vertex lies on a cube edge: two coordinates in {0, 1}.
    for e in &mesh.feature_edges {
        for &v in e {
            let q = mesh.positions[v as usize];
            let on = [q.x, q.y, q.z].iter().filter(|c| c.abs() < 1e-9 || (*c - 1.0).abs() < 1e-9).count();
            assert!(on >= 2, "feature vertex {q:?} left its cube edge");
        }
    }
    assert!(report.max_surface_distance < 1e-9, "{}", report.max_surface_distance);
}

#[test]
fn sphere_becomes_isotropic() {
    // A coarse 224-face polyhedral sphere. Under the default bound (1/2500 of
    // the diagonal) faces must hug its facets, so it can't become isotropic;
    // with a loose bound the remesher is free to.
    let (p, t) = sphere_mesh(8, 16);
    let prm = RemeshParams { max_surface_distance_rel: 0.01, ..params(0.08) };
    let (mesh, report) = run(&p, &t, &[], &prm);
    assert!(mesh.triangles.len() > 200);
    assert!(report.mean_radius_ratio > 0.85, "{report:?}");
    assert!(report.min_angle_deg > 20.0, "{report:?}");
}

#[test]
fn distance_bound_respected() {
    let (p, t) = sphere_mesh(8, 16);
    let prm = params(0.08);
    let (_, report) = run(&p, &t, &[], &prm);
    let diag = (DVec3::splat(2.0)).length();
    assert!(report.max_surface_distance <= prm.max_surface_distance_rel * diag * 1.05, "{report:?}");
}

#[test]
fn open_plane_boundary_stays() {
    let mut p = Vec::new();
    for y in 0..=4 {
        for x in 0..=4 {
            p.push(DVec3::new(f64::from(x) / 4.0, f64::from(y) / 4.0, 0.0));
        }
    }
    let mut t = Vec::new();
    for y in 0..4u32 {
        for x in 0..4u32 {
            let a = y * 5 + x;
            t.extend([[a, a + 1, a + 6], [a, a + 6, a + 5]]);
        }
    }
    let (mesh, _) = run(&p, &t, &[], &params(0.05));
    let boundary =
        crate::mesh::halfedge::HalfEdges::build(&mesh.triangles, mesh.positions.len()).boundary_edge_count();
    assert!(boundary > 16, "the boundary should be refined, not closed ({boundary} edges)");
    for q in &mesh.positions {
        assert!(q.z.abs() < 1e-12);
        assert!(
            (-1e-9..=1.0 + 1e-9).contains(&q.x) && (-1e-9..=1.0 + 1e-9).contains(&q.y),
            "{q:?} left the square"
        );
    }
    for corner in [DVec3::ZERO, DVec3::X, DVec3::new(1., 1., 0.), DVec3::Y] {
        assert!(
            mesh.positions.iter().any(|q| (*q - corner).length() < 1e-12),
            "square corner {corner:?} lost"
        );
    }
}

#[test]
fn thin_plate_not_collapsed_through() {
    let (p, t) = plate(0.02);
    let (mesh, _) = run(&p, &t, &cube_features(), &params(0.1));
    let he = crate::mesh::halfedge::HalfEdges::build(&mesh.triangles, mesh.positions.len());
    assert_eq!(he.boundary_edge_count(), 0);
    assert_eq!(he.non_manifold_edge_count(), 0);
    let volume = crate::repair::orient::signed_volume(&mesh.positions, &mesh.triangles) / 6.0;
    assert!((volume - 0.02).abs() < 0.01, "volume {volume} (expected ≈ 0.02)");
    // Every face lies on one of the box's six sides and faces outward: nothing
    // folded through or bridged across the thin gap.
    let h = 0.02;
    for &[a, b, c] in &mesh.triangles {
        let [pa, pb, pc] = [a, b, c].map(|v| mesh.positions[v as usize]);
        let n = (pb - pa).cross(pc - pa).normalize_or_zero();
        let centre = (pa + pb + pc) / 3.0;
        let on = |value: f64, plane: f64| (value - plane).abs() < 1e-9;
        let ok = (n.z > 0.99 && on(centre.z, h))
            || (n.z < -0.99 && on(centre.z, 0.0))
            || (n.x > 0.99 && on(centre.x, 1.0))
            || (n.x < -0.99 && on(centre.x, 0.0))
            || (n.y > 0.99 && on(centre.y, 1.0))
            || (n.y < -0.99 && on(centre.y, 0.0));
        assert!(ok, "face at {centre:?} with normal {n:?} isn't on an outward box side");
    }
}

#[test]
fn deterministic() {
    let (p, t) = sphere_mesh(6, 12);
    let a = run(&p, &t, &[], &params(0.1)).0;
    let b = run(&p, &t, &[], &params(0.1)).0;
    assert_eq!(a, b);
}

#[test]
fn cancel_honoured() {
    let (p, t) = sphere_mesh(6, 12);
    let control = Control::default();
    control.cancel();
    let prm = params(0.1);
    let target = target_length(&p, &t, &prm);
    let scale = Scale { target, ..Scale::of(&p, &t, &prm, usize::MAX) };
    assert!(remesh(&p, &t, &[], &[], 0.0, scale, &prm, &control).is_err());
}

#[test]
#[ignore = "performance check; run with --release -- --ignored"]
fn perf_dense_scan_remeshes_in_time() {
    // A 708×708 wavy grid ≈ 1M triangles, remeshed at the automatic target
    // (≈155k output faces, the oracle's density). Budget 40 s: ~29–31 s here,
    // memory-bound on the directed-edge map (see decision 0004).
    let n = 708u32;
    let mut p = Vec::new();
    for y in 0..=n {
        for x in 0..=n {
            let (u, v) = (f64::from(x) / f64::from(n), f64::from(y) / f64::from(n));
            p.push(DVec3::new(u, v, 0.05 * (6.0 * u).sin() * (4.0 * v).cos()));
        }
    }
    let w = n + 1;
    let mut t = Vec::new();
    for y in 0..n {
        for x in 0..n {
            let a = y * w + x;
            t.extend([[a, a + 1, a + w + 1], [a, a + w + 1, a + w]]);
        }
    }
    let prm = RemeshParams::default();
    let started = std::time::Instant::now();
    let (mesh, report) = run(&p, &t, &[], &prm);
    let elapsed = started.elapsed();
    eprintln!("1M triangles → {} in {elapsed:?}: {report:?}", mesh.triangles.len());
    assert!(elapsed.as_secs_f64() < 40.0, "{elapsed:?}");
}

/// A unit square triangulated as a strip of needles between its bottom and top
/// edges (like a coarsely tessellated developable surface): every triangle is
/// 1 long and 1/n wide.
fn sliver_strip(n: u32) -> (Vec<DVec3>, Vec<[u32; 3]>) {
    let mut p = Vec::new();
    for i in 0..=n {
        p.push(DVec3::new(f64::from(i) / f64::from(n), 0.0, 0.0));
    }
    for i in 0..=n {
        p.push(DVec3::new(f64::from(i) / f64::from(n), 1.0, 0.0));
    }
    let top = n + 1;
    let mut t = Vec::new();
    for i in 0..n {
        t.push([i, i + 1, top + i + 1]);
        t.push([i, top + i + 1, top + i]);
    }
    (p, t)
}

#[test]
fn sliver_strip_is_repaired_not_exploded() {
    let (p, t) = sliver_strip(200);
    let prm = params(0.05);
    let target = target_length(&p, &t, &prm);
    let expected = 1.0 / (0.433 * target * target);
    let (mesh, report) = run(&p, &t, &[], &prm);
    assert!(
        (mesh.triangles.len() as f64) < 4.0 * expected,
        "{} faces for ~{expected:.0} expected: {report:?}",
        mesh.triangles.len()
    );
    assert!(report.mean_radius_ratio > 0.8, "{report:?}");
    assert!(report.min_angle_deg > 15.0, "{report:?}");
}

#[test]
fn noisy_soft_features_are_released() {
    // Every interior rib of the sliver strip marked as an (angle-detected)
    // feature, like the noise on a coarsely tessellated developable surface.
    let (p, t) = sliver_strip(200);
    let ribs: Vec<[u32; 2]> = (0..=200u32).map(|i| [i, 201 + i]).collect();
    let prm = params(0.05);
    let target = target_length(&p, &t, &prm);
    let expected = 1.0 / (0.433 * target * target);
    let (mesh, report) = run_soft(&p, &t, &ribs, &prm);
    assert!(report.released_features > 0, "{report:?}");
    assert!((mesh.triangles.len() as f64) < 4.0 * expected, "{} faces: {report:?}", mesh.triangles.len());
    assert!(report.mean_radius_ratio > 0.8, "{report:?}");
}

#[test]
fn real_creases_survive_release() {
    let (p, t) = cube();
    let (mesh, report) = run_soft(&p, &t, &cube_features(), &params(0.1));
    assert_eq!(report.released_features, 0, "cube edges are 90° creases");
    assert!(mesh.feature_edges.len() > 12);
    for corner in &p {
        assert!(mesh.positions.iter().any(|q| (*q - *corner).length() < 1e-12), "corner {corner:?} lost");
    }
}

#[test]
fn corrugated_noise_features_released() {
    // Ribs every 1/2000 alternate ±0.0002 in z: neighbouring slivers meet at
    // ~77° (so the angle test fires), yet at the target scale the strip is flat
    // and well within the distance bound. That's tessellation noise, not creases.
    let (p, t, ribs) = corrugated_strip();
    let prm = params(0.05);
    let target = target_length(&p, &t, &prm);
    let expected = 1.0 / (0.433 * target * target);
    let (mesh, report) = run_soft(&p, &t, &ribs, &prm);
    assert!(report.released_features > ribs.len() / 2, "{report:?}");
    assert!((mesh.triangles.len() as f64) < 4.0 * expected, "{} faces: {report:?}", mesh.triangles.len());
    assert!(report.mean_radius_ratio > 0.8, "{report:?}");
}

/// The sliver strip with ribs every 1/2000 alternating ±0.0002 in z, plus its
/// interior ribs (the edges an angle test flags).
fn corrugated_strip() -> (Vec<DVec3>, Vec<[u32; 3]>, Vec<[u32; 2]>) {
    let n = 2000u32;
    let (mut p, t) = sliver_strip(n);
    for (i, q) in p.iter_mut().enumerate() {
        let rib = (i as u32) % (n + 1);
        q.z = if rib % 2 == 0 { 0.0002 } else { -0.0002 };
    }
    let ribs = (1..n).map(|i| [i, n + 1 + i]).collect();
    (p, t, ribs)
}

#[test]
fn adaptive_remesh_of_corrugated_slivers_neither_folds_nor_explodes() {
    // Regression: at vertex scale the corrugation looks like strong curvature,
    // and sliver normals swing with tiny moves, so faces used to fold over
    // step by step while the sizing refined the folds further (18k faces).
    let (p, t, ribs) = corrugated_strip();
    let prm = RemeshParams { adaptive: true, ..params(0.05) };
    let target = target_length(&p, &t, &prm);
    let expected = 1.0 / (0.433 * target * target);
    let (mesh, report) = run_soft(&p, &t, &ribs, &prm);
    assert!((mesh.triangles.len() as f64) < 2.0 * expected, "{} faces: {report:?}", mesh.triangles.len());
    for &[a, b, c] in &mesh.triangles {
        let [pa, pb, pc] = [a, b, c].map(|v| mesh.positions[v as usize]);
        let n = (pb - pa).cross(pc - pa).normalize_or_zero();
        assert!(n.z > 0.5, "folded face {:?}: {report:?}", [pa, pb, pc]);
    }
}

#[test]
fn deviation_bound_limits_coarsening() {
    // A fine unit sphere at a target far too coarse for it: collapses stop
    // where the surface would pull away from the input by more than the bound.
    // Flips and smoothing aren't bounded (bounding flips cost quality and
    // time), so the result may sit a little further out; unbounded, this
    // target would leave ~15 faces, about 100× the bound away.
    let (p, t) = sphere_mesh(32, 64);
    let prm = RemeshParams { target_edge_rel: 0.4, max_surface_distance_rel: 0.002, ..params(0.4) };
    let bound = 0.002 * bbox_diagonal(&p);
    let (mesh, report) = run(&p, &t, &[], &prm);
    assert!(mesh.triangles.len() < t.len(), "{report:?}");
    let output = crate::spatial::TriangleBvh::new(&mesh.positions, &mesh.triangles);
    let worst = p.iter().filter_map(|&q| output.closest_point(q)).map(|(_, d)| d).fold(0.0, f64::max);
    assert!(worst <= 3.0 * bound, "input vertex {:.2}× the bound from the result: {report:?}", worst / bound);
}

#[test]
fn split_sweep_reaches_edges_whose_faces_were_replaced() {
    // One triangle with all three edges too long. Splitting the first replaces
    // the face, so the other two edges get new half-edge ids mid-sweep; the
    // sweep must still split them (it used to skip them as stale).
    let p = vec![DVec3::ZERO, DVec3::X, DVec3::new(0.5, 0.8, 0.0)];
    let t = vec![[0, 1, 2]];
    let mut mesh = crate::edit::EditMesh::from_triangles(&p, &t, &[]).unwrap();
    let projector = Projector::new(&p, &t, &[], 1.0, 1.0);
    let control = Control::default();
    let mut pass = passes::Pass {
        mesh: &mut mesh,
        sizes: vec![0.1; 3],
        projector: &projector,
        max_faces: 1000,
        control: &control,
    };
    assert_eq!(pass.split_long(), 3);
}

/// A closed cylindrical shell: outer radius 1, inner 1 − `t`, height 0.5,
/// with annular caps. Returns the mesh and its four rim circles as features.
fn cylinder_shell(t: f64, rings: u32, segments: u32) -> (Vec<DVec3>, Vec<[u32; 3]>, Vec<[u32; 2]>) {
    let mut p = Vec::new();
    let ring = |r: f64, z: f64, p: &mut Vec<DVec3>| -> u32 {
        let start = p.len() as u32;
        for j in 0..segments {
            let a = std::f64::consts::TAU * f64::from(j) / f64::from(segments);
            p.push(DVec3::new(r * a.cos(), r * a.sin(), z));
        }
        start
    };
    let outer: Vec<u32> =
        (0..=rings).map(|i| ring(1.0, 0.5 * f64::from(i) / f64::from(rings), &mut p)).collect();
    let inner: Vec<u32> =
        (0..=rings).map(|i| ring(1.0 - t, 0.5 * f64::from(i) / f64::from(rings), &mut p)).collect();
    let mut tris = Vec::new();
    // Quad strip between rings `a` and `b`, oriented by `flip`.
    let mut strip = |a: u32, b: u32, flip: bool| {
        for j in 0..segments {
            let k = (j + 1) % segments;
            let (q0, q1, q2, q3) = (a + j, a + k, b + k, b + j);
            if flip {
                tris.extend([[q0, q2, q1], [q0, q3, q2]]);
            } else {
                tris.extend([[q0, q1, q2], [q0, q2, q3]]);
            }
        }
    };
    for i in 0..rings as usize {
        strip(outer[i], outer[i + 1], false); // outward
        strip(inner[i], inner[i + 1], true); // inward
    }
    strip(inner[0], outer[0], false); // bottom cap, facing down
    strip(outer[rings as usize], inner[rings as usize], false); // top cap, facing up
    let circle = |start: u32| (0..segments).map(move |j| [start + j, start + (j + 1) % segments]);
    let features = circle(outer[0])
        .chain(circle(outer[rings as usize]))
        .chain(circle(inner[0]))
        .chain(circle(inner[rings as usize]))
        .collect();
    (p, tris, features)
}

#[test]
fn thin_shell_sheets_stay_apart() {
    // Regression (review of M3b): the wall is 0.002 thick, about twice the
    // deviation bound. Projection used to snap to whichever sheet was nearer,
    // so faces bridged the outer and inner sheets mid-wall and some flipped.
    let t = 0.002;
    let (p, tris, features) = cylinder_shell(t, 20, 160);
    let prm = RemeshParams { target_edge_rel: 0.03, ..RemeshParams::default() };
    let (mesh, report) = run(&p, &tris, &features, &prm);
    let radius = |q: DVec3| q.truncate().length();
    let (mut bridging, mut flipped) = (0, 0);
    for &[a, b, c] in &mesh.triangles {
        let [pa, pb, pc] = [a, b, c].map(|v| mesh.positions[v as usize]);
        let centroid = (pa + pb + pc) / 3.0;
        if centroid.z < 0.01 || centroid.z > 0.49 {
            continue; // the caps join the sheets legitimately
        }
        let rs = [pa, pb, pc].map(radius);
        let on_outer = rs.iter().any(|&r| r > 1.0 - 0.25 * t);
        let on_inner = rs.iter().any(|&r| r < 1.0 - 0.75 * t);
        bridging += usize::from(on_outer && on_inner);
        let outward = DVec3::new(centroid.x, centroid.y, 0.0).normalize_or_zero();
        let normal = (pb - pa).cross(pc - pa).normalize_or_zero();
        // Which sheet by its vertices (a coarse chord's centroid sinks inside).
        let expected = if on_outer { 1.0 } else { -1.0 };
        flipped += usize::from(normal.dot(outward) * expected < 0.0);
    }
    assert_eq!((bridging, flipped), (0, 0), "{} faces: {report:?}", mesh.triangles.len());
}

/// A flat grid with a square feature loop (see [`grid_with_loop`]).
struct LoopGrid {
    p: Vec<DVec3>,
    t: Vec<[u32; 3]>,
    ring: Vec<[u32; 2]>,
    corners: [DVec3; 4],
}

/// A flat 1×1 grid of `n`×`n` cells and a square feature loop `cells` cells
/// wide with its lower-left corner at grid point (`n`/2, `n`/2): the mesh, the
/// loop's edges and its four corner positions.
fn grid_with_loop(n: u32, cells: u32) -> LoopGrid {
    let w = n + 1;
    let mut p = Vec::new();
    for y in 0..=n {
        for x in 0..=n {
            p.push(DVec3::new(f64::from(x) / f64::from(n), f64::from(y) / f64::from(n), 0.0));
        }
    }
    let mut t = Vec::new();
    for y in 0..n {
        for x in 0..n {
            let a = y * w + x;
            t.extend([[a, a + 1, a + w + 1], [a, a + w + 1, a + w]]);
        }
    }
    let (x0, y0) = (n / 2, n / 2);
    let id = |x: u32, y: u32| y * w + x;
    let mut ring = Vec::new();
    for i in 0..cells {
        ring.push([id(x0 + i, y0), id(x0 + i + 1, y0)]);
        ring.push([id(x0 + cells, y0 + i), id(x0 + cells, y0 + i + 1)]);
        ring.push([id(x0 + i, y0 + cells), id(x0 + i + 1, y0 + cells)]);
        ring.push([id(x0, y0 + i), id(x0, y0 + i + 1)]);
    }
    let corners = [id(x0, y0), id(x0 + cells, y0), id(x0 + cells, y0 + cells), id(x0, y0 + cells)]
        .map(|v| p[v as usize]);
    LoopGrid { p, t, ring, corners }
}

#[test]
fn small_feature_loops_keep_their_corners() {
    // Regression (review of M3b): loops 0.28× and 0.42× the target lost their
    // corners or vanished. Corner suppression judged them at a reach longer
    // than the loop, and feature collapses weren't bounded along the line.
    for cells in [2, 3] {
        let LoopGrid { p, t, ring, corners } = grid_with_loop(100, cells);
        let prm = params(0.05);
        let (mesh, report) = run(&p, &t, &ring, &prm);
        for corner in corners {
            assert!(
                mesh.positions.iter().any(|q| (*q - corner).length() < 1e-9),
                "loop of {cells} cells lost corner {corner:?}: {report:?}"
            );
        }
        let interior: Vec<&[u32; 2]> = mesh
            .feature_edges
            .iter()
            .filter(|&&[a, b]| {
                let inside = |q: DVec3| q.x > 0.01 && q.x < 0.99 && q.y > 0.01 && q.y < 0.99;
                inside(mesh.positions[a as usize]) && inside(mesh.positions[b as usize])
            })
            .collect();
        assert!(interior.len() >= 4, "loop of {cells} cells kept {} edges: {report:?}", interior.len());
    }
}
