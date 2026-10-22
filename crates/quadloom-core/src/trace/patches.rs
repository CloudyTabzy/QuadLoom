//! The patches cut out by the boundary and the chosen paths, and the
//! conditions they must (validity) or should (quality) meet.
//!
//! Validity (the paper's §5.1): a disk, with only convex or flat boundary
//! vertices, and 3 to `max_sides` sides (one side per convex corner).
//! Quality: "CC-able" side lengths, from the closed-form conditions of
//! Tarini, *Closed-form Quadrangulation of n-Sided Patches* (2021), and a
//! valence match: no singularity inside a 4-sided patch, one singularity of
//! index k (in quarters) inside a (4 − k)-sided one.

use std::collections::HashMap;

use glam::DVec3;

use super::cut::CutMesh;
use super::graph::FieldGraph;
use super::search::TracePath;
use super::turns::{FaceCrosses, VertexClass, turn_at};

/// The directed edges of the chosen paths, each with its travel direction.
#[derive(Clone, Debug, Default)]
pub struct PathEdges {
    map: HashMap<(u32, u32), DVec3>,
    /// Path edges at each vertex.
    degree: HashMap<u32, u32>,
    /// Per half-edge of the mesh given to [`Self::for_mesh`]: on a path.
    half: Vec<bool>,
}

impl PathEdges {
    pub fn new() -> Self {
        Self::default()
    }

    /// Path edges that also answer "is this half-edge on a path" in O(1).
    pub fn for_mesh(cut: &CutMesh) -> Self {
        Self { half: vec![false; 3 * cut.face_count()], ..Self::default() }
    }

    fn mark(&mut self, cut: &CutMesh, a: u32, b: u32, on: bool) {
        if !self.half.is_empty() {
            for h in [cut.half_edge(a, b), cut.half_edge(b, a)].into_iter().flatten() {
                self.half[h as usize] = on;
            }
        }
    }

    /// Whether half-edge `h` of `cut` runs along a path.
    pub fn cuts(&self, cut: &CutMesh, h: u32) -> bool {
        match self.half.get(h as usize) {
            Some(&on) => on,
            None => self.contains(cut.tail(h), cut.head(h)),
        }
    }

    /// Adds edge `a → b` of `cut` travelling along `d`.
    pub fn insert_edge_on(&mut self, cut: &CutMesh, a: u32, b: u32, d: DVec3) {
        self.insert_edge(a, b, d);
        self.mark(cut, a, b, true);
    }

    /// Adds `path`'s mesh edges (not its twin steps), labelled with the
    /// path's direction at each step's start.
    pub fn insert(&mut self, path: &TracePath, cut: &CutMesh, graph: &FieldGraph) {
        for (a, b) in path.steps() {
            let (x, y) = (a.vertex(), b.vertex());
            if cut.twins[x as usize].contains(&y) {
                continue;
            }
            let d =
                graph.direction(x, a.dir()).unwrap_or(cut.positions[y as usize] - cut.positions[x as usize]);
            self.insert_edge_on(cut, x, y, d);
        }
    }

    /// Adds edge `a → b` travelling along `d`.
    pub fn insert_edge(&mut self, a: u32, b: u32, d: DVec3) {
        if self.map.insert((a, b), d).is_none() {
            *self.degree.entry(a).or_default() += 1;
            *self.degree.entry(b).or_default() += 1;
        }
    }

    pub fn remove(&mut self, path: &TracePath, cut: &CutMesh) {
        for (a, b) in path.steps() {
            let (x, y) = (a.vertex(), b.vertex());
            if self.map.remove(&(x, y)).is_some() {
                if !self.contains(x, y) {
                    self.mark(cut, x, y, false);
                }
                for v in [x, y] {
                    if let Some(d) = self.degree.get_mut(&v) {
                        *d = d.saturating_sub(1);
                    }
                }
            }
        }
    }

    /// How many path edges meet at `v`.
    pub fn degree(&self, v: u32) -> u32 {
        self.degree.get(&v).copied().unwrap_or(0)
    }

    /// Whether `a`–`b` (either way round) is a path edge.
    pub fn contains(&self, a: u32, b: u32) -> bool {
        self.map.contains_key(&(a, b)) || self.map.contains_key(&(b, a))
    }

