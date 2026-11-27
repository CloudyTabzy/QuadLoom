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

pub mod curves;
pub mod embed;
pub mod ilp;
pub mod pattern;
pub mod quantize;
pub mod segments;
pub mod smooth;
pub mod target;
#[cfg(test)]
mod tests;
pub mod turn;

use hashbrown::HashMap;
use tinyvec::TinyVec;

use glam::{DVec2, DVec3};

use crate::control::{Control, Stage};
use crate::error::Error;
use crate::mesh::PolyMesh;
use crate::trace::TraceOutput;
use curves::Curves;
use embed::PatchMap;
use pattern::{PatternKind, Point};
use segments::Segments;
use smooth::Hold;

/// Layouts each part gets unless asked otherwise. The best of N draws grows
/// like 0.56, 0.85, 1.03, 1.16, 1.27 of their spread for N = 2 to 6, and
/// measured on the 70 benchmark models four attempts keep 82% of what six
/// gain (three: 67%). The later attempts run side by side, so three and
/// four cost the same, about half as long again as one (decision 0027).
/// The add-on's Layout Attempts starts at the same number.
pub const DEFAULT_ATTEMPTS: usize = 4;

/// Settings for the quad stages.
#[derive(Clone, Debug, PartialEq)]
pub struct QuadParams {
    /// Build the quad mesh (needs the patch layout).
    pub enabled: bool,
    /// Requested total output quads, including symmetry; 0 keeps Density sizing.
    pub target_quads: usize,
    /// Quad edge length as a multiple of the prep remesh's edge length.
    pub edge_scale: f64,
    /// How strongly quantization prefers regular patterns over matching the
    /// target lengths (see [`quantize::DEFAULT_REGULARITY`]).
    pub regularity: f64,
    /// Rounds of smoothing on the surface (0 = none).
    pub smoothing: usize,
    /// How hard the integer program searches for better counts: a multiple
    /// of its branch-and-bound node limits (1 = the default limits). The
    /// limits are node counts, not time, so results stay deterministic.
    pub solver_effort: f64,
    /// How much quad size follows the prep remesh's local detail: 0 keeps
    /// every quad the same size, 1 follows it fully (geometric blend between).
    pub adaptivity: f64,
    /// How many layouts each part gets: 1 is one layout and its quads, more
    /// lays the part out again and keeps the quads that come out best (see
    /// `attempts`).
    pub attempts: usize,
}

impl Default for QuadParams {
    fn default() -> Self {
        Self {
            enabled: true,
            target_quads: 0,
            edge_scale: 0.75,
            regularity: quantize::DEFAULT_REGULARITY,
            smoothing: 20,
            solver_effort: 1.0,
            adaptivity: 0.5,
            attempts: DEFAULT_ATTEMPTS,
        }
    }
}

/// Totals of the quad stages.
#[derive(Clone, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct QuadReport {
    pub quads: usize,
    pub target_quads: usize,
    pub target_attempts: usize,
    pub target_refinements: usize,
    /// Triangles of patches that kept them.
    pub triangles: usize,
    pub segments: usize,
    /// Patches filled with a regular grid / the one-singularity closed form /
    /// a diagonal split / a grid bridged to its sides / the fan fallback.
    pub grid_patches: usize,
    pub center_patches: usize,
    pub split_patches: usize,
    pub bridge_patches: usize,
    pub fan_patches: usize,
    /// Patches left as triangles.
    pub kept_patches: usize,
    /// Quad edge length used.
    pub edge_length: f64,
    /// Layouts made per part (1: only the first).
    pub attempts: usize,
    /// Parts given more than one layout, and how many of them took their
    /// quads from one of the later ones.
    pub parts_retried: usize,
    pub parts_improved: usize,
}

/// How a layout patch was filled.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Fill {
    /// Its triangles, each split into three quads.
    #[default]
    Kept,
    Grid,
    Center,
    Split,
    Bridge,
    Fan,
}

