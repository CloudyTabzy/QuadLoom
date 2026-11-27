//! Quads round thin parts (decision 0026).
//!
//! Quad size follows the model's size and, by Adaptive Detail, its
//! triangles' size. Neither knows how thick a part is: at a low Density or
//! Adaptive Detail a tail a quad thick got two or three quads round it, and
//! no layout holds a tube's shape with that. [`cap`] lowers the quad-size
//! multiplier where the surface turns all the way round within a quad or
//! two, so [`QUADS_PER_TURN`] quads go round it, and lets the size grow back
//! gradually on the surface beside it.
//!
//! Whether it turns all the way round, and how thick it is there, are read
//! from the normals within a ball sized by the quads ([`Ball::thickness`]),
//! not from curvature at a point: the ball may be many triangles wide, and a
//! low-poly tube is flats and creases at any one point. A tube or a blob
//! has normals pointing every way round; a plate has two ways and a thin
//! rim between them, a fold or a ripple a fan in one plane. Creases are not
//! crossed, so a bar with sharp edges is four flat faces and keeps its one
//! quad a face.

use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::f64::consts::TAU;

use glam::DVec3;
use hashbrown::HashSet;
use rayon::prelude::*;

/// Quads round a full turn of the surface, at least: half the prep remesh's
/// twelve triangle edges ([`crate::remesh::EDGES_PER_TURN`]).
pub const QUADS_PER_TURN: f64 = 6.0;
/// The cap makes quads no smaller than this share of the quad size, so a
/// hair can't ask for thousands.
const SMALLEST: f64 = 0.2;
/// The surface within this many quad edges of a vertex decides whether it
/// turns all the way round there: a tube with fewer than six quads round it
/// is under two quad edges across and lies in that ball whole.
const REACH: f64 = 2.0;
/// Balls of that radius halved this many times are tried first, smallest
/// first ([`Ball::round`]): down to a tube an eighth of a quad edge across.
const HALVINGS: u32 = 3;
/// The surface is read at vertices this share of a quad edge apart, and
/// each vertex goes by the nearest ([`Surface::readers`]). Under quads many
/// triangles wide the balls are many triangles wide too; read at every
/// vertex they would cost the square of that.
const SPACING: f64 = 0.25;
/// At most this many vertices in a ball: all of one under quads forty
/// triangles wide.
const MAX_BALL: usize = 1 << 16;
/// How round the normals in a ball must be spread ([`Ball::thickness`]): a
/// full tube has 0.25 and a ball 0.33; three quarters of a tube 0.18, a
/// dome 0.17, half a tube 0.05, a plate with its rim next to nothing. At
/// 0.2 it takes a full turn round an axis, or nearly: a stud or a bump is
/// detail, not a thin part.
const ROUND: f64 = 0.2;
/// A capped size grows back by this share of a quad edge per quad edge of
/// distance: a quad's neighbour is at most about a third longer than it.
const GROWTH: f64 = 0.3;

/// Lowers `local`, the quad-size multiplier of each triangle, where the
/// surface turns all the way round within about two quad edges, so
/// [`QUADS_PER_TURN`] quads go round it, and grades the sizes beside such a
/// place. Returns how many triangles it lowered.
///
/// `feature_edges` are the mesh's creases and borders, as vertex pairs;
/// `unit` is the length of a quad edge at multiplier 1.
pub fn cap(
    local: &mut [f64],
    positions: &[DVec3],
    triangles: &[[u32; 3]],
    feature_edges: &[[u32; 2]],
    unit: f64,
) -> usize {
    let vertices = positions.len();
    if local.len() != triangles.len()
        || unit.is_nan()
        || unit <= 0.0
        || triangles.iter().flatten().any(|&v| v as usize >= vertices)
    {
        return 0;
    }
    let surface = Surface::new(positions, triangles, feature_edges, local, unit);
    let reader = surface.readers();
    let read: Vec<Option<f64>> = (0..vertices as u32)
        .into_par_iter()
        .map_init(Ball::default, |ball, v| {
            (reader[v as usize] == v).then(|| ball.round(&surface, v, REACH * surface.edge[v as usize]))?
        })
        .collect();
    let caps: Vec<Option<f64>> = (0..vertices)
        .map(|v| {
            let radius = (*read.get(reader[v] as usize)?)?;
            let longest = TAU * radius / QUADS_PER_TURN;
            (longest < surface.edge[v]).then(|| (longest / unit).max(SMALLEST))
        })
        .collect();
    grade(local, &surface, triangles, &caps, unit)
}