    /// The travel direction along `a → b`, if it is a path edge.
    pub fn label(&self, a: u32, b: u32) -> Option<DVec3> {
        self.map.get(&(a, b)).copied().or_else(|| self.map.get(&(b, a)).map(|&d| -d))
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

/// What a patch needs (or lacks), in the order the conditions are checked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum PatchKind {
    /// Not homeomorphic to a disk.
    NonDisk,
    /// Has a concave, narrow or cusp boundary vertex.
    NotConvex,
    /// Fewer than 3 sides.
    LowSides,
    /// More than `max_sides` sides.
    HighSides,
    /// Side lengths far from any single-singularity quadrangulation.
    NotCcAble,
    /// Its singularities don't match its number of sides.
    ValenceMismatch,
    Ok,
}

impl PatchKind {
    /// Whether the patch can be quadrangulated at all (validity, not quality).
    pub fn is_valid(self) -> bool {
        matches!(self, Self::NotCcAble | Self::ValenceMismatch | Self::Ok)
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::NonDisk => "non_disk",
            Self::NotConvex => "not_convex",
            Self::LowSides => "low_sides",
            Self::HighSides => "high_sides",
            Self::NotCcAble => "not_cc_able",
            Self::ValenceMismatch => "valence_mismatch",
            Self::Ok => "ok",
        }
    }
}

/// One side of a patch: from one corner to the next.
#[derive(Clone, Debug, PartialEq)]
pub struct SideInfo {
    /// Corner to corner, inclusive.
    pub vertices: Vec<u32>,
    /// Length along the field.
    pub length: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PatchInfo {
    /// Ascending.
    pub faces: Vec<u32>,
    pub loops: usize,
    /// Each boundary loop's vertices, in order (patch on the left).
    pub loop_vertices: Vec<Vec<u32>>,
    /// Convex corners in loop order (counter-clockwise around the patch).
    pub corners: Vec<u32>,
    pub sides: Vec<SideInfo>,
    /// Boundary vertices that aren't convex or flat.
    pub not_convex: Vec<(u32, VertexClass)>,
    /// Singular vertices inside the patch, `(vertex, quarters)`.
    pub singularities: Vec<(u32, i32)>,
    pub kind: PatchKind,
    /// [`cc_margin`] of the side lengths.
    pub cc_margin: f64,
    /// Whether the side lengths pass the CC-ability test (always, for 4
    /// sides when matching valence; never, outside 3–6 sides).
    pub cc_able: bool,
    /// For a single loop: each boundary step, in loop order.
    pub steps: Vec<BoundaryStep>,
    /// A disk as cut open (along features and paths).
    pub cut_disk: bool,
    /// How far the cut-open patch is from a disk: 1 − χ (+2 if closed).
    pub excess: i64,
    /// 1 − χ with features glued back (pinches and self-gluing show here).
    pub merged_excess: i64,
    /// Boundary half-edges whose other side is this same patch.
    pub glued: usize,
}

/// One vertex along a patch's boundary loop.
#[derive(Clone, Debug, PartialEq)]
pub struct BoundaryStep {
    pub vertex: u32,
    pub class: VertexClass,
    /// The patch's interior angle there (radians).
    pub angle: f64,
    /// Field length of the edge arriving at `vertex`.
    pub length: f64,
}

impl PatchInfo {
    /// A non-quad patch whose one singularity matches its side count.
    pub fn matches_singularity(&self) -> bool {
        let n = self.sides.len();
        expected_sides(&self.singularities).is_some_and(|e| e != 4 && e == n)
    }
}

/// What the patch conditions depend on.
#[derive(Clone, Copy, Debug)]
pub struct PatchContext<'a> {
    pub cut: &'a CutMesh,
    pub crosses: &'a FaceCrosses,
    /// Index in quarters per cut vertex (0 for regular vertices).
    pub singular: &'a [i32],
    /// Vertices whose boundary turns are measured geometrically (see [`turn_at`]).
    pub geometric_turns: &'a [bool],
    /// Each cut vertex's id in the uncut mesh (empty: the cut ids themselves).
    /// A patch must be a disk with features glued back, as in the final mesh.
    pub merge_id: &'a [u32],
    pub max_sides: usize,
    pub match_valence: bool,
    /// Added to the shortest side when testing CC-ability.
    pub tolerance: f64,
}

/// Tarini's sub-side counts sᵢ (eᵢ = sᵢ₋₁ + sᵢ₊₁) for n = 3, 5 and 6 sides.
pub fn subside_counts(e: &[f64]) -> Option<Vec<f64>> {
    let n = e.len();
    let at = |i: isize| e[i.rem_euclid(n as isize) as usize];
    let s = |i: usize| -> f64 {
        let i = i as isize;
        match n {
            3 => 0.5 * (at(i - 1) - at(i) + at(i + 1)),
            5 => 0.5 * ((at(i - 1) + at(i) + at(i + 1)) - (at(i - 2) + at(i + 2))),
            _ => 0.5 * (at(i - 1) + at(i + 1) - at(i + 3)),
        }
    };
    matches!(n, 3 | 5 | 6).then(|| (0..n).map(s).collect())
}

