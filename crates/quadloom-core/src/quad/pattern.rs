//! Quad patterns for one patch, in its polygon domain.
//!
//! The domain is a regular N-gon with corner `i` at `corner(N, i)` and side
//! `i` running from corner `i` to corner `i+1` (counter-clockwise). Side `i`
//! has `l[i]` edges, sampled evenly: boundary point `(i, m)` sits at fraction
//! `m / l[i]` along the side.
//!
//! Patterns, best first:
//! - **Grid**: 4 sides with `l0 = l2` and `l1 = l3`, a regular grid.
//! - **Center** (Tarini 2021, closed form): one interior vertex joined to a
//!   split point on every side, cutting the patch into N regular sub-grids.
//!   Side i splits into `x_i + y_i = l_i`, and the sub-grid at corner i+1 has
//!   `y_i × x_{i+1}` quads, so `x_{i+1} = y_{i−1}`. Every split must be ≥ 1.
//! - **Split**: one straight chord between two boundary points cuts the patch
//!   into two pieces that are each a Grid or a Center. Among all chords, the
//!   one with the fewest irregular vertices and the best-proportioned length
//!   wins.
//! - **Fan**: any even boundary. One quad if the boundary has 4 edges, else a
//!   center vertex and one quad per two boundary edges. Always valid, poor
//!   shape; the last resort.

use std::collections::HashMap;
use std::f64::consts::TAU;

use glam::DVec2;

/// What a pattern point is, so patches agree on shared vertices.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Point {
    /// Corner `i` of the patch.
    Corner(u8),
    /// Boundary sample `m` (0 < m < l) of side `i`.
    Side(u8, u32),
    /// A vertex inside the patch (numbered per pattern).
    Inner(u32),
}

/// Which pattern filled a patch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PatternKind {
    Grid,
    Center,
    Split,
    Fan,
}

/// A quad pattern: domain points with identities, and counter-clockwise quads.
#[derive(Clone, Debug)]
pub struct Pattern {
    pub points: Vec<(Point, DVec2)>,
    pub quads: Vec<[u32; 4]>,
    pub kind: PatternKind,
}

/// Corner `i` of the regular N-gon (side 0 at the bottom).
pub fn corner(n: usize, i: usize) -> DVec2 {
    let a = TAU * (i % n) as f64 / n as f64 - std::f64::consts::FRAC_PI_2 - std::f64::consts::PI / n as f64;
    DVec2::new(a.cos(), a.sin())
}

/// Domain position of boundary fraction `t` along side `i`.
pub fn side_point(n: usize, i: usize, t: f64) -> DVec2 {
    corner(n, i).lerp(corner(n, i + 1), t)
}

/// The closed-form splits `(x_i, y_i)` of an N-gon (N = 3, 5, 6) with side
/// counts `l`, if all are integers ≥ 1.
pub fn center_splits(l: &[u32]) -> Option<Vec<(u32, u32)>> {
    let n = l.len();
    if !matches!(n, 3 | 5 | 6) {
        return None;
    }
    let l: Vec<i64> = l.iter().map(|&v| v as i64).collect();
    // x_{j+2} = l_j − x_j around each cycle of step 2: one cycle for odd N, two for N = 6.
    let mut x = vec![0i64; n];
    for start in 0..(if n % 2 == 1 { 1 } else { 2 }) {
        // Walking the cycle once gives x_start = (alternating sum) / 2.
        let mut alternating = 0i64;
        let mut sign = 1i64;
        let mut j = start;
        loop {
            alternating += sign * l[j];
            sign = -sign;
            j = (j + 2) % n;
            if j == start {
                break;
            }
        }
        if alternating % 2 != 0 {
            return None;
        }
        x[start] = alternating / 2;
        let mut j = start;
        loop {
            let next = (j + 2) % n;
            if next == start {
                break;
            }
            x[next] = l[j] - x[j];
            j = next;
        }
    }
    let splits: Vec<(i64, i64)> = (0..n).map(|i| (x[i], l[i] - x[i])).collect();
    if splits.iter().any(|&(a, b)| a < 1 || b < 1) {
        return None;
    }
    Some(splits.into_iter().map(|(a, b)| (a as u32, b as u32)).collect())
}

