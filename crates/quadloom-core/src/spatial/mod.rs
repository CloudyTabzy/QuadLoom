//! Spatial queries over triangle sets.
//!
//! [`TriangleBvh`] is a bounding-volume hierarchy built top-down with median
//! splits, then flattened to four children per node, so a query measures four
//! boxes at once and, at a leaf, four triangles at once (SIMD through
//! `fearless_simd`, at the best level the CPU has). Queries still visit the
//! leaves in the binary tree's order and each lane computes exactly what the
//! scalar routine would, so results are bit for bit those of a plain binary
//! BVH on any CPU. Both the build and the queries use explicit stacks, never
//! recursion.
//!
//! A triangle far longer than the rest is held as several pieces
//! ([`cut_long`]): a box round a long sliver is mostly empty, and a mesh made
//! of them is boxes that all overlap.

use fearless_simd::{Level, dispatch, f64x4, prelude::*};
pub use glam::DVec3;
use tinyvec::TinyVec;

/// Triangles per leaf: one SIMD packet.
const LEAF_SIZE: usize = 4;
/// A child that is a leaf: the rest of the bits are its packet.
const LEAF: u32 = 1 << 31;
/// An empty child slot, or a packet lane past its leaf's triangles.
const NONE: u32 = u32::MAX;

#[derive(Clone, Copy, Debug)]
struct Aabb {
    min: DVec3,
    max: DVec3,
}

impl Aabb {
    const EMPTY: Self = Self { min: DVec3::INFINITY, max: DVec3::NEG_INFINITY };

    fn grow(self, p: DVec3) -> Self {
        Self { min: self.min.min(p), max: self.max.max(p) }
    }

    #[inline(always)]
    fn distance_squared(&self, p: DVec3) -> f64 {
        let outside = (self.min - p).max(p - self.max).max(DVec3::ZERO);
        outside.length_squared()
    }
}

/// A node of the binary tree the flat one is made from.
#[derive(Clone, Copy, Debug)]
struct Binary {
    bounds: Aabb,
    /// `LEAF | packet`, or the two children.
    kind: [u32; 2],
}

/// Four children and their boxes lane by lane (`[axis][lane]`). Children
/// 0–1 and 2–3 are the two binary halves; a half's box, which decides the
/// visiting order, is exactly the union of its children's.
#[derive(Clone, Copy, Debug)]
struct Node {
    min: [[f64; 4]; 3],
    max: [[f64; 4]; 3],
    child: [u32; 4],
}

/// Four triangles lane by lane: `[corner][axis][lane]`.
#[derive(Clone, Copy, Debug)]
struct Packet([[[f64; 4]; 3]; 3]);

/// Closest-point queries against a fixed set of triangles.
#[derive(Clone, Debug)]
pub struct TriangleBvh {
    nodes: Vec<Node>,
    packets: Vec<Packet>,
    /// The input index of each packet lane (`NONE` past a leaf's triangles).
    ids: Vec<u32>,
    bounds: Option<Aabb>,
    /// The mean length of a triangle's edges: the scale a search widens by.
    spacing: f64,
    level: Level,
}

/// A filtered search looks within this many [`TriangleBvh::spacing`]s first,
/// then [`WIDER`] times as far each time, [`WIDENINGS`] times in all before
/// it looks everywhere.
const NEAR: f64 = 2.0;
const WIDER: f64 = 4.0;
const WIDENINGS: usize = 3;

impl TriangleBvh {
    pub fn new(positions: &[DVec3], triangles: &[[u32; 3]]) -> Self {
        Self::with_level(positions, triangles, Level::new())
    }

    /// A BVH whose queries run at SIMD `level`.
    fn with_level(positions: &[DVec3], triangles: &[[u32; 3]], level: Level) -> Self {
        match cut_long(positions, triangles) {
            Some(cut) => {
                // The tree is of the pieces; what a query names is the
                // triangle a piece is of.
                let mut bvh = Self::of(&cut.positions, &cut.triangles, level);
                for id in bvh.ids.iter_mut().filter(|id| **id != NONE) {
                    *id = cut.source[*id as usize];
                }
                bvh
            }
            None => Self::of(positions, triangles, level),
        }
    }

