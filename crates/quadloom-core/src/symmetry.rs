//! Symmetry: remesh one side of a mirror plane, then mirror the result.
//!
//! 1. **Planes.** Each chosen axis gives a plane `x_axis = offset` in object
//!    space. Explicit axes go through the origin, like Blender's Mirror
//!    modifier. Auto-detection tests each axis through the origin and through
//!    the bounding-box centre, by reflecting sample points and measuring their
//!    distance to the surface, and keeps the planes the mesh is symmetric about:
//!    no vertex's mirror image farther from the surface than the tolerance.
//! 2. **Cut.** The repaired mesh is clipped to the kept side of every plane.
//!    Cut vertices lie exactly on the plane, and the cut boundary becomes a
//!    feature line, so the cross field aligns to it and edge loops meet the
//!    centre line squarely.
//! 3. **Mirror.** The finished half is reflected across each plane in turn
//!    (with flipped winding), and vertices on the plane are welded.

use hashbrown::HashMap;

use glam::DVec3;
use rayon::prelude::*;

use crate::repair::RepairedMesh;
use crate::spatial::TriangleBvh;

/// Symmetry settings.
#[derive(Clone, Debug, PartialEq)]
pub struct SymmetryParams {
    /// Mirror across the X, Y and Z planes (object space, through the origin).
    pub axes: [bool; 3],
    /// Find the symmetry planes automatically (replaces `axes`).
    pub auto_detect: bool,
    /// Keep the negative side of each plane instead of the positive one.
    pub keep_negative: bool,
    /// Largest asymmetry auto-detection accepts, as a fraction of the bbox
    /// diagonal: the farthest any vertex's mirror image may lie from the
    /// surface.
    ///
    /// It used to bound the root mean square over sample points only, and
    /// one detail does not move a root mean square: of the 70 benchmark
    /// models 44 passed, and ten of those have something on one side only (a
    /// watch's crown, 3.4% of its size; a cow's tail, 1.9%; a bracket's lug,
    /// 6.7%). Mirrored, the kept side's detail appears twice or the other
    /// side's is gone. The root mean square is still taken first, being
    /// cheap, and every vertex is looked at only for a plane that passes it.
    pub tolerance: f64,
}

impl Default for SymmetryParams {
    fn default() -> Self {
        Self { axes: [false; 3], auto_detect: false, keep_negative: false, tolerance: 0.005 }
    }
}

impl SymmetryParams {
    /// Whether any symmetry is requested.
    pub fn enabled(&self) -> bool {
        self.auto_detect || self.axes.iter().any(|&a| a)
    }
}

/// A mirror plane: coordinate `axis` equals `offset`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plane {
    pub axis: usize,
    pub offset: f64,
}

/// The whole model a mesh stands for: itself, or the kept side of mirror
/// planes together with its copies.
///
/// Every size the pipeline takes from the model (the triangle budget over
/// its area, the tolerance over its diagonal) has to be the whole model's.
/// Taken from the kept side they came out as if half the model were all of
/// it: the same budget of triangles over half the area, so edges 0.7 times
/// as long and, mirrored, a result of nearly twice the quads that Density
/// asks for (a symmetric head: 19,000 quads with Symmetry on, 11,700 off).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Whole {
    /// How many copies of the mesh make the whole: 2 to the number of planes.
    pub copies: f64,
    /// The whole's bounding-box diagonal.
    pub diagonal: f64,
}

impl Whole {
    /// The whole that `positions` are the kept side of `planes` of (with no
    /// planes, the mesh itself).
    pub fn new(positions: &[DVec3], planes: &[Plane]) -> Self {
        if positions.is_empty() {
            return Self { copies: 1.0, diagonal: 0.0 };
        }
        let (mut lo, mut hi) = bounds(positions);
        for plane in planes {
            // The copy across a plane reaches as far past it as the mesh does.
            let reach = (hi[plane.axis] - plane.offset).abs().max((lo[plane.axis] - plane.offset).abs());
            lo[plane.axis] = plane.offset - reach;
            hi[plane.axis] = plane.offset + reach;
        }
        Self { copies: (1u32 << planes.len().min(3)) as f64, diagonal: (hi - lo).length() }
    }
}