/// What one layout patch came to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PatchNote {
    pub fill: Fill,
    pub quads: u32,
    /// Segments counted for this patch: each segment counts once, for the
    /// lowest-numbered patch it borders.
    pub segments: u32,
}

impl QuadReport {
    /// The totals of `notes` (the counts a report takes from its patches).
    pub(crate) fn tally(&mut self, notes: &[PatchNote]) {
        let mut count = [0usize; 6];
        (self.quads, self.segments) = (0, 0);
        for note in notes {
            count[note.fill as usize] += 1;
            self.quads += note.quads as usize;
            self.segments += note.segments as usize;
        }
        self.kept_patches = count[Fill::Kept as usize];
        self.grid_patches = count[Fill::Grid as usize];
        self.center_patches = count[Fill::Center as usize];
        self.split_patches = count[Fill::Split as usize];
        self.bridge_patches = count[Fill::Bridge as usize];
        self.fan_patches = count[Fill::Fan as usize];
    }
}

/// The quad mesh.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct QuadOutput {
    pub mesh: PolyMesh,
    /// The layout patch of each output face.
    pub patch_of_face: Vec<u32>,
    /// Edges of the quad mesh along feature lines and borders, as vertex pairs.
    pub feature_edges: Vec<[u32; 2]>,
    /// Per layout patch, what it came to (before any mirroring).
    pub patches: Vec<PatchNote>,
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
    retrying_clashes(trace, |keep| {
        let prepared = Prepared::new(trace, keep, control)?;
        let targets = targets(trace, &prepared.segs, sizing, edge);
        let counts =
            quantize::quantize(&prepared.segs, &targets, params.regularity, params.solver_effort, control)?;
        prepared.assemble(trace, &counts, edge, params, None, control)
    })
}

/// What assembling the patches gave.
#[derive(Debug)]
enum Assembled {
    Mesh(Box<QuadOutput>),
    /// These patches' quads can't be used, and the patches should keep
    /// their triangles instead. Either they would share an edge with more
    /// than two faces (it happens where the quads are too big for what they
    /// cover: a tube two quads round, whose two "rungs" join the same two
    /// vertices), or they stray from the surface ([`astray_patches`]).
    Clash(Vec<u32>),
}

/// A quad edge over this many times the quad size is looked at by [`astray_patches`].
const LONG_EDGE: f64 = 3.0;
/// A patch with a long edge whose midpoint is this many quad sizes off the
/// surface keeps its triangles.
const ASTRAY: f64 = 1.0;

/// The patches with a quad edge far longer than was asked for that leaves
/// the surface: its midpoint more than [`ASTRAY`] quad sizes from the
/// layout's mesh.
///
/// A pattern's points are put on the surface one by one; nothing puts its
/// edges there. In a patch whose shape its pattern doesn't suit, two points
/// far apart get joined, and the edge is a chord through the model.
/// SpecularSilkPouf laid out without its faint creases has a seam strip 0.7
/// long and almost no width, with five sides: its pattern's quads ran a
/// third of the model's length each, and the result was 9.4% of the
/// diagonal off its input with no error. A long edge that stays on the
/// surface (a flat face with large quads) is no fault and isn't refused.
fn astray_patches(
    positions: &[DVec3],
    face_offsets: &[u32],
    face_indices: &[u32],
    patch_of_face: &[u32],
    trace: &TraceOutput,
    edge: f64,
) -> Vec<u32> {
    if edge.is_nan() || edge <= 0.0 {
        return Vec::new();
    }
    let at = |v: u32| positions.get(v as usize).copied().unwrap_or(DVec3::ZERO);
    let mut suspects: Vec<(u32, DVec3)> = Vec::new();
    for (f, w) in face_offsets.windows(2).enumerate() {
        let face = face_indices.get(w[0] as usize..w[1] as usize).unwrap_or_default();
        let Some(&patch) = patch_of_face.get(f) else { continue };
        for (i, &a) in face.iter().enumerate() {
            let (pa, pb) = (at(a), at(face[(i + 1) % face.len()]));
            if (pa - pb).length() > LONG_EDGE * edge {
                suspects.push((patch, 0.5 * (pa + pb)));
            }
        }
    }
    if suspects.is_empty() {
        return Vec::new();
    }
    let surface = crate::spatial::TriangleBvh::new(&trace.mesh.positions, &trace.mesh.triangles);
    let mut astray: Vec<u32> = suspects
        .into_iter()
        .filter(|&(_, midpoint)| surface.closest_point(midpoint).is_some_and(|(_, d)| d > ASTRAY * edge))
        .map(|(patch, _)| patch)
        .collect();
    astray.sort_unstable();
    astray.dedup();
    astray
}