    /// The BVH of these triangles as they are.
    fn of(positions: &[DVec3], triangles: &[[u32; 3]], level: Level) -> Self {
        let (binary, packets, ids) = build_binary(positions, triangles);
        let bounds = binary.first().map(|root| root.bounds);
        let edges: f64 = triangles
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|i| positions[i as usize]);
                (a - b).length() + (b - c).length() + (c - a).length()
            })
            .sum();
        let spacing = if triangles.is_empty() { 0.0 } else { edges / (3 * triangles.len()) as f64 };
        Self { nodes: flatten(&binary), packets, ids, bounds, spacing, level }
    }

    /// The closest point on any triangle to `p`, and its distance. `None` if empty.
    pub fn closest_point(&self, p: DVec3) -> Option<(DVec3, f64)> {
        self.closest_point_where(p, |_| true)
    }

    /// The closest point to `p` on the triangles (by input index) that
    /// `accept` lets through, and its distance. `None` if none does.
    pub fn closest_point_where(&self, p: DVec3, accept: impl Fn(u32) -> bool) -> Option<(DVec3, f64)> {
        self.closest_triangle_where(p, accept).map(|(q, d, _)| (q, d))
    }

    /// [`closest_point_where`](Self::closest_point_where) that also returns
    /// the input index of the triangle the point lies on.
    ///
    /// The search widens. A search that has found nothing yet has no bound,
    /// and goes through every triangle of each subtree it enters; with a
    /// choosy `accept` (one patch's triangles out of a whole mesh, the faces
    /// turned one way) that was most of the work, ten to a hundred times a
    /// plain search's. So it looks within a couple of triangle sizes first,
    /// then four times as far, and again, and only then everywhere. What a
    /// bounded pass finds is what the unbounded one would have, down to
    /// which of two equally near triangles: the passes visit in the same
    /// order and a bound only leaves out what lies past it. A point close to
    /// what it is looking for, which is nearly every query, is answered by
    /// the first pass; a point far from everything pays three passes that
    /// end at the root.
    pub fn closest_triangle_where(
        &self,
        p: DVec3,
        accept: impl Fn(u32) -> bool,
    ) -> Option<(DVec3, f64, u32)> {
        let mut reach = NEAR * self.spacing;
        for _ in 0..WIDENINGS {
            if reach.is_nan() || reach <= 0.0 || !reach.is_finite() {
                break;
            }
            let found = dispatch!(self.level, simd => self.search(simd, p, reach * reach, &accept));
            if found.is_some() {
                return found;
            }
            reach *= WIDER;
        }
        self.closest_triangle_anywhere(p, accept)
    }

    /// [`closest_triangle_where`](Self::closest_triangle_where) in one pass
    /// with no bound: what the widening search is measured against.
    fn closest_triangle_anywhere(&self, p: DVec3, accept: impl Fn(u32) -> bool) -> Option<(DVec3, f64, u32)> {
        dispatch!(self.level, simd => self.search(simd, p, f64::INFINITY, &accept))
    }

    /// [`closest_triangle_where`](Self::closest_triangle_where) among the
    /// triangles nearer to `p` than `reach`; `None` if `accept` lets none of
    /// those through. For a caller that knows how far the answer can be
    /// (where the thing was a moment ago) and wants one pass at that reach.
    pub fn closest_triangle_within(
        &self,
        p: DVec3,
        reach: f64,
        accept: impl Fn(u32) -> bool,
    ) -> Option<(DVec3, f64, u32)> {
        if reach.is_nan() || reach <= 0.0 {
            return None;
        }
        dispatch!(self.level, simd => self.search(simd, p, reach * reach, &accept))
    }

    /// `limit` is the squared distance a triangle must be under to count.
    #[inline(always)]
    fn search<S: Simd>(
        &self,
        simd: S,
        p: DVec3,
        limit: f64,
        accept: &impl Fn(u32) -> bool,
    ) -> Option<(DVec3, f64, u32)> {
        if self.nodes.is_empty() {
            return None;
        }
        let ps = [p.x, p.y, p.z].map(|x| f64x4::splat(simd, x));
        let zero = f64x4::splat(simd, 0.0);
        let mut best = (DVec3::ZERO, limit, NONE); // (point, squared distance, triangle)
        let mut stack: TinyVec<[(u32, f64); 64]> = TinyVec::new();
        stack.push((0, 0.0));
        while let Some((child, box_d2)) = stack.pop() {
            if box_d2 >= best.1 {
                continue;
            }
            if child & LEAF != 0 {
                let packet = (child & !LEAF) as usize;
                let lanes = closest4(simd, &self.packets[packet], p);
                for (k, &id) in self.ids[4 * packet..4 * packet + 4].iter().enumerate() {
                    if id == NONE {
                        break;
                    }
                    if !accept(id) {
                        continue;
                    }
                    let (q, d2) = if lanes.degenerate & (1 << k) != 0 {
                        let [a, b, c] = self.packets[packet].corners(k);
                        let q = closest_point_on_triangle(p, a, b, c);
                        (q, (q - p).length_squared())
                    } else {
                        (DVec3::new(lanes.q[0][k], lanes.q[1][k], lanes.q[2][k]), lanes.d2[k])
                    };
                    if d2 < best.1 {
                        best = (q, d2, id);
                    }
                }
                continue;
            }
            let node = &self.nodes[child as usize];
            let mut d2 = zero;
            for ((lo, hi), p) in node.min.iter().zip(&node.max).zip(ps) {
                let (lo, hi) = (f64x4::simd_from(simd, *lo), f64x4::simd_from(simd, *hi));
                let out = (lo - p).max(p - hi).max(zero);
                d2 += out * out;
            }
            let d2 = d2.to_array();
            // The binary tree's order: the nearer half first, in each half the
            // nearer child first (so ties between equally near triangles go
            // the same way). Pushed farthest first.
            let pair = |i: usize| if d2[i] <= d2[i + 1] { [i, i + 1] } else { [i + 1, i] };
            let [left, right] = [0, 2].map(|k| node.half(k).distance_squared(p));
            let ([a, b], [c, d]) = if left <= right { (pair(0), pair(2)) } else { (pair(2), pair(0)) };
            for k in [d, c, b, a] {
                if node.child[k] != NONE && d2[k] < best.1 {
                    stack.push((node.child[k], d2[k]));
                }
            }
        }
        (best.2 != NONE).then(|| (best.0, best.1.sqrt(), best.2))
    }

    pub fn bounds(&self) -> Option<(DVec3, DVec3)> {
        self.bounds.map(|b| (b.min, b.max))
    }
}

/// Leaves of the median-split tree over `n` triangles. Each level has at
/// most two distinct part sizes, so this takes O(log n).
fn leaf_count(n: usize) -> usize {
    let (mut leaves, mut level) = (0, vec![(n, 1usize)]);
    while !level.is_empty() {
        let mut next: Vec<(usize, usize)> = Vec::new();
        for (size, count) in level {
            if size <= LEAF_SIZE {
                leaves += count;
                continue;
            }
            for half in [size / 2, size - size / 2] {
                match next.iter_mut().find(|(s, _)| *s == half) {
                    Some((_, c)) => *c += count,
                    None => next.push((half, count)),
                }
            }
        }
        level = next;
    }
    leaves
}

