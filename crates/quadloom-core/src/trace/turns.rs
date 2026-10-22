//! How a boundary turns at a vertex, measured in the field.
//!
//! Walking a boundary loop with the patch on the left, each vertex is
//! classified by the patch's interior angle there in quarter turns of the
//! field (the paper's Fig. 6): convex corners (1), flat (2), concave (3),
//! narrow U-turns such as a dangling feature end (4), and cusps (0). The
//! turn is read from the field, not the geometry: the incoming edge's field
//! direction is carried face to face through the patch's fan to the
//! outgoing edge. Each boundary edge has a *label*, the field direction it
//! travels along (for a traced path, the path's direction; for a feature,
//! the edge itself), so a jagged path still counts as straight.

use std::f64::consts::{FRAC_PI_2, PI};

use glam::DVec3;

use super::cut::CutMesh;
use super::graph::transport;

/// A boundary vertex's interior angle in quarter turns of the field.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum VertexClass {
    Cusp,
    Convex,
    Flat,
    Concave,
    Narrow,
}

impl VertexClass {
    pub fn from_quarters(q: u8) -> Self {
        match q {
            0 => Self::Cusp,
            1 => Self::Convex,
            2 => Self::Flat,
            3 => Self::Concave,
            _ => Self::Narrow,
        }
    }

    pub fn quarters(self) -> u8 {
        self as u8
    }

    /// Allowed on a valid patch's boundary (the paper's convexity condition).
    pub fn is_convex(self) -> bool {
        matches!(self, Self::Convex | Self::Flat)
    }
}

/// Each face's normal and its cross's four directions.
#[derive(Clone, Debug)]
pub struct FaceCrosses {
    normal: Vec<DVec3>,
    dir: Vec<DVec3>,
}

impl FaceCrosses {
    /// `directions` holds one unit tangent direction per face.
    pub fn new(cut: &CutMesh, directions: &[DVec3]) -> Self {
        let normal: Vec<DVec3> = cut
            .triangles
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|v| cut.positions[v as usize]);
                (b - a).cross(c - a).normalize_or_zero()
            })
            .collect();
        let dir = normal
            .iter()
            .enumerate()
            .map(|(f, &n)| {
                let u = directions.get(f).copied().unwrap_or(DVec3::ZERO);
                if n == DVec3::ZERO { DVec3::ZERO } else { (u - n * u.dot(n)).normalize_or_zero() }
            })
            .collect();
        Self { normal, dir }
    }

    pub fn normal(&self, f: u32) -> DVec3 {
        self.normal[f as usize]
    }

    /// Direction `k` (counter-clockwise about the normal) of face `f`'s cross.
    pub fn direction(&self, f: u32, k: u8) -> Option<DVec3> {
        let (u, n) = (self.dir[f as usize], self.normal[f as usize]);
        if u == DVec3::ZERO {
            return None;
        }
        Some(match k % 4 {
            0 => u,
            1 => n.cross(u),
            2 => -u,
            _ => -n.cross(u),
        })
    }

    /// The index and vector of `f`'s cross direction closest to `d`.
    pub fn closest(&self, f: u32, d: DVec3) -> Option<(u8, DVec3)> {
        (0..4u8)
            .filter_map(|k| self.direction(f, k).map(|x| (k, x)))
            .max_by(|a, b| a.1.dot(d).total_cmp(&b.1.dot(d)).then(b.0.cmp(&a.0)))
    }
}

/// The turn at the end of a boundary half-edge.
#[derive(Clone, Debug, PartialEq)]
pub struct Turn {
    pub class: VertexClass,
    /// The next boundary half-edge of the loop (leaving the same vertex).
    pub outgoing: u32,
    /// The patch's faces around the vertex, from `incoming`'s face to `outgoing`'s.
    pub faces: Vec<u32>,
    /// The geometric interior angle, in radians.
    pub angle: f64,
}