/// How far the side lengths are from admitting a single-singularity
/// quadrangulation: 2·min sᵢ for 3, 5 and 6 sides (negative when some
/// sub-side would be negative), and −max |opposite-side difference| for 4.
pub fn cc_margin(lengths: &[f64]) -> f64 {
    match lengths.len() {
        4 => -(lengths[0] - lengths[2]).abs().max((lengths[1] - lengths[3]).abs()),
        _ => subside_counts(lengths)
            .map_or(f64::NEG_INFINITY, |s| 2.0 * s.iter().copied().fold(f64::INFINITY, f64::min)),
    }
}

/// The number of sides a patch with these interior singularities should have.
pub fn expected_sides(singularities: &[(u32, i32)]) -> Option<usize> {
    match singularities {
        [] => Some(4),
        [(_, k)] => usize::try_from(4 - k).ok(),
        _ => None,
    }
}

/// The patches of a cut mesh with paths.
#[derive(Clone, Debug)]
pub struct Partition {
    patch_of_face: Vec<u32>,
    infos: Vec<Option<PatchInfo>>,
    scratch: Scratch,
}

/// Reusable marks for [`describe`] (all zero between calls).
#[derive(Clone, Debug, Default)]
struct Scratch {
    vertex: Vec<u8>,
}

/// What an update changed.
#[derive(Clone, Debug, Default)]
pub struct Change {
    pub removed: Vec<PatchInfo>,
    pub added: Vec<u32>,
}

const UNSET: u32 = u32::MAX;

impl Partition {
    pub fn new(ctx: &PatchContext, paths: &PathEdges) -> Self {
        let mut partition = Self {
            patch_of_face: vec![UNSET; ctx.cut.face_count()],
            infos: Vec::new(),
            scratch: Scratch::default(),
        };
        let all: Vec<u32> = (0..ctx.cut.face_count() as u32).collect();
        partition.flood(ctx, paths, &all);
        partition
    }

    pub fn patch_of_face(&self) -> &[u32] {
        &self.patch_of_face
    }

    pub fn info(&self, p: u32) -> Option<&PatchInfo> {
        self.infos.get(p as usize).and_then(|i| i.as_ref())
    }

    /// Re-declares corners of every convex disk with too few or too many
    /// sides (see [`redeclare_corners`]); returns how many changed.
    pub fn redeclare_corners(&mut self, ctx: &PatchContext) -> usize {
        let mut changed = 0;
        for info in self.infos.iter_mut().flatten() {
            changed += usize::from(redeclare_corners(info, ctx));
        }
        changed
    }

    /// Live patches, by id.
    pub fn patches(&self) -> impl Iterator<Item = (u32, &PatchInfo)> {
        self.infos.iter().enumerate().filter_map(|(p, i)| i.as_ref().map(|i| (p as u32, i)))
    }

    /// Re-derives the patches containing `touched` faces (after paths through
    /// them were added or removed).
    pub fn update(&mut self, ctx: &PatchContext, paths: &PathEdges, touched: &[u32]) -> Change {
        let mut ids: Vec<u32> =
            touched.iter().map(|&f| self.patch_of_face[f as usize]).filter(|&p| p != UNSET).collect();
        ids.sort_unstable();
        ids.dedup();
        let mut faces = Vec::new();
        let mut removed = Vec::with_capacity(ids.len());
        for p in ids {
            if let Some(info) = self.infos[p as usize].take() {
                for &f in &info.faces {
                    self.patch_of_face[f as usize] = UNSET;
                }
                faces.extend_from_slice(&info.faces);
                removed.push(info);
            }
        }
        faces.sort_unstable();
        let added = self.flood(ctx, paths, &faces);
        Change { removed, added }
    }

    /// Assigns new patches to the unset faces among `faces` (ascending).
    fn flood(&mut self, ctx: &PatchContext, paths: &PathEdges, faces: &[u32]) -> Vec<u32> {
        let cut = ctx.cut;
        let mut added = Vec::new();
        let mut stack = Vec::new();
        for &seed in faces {
            if self.patch_of_face[seed as usize] != UNSET {
                continue;
            }
            let id = self.infos.len() as u32;
            self.infos.push(None);
            let mut members = vec![seed];
            self.patch_of_face[seed as usize] = id;
            stack.push(seed);
            while let Some(f) = stack.pop() {
                for h in 3 * f..3 * f + 3 {
                    if is_cut(cut, paths, h) {
                        continue;
                    }
                    if let Some(t) = cut.twin(h) {
                        let g = t / 3;
                        if self.patch_of_face[g as usize] == UNSET {
                            self.patch_of_face[g as usize] = id;
                            members.push(g);
                            stack.push(g);
                        }
                    }
                }
            }
            members.sort_unstable();
            self.infos[id as usize] = Some(describe(ctx, paths, members, &mut self.scratch));
            added.push(id);
        }
        added
    }
}

fn is_cut(cut: &CutMesh, paths: &PathEdges, h: u32) -> bool {
    cut.twin(h).is_none() || paths.cuts(cut, h)
}