/// How far side counts `l` (N = 3, 5, 6, even total) are from a valid Center:
/// the sum of how much each split falls below 1. 0 means feasible. `None`
/// if a cycle's alternating sum is odd (no integer splits at all).
pub fn center_deficit(l: &[u32]) -> Option<u32> {
    let n = l.len();
    if !matches!(n, 3 | 5 | 6) {
        return None;
    }
    let l: Vec<i64> = l.iter().map(|&v| v as i64).collect();
    let mut deficit = 0i64;
    for start in 0..(if n % 2 == 1 { 1 } else { 2 }) {
        let (mut alternating, mut sign, mut j) = (0i64, 1i64, start);
        loop {
            alternating += sign * l[j];
            sign = -sign;
            j = (j + 2) % n;
            if j == start {
                break;
            }
        }
        if alternating % 2 != 0 {
            return None;
        }
        let mut x = alternating / 2;
        let mut j = start;
        loop {
            deficit += (1 - x).max(0) + (1 - (l[j] - x)).max(0);
            let next = (j + 2) % n;
            if next == start {
                break;
            }
            x = l[j] - x;
            j = next;
        }
    }
    Some(deficit as u32)
}

/// Whether a polygon with edge counts `l` is a Grid or a Center directly.
fn fillable(l: &[u32]) -> Option<PatternKind> {
    if l.len() == 4 && l[0] == l[2] && l[1] == l[3] && l[0] > 0 && l[1] > 0 {
        Some(PatternKind::Grid)
    } else if center_splits(l).is_some() {
        Some(PatternKind::Center)
    } else {
        None
    }
}

/// The best pattern for side counts `l` (3–6 sides, all ≥ 1), or `None` if
/// the boundary count is odd (no quad mesh exists).
pub fn build(l: &[u32]) -> Option<Pattern> {
    let n = l.len();
    if !(3..=6).contains(&n) || l.contains(&0) || l.iter().sum::<u32>() % 2 == 1 {
        return None;
    }
    let mut b = Builder::new(l);
    let whole: Vec<Edge> = (0..n).map(|i| Edge::Run { side: i as u8, from: 0, len: l[i] }).collect();
    if let Some(kind) = b.fill(&whole) {
        return Some(b.finish(kind));
    }
    if b.split() {
        return Some(b.finish(PatternKind::Split));
    }
    let mut b = Builder::new(l);
    b.fan();
    Some(b.finish(PatternKind::Fan))
}

/// An edge of a (sub-)polygon.
#[derive(Clone, Copy, Debug)]
enum Edge {
    /// `len` boundary edges along side `side`, starting at sample `from`.
    Run { side: u8, from: u32, len: u32 },
    /// Chord `id` with `k` edges, from its start to its end (`forward`) or back.
    Chord { id: u32, k: u32, forward: bool },
}

/// Identity of a point while building (inner points get structural keys so
/// that sub-patterns sharing an edge share its points).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Key {
    Boundary(Point),
    Chord { id: u32, t: u32 },
    Ray { region: u32, side: u32, k: u32 },
    Center(u32),
    Cell { region: u32, sub: u32, a: u32, b: u32 },
}

struct Builder<'a> {
    l: &'a [u32],
    n: usize,
    points: Vec<(Point, DVec2)>,
    ids: HashMap<Key, u32>,
    quads: Vec<[u32; 4]>,
    inner: u32,
    regions: u32,
    /// Chord ends as boundary positions `(side, m)`.
    chords: Vec<((u8, u32), (u8, u32))>,
}

impl<'a> Builder<'a> {
    fn new(l: &'a [u32]) -> Self {
        Self {
            l,
            n: l.len(),
            points: Vec::new(),
            ids: HashMap::new(),
            quads: Vec::new(),
            inner: 0,
            regions: 0,
            chords: Vec::new(),
        }
    }

