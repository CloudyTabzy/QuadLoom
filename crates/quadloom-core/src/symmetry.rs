//! Symmetry: remesh one side of a mirror plane, then mirror the result.
//!
//! 1. **Planes.** Each chosen axis gives a plane `x_axis = offset` in object
//!    space. Explicit axes go through the origin, like Blender's Mirror
//!    modifier. Auto-detection tests each axis through the origin and through
//!    the bounding-box centre, by reflecting sample points and measuring their
//!    distance to the surface, and keeps the planes the mesh is symmetric about.
//! 2. **Cut.** The repaired mesh is clipped to the kept side of every plane.
//!    Cut vertices lie exactly on the plane, and the cut boundary becomes a
//!    feature line, so the cross field aligns to it and edge loops meet the
//!    centre line squarely.
//! 3. **Mirror.** The finished half is reflected across each plane in turn
//!    (with flipped winding), and vertices on the plane are welded.

use std::collections::HashMap;

use glam::DVec3;

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
    /// Largest asymmetry auto-detection accepts, as a fraction of the bbox diagonal.
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
    /// Edges of the cut that became the seam.
    pub seam_edges: usize,
    /// Why symmetry was skipped, if it was.
    pub skipped: Option<String>,
}

/// Samples per auto-detection test.
const DETECT_SAMPLES: usize = 4000;

/// The planes `params` asks for, with auto-detection's measurements.
pub fn planes(
    positions: &[DVec3],
    triangles: &[[u32; 3]],
    params: &SymmetryParams,
) -> (Vec<Plane>, SymmetryReport) {
    let mut report = SymmetryReport { asymmetry: [-1.0; 3], ..SymmetryReport::default() };
    if !params.auto_detect {
        let planes: Vec<Plane> =
            (0..3).filter(|&a| params.axes[a]).map(|axis| Plane { axis, offset: 0.0 }).collect();
        for p in &planes {
            report.axes[p.axis] = true;
        }
        return (planes, report);
    }
    let (lo, hi) = bounds(positions);
    let diagonal = (hi - lo).length();
    if diagonal <= 0.0 || triangles.is_empty() {
        report.skipped = Some("the mesh is empty".into());
        return (Vec::new(), report);
    }
    let surface = TriangleBvh::new(positions, triangles);
    let step = (positions.len() / DETECT_SAMPLES).max(1);
    let center = 0.5 * (lo + hi);
    let mut planes = Vec::new();
    for axis in 0..3 {
        let mut best: Option<(f64, f64)> = None;
        for offset in [0.0, center[axis]] {
            let error = asymmetry(positions, &surface, axis, offset, step) / diagonal;
            if best.is_none_or(|(e, _)| error < e) {
                best = Some((error, offset));
            }
        }
        if let Some((error, offset)) = best {
            report.asymmetry[axis] = error;
            if error <= params.tolerance {
                planes.push(Plane { axis, offset });
                report.axes[axis] = true;
                report.offsets[axis] = offset;
            }
        }
    }
    if planes.is_empty() {
        report.skipped = Some("no symmetry plane found".into());
    }
    (planes, report)
}

/// Root-mean-square distance from reflected sample vertices to the surface.
fn asymmetry(positions: &[DVec3], surface: &TriangleBvh, axis: usize, offset: f64, step: usize) -> f64 {
    let (mut sum, mut count) = (0.0, 0usize);
    for p in positions.iter().step_by(step) {
        let mut q = *p;
        q[axis] = 2.0 * offset - q[axis];
        if let Some((_, d)) = surface.closest_point(q) {
            sum += d * d;
            count += 1;
        }
    }
    if count == 0 { f64::INFINITY } else { (sum / count as f64).sqrt() }
}

fn bounds(positions: &[DVec3]) -> (DVec3, DVec3) {
    positions.iter().fold((DVec3::INFINITY, DVec3::NEG_INFINITY), |(lo, hi), &p| (lo.min(p), hi.max(p)))
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
    let snap = 1e-9 * (hi - lo).length().max(1e-300);
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
        let all: std::collections::HashSet<_> = q.mesh.positions.iter().map(|&v| key(v)).collect();
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
        let all: std::collections::HashSet<_> = q.mesh.positions.iter().map(|&v| key(v)).collect();
        for &v in &q.mesh.positions {
            for m in [DVec3::new(-v.x, v.y, v.z), DVec3::new(v.x, -v.y, v.z), DVec3::new(-v.x, -v.y, v.z)] {
                assert!(all.contains(&key(m)), "no image {m} of {v}");
            }
        }
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
