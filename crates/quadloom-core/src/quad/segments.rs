//! Segments: every patch side cut at every corner of the layout.
//!
//! A corner of one patch can sit in the middle of a neighbour's side (a
//! T-junction), so sides are cut at every layout corner. The pieces, segments,
//! are each shared by at most two patches and carry one integer quad count.

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

/// Cuts the sides of every usable patch into segments. Patches marked in
/// `excluded` are treated as unusable (kept as triangles).
pub fn build(trace: &TraceOutput, excluded: &[bool]) -> Segments {
    let corners: HashSet<u32> = trace.patches.iter().flat_map(|p| p.corners.iter().copied()).collect();
    let mut segments: Vec<Segment> = Vec::new();
    let mut by_key: HashMap<(u32, u32), u32> = HashMap::new();
    let mut sides = Vec::with_capacity(trace.patches.len());
    for (p, patch) in trace.patches.iter().enumerate() {
        if !usable(trace, p) || excluded.get(p).copied().unwrap_or(false) {
            sides.push(None);
            continue;
        }
        let mut patch_sides = Vec::with_capacity(patch.sides.len());
        for (i, side) in patch.sides.iter().enumerate() {
            let mut pieces = Vec::new();
            let mut start = 0;
            for k in 1..side.vertices.len() {
                if k + 1 == side.vertices.len() || corners.contains(&side.vertices[k]) {
                    let piece = &side.vertices[start..=k];
                    let (key, reversed) = canonical(piece);
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
                    pieces.push((id, reversed));
                    start = k;
                }
            }
            patch_sides.push(pieces);
        }
        sides.push(Some(patch_sides));
    }
    mark_polylines(trace, &mut segments, &sides);
    Segments { segments, sides }
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