/// A vertex this close to a mirror plane is on it: as a fraction of the
/// bounding-box diagonal, the distance cleanup welds vertices at
/// ([`crate::repair::RepairParams::weld_epsilon_rel`]'s default).
///
/// It was a billionth, which is nothing: a model that is not centred on its
/// origin gets its plane from the bounding box, and the box's centre and the
/// model's own centre-line vertices differ by the rounding of their 32-bit
/// coordinates, about a ten-millionth. The cut then went through every
/// centre-line triangle a hair from its edge and left slivers all along the
/// seam, which came out as quads of no size.
const ON_PLANE: f64 = 1e-6;

/// The distance from a plane within which a vertex of a mesh with these
/// bounds is taken to be on it: [`ON_PLANE`] of the diagonal, and never
/// less than a few steps of a 32-bit coordinate as far out as the mesh lies.
fn snap_distance(lo: DVec3, hi: DVec3) -> f64 {
    let far = lo.abs().max(hi.abs()).max_element();
    (ON_PLANE * (hi - lo).length()).max(4.0 * f64::from(f32::EPSILON) * far).max(1e-300)
}

/// What symmetry did.
#[derive(Clone, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct SymmetryReport {
    /// Axes mirrored across.
    pub axes: [bool; 3],
    /// Plane offset per axis (0 = through the origin).
    pub offsets: [f64; 3],
    /// Auto-detection's asymmetry per axis, as a fraction of the bbox
    /// diagonal (negative when not measured).
    pub asymmetry: [f64; 3],
    /// The farthest any vertex's mirror image lies from the surface, per
    /// axis and as a fraction of the bbox diagonal (negative when not
    /// measured): a model can be symmetric on the whole and carry a detail
    /// on one side only, which the mirror would lose or double. Over every
    /// vertex for a plane that passed the first test or was chosen by hand,
    /// over the samples otherwise.
    pub worst: [f64; 3],
    /// Edges of the cut that became the seam.
    pub seam_edges: usize,
    /// Why symmetry was skipped, if it was.
    pub skipped: Option<String>,
    /// Share of the surface area on the kept side (1 when skipped).
    pub kept_share: f64,
    /// Whether the mesh reaches every plane (a seam exists): if not, the
    /// mirrored copy is a separate piece.
    pub touches_planes: bool,
}

/// Samples per auto-detection test.
const DETECT_SAMPLES: usize = 4000;

/// The planes `params` asks for, with auto-detection's measurements.
pub fn planes(
    positions: &[DVec3],
    triangles: &[[u32; 3]],
    params: &SymmetryParams,
) -> (Vec<Plane>, SymmetryReport) {
    let mut report = SymmetryReport {
        asymmetry: [-1.0; 3],
        worst: [-1.0; 3],
        kept_share: 1.0,
        ..SymmetryReport::default()
    };
    let (lo, hi) = bounds(positions);
    let diagonal = (hi - lo).length();
    if diagonal <= 0.0 || triangles.is_empty() {
        report.skipped = Some("the mesh is empty".into());
        return (Vec::new(), report);
    }
    let surface = TriangleBvh::new(positions, triangles);
    let step = (positions.len() / DETECT_SAMPLES).max(1);
    if !params.auto_detect {
        let planes: Vec<Plane> =
            (0..3).filter(|&a| params.axes[a]).map(|axis| Plane { axis, offset: 0.0 }).collect();
        for p in &planes {
            report.axes[p.axis] = true;
            // Measured even when chosen by hand, so a mismatch can be warned about.
            let (error, _) = asymmetry(positions, &surface, p.axis, 0.0, step);
            report.asymmetry[p.axis] = error / diagonal;
            report.worst[p.axis] = largest_difference(positions, &surface, p.axis, 0.0) / diagonal;
        }
        return (planes, report);
    }
    let center = 0.5 * (lo + hi);
    let mut planes = Vec::new();
    let mut one_sided: Option<(usize, f64)> = None;
    for axis in 0..3 {
        let mut best: Option<(f64, f64, f64)> = None;
        for offset in [0.0, center[axis]] {
            let (error, worst) = asymmetry(positions, &surface, axis, offset, step);
            if best.is_none_or(|(e, _, _)| error / diagonal < e) {
                best = Some((error / diagonal, offset, worst / diagonal));
            }
        }
        if let Some((error, offset, worst)) = best {
            report.asymmetry[axis] = error;
            report.worst[axis] = worst;
            if error <= params.tolerance {
                // Symmetric on the whole: now every vertex, for a detail on one side.
                let worst = largest_difference(positions, &surface, axis, offset) / diagonal;
                report.worst[axis] = worst;
                if worst <= params.tolerance {
                    planes.push(Plane { axis, offset });
                    report.axes[axis] = true;
                    report.offsets[axis] = offset;
                } else if one_sided.is_none_or(|(_, w)| worst < w) {
                    one_sided = Some((axis, worst));
                }
            }
        }
    }
    if planes.is_empty() {
        report.skipped = Some(match one_sided {
            Some((axis, worst)) => format!(
                "no symmetry plane found: across {} the sides differ by {:.1}% of the size in one place",
                ["X", "Y", "Z"][axis],
                100.0 * worst
            ),
            None => "no symmetry plane found".into(),
        });
    }
    (planes, report)
}