/// Rounds of keeping clashing patches as triangles before giving up.
const CLASH_ROUNDS: usize = 4;

/// Runs `build` with the patches to keep as triangles, adding to them the
/// patches whose quads clash until none do. A clash costs those patches
/// their quads (their triangles are split instead, which never clashes:
/// every vertex is a mesh vertex or an edge's midpoint), not the whole mesh.
fn retrying_clashes(
    trace: &TraceOutput,
    mut build: impl FnMut(&[bool]) -> Result<Assembled, Error>,
) -> Result<QuadOutput, Error> {
    let mut keep = vec![false; trace.patches.len()];
    for _ in 0..=CLASH_ROUNDS {
        match build(&keep)? {
            Assembled::Mesh(output) => return Ok(*output),
            Assembled::Clash(patches) => {
                for p in patches {
                    if let Some(flag) = keep.get_mut(p as usize) {
                        *flag = true;
                    }
                }
            }
        }
    }
    Err(Error::internal(
        Stage::Patterns,
        "quad mesh check failed: patches still share an edge or stray from the surface",
    ))
}

struct Prepared {
    faces_of: Vec<Vec<u32>>,
    segs: Segments,
}

impl Prepared {
    /// `keep` marks patches that keep their triangles whatever their shape.
    fn new(trace: &TraceOutput, keep: &[bool], control: &Control) -> Result<Self, Error> {
        let stage = Stage::Quantize;
        let mut faces_of: Vec<Vec<u32>> = vec![Vec::new(); trace.patches.len()];
        for (f, &p) in trace.patch_of_face.iter().enumerate() {
            if let Some(list) = faces_of.get_mut(p as usize) {
                list.push(f as u32);
            }
        }
        // Patches whose triangles can't be mapped onto their polygon keep them,
        // and so do patches that bulge far past their boundary; find them
        // first, so their neighbours sample at their mesh vertices.
        let first = segments::build(trace, keep);
        let mut excluded: Vec<bool> =
            (0..trace.patches.len()).map(|p| keep.get(p).copied().unwrap_or(false)).collect();
        for (p, sides) in first.sides.iter().enumerate() {
            if p % 256 == 0 {
                control.checkpoint(stage)?;
            }
            if let Some(sides) = sides
                && (bulges(trace, p, &faces_of[p]) || !mappable(trace, &first, sides, &faces_of[p]))
            {
                excluded[p] = true;
            }
        }
        let changed = excluded.iter().enumerate().any(|(p, &e)| e != keep.get(p).copied().unwrap_or(false));
        let segs = if changed { segments::build(trace, &excluded) } else { first };
        Ok(Self { faces_of, segs })
    }

