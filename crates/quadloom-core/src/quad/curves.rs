//! Feature lines and borders as curves a vertex can slide along.
//!
//! The quads' vertices on a feature line must stay on it, but not where they
//! were first put: evenly spaced by arc length per segment is rarely where
//! the quads on both sides want them. A [`Chain`] is a run of feature (or
//! border) edges of the layout's mesh between two points that must not move:
//! a junction, an end, or a corner the line turns at. A vertex on a chain is a
//! distance along it, and smoothing changes only that.

use hashbrown::HashMap;

use glam::DVec3;

/// A feature line turning by more than this at a mesh vertex has a corner
/// there (cos 45°). Below it the line is a curve, sampled coarsely.
const CORNER_COS: f64 = std::f64::consts::FRAC_1_SQRT_2;

/// One run of feature edges. A closed chain repeats its first vertex last.
#[derive(Clone, Debug)]
pub struct Chain {
    points: Vec<DVec3>,
    /// Arc length from the first point to each point.
    arc: Vec<f64>,
    closed: bool,
}

impl Chain {
    pub fn length(&self) -> f64 {
        self.arc.last().copied().unwrap_or(0.0)
    }

    /// `at` brought into the chain: wrapped around a closed one, clamped to
    /// the ends of an open one.
    pub fn bound(&self, at: f64) -> f64 {
        let length = self.length();
        if length.is_nan() || length <= 0.0 || !at.is_finite() {
            return 0.0;
        }
        if self.closed { at.rem_euclid(length) } else { at.clamp(0.0, length) }
    }

    /// The edge `at` lies on: its first point's index and the fraction along it.
    fn locate(&self, at: f64) -> (usize, f64) {
        let at = self.bound(at);
        let last = self.arc.len().saturating_sub(1).max(1);
        let i = self.arc.partition_point(|&a| a <= at).clamp(1, last) - 1;
        let (a0, a1) = (self.arc[i], self.arc.get(i + 1).copied().unwrap_or(self.arc[i]));
        (i, if a1 > a0 { ((at - a0) / (a1 - a0)).clamp(0.0, 1.0) } else { 0.0 })
    }

    /// The point at arc length `at`.
    pub fn point(&self, at: f64) -> DVec3 {
        let (i, w) = self.locate(at);
        let next = self.points.get(i + 1).copied().unwrap_or(self.points[i]);
        self.points[i].lerp(next, w)
    }

    /// The unit direction of the chain at arc length `at` (zero on a
    /// degenerate edge).
    pub fn tangent(&self, at: f64) -> DVec3 {
        let (i, _) = self.locate(at);
        let next = self.points.get(i + 1).copied().unwrap_or(self.points[i]);
        (next - self.points[i]).normalize_or_zero()
    }
}

/// Where a mesh vertex or edge sits on the chains.
#[derive(Clone, Debug, Default)]
pub struct Curves {
    pub chains: Vec<Chain>,
    /// Mesh edge `(low, high)` to its chain, the arc length of `low`, and
    /// the arc length of `high`.
    of_edge: HashMap<(u32, u32), (u32, f64, f64)>,
    /// A mesh vertex inside a chain (not one of its fixed ends) to its chain
    /// and arc length.
    of_vertex: HashMap<u32, (u32, f64)>,
}