/// The farthest any vertex's mirror image lies from the surface.
fn largest_difference(positions: &[DVec3], surface: &TriangleBvh, axis: usize, offset: f64) -> f64 {
    positions
        .par_iter()
        .map(|p| {
            let mut q = *p;
            q[axis] = 2.0 * offset - q[axis];
            surface.closest_point(q).map_or(0.0, |(_, d)| d)
        })
        .reduce(|| 0.0, f64::max)
}

/// The distance from reflected sample vertices to the surface: its root
/// mean square, and the largest.
fn asymmetry(
    positions: &[DVec3],
    surface: &TriangleBvh,
    axis: usize,
    offset: f64,
    step: usize,
) -> (f64, f64) {
    let (mut sum, mut worst, mut count) = (0.0, 0.0f64, 0usize);
    for p in positions.iter().step_by(step) {
        let mut q = *p;
        q[axis] = 2.0 * offset - q[axis];
        if let Some((_, d)) = surface.closest_point(q) {
            sum += d * d;
            worst = worst.max(d);
            count += 1;
        }
    }
    if count == 0 { (f64::INFINITY, f64::INFINITY) } else { ((sum / count as f64).sqrt(), worst) }
}

fn bounds(positions: &[DVec3]) -> (DVec3, DVec3) {
    positions.iter().fold((DVec3::INFINITY, DVec3::NEG_INFINITY), |(lo, hi), &p| (lo.min(p), hi.max(p)))
}

/// Share of the surface area of `mesh` on the kept side of every plane, and
/// whether the mesh crosses or touches each plane.
pub fn kept_share(mesh: &RepairedMesh, planes: &[Plane], keep_negative: bool) -> (f64, bool) {
    let sign = if keep_negative { -1.0 } else { 1.0 };
    let (mut kept, mut total) = (0.0, 0.0);
    let (lo, hi) = bounds(&mesh.positions);
    let snap = snap_distance(lo, hi);
    let mut touches = vec![false; planes.len()];
    for t in &mesh.triangles {
        let [a, b, c] = t.map(|v| mesh.positions[v as usize]);
        let area = 0.5 * (b - a).cross(c - a).length();
        let centroid = (a + b + c) / 3.0;
        total += area;
        if planes.iter().all(|pl| sign * (centroid[pl.axis] - pl.offset) >= 0.0) {
            kept += area;
        }
        for (i, pl) in planes.iter().enumerate() {
            let d = [a, b, c].map(|p| p[pl.axis] - pl.offset);
            if d.iter().any(|&x| x.abs() <= snap)
                || (d.iter().any(|&x| x > 0.0) && d.iter().any(|&x| x < 0.0))
            {
                touches[i] = true;
            }
        }
    }
    (if total > 0.0 { kept / total } else { 0.0 }, touches.iter().all(|&t| t))
}