/// What [`cap`] reads at each vertex of the mesh.
struct Surface<'a> {
    positions: &'a [DVec3],
    rings: Rings,
    /// On a crease or a border: not looked at, and not crossed.
    crease: Vec<bool>,
    /// The unit normal (zero where the faces round it cancel).
    normal: Vec<DVec3>,
    /// A third of the area of the faces round it.
    area: Vec<f64>,
    /// The quad edge asked for there; infinite at a vertex no face uses.
    edge: Vec<f64>,
}

impl<'a> Surface<'a> {
    fn new(
        positions: &'a [DVec3],
        triangles: &[[u32; 3]],
        feature_edges: &[[u32; 2]],
        local: &[f64],
        unit: f64,
    ) -> Self {
        let vertices = positions.len();
        let (mut normal, mut area) = (vec![DVec3::ZERO; vertices], vec![0.0; vertices]);
        let (mut size, mut faces) = (vec![0.0; vertices], vec![0u32; vertices]);
        for (t, &multiplier) in triangles.iter().zip(local) {
            let [a, b, c] = t.map(|v| positions[v as usize]);
            let n = (b - a).cross(c - a);
            for &v in t {
                normal[v as usize] += n;
                area[v as usize] += n.length() / 6.0;
                size[v as usize] += multiplier;
                faces[v as usize] += 1;
            }
        }
        let edge = (0..vertices)
            .map(|v| if faces[v] > 0 { unit * size[v] / f64::from(faces[v]) } else { f64::INFINITY })
            .collect();
        let mut crease = vec![false; vertices];
        for &v in feature_edges.iter().flatten() {
            if let Some(on) = crease.get_mut(v as usize) {
                *on = true;
            }
        }
        Self {
            positions,
            rings: Rings::new(vertices, triangles),
            crease,
            normal: normal.into_iter().map(DVec3::normalize_or_zero).collect(),
            area,
            edge,
        }
    }
}

impl Surface<'_> {
    /// Per vertex, the vertex the surface is read at for it: itself, or one
    /// within [`SPACING`] quad edges of it over the surface. `u32::MAX` on a
    /// crease and at a vertex no face uses, which are not read at all.
    ///
    /// Taken in vertex order: each vertex not yet spoken for reads for
    /// itself and for those round it that aren't.
    fn readers(&self) -> Vec<u32> {
        let mut reader = vec![u32::MAX; self.positions.len()];
        let mut queue: Vec<u32> = Vec::new();
        for v in 0..self.positions.len() as u32 {
            if reader[v as usize] != u32::MAX || self.crease[v as usize] || !self.edge[v as usize].is_finite()
            {
                continue;
            }
            let (at, radius) = (self.positions[v as usize], SPACING * self.edge[v as usize]);
            reader[v as usize] = v;
            queue.clear();
            queue.push(v);
            let mut head = 0;
            while let Some(&u) = queue.get(head) {
                head += 1;
                for &w in self.rings.of(u) {
                    if reader[w as usize] == u32::MAX
                        && !self.crease[w as usize]
                        && (self.positions[w as usize] - at).length() < radius
                    {
                        reader[w as usize] = v;
                        queue.push(w);
                    }
                }
            }
        }
        reader
    }
}

/// The neighbours of every vertex, as one flat list.
struct Rings {
    start: Vec<u32>,
    to: Vec<u32>,
}

