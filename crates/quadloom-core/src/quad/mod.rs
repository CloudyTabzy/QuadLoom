//! Stages 5–6: quantization and quad patterns. From the patch layout to an
//! all-quad mesh.
//!
//! 1. Sides are cut into segments at every layout corner (`segments`).
//! 2. Each segment gets an integer quad count (`quantize`), close to its
//!    length over the quad size (measured in the frame's metric when Quad
//!    Shape ran), with every patch's boundary even and as many patches as
//!    possible regular.
//! 3. Each patch is filled with a quad pattern in its polygon domain
//!    (`pattern`) and the pattern is placed on the surface through a map of
//!    the patch's triangles onto that polygon (`embed`).
//! 4. Patches are stitched: corners are mesh vertices, and boundary samples
//!    are owned by their segment, so neighbours share them exactly.
//!
//! A patch that can't be filled (not a disk, too few or too many sides, or a
//! failed map) keeps its triangles. Segments bordering it are sampled at their
//! own mesh vertices, so the result stays watertight.

pub mod embed;
pub mod ilp;
pub mod pattern;
pub mod quantize;
pub mod segments;
pub mod smooth;
#[cfg(test)]
mod tests;

use std::collections::HashMap;

use glam::{DVec2, DVec3};

use crate::control::{Control, Stage};
use crate::error::Error;
use crate::mesh::PolyMesh;
use crate::trace::TraceOutput;
use embed::PatchMap;
use pattern::{PatternKind, Point};
use segments::Segments;

/// Settings for the quad stages.
#[derive(Clone, Debug, PartialEq)]
pub struct QuadParams {
    /// Build the quad mesh (needs the patch layout).
    pub enabled: bool,
    /// Quad edge length as a multiple of the prep remesh's edge length.
    pub edge_scale: f64,
    /// How strongly quantization prefers regular patterns over matching the
    /// target lengths (see [`quantize::DEFAULT_REGULARITY`]).
    pub regularity: f64,
    /// Rounds of smoothing on the surface (0 = none).
    pub smoothing: usize,
    /// How much quad size follows the prep remesh's local detail: 0 keeps
    /// every quad the same size, 1 follows it fully (geometric blend between).
    pub adaptivity: f64,
}

impl Default for QuadParams {
    fn default() -> Self {
        Self {
            enabled: true,
            edge_scale: 0.75,
            regularity: quantize::DEFAULT_REGULARITY,
            smoothing: 20,
            adaptivity: 0.5,
        }
    }
}

/// Totals of the quad stages.
#[derive(Clone, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct QuadReport {
    pub quads: usize,
    /// Triangles of patches that kept them.
    pub triangles: usize,
    pub segments: usize,
    /// Patches filled with a regular grid / the one-singularity closed form /
    /// a diagonal split / the fan fallback.
    pub grid_patches: usize,
    pub center_patches: usize,
    pub split_patches: usize,
    pub fan_patches: usize,
    /// Patches left as triangles.
    pub kept_patches: usize,
    /// Quad edge length used.
    pub edge_length: f64,
}

/// The quad mesh.
#[derive(Clone, Debug, Default)]
pub struct QuadOutput {
    pub mesh: PolyMesh,
    /// The layout patch of each output face.
    pub patch_of_face: Vec<u32>,
    /// Edges of the quad mesh along feature lines and borders, as vertex pairs.
    pub feature_edges: Vec<[u32; 2]>,
    pub report: QuadReport,
}

/// Per-face inputs from the mesh the layout was traced on (indexed through
/// `trace.source_face`).
#[derive(Clone, Copy, Debug, Default)]
pub struct Sizing<'a> {
    /// Quad Shape's `[along, across]` scales.
    pub scales: Option<&'a [[f64; 2]]>,
    /// Local quad-size multiplier (1 = the global size).
    pub local: Option<&'a [f64]>,
}