/// The loops, corners, sides, singularities and kind of the patch made of `faces`.
fn describe(ctx: &PatchContext, paths: &PathEdges, faces: Vec<u32>, scratch: &mut Scratch) -> PatchInfo {
    let cut = ctx.cut;
    let cut_at = |h: u32| is_cut(cut, paths, h);
    let label = |h: u32| {
        let (a, b) = (cut.tail(h), cut.head(h));
        paths.label(a, b).unwrap_or(cut.positions[b as usize] - cut.positions[a as usize])
    };
    let boundary: Vec<u32> = faces.iter().flat_map(|&f| 3 * f..3 * f + 3).filter(|&h| cut_at(h)).collect();
    // Marks: 1 = a vertex of the patch, 2 = on its boundary.
    let mark = &mut scratch.vertex;
    mark.resize(cut.vertex_count(), 0);
    let mut vertices = Vec::new();
    for &f in &faces {
        for v in cut.triangles[f as usize] {
            if mark[v as usize] == 0 {
                mark[v as usize] = 1;
                vertices.push(v);
            }
        }
    }
    let mut boundary_count = 0;
    for &h in &boundary {
        let v = cut.tail(h);
        if mark[v as usize] == 1 {
            mark[v as usize] = 2;
            boundary_count += 1;
        }
    }
    let interior = vertices.len() - boundary_count;
    // χ = V − E + F of the patch cut along its boundary (see the module notes).
    let chi2 = 2 * interior as i64 + boundary.len() as i64 - faces.len() as i64;

    // Walk the loops; each step is (half-edge, class at its head).
    let mut seen: std::collections::HashSet<u32> = std::collections::HashSet::with_capacity(boundary.len());
    let mut loops: Vec<Vec<(u32, VertexClass, f64)>> = Vec::new();
    let mut broken = false;
    for &start in &boundary {
        if seen.contains(&start) {
            continue;
        }
        let mut steps = Vec::new();
        let mut h = start;
        loop {
            seen.insert(h);
            let turn = turn_at(cut, ctx.crosses, ctx.geometric_turns, h, &cut_at, &label);
            steps.push((h, turn.class, turn.angle));
            h = turn.outgoing;
            if h == start {
                break;
            }
            if seen.contains(&h) || steps.len() > boundary.len() {
                broken = true;
                break;
            }
        }
        loops.push(steps);
    }
    let not_convex: Vec<(u32, VertexClass)> = loops
        .iter()
        .flatten()
        .filter(|(_, c, _)| !c.is_convex())
        .map(|&(h, c, _)| (cut.head(h), c))
        .collect();
    // A single loop keeps its steps, each with its edge's field length.
    let steps: Vec<BoundaryStep> = match loops.as_slice() {
        [steps] => steps
            .iter()
            .map(|&(h, class, angle)| {
                let e = cut.positions[cut.head(h) as usize] - cut.positions[cut.tail(h) as usize];
                let length = ctx.crosses.closest(h / 3, label(h)).map_or(e.length(), |(_, d)| e.dot(d).abs());
                BoundaryStep { vertex: cut.head(h), class, angle, length }
            })
            .collect(),
        _ => Vec::new(),
    };
    let (corners, sides) = sides_between(&steps, &|k| steps[k].class.is_corner());
    let mut singularities: Vec<(u32, i32)> = vertices
        .iter()
        .filter(|&&v| mark[v as usize] == 1)
        .filter_map(|&v| ctx.singular.get(v as usize).filter(|&&k| k != 0).map(|&k| (v, k)))
        .collect();
    singularities.sort_unstable();
    for &v in &vertices {
        mark[v as usize] = 0;
    }
    let mut info = PatchInfo {
        faces,
        loops: loops.len(),
        loop_vertices: loops
            .iter()
            .map(|steps| steps.iter().map(|&(h, _, _)| cut.head(h)).collect())
            .collect(),
        corners,
        sides,
        not_convex,
        singularities,
        kind: PatchKind::Ok,
        cc_margin: 0.0,
        cc_able: false,
        steps,
        cut_disk: !broken && loops.len() == 1 && chi2 == 2,
        // 1 − χ, plus 2 for a closed piece (so a sphere needs a loop too).
        excess: 1 - chi2 / 2 + if loops.is_empty() { 2 } else { 0 },
        merged_excess: 0,
        glued: 0,
    };
    // Boundary edges glued to the same patch on their other side (a feature
    // or path with this patch on both sides), as merged edges.
    let merged_of = |v: u32| ctx.merge_id.get(v as usize).copied().unwrap_or(v);
    let directed: std::collections::HashSet<(u32, u32)> =
        boundary.iter().map(|&h| (merged_of(cut.tail(h)), merged_of(cut.head(h)))).collect();
    info.glued = boundary
        .iter()
        .filter(|&&h| directed.contains(&(merged_of(cut.head(h)), merged_of(cut.tail(h)))))
        .count();
    // χ with feature copies merged: a patch on both sides of a feature line
    // (an annulus joined by it, say) is a disk cut open but not in the mesh.
    let merged = |v: u32| ctx.merge_id.get(v as usize).copied().unwrap_or(v);
    let mut merged_vertices: Vec<u32> =
        info.faces.iter().flat_map(|&f| cut.triangles[f as usize]).map(merged).collect();
    merged_vertices.sort_unstable();
    merged_vertices.dedup();
    let mut merged_edges: Vec<(u32, u32)> = info
        .faces
        .iter()
        .flat_map(|&f| {
            let t = cut.triangles[f as usize].map(merged);
            (0..3).map(move |i| (t[i].min(t[(i + 1) % 3]), t[i].max(t[(i + 1) % 3])))
        })
        .collect();
    merged_edges.sort_unstable();
    merged_edges.dedup();
    let merged_chi = merged_vertices.len() as i64 - merged_edges.len() as i64 + info.faces.len() as i64;
    info.merged_excess = 1 - merged_chi;
    let disk = info.cut_disk && merged_chi == 1;
    classify(&mut info, ctx, disk);
    info
}