impl Rings {
    fn new(vertices: usize, triangles: &[[u32; 3]]) -> Self {
        let mut pairs: Vec<(u32, u32)> = Vec::with_capacity(triangles.len() * 6);
        for t in triangles {
            for i in 0..3 {
                let (a, b) = (t[i], t[(i + 1) % 3]);
                if a != b {
                    pairs.extend([(a, b), (b, a)]);
                }
            }
        }
        pairs.sort_unstable();
        pairs.dedup();
        let mut start = vec![0u32; vertices + 1];
        for &(a, _) in &pairs {
            start[a as usize + 1] += 1;
        }
        for v in 0..vertices {
            start[v + 1] += start[v];
        }
        Self { start, to: pairs.into_iter().map(|(_, b)| b).collect() }
    }

    fn of(&self, v: u32) -> &[u32] {
        &self.to[self.start[v as usize] as usize..self.start[v as usize + 1] as usize]
    }
}

/// Reusable scratch for [`Ball::round`].
#[derive(Default)]
struct Ball {
    seen: HashSet<u32>,
    queue: Vec<u32>,
}

impl Ball {
    /// The radius of the part round `v`, if the surface turns all the way
    /// round within `radius` of it: a tube's or a ball's radius, half a
    /// rounded bar's thickness. `None` where it doesn't (a flat, a fold, a
    /// plate and its rim).
    ///
    /// Balls are tried smallest first, from `radius` halved [`HALVINGS`]
    /// times up to `radius` itself. The smallest that is round holds one
    /// section of the part and little of what it is attached to or bends
    /// towards, so a tail is read as a tail next to the body it grows from,
    /// and round its own curl.
    fn round(&mut self, surface: &Surface<'_>, v: u32, radius: f64) -> Option<f64> {
        (0..=HALVINGS)
            .rev()
            .find_map(|halvings| self.thickness(surface, v, radius / f64::from(1u32 << halvings)))
    }

    /// The radius of the part within `radius` of `v`, if the surface there
    /// turns all the way round.
    ///
    /// The ball is flooded from `v` through vertices off the creases, at
    /// most [`MAX_BALL`] of them. Two things are read from it, both from
    /// the whole ball, so a low-poly tube (flats and creases at any one
    /// point) reads as the tube it is:
    ///
    /// - **How round.** The normals' covariance about their mean, weighted
    ///   by area, has eigenvalues (½, ½, 0) round a tube, (⅓, ⅓, ⅓) on a
    ///   ball and (1, 0, 0) on a plate seen from both sides. The sum of
    ///   their products in pairs is 0.25, 0.33 and 0: over [`ROUND`], the
    ///   normals point every way round an axis or more.
    /// - **How thick.** On a tube or a ball of radius r about a centre c,
    ///   a point is `c + r · normal` (plus a step along the tube, which no
    ///   normal has a part of). So r is the slope of position against
    ///   normal: their covariance over the normals' variance. On a tube of
    ///   flat sides it is the distance of the sides from the axis; on a
    ///   hollow one it is negative, and its size is taken.
    fn thickness(&mut self, surface: &Surface<'_>, v: u32, radius: f64) -> Option<f64> {
        let at = surface.positions[v as usize];
        self.seen.clear();
        self.queue.clear();
        self.seen.insert(v);
        self.queue.push(v);
        let (mut area, mut normal, mut place, mut both) = (0.0, DVec3::ZERO, DVec3::ZERO, 0.0);
        // The upper triangle of Σ area · n nᵀ: xx, yy, zz, xy, xz, yz.
        let mut spread = [0.0; 6];
        let mut head = 0;
        while let Some(&u) = self.queue.get(head) {
            head += 1;
            let (n, a) = (surface.normal[u as usize], surface.area[u as usize]);
            let p = surface.positions[u as usize] - at;
            area += a;
            normal += a * n;
            place += a * p;
            both += a * p.dot(n);
            let products = [n.x * n.x, n.y * n.y, n.z * n.z, n.x * n.y, n.x * n.z, n.y * n.z];
            for (sum, product) in spread.iter_mut().zip(products) {
                *sum += a * product;
            }
            for &w in surface.rings.of(u) {
                if self.queue.len() >= MAX_BALL {
                    break;
                }
                if !surface.crease[w as usize]
                    && (surface.positions[w as usize] - at).length() < radius
                    && self.seen.insert(w)
                {
                    self.queue.push(w);
                }
            }
        }
        if area <= 0.0 {
            return None;
        }
        let (mean, centre) = (normal / area, place / area);
        let [xx, yy, zz, xy, xz, yz] = spread.map(|sum| sum / area);
        let (xx, yy, zz) = (xx - mean.x * mean.x, yy - mean.y * mean.y, zz - mean.z * mean.z);
        let (xy, xz, yz) = (xy - mean.x * mean.y, xz - mean.x * mean.z, yz - mean.y * mean.z);
        let round = xx * yy + xx * zz + yy * zz - xy * xy - xz * xz - yz * yz;
        if round.is_nan() || round < ROUND {
            return None;
        }
        let thickness = ((both / area - centre.dot(mean)) / (xx + yy + zz)).abs();
        // A part thicker than the ball that holds it is no reading.
        (thickness.is_finite() && thickness <= radius).then_some(thickness)
    }
}

