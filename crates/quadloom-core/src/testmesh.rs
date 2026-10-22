//! Small meshes for unit tests (test builds only).

// Shared fixtures: not every test build uses each one.
#![allow(dead_code)]

use std::collections::HashMap;
use std::f64::consts::TAU;

use glam::DVec3;

/// A flat `nx`×`ny` grid of unit squares in z = 0, two triangles each,
/// counter-clockwise seen from +z. Vertex (i, j) is `j·(nx + 1) + i` at (i, j, 0).
pub(crate) fn grid(nx: u32, ny: u32) -> (Vec<DVec3>, Vec<[u32; 3]>) {
    let mut p = Vec::new();
    for j in 0..=ny {
        for i in 0..=nx {
            p.push(DVec3::new(f64::from(i), f64::from(j), 0.0));
        }
    }
    let id = |i: u32, j: u32| j * (nx + 1) + i;
    let mut t = Vec::new();
    for j in 0..ny {
        for i in 0..nx {
            let (a, b, c, d) = (id(i, j), id(i + 1, j), id(i + 1, j + 1), id(i, j + 1));
            t.extend([[a, b, c], [a, c, d]]);
        }
    }
    (p, t)
}

/// An open cylinder, radius 1, height 2, `rings` × `segments` quads.
pub(crate) fn cylinder(rings: u32, segments: u32) -> (Vec<DVec3>, Vec<[u32; 3]>) {
    let mut p = Vec::new();
    for i in 0..=rings {
        for j in 0..segments {
            let a = TAU * f64::from(j) / f64::from(segments);
            p.push(DVec3::new(a.cos(), a.sin(), 2.0 * f64::from(i) / f64::from(rings)));
        }
    }
    let mut t = Vec::new();
    for i in 0..rings {
        for j in 0..segments {
            let (a, b) = (i * segments + j, i * segments + (j + 1) % segments);
            t.extend([[a, b, b + segments], [a, b + segments, a + segments]]);
        }
    }
    (p, t)
}

/// A torus (radii 1 and 0.4), `rings` × `segments` quads.
pub(crate) fn torus(rings: u32, segments: u32) -> (Vec<DVec3>, Vec<[u32; 3]>) {
    let (big, small) = (1.0, 0.4);
    let mut p = Vec::new();
    for i in 0..rings {
        let u = TAU * f64::from(i) / f64::from(rings);
        for j in 0..segments {
            let v = TAU * f64::from(j) / f64::from(segments);
            let r = big + small * v.cos();
            p.push(DVec3::new(r * u.cos(), r * u.sin(), small * v.sin()));
        }
    }
    let id = |i: u32, j: u32| (i % rings) * segments + (j % segments);
    let mut t = Vec::new();
    for i in 0..rings {
        for j in 0..segments {
            let (a, b, c, d) = (id(i, j), id(i + 1, j), id(i + 1, j + 1), id(i, j + 1));
            t.extend([[a, b, c], [a, c, d]]);
        }
    }
    (p, t)
}

/// A unit cube, each side an `n`×`n` grid, and its 12 edges as features.
pub(crate) fn cube(n: u32) -> (Vec<DVec3>, Vec<[u32; 3]>, Vec<[u32; 2]>) {
    let mut p: Vec<DVec3> = Vec::new();
    let mut index: HashMap<[i64; 3], u32> = HashMap::new();
    let mut id = |q: DVec3, p: &mut Vec<DVec3>| {
        let key = (q * f64::from(n)).round().as_i64vec3().to_array();
        *index.entry(key).or_insert_with(|| {
            p.push(q);
            (p.len() - 1) as u32
        })
    };
    let mut t = Vec::new();
    // (origin, u, v): u × v points outward.
    let sides = [
        (DVec3::ZERO, DVec3::Y, DVec3::X),
        (DVec3::Z, DVec3::X, DVec3::Y),
        (DVec3::ZERO, DVec3::X, DVec3::Z),
        (DVec3::Y, DVec3::Z, DVec3::X),
        (DVec3::ZERO, DVec3::Z, DVec3::Y),
        (DVec3::X, DVec3::Y, DVec3::Z),
    ];
    let s = 1.0 / f64::from(n);
    for (o, u, v) in sides {
        for i in 0..n {
            for j in 0..n {
                let corner = |a: u32, b: u32| o + u * (f64::from(a) * s) + v * (f64::from(b) * s);
                let [q00, q10, q11, q01] =
                    [corner(i, j), corner(i + 1, j), corner(i + 1, j + 1), corner(i, j + 1)]
                        .map(|q| id(q, &mut p));
                t.extend([[q00, q10, q11], [q00, q11, q01]]);
            }
        }
    }
    let on_face = |c: f64| c.abs() < 1e-9 || (c - 1.0).abs() < 1e-9;
    let mut features = Vec::new();
    for tri in &t {
        for i in 0..3 {
            let (a, b) = (tri[i], tri[(i + 1) % 3]);
            let (pa, pb) = (p[a as usize], p[b as usize]);
            let fixed = (0..3).filter(|&k| (pa[k] - pb[k]).abs() < 1e-9 && on_face(pa[k])).count();
            if a < b && fixed == 2 {
                features.push([a, b]);
            }
        }
    }
    features.sort_unstable();
    (p, t, features)
}

/// A unit icosphere: an icosahedron with each face split `level` times into four.
pub(crate) fn icosphere(level: u32) -> (Vec<DVec3>, Vec<[u32; 3]>) {
    let g = (1.0 + 5f64.sqrt()) / 2.0;
    let mut p: Vec<DVec3> = [
        [-1.0, g, 0.0],
        [1.0, g, 0.0],
        [-1.0, -g, 0.0],
        [1.0, -g, 0.0],
        [0.0, -1.0, g],
        [0.0, 1.0, g],
        [0.0, -1.0, -g],
        [0.0, 1.0, -g],
        [g, 0.0, -1.0],
        [g, 0.0, 1.0],
        [-g, 0.0, -1.0],
        [-g, 0.0, 1.0],
    ]
    .iter()
    .map(|&c| DVec3::from_array(c).normalize())
    .collect();
    let mut t: Vec<[u32; 3]> = vec![
        [0, 11, 5],
        [0, 5, 1],
        [0, 1, 7],
        [0, 7, 10],
        [0, 10, 11],
        [1, 5, 9],
        [5, 11, 4],
        [11, 10, 2],
        [10, 7, 6],
        [7, 1, 8],
        [3, 9, 4],
        [3, 4, 2],
        [3, 2, 6],
        [3, 6, 8],
        [3, 8, 9],
        [4, 9, 5],
        [2, 4, 11],
        [6, 2, 10],
        [8, 6, 7],
        [9, 8, 1],
    ];
    for _ in 0..level {
        let mut mid: HashMap<(u32, u32), u32> = HashMap::new();
        let mut half = |a: u32, b: u32, p: &mut Vec<DVec3>| {
            *mid.entry((a.min(b), a.max(b))).or_insert_with(|| {
                p.push((p[a as usize] + p[b as usize]).normalize());
                (p.len() - 1) as u32
            })
        };
        let mut next = Vec::with_capacity(4 * t.len());
        for &[a, b, c] in &t {
            let (ab, bc, ca) = (half(a, b, &mut p), half(b, c, &mut p), half(c, a, &mut p));
            next.extend([[a, ab, ca], [b, bc, ab], [c, ca, bc], [ab, bc, ca]]);
        }
        t = next;
    }
    (p, t)
}
