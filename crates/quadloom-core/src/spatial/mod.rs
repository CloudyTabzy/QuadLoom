//! Spatial queries over triangle sets.
//!
//! [`TriangleBvh`] is a bounding-volume hierarchy built top-down with median
//! splits. Both the build and the queries use explicit stacks, never recursion.

pub use glam::DVec3;

const LEAF_SIZE: usize = 4;

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

    fn distance_squared(&self, p: DVec3) -> f64 {
        let outside = (self.min - p).max(p - self.max).max(DVec3::ZERO);
        outside.length_squared()
    }
}

#[derive(Clone, Copy, Debug)]
enum NodeKind {
    /// Triangles `order[start..start + count]`.
    Leaf {
        start: u32,
        count: u32,
    },
    Inner {
        left: u32,
        right: u32,
    },
}

#[derive(Clone, Copy, Debug)]
struct Node {
    bounds: Aabb,
    kind: NodeKind,
}

/// Closest-point queries against a fixed set of triangles.
#[derive(Clone, Debug)]
pub struct TriangleBvh {
    nodes: Vec<Node>,
    /// Triangle corners, in leaf order.
    corners: Vec<[DVec3; 3]>,
    /// The input index of each triangle, in leaf order.
    ids: Vec<u32>,
}

impl TriangleBvh {
    pub fn new(positions: &[DVec3], triangles: &[[u32; 3]]) -> Self {
        let corners: Vec<[DVec3; 3]> = triangles.iter().map(|t| t.map(|i| positions[i as usize])).collect();
        if corners.is_empty() {
            return Self { nodes: Vec::new(), corners, ids: Vec::new() };
        }
        let centroids: Vec<DVec3> = corners.iter().map(|[a, b, c]| (*a + *b + *c) / 3.0).collect();
        let mut order: Vec<u32> = (0..corners.len() as u32).collect();

        let mut nodes = vec![Node { bounds: Aabb::EMPTY, kind: NodeKind::Leaf { start: 0, count: 0 } }];
        let mut work = vec![(0usize, 0usize, order.len())];
        while let Some((node, start, end)) = work.pop() {
            let slice = &mut order[start..end];
            let bounds = slice
                .iter()
                .fold(Aabb::EMPTY, |acc, &t| corners[t as usize].iter().fold(acc, |acc, &p| acc.grow(p)));
            if slice.len() <= LEAF_SIZE {
                nodes[node] =
                    Node { bounds, kind: NodeKind::Leaf { start: start as u32, count: slice.len() as u32 } };
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
            let placeholder = Node { bounds: Aabb::EMPTY, kind: NodeKind::Leaf { start: 0, count: 0 } };
            nodes.extend([placeholder, placeholder]);
            nodes[node] = Node { bounds, kind: NodeKind::Inner { left: left as u32, right: right as u32 } };
            work.push((left, start, start + mid));
            work.push((right, start + mid, end));
        }
        let corners = order.iter().map(|&t| corners[t as usize]).collect();
        Self { nodes, corners, ids: order }
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
        let root = self.nodes.first()?;
        let mut best = (DVec3::ZERO, f64::INFINITY, 0u32); // (point, squared distance, triangle)
        let mut stack = vec![(0u32, root.bounds.distance_squared(p))];
        while let Some((node, box_d2)) = stack.pop() {
            if box_d2 >= best.1 {
                continue;
            }
            match self.nodes[node as usize].kind {
                NodeKind::Leaf { start, count } => {
                    let range = start as usize..(start + count) as usize;
                    for ([a, b, c], &id) in self.corners[range.clone()].iter().zip(&self.ids[range]) {
                        if !accept(id) {
                            continue;
                        }
                        let q = closest_point_on_triangle(p, *a, *b, *c);
                        let d2 = (q - p).length_squared();
                        if d2 < best.1 {
                            best = (q, d2, id);
                        }
                    }
                }
                NodeKind::Inner { left, right } => {
                    let dl = self.nodes[left as usize].bounds.distance_squared(p);
                    let dr = self.nodes[right as usize].bounds.distance_squared(p);
                    // Push the farther child first so the nearer one is searched first.
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

    pub fn bounds(&self) -> Option<(DVec3, DVec3)> {
        self.nodes.first().map(|n| (n.bounds.min, n.bounds.max))
    }
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
}