    fn finish(self, kind: PatternKind) -> Pattern {
        Pattern { points: self.points, quads: self.quads, kind }
    }

    /// The point id for `key` at domain position `pos` (created on first use).
    fn id(&mut self, key: Key, pos: DVec2) -> u32 {
        if let Some(&id) = self.ids.get(&key) {
            return id;
        }
        let point = match key {
            Key::Boundary(p) => p,
            _ => {
                self.inner += 1;
                Point::Inner(self.inner - 1)
            }
        };
        self.points.push((point, pos));
        let id = (self.points.len() - 1) as u32;
        self.ids.insert(key, id);
        id
    }

    /// The canonical identity and domain position of boundary sample m of side i.
    fn boundary(&self, i: usize, m: u32) -> (Point, DVec2) {
        let n = self.n;
        let i = i % n;
        let l = self.l[i];
        if m == 0 {
            (Point::Corner(i as u8), corner(n, i))
        } else if m >= l {
            (Point::Corner(((i + 1) % n) as u8), corner(n, i + 1))
        } else {
            (Point::Side(i as u8, m), side_point(n, i, m as f64 / l as f64))
        }
    }

    fn outer(&mut self, i: usize, m: u32) -> u32 {
        let (point, pos) = self.boundary(i, m);
        self.id(Key::Boundary(point), pos)
    }

    fn edge_len(e: Edge) -> u32 {
        match e {
            Edge::Run { len, .. } => len,
            Edge::Chord { k, .. } => k,
        }
    }

    /// Point m along polygon edge e (0 = its start).
    fn edge_point(&mut self, e: Edge, m: u32) -> u32 {
        match e {
            Edge::Run { side, from, .. } => self.outer(side as usize, from + m),
            Edge::Chord { id, k, forward } => {
                let t = if forward { m } else { k - m };
                let ((sa, ma), (sb, mb)) = self.chords[id as usize];
                if t == 0 {
                    self.outer(sa as usize, ma)
                } else if t >= k {
                    self.outer(sb as usize, mb)
                } else {
                    let (a, b) = (self.boundary(sa as usize, ma).1, self.boundary(sb as usize, mb).1);
                    self.id(Key::Chord { id, t }, a.lerp(b, t as f64 / k as f64))
                }
            }
        }
    }

    /// Fills polygon `poly` with a Grid or a Center, if one fits.
    fn fill(&mut self, poly: &[Edge]) -> Option<PatternKind> {
        let l: Vec<u32> = poly.iter().map(|&e| Self::edge_len(e)).collect();
        let kind = fillable(&l)?;
        match kind {
            PatternKind::Grid => self.grid(poly),
            _ => self.center(poly, &center_splits(&l)?),
        }
        Some(kind)
    }