/// Clips `mesh` to the kept side of every plane. Returns the cut mesh and the
/// seam edges (sorted vertex pairs of the cut boundary, on a plane).
/// `None` if nothing is left on the kept side.
pub fn cut(
    mesh: &RepairedMesh,
    planes: &[Plane],
    keep_negative: bool,
) -> Option<(RepairedMesh, Vec<[u32; 2]>)> {
    let (lo, hi) = bounds(&mesh.positions);
    let snap = snap_distance(lo, hi);
    let sign = if keep_negative { -1.0 } else { 1.0 };
    let mut positions = mesh.positions.clone();
    let mut triangles: Vec<[u32; 3]> = mesh.triangles.clone();
    let mut origin: Vec<u32> = mesh.face_origin.clone();
    for plane in planes {
        let distance = |p: DVec3| sign * (p[plane.axis] - plane.offset);
        // Snap near-plane vertices onto it, so slivers aren't created.
        for p in &mut positions {
            if distance(*p).abs() <= snap {
                p[plane.axis] = plane.offset;
            }
        }
        let mut crossing: HashMap<(u32, u32), u32> = HashMap::new();
        let (mut next_tris, mut next_origin) = (Vec::new(), Vec::new());
        for (t, &tri) in triangles.iter().enumerate() {
            let d = tri.map(|v| distance(positions[v as usize]));
            if d.iter().all(|&x| x >= 0.0) && d.iter().any(|&x| x > 0.0) {
                next_tris.push(tri);
                next_origin.push(origin[t]);
                continue;
            }
            if d.iter().all(|&x| x <= 0.0) {
                continue; // dropped (faces lying in the plane too: the mirror recreates them)
            }
            // The kept part: corners on the kept side plus the crossing points, in order.
            let mut poly: Vec<u32> = Vec::with_capacity(4);
            for i in 0..3 {
                let (a, b) = (tri[i], tri[(i + 1) % 3]);
                let (da, db) = (d[i], d[(i + 1) % 3]);
                if da >= 0.0 {
                    poly.push(a);
                }
                if (da > 0.0 && db < 0.0) || (da < 0.0 && db > 0.0) {
                    let key = (a.min(b), a.max(b));
                    let id = *crossing.entry(key).or_insert_with(|| {
                        let (pa, pb) = (positions[a as usize], positions[b as usize]);
                        let mut q = pa.lerp(pb, da / (da - db));
                        q[plane.axis] = plane.offset;
                        positions.push(q);
                        (positions.len() - 1) as u32
                    });
                    poly.push(id);
                }
            }
            for k in 1..poly.len().saturating_sub(1) {
                next_tris.push([poly[0], poly[k], poly[k + 1]]);
                next_origin.push(origin[t]);
            }
        }
        triangles = next_tris;
        origin = next_origin;
    }
    if triangles.is_empty() {
        return None;
    }
    // Compact: keep used vertices only, carrying the input-vertex maps over.
    let mut remap = vec![u32::MAX; positions.len()];
    let mut kept = Vec::new();
    for t in &mut triangles {
        for v in t.iter_mut() {
            if remap[*v as usize] == u32::MAX {
                remap[*v as usize] = kept.len() as u32;
                kept.push(positions[*v as usize]);
            }
            *v = remap[*v as usize];
        }
    }
    let carry = |v: u32| remap.get(v as usize).copied().filter(|&r| r != u32::MAX);
    let vertex_map = mesh.vertex_map.iter().map(|m| m.and_then(carry)).collect();
    let vertex_copies =
        mesh.vertex_copies.iter().map(|c| c.iter().filter_map(|&v| carry(v)).collect()).collect();
    // Seam: boundary edges with both ends on a plane.
    let on_plane = |p: DVec3| planes.iter().any(|pl| (p[pl.axis] - pl.offset).abs() <= snap);
    let mut uses: HashMap<(u32, u32), u32> = HashMap::new();
    for t in &triangles {
        for i in 0..3 {
            let (a, b) = (t[i], t[(i + 1) % 3]);
            *uses.entry((a.min(b), a.max(b))).or_default() += 1;
        }
    }
    let mut seam: Vec<[u32; 2]> = uses
        .iter()
        .filter(|&(&(a, b), &n)| n == 1 && on_plane(kept[a as usize]) && on_plane(kept[b as usize]))
        .map(|(&(a, b), _)| [a, b])
        .collect();
    seam.sort_unstable();
    let cut = RepairedMesh { positions: kept, triangles, vertex_map, vertex_copies, face_origin: origin };
    Some((cut, seam))
}

/// A polygon mesh with per-face sources and feature edges, as mirrored.
#[derive(Clone, Debug, Default)]
pub struct Mirrored {
    pub positions: Vec<DVec3>,
    pub offsets: Vec<u32>,
    pub indices: Vec<u32>,
    /// Per output face, the input face it copies.
    pub source: Vec<u32>,
    /// Feature edges, with their mirror images.
    pub edges: Vec<[u32; 2]>,
}