    fn assemble(
        &self,
        trace: &TraceOutput,
        counts: &[u32],
        edge: f64,
        params: &QuadParams,
        patterns: Option<(&[Option<pattern::Pattern>], usize)>,
        control: &Control,
    ) -> Result<Assembled, Error> {
        let segs = &self.segs;
        let faces_of = &self.faces_of;
        let stage = Stage::Quantize;
        control.set_progress(stage, 1.0);

        control.checkpoint(Stage::Patterns)?;
        let mut out = Assembly::new(trace, segs, counts);
        let mut report = QuadReport {
            segments: segs.segments.len(),
            edge_length: edge,
            attempts: 1,
            ..QuadReport::default()
        };
        let mut notes = vec![PatchNote::default(); trace.patches.len()];
        for segment in &segs.segments {
            if let Some(note) =
                segment.uses.iter().map(|u| u.patch).min().and_then(|owner| notes.get_mut(owner as usize))
            {
                note.segments += 1;
            }
        }
        for (p, faces) in faces_of.iter().enumerate() {
            let before = out.quads;
            if p % 64 == 0 {
                control.checkpoint(Stage::Patterns)?;
                control.set_progress(Stage::Patterns, p as f32 / trace.patches.len().max(1) as f32);
            }
            let kind = match patterns {
                Some((planned, limit)) => {
                    let pattern = planned.get(p).and_then(Option::as_ref);
                    if pattern.is_some_and(|pattern| {
                        out.quads.checked_add(pattern.quads.len()).is_none_or(|total| total > limit)
                    }) {
                        return Err(Error::invalid_input(
                            Stage::Patterns,
                            "target pattern exceeds the cumulative quad output limit",
                        ));
                    }
                    pattern.and_then(|pattern| out.fill_pattern(p, faces, pattern))
                }
                None => out.fill(p, faces),
            };
            notes[p].fill = match kind {
                Some(PatternKind::Grid) => Fill::Grid,
                Some(PatternKind::Center) => Fill::Center,
                Some(PatternKind::Split) => Fill::Split,
                Some(PatternKind::Bridge) => Fill::Bridge,
                Some(PatternKind::Fan) => Fill::Fan,
                None => Fill::Kept,
            };
            match kind {
                Some(kind) => match kind {
                    PatternKind::Grid => report.grid_patches += 1,
                    PatternKind::Center => report.center_patches += 1,
                    PatternKind::Split => report.split_patches += 1,
                    PatternKind::Bridge => report.bridge_patches += 1,
                    PatternKind::Fan => report.fan_patches += 1,
                },
                None => {
                    if let Some((_, limit)) = patterns
                        && faces
                            .len()
                            .checked_mul(3)
                            .and_then(|n| out.quads.checked_add(n))
                            .is_none_or(|total| total > limit)
                    {
                        return Err(Error::invalid_input(
                            Stage::Patterns,
                            "target fallback exceeds the quad output limit",
                        ));
                    }
                    out.split_triangles(p, faces);
                    report.kept_patches += 1;
                }
            }
            notes[p].quads = (out.quads - before) as u32;
        }
        let mut refused = out.clashing_patches();
        refused.extend(astray_patches(
            &out.positions,
            &out.face_offsets,
            &out.face_indices,
            &out.patch_of_face,
            trace,
            edge,
        ));
        if !refused.is_empty() {
            refused.sort_unstable();
            refused.dedup();
            return Ok(Assembled::Clash(refused));
        }
        let feature_edges = out.feature_edges();
        let (mut mesh, patch_of_face, mut holds, curves, quads, triangles) = out.finish();
        control.checkpoint(Stage::Smooth)?;
        smooth::smooth(
            &mut mesh,
            &mut holds,
            &feature_edges,
            &curves,
            &patch_of_face,
            trace,
            params.smoothing,
            control,
        );
        check_output(&mesh)?;
        report.quads = quads;
        report.triangles = triangles;
        control.set_progress(Stage::Patterns, 1.0);
        Ok(Assembled::Mesh(Box::new(QuadOutput {
            mesh,
            patch_of_face,
            feature_edges,
            patches: notes,
            report,
        })))
    }
}

/// A patch with this many times the area of a square with its perimeter
/// keeps its triangles. A square and a disk have about once, a hemisphere on
/// its rim two and a half times; past four the patch is more than a
/// hemisphere, or a tube five times longer than wide closed at the far end.
const BULGE: f64 = 4.0;