/// Builds the quad mesh of `trace` with quads of edge length `edge` (times
/// the local multiplier where `sizing` has one).
pub fn compute(
    trace: &TraceOutput,
    sizing: Sizing<'_>,
    edge: f64,
    params: &QuadParams,
    control: &Control,
) -> Result<QuadOutput, Error> {
    let stage = Stage::Quantize;
    control.checkpoint(stage)?;
    if !(edge.is_finite() && edge > 0.0) {
        return Err(Error::invalid_input(stage, "the quad size must be positive"));
    }
    let mut faces_of: Vec<Vec<u32>> = vec![Vec::new(); trace.patches.len()];
    for (f, &p) in trace.patch_of_face.iter().enumerate() {
        if let Some(list) = faces_of.get_mut(p as usize) {
            list.push(f as u32);
        }
    }
    // Patches whose triangles can't be mapped onto their polygon keep them; find
    // them first, so their neighbours sample at their mesh vertices.
    let first = segments::build(trace, &[]);
    let mut excluded = vec![false; trace.patches.len()];
    for (p, sides) in first.sides.iter().enumerate() {
        if p % 256 == 0 {
            control.checkpoint(stage)?;
        }
        if let Some(sides) = sides
            && !mappable(trace, &first, sides, &faces_of[p])
        {
            excluded[p] = true;
        }
    }
    let segs = if excluded.contains(&true) { segments::build(trace, &excluded) } else { first };
    let targets = targets(trace, &segs, sizing, edge);
    let counts = quantize::quantize(&segs, &targets, params.regularity, control)?;
    control.set_progress(stage, 1.0);

    control.checkpoint(Stage::Patterns)?;
    let mut out = Assembly::new(trace, &segs, &counts);
    let mut report = QuadReport { segments: segs.segments.len(), edge_length: edge, ..QuadReport::default() };
    for (p, faces) in faces_of.iter().enumerate() {
        if p % 64 == 0 {
            control.checkpoint(Stage::Patterns)?;
            control.set_progress(Stage::Patterns, p as f32 / trace.patches.len().max(1) as f32);
        }
        match out.fill(p, faces) {
            Some(kind) => match kind {
                PatternKind::Grid => report.grid_patches += 1,
                PatternKind::Center => report.center_patches += 1,
                PatternKind::Split => report.split_patches += 1,
                PatternKind::Fan => report.fan_patches += 1,
            },
            None => {
                out.split_triangles(p, faces);
                report.kept_patches += 1;
            }
        }
    }
    let feature_edges = out.feature_edges();
    let (mut mesh, patch_of_face, fixed, quads, triangles) = out.finish();
    control.checkpoint(Stage::Smooth)?;
    smooth::smooth(&mut mesh, &fixed, &patch_of_face, trace, params.smoothing);
    check_output(&mesh)?;
    report.quads = quads;
    report.triangles = triangles;
    control.set_progress(Stage::Patterns, 1.0);
    Ok(QuadOutput { mesh, patch_of_face, feature_edges, report })
}

/// Whether a patch's triangles map onto its polygon (pins by arc length).
fn mappable(trace: &TraceOutput, segs: &Segments, sides: &[Vec<(u32, bool)>], faces: &[u32]) -> bool {
    let n = sides.len();
    let position = |v: u32| trace.mesh.positions.get(v as usize).copied().unwrap_or(DVec3::ZERO);
    let mut boundary: HashMap<u32, DVec2> = HashMap::new();
    for (i, side) in sides.iter().enumerate() {
        let chain: Vec<u32> = side
            .iter()
            .flat_map(|&(s, reversed)| {
                let v = &segs.segments[s as usize].vertices;
                let mut v = v.clone();
                if reversed {
                    v.reverse();
                }
                v
            })
            .collect();
        let mut acc = vec![0.0];
        for w in chain.windows(2) {
            let last = acc.last().copied().unwrap_or(0.0);
            acc.push(last + (position(w[1]) - position(w[0])).length());
        }
        let total = acc.last().copied().unwrap_or(0.0).max(1e-300);
        for (k, &v) in chain.iter().enumerate() {
            boundary.entry(v).or_insert_with(|| pattern::side_point(n, i, acc[k] / total));
        }
    }
    PatchMap::new(&trace.mesh.positions, &trace.mesh.triangles, faces, &boundary).is_some()
}

