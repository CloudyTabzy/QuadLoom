//! Segments: every patch side cut wherever the layout's boundary branches.
//!
//! A corner of one patch can sit in the middle of a neighbour's side (a
//! T-junction), so sides are cut at every layout corner. They are also cut at
//! every vertex where three or more boundary lines meet, corner or not: the
//! layout can re-declare a corner away, and then a side runs round the bend at
//! a junction that is no patch's corner. Left uncut, the two patches on either
//! side of the line that ends there get pieces that start with the same edge
//! and then part ways. The pieces, segments, are each shared by at most two
//! patches and carry one integer quad count.

use hashbrown::{HashMap, HashSet};

use crate::trace::{PatchKind, TraceOutput};

/// One use of a segment by a patch side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SegmentUse {
    pub patch: u32,
    pub side: u8,
    /// Whether the side runs against the segment's canonical vertex order.
    pub reversed: bool,
}

/// A chain of mesh vertices from one layout corner to the next.
#[derive(Clone, Debug)]
pub struct Segment {
    /// Mesh vertices in canonical order.
    pub vertices: Vec<u32>,
    pub uses: Vec<SegmentUse>,
    /// Sampled at its own mesh vertices: it borders a patch that is kept as
    /// triangles, whose edges must line up with the quads.
    pub polyline: bool,
}

/// A patch's sides, each as its `(segment, reversed)` pieces in side order.
pub type PatchSides = Vec<Vec<(u32, bool)>>;

/// The segments of a layout and, per patch, its sides as segment lists.
#[derive(Clone, Debug, Default)]
pub struct Segments {
    pub segments: Vec<Segment>,
    /// Per patch: `None` if the patch is kept as triangles, else per side the
    /// `(segment, reversed)` pieces in side order.
    pub sides: Vec<Option<PatchSides>>,
    /// Per segment: it joins the same two vertices as a shorter segment, so
    /// it needs at least two quads (see [`Segments::least`]). Empty: none do.
    pub twins: Vec<bool>,
}

impl Segments {
    /// The fewest quads segment `s` can take. One quad makes a segment the
    /// straight edge between its ends, so of several segments joining the
    /// same two vertices only one can have one quad: the others would be
    /// that same edge again, in more than two faces.
    pub fn least(&self, s: usize) -> u32 {
        if self.twins.get(s).copied().unwrap_or(false) { 2 } else { 1 }
    }

    /// Marks every segment that joins the same two vertices as another,
    /// except the shortest of each such group (fewest vertices, then first).
    /// A polyline segment is sampled at its own vertices and never a twin.
    pub(crate) fn mark_twins(&mut self) {
        let mut shortest: HashMap<(u32, u32), usize> = HashMap::new();
        let ends = |segment: &Segment| -> Option<(u32, u32)> {
            let (&a, &b) = (segment.vertices.first()?, segment.vertices.last()?);
            (!segment.polyline && a != b).then_some((a.min(b), a.max(b)))
        };
        for (s, segment) in self.segments.iter().enumerate() {
            let Some(key) = ends(segment) else { continue };
            let best = shortest.entry(key).or_insert(s);
            if segment.vertices.len() < self.segments[*best].vertices.len() {
                *best = s;
            }
        }
        self.twins = self
            .segments
            .iter()
            .enumerate()
            .map(|(s, segment)| ends(segment).is_some_and(|key| shortest.get(&key) != Some(&s)))
            .collect();
    }
}

/// Whether a patch can be quadrangulated: a disk with 3–6 sides that join up
/// into one simple boundary loop.
pub fn usable(trace: &TraceOutput, p: usize) -> bool {
    let Some(patch) = trace.patches.get(p) else { return false };
    if matches!(patch.kind, PatchKind::NonDisk | PatchKind::LowSides) {
        return false;
    }
    let n = patch.sides.len();
    if !(3..=6).contains(&n) || patch.corners.len() != n {
        return false;
    }
    let mut seen = HashSet::new();
    for (i, side) in patch.sides.iter().enumerate() {
        let (Some(&first), Some(&last)) = (side.vertices.first(), side.vertices.last()) else { return false };
        if side.vertices.len() < 2 || first != patch.corners[i] || last != patch.corners[(i + 1) % n] {
            return false;
        }
        // Each boundary vertex once (a side's last vertex is the next side's first).
        if side.vertices[..side.vertices.len() - 1].iter().any(|&v| !seen.insert(v)) {
            return false;
        }
    }
    true
}

/// The vertices every side is cut at: the corners of all patches, and every
/// vertex where the layout's boundary lines don't simply pass through (three
/// or more meet, or one ends).
fn cuts(trace: &TraceOutput) -> HashSet<u32> {
    let mut cuts: HashSet<u32> = trace.patches.iter().flat_map(|p| p.corners.iter().copied()).collect();
    let lines: HashSet<(u32, u32)> = trace
        .patches
        .iter()
        .flat_map(|p| p.sides.iter())
        .flat_map(|side| side.vertices.windows(2).map(|w| (w[0].min(w[1]), w[0].max(w[1]))))
        .collect();
    let mut meeting: HashMap<u32, u32> = HashMap::new();
    for &(a, b) in &lines {
        *meeting.entry(a).or_default() += 1;
        *meeting.entry(b).or_default() += 1;
    }
    cuts.extend(meeting.into_iter().filter(|&(_, lines)| lines != 2).map(|(v, _)| v));
    cuts
}