/// Sets `info`'s CC margin, CC-ability and kind from its sides.
fn classify(info: &mut PatchInfo, ctx: &PatchContext, disk: bool) {
    let lengths: Vec<f64> = info.sides.iter().map(|s| s.length).collect();
    let margin = cc_margin(&lengths);
    let n = info.sides.len();
    let shortest = lengths.iter().copied().fold(f64::INFINITY, f64::min);
    let cc_able =
        (n == 4 && ctx.match_valence) || ((3..=6).contains(&n) && margin + shortest + ctx.tolerance > 0.0);
    info.cc_margin = margin;
    info.cc_able = cc_able;
    info.kind = if !disk {
        PatchKind::NonDisk
    } else if !info.not_convex.is_empty() {
        PatchKind::NotConvex
    } else if n < 3 {
        PatchKind::LowSides
    } else if n > ctx.max_sides {
        PatchKind::HighSides
    } else if !cc_able {
        PatchKind::NotCcAble
    } else if ctx.match_valence && expected_sides(&info.singularities) != Some(n) {
        PatchKind::ValenceMismatch
    } else {
        PatchKind::Ok
    };
}

/// Interior angles below this (radians) make a vertex a natural extra corner.
const SHARP_ENOUGH: f64 = 150.0 * std::f64::consts::PI / 180.0;

/// The oracle's last resort (`FixValences`): a convex disk with too few or
/// too many sides gets corners re-declared, with no geometry changed. Too
/// few: the sharpest non-corner vertex becomes a corner, or, where the
/// boundary is smooth, the middle of the longest side. Too many: the
/// flattest corners stop being corners. Returns whether `info` changed.
pub fn redeclare_corners(info: &mut PatchInfo, ctx: &PatchContext) -> bool {
    let n_sides = info.sides.len();
    let out_of_range = n_sides < 3 || n_sides > ctx.max_sides;
    let eligible = matches!(info.kind, PatchKind::LowSides | PatchKind::HighSides)
        || (info.kind == PatchKind::NotConvex && out_of_range);
    if !eligible || info.steps.len() < 3 {
        return false;
    }
    let n = info.steps.len();
    let mut corner: Vec<bool> = info.steps.iter().map(|s| s.class.is_corner()).collect();
    let count = |c: &[bool]| c.iter().filter(|&&x| x).count();
    while count(&corner) < 3 {
        let sharpest = (0..n)
            .filter(|&k| !corner[k])
            .min_by(|&a, &b| info.steps[a].angle.total_cmp(&info.steps[b].angle).then(a.cmp(&b)));
        let Some(sharpest) = sharpest else { break };
        let pick = if info.steps[sharpest].angle < SHARP_ENOUGH || count(&corner) == 0 {
            sharpest
        } else {
            middle_of_longest_side(&info.steps, &corner).unwrap_or(sharpest)
        };
        corner[pick] = true;
    }
    while count(&corner) > ctx.max_sides {
        let flattest = (0..n)
            .filter(|&k| corner[k])
            .max_by(|&a, &b| info.steps[a].angle.total_cmp(&info.steps[b].angle).then(b.cmp(&a)));
        let Some(flattest) = flattest else { break };
        corner[flattest] = false;
    }
    let (corners, sides) = sides_between(&info.steps, &|k| corner[k]);
    info.corners = corners;
    info.sides = sides;
    classify(info, ctx, true);
    true
}