/// A last guard before the mesh leaves the stage: finite positions, faces of
/// at least 3 distinct vertices, and no edge in more than two faces. A failure
/// is a bug; the pipeline then shows the layout instead of a broken mesh.
fn check_output(mesh: &PolyMesh) -> Result<(), Error> {
    let bad = |m: &str| Err(Error::internal(Stage::Patterns, format!("quad mesh check failed: {m}")));
    if mesh.positions.iter().any(|p| !p.is_finite()) {
        return bad("non-finite vertex");
    }
    let mut uses: HashMap<(u32, u32), u8> = HashMap::with_capacity(mesh.face_indices.len());
    for w in mesh.face_offsets.windows(2) {
        let face = mesh.face_indices.get(w[0] as usize..w[1] as usize).unwrap_or_default();
        if face.len() < 3 || face.iter().any(|&v| v as usize >= mesh.positions.len()) {
            return bad("malformed face");
        }
        for (i, &a) in face.iter().enumerate() {
            if face[i + 1..].contains(&a) {
                return bad("face repeats a vertex");
            }
            let b = face[(i + 1) % face.len()];
            let n = uses.entry((a.min(b), a.max(b))).or_default();
            *n += 1;
            if *n > 2 {
                return bad("edge in more than two faces");
            }
        }
    }
    Ok(())
}

/// The ideal quad count of every segment: its length over the local quad
/// size, measured with the frame's per-face scales when given.
fn targets(trace: &TraceOutput, segs: &Segments, sizing: Sizing<'_>, edge: f64) -> Vec<f64> {
    let mesh = &trace.mesh;
    let scales = sizing.scales;
    let mut face_of: HashMap<(u32, u32), u32> = HashMap::new();
    for (f, t) in mesh.triangles.iter().enumerate() {
        for i in 0..3 {
            face_of.insert((t[i], t[(i + 1) % 3]), f as u32);
        }
    }
    let local = |f: Option<u32>| -> f64 {
        let multiplier = f
            .and_then(|f| trace.source_face.get(f as usize))
            .and_then(|&g| sizing.local.and_then(|l| l.get(g as usize)))
            .copied()
            .unwrap_or(1.0);
        if multiplier.is_finite() && multiplier > 0.0 { multiplier } else { 1.0 }
    };
    let position = |v: u32| mesh.positions.get(v as usize).copied().unwrap_or(DVec3::ZERO);
    segs.segments
        .iter()
        .map(|s| {
            let length: f64 = s
                .vertices
                .windows(2)
                .map(|w| {
                    let e = position(w[1]) - position(w[0]);
                    let face = face_of.get(&(w[0], w[1])).or_else(|| face_of.get(&(w[1], w[0]))).copied();
                    let size = local(face);
                    let Some(scales) = scales else { return e.length() / size };
                    let Some(f) = face else { return e.length() / size };
                    let (Some(&d), Some(t)) =
                        (mesh.directions.get(f as usize), mesh.triangles.get(f as usize))
                    else {
                        return e.length() / size;
                    };
                    let [a, b, c] = t.map(position);
                    let normal = (b - a).cross(c - a).normalize_or_zero();
                    let across = normal.cross(d);
                    let [sx, sy] = trace
                        .source_face
                        .get(f as usize)
                        .and_then(|&g| scales.get(g as usize))
                        .copied()
                        .filter(|s| s.iter().all(|v| v.is_finite() && *v > 0.0))
                        .unwrap_or([1.0, 1.0]);
                    (e.dot(d) * sx).hypot(e.dot(across) * sy) / size
                })
                .sum();
            (length / edge).max(1e-6)
        })
        .collect()
}