/// The binary tree (root first), its leaves' packets and the packets' lane
/// ids, each allocated at its final size.
/// A triangle is cut up for the tree when its longest edge is over this
/// many edges of an equilateral triangle of the mesh's mean triangle area
/// (the mesh's own size of triangle), into pieces no longer than one such
/// edge.
const LONG: f64 = 4.0;
/// Cutting adds at most this many pieces per triangle, or [`SPARE_PIECES`]
/// in all if that is more, and never over [`MOST_PIECES`]: the pieces are
/// made as long as it takes to keep to that.
const PIECES_PER_TRIANGLE: usize = 4;
const SPARE_PIECES: usize = 1 << 19;
const MOST_PIECES: usize = 1 << 21;
/// A triangle is halved at most this many times over.
const DEEPEST_CUT: u32 = 40;
/// A mesh is cut only if it is made of long triangles: if their lengths,
/// in pieces, come to this many per triangle of the mesh or more
/// (`crane/oloid`: 29; `thingi10k/1514900`: 8).
///
/// A few long triangles among compact ones cost a query nothing, and a tree
/// of pieces is a copy of the whole mesh. Most prep meshes and quad meshes
/// have a handful, and CAD exports have a tenth to a third of their
/// triangles long (1 to 2.5 by this measure): cutting those made the tree
/// up to sixteen times the mesh for no gain that showed.
const WORTH_CUTTING: f64 = 4.0;

/// Triangles cut into pieces: a soup (three positions a piece), and the
/// input triangle each piece is of.
#[derive(Debug, Default)]
struct Cut {
    positions: Vec<DVec3>,
    triangles: Vec<[u32; 3]>,
    source: Vec<u32>,
}

/// The mesh with its long triangles ([`LONG`]) cut into pieces of its own
/// size of triangle, for the tree to be built of. `None` if it has none or
/// only a few ([`WORTH_CUTTING`]), which is most meshes: they are built as
/// they are, to the bit as before.
///
/// A query costs what it has to look into, and it looks into every box that
/// reaches nearer than the best found so far. Round a compact triangle the
/// box is the triangle, near enough. Round a sliver that runs across the
/// model it is a slab of empty space, and a surface made of such slivers (a
/// ruled surface triangulated along its rulings, the long flats of a CAD
/// export) is boxes that all overlap: every query went through thousands of
/// triangles, 31 microseconds where a mesh of compact triangles takes one.
/// Cut into pieces, each piece has a box its own size. The points of the
/// surface are the same points, so a query finds what it found. (The idea
/// is early split clipping, Ernst and Greiner 2007, done before the build.)
fn cut_long(positions: &[DVec3], triangles: &[[u32; 3]]) -> Option<Cut> {
    let corners = |t: &[u32; 3]| t.map(|i| positions[i as usize]);
    let area: f64 = triangles
        .iter()
        .map(|t| {
            let [a, b, c] = corners(t);
            0.5 * (b - a).cross(c - a).length()
        })
        .filter(|area| area.is_finite())
        .sum();
    // The edge of an equilateral triangle of the mean area.
    let mut even = (4.0 / 3f64.sqrt() * area / triangles.len().max(1) as f64).sqrt();
    if !(even.is_finite() && even > 0.0) {
        return None;
    }
    let most = triangles.len() + (PIECES_PER_TRIANGLE * triangles.len()).clamp(SPARE_PIECES, MOST_PIECES);
    loop {
        let long = LONG * even;
        // What cutting would add, roughly: each long triangle's length in pieces.
        let added: f64 = triangles
            .iter()
            .map(|t| longest_edge(corners(t)).1.sqrt())
            .filter(|&length| length > long)
            .map(|length| length / even)
            .sum();
        if added < WORTH_CUTTING * triangles.len() as f64 {
            return None;
        }
        if let Some(cut) = cut_at(positions, triangles, long, even, most) {
            return Some(cut);
        }
        // Too many pieces: longer ones, of fewer triangles.
        even *= 2.0;
    }
}

/// Which edge of a triangle is the longest (0: a to b, 1: b to c, 2: c to
/// a; the first of equals), and its squared length.
fn longest_edge([a, b, c]: [DVec3; 3]) -> (usize, f64) {
    let lengths = [(a - b).length_squared(), (b - c).length_squared(), (c - a).length_squared()];
    let mut k = 0;
    for (i, &length) in lengths.iter().enumerate().skip(1) {
        if length > lengths[k] {
            k = i;
        }
    }
    (k, lengths[k])
}

/// [`cut_long`] at one size: every triangle with an edge over `long`
/// halved across its longest edge, and its halves again, until none is
/// over `piece`. `None` if that takes more than `most` pieces.
fn cut_at(positions: &[DVec3], triangles: &[[u32; 3]], long: f64, piece: f64, most: usize) -> Option<Cut> {
    let over = |length_squared: f64, limit: f64| {
        length_squared.partial_cmp(&(limit * limit)) == Some(std::cmp::Ordering::Greater)
    };
    let mut cut = Cut::default();
    let mut stack: Vec<([DVec3; 3], u32)> = Vec::new();
    for (t, triangle) in triangles.iter().enumerate() {
        stack.push((triangle.map(|i| positions[i as usize]), 0));
        while let Some((piece_of, depth)) = stack.pop() {
            let (k, longest) = longest_edge(piece_of);
            // A triangle is cut at all only if it is long; once cut, down to the piece size.
            let limit = if depth == 0 { long } else { piece };
            if depth >= DEEPEST_CUT || !over(longest, limit) {
                if cut.source.len() >= most {
                    return None;
                }
                let first = cut.positions.len() as u32;
                cut.positions.extend(piece_of);
                cut.triangles.push([first, first + 1, first + 2]);
                cut.source.push(t as u32);
                continue;
            }
            // Turned so that the longest edge is p to q: both halves keep
            // the triangle's winding.
            let [p, q, r] = [piece_of[k], piece_of[(k + 1) % 3], piece_of[(k + 2) % 3]];
            let middle = 0.5 * (p + q);
            stack.push(([middle, q, r], depth + 1));
            stack.push(([p, middle, r], depth + 1));
        }
    }
    Some(cut)
}

