use glam::DVec3;

use super::*;
use crate::error::ErrorKind;
use crate::testmesh::grid;

/// A quad grid of `nx` × `ny` faces over [0, w] × [0, h] at height `z`.
fn quads(nx: u32, ny: u32, w: f64, h: f64, z: f64) -> (Vec<DVec3>, Vec<u32>, Vec<u32>) {
    let mut p = Vec::new();
    for j in 0..=ny {
        for i in 0..=nx {
            p.push(DVec3::new(w * f64::from(i) / f64::from(nx), h * f64::from(j) / f64::from(ny), z));
        }
    }
    let id = |i: u32, j: u32| j * (nx + 1) + i;
    let mut corners = Vec::new();
    for j in 0..ny {
        for i in 0..nx {
            corners.extend([id(i, j), id(i + 1, j), id(i + 1, j + 1), id(i, j + 1)]);
        }
    }
    let offsets = (0..=nx * ny).map(|f| 4 * f).collect();
    (p, offsets, corners)
}

fn source<'a>(p: &'a [DVec3], t: &'a [[u32; 3]]) -> Source<'a> {
    Source { positions: p, triangles: t, corner_keys: None, face_keys: None }
}

fn point(s: Source<'_>, sample: Sample) -> DVec3 {
    let tri = s.triangles[sample.triangle as usize];
    (0..3).map(|k| s.positions[tri[k] as usize] * f64::from(sample.weights[k])).sum()
}

#[test]
fn samples_reproduce_the_points_they_map() {
    let (sp, st) = grid(6, 4);
    let s = source(&sp, &st);
    let (tp, offsets, corners) = quads(5, 3, 6.0, 4.0, 0.0);
    let t = Target { positions: &tp, offsets: &offsets, corners: &corners };
    let map = map(s, t, &Control::default()).unwrap();
    assert_eq!(map.vertices.len(), tp.len());
    assert_eq!(map.corners.len(), corners.len());
    assert_eq!(map.faces.len(), offsets.len() - 1);
    for (v, &sample) in map.vertices.iter().enumerate() {
        assert!((sample.weights.iter().sum::<f32>() - 1.0).abs() < 1e-5);
        assert!(point(s, sample).distance(tp[v]) < 1e-5, "vertex {v}");
    }
    for (c, &sample) in map.corners.iter().enumerate() {
        assert!(point(s, sample).distance(tp[corners[c] as usize]) < 1e-5, "corner {c}");
    }
}

/// Corner keys for `grid` triangles: `10 v`, plus 1 on the triangles where
/// `cut(triangle centre, vertex)` holds (so values jump there).
fn keys_cut(p: &[DVec3], t: &[[u32; 3]], cut: impl Fn(DVec3, DVec3) -> bool) -> Vec<u32> {
    t.iter()
        .flat_map(|tri| {
            let centre = tri.iter().map(|&v| p[v as usize]).sum::<DVec3>() / 3.0;
            tri.map(|v| 10 * v + u32::from(cut(centre, p[v as usize])))
        })
        .collect()
}

#[test]
fn corners_stay_on_their_side_of_a_seam() {
    // Two islands, split at x = 1.
    let (sp, st) = grid(2, 1);
    let keys = keys_cut(&sp, &st, |c, _| c.x > 1.0);
    let s = Source { corner_keys: Some(&keys), ..source(&sp, &st) };
    // Result: two quads meeting on the seam.
    let (tp, offsets, corners) = quads(2, 1, 2.0, 1.0, 0.0);
    let t = Target { positions: &tp, offsets: &offsets, corners: &corners };
    let map = map(s, t, &Control::default()).unwrap();
    for (c, sample) in map.corners.iter().enumerate() {
        let right_face = c >= 4;
        assert_eq!(sample.triangle >= 2, right_face, "corner {c} crossed the seam: {sample:?}");
        assert!(point(s, *sample).distance(tp[corners[c] as usize]) < 1e-5);
    }
}

#[test]
fn a_seam_that_does_not_split_the_mesh_still_holds() {
    // A 2 x 2 grid cut along x = 1 in the top row only: one island, but the
    // corners at (1, 2) differ on the two sides of the cut.
    let (sp, st) = grid(2, 2);
    let keys = keys_cut(&sp, &st, |c, v| c.x > 1.0 && c.y > 1.0 && v.x == 1.0 && v.y == 2.0);
    let s = Source { corner_keys: Some(&keys), ..source(&sp, &st) };
    let (tp, offsets, corners) = quads(2, 2, 2.0, 2.0, 0.0);
    let t = Target { positions: &tp, offsets: &offsets, corners: &corners };
    let map = map(s, t, &Control::default()).unwrap();
    let top_middle = 7; // vertex (1, 2) of the result
    for (c, sample) in map.corners.iter().enumerate() {
        if corners[c] != top_middle {
            continue;
        }
        let face_right = tp[corners[4 * (c / 4)] as usize].x >= 1.0;
        let tri = st[sample.triangle as usize];
        let centre = tri.iter().map(|&v| sp[v as usize]).sum::<DVec3>() / 3.0;
        assert_eq!(centre.x > 1.0, face_right, "corner {c} took the other side of the cut");
    }
}