/// Whether patch `p` has far more area than its boundary could span.
///
/// Quads are counted along a patch's boundary, so its pattern holds about as
/// many as fit in the boundary laid flat. Given a patch that is most of a
/// closed part behind a boundary of a few edges, the pattern draws a few
/// quads across the mouth and the part behind it is gone from the result:
/// no error, and a surface that is no longer the input's. Such a patch is a
/// layout that failed (its part wanted loops it never got), and it is rare:
/// one patch in the 70 benchmark models. Keeping its triangles keeps the
/// shape.
fn bulges(trace: &TraceOutput, p: usize, faces: &[u32]) -> bool {
    let position = |v: u32| trace.mesh.positions.get(v as usize).copied().unwrap_or(DVec3::ZERO);
    let Some(patch) = trace.patches.get(p) else { return false };
    let perimeter: f64 = patch
        .sides
        .iter()
        .map(|side| side.vertices.windows(2).map(|w| (position(w[0]) - position(w[1])).length()).sum::<f64>())
        .sum();
    let area: f64 = faces
        .iter()
        .filter_map(|&f| trace.mesh.triangles.get(f as usize))
        .map(|t| {
            let [a, b, c] = t.map(position);
            0.5 * (b - a).cross(c - a).length()
        })
        .sum();
    16.0 * area > BULGE * perimeter * perimeter
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

/// Where an output vertex was put on the layout's mesh, which decides how
/// smoothing may move it.
#[derive(Clone, Copy, Debug)]
enum Origin {
    /// Inside a patch.
    Inner,
    /// At mesh vertex `v`.
    Vertex(u32),
    /// A fraction `w` of the way along mesh edge `(a, b)`.
    Edge { a: u32, b: u32, w: f64 },
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
    /// Per output vertex: where on the layout's mesh it was put.
    origin: Vec<Origin>,
    /// Mesh vertices on a feature line or the border.
    feature_vertex: hashbrown::HashSet<u32>,
    /// Mesh edges on a feature line or the border.
    sharp_edges: hashbrown::HashSet<(u32, u32)>,
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
        let mut sharp: hashbrown::HashSet<(u32, u32)> =
            trace.mesh.feature_edges.iter().map(|&[a, b]| (a.min(b), a.max(b))).collect();
        sharp.extend(uses.iter().filter(|&(_, &n)| n == 1).map(|(&e, _)| e));
        let feature_vertex = sharp.iter().flat_map(|&(a, b)| [a, b]).collect();
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
        let sharp_edges = sharp;
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
            origin: Vec::new(),
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
        let id = self.push(p, Origin::Vertex(v));
        self.by_mesh_vertex.insert(v, id);
        id
    }

    fn push(&mut self, p: DVec3, origin: Origin) -> u32 {
        self.positions.push(p);
        self.origin.push(origin);
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
            if k.is_multiple_of(2) {
                return self.mesh_vertex(segment.vertices[(k / 2).min(last)]);
            }
            let j = (k / 2).min(last.saturating_sub(1));
            return self.midpoint(segment.vertices[j], segment.vertices[j + 1]);
        }
        if let Some(&id) = self.by_sample.get(&(s, k)) {
            return id;
        }
        let (pos, origin) = self.arc_point(s, k as f64 / n as f64);
        let id = self.push(pos, origin);
        self.by_sample.insert((s, k), id);
        id
    }

    /// The point at arc-length fraction `t` along segment `s`, and the mesh
    /// edge it lies on.
    fn arc_point(&self, s: u32, t: f64) -> (DVec3, Origin) {
        let segment = &self.segs.segments[s as usize];
        let arc = &self.arc[s as usize];
        let total = arc.last().copied().unwrap_or(0.0);
        let target = t * total;
        let i = arc.partition_point(|&a| a < target).clamp(1, arc.len() - 1);
        let (a0, a1) = (arc[i - 1], arc[i]);
        let w = if a1 > a0 { (target - a0) / (a1 - a0) } else { 0.0 };
        let position = |v: u32| self.trace.mesh.positions.get(v as usize).copied().unwrap_or(DVec3::ZERO);
        let (a, b) = (segment.vertices[i - 1], segment.vertices[i]);
        (position(a).lerp(position(b), w), Origin::Edge { a, b, w })
    }

    /// Fills patch `p` with quads; `None` if it must keep its triangles.
    fn fill(&mut self, p: usize, faces: &[u32]) -> Option<PatternKind> {
        let sides = self.segs.sides.get(p)?.as_ref()?;
        let l: Vec<u32> =
            sides.iter().map(|side| side.iter().map(|&(s, _)| self.counts[s as usize]).sum()).collect();
        let pattern = pattern::build(&l)?;
        self.fill_pattern(p, faces, &pattern)
    }

    fn fill_pattern(&mut self, p: usize, faces: &[u32], pattern: &pattern::Pattern) -> Option<PatternKind> {
        let sides = self.segs.sides.get(p)?.as_ref()?;
        let patch = self.trace.patches.get(p)?;
        let n = sides.len();
        let l: Vec<u32> =
            sides.iter().map(|side| side.iter().map(|&(s, _)| self.counts[s as usize]).sum()).collect();

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
        let domain = pattern.place();
        let mut inner = Vec::new();
        for (&(point, _), &q) in pattern.points.iter().zip(&domain) {
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
                    self.push(pos, Origin::Inner)
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

    /// The patches with a face on an edge that more than two faces use
    /// (sorted). Empty for a mesh that is a surface.
    fn clashing_patches(&self) -> Vec<u32> {
        let mut uses: HashMap<(u32, u32), TinyVec<[u32; 4]>> =
            HashMap::with_capacity(self.face_indices.len() / 2);
        for (f, w) in self.face_offsets.windows(2).enumerate() {
            let face = self.face_indices.get(w[0] as usize..w[1] as usize).unwrap_or_default();
            let patch = self.patch_of_face.get(f).copied().unwrap_or(u32::MAX);
            for (i, &a) in face.iter().enumerate() {
                let b = face[(i + 1) % face.len()];
                uses.entry((a.min(b), a.max(b))).or_default().push(patch);
            }
        }
        let mut patches: Vec<u32> =
            uses.into_values().filter(|faces| faces.len() > 2).flat_map(|faces| faces.into_iter()).collect();
        patches.sort_unstable();
        patches.dedup();
        patches
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
        let id = self.push(position(a).lerp(position(b), 0.5), Origin::Edge { a, b, w: 0.5 });
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
            let centroid = self.push(t.iter().map(|&v| position(v)).sum::<DVec3>() / 3.0, Origin::Inner);
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

    /// The mesh, each face's patch, how smoothing may move each vertex, the
    /// feature curves those holds refer to, and the quad and triangle counts.
    fn finish(self) -> (PolyMesh, Vec<u32>, Vec<Hold>, Curves, usize, usize) {
        // Feature lines and borders, as curves their vertices slide along. A
        // vertex where such a curve ends, branches or turns a corner stays.
        let curves = Curves::new(&self.trace.mesh.positions, self.sharp_edges.iter().copied());
        let on_curve =
            |at: Option<(u32, f64)>| at.map_or(Hold::Fixed, |(chain, at)| Hold::Curve { chain, at });
        let holds = self
            .origin
            .iter()
            .map(|&origin| match origin {
                Origin::Inner => Hold::Free,
                Origin::Vertex(v) if !self.feature_vertex.contains(&v) => Hold::Free,
                Origin::Vertex(v) => on_curve(curves.at_vertex(v)),
                Origin::Edge { a, b, .. } if !self.sharp_edges.contains(&(a.min(b), a.max(b))) => Hold::Free,
                Origin::Edge { a, b, w } => on_curve(curves.on_edge(a, b, w)),
            })
            .collect();
        let mesh = PolyMesh {
            positions: self.positions,
            face_offsets: self.face_offsets,
            face_indices: self.face_indices,
        };
        (mesh, self.patch_of_face, holds, curves, self.quads, self.triangles)
    }
}