    fn grid(&mut self, poly: &[Edge]) {
        let region = self.regions;
        self.regions += 1;
        let (nu, nv) = (Self::edge_len(poly[0]), Self::edge_len(poly[1]));
        let c: Vec<DVec2> = (0..4)
            .map(|i| {
                let id = self.edge_point(poly[i], 0);
                self.points[id as usize].1
            })
            .collect();
        let mut ids = vec![0u32; ((nu + 1) * (nv + 1)) as usize];
        for j in 0..=nv {
            for i in 0..=nu {
                let id = if j == 0 {
                    self.edge_point(poly[0], i)
                } else if i == nu {
                    self.edge_point(poly[1], j)
                } else if j == nv {
                    self.edge_point(poly[2], nu - i)
                } else if i == 0 {
                    self.edge_point(poly[3], nv - j)
                } else {
                    let (u, v) = (i as f64 / nu as f64, j as f64 / nv as f64);
                    let pos = c[0].lerp(c[1], u).lerp(c[3].lerp(c[2], u), v);
                    self.id(Key::Cell { region, sub: 0, a: i, b: j }, pos)
                };
                ids[(j * (nu + 1) + i) as usize] = id;
            }
        }
        for j in 0..nv {
            for i in 0..nu {
                let at = |i: u32, j: u32| ids[(j * (nu + 1) + i) as usize];
                self.quads.push([at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1)]);
            }
        }
    }

    /// The Center pattern over polygon `poly` (its edges in order) with the
    /// given splits.
    fn center(&mut self, poly: &[Edge], splits: &[(u32, u32)]) {
        let region = self.regions;
        self.regions += 1;
        let k = poly.len();
        let corners: Vec<u32> = (0..k).map(|i| self.edge_point(poly[i], 0)).collect();
        let centroid = corners.iter().map(|&c| self.points[c as usize].1).sum::<DVec2>() / k as f64;
        let center = self.id(Key::Center(region), centroid);
        let split_point: Vec<u32> = (0..k).map(|i| self.edge_point(poly[i], splits[i].0)).collect();
        // Ray i from the center to split point i has x_{i+1} edges.
        let ray = |b: &mut Self, i: usize, t: u32| -> u32 {
            let len = splits[(i + 1) % k].0;
            if t == 0 {
                center
            } else if t >= len {
                split_point[i]
            } else {
                let (c, p) = (b.points[center as usize].1, b.points[split_point[i] as usize].1);
                b.id(Key::Ray { region, side: i as u32, k: t }, c.lerp(p, t as f64 / len as f64))
            }
        };
        for i in 0..k {
            let j = (i + 1) % k;
            // u: center → p_i (x_{i+1} edges); v: center → p_j (y_i edges).
            let (nu, nv) = (splits[j].0, splits[i].1);
            let corner_pos = self.points[corners[j] as usize].1;
            let (c, pi, pj) = (
                self.points[center as usize].1,
                self.points[split_point[i] as usize].1,
                self.points[split_point[j] as usize].1,
            );
            let mut ids = vec![0u32; ((nu + 1) * (nv + 1)) as usize];
            for b in 0..=nv {
                for a in 0..=nu {
                    let id = if b == 0 {
                        ray(self, i, a)
                    } else if a == 0 {
                        ray(self, j, b)
                    } else if a == nu {
                        // Along edge i from p_i toward corner j.
                        self.edge_point(poly[i], splits[i].0 + b)
                    } else if b == nv {
                        // Along edge j from p_j back toward corner j.
                        self.edge_point(poly[j], splits[j].0 - a)
                    } else {
                        let (u, v) = (a as f64 / nu as f64, b as f64 / nv as f64);
                        let pos = c.lerp(pi, u).lerp(pj.lerp(corner_pos, u), v);
                        self.id(Key::Cell { region, sub: i as u32, a, b }, pos)
                    };
                    ids[(b * (nu + 1) + a) as usize] = id;
                }
            }
            for b in 0..nv {
                for a in 0..nu {
                    let at = |a: u32, b: u32| ids[(b * (nu + 1) + a) as usize];
                    self.quads.push([at(a, b), at(a + 1, b), at(a + 1, b + 1), at(a, b + 1)]);
                }
            }
        }
    }

    /// Boundary positions in order: `(side, m)` for every corner and sample.
    fn ring(&self) -> Vec<(u8, u32)> {
        (0..self.n).flat_map(|i| (0..self.l[i]).map(move |m| (i as u8, m))).collect()
    }

    /// The edges of the boundary run from ring position `a` to ring position
    /// `b` (counter-clockwise), cut at every patch corner.
    fn run(&self, ring: &[(u8, u32)], a: usize, b: usize) -> Vec<Edge> {
        let t = ring.len();
        let mut edges = Vec::new();
        let (mut side, mut from) = ring[a];
        let mut steps = (b + t - a) % t;
        while steps > 0 {
            let left = self.l[side as usize] - from;
            let len = left.min(steps as u32);
            edges.push(Edge::Run { side, from, len });
            steps -= len as usize;
            side = (side + 1) % self.n as u8;
            from = 0;
        }
        edges
    }

    /// Splits the patch along the best chord into two fillable pieces.
    /// Position of ring point `i` in count space: one unit per boundary edge.
    /// A 4-sided patch is laid out as a rectangle of its mean side counts (a
    /// long thin patch stays long and thin), other patches as a regular polygon.
    fn count_position(&self, ring: &[(u8, u32)], i: usize) -> DVec2 {
        let (side, m) = (ring[i].0 as usize, ring[i].1 as f64);
        let l: Vec<f64> = self.l.iter().map(|&v| v as f64).collect();
        if self.n == 4 {
            let (w, h) = (0.5 * (l[0] + l[2]), 0.5 * (l[1] + l[3]));
            return match side {
                0 => DVec2::new(w * m / l[0], 0.0),
                1 => DVec2::new(w, h * m / l[1]),
                2 => DVec2::new(w - w * m / l[2], h),
                _ => DVec2::new(0.0, h - h * m / l[3]),
            };
        }
        let perimeter: f64 = (0..self.n).map(|i| (corner(self.n, i + 1) - corner(self.n, i)).length()).sum();
        self.boundary(side, ring[i].1).1 * (l.iter().sum::<f64>() / perimeter)
    }

    fn split(&mut self) -> bool {
        let ring = self.ring();
        let t = ring.len();
        let mut best: Option<(f64, usize, usize, u32)> = None;
        for a in 0..t {
            for b in (a + 1)..t {
                let (run_ab, run_ba) = (self.run(&ring, a, b), self.run(&ring, b, a));
                // Each piece needs at least two boundary edges (a patch corner inside its run).
                if run_ab.len() < 2 || run_ba.len() < 2 || run_ab.len() > 5 || run_ba.len() > 5 {
                    continue;
                }
                let ideal = (self.count_position(&ring, a) - self.count_position(&ring, b)).length();
                // Every chord length is tried (the closed forms decide), up to half the boundary.
                let mut la: Vec<u32> = run_ab.iter().map(|&e| Self::edge_len(e)).chain([0]).collect();
                let mut lb: Vec<u32> = run_ba.iter().map(|&e| Self::edge_len(e)).chain([0]).collect();
                for k in 1..=(t as u32 / 2).max(1) {
                    if let Some(last) = la.last_mut() {
                        *last = k;
                    }
                    if let Some(last) = lb.last_mut() {
                        *last = k;
                    }
                    let (Some(ka), Some(kb)) = (fillable(&la), fillable(&lb)) else { continue };
                    // Irregular vertices: a Center's middle, and a patch corner the chord splits.
                    let irregular = [ka, kb].iter().filter(|&&k| k == PatternKind::Center).count()
                        + [a, b].iter().filter(|&&i| ring[i].1 == 0).count();
                    let score = 10.0 * irregular as f64 + (k as f64 - ideal).abs();
                    if best.is_none_or(|(s, ..)| score < s - 1e-12) {
                        best = Some((score, a, b, k));
                    }
                }
            }
        }
        let Some((_, a, b, k)) = best else { return false };
        let id = self.chords.len() as u32;
        self.chords.push((ring[a], ring[b]));
        let mut piece_a = self.run(&ring, a, b);
        piece_a.push(Edge::Chord { id, k, forward: false });
        let mut piece_b = self.run(&ring, b, a);
        piece_b.push(Edge::Chord { id, k, forward: true });
        self.fill(&piece_a).is_some() && self.fill(&piece_b).is_some()
    }

    /// One quad for a 4-edge boundary, else a center vertex and a quad per two
    /// boundary edges.
    fn fan(&mut self) {
        let mut ring = Vec::new();
        for i in 0..self.n {
            for m in 0..self.l[i] {
                ring.push(self.outer(i, m));
            }
        }
        if ring.len() == 4 {
            self.quads.push([ring[0], ring[1], ring[2], ring[3]]);
            return;
        }
        let center = self.id(Key::Center(u32::MAX), DVec2::ZERO);
        let t = ring.len();
        for j in (0..t).step_by(2) {
            self.quads.push([center, ring[j], ring[(j + 1) % t], ring[(j + 2) % t]]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(p: &Pattern, l: &[u32]) {
        let n = l.len();
        // Euler: a disk has V − E + F = 1; with quads, E = (4F + boundary)/2.
        let boundary: u32 = l.iter().sum();
        let edges = (4 * p.quads.len() as u32 + boundary) / 2;
        assert_eq!(p.points.len() as i64 - edges as i64 + p.quads.len() as i64, 1, "{l:?} {:?}", p.kind);
        // Every boundary sample and corner present exactly once.
        for (i, &len) in l.iter().enumerate().take(n) {
            assert!(p.points.iter().any(|(pt, _)| *pt == Point::Corner(i as u8)));
            for m in 1..len {
                assert_eq!(p.points.iter().filter(|(pt, _)| *pt == Point::Side(i as u8, m)).count(), 1);
            }
        }
        // Quads are counter-clockwise (positive area) in the domain.
        for q in &p.quads {
            let v: Vec<DVec2> = q.iter().map(|&i| p.points[i as usize].1).collect();
            let area: f64 = (0..4).map(|k| v[k].perp_dot(v[(k + 1) % 4])).sum::<f64>() * 0.5;
            assert!(area > 0.0, "{l:?} {:?} quad {q:?} area {area}", p.kind);
        }
    }

    #[test]
    fn grid_for_balanced_quads() {
        let p = build(&[3, 2, 3, 2]).unwrap();
        assert_eq!(p.kind, PatternKind::Grid);
        assert_eq!(p.quads.len(), 6);
        check(&p, &[3, 2, 3, 2]);
    }

    #[test]
    fn center_for_triangles_pentagons_hexagons() {
        for l in
            [vec![2, 2, 2], vec![4, 3, 3], vec![2, 2, 2, 2, 2], vec![3, 4, 3, 4, 2], vec![2, 2, 2, 2, 2, 2]]
        {
            let p = build(&l).unwrap();
            assert_eq!(p.kind, PatternKind::Center, "{l:?}");
            check(&p, &l);
        }
    }

    #[test]
    fn closed_form_matches_tarini() {
        // x0 = (4 − 3 + 3)/2 = 2, x2 = l0 − x0 = 2, x1 = l2 − x2 = 1.
        assert_eq!(center_splits(&[4, 3, 3]), Some(vec![(2, 2), (1, 2), (2, 1)]));
        assert_eq!(center_splits(&[1, 1, 4]), None);
        assert_eq!(center_splits(&[3, 3, 3]), None); // odd total
    }

    #[test]
    fn unbalanced_quads_split() {
        // (4,2,2,2) = a 2×2 grid plus the closed-form triangle (2,2,2).
        // (3,47,1,47) = a 1×47 grid plus the closed-form triangle (47,2,47).
        for l in [
            vec![4, 2, 2, 2],
            vec![6, 3, 2, 3],
            vec![5, 3, 3, 3],
            vec![8, 4, 4, 4],
            vec![3, 47, 1, 47],
            vec![2, 27, 4, 27],
        ] {
            let p = build(&l).unwrap();
            assert_eq!(p.kind, PatternKind::Split, "{l:?}");
            check(&p, &l);
        }
    }

    #[test]
    fn small_and_hard_cases_still_fill() {
        for l in [vec![1, 1, 2], vec![1, 1, 3, 1], vec![2, 1, 1, 2], vec![1, 1, 1, 1, 2], vec![9, 1, 1, 1]] {
            let p = build(&l).unwrap();
            check(&p, &l);
        }
        assert_eq!(build(&[1, 1, 2]).unwrap().quads.len(), 1);
    }

    #[test]
    fn odd_boundary_has_no_pattern() {
        assert!(build(&[2, 1, 2, 2]).is_none());
    }
}
