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
}

impl PathEdges {
    pub fn new() -> Self {
        Self::default()
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
            self.map.insert((x, y), d);
        }
    }

    pub fn remove(&mut self, path: &TracePath) {
        for (a, b) in path.steps() {
            self.map.remove(&(a.vertex(), b.vertex()));
        }
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
}

/// What the patch conditions depend on.
#[derive(Clone, Copy, Debug)]
pub struct PatchContext<'a> {
    pub cut: &'a CutMesh,
    pub crosses: &'a FaceCrosses,
    /// Index in quarters per cut vertex (0 for regular vertices).
    pub singular: &'a [i32],
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
        let mut partition = Self { patch_of_face: vec![UNSET; ctx.cut.face_count()], infos: Vec::new() };
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
            self.infos[id as usize] = Some(describe(ctx, paths, members));
            added.push(id);
        }
        added
    }
}

fn is_cut(cut: &CutMesh, paths: &PathEdges, h: u32) -> bool {
    cut.twin(h).is_none() || paths.contains(cut.tail(h), cut.head(h))
}

/// The loops, corners, sides, singularities and kind of the patch made of `faces`.
fn describe(ctx: &PatchContext, paths: &PathEdges, faces: Vec<u32>) -> PatchInfo {
    let cut = ctx.cut;
    let cut_at = |h: u32| is_cut(cut, paths, h);
    let label = |h: u32| {
        let (a, b) = (cut.tail(h), cut.head(h));
        paths.label(a, b).unwrap_or(cut.positions[b as usize] - cut.positions[a as usize])
    };
    let boundary: Vec<u32> = faces.iter().flat_map(|&f| 3 * f..3 * f + 3).filter(|&h| cut_at(h)).collect();
    let mut vertices: Vec<u32> = faces.iter().flat_map(|&f| cut.triangles[f as usize]).collect();
    vertices.sort_unstable();
    vertices.dedup();
    let mut on_boundary: Vec<u32> = boundary.iter().map(|&h| cut.tail(h)).collect();
    on_boundary.sort_unstable();
    on_boundary.dedup();
    let interior = vertices.len() - on_boundary.len();
    // χ = V − E + F of the patch cut along its boundary (see the module notes).
    let chi2 = 2 * interior as i64 + boundary.len() as i64 - faces.len() as i64;

    // Walk the loops; each step is (half-edge, class at its head).
    let mut seen: HashMap<u32, ()> = HashMap::with_capacity(boundary.len());
    let mut loops: Vec<Vec<(u32, VertexClass)>> = Vec::new();
    let mut broken = false;
    for &start in &boundary {
        if seen.contains_key(&start) {
            continue;
        }
        let mut steps = Vec::new();
        let mut h = start;
        loop {
            seen.insert(h, ());
            let turn = turn_at(cut, ctx.crosses, h, &cut_at, &label);
            steps.push((h, turn.class));
            h = turn.outgoing;
            if h == start {
                break;
            }
            if seen.contains_key(&h) || steps.len() > boundary.len() {
                broken = true;
                break;
            }
        }
        loops.push(steps);
    }
    let not_convex: Vec<(u32, VertexClass)> =
        loops.iter().flatten().filter(|(_, c)| !c.is_convex()).map(|&(h, c)| (cut.head(h), c)).collect();
    let (corners, sides) = match loops.as_slice() {
        [steps] => corners_and_sides(ctx, &label, steps),
        _ => (Vec::new(), Vec::new()),
    };
    let mut singularities: Vec<(u32, i32)> = vertices
        .iter()
        .filter(|v| on_boundary.binary_search(v).is_err())
        .filter_map(|&v| ctx.singular.get(v as usize).filter(|&&k| k != 0).map(|&k| (v, k)))
        .collect();
    singularities.sort_unstable();
    let lengths: Vec<f64> = sides.iter().map(|s| s.length).collect();
    let margin = cc_margin(&lengths);
    let n = sides.len();
    let kind = if broken || loops.len() != 1 || chi2 != 2 {
        PatchKind::NonDisk
    } else if !not_convex.is_empty() {
        PatchKind::NotConvex
    } else if n < 3 {
        PatchKind::LowSides
    } else if n > ctx.max_sides {
        PatchKind::HighSides
    } else if !(n == 4 && ctx.match_valence) && {
        let shortest = lengths.iter().copied().fold(f64::INFINITY, f64::min);
        margin + shortest + ctx.tolerance <= 0.0
    } {
        PatchKind::NotCcAble
    } else if ctx.match_valence && expected_sides(&singularities) != Some(n) {
        PatchKind::ValenceMismatch
    } else {
        PatchKind::Ok
    };
    PatchInfo {
        faces,
        loops: loops.len(),
        corners,
        sides,
        not_convex,
        singularities,
        kind,
        cc_margin: margin,
    }
}

/// Corners (convex vertices) of a single loop and the sides between them.
/// A side's length sums its edges along one field direction, carried from
/// edge to edge, so the zig-zags of a traced path don't count.
fn corners_and_sides(
    ctx: &PatchContext,
    label: &dyn Fn(u32) -> DVec3,
    steps: &[(u32, VertexClass)],
) -> (Vec<u32>, Vec<SideInfo>) {
    let cut = ctx.cut;
    let Some(first) = steps.iter().position(|(_, c)| *c == VertexClass::Convex) else {
        return (Vec::new(), Vec::new());
    };
    let n = steps.len();
    let (mut corners, mut sides) = (Vec::new(), Vec::new());
    let mut side = SideInfo { vertices: vec![cut.head(steps[first].0)], length: 0.0 };
    for k in 1..=n {
        let (h, class) = steps[(first + k) % n];
        let e = cut.positions[cut.head(h) as usize] - cut.positions[cut.tail(h) as usize];
        let along = ctx.crosses.closest(h / 3, label(h)).map_or(e.length(), |(_, d)| e.dot(d).abs());
        side.length += along;
        side.vertices.push(cut.head(h));
        if class == VertexClass::Convex {
            corners.push(cut.head(h));
            let next = SideInfo { vertices: vec![cut.head(h)], length: 0.0 };
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
            paths.map.insert((w[0], w[1]), d);
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
                    paths.remove(&path);
                } else {
                    present.push(op);
                    for w in vertices.windows(2) {
                        paths.map.insert((w[0], w[1]), d);
                    }
                }
                part.update(&ctx, &paths, &touched);
                let rebuilt = Partition::new(&ctx, &paths);
                proptest::prop_assert_eq!(canonical(&part), canonical(&rebuilt));
            }
        }
    }
}