/// A side's pieces between cuts, each as its canonical key, whether the side
/// runs against the canonical order, and its vertices in side order.
fn pieces<'a>(side: &'a [u32], cuts: &HashSet<u32>) -> Vec<((u32, u32), bool, &'a [u32])> {
    let mut pieces = Vec::new();
    let mut start = 0;
    for k in 1..side.len() {
        if k + 1 == side.len() || cuts.contains(&side[k]) {
            let piece = &side[start..=k];
            let (key, reversed) = canonical(piece);
            pieces.push((key, reversed, piece));
            start = k;
        }
    }
    pieces
}

/// Cuts the sides of every usable patch into segments. Patches marked in
/// `excluded` are treated as unusable (kept as triangles).
pub fn build(trace: &TraceOutput, excluded: &[bool]) -> Segments {
    let cuts = cuts(trace);
    let wanted = |p: usize| usable(trace, p) && !excluded.get(p).copied().unwrap_or(false);
    let mut segments: Vec<Segment> = Vec::new();
    let mut by_key: HashMap<(u32, u32), u32> = HashMap::new();
    let mut sides = Vec::with_capacity(trace.patches.len());
    for (p, patch) in trace.patches.iter().enumerate() {
        if !wanted(p) {
            sides.push(None);
            continue;
        }
        let mut patch_sides = Vec::with_capacity(patch.sides.len());
        for (i, side) in patch.sides.iter().enumerate() {
            let mut list = Vec::new();
            for (key, reversed, piece) in pieces(&side.vertices, &cuts) {
                let id = *by_key.entry(key).or_insert_with(|| {
                    let mut vertices = piece.to_vec();
                    if reversed {
                        vertices.reverse();
                    }
                    segments.push(Segment { vertices, uses: Vec::new(), polyline: false });
                    (segments.len() - 1) as u32
                });
                if let Some(segment) = segments.get_mut(id as usize) {
                    segment.uses.push(SegmentUse { patch: p as u32, side: i as u8, reversed });
                }
                list.push((id, reversed));
            }
            patch_sides.push(list);
        }
        sides.push(Some(patch_sides));
    }
    mark_polylines(trace, &mut segments, &sides);
    let mut built = Segments { segments, sides, twins: Vec::new() };
    built.mark_twins();
    built
}

/// A segment's key (its smaller end edge, unordered) and whether `piece` runs
/// against the canonical order (the one that starts with that edge).
fn canonical(piece: &[u32]) -> ((u32, u32), bool) {
    let edge = |a: u32, b: u32| (a.min(b), a.max(b));
    let m = piece.len() - 1;
    let first = edge(piece[0], piece[1]);
    let last = edge(piece[m - 1], piece[m]);
    if first < last {
        (first, false)
    } else if last < first {
        (last, true)
    } else {
        // A single edge: canonical from the lower vertex.
        (first, piece[0] > piece[1])
    }
}