/// The step halfway (by length) along the longest side between corners.
fn middle_of_longest_side(steps: &[BoundaryStep], corner: &[bool]) -> Option<usize> {
    let n = steps.len();
    let first = (0..n).find(|&k| corner[k])?;
    let (mut best, mut best_len) = (None, -1.0);
    let mut k = first;
    for _ in 0..n {
        // The side from corner k to the next corner.
        let mut members = Vec::new();
        let mut length = 0.0;
        let mut j = (k + 1) % n;
        while !corner[j] {
            length += steps[j].length;
            members.push((j, length));
            j = (j + 1) % n;
        }
        length += steps[j].length;
        if length > best_len && !members.is_empty() {
            let half = length / 2.0;
            best =
                members.iter().min_by(|a, b| (a.1 - half).abs().total_cmp(&(b.1 - half).abs())).map(|m| m.0);
            best_len = length;
        }
        k = j;
        if k == first {
            break;
        }
    }
    best
}

/// The corners (steps where `is_corner` holds) of a single loop and the
/// sides between them. A side's length sums its edges along one field
/// direction, so the zig-zags of a traced path don't count.
fn sides_between(steps: &[BoundaryStep], is_corner: &dyn Fn(usize) -> bool) -> (Vec<u32>, Vec<SideInfo>) {
    let n = steps.len();
    let Some(first) = (0..n).find(|&k| is_corner(k)) else {
        return (Vec::new(), Vec::new());
    };
    let (mut corners, mut sides) = (Vec::new(), Vec::new());
    let mut side = SideInfo { vertices: vec![steps[first].vertex], length: 0.0 };
    for k in 1..=n {
        let step = &steps[(first + k) % n];
        side.length += step.length;
        side.vertices.push(step.vertex);
        if is_corner((first + k) % n) {
            corners.push(step.vertex);
            let next = SideInfo { vertices: vec![step.vertex], length: 0.0 };
            sides.push(std::mem::replace(&mut side, next));
        }
    }
    // The first corner found was pushed last; side k runs from corner k to k + 1.
    corners.rotate_right(1);
    (corners, sides)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testmesh::grid;
    use crate::trace::graph::Node;

    struct Fixture {
        cut: CutMesh,
        crosses: FaceCrosses,
        singular: Vec<i32>,
    }

    impl Fixture {
        fn new(p: &[DVec3], t: &[[u32; 3]], barrier: &[[u32; 2]]) -> Self {
            let cut = CutMesh::new(p, t, barrier);
            let crosses = FaceCrosses::new(&cut, &vec![DVec3::X; t.len()]);
            let singular = vec![0; cut.vertex_count()];
            Self { cut, crosses, singular }
        }

        fn ctx(&self) -> PatchContext<'_> {
            PatchContext {
                cut: &self.cut,
                crosses: &self.crosses,
                singular: &self.singular,
                geometric_turns: &[],
                merge_id: &[],
                max_sides: 5,
                match_valence: true,
                tolerance: 1.0,
            }
        }
    }

    /// Path edges along `vertices`, travelling along `d`.
    fn path_edges(vertices: &[u32], d: DVec3) -> PathEdges {
        let mut paths = PathEdges::new();
        for w in vertices.windows(2) {
            paths.insert_edge(w[0], w[1], d);
        }
        paths
    }

    #[test]
    fn square_patch_has_four_convex_corners() {
        let (p, t) = grid(4, 4);
        let fx = Fixture::new(&p, &t, &[]);
        let part = Partition::new(&fx.ctx(), &PathEdges::new());
        let infos: Vec<&PatchInfo> = part.patches().map(|(_, i)| i).collect();
        assert_eq!(infos.len(), 1);
        let info = infos[0];
        let mut corners = info.corners.clone();
        corners.sort_unstable();
        assert_eq!(corners, vec![0, 4, 20, 24]);
        assert_eq!(info.sides.len(), 4);
        for s in &info.sides {
            assert!((s.length - 4.0).abs() < 1e-9, "{s:?}");
            assert_eq!(s.vertices.len(), 5);
        }
        assert_eq!(info.kind, PatchKind::Ok);
    }

    #[test]
    fn corners_run_counter_clockwise() {
        let (p, t) = grid(4, 4);
        let fx = Fixture::new(&p, &t, &[]);
        let part = Partition::new(&fx.ctx(), &PathEdges::new());
        let info = part.info(0).unwrap();
        let k = info.corners.iter().position(|&c| c == 0).unwrap();
        let ordered: Vec<u32> = (0..4).map(|i| info.corners[(k + i) % 4]).collect();
        assert_eq!(ordered, vec![0, 4, 24, 20]);
    }

    #[test]
    fn l_shape_is_not_convex() {
        let (p, t) = grid(4, 4);
        let t: Vec<[u32; 3]> = t
            .into_iter()
            .filter(|tri| !tri.iter().all(|&v| p[v as usize].x >= 2.0 && p[v as usize].y >= 2.0))
            .collect();
        let fx = Fixture::new(&p, &t, &[]);
        let part = Partition::new(&fx.ctx(), &PathEdges::new());
        let info = part.info(0).unwrap();
        assert_eq!(info.kind, PatchKind::NotConvex);
        assert_eq!(info.not_convex, vec![(12, VertexClass::Concave)]);
    }

    #[test]
    fn annulus_is_non_disk() {
        let (p, t) = grid(6, 6);
        let hole = |q: DVec3| (2.0..=4.0).contains(&q.x) && (2.0..=4.0).contains(&q.y);
        let t: Vec<[u32; 3]> =
            t.into_iter().filter(|tri| !tri.iter().all(|&v| hole(p[v as usize]))).collect();
        let fx = Fixture::new(&p, &t, &[]);
        let part = Partition::new(&fx.ctx(), &PathEdges::new());
        let info = part.info(0).unwrap();
        assert_eq!(info.loops, 2);
        assert_eq!(info.kind, PatchKind::NonDisk);
    }

    #[test]
    fn a_path_splits_a_square_into_two_quads() {
        let (p, t) = grid(4, 4);
        let fx = Fixture::new(&p, &t, &[]);
        let row: Vec<u32> = (0..=4).map(|i| 2 * 5 + i).collect();
        let part = Partition::new(&fx.ctx(), &path_edges(&row, DVec3::X));
        let infos: Vec<&PatchInfo> = part.patches().map(|(_, i)| i).collect();
        assert_eq!(infos.len(), 2);
        for info in infos {
            assert_eq!(info.corners.len(), 4, "{info:?}");
            assert_eq!(info.kind, PatchKind::Ok);
        }
    }

    #[test]
    fn side_length_uses_field_projection() {
        // A path along +x with one jag: y = 2 for x ≤ 2, then y = 3.
        let (p, t) = grid(6, 6);
        let id = |i: u32, j: u32| j * 7 + i;
        let jagged = [id(0, 2), id(1, 2), id(2, 2), id(2, 3), id(3, 3), id(4, 3), id(5, 3), id(6, 3)];
        let fx = Fixture::new(&p, &t, &[]);
        let part = Partition::new(&fx.ctx(), &path_edges(&jagged, DVec3::X));
        let below = part.info(part.patch_of_face()[0]).unwrap();
        assert_eq!(below.corners.len(), 4, "the jag is not a corner: {below:?}");
        let top = below.sides.iter().find(|s| s.vertices.contains(&id(2, 3))).unwrap();
        assert!((top.length - 6.0).abs() < 1e-9, "field length {}", top.length);
    }

    #[test]
    fn tarini_subsides_match_the_paper() {
        let s = subside_counts(&[6.0, 4.0, 3.0, 5.0, 4.0]).unwrap();
        assert_eq!(s, vec![3.0, 2.0, 1.0, 1.0, 4.0]);
        let e = [6.0, 4.0, 3.0, 5.0, 4.0];
        for i in 0..5 {
            assert_eq!(e[i], s[(i + 4) % 5] + s[(i + 1) % 5]);
        }
        assert_eq!(subside_counts(&[3.0, 4.0, 5.0]).unwrap(), vec![3.0, 2.0, 1.0]);
        assert_eq!(subside_counts(&[2.0; 6]).unwrap(), vec![1.0; 6]);
        assert!(subside_counts(&[1.0; 4]).is_none());
    }

    #[test]
    fn cc_margin_rejects_a_needle_triangle() {
        assert!(cc_margin(&[1.0, 1.0, 5.0]) < 0.0);
        assert!(cc_margin(&[3.0, 4.0, 5.0]) > 0.0);
        assert_eq!(cc_margin(&[2.0, 3.0, 2.5, 3.0]), -0.5);
    }

    #[test]
    fn valence_match_rules() {
        assert_eq!(expected_sides(&[]), Some(4));
        assert_eq!(expected_sides(&[(7, -1)]), Some(5));
        assert_eq!(expected_sides(&[(7, 1)]), Some(3));
        assert_eq!(expected_sides(&[(7, 1), (9, 1)]), None);
    }

    #[test]
    fn singular_vertex_inside_a_square_is_a_mismatch() {
        let (p, t) = grid(4, 4);
        let mut fx = Fixture::new(&p, &t, &[]);
        fx.singular[12] = -1;
        let part = Partition::new(&fx.ctx(), &PathEdges::new());
        assert_eq!(part.info(0).unwrap().kind, PatchKind::ValenceMismatch);
    }

    #[test]
    fn a_patch_glued_to_itself_along_a_feature_is_non_disk() {
        // An annulus whose two borders are joined by a feature line: a disk
        // once cut open along it, but not in the final mesh.
        let (p, t) = grid(6, 6);
        let hole = |q: DVec3| (2.0..=4.0).contains(&q.x) && (2.0..=4.0).contains(&q.y);
        let t: Vec<[u32; 3]> =
            t.into_iter().filter(|tri| !tri.iter().all(|&v| hole(p[v as usize]))).collect();
        let id = |i: u32, j: u32| j * 7 + i;
        let feature = [[id(0, 3), id(1, 3)], [id(1, 3), id(2, 3)]];
        let fx = Fixture::new(&p, &t, &feature);
        let merge = fx.cut.original.clone();
        let ctx = PatchContext { merge_id: &merge, ..fx.ctx() };
        let part = Partition::new(&ctx, &PathEdges::new());
        let info = part.info(0).unwrap();
        assert!(info.cut_disk, "a disk cut open");
        assert!(info.glued > 0);
        assert_eq!(info.kind, PatchKind::NonDisk);
    }

    #[test]
    fn redeclaring_corners_fixes_a_lens() {
        let (p, t) = grid(4, 4);
        let fx = Fixture::new(&p, &t, &[]);
        let ctx = fx.ctx();
        let step = |vertex: u32, class: VertexClass| BoundaryStep {
            vertex,
            class,
            angle: if class.is_corner() { 0.5 } else { std::f64::consts::PI },
            length: 1.0,
        };
        let steps: Vec<BoundaryStep> = (0..8)
            .map(|k| step(k, if k % 4 == 0 { VertexClass::Convex } else { VertexClass::Flat }))
            .collect();
        let (corners, sides) = sides_between(&steps, &|k| steps[k].class.is_corner());
        let mut info = PatchInfo {
            faces: vec![0],
            loops: 1,
            loop_vertices: vec![(0..8).collect()],
            corners,
            sides,
            not_convex: Vec::new(),
            singularities: Vec::new(),
            kind: PatchKind::LowSides,
            cc_margin: 0.0,
            cc_able: false,
            steps,
            cut_disk: true,
            excess: 0,
            merged_excess: 0,
            glued: 0,
        };
        assert!(redeclare_corners(&mut info, &ctx));
        assert_eq!(info.corners.len(), 3);
        assert!(
            info.corners.contains(&2) || info.corners.contains(&6),
            "the middle of a side: {:?}",
            info.corners
        );
        assert!(info.kind.is_valid(), "{:?}", info.kind);
    }

    #[test]
    fn a_cusp_is_a_corner() {
        assert!(VertexClass::Cusp.is_corner() && VertexClass::Cusp.is_convex());
        assert!(!VertexClass::Flat.is_corner() && VertexClass::Flat.is_convex());
        assert!(!VertexClass::Concave.is_convex() && !VertexClass::Narrow.is_convex());
    }

    fn canonical(part: &Partition) -> Vec<(Vec<u32>, PatchKind)> {
        let mut v: Vec<(Vec<u32>, PatchKind)> =
            part.patches().map(|(_, i)| (i.faces.clone(), i.kind)).collect();
        v.sort();
        v
    }

    proptest::proptest! {
        #[test]
        fn incremental_update_matches_rebuild(ops in proptest::collection::vec((0u32..2, 1u32..12), 1..12)) {
            let (p, t) = grid(12, 12);
            let fx = Fixture::new(&p, &t, &[]);
            let ctx = fx.ctx();
            let id = |i: u32, j: u32| j * 13 + i;
            let line = |(axis, k): (u32, u32)| -> Vec<u32> {
                (0..=12).map(|s| if axis == 0 { id(s, k) } else { id(k, s) }).collect()
            };
            let mut paths = PathEdges::new();
            let mut part = Partition::new(&ctx, &paths);
            let mut present: Vec<(u32, u32)> = Vec::new();
            for op in ops {
                let vertices = line(op);
                let d = if op.0 == 0 { DVec3::X } else { DVec3::Y };
                let nodes = vertices.iter().map(|&v| Node::new(v, if op.0 == 0 { 0 } else { 1 })).collect();
                let path = TracePath { nodes, closed: false, cost: 0.0 };
                let touched: Vec<u32> = vertices
                    .windows(2)
                    .flat_map(|w| [fx.cut.half_edge(w[0], w[1]), fx.cut.half_edge(w[1], w[0])])
                    .flatten()
                    .map(|h| h / 3)
                    .collect();
                if let Some(k) = present.iter().position(|&x| x == op) {
                    present.remove(k);
                    paths.remove(&path, &fx.cut);
                } else {
                    present.push(op);
                    for w in vertices.windows(2) {
                        paths.insert_edge(w[0], w[1], d);
                    }
                }
                part.update(&ctx, &paths, &touched);
                let rebuilt = Partition::new(&ctx, &paths);
                proptest::prop_assert_eq!(canonical(&part), canonical(&rebuilt));
            }
        }
    }
}