/// Lowers `local` to `caps` at the capped vertices and lets it grow back
/// from there by [`GROWTH`]: per vertex, `limit(n) ≤ limit(v) + GROWTH ·
/// distance / unit` along every edge (Persson's gradient limiting, swept
/// from the smallest outward), and a triangle takes the mean of its
/// corners'. Returns how many triangles it lowered.
fn grade(
    local: &mut [f64],
    surface: &Surface<'_>,
    triangles: &[[u32; 3]],
    caps: &[Option<f64>],
    unit: f64,
) -> usize {
    struct Reached(f64, u32);
    impl PartialEq for Reached {
        fn eq(&self, other: &Self) -> bool {
            self.cmp(other) == Ordering::Equal
        }
    }
    impl Eq for Reached {}
    impl PartialOrd for Reached {
        fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
            Some(self.cmp(other))
        }
    }
    impl Ord for Reached {
        // Smallest first out of a max-heap; ties by vertex, so the sweep is repeatable.
        fn cmp(&self, other: &Self) -> Ordering {
            other.0.total_cmp(&self.0).then(other.1.cmp(&self.1))
        }
    }
    let mut limit = vec![f64::INFINITY; caps.len()];
    let mut heap = BinaryHeap::new();
    for (v, cap) in caps.iter().enumerate() {
        if let Some(cap) = *cap {
            limit[v] = cap;
            heap.push(Reached(cap, v as u32));
        }
    }
    if heap.is_empty() {
        return 0;
    }
    while let Some(Reached(here, v)) = heap.pop() {
        if here > limit[v as usize] {
            continue;
        }
        let at = surface.positions[v as usize];
        for &n in surface.rings.of(v) {
            let reached = here + GROWTH * (surface.positions[n as usize] - at).length() / unit;
            // Past the size already asked for there, the cap has faded out.
            if reached < limit[n as usize] && reached < surface.edge[n as usize] / unit {
                limit[n as usize] = reached;
                heap.push(Reached(reached, n));
            }
        }
    }
    let mut lowered = 0;
    for (multiplier, t) in local.iter_mut().zip(triangles) {
        // A corner the growth didn't reach leaves the triangle's own size there.
        let limit = t.iter().map(|&v| limit[v as usize].min(*multiplier)).sum::<f64>() / 3.0;
        if limit < *multiplier {
            *multiplier = limit;
            lowered += 1;
        }
    }
    lowered
}

#[cfg(test)]
mod tests {
    use std::f64::consts::PI;