#[test]
fn neighbours_are_joined_only_where_keys_match_on_both_ends() {
    let (sp, st) = grid(2, 1);
    let same: Vec<u32> = st.iter().flatten().copied().collect();
    let joined = Neighbours::continuous(&st, &same);
    assert_eq!(joined.of(0), [1, 3]);
    let cut = keys_cut(&sp, &st, |c, _| c.x > 1.0);
    let split = Neighbours::continuous(&st, &cut);
    assert_eq!(split.of(0), [1]);
    assert_eq!(split.of(3), [2]);
}

#[test]
fn a_thin_part_does_not_borrow_its_far_side() {
    // A top sheet facing +z at z = 0.01 above a bottom sheet facing -z at z = 0.
    let (mut sp, top) = grid(1, 1);
    for p in &mut sp {
        p.z = 0.01;
    }
    let n = sp.len() as u32;
    let bottom: Vec<DVec3> = sp.iter().map(|p| DVec3::new(p.x, p.y, 0.0)).collect();
    sp.extend(bottom);
    let mut st = top.clone();
    st.extend(top.iter().map(|t| [t[0] + n, t[2] + n, t[1] + n]));
    let s = source(&sp, &st);
    // A result face facing +z, nearer the bottom sheet.
    let (tp, offsets, corners) = quads(1, 1, 1.0, 1.0, 0.004);
    let t = Target { positions: &tp, offsets: &offsets, corners: &corners };
    let map = map(s, t, &Control::default()).unwrap();
    assert!(map.vertices.iter().chain(&map.corners).all(|s| s.triangle < 2), "{map:?}");
    assert!(map.faces[0] < 2);
}

#[test]
fn a_thin_strip_does_not_win_a_face_by_its_centre() {
    // An 8 × 8 grid; the strip of key 1 runs through the middle column.
    let (sp, st) = grid(8, 8);
    let keys: Vec<u32> = st
        .iter()
        .map(|t| {
            let x = t.iter().map(|&v| sp[v as usize].x).sum::<f64>() / 3.0;
            u32::from((4.0..5.0).contains(&x))
        })
        .collect();
    let s = Source { face_keys: Some(&keys), ..source(&sp, &st) };
    // One result quad centred on the strip.
    let tp = [
        DVec3::new(0.5, 0.5, 0.0),
        DVec3::new(8.5, 0.5, 0.0),
        DVec3::new(8.5, 7.5, 0.0),
        DVec3::new(0.5, 7.5, 0.0),
    ];
    let t = Target { positions: &tp, offsets: &[0, 4], corners: &[0, 1, 2, 3] };
    let map = map(s, t, &Control::default()).unwrap();
    assert_eq!(keys[map.faces[0] as usize], 0);
    // Without keys the centre's triangle (on the strip) represents the face.
    let plain = super::map(source(&sp, &st), t, &Control::default()).unwrap();
    assert_eq!(keys[plain.faces[0] as usize], 1);
}

#[test]
fn bad_input_is_refused() {
    let (sp, st) = grid(1, 1);
    let tp = [DVec3::ZERO, DVec3::X, DVec3::Y];
    let ok = Target { positions: &tp, offsets: &[0, 3], corners: &[0, 1, 2] };
    let refused = |s: Source<'_>, t: Target<'_>| {
        map(s, t, &Control::default()).is_err_and(|e| e.kind == ErrorKind::InvalidInput)
    };
    assert!(refused(source(&sp, &[]), ok));
    assert!(refused(source(&sp, &[[0, 1, 9]]), ok));
    assert!(refused(Source { corner_keys: Some(&[0; 5]), ..source(&sp, &st) }, ok));
    assert!(refused(Source { face_keys: Some(&[0; 3]), ..source(&sp, &st) }, ok));
    assert!(refused(source(&sp, &st), Target { offsets: &[0, 2], corners: &[0, 1], ..ok }));
    assert!(refused(source(&sp, &st), Target { corners: &[0, 1, 7], ..ok }));
    assert!(refused(source(&sp, &st), Target { offsets: &[1, 3], ..ok }));
}

#[test]
fn cancel_stops_the_mapping() {
    let (sp, st) = grid(2, 2);
    let (tp, offsets, corners) = quads(2, 2, 2.0, 2.0, 0.0);
    let control = Control::default();
    control.cancel();
    let t = Target { positions: &tp, offsets: &offsets, corners: &corners };
    let error = map(source(&sp, &st), t, &control).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Cancelled);
}

#[test]
fn a_degenerate_triangle_weights_its_nearest_corner() {
    let a = DVec3::ZERO;
    let w = barycentric(DVec3::new(2.1, 0.0, 0.0), a, DVec3::X * 2.0, DVec3::X * 2.0);
    assert!(w[1] + w[2] > 0.99, "{w:?}");
}
