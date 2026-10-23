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

/// Four children, their boxes lane by lane (`[axis][lane]`), and the boxes
/// of the two binary halves they came from (children 0–1 and 2–3), which
/// decide the visiting order.
#[derive(Clone, Copy, Debug)]
struct Node {
    min: [[f64; 4]; 3],
    max: [[f64; 4]; 3],
    child: [u32; 4],
    halves: [Aabb; 2],
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
    level: Level,
}

impl TriangleBvh {
    pub fn new(positions: &[DVec3], triangles: &[[u32; 3]]) -> Self {
        Self::with_level(positions, triangles, Level::new())
    }

    /// A BVH whose queries run at SIMD `level`.
    fn with_level(positions: &[DVec3], triangles: &[[u32; 3]], level: Level) -> Self {
        let corners: Vec<[DVec3; 3]> = triangles.iter().map(|t| t.map(|i| positions[i as usize])).collect();
        let (binary, packets, ids) = build_binary(&corners);
        let bounds = binary.first().map(|root| root.bounds);
        Self { nodes: flatten(&binary), packets, ids, bounds, level }
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
    pub fn closest_triangle_where(
        &self,
        p: DVec3,
        accept: impl Fn(u32) -> bool,
    ) -> Option<(DVec3, f64, u32)> {
        dispatch!(self.level, simd => self.search(simd, p, &accept))
    }

    #[inline(always)]
    fn search<S: Simd>(&self, simd: S, p: DVec3, accept: &impl Fn(u32) -> bool) -> Option<(DVec3, f64, u32)> {
        if self.nodes.is_empty() {
            return None;
        }
        let ps = [p.x, p.y, p.z].map(|x| f64x4::splat(simd, x));
        let zero = f64x4::splat(simd, 0.0);
        let mut best = (DVec3::ZERO, f64::INFINITY, 0u32); // (point, squared distance, triangle)
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
            let [left, right] = node.halves.map(|h| h.distance_squared(p));
            let ([a, b], [c, d]) = if left <= right { (pair(0), pair(2)) } else { (pair(2), pair(0)) };
            for k in [d, c, b, a] {
                if node.child[k] != NONE && d2[k] < best.1 {
                    stack.push((node.child[k], d2[k]));
                }
            }
        }
        best.1.is_finite().then(|| (best.0, best.1.sqrt(), best.2))
    }

    pub fn bounds(&self) -> Option<(DVec3, DVec3)> {
        self.bounds.map(|b| (b.min, b.max))
    }
}

/// The binary tree (root first), its leaves' packets and the packets' lane ids.
fn build_binary(corners: &[[DVec3; 3]]) -> (Vec<Binary>, Vec<Packet>, Vec<u32>) {
    let (mut packets, mut ids) = (Vec::new(), Vec::new());
    if corners.is_empty() {
        return (Vec::new(), packets, ids);
    }
    let centroids: Vec<DVec3> = corners.iter().map(|[a, b, c]| (*a + *b + *c) / 3.0).collect();
    let mut order: Vec<u32> = (0..corners.len() as u32).collect();
    let mut nodes = vec![Binary { bounds: Aabb::EMPTY, kind: [NONE; 2] }];
    let mut work = vec![(0usize, 0usize, order.len())];
    while let Some((node, start, end)) = work.pop() {
        let slice = &mut order[start..end];
        let bounds = slice
            .iter()
            .fold(Aabb::EMPTY, |acc, &t| corners[t as usize].iter().fold(acc, |acc, &p| acc.grow(p)));
        if slice.len() <= LEAF_SIZE {
            let mut packet = Packet([[[0.0; 4]; 3]; 3]);
            for lane in 0..LEAF_SIZE {
                // Spare lanes repeat the last triangle; their ids say to skip them.
                let t = slice[lane.min(slice.len() - 1)];
                for (corner, p) in corners[t as usize].iter().enumerate() {
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
    let mut nodes = Vec::new();
    if binary.is_empty() {
        return nodes;
    }
    let is_leaf = |x: u32| binary[x as usize].kind[0] & LEAF != 0;
    let empty = Node {
        min: [[f64::INFINITY; 4]; 3],
        max: [[f64::NEG_INFINITY; 4]; 3],
        child: [NONE; 4],
        halves: [Aabb::EMPTY; 2],
    };
    nodes.push(empty);
    let mut work = vec![(0usize, 0u32)];
    while let Some((at, x)) = work.pop() {
        let halves = if is_leaf(x) { [x, NONE] } else { binary[x as usize].kind };
        let mut node = empty;
        for (h, &y) in halves.iter().enumerate() {
            if y == NONE {
                continue;
            }
            node.halves[h] = binary[y as usize].bounds;
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
    fn binary_search(corners: &[[DVec3; 3]], p: DVec3) -> Option<(DVec3, f64, u32)> {
        let (nodes, _, ids) = build_binary(corners);
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
                        let [a, b, c] = corners[id as usize];
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
        let corners: Vec<[DVec3; 3]> = tris.iter().map(|t| t.map(|i| positions[i as usize])).collect();
        let fast = TriangleBvh::new(&positions, &tris);
        let baseline = TriangleBvh::with_level(&positions, &tris, Level::baseline());
        for i in 0..2000 {
            let p = v((i % 47) as f64 * 0.5, (i / 47) as f64 * 0.5, ((i % 7) as f64 - 3.0) * 0.4);
            let want = bits(binary_search(&corners, p));
            assert_eq!(bits(fast.closest_triangle_where(p, |_| true)), want, "{p:?}");
            assert_eq!(bits(baseline.closest_triangle_where(p, |_| true)), want, "{p:?}");
        }
    }
}