fn build_binary(positions: &[DVec3], triangles: &[[u32; 3]]) -> (Vec<Binary>, Vec<Packet>, Vec<u32>) {
    if triangles.is_empty() {
        return (Vec::new(), Vec::new(), Vec::new());
    }
    let corners = |t: u32| triangles[t as usize].map(|i| positions[i as usize]);
    let leaves = leaf_count(triangles.len());
    let (mut packets, mut ids) = (Vec::with_capacity(leaves), Vec::with_capacity(LEAF_SIZE * leaves));
    let centroids: Vec<DVec3> = (0..triangles.len() as u32)
        .map(|t| {
            let [a, b, c] = corners(t);
            (a + b + c) / 3.0
        })
        .collect();
    let mut order: Vec<u32> = (0..triangles.len() as u32).collect();
    let mut nodes = Vec::with_capacity(2 * leaves - 1);
    nodes.push(Binary { bounds: Aabb::EMPTY, kind: [NONE; 2] });
    let mut work = vec![(0usize, 0usize, order.len())];
    while let Some((node, start, end)) = work.pop() {
        let slice = &mut order[start..end];
        let bounds =
            slice.iter().fold(Aabb::EMPTY, |acc, &t| corners(t).into_iter().fold(acc, |acc, p| acc.grow(p)));
        if slice.len() <= LEAF_SIZE {
            let mut packet = Packet([[[0.0; 4]; 3]; 3]);
            for lane in 0..LEAF_SIZE {
                // Spare lanes repeat the last triangle; their ids say to skip them.
                let t = slice[lane.min(slice.len() - 1)];
                for (corner, p) in corners(t).iter().enumerate() {
                    for axis in 0..3 {
                        packet.0[corner][axis][lane] = p[axis];
                    }
                }
                ids.push(if lane < slice.len() { t } else { NONE });
            }
            nodes[node] = Binary { bounds, kind: [LEAF | packets.len() as u32, NONE] };
            packets.push(packet);
            continue;
        }
        let spread = slice.iter().fold(Aabb::EMPTY, |acc, &t| acc.grow(centroids[t as usize]));
        let extent = spread.max - spread.min;
        let axis = if extent.x >= extent.y && extent.x >= extent.z {
            0
        } else if extent.y >= extent.z {
            1
        } else {
            2
        };
        let mid = slice.len() / 2;
        slice.select_nth_unstable_by(mid, |&a, &b| {
            centroids[a as usize][axis].total_cmp(&centroids[b as usize][axis]).then(a.cmp(&b))
        });
        let (left, right) = (nodes.len(), nodes.len() + 1);
        nodes.extend([Binary { bounds: Aabb::EMPTY, kind: [NONE; 2] }; 2]);
        nodes[node] = Binary { bounds, kind: [left as u32, right as u32] };
        work.push((left, start, start + mid));
        work.push((right, start + mid, end));
    }
    (nodes, packets, ids)
}

/// Four-wide nodes, two binary levels each: a node's children are the binary
/// node's grandchildren (or a child itself where that is a leaf).
fn flatten(binary: &[Binary]) -> Vec<Node> {
    if binary.is_empty() {
        return Vec::new();
    }
    let is_leaf = |x: u32| binary[x as usize].kind[0] & LEAF != 0;
    // One node per inner binary node at an even depth (the root always).
    let (mut count, mut depths) = (0, vec![(0u32, 0u32)]);
    while let Some((x, depth)) = depths.pop() {
        if depth % 2 == 0 && (depth == 0 || !is_leaf(x)) {
            count += 1;
        }
        if !is_leaf(x) {
            depths.extend(binary[x as usize].kind.map(|y| (y, depth + 1)));
        }
    }
    let empty = Node { min: [[f64::INFINITY; 4]; 3], max: [[f64::NEG_INFINITY; 4]; 3], child: [NONE; 4] };
    let mut nodes = Vec::with_capacity(count);
    nodes.push(empty);
    let mut work = vec![(0usize, 0u32)];
    while let Some((at, x)) = work.pop() {
        let halves = if is_leaf(x) { [x, NONE] } else { binary[x as usize].kind };
        let mut node = empty;
        for (h, &y) in halves.iter().enumerate() {
            if y == NONE {
                continue;
            }
            let slots = if is_leaf(y) { [y, NONE] } else { binary[y as usize].kind };
            for (s, &z) in slots.iter().enumerate() {
                if z == NONE {
                    continue;
                }
                let k = 2 * h + s;
                let b = binary[z as usize].bounds;
                for axis in 0..3 {
                    node.min[axis][k] = b.min[axis];
                    node.max[axis][k] = b.max[axis];
                }
                node.child[k] = if is_leaf(z) {
                    binary[z as usize].kind[0]
                } else {
                    nodes.push(empty);
                    work.push((nodes.len() - 1, z));
                    (nodes.len() - 1) as u32
                };
            }
        }
        nodes[at] = node;
    }
    nodes
}

impl Node {
    /// The box of the binary half made of children `k` and `k + 1`.
    fn half(&self, k: usize) -> Aabb {
        let lane = |b: &[[f64; 4]; 3], k: usize| DVec3::new(b[0][k], b[1][k], b[2][k]);
        Aabb {
            min: lane(&self.min, k).min(lane(&self.min, k + 1)),
            max: lane(&self.max, k).max(lane(&self.max, k + 1)),
        }
    }
}

impl Packet {
    fn corners(&self, lane: usize) -> [DVec3; 3] {
        self.0.map(|c| DVec3::new(c[0][lane], c[1][lane], c[2][lane]))
    }
}

/// [`closest4`]'s result, lane by lane.
struct Lanes {
    q: [[f64; 4]; 3],
    d2: [f64; 4],
    /// Bit per lane whose triangle has no area (left to the scalar routine).
    degenerate: u64,
}

type V<S> = [f64x4<S>; 3];