/// The output mesh under construction.
struct Assembly<'a> {
    trace: &'a TraceOutput,
    segs: &'a Segments,
    counts: &'a [u32],
    positions: Vec<DVec3>,
    face_offsets: Vec<u32>,
    face_indices: Vec<u32>,
    patch_of_face: Vec<u32>,
    by_mesh_vertex: HashMap<u32, u32>,
    by_sample: HashMap<(u32, u32), u32>,
    /// Per output vertex: whether smoothing must leave it (on a feature or border).
    fixed: Vec<bool>,
    /// Mesh vertices on a feature line or the border.
    feature_vertex: std::collections::HashSet<u32>,
    /// Mesh edges on a feature line or the border.
    sharp_edges: std::collections::HashSet<(u32, u32)>,
    /// Segments along a feature line or the border.
    feature_segment: Vec<bool>,
    /// Midpoint vertices of mesh edges (patches split from triangles, and the polyline segments around them).
    midpoints: HashMap<(u32, u32), u32>,
    quads: usize,
    triangles: usize,
    /// Cumulative arc length along each segment (canonical order).
    arc: Vec<Vec<f64>>,
}

impl<'a> Assembly<'a> {
    fn new(trace: &'a TraceOutput, segs: &'a Segments, counts: &'a [u32]) -> Self {
        let position = |v: u32| trace.mesh.positions.get(v as usize).copied().unwrap_or(DVec3::ZERO);
        let arc = segs
            .segments
            .iter()
            .map(|s| {
                let mut acc = vec![0.0];
                for w in s.vertices.windows(2) {
                    let last = acc.last().copied().unwrap_or(0.0);
                    acc.push(last + (position(w[1]) - position(w[0])).length());
                }
                acc
            })
            .collect();
        // Feature and border edges of the layout's mesh.
        let mut uses: HashMap<(u32, u32), u32> = HashMap::new();
        for t in &trace.mesh.triangles {
            for i in 0..3 {
                let (a, b) = (t[i], t[(i + 1) % 3]);
                *uses.entry((a.min(b), a.max(b))).or_default() += 1;
            }
        }
        let mut sharp: std::collections::HashSet<(u32, u32)> =
            trace.mesh.feature_edges.iter().map(|&[a, b]| (a.min(b), a.max(b))).collect();
        sharp.extend(uses.iter().filter(|&(_, &n)| n == 1).map(|(&e, _)| e));
        let feature_vertex = sharp.iter().flat_map(|&(a, b)| [a, b]).collect();
        let sharp_edges = sharp.clone();
        let feature_segment = segs
            .segments
            .iter()
            .map(|s| {
                s.vertices
                    .windows(2)
                    .next()
                    .is_some_and(|w| sharp.contains(&(w[0].min(w[1]), w[0].max(w[1]))))
            })
            .collect();
        Self {
            trace,
            segs,
            counts,
            positions: Vec::new(),
            face_offsets: vec![0],
            face_indices: Vec::new(),
            patch_of_face: Vec::new(),
            by_mesh_vertex: HashMap::new(),
            by_sample: HashMap::new(),
            fixed: Vec::new(),
            feature_vertex,
            sharp_edges,
            feature_segment,
            midpoints: HashMap::new(),
            quads: 0,
            triangles: 0,
            arc,
        }
    }

    fn mesh_vertex(&mut self, v: u32) -> u32 {
        if let Some(&id) = self.by_mesh_vertex.get(&v) {
            return id;
        }
        let p = self.trace.mesh.positions.get(v as usize).copied().unwrap_or(DVec3::ZERO);
        let id = self.push(p, self.feature_vertex.contains(&v));
        self.by_mesh_vertex.insert(v, id);
        id
    }

    fn push(&mut self, p: DVec3, fixed: bool) -> u32 {
        self.positions.push(p);
        self.fixed.push(fixed);
        (self.positions.len() - 1) as u32
    }