/// Segments with one use that border a patch kept as triangles are sampled at
/// their mesh vertices, so the triangles and quads share every vertex.
fn mark_polylines(trace: &TraceOutput, segments: &mut [Segment], sides: &[Option<PatchSides>]) {
    let mut face_of: HashMap<(u32, u32), u32> = HashMap::with_capacity(3 * trace.mesh.triangles.len());
    for (f, t) in trace.mesh.triangles.iter().enumerate() {
        for i in 0..3 {
            face_of.insert((t[i], t[(i + 1) % 3]), f as u32);
        }
    }
    for segment in segments.iter_mut() {
        if segment.uses.len() != 1 || segment.vertices.len() < 2 {
            continue;
        }
        let (a, b) = (segment.vertices[0], segment.vertices[1]);
        let owner = segment.uses[0].patch;
        // The neighbour across the first edge, whichever way the owner runs it.
        let across = [face_of.get(&(a, b)), face_of.get(&(b, a))]
            .into_iter()
            .flatten()
            .filter_map(|&f| trace.patch_of_face.get(f as usize).copied())
            .find(|&q| q != owner);
        if let Some(q) = across
            && sides.get(q as usize).is_some_and(|s| s.is_none())
        {
            segment.polyline = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trace::{Patch, Side, TraceMesh};
    use glam::DVec3;

    /// A 2 × 2 grid of squares, vertices numbered row by row from the bottom:
    ///
    /// ```text
    /// 6 7 8
    /// 3 4 5
    /// 0 1 2
    /// ```
    ///
    /// The top row is one patch; the bottom squares are a patch each, the
    /// line between them (1–4) ending on the top patch's side at 4.
    fn tee(corner_at_the_junction: bool) -> TraceOutput {
        let positions = (0..9).map(|i| DVec3::new(f64::from(i % 3), f64::from(i / 3), 0.0)).collect();
        let square = |a: u32, b: u32, c: u32, d: u32| [[a, b, c], [a, c, d]];
        let triangles: Vec<[u32; 3]> =
            [square(0, 1, 4, 3), square(1, 2, 5, 4), square(3, 4, 7, 6), square(4, 5, 8, 7)].concat();
        let side =
            |vertices: &[u32]| Side { vertices: vertices.to_vec(), length: (vertices.len() - 1) as f64 };
        let patch = |corners: &[u32], sides: &[&[u32]]| Patch {
            corners: corners.to_vec(),
            sides: sides.iter().map(|s| side(s)).collect(),
            singularities: Vec::new(),
            kind: PatchKind::Ok,
            cc_margin: 0.0,
        };
        let (left, right) = if corner_at_the_junction {
            (
                patch(&[0, 1, 4, 3], &[&[0, 1], &[1, 4], &[4, 3], &[3, 0]]),
                patch(&[1, 2, 5, 4], &[&[1, 2], &[2, 5], &[5, 4], &[4, 1]]),
            )
        } else {
            // The junction is no patch's corner: each bottom patch has a side
            // that runs round the bend at 4.
            (
                patch(&[0, 1, 3], &[&[0, 1], &[1, 4, 3], &[3, 0]]),
                patch(&[1, 2, 5], &[&[1, 2], &[2, 5], &[5, 4, 1]]),
            )
        };
        let top = patch(&[3, 5, 8, 6], &[&[3, 4, 5], &[5, 8], &[8, 7, 6], &[6, 3]]);
        TraceOutput {
            mesh: TraceMesh { positions, triangles, ..TraceMesh::default() },
            patch_of_face: vec![0, 0, 1, 1, 2, 2, 2, 2],
            patches: vec![left, right, top],
            ..TraceOutput::default()
        }
    }

    type Edges = Vec<(u32, u32)>;

    /// The undirected mesh edges of a patch's segments, and of its sides.
    fn edges(trace: &TraceOutput, segments: &Segments, p: usize) -> (Edges, Edges) {
        let key = |w: &[u32]| (w[0].min(w[1]), w[0].max(w[1]));
        let mut from_segments: Vec<(u32, u32)> = segments.sides[p]
            .iter()
            .flatten()
            .flatten()
            .flat_map(|&(s, _)| segments.segments[s as usize].vertices.windows(2).map(key))
            .collect();
        let mut from_sides: Vec<(u32, u32)> =
            trace.patches[p].sides.iter().flat_map(|s| s.vertices.windows(2).map(key)).collect();
        from_segments.sort_unstable();
        from_sides.sort_unstable();
        (from_segments, from_sides)
    }

    #[test]
    fn of_segments_joining_the_same_two_vertices_only_the_shortest_may_take_one_quad() {
        // Two lines from vertex 0 to vertex 2 around a small lens. With one
        // quad each, both became the edge (0, 2): an edge in four faces.
        let segment = |vertices: &[u32], polyline: bool| Segment {
            vertices: vertices.to_vec(),
            uses: Vec::new(),
            polyline,
        };
        let mut segments = Segments {
            segments: vec![
                segment(&[0, 5, 6, 2], false),
                segment(&[2, 1, 0], false),
                segment(&[0, 3], false),
                segment(&[2, 7, 0], true),
                segment(&[4, 4], false),
            ],
            ..Segments::default()
        };
        assert_eq!((0..5).map(|s| segments.least(s)).collect::<Vec<_>>(), [1; 5], "nothing marked yet");
        segments.mark_twins();
        assert_eq!(segments.twins, [true, false, false, false, false]);
        assert_eq!((0..5).map(|s| segments.least(s)).collect::<Vec<_>>(), [2, 1, 1, 1, 1]);
    }

    #[test]
    fn a_junction_that_is_no_corner_still_cuts_the_sides() {
        // Without the cut, the two bottom patches' bent sides both start with
        // edge (1, 4) and were taken for one segment: the second patch got
        // the first one's boundary.
        for corner_at_the_junction in [true, false] {
            let trace = tee(corner_at_the_junction);
            let segments = build(&trace, &[]);
            for p in 0..3 {
                assert!(segments.sides[p].is_some(), "patch {p} is usable");
                let (from_segments, from_sides) = edges(&trace, &segments, p);
                assert_eq!(
                    from_segments, from_sides,
                    "patch {p}, corner at the junction: {corner_at_the_junction}"
                );
            }
            // The three lines into the junction are shared by two patches
            // each; the seven outer runs (the top one has a vertex midway) are
            // used once.
            for segment in &segments.segments {
                let inner = segment.vertices.contains(&4);
                assert_eq!(segment.uses.len(), if inner { 2 } else { 1 }, "{:?}", segment.vertices);
                assert_eq!(segment.vertices.len(), if segment.vertices.contains(&7) { 3 } else { 2 });
            }
            assert_eq!(segments.segments.len(), 10);
        }
    }
}