    use super::*;
    use crate::testmesh;

    fn scaled(p: &[DVec3], across: f64) -> Vec<DVec3> {
        p.iter().map(|q| DVec3::new(across * q.x, across * q.y, q.z)).collect()
    }

    #[test]
    fn six_quads_go_round_a_thin_tube() {
        // A tube of radius 0.3, 1.88 round, under quads 1 long: two round it.
        let (p, t) = testmesh::cylinder(32, 24);
        let p = scaled(&p, 0.3);
        let mut local = vec![1.0; t.len()];
        assert_eq!(cap(&mut local, &p, &t, &[], 1.0), t.len());
        let sixth = TAU * 0.3 / 6.0;
        for (f, &size) in local.iter().enumerate() {
            assert!((size - sixth).abs() < 0.02 * sixth, "face {f}: {size} for {sixth}");
        }
        // Under quads a quarter the size it has seven round it already.
        let mut fine = vec![1.0; t.len()];
        assert_eq!(cap(&mut fine, &p, &t, &[], 0.25), 0);
        assert!(fine.iter().all(|&s| s == 1.0));
        // Sizes already under the cap stay.
        let mut small = vec![0.25; t.len()];
        assert_eq!(cap(&mut small, &p, &t, &[], 1.0), 0);
        // A hair gets quads a fifth the size and no smaller.
        let hair = scaled(&p, 1.0 / 30.0);
        let mut local = vec![1.0; t.len()];
        cap(&mut local, &hair, &t, &[], 1.0);
        assert!(local.iter().all(|&s| (s - SMALLEST).abs() < 1e-12), "{local:?}");
    }

    #[test]
    fn a_low_poly_tube_and_a_hollow_one_are_tubes_too() {
        // Eight flat sides: at any one point a flat or a crease, round the
        // ball a tube. Its vertices are 0.3 from the axis.
        let (p, t) = testmesh::cylinder(16, 8);
        let p = scaled(&p, 0.3);
        let mut local = vec![1.0; t.len()];
        assert_eq!(cap(&mut local, &p, &t, &[], 1.0), t.len());
        let sixth = TAU * 0.3 / 6.0;
        assert!(local.iter().all(|&s| (s - sixth).abs() < 0.1 * sixth), "{local:?}");
        // The same tube inside out (a hole's wall) is as thin.
        let inside: Vec<[u32; 3]> = t.iter().map(|&[a, b, c]| [a, c, b]).collect();
        let mut hollow = vec![1.0; t.len()];
        assert_eq!(cap(&mut hollow, &p, &inside, &[], 1.0), t.len());
        assert!(hollow.iter().zip(&local).all(|(a, b)| (a - b).abs() < 1e-9));
        // With its eight edges marked sharp it is eight flat faces: a bar,
        // and a quad a face will do.
        let sharp: Vec<[u32; 2]> =
            (0..16u32).flat_map(|i| (0..8u32).map(move |j| [i * 8 + j, (i + 1) * 8 + j])).collect();
        let mut bar = vec![1.0; t.len()];
        assert_eq!(cap(&mut bar, &p, &t, &sharp, 1.0), 0);
    }