    /// Sample `k` (0..=n) of segment `s`, in canonical order.
    fn sample(&mut self, s: u32, k: u32) -> u32 {
        let segment = &self.segs.segments[s as usize];
        let n = self.counts[s as usize];
        let last = segment.vertices.len() - 1;
        if k == 0 {
            return self.mesh_vertex(segment.vertices[0]);
        }
        if k >= n {
            return self.mesh_vertex(segment.vertices[last]);
        }
        if segment.polyline {
            // Sampled at its mesh vertices (even k) and its edge midpoints (odd k),
            // matching the kept patch's triangles split into quads.
            let k = k as usize;
            if k % 2 == 0 {
                return self.mesh_vertex(segment.vertices[(k / 2).min(last)]);
            }
            let j = (k / 2).min(last.saturating_sub(1));
            return self.midpoint(segment.vertices[j], segment.vertices[j + 1]);
        }
        if let Some(&id) = self.by_sample.get(&(s, k)) {
            return id;
        }
        let pos = self.arc_point(s, k as f64 / n as f64);
        let id = self.push(pos, self.feature_segment.get(s as usize).copied().unwrap_or(true));
        self.by_sample.insert((s, k), id);
        id
    }

    /// The point at arc-length fraction `t` along segment `s`.
    fn arc_point(&self, s: u32, t: f64) -> DVec3 {
        let segment = &self.segs.segments[s as usize];
        let arc = &self.arc[s as usize];
        let total = arc.last().copied().unwrap_or(0.0);
        let target = t * total;
        let i = arc.partition_point(|&a| a < target).clamp(1, arc.len() - 1);
        let (a0, a1) = (arc[i - 1], arc[i]);
        let w = if a1 > a0 { (target - a0) / (a1 - a0) } else { 0.0 };
        let position = |v: u32| self.trace.mesh.positions.get(v as usize).copied().unwrap_or(DVec3::ZERO);
        position(segment.vertices[i - 1]).lerp(position(segment.vertices[i]), w)
    }

    /// Fills patch `p` with quads; `None` if it must keep its triangles.
    fn fill(&mut self, p: usize, faces: &[u32]) -> Option<PatternKind> {
        let sides = self.segs.sides.get(p)?.as_ref()?;
        let patch = self.trace.patches.get(p)?;
        let n = sides.len();
        let l: Vec<u32> =
            sides.iter().map(|side| side.iter().map(|&(s, _)| self.counts[s as usize]).sum()).collect();
        let pattern = pattern::build(&l)?;

        // Pin every boundary mesh vertex at its domain position.
        let mut boundary: HashMap<u32, DVec2> = HashMap::new();
        for (i, side) in sides.iter().enumerate() {
            let mut before = 0u32;
            for &(s, reversed) in side {
                let segment = &self.segs.segments[s as usize];
                let count = self.counts[s as usize];
                let arc = &self.arc[s as usize];
                let total = arc.last().copied().unwrap_or(0.0);
                let last = segment.vertices.len() - 1;
                for (k, &v) in segment.vertices.iter().enumerate() {
                    // Fraction along the segment in canonical order, then flipped to side order.
                    let tau = if segment.polyline || total <= 0.0 {
                        k as f64 / last as f64
                    } else {
                        arc[k] / total
                    };
                    let tau = if reversed { 1.0 - tau } else { tau };
                    let t = (before as f64 + tau * count as f64) / l[i] as f64;
                    boundary.entry(v).or_insert_with(|| pattern::side_point(n, i, t));
                }
                before += count;
            }
        }
        let map = PatchMap::new(&self.trace.mesh.positions, &self.trace.mesh.triangles, faces, &boundary)?;

        // Place interior points first: nothing is created unless all succeed.
        let mut inner = Vec::new();
        for &(point, q) in &pattern.points {
            if let Point::Inner(_) = point {
                inner.push(map.locate(&self.trace.mesh.positions, q)?);
            }
        }
        for &(point, _) in &pattern.points {
            match point {
                Point::Corner(c) if patch.corners.get(c as usize).is_none() => return None,
                Point::Side(i, m) if self.side_sample(&sides[i as usize], m).is_none() => return None,
                _ => {}
            }
        }
        let mut inner = inner.into_iter();
        let mut ids = Vec::with_capacity(pattern.points.len());
        for &(point, _) in &pattern.points {
            let id = match point {
                Point::Corner(c) => self.mesh_vertex(patch.corners[c as usize]),
                Point::Side(i, m) => {
                    let (s, k) = self.side_sample(&sides[i as usize], m)?;
                    self.sample(s, k)
                }
                Point::Inner(_) => {
                    let pos = inner.next()?;
                    self.push(pos, false)
                }
            };
            ids.push(id);
        }
        for quad in &pattern.quads {
            let face = quad.map(|i| ids[i as usize]);
            self.face_indices.extend_from_slice(&face);
            self.face_offsets.push(self.face_indices.len() as u32);
            self.patch_of_face.push(p as u32);
            self.quads += 1;
        }
        Some(pattern.kind)
    }