/// The turn at the head of boundary half-edge `incoming` (its face is on the
/// patch). `is_cut(h)` says whether half-edge `h` is on a boundary, and
/// `label(h)` gives a boundary half-edge's travel direction. Where
/// `geometric[v]` holds (the field is singular at v, so carrying it around v
/// is meaningless) the geometric angle decides instead.
pub fn turn_at(
    cut: &CutMesh,
    crosses: &FaceCrosses,
    geometric: &[bool],
    incoming: u32,
    is_cut: &dyn Fn(u32) -> bool,
    label: &dyn Fn(u32) -> DVec3,
) -> Turn {
    let b = cut.head(incoming);
    let p = cut.positions[b as usize];
    let mut faces = vec![incoming / 3];
    let mut g = CutMesh::next(incoming);
    for _ in 0..3 * cut.face_count() {
        if is_cut(g) {
            break;
        }
        match cut.twin(g) {
            Some(t) => {
                g = CutMesh::next(t);
                faces.push(g / 3);
            }
            None => break,
        }
    }
    let outgoing = g;
    // The angle of each fan face at b: between its outgoing edge and the
    // edge arriving at b.
    let angle: f64 = faces
        .iter()
        .map(|&f| {
            let t = cut.triangles[f as usize];
            let k = t.iter().position(|&v| v == b).unwrap_or(0);
            let (x, y) = (cut.positions[t[(k + 1) % 3] as usize], cut.positions[t[(k + 2) % 3] as usize]);
            (x - p).angle_between(y - p)
        })
        .sum();
    let by_angle = || ((angle / FRAC_PI_2).round().clamp(0.0, 4.0)) as u8;
    let by_field = if geometric.get(b as usize).copied().unwrap_or(false) {
        None
    } else {
        field_turn(crosses, &faces, label(incoming), label(outgoing))
    };
    let class = match by_field {
        Some(t) => {
            let r = (2 + 4 - t) % 4;
            VertexClass::from_quarters(if r == 0 { if angle > PI { 4 } else { 0 } } else { r })
        }
        None => VertexClass::from_quarters(by_angle()),
    };
    Turn { class, outgoing, faces, angle }
}

/// The counter-clockwise turn in quarters (mod 4) from direction `din` in
/// the first face to `dout` in the last, carrying the field through `faces`.
fn field_turn(crosses: &FaceCrosses, faces: &[u32], din: DVec3, dout: DVec3) -> Option<u8> {
    let mut carried: Option<(u32, DVec3)> = None;
    let mut index_in_last = 0u8;
    for &f in faces {
        match carried {
            None => {
                if let Some((k, d)) = crosses.closest(f, din) {
                    carried = Some((f, d));
                    index_in_last = k;
                }
            }
            Some((from, d)) => {
                let moved = transport(d, crosses.normal(from), crosses.normal(f));
                if let Some((k, snapped)) = crosses.closest(f, moved) {
                    carried = Some((f, snapped));
                    index_in_last = k;
                }
            }
        }
    }
    let (last, _) = carried?;
    let (out, _) = crosses.closest(last, dout)?;
    Some((out + 4 - index_in_last) % 4)
}

/// The class of every boundary vertex of `cut` (None for interior ones),
/// with boundary edges labelled by `label` (e.g. the edge itself).
pub fn classify_boundary(
    cut: &CutMesh,
    crosses: &FaceCrosses,
    geometric: &[bool],
    label: &dyn Fn(u32) -> DVec3,
) -> Vec<Option<VertexClass>> {
    let is_cut = |h: u32| cut.twin(h).is_none();
    (0..cut.vertex_count() as u32)
        .map(|v| {
            if !cut.boundary[v as usize] {
                return None;
            }
            let last = *cut.fan(v).last()?;
            Some(turn_at(cut, crosses, geometric, CutMesh::prev(last), &is_cut, label).class)
        })
        .collect()
}