    #[test]
    fn a_plate_keeps_its_quads_rounded_rim_and_all() {
        // A plate 0.1 thick and 4 wide, its rims half circles of radius
        // 0.05, under quads 1 long. The rims are as tight as a hair, but
        // each turns half way and stops: the plate is not a thin part.
        let (radius, flat) = (0.05, 4.0);
        let outline = |j: u32| {
            let turn = |step: u32| PI * f64::from(step) / 8.0 - PI / 2.0;
            match j {
                0..8 => (flat / 2.0 + radius * turn(j).cos(), radius * turn(j).sin()),
                8..24 => (flat / 2.0 - flat * f64::from(j - 8) / 16.0, radius),
                24..32 => (-flat / 2.0 - radius * turn(j - 24).cos(), -radius * turn(j - 24).sin()),
                _ => (-flat / 2.0 + flat * f64::from(j - 32) / 16.0, -radius),
            }
        };
        let (p, t) = testmesh::extrusion(48, 32, 4.0, outline);
        let mut local = vec![1.0; t.len()];
        assert_eq!(cap(&mut local, &p, &t, &[], 1.0), 0);
        assert!(local.iter().all(|&s| s == 1.0));
        // So does a flat sheet, and one rippled 0.6 high every 4 units under quads 2 long.
        let (p, t) = testmesh::grid(12, 12);
        let mut local = vec![1.0; t.len()];
        assert_eq!(cap(&mut local, &p, &t, &[], 2.0), 0);
        let rippled: Vec<DVec3> =
            p.iter().map(|q| DVec3::new(q.x, q.y, 0.6 * (TAU * q.x / 4.0).cos())).collect();
        assert_eq!(cap(&mut local, &rippled, &t, &[], 2.0), 0);
    }

    #[test]
    fn a_capped_size_grows_back_gradually() {
        // A 20 × 4 sheet of unit squares under quads 1 long, its first two
        // columns of vertices capped at 0.25: the size climbs 0.3 a unit
        // from there until it is back at what was asked.
        let (p, t) = testmesh::grid(20, 4);
        let mut local = vec![1.0; t.len()];
        // Far from the cap, sizes that differ sharply are none of its business.
        let centre = |f: usize| t[f].iter().map(|&v| p[v as usize]).sum::<DVec3>() / 3.0;
        for (f, size) in local.iter_mut().enumerate() {
            if centre(f).x > 12.0 && f % 2 == 0 {
                *size = 0.5;
            }
        }
        let before = local.clone();
        let surface = Surface::new(&p, &t, &[], &local, 1.0);
        let caps: Vec<Option<f64>> = p.iter().map(|q| (q.x <= 1.0).then_some(0.25)).collect();
        assert!(grade(&mut local, &surface, &t, &caps, 1.0) > 0);
        for f in 0..t.len() {
            let x = centre(f).x;
            if x < 1.0 {
                assert!((local[f] - 0.25).abs() < 1e-9, "face {f} at {x}: {}", local[f]);
            } else if x < 3.0 {
                let grown = 0.25 + GROWTH * (x - 1.0);
                assert!(local[f] > 0.25 && local[f] <= grown + 0.11, "face {f} at {x}: {}", local[f]);
            } else if x > 5.0 {
                assert_eq!(local[f], before[f], "face {f} at {x}");
            }
        }
        // No step between neighbours steeper than the growth allows.
        for f in 0..t.len() {
            for g in f + 1..t.len() {
                let shared = t[f].iter().filter(|v| t[g].contains(v)).count();
                if shared == 2 && centre(f).x < 5.0 && centre(g).x < 5.0 {
                    let allowed = GROWTH * (centre(f) - centre(g)).length() + 1e-9;
                    assert!((local[f] - local[g]).abs() <= allowed, "faces {f} and {g}: {local:?}");
                }
            }
        }
        // No cap, no change.
        let mut untouched = before.clone();
        assert_eq!(grade(&mut untouched, &surface, &t, &vec![None; p.len()], 1.0), 0);
        assert_eq!(untouched, before);
    }

    #[test]
    fn bad_input_changes_nothing() {
        let (p, t) = testmesh::cylinder(8, 12);
        let p = scaled(&p, 0.1);
        let mut local = vec![1.0; t.len()];
        assert_eq!(cap(&mut local, &p, &t, &[], 0.0), 0);
        assert_eq!(cap(&mut local, &p, &t, &[], f64::NAN), 0);
        assert_eq!(cap(&mut local[1..], &p, &t, &[], 1.0), 0);
        assert_eq!(cap(&mut local, &p[1..], &t, &[], 1.0), 0);
        assert!(local.iter().all(|&s| s == 1.0));
    }
}