    /// The segment sample at boundary sample `m` of a side: `(segment,
    /// canonical index)`. At a junction between two segments the shared end
    /// vertex is returned (as index 0 of the later segment).
    fn side_sample(&self, side: &[(u32, bool)], m: u32) -> Option<(u32, u32)> {
        let mut before = 0;
        for &(s, reversed) in side {
            let count = self.counts[s as usize];
            if m < before + count {
                let j = m - before;
                return Some((s, if reversed { count - j } else { j }));
            }
            before += count;
        }
        let &(s, reversed) = side.last()?;
        let count = self.counts[s as usize];
        Some((s, if reversed { 0 } else { count }))
    }

    /// The quad-mesh edges along feature and border segments that some patch used.
    fn feature_edges(&mut self) -> Vec<[u32; 2]> {
        let mut edges = Vec::new();
        for s in 0..self.segs.segments.len() {
            let used = self.segs.segments[s]
                .uses
                .iter()
                .any(|u| self.segs.sides.get(u.patch as usize).is_some_and(|sides| sides.is_some()));
            if !used || !self.feature_segment.get(s).copied().unwrap_or(false) {
                continue;
            }
            let n = self.counts[s];
            let ids: Vec<u32> = (0..=n).map(|k| self.sample(s as u32, k)).collect();
            edges.extend(ids.windows(2).map(|w| [w[0].min(w[1]), w[0].max(w[1])]));
        }
        edges.sort_unstable();
        edges.dedup();
        edges
    }

    /// The output vertex at the midpoint of mesh edge (a, b), shared by every
    /// face and segment that uses it.
    fn midpoint(&mut self, a: u32, b: u32) -> u32 {
        let key = (a.min(b), a.max(b));
        if let Some(&id) = self.midpoints.get(&key) {
            return id;
        }
        let position = |v: u32| self.trace.mesh.positions.get(v as usize).copied().unwrap_or(DVec3::ZERO);
        let id = self.push(position(a).lerp(position(b), 0.5), self.sharp_edges.contains(&key));
        self.midpoints.insert(key, id);
        id
    }

    /// The fallback for a patch no pattern fills: each of its triangles splits
    /// into 3 quads through its edge midpoints and centroid. The segments
    /// around it are sampled at those same midpoints, so the mesh stays
    /// watertight and all-quad.
    fn split_triangles(&mut self, p: usize, faces: &[u32]) {
        for &f in faces {
            let Some(&t) = self.trace.mesh.triangles.get(f as usize) else { continue };
            let corner = t.map(|v| self.mesh_vertex(v));
            let mid = [0, 1, 2].map(|i| self.midpoint(t[i], t[(i + 1) % 3]));
            let position = |v: u32| self.trace.mesh.positions.get(v as usize).copied().unwrap_or(DVec3::ZERO);
            let centroid = self.push(t.iter().map(|&v| position(v)).sum::<DVec3>() / 3.0, false);
            // Counter-clockwise like the triangle: corner, its outgoing midpoint, centroid, its incoming midpoint.
            for i in 0..3 {
                let quad = [corner[i], mid[i], centroid, mid[(i + 2) % 3]];
                self.face_indices.extend_from_slice(&quad);
                self.face_offsets.push(self.face_indices.len() as u32);
                self.patch_of_face.push(p as u32);
                self.quads += 1;
            }
        }
    }

    fn finish(self) -> (PolyMesh, Vec<u32>, Vec<bool>, usize, usize) {
        let mesh = PolyMesh {
            positions: self.positions,
            face_offsets: self.face_offsets,
            face_indices: self.face_indices,
        };
        (mesh, self.patch_of_face, self.fixed, self.quads, self.triangles)
    }
}