/// The label of a boundary edge that is its own direction.
pub fn edge_label(cut: &CutMesh) -> impl Fn(u32) -> DVec3 + '_ {
    move |h: u32| cut.positions[cut.head(h) as usize] - cut.positions[cut.tail(h) as usize]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testmesh::{cube, grid};

    fn classes(p: &[DVec3], t: &[[u32; 3]], barrier: &[[u32; 2]]) -> (CutMesh, Vec<Option<VertexClass>>) {
        let cut = CutMesh::new(p, t, barrier);
        let crosses = FaceCrosses::new(&cut, &vec![DVec3::X; t.len()]);
        let classes = classify_boundary(&cut, &crosses, &[], &edge_label(&cut));
        (cut, classes)
    }

    #[test]
    fn square_border_has_four_convex_corners() {
        let (p, t) = grid(4, 4);
        let (cut, c) = classes(&p, &t, &[]);
        for (v, (&q, &class)) in cut.positions.iter().zip(&c).enumerate() {
            let corner = (q.x == 0.0 || q.x == 4.0) && (q.y == 0.0 || q.y == 4.0);
            let border = q.x == 0.0 || q.x == 4.0 || q.y == 0.0 || q.y == 4.0;
            let expected = if corner {
                Some(VertexClass::Convex)
            } else if border {
                Some(VertexClass::Flat)
            } else {
                None
            };
            assert_eq!(class, expected, "vertex {v} at {q}");
        }
    }

    #[test]
    fn l_shape_has_one_concave_vertex() {
        let (p, t) = grid(4, 4);
        // Drop the cells with i ≥ 2 and j ≥ 2.
        let t: Vec<[u32; 3]> = t
            .into_iter()
            .filter(|tri| !tri.iter().all(|&v| p[v as usize].x >= 2.0 && p[v as usize].y >= 2.0))
            .collect();
        let (cut, c) = classes(&p, &t, &[]);
        let concave: Vec<DVec3> = (0..cut.vertex_count())
            .filter(|&v| c[v] == Some(VertexClass::Concave))
            .map(|v| cut.positions[v])
            .collect();
        assert_eq!(concave, vec![DVec3::new(2.0, 2.0, 0.0)]);
        assert_eq!(c.iter().filter(|x| **x == Some(VertexClass::Convex)).count(), 5);
    }

    #[test]
    fn dangling_feature_end_is_narrow() {
        let (p, t) = grid(6, 6);
        let id = |i: u32, j: u32| j * 7 + i;
        let feature = [[id(1, 3), id(2, 3)], [id(2, 3), id(3, 3)]];
        let (cut, c) = classes(&p, &t, &feature);
        assert_eq!(c[id(1, 3) as usize], Some(VertexClass::Narrow));
        assert_eq!(c[id(3, 3) as usize], Some(VertexClass::Narrow));
        let middle = id(2, 3);
        assert_eq!(cut.twins[middle as usize].len(), 1);
        assert_eq!(c[middle as usize], Some(VertexClass::Flat));
        assert_eq!(c[cut.twins[middle as usize][0] as usize], Some(VertexClass::Flat));
    }

    #[test]
    fn three_feature_junction_classifies_each_sector() {
        let (p, t, features) = cube(2);
        let (cut, c) = classes(&p, &t, &features);
        let corner = p.iter().position(|q| *q == DVec3::ZERO).unwrap() as u32;
        let copies: Vec<u32> =
            std::iter::once(corner).chain(cut.twins[corner as usize].iter().copied()).collect();
        assert_eq!(copies.len(), 3);
        for v in copies {
            assert_eq!(c[v as usize], Some(VertexClass::Convex));
        }
        // A planar T: a feature row y = 3 and a feature column x = 3 above it.
        let (p, t) = grid(6, 6);
        let id = |i: u32, j: u32| j * 7 + i;
        let mut feature: Vec<[u32; 2]> = (0..6).map(|i| [id(i, 3), id(i + 1, 3)]).collect();
        feature.extend((3..6).map(|j| [id(3, j), id(3, j + 1)]));
        let (cut, c) = classes(&p, &t, &feature);
        let junction = id(3, 3);
        let mut found: Vec<VertexClass> = std::iter::once(junction)
            .chain(cut.twins[junction as usize].iter().copied())
            .map(|v| c[v as usize].unwrap())
            .collect();
        found.sort();
        assert_eq!(found, vec![VertexClass::Convex, VertexClass::Convex, VertexClass::Flat]);
    }
}