/// Mirrors a polygon mesh across each plane in turn and welds the vertices
/// lying on the plane (snapping them onto it).
pub fn mirror(
    positions: &[DVec3],
    offsets: &[u32],
    indices: &[u32],
    edges: &[[u32; 2]],
    planes: &[Plane],
) -> Mirrored {
    let (lo, hi) = bounds(positions);
    let weld = 1e-6 * (hi - lo).length().max(1e-300);
    let mut out = Mirrored {
        positions: positions.to_vec(),
        offsets: offsets.to_vec(),
        indices: indices.to_vec(),
        source: (0..offsets.len().saturating_sub(1) as u32).collect(),
        edges: edges.to_vec(),
    };
    for plane in planes {
        let n = out.positions.len();
        let mut image = vec![0u32; n];
        for (v, slot) in image.iter_mut().enumerate() {
            let p = out.positions[v];
            if (p[plane.axis] - plane.offset).abs() <= weld {
                out.positions[v][plane.axis] = plane.offset;
                *slot = v as u32;
            } else {
                let mut q = p;
                q[plane.axis] = 2.0 * plane.offset - q[plane.axis];
                out.positions.push(q);
                *slot = (out.positions.len() - 1) as u32;
            }
        }
        let faces = out.offsets.len().saturating_sub(1);
        for f in 0..faces {
            let (start, end) = (out.offsets[f] as usize, out.offsets[f + 1] as usize);
            // Reversed winding keeps the mirrored half facing outward.
            let mirrored: Vec<u32> =
                out.indices[start..end].iter().rev().map(|&v| image[v as usize]).collect();
            out.indices.extend_from_slice(&mirrored);
            out.offsets.push(out.indices.len() as u32);
            out.source.push(out.source[f]);
        }
        let images: Vec<[u32; 2]> = out
            .edges
            .iter()
            .map(|&[a, b]| {
                let (a, b) = (image[a as usize], image[b as usize]);
                [a.min(b), a.max(b)]
            })
            .collect();
        out.edges.extend(images);
        out.edges.sort_unstable();
        out.edges.dedup();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::Control;
    use crate::features::FeatureInput;
    use crate::mesh::InputMesh;
    use crate::params::Params;
    use crate::repair::{RepairParams, repair};
    use crate::testmesh::{cube, torus};

    fn repaired(p: &[DVec3], t: &[[u32; 3]]) -> RepairedMesh {
        let flat: Vec<f32> = p.iter().flat_map(|v| [v.x as f32, v.y as f32, v.z as f32]).collect();
        let idx: Vec<u32> = t.iter().flatten().copied().collect();
        repair(&InputMesh::from_buffers(&flat, &idx).unwrap(), &RepairParams::default(), &Control::default())
            .unwrap()
            .0
    }

    #[test]
    fn cut_keeps_one_side_with_a_seam_on_the_plane() {
        // A cube centred on the origin.
        let (p, t, _) = cube(4);
        let centred: Vec<DVec3> = p.iter().map(|&v| v - DVec3::splat(0.5)).collect();
        let mesh = repaired(&centred, &t);
        let plane = Plane { axis: 0, offset: 0.0 };
        let (half, seam) = cut(&mesh, &[plane], false).unwrap();
        assert!(half.positions.iter().all(|p| p.x >= 0.0));
        assert!(!seam.is_empty());
        assert!(seam.iter().flatten().all(|&v| half.positions[v as usize].x == 0.0));
        // The seam is a closed square of side 1: total length 4.
        let length: f64 = seam
            .iter()
            .map(|&[a, b]| (half.positions[a as usize] - half.positions[b as usize]).length())
            .sum();
        assert!((length - 4.0).abs() < 1e-9, "seam length {length}");
        let (neg, _) = cut(&mesh, &[plane], true).unwrap();
        assert!(neg.positions.iter().all(|p| p.x <= 0.0));
    }

    #[test]
    fn auto_detection_finds_the_planes() {
        let (p, t) = torus(48, 24);
        let auto = SymmetryParams { auto_detect: true, ..SymmetryParams::default() };
        let (planes, report) = super::planes(&p, &t, &auto);
        assert_eq!(report.axes, [true, true, true], "{report:?}");
        assert_eq!(planes.len(), 3);
        // Off-centre: the planes go through the bounding-box centre.
        let shifted: Vec<DVec3> = p.iter().map(|&v| v + DVec3::new(3.0, 0.0, 0.0)).collect();
        let (planes, _) = super::planes(&shifted, &t, &auto);
        let x = planes.iter().find(|pl| pl.axis == 0).unwrap();
        assert!((x.offset - 3.0).abs() < 1e-9, "offset {}", x.offset);
        // Squash one side: no longer symmetric across X.
        let lopsided: Vec<DVec3> =
            p.iter().map(|&v| if v.x > 0.0 { v * DVec3::new(1.4, 1.0, 1.0) } else { v }).collect();
        let (_, report) = super::planes(&lopsided, &t, &auto);
        assert!(!report.axes[0], "{report:?}");
        assert!(report.axes[1]);
    }

    #[test]
    fn a_detail_on_one_side_shows_in_the_largest_difference() {
        let (p, t) = torus(48, 24);
        let auto = SymmetryParams { auto_detect: true, ..SymmetryParams::default() };
        let (_, even) = super::planes(&p, &t, &auto);
        assert!(even.worst[0] < 1e-9 && even.asymmetry[0] < 1e-9, "{even:?}");
        // A bump on the outer rim at +X, three vertices of 1,152: the model
        // is still symmetric on the whole and the plane is still found, but
        // one sample's mirror image lies far from the surface.
        let tip = DVec3::new(1.4, 0.0, 0.0);
        let bumped: Vec<DVec3> = p
            .iter()
            .map(|&v| if (v - tip).length() < 0.12 { v + DVec3::new(0.15, 0.0, 0.0) } else { v })
            .collect();
        assert_eq!(bumped.iter().zip(&p).filter(|(a, b)| a != b).count(), 3);
        let (planes, report) = super::planes(&bumped, &t, &auto);
        assert!(report.asymmetry[0] < 0.005, "symmetric on the whole: {report:?}");
        assert!(report.worst[0] > 0.02 && report.worst[0] > 5.0 * report.asymmetry[0], "{report:?}");
        // So X is not taken (the bump would be doubled, or lost with the
        // other side kept); Y and Z, which the bump lies on, still are.
        assert_eq!(report.axes, [false, true, true], "{report:?}");
        assert!(planes.iter().all(|pl| pl.axis != 0) && report.skipped.is_none());
        // Chosen by hand, the plane is used, and measured the same way.
        let by_hand = SymmetryParams { axes: [true, false, false], ..SymmetryParams::default() };
        let (chosen_planes, chosen) = super::planes(&bumped, &t, &by_hand);
        assert_eq!(chosen_planes, [Plane { axis: 0, offset: 0.0 }]);
        assert_eq!((chosen.asymmetry[0], chosen.worst[0]), (report.asymmetry[0], report.worst[0]));
        assert_eq!(chosen.worst[1], -1.0, "not measured");
        // With nothing else to find, the report says what stood in the way.
        // (A bump off both other planes: the same three vertices, moved in y as well.)
        let off: Vec<DVec3> = bumped
            .iter()
            .zip(&p)
            .map(|(&b, &v)| if b != v { b + DVec3::new(0.0, 0.1, 0.1) } else { b })
            .collect();
        let (none, report) = super::planes(&off, &t, &auto);
        let why = report.skipped.clone().unwrap_or_default();
        assert!(none.is_empty() && why.contains("differ by") && why.contains("in one place"), "{report:?}");
        // A looser tolerance lets a larger difference through.
        let loose = SymmetryParams { auto_detect: true, tolerance: 0.05, ..SymmetryParams::default() };
        assert_eq!(super::planes(&bumped, &t, &loose).1.axes, [true, true, true]);
    }

    #[test]
    fn the_whole_is_the_kept_side_and_its_copies() {
        // One side of a box 2 units each way: x from 0 to 1.
        let half = [DVec3::new(0.0, -1.0, -1.0), DVec3::new(1.0, 1.0, 1.0)];
        let alone = Whole::new(&half, &[]);
        assert_eq!((alone.copies, alone.diagonal), (1.0, 3.0));
        let x = Plane { axis: 0, offset: 0.0 };
        let mirrored = Whole::new(&half, &[x]);
        assert_eq!(mirrored.copies, 2.0);
        assert!((mirrored.diagonal - 12f64.sqrt()).abs() < 1e-12, "{mirrored:?}");
        // A plane off the origin: the copy reaches as far past it.
        let shifted: Vec<DVec3> = half.iter().map(|&p| p + DVec3::new(5.0, 0.0, 0.0)).collect();
        assert_eq!(Whole::new(&shifted, &[Plane { axis: 0, offset: 5.0 }]), mirrored);
        // Two planes: a quarter, four copies, the same box.
        let quarter = [DVec3::new(0.0, 0.0, -1.0), DVec3::new(1.0, 1.0, 1.0)];
        let both = Whole::new(&quarter, &[x, Plane { axis: 1, offset: 0.0 }]);
        assert_eq!(both.copies, 4.0);
        assert!((both.diagonal - 12f64.sqrt()).abs() < 1e-12, "{both:?}");
        assert_eq!(Whole::new(&[], &[x]), Whole { copies: 1.0, diagonal: 0.0 });
    }

    #[test]
    fn a_centre_line_a_rounding_error_off_the_plane_leaves_no_slivers() {
        // A cube whose middle row of vertices sits two ten-millionths of its
        // size past the plane, as the 32-bit coordinates of a model away
        // from its origin do against its bounding box's centre.
        let (p, t, _) = cube(4);
        let moved: Vec<DVec3> =
            p.iter().map(|&v| if v.x == 0.5 { v + DVec3::new(2e-7, 0.0, 0.0) } else { v }).collect();
        assert_eq!(moved.iter().filter(|v| v.x > 0.5 && v.x < 0.6).count(), 16);
        let mesh = repaired(&moved, &t);
        let (half, seam) = cut(&mesh, &[Plane { axis: 0, offset: 0.5 }], false).unwrap();
        // The row is on the plane: nothing was cut, so half the triangles
        // are kept as they were (each 1/32 of area), and the seam runs
        // through the row's sixteen vertices.
        assert_eq!(half.triangles.len(), t.len() / 2);
        for tri in &half.triangles {
            let [a, b, c] = tri.map(|v| half.positions[v as usize]);
            let area = 0.5 * (b - a).cross(c - a).length();
            assert!(area > 0.03, "a sliver of area {area}");
        }
        assert_eq!(seam.len(), 16);
        assert!(seam.iter().flatten().all(|&v| half.positions[v as usize].x == 0.5));
    }

    #[test]
    fn the_snap_distance_follows_the_size_of_the_coordinates() {
        // Round the origin: a millionth of the diagonal.
        let near = snap_distance(DVec3::splat(-1.0), DVec3::splat(1.0));
        assert!((near - 1e-6 * 12f64.sqrt()).abs() < 1e-15, "{near}");
        // A unit-sized mesh a thousand units out: its 32-bit coordinates
        // step by more than that, and a few steps is the least it can be.
        let far = snap_distance(DVec3::new(1000.0, 0.0, 0.0), DVec3::new(1001.0, 1.0, 1.0));
        assert!((far - 4.0 * f64::from(f32::EPSILON) * 1001.0).abs() < 1e-12, "{far}");
        assert!(far > 100.0 * 1e-6 * 3f64.sqrt());
        // No size at all: still more than nothing.
        assert!(snap_distance(DVec3::ZERO, DVec3::ZERO) > 0.0);
    }

    #[test]
    fn symmetric_remesh_is_closed_quads_and_mirror_exact() {
        let (p, t) = torus(48, 24);
        let flat: Vec<f32> = p.iter().flat_map(|v| [v.x as f32, v.y as f32, v.z as f32]).collect();
        let idx: Vec<u32> = t.iter().flatten().copied().collect();
        let mut params = Params::default();
        params.symmetry.axes = [true, false, false];
        let out = crate::pipeline::run(&flat, &idx, &FeatureInput::default(), &params, &Control::default())
            .unwrap();
        let report = out.symmetry.unwrap();
        assert_eq!(report.axes, [true, false, false]);
        assert!(report.seam_edges > 0);
        let q = out.quads.unwrap();
        let mut uses: HashMap<(u32, u32), u32> = HashMap::new();
        for w in q.mesh.face_offsets.windows(2) {
            let face = &q.mesh.face_indices[w[0] as usize..w[1] as usize];
            assert_eq!(face.len(), 4);
            for i in 0..4 {
                let (a, b) = (face[i], face[(i + 1) % 4]);
                *uses.entry((a.min(b), a.max(b))).or_default() += 1;
            }
        }
        assert!(uses.values().all(|&n| n == 2), "closed and manifold");
        // Every vertex has its mirror image among the vertices.
        let key =
            |v: DVec3| ((v.x * 1e6).round() as i64, (v.y * 1e6).round() as i64, (v.z * 1e6).round() as i64);
        let all: hashbrown::HashSet<_> = q.mesh.positions.iter().map(|&v| key(v)).collect();
        for &v in &q.mesh.positions {
            assert!(all.contains(&key(DVec3::new(-v.x, v.y, v.z))), "no mirror for {v}");
        }
    }

    #[test]
    fn two_planes_give_quarter_symmetry() {
        let (p, t) = torus(48, 24);
        let flat: Vec<f32> = p.iter().flat_map(|v| [v.x as f32, v.y as f32, v.z as f32]).collect();
        let idx: Vec<u32> = t.iter().flatten().copied().collect();
        let mut params = Params::default();
        params.symmetry.axes = [true, true, false];
        let out = crate::pipeline::run(&flat, &idx, &FeatureInput::default(), &params, &Control::default())
            .unwrap();
        let q = out.quads.unwrap();
        let mut uses: HashMap<(u32, u32), u32> = HashMap::new();
        for w in q.mesh.face_offsets.windows(2) {
            let face = &q.mesh.face_indices[w[0] as usize..w[1] as usize];
            for i in 0..face.len() {
                let (a, b) = (face[i], face[(i + 1) % face.len()]);
                *uses.entry((a.min(b), a.max(b))).or_default() += 1;
            }
        }
        assert!(uses.values().all(|&n| n == 2), "closed and manifold");
        let key =
            |v: DVec3| ((v.x * 1e6).round() as i64, (v.y * 1e6).round() as i64, (v.z * 1e6).round() as i64);
        let all: hashbrown::HashSet<_> = q.mesh.positions.iter().map(|&v| key(v)).collect();
        for &v in &q.mesh.positions {
            for m in [DVec3::new(-v.x, v.y, v.z), DVec3::new(v.x, -v.y, v.z), DVec3::new(-v.x, -v.y, v.z)] {
                assert!(all.contains(&key(m)), "no image {m} of {v}");
            }
        }
    }

    fn run_with(
        p: &[DVec3],
        t: &[[u32; 3]],
        axes: [bool; 3],
        keep_negative: bool,
    ) -> crate::pipeline::RunOutput {
        let flat: Vec<f32> = p.iter().flat_map(|v| [v.x as f32, v.y as f32, v.z as f32]).collect();
        let idx: Vec<u32> = t.iter().flatten().copied().collect();
        let mut params = Params::default();
        params.symmetry.axes = axes;
        params.symmetry.keep_negative = keep_negative;
        crate::pipeline::run(&flat, &idx, &FeatureInput::default(), &params, &Control::default()).unwrap()
    }

    fn warned(out: &crate::pipeline::RunOutput, needle: &str) -> bool {
        out.warnings.iter().any(|w| w.message.contains(needle))
    }

    #[test]
    fn mismatched_symmetry_is_warned_about() {
        let (p, t) = torus(48, 24);
        // Lopsided across X: still mirrored as asked, with a warning.
        let lopsided: Vec<DVec3> =
            p.iter().map(|&v| if v.x > 0.0 { v * DVec3::new(1.5, 1.0, 1.0) } else { v }).collect();
        let out = run_with(&lopsided, &t, [true, false, false], false);
        assert!(warned(&out, "isn't symmetric across X"), "{:?}", out.warnings);
        // A symmetric torus: no warning.
        let out = run_with(&p, &t, [true, false, false], false);
        assert!(out.warnings.is_empty(), "{:?}", out.warnings);
        // Mostly on the other side.
        let shifted: Vec<DVec3> = p.iter().map(|&v| v - DVec3::new(0.9, 0.0, 0.0)).collect();
        let out = run_with(&shifted, &t, [true, false, false], false);
        assert!(warned(&out, "of the surface is on the kept side"), "{:?}", out.warnings);
    }

    #[test]
    fn a_mesh_away_from_the_plane_is_warned_about() {
        let (p, t) = torus(24, 16);
        // Entirely on the kept side, not touching x = 0: the mirror is a separate piece.
        let away: Vec<DVec3> = p.iter().map(|&v| v + DVec3::new(3.0, 0.0, 0.0)).collect();
        let out = run_with(&away, &t, [true, false, false], false);
        assert!(warned(&out, "doesn't reach the mirror plane"), "{:?}", out.warnings);
        // Entirely on the other side: skipped, with a hint.
        let out = run_with(&away, &t, [true, false, false], true);
        assert!(warned(&out, "symmetry skipped"), "{:?}", out.warnings);
        assert!(out.warnings.iter().any(|w| w.hint.is_some()));
    }

    #[test]
    fn nothing_on_the_kept_side_skips_symmetry() {
        let (p, t) = torus(24, 16);
        let shifted: Vec<f32> = p.iter().flat_map(|v| [(v.x - 5.0) as f32, v.y as f32, v.z as f32]).collect();
        let idx: Vec<u32> = t.iter().flatten().copied().collect();
        let mut params = Params::default();
        params.symmetry.axes = [true, false, false];
        let out =
            crate::pipeline::run(&shifted, &idx, &FeatureInput::default(), &params, &Control::default())
                .unwrap();
        let report = out.symmetry.unwrap();
        assert!(report.skipped.is_some());
        assert!(out.quads.is_some(), "the remesh still runs without symmetry");
    }
}