impl Curves {
    /// The chains of `edges` (vertex pairs, each once) over `positions`.
    pub fn new(positions: &[DVec3], edges: impl IntoIterator<Item = (u32, u32)>) -> Self {
        let position = |v: u32| positions.get(v as usize).copied().unwrap_or(DVec3::ZERO);
        let mut around: HashMap<u32, Vec<u32>> = HashMap::new();
        let mut list: Vec<(u32, u32)> = edges.into_iter().filter(|&(a, b)| a != b).collect();
        for e in &mut list {
            *e = (e.0.min(e.1), e.0.max(e.1));
        }
        list.sort_unstable();
        list.dedup();
        for &(a, b) in &list {
            around.entry(a).or_default().push(b);
            around.entry(b).or_default().push(a);
        }
        // A vertex the chains stop at: an end, a junction, or a corner.
        let stops = |v: u32| -> bool {
            let Some(next) = around.get(&v) else { return true };
            let &[a, b] = next.as_slice() else { return true };
            let (da, db) = (
                (position(v) - position(a)).normalize_or_zero(),
                (position(b) - position(v)).normalize_or_zero(),
            );
            da.dot(db) < CORNER_COS
        };
        let mut curves = Self::default();
        let mut used: hashbrown::HashSet<(u32, u32)> = hashbrown::HashSet::with_capacity(list.len());
        let key = |a: u32, b: u32| (a.min(b), a.max(b));
        // Walks from `start` along its edge to `first` until a stop (or back to start).
        let walk = |curves: &mut Self, used: &mut hashbrown::HashSet<(u32, u32)>, start: u32, first: u32| {
            let mut vertices = vec![start, first];
            used.insert(key(start, first));
            loop {
                let (prev, here) = (vertices[vertices.len() - 2], vertices[vertices.len() - 1]);
                if here == start || stops(here) {
                    break;
                }
                let Some(&next) = around.get(&here).and_then(|n| n.iter().find(|&&n| n != prev)) else {
                    break;
                };
                if !used.insert(key(here, next)) {
                    break;
                }
                vertices.push(next);
            }
            let closed = vertices.len() > 2 && vertices.first() == vertices.last();
            let points: Vec<DVec3> = vertices.iter().map(|&v| position(v)).collect();
            let mut arc = vec![0.0];
            for w in points.windows(2) {
                let last = arc.last().copied().unwrap_or(0.0);
                arc.push(last + (w[1] - w[0]).length());
            }
            let id = curves.chains.len() as u32;
            for (i, w) in vertices.windows(2).enumerate() {
                let (low, high) = if w[0] < w[1] { (arc[i], arc[i + 1]) } else { (arc[i + 1], arc[i]) };
                curves.of_edge.insert(key(w[0], w[1]), (id, low, high));
            }
            let inside = if closed { 0..vertices.len() - 1 } else { 1..vertices.len() - 1 };
            for i in inside {
                // A closed chain's first vertex is inside it too, unless it is a stop.
                if !(closed && i == 0 && stops(vertices[0])) {
                    curves.of_vertex.insert(vertices[i], (id, arc[i]));
                }
            }
            curves.chains.push(Chain { points, arc, closed });
        };
        // Chains between stops first, in a fixed order, then the loops left over.
        let mut starts: Vec<u32> = around.keys().copied().filter(|&v| stops(v)).collect();
        starts.sort_unstable();
        for &v in &starts {
            let mut next = around.get(&v).cloned().unwrap_or_default();
            next.sort_unstable();
            for n in next {
                if !used.contains(&key(v, n)) {
                    walk(&mut curves, &mut used, v, n);
                }
            }
        }
        for &(a, b) in &list {
            if !used.contains(&(a, b)) {
                walk(&mut curves, &mut used, a, b);
            }
        }
        curves
    }

    /// The chain and arc length of mesh vertex `v`, if it lies inside one
    /// chain. `None` for a vertex off the chains, and for one a chain stops at.
    pub fn at_vertex(&self, v: u32) -> Option<(u32, f64)> {
        self.of_vertex.get(&v).copied()
    }

    /// The chain and arc length of the point a fraction `w` of the way from
    /// mesh vertex `a` to `b`, if `(a, b)` is an edge of a chain.
    pub fn on_edge(&self, a: u32, b: u32, w: f64) -> Option<(u32, f64)> {
        let &(chain, low, high) = self.of_edge.get(&(a.min(b), a.max(b)))?;
        let (from, to) = if a < b { (low, high) } else { (high, low) };
        // A closed chain's last edge runs up to its full length, not back to 0.
        Some((chain, from + (to - from) * w.clamp(0.0, 1.0)))
    }