#[inline(always)]
fn sub<S: Simd>(a: V<S>, b: V<S>) -> V<S> {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

#[inline(always)]
fn dot<S: Simd>(a: V<S>, b: V<S>) -> f64x4<S> {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// `a + d * t`, axis by axis.
#[inline(always)]
fn along<S: Simd>(a: V<S>, d: V<S>, t: f64x4<S>) -> V<S> {
    [a[0] + d[0] * t, a[1] + d[1] * t, a[2] + d[2] * t]
}

#[inline(always)]
fn pick<S: Simd>(mask: <f64x4<S> as SimdBase<S>>::Mask, yes: V<S>, no: V<S>) -> V<S> {
    [mask.select(yes[0], no[0]), mask.select(yes[1], no[1]), mask.select(yes[2], no[2])]
}

/// [`closest_point_on_triangle`] for a packet's four triangles at once. Every
/// region's point is computed with the scalar routine's operations, in its
/// order, and the first region that applies is selected, so each lane is bit
/// for bit the scalar result (barring flagged degenerate lanes).
#[inline(always)]
fn closest4<S: Simd>(simd: S, packet: &Packet, p: DVec3) -> Lanes {
    let load = |corner: &[[f64; 4]; 3]| corner.map(|axis| f64x4::simd_from(simd, axis));
    let [a, b, c] = [0, 1, 2].map(|k| load(&packet.0[k]));
    let p = [p.x, p.y, p.z].map(|x| f64x4::splat(simd, x));
    let zero = f64x4::splat(simd, 0.0);
    let (ab, ac, ap) = (sub(b, a), sub(c, a), sub(p, a));
    let (d1, d2) = (dot(ab, ap), dot(ac, ap));
    let bp = sub(p, b);
    let (d3, d4) = (dot(ab, bp), dot(ac, bp));
    let vc = d1 * d4 - d3 * d2;
    let cp = sub(p, c);
    let (d5, d6) = (dot(ab, cp), dot(ac, cp));
    let vb = d5 * d2 - d1 * d6;
    let va = d3 * d6 - d5 * d4;
    let (d43, d56) = (d4 - d3, d5 - d6);

    let at_a = d1.simd_le(zero) & d2.simd_le(zero);
    let at_b = d3.simd_ge(zero) & d4.simd_le(d3);
    let on_ab = vc.simd_le(zero) & d1.simd_ge(zero) & d3.simd_le(zero);
    let at_c = d6.simd_ge(zero) & d5.simd_le(d6);
    let on_ac = vb.simd_le(zero) & d2.simd_ge(zero) & d6.simd_le(zero);
    let on_bc = va.simd_le(zero) & d43.simd_ge(zero) & d56.simd_ge(zero);

    // Lowest priority first, so the scalar routine's first match wins.
    let denom = va + vb + vc;
    let mut q = along(along(a, ab, vb / denom), ac, vc / denom);
    q = pick(on_bc, along(b, sub(c, b), d43 / (d43 + d56)), q);
    q = pick(on_ac, along(a, ac, d2 / (d2 - d6)), q);
    q = pick(at_c, c, q);
    q = pick(on_ab, along(a, ab, d1 / (d1 - d3)), q);
    q = pick(at_b, b, q);
    q = pick(at_a, a, q);

    let any = at_a | at_b | on_ab | at_c | on_ac | on_bc;
    let flat = denom.abs().simd_lt(f64x4::splat(simd, f64::MIN_POSITIVE));
    let off = sub(q, p);
    Lanes { q: q.map(|x| x.to_array()), d2: dot(off, off).to_array(), degenerate: (!any & flat).to_bitmask() }
}

/// Closest point on triangle `abc` to `p` (Ericson, *Real-Time Collision Detection*, §5.1.5).
pub fn closest_point_on_triangle(p: DVec3, a: DVec3, b: DVec3, c: DVec3) -> DVec3 {
    let (ab, ac, ap) = (b - a, c - a, p - a);
    let (d1, d2) = (ab.dot(ap), ac.dot(ap));
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = p - b;
    let (d3, d4) = (ab.dot(bp), ac.dot(bp));
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return a + ab * (d1 / (d1 - d3));
    }
    let cp = p - c;
    let (d5, d6) = (ab.dot(cp), ac.dot(cp));
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return a + ac * (d2 / (d2 - d6));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        return b + (c - b) * ((d4 - d3) / ((d4 - d3) + (d5 - d6)));
    }
    let denom = va + vb + vc;
    if denom.abs() < f64::MIN_POSITIVE {
        // Degenerate (zero-area) triangle: fall back to the closest edge point.
        let candidates = [a + ab * (d1 / ab.length_squared().max(f64::MIN_POSITIVE)).clamp(0.0, 1.0), b, c];
        return candidates
            .into_iter()
            .min_by(|x, y| (*x - p).length_squared().total_cmp(&(*y - p).length_squared()))
            .unwrap_or(a);
    }
    let (v, w) = (vb / denom, vc / denom);
    a + ab * v + ac * w
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closest_point_where_skips_rejected_triangles() {
        // Two parallel unit triangles 0.01 apart; rejecting the near one finds the far one.
        let p = [
            DVec3::ZERO,
            DVec3::X,
            DVec3::Y,
            DVec3::new(0.0, 0.0, 0.01),
            DVec3::new(1.0, 0.0, 0.01),
            DVec3::new(0.0, 1.0, 0.01),
        ];
        let bvh = TriangleBvh::new(&p, &[[0, 1, 2], [3, 4, 5]]);
        let q = DVec3::new(0.2, 0.2, -0.001);
        assert!(bvh.closest_point(q).unwrap().0.z.abs() < 1e-12);
        let (far, _) = bvh.closest_point_where(q, |t| t == 1).unwrap();
        assert!((far.z - 0.01).abs() < 1e-12);
        assert!(bvh.closest_point_where(q, |_| false).is_none());
    }
    use proptest::prelude::*;

    fn v(x: f64, y: f64, z: f64) -> DVec3 {
        DVec3::new(x, y, z)
    }

    #[test]
    fn closest_point_regions() {
        let (a, b, c) = (v(0., 0., 0.), v(1., 0., 0.), v(0., 1., 0.));
        let cases = [
            (v(0.2, 0.2, 1.0), v(0.2, 0.2, 0.0)),  // face
            (v(-1.0, -1.0, 0.0), a),               // vertex a
            (v(2.0, -0.5, 0.0), b),                // vertex b
            (v(-0.5, 2.0, 0.0), c),                // vertex c
            (v(0.5, -1.0, 0.0), v(0.5, 0.0, 0.0)), // edge ab
            (v(-1.0, 0.5, 0.0), v(0.0, 0.5, 0.0)), // edge ac
            (v(1.0, 1.0, 0.0), v(0.5, 0.5, 0.0)),  // edge bc
        ];
        for (p, expected) in cases {
            let got = closest_point_on_triangle(p, a, b, c);
            assert!((got - expected).length() < 1e-12, "{p:?}: got {got:?}, expected {expected:?}");
        }
    }

    #[test]
    fn arrays_are_allocated_at_their_final_size() {
        for n in [1usize, 4, 5, 7, 8, 9, 63, 64, 65, 1000, 4097, 99_999] {
            let positions: Vec<DVec3> = (0..n + 2).map(|i| v(i as f64, (i * i % 7) as f64, 0.0)).collect();
            let tris: Vec<[u32; 3]> = (0..n as u32).map(|i| [i, i + 1, i + 2]).collect();
            let (binary, packets, ids) = build_binary(&positions, &tris);
            assert_eq!((packets.len(), packets.capacity()), (leaf_count(n), leaf_count(n)), "n = {n}");
            assert_eq!(binary.len(), binary.capacity(), "n = {n}");
            assert_eq!(ids.len(), ids.capacity(), "n = {n}");
            let nodes = flatten(&binary);
            assert_eq!(nodes.len(), nodes.capacity(), "n = {n}");
        }
    }

    #[test]
    fn empty_bvh_returns_none() {
        let bvh = TriangleBvh::new(&[], &[]);
        assert!(bvh.closest_point(DVec3::ZERO).is_none());
    }

    fn brute_force(positions: &[DVec3], tris: &[[u32; 3]], p: DVec3) -> f64 {
        tris.iter()
            .map(|t| {
                let [a, b, c] = t.map(|i| positions[i as usize]);
                (closest_point_on_triangle(p, a, b, c) - p).length()
            })
            .fold(f64::INFINITY, f64::min)
    }

    fn point() -> impl Strategy<Value = DVec3> {
        (-5.0..5.0f64, -5.0..5.0f64, -5.0..5.0f64).prop_map(|(x, y, z)| v(x, y, z))
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(32))]
        #[test]
        fn bvh_matches_brute_force(
            points in proptest::collection::vec(point(), 150),
            queries in proptest::collection::vec(point(), 200),
        ) {
            let tris: Vec<[u32; 3]> = (0..50u32).map(|i| [3 * i, 3 * i + 1, 3 * i + 2]).collect();
            let bvh = TriangleBvh::new(&points, &tris);
            for q in queries {
                let (closest, dist) = bvh.closest_point(q).unwrap();
                let expected = brute_force(&points, &tris, q);
                prop_assert!((dist - expected).abs() < 1e-9, "bvh {dist} vs brute {expected}");
                prop_assert!(((closest - q).length() - dist).abs() < 1e-9);
            }
        }
    }

    /// The binary tree searched the plain way: what the flat SIMD search must
    /// reproduce exactly, ties included.
    fn binary_search(positions: &[DVec3], tris: &[[u32; 3]], p: DVec3) -> Option<(DVec3, f64, u32)> {
        let (nodes, _, ids) = build_binary(positions, tris);
        let mut best = (DVec3::ZERO, f64::INFINITY, 0u32);
        let mut stack = vec![(0u32, nodes.first()?.bounds.distance_squared(p))];
        while let Some((node, box_d2)) = stack.pop() {
            if box_d2 >= best.1 {
                continue;
            }
            match nodes[node as usize].kind {
                [leaf, _] if leaf & LEAF != 0 => {
                    let packet = (leaf & !LEAF) as usize;
                    for &id in ids[4 * packet..4 * packet + 4].iter().take_while(|&&id| id != NONE) {
                        let [a, b, c] = tris[id as usize].map(|i| positions[i as usize]);
                        let q = closest_point_on_triangle(p, a, b, c);
                        let d2 = (q - p).length_squared();
                        if d2 < best.1 {
                            best = (q, d2, id);
                        }
                    }
                }
                [left, right] => {
                    let dl = nodes[left as usize].bounds.distance_squared(p);
                    let dr = nodes[right as usize].bounds.distance_squared(p);
                    if dl <= dr {
                        stack.extend([(right, dr), (left, dl)]);
                    } else {
                        stack.extend([(left, dl), (right, dr)]);
                    }
                }
            }
        }
        best.1.is_finite().then(|| (best.0, best.1.sqrt(), best.2))
    }

    fn bits(r: Option<(DVec3, f64, u32)>) -> Option<([u64; 3], u64, u32)> {
        r.map(|(q, d, t)| (q.to_array().map(f64::to_bits), d.to_bits(), t))
    }

    #[test]
    fn simd_lanes_match_the_scalar_routine_bit_for_bit() {
        // Random triangles plus degenerate ones: segments (a, b, b), collinear and points.
        let mut seed = 1u64;
        let mut r = || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (seed >> 11) as f64 / (1u64 << 53) as f64 * 4.0 - 2.0
        };
        let mut corners = Vec::new();
        for i in 0..400 {
            let (a, b, c) = (v(r(), r(), r()), v(r(), r(), r()), v(r(), r(), r()));
            corners.push(match i % 8 {
                0 => [a, b, b],
                1 => [a, b, a + (b - a) * 0.25],
                2 => [a, a, a],
                _ => [a, b, c],
            });
        }
        let level = Level::new();
        for packet in corners.as_chunks::<4>().0 {
            let mut lanes = Packet([[[0.0; 4]; 3]; 3]);
            for (lane, t) in packet.iter().enumerate() {
                for (k, p) in t.iter().enumerate() {
                    for axis in 0..3 {
                        lanes.0[k][axis][lane] = p[axis];
                    }
                }
            }
            for _ in 0..20 {
                let p = v(r(), r(), r()) * 1.5;
                let got = dispatch!(level, simd => closest4(simd, &lanes, p));
                for (lane, &[a, b, c]) in packet.iter().enumerate() {
                    if got.degenerate & (1 << lane) != 0 {
                        continue;
                    }
                    let want = closest_point_on_triangle(p, a, b, c);
                    let q = v(got.q[0][lane], got.q[1][lane], got.q[2][lane]);
                    assert_eq!(q.to_array().map(f64::to_bits), want.to_array().map(f64::to_bits));
                    assert_eq!(got.d2[lane].to_bits(), (want - p).length_squared().to_bits());
                }
            }
        }
    }

    #[test]
    fn flat_search_matches_the_binary_tree_and_every_simd_level() {
        // A grid: many queries are equally near two triangles (shared edges and
        // vertices), so the tie order matters.
        let n = 24;
        let positions: Vec<DVec3> =
            (0..n * n).map(|i| v((i % n) as f64, (i / n) as f64, ((i % 5) as f64) * 0.1)).collect();
        let mut tris = Vec::new();
        for y in 0..n - 1 {
            for x in 0..n - 1 {
                let i = (y * n + x) as u32;
                tris.extend([[i, i + 1, i + n as u32 + 1], [i, i + n as u32 + 1, i + n as u32]]);
            }
        }
        let fast = TriangleBvh::new(&positions, &tris);
        let baseline = TriangleBvh::with_level(&positions, &tris, Level::baseline());
        for i in 0..2000 {
            let p = v((i % 47) as f64 * 0.5, (i / 47) as f64 * 0.5, ((i % 7) as f64 - 3.0) * 0.4);
            let want = bits(binary_search(&positions, &tris, p));
            assert_eq!(bits(fast.closest_triangle_where(p, |_| true)), want, "{p:?}");
            assert_eq!(bits(baseline.closest_triangle_where(p, |_| true)), want, "{p:?}");
        }
    }

    #[test]
    fn a_widening_search_is_the_one_pass_search() {
        // A bumpy grid of unit cells, so the spacing is a little over 1.
        let n = 24;
        let positions: Vec<DVec3> =
            (0..n * n).map(|i| v((i % n) as f64, (i / n) as f64, ((i % 5) as f64) * 0.1)).collect();
        let mut tris = Vec::new();
        for y in 0..n - 1 {
            for x in 0..n - 1 {
                let i = (y * n + x) as u32;
                tris.extend([[i, i + 1, i + n as u32 + 1], [i, i + n as u32 + 1, i + n as u32]]);
            }
        }
        let levels = [
            TriangleBvh::new(&positions, &tris),
            TriangleBvh::with_level(&positions, &tris, Level::baseline()),
        ];
        // From everything let through to one triangle in 97, and none at all.
        let filters: [&dyn Fn(u32) -> bool; 5] =
            [&|_| true, &|t| t.is_multiple_of(7), &|t| t.is_multiple_of(97), &|t| t == 1000, &|_| false];
        for bvh in &levels {
            assert!(bvh.spacing > 1.0 && bvh.spacing < 1.5, "{}", bvh.spacing);
            for i in 0..600 {
                // On the grid, a little off it, and far outside every widening.
                let off = [0.0, 0.3, 3.0, 40.0, 900.0][i % 5];
                let p = v((i % 29) as f64 * 0.83 - 1.0, (i / 29) as f64 * 1.07 - 1.0, off);
                for accept in filters {
                    assert_eq!(
                        bits(bvh.closest_triangle_where(p, accept)),
                        bits(bvh.closest_triangle_anywhere(p, accept)),
                        "{p:?}"
                    );
                }
            }
        }
        // Nothing to scale by: straight to the one pass.
        let point = TriangleBvh::new(&[v(1.0, 2.0, 3.0)], &[[0, 0, 0]]);
        assert_eq!(point.spacing, 0.0);
        assert_eq!(point.closest_triangle_where(v(0.0, 0.0, 0.0), |_| true).map(|f| f.2), Some(0));
        assert_eq!(TriangleBvh::new(&[], &[]).closest_triangle_where(v(0.0, 0.0, 0.0), |_| true), None);
    }

    /// A ruled strip: `n` pairs of slivers 100 long and a quarter wide,
    /// leaning and a little bumpy.
    fn slivers(n: usize) -> (Vec<DVec3>, Vec<[u32; 3]>) {
        let positions: Vec<DVec3> = (0..=n)
            .flat_map(|i| {
                let x = i as f64 * 0.25;
                [v(x, 0.0, (i % 3) as f64 * 0.2), v(x + 3.0, 100.0, (i % 4) as f64 * 0.2)]
            })
            .collect();
        let tris =
            (0..n as u32).flat_map(|i| [[2 * i, 2 * i + 2, 2 * i + 1], [2 * i + 2, 2 * i + 3, 2 * i + 1]]);
        (positions, tris.collect())
    }

    #[test]
    fn long_triangles_are_cut_into_pieces_for_the_tree() {
        let (positions, tris) = slivers(200);
        let area = |p: &[DVec3], t: &[[u32; 3]]| -> f64 {
            t.iter()
                .map(|t| {
                    let [a, b, c] = t.map(|i| p[i as usize]);
                    0.5 * (b - a).cross(c - a).length()
                })
                .sum()
        };
        let cut = cut_long(&positions, &tris).unwrap();
        // The same surface, in pieces of the mesh's own size of triangle
        // (7.6 for slivers of this area), each triangle's pieces together
        // and in the triangles' order.
        assert!((area(&cut.positions, &cut.triangles) - area(&positions, &tris)).abs() < 1e-6);
        assert!(
            cut.source.len() >= 16 * tris.len() && cut.source.len() <= 128 * tris.len(),
            "{}",
            cut.source.len()
        );
        assert!(
            cut.source.windows(2).all(|w| w[0] <= w[1])
                && cut.source.last() == Some(&(tris.len() as u32 - 1))
        );
        let longest = cut
            .triangles
            .iter()
            .map(|t| longest_edge(t.map(|i| cut.positions[i as usize])).1)
            .fold(0.0, f64::max);
        assert!(longest.sqrt() > 3.5 && longest.sqrt() < 8.0, "{}", longest.sqrt());
        // With no room for that many there are none; and a triangle that
        // is not long is not cut, whatever the piece size.
        assert!(cut_at(&positions, &tris, 30.0, 7.6, 1000).is_none());
        assert_eq!(cut_at(&positions, &tris, 200.0, 7.6, 1000).unwrap().source.len(), tris.len());
        let coarse = cut_at(&positions, &tris, 30.0, 60.0, 4000).unwrap();
        assert!(coarse.source.len() >= 2 * tris.len() && coarse.source.len() <= 4 * tris.len());
        // Compact triangles are left as they are, and so is a mesh with no
        // area to go by.
        let n = 24;
        let grid: Vec<DVec3> = (0..n * n).map(|i| v((i % n) as f64, (i / n) as f64, 0.0)).collect();
        let mut cells = Vec::new();
        for y in 0..n - 1 {
            for x in 0..n - 1 {
                let i = (y * n + x) as u32;
                cells.extend([[i, i + 1, i + n as u32 + 1], [i, i + n as u32 + 1, i + n as u32]]);
            }
        }
        assert!(cut_long(&grid, &cells).is_none());
        assert!(cut_long(&[v(0.0, 0.0, 0.0), v(9.0, 0.0, 0.0)], &[[0, 1, 1], [0, 0, 0]]).is_none());
        assert!(cut_long(&[], &[]).is_none());
        // A few long triangles among compact ones are not worth a tree of
        // pieces: one triangle a hundred times the size of the rest.
        let mut mixed = grid.clone();
        mixed.extend([v(0.0, 0.0, 5.0), v(300.0, 0.0, 5.0), v(0.0, 300.0, 5.0)]);
        let base = (n * n) as u32;
        let mut both = cells.clone();
        both.push([base, base + 1, base + 2]);
        assert!(cut_long(&mixed, &both).is_none());
        // A corner that is not a number is passed over, not gone round for ever.
        let (mut broken, mut with_it) = slivers(200);
        broken.push(v(f64::NAN, 0.0, 0.0));
        with_it.push([0, 2, broken.len() as u32 - 1]);
        let cut = cut_long(&broken, &with_it).unwrap();
        assert_eq!(cut.source.iter().filter(|&&s| s as usize == with_it.len() - 1).count(), 1);
        assert!(cut.source.len() > 16 * tris.len());
    }

    #[test]
    fn a_tree_of_pieces_answers_as_the_triangles_do() {
        let (positions, tris) = slivers(60);
        let whole = TriangleBvh::of(&positions, &tris, Level::new());
        let levels = [
            TriangleBvh::new(&positions, &tris),
            TriangleBvh::with_level(&positions, &tris, Level::baseline()),
        ];
        let filters: [&dyn Fn(u32) -> bool; 4] =
            [&|_| true, &|t| t.is_multiple_of(7), &|t| t == 101, &|_| false];
        for bvh in &levels {
            assert!(bvh.packets.len() > 16 * whole.packets.len());
            assert!(bvh.spacing < 0.2 * whole.spacing, "{} {}", bvh.spacing, whole.spacing);
            for i in 0..160 {
                let off = [0.0, 0.3, 4.0, 60.0][i % 4];
                let p = v((i % 13) as f64 * 1.6 - 3.0, (i / 13) as f64 * 9.5 - 5.0, off);
                for accept in filters {
                    let (cut, plain) =
                        (bvh.closest_triangle_where(p, accept), whole.closest_triangle_where(p, accept));
                    let (Some((q, d, t)), Some((_, e, _))) = (cut, plain) else {
                        assert_eq!((cut, plain), (None, None), "{p:?}");
                        continue;
                    };
                    // The same distance, to a triangle of the mesh that is let
                    // through and lies that near (which of two that share
                    // an edge is not told apart).
                    assert!((d - e).abs() < 1e-9 && (q - p).length() - d < 1e-9, "{p:?}: {d} {e}");
                    assert!((t as usize) < tris.len() && accept(t), "{p:?}: {t}");
                    let [a, b, c] = tris[t as usize].map(|i| positions[i as usize]);
                    assert!(((closest_point_on_triangle(p, a, b, c) - p).length() - d).abs() < 1e-9, "{p:?}");
                }
            }
        }
    }

    #[test]
    fn a_bounded_search_finds_what_the_unbounded_one_does() {
        // The same grid, with one triangle in seven let through: most of what
        // is near a query is refused, as when a vertex looks for its own patch.
        let n = 24;
        let positions: Vec<DVec3> =
            (0..n * n).map(|i| v((i % n) as f64, (i / n) as f64, ((i % 5) as f64) * 0.1)).collect();
        let mut tris = Vec::new();
        for y in 0..n - 1 {
            for x in 0..n - 1 {
                let i = (y * n + x) as u32;
                tris.extend([[i, i + 1, i + n as u32 + 1], [i, i + n as u32 + 1, i + n as u32]]);
            }
        }
        let choosy = |t: u32| t.is_multiple_of(7);
        let levels = [
            TriangleBvh::new(&positions, &tris),
            TriangleBvh::with_level(&positions, &tris, Level::baseline()),
        ];
        for bvh in levels {
            for i in 0..2000 {
                let p = v((i % 47) as f64 * 0.5, (i / 47) as f64 * 0.5, ((i % 7) as f64 - 3.0) * 0.4);
                let (q, d, t) = bvh.closest_triangle_where(p, choosy).expect("some triangle is let through");
                let want = bits(Some((q, d, t)));
                // Any reach past the answer gives the answer, bit for bit.
                for reach in [d * 1.000_001 + 1e-9, d + 3.0, f64::INFINITY] {
                    assert_eq!(
                        bits(bvh.closest_triangle_within(p, reach, choosy)),
                        want,
                        "{p:?} within {reach}"
                    );
                }
                // A reach short of it gives nothing, and so does no reach at all.
                assert_eq!(bvh.closest_triangle_within(p, d * 0.999, choosy), None, "{p:?}");
                assert_eq!(bvh.closest_triangle_within(p, 0.0, choosy), None);
                assert_eq!(bvh.closest_triangle_within(p, f64::NAN, choosy), None);
            }
            assert_eq!(bvh.closest_triangle_within(v(1.0, 1.0, 0.0), 100.0, |_| false), None);
        }
    }
}