    pub fn chain(&self, id: u32) -> Option<&Chain> {
        self.chains.get(id as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ring(n: u32) -> Vec<DVec3> {
        (0..n)
            .map(|i| {
                let a = std::f64::consts::TAU * f64::from(i) / f64::from(n);
                DVec3::new(a.cos(), a.sin(), 0.0)
            })
            .collect()
    }

    #[test]
    fn a_smooth_loop_is_one_closed_chain_every_vertex_inside() {
        let positions = ring(24);
        let curves = Curves::new(&positions, (0..24).map(|i| (i, (i + 1) % 24)));
        assert_eq!(curves.chains.len(), 1);
        let chain = &curves.chains[0];
        assert!(chain.closed);
        assert!((chain.length() - 24.0 * (positions[1] - positions[0]).length()).abs() < 1e-12);
        for v in 0..24 {
            let (id, at) = curves.at_vertex(v).unwrap();
            assert_eq!(id, 0);
            assert!((chain.point(at) - positions[v as usize]).length() < 1e-12, "vertex {v}");
        }
        // Past the end wraps to the start.
        assert!((chain.point(chain.length() + 0.1) - chain.point(0.1)).length() < 1e-12);
    }

    #[test]
    fn a_square_loop_stops_at_its_four_corners() {
        // Three vertices per side: the corners turn 90°, the others are straight.
        let mut positions = Vec::new();
        for (from, to) in [
            ((0.0, 0.0), (1.0, 0.0)),
            ((1.0, 0.0), (1.0, 1.0)),
            ((1.0, 1.0), (0.0, 1.0)),
            ((0.0, 1.0), (0.0, 0.0)),
        ] {
            for k in 0..3 {
                let t = f64::from(k) / 3.0;
                positions.push(DVec3::new(from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t, 0.0));
            }
        }
        let curves = Curves::new(&positions, (0..12).map(|i| (i, (i + 1) % 12)));
        assert_eq!(curves.chains.len(), 4);
        for v in 0..12 {
            assert_eq!(curves.at_vertex(v).is_some(), v % 3 != 0, "vertex {v}");
        }
        assert!(curves.chains.iter().all(|c| !c.closed && (c.length() - 1.0).abs() < 1e-12));
    }

    #[test]
    fn chains_stop_at_junctions_and_ends() {
        // A "T": 0-1-2-3 straight, and 2-4 branching off.
        let positions = vec![
            DVec3::new(0.0, 0.0, 0.0),
            DVec3::new(1.0, 0.0, 0.0),
            DVec3::new(2.0, 0.0, 0.0),
            DVec3::new(3.0, 0.0, 0.0),
            DVec3::new(2.0, 1.0, 0.0),
        ];
        let curves = Curves::new(&positions, [(0, 1), (1, 2), (2, 3), (2, 4)]);
        assert_eq!(curves.chains.len(), 3);
        assert!(curves.at_vertex(1).is_some());
        for v in [0, 2, 3, 4] {
            assert!(curves.at_vertex(v).is_none(), "vertex {v} is an end or a junction");
        }
        assert!(curves.at_vertex(9).is_none());
    }

    #[test]
    fn a_point_on_an_edge_is_found_from_either_end() {
        let positions = vec![DVec3::ZERO, DVec3::new(1.0, 0.0, 0.0), DVec3::new(3.0, 0.0, 0.0)];
        let curves = Curves::new(&positions, [(0, 1), (1, 2)]);
        let (chain, at) = curves.on_edge(1, 2, 0.25).unwrap();
        let (same, back) = curves.on_edge(2, 1, 0.75).unwrap();
        assert_eq!(chain, same);
        assert!((at - back).abs() < 1e-12);
        let c = curves.chain(chain).unwrap();
        assert!((c.point(at) - DVec3::new(1.5, 0.0, 0.0)).length() < 1e-12);
        assert!((c.tangent(at).x.abs() - 1.0).abs() < 1e-12);
        // An open chain clamps at its ends.
        assert_eq!(c.bound(-1.0), 0.0);
        assert_eq!(c.bound(99.0), c.length());
        assert!(curves.on_edge(0, 2, 0.5).is_none());
    }
}
