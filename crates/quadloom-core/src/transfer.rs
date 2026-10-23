//! Mapping a result back onto its source surface, for attribute transfer
//! (decision 0019).
//!
//! The mapping knows nothing about attributes. Each result vertex, corner
//! and face gets a [`Sample`]: a source triangle and barycentric weights. The
//! caller gathers whatever values it has at that triangle's corners. Two
//! optional keys from the caller make the mapping exact where values jump:
//! corner keys mark where corner values (UVs, say) are continuous, so a
//! result face's corners are never taken from across a seam, and face keys
//! let a result face's samples vote on which source face value it takes.

use glam::DVec3;
use hashbrown::HashMap;
use rayon::prelude::*;

use crate::control::{Control, Stage};
use crate::error::Error;
use crate::spatial::{TriangleBvh, closest_point_on_triangle};

/// Result faces handled between progress reports and cancel checks.
const CHUNK: usize = 4096;

/// A corner's search starts this far from it towards its face's centre, so
/// it starts on the face's own side of any seam through the corner.
const INSET: f64 = 0.2;

/// Triangles a corner's local search looks at, at most.
const MAX_VISITS: usize = 256;

/// The source surface.
#[derive(Clone, Copy, Debug)]
pub struct Source<'a> {
    pub positions: &'a [DVec3],
    pub triangles: &'a [[u32; 3]],
    /// Three per triangle: equal keys at a shared edge's two ends, on both
    /// sides, mean the corner values are continuous across it.
    pub corner_keys: Option<&'a [u32]>,
    /// One per triangle: the source face's values, as one key.
    pub face_keys: Option<&'a [u32]>,
}

/// The result mesh: polygons as `offsets` into `corners`.
#[derive(Clone, Copy, Debug)]
pub struct Target<'a> {
    pub positions: &'a [DVec3],
    pub offsets: &'a [u32],
    pub corners: &'a [u32],
}

/// A point on the source: a triangle and the weights of its three corners.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample {
    pub triangle: u32,
    pub weights: [f32; 3],
}

/// Where each part of the result comes from.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TransferMap {
    /// One per result vertex (point values).
    pub vertices: Vec<Sample>,
    /// One per result corner, in `corners` order (corner values).
    pub corners: Vec<Sample>,
    /// One source triangle per result face (face values).
    pub faces: Vec<u32>,
}

/// Maps `target` onto `source`. Reports progress as [`Stage::Transfer`].
pub fn map(source: Source<'_>, target: Target<'_>, control: &Control) -> Result<TransferMap, Error> {
    check(source, target)?;
    control.checkpoint(Stage::Transfer)?;
    let surface = Surface::new(source);
    let face_count = target.offsets.len() - 1;
    let face_normals: Vec<DVec3> = (0..face_count).map(|f| newell(target, f)).collect();
    let mut vertex_normals = vec![DVec3::ZERO; target.positions.len()];
    for (f, &n) in face_normals.iter().enumerate() {
        for &v in polygon(target, f) {
            vertex_normals[v as usize] += n;
        }
    }

    // Faces (and their corners) in chunks, so cancel is checked between them.
    let mut faces = Vec::with_capacity(face_count);
    let mut corners = Vec::with_capacity(target.corners.len());
    let total = face_count + target.positions.len();
    for start in (0..face_count).step_by(CHUNK) {
        control.checkpoint(Stage::Transfer)?;
        control.set_progress(Stage::Transfer, start as f32 / total as f32);
        let end = (start + CHUNK).min(face_count);
        let done: Vec<(u32, Vec<Sample>)> =
            (start..end).into_par_iter().map(|f| surface.face(target, f, face_normals[f])).collect();
        for (triangle, samples) in done {
            faces.push(triangle);
            corners.extend(samples);
        }
    }
    let mut vertices = Vec::with_capacity(target.positions.len());
    for start in (0..target.positions.len()).step_by(CHUNK) {
        control.checkpoint(Stage::Transfer)?;
        control.set_progress(Stage::Transfer, (face_count + start) as f32 / total as f32);
        let end = (start + CHUNK).min(target.positions.len());
        vertices.par_extend(
            (start..end)
                .into_par_iter()
                .map(|v| surface.locate(target.positions[v], vertex_normals[v], |_| true)),
        );
    }
    control.set_progress(Stage::Transfer, 1.0);
    Ok(TransferMap { vertices, corners, faces })
}

fn check(source: Source<'_>, target: Target<'_>) -> Result<(), Error> {
    let bad = |message: String| Err(Error::invalid_input(Stage::Transfer, message));
    if source.triangles.is_empty() {
        return bad("the source has no triangles".into());
    }
    let n = source.positions.len() as u32;
    if source.triangles.iter().flatten().any(|&v| v >= n) {
        return bad("a source triangle refers to a vertex that doesn't exist".into());
    }
    if let Some(keys) = source.corner_keys
        && keys.len() != 3 * source.triangles.len()
    {
        return bad(format!(
            "{} corner keys for {} triangles (need 3 each)",
            keys.len(),
            source.triangles.len()
        ));
    }
    if let Some(keys) = source.face_keys
        && keys.len() != source.triangles.len()
    {
        return bad(format!("{} face keys for {} triangles", keys.len(), source.triangles.len()));
    }
    let (offsets, corners) = (target.offsets, target.corners);
    if offsets.first() != Some(&0)
        || offsets.last().map(|&o| o as usize) != Some(corners.len())
        || offsets.windows(2).any(|w| w[1] < w[0] + 3)
    {
        return bad("the result's face offsets are malformed".into());
    }
    let m = target.positions.len() as u32;
    if corners.iter().any(|&v| v >= m) {
        return bad("a result face refers to a vertex that doesn't exist".into());
    }
    Ok(())
}

/// The source with its search structure, normals and seams.
struct Surface<'a> {
    source: Source<'a>,
    bvh: TriangleBvh,
    normals: Vec<DVec3>,
    /// Neighbours across edges the corner values are continuous over.
    smooth: Option<Neighbours>,
}

impl<'a> Surface<'a> {
    fn new(source: Source<'a>) -> Self {
        let p = source.positions;
        let normals = source
            .triangles
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|v| p[v as usize]);
                (b - a).cross(c - a).normalize_or_zero()
            })
            .collect();
        let smooth = source.corner_keys.map(|keys| Neighbours::continuous(source.triangles, keys));
        Self { source, bvh: TriangleBvh::new(p, source.triangles), normals, smooth }
    }

    fn corners_of(&self, t: u32) -> [DVec3; 3] {
        self.source.triangles[t as usize].map(|v| self.source.positions[v as usize])
    }

    /// The closest point to `p` on a triangle that `accept` lets through,
    /// preferring triangles facing along `normal` (so a thin part doesn't
    /// borrow its far side).
    fn locate(&self, p: DVec3, normal: DVec3, accept: impl Fn(u32) -> bool) -> Sample {
        let facing = |t: u32| accept(t) && self.normals[t as usize].dot(normal) > 0.0;
        let found = self
            .bvh
            .closest_triangle_where(p, facing)
            .or_else(|| self.bvh.closest_triangle_where(p, &accept))
            .or_else(|| self.bvh.closest_triangle_where(p, |_| true));
        // The source has triangles (checked), so some search finds one.
        let (q, _, t) = found.unwrap_or((p, 0.0, 0));
        let [a, b, c] = self.corners_of(t);
        Sample { triangle: t, weights: barycentric(q, a, b, c) }
    }

    /// Corner `p` of a face whose inside is towards `centre`: the closest
    /// source point to `p` reachable from the face's own side without
    /// crossing a seam. The search starts at the source triangle under a
    /// point inset from `p` and walks over continuous edges, only as far as
    /// could still beat the start.
    fn corner(&self, smooth: &Neighbours, p: DVec3, centre: DVec3, normal: DVec3) -> Sample {
        let start = self.locate(p + INSET * (centre - p), normal, |_| true).triangle;
        let closest = |t: u32| {
            let [a, b, c] = self.corners_of(t);
            let q = closest_point_on_triangle(p, a, b, c);
            (q.distance_squared(p), q)
        };
        let (mut best_d, mut best_q) = closest(start);
        let mut best = start;
        // 1.5 times the start's distance (squared), as slack for bends.
        let reach = 2.25 * best_d + f64::EPSILON;
        let mut seen = vec![start];
        let mut queue = vec![start];
        while let Some(t) = queue.pop() {
            for &n in smooth.of(t) {
                if seen.len() >= MAX_VISITS || seen.contains(&n) {
                    continue;
                }
                seen.push(n);
                let (d, q) = closest(n);
                if d > reach {
                    continue;
                }
                if d < best_d {
                    (best_d, best_q, best) = (d, q, n);
                }
                queue.push(n);
            }
        }
        let [a, b, c] = self.corners_of(best);
        Sample { triangle: best, weights: barycentric(best_q, a, b, c) }
    }

    /// Face `f`'s source triangle (by a vote of face keys) and its corners'
    /// samples (from its own side of any seam).
    fn face(&self, target: Target<'_>, f: usize, normal: DVec3) -> (u32, Vec<Sample>) {
        let ring = polygon(target, f);
        let points: Vec<DVec3> = ring.iter().map(|&v| target.positions[v as usize]).collect();
        let centre = points.iter().sum::<DVec3>() / points.len() as f64;
        // The centre first, so it wins ties and represents its key.
        let samples: Vec<u32> = std::iter::once(centre)
            .chain(points.iter().map(|&p| 0.5 * (centre + p)))
            .map(|s| self.locate(s, normal, |_| true).triangle)
            .collect();
        let key = |t: u32| self.source.face_keys.map_or(t, |k| k[t as usize]);
        let mut votes: HashMap<u32, usize> = HashMap::new();
        for &t in &samples {
            *votes.entry(key(t)).or_default() += 1;
        }
        let most = votes.values().copied().max().unwrap_or(0);
        let face = samples.iter().copied().find(|&t| votes[&key(t)] == most).unwrap_or(samples[0]);
        let corners = match &self.smooth {
            Some(smooth) => points.iter().map(|&p| self.corner(smooth, p, centre, normal)).collect(),
            None => points.iter().map(|&p| self.locate(p, normal, |_| true)).collect(),
        };
        (face, corners)
    }
}

/// Each triangle's neighbours, in compressed rows.
#[derive(Debug)]
struct Neighbours {
    start: Vec<u32>,
    list: Vec<u32>,
}

impl Neighbours {
    /// Neighbours across edges whose corner keys match at both ends on both
    /// sides: the corner values are continuous there.
    fn continuous(triangles: &[[u32; 3]], keys: &[u32]) -> Self {
        // Each edge's first triangle, with its keys at the (low, high) vertex.
        let mut first: HashMap<(u32, u32), (u32, u32, u32)> = HashMap::with_capacity(triangles.len() * 3 / 2);
        let mut pairs = Vec::new();
        for (t, tri) in triangles.iter().enumerate() {
            for i in 0..3 {
                let j = (i + 1) % 3;
                let (a, b, ka, kb) = (tri[i], tri[j], keys[3 * t + i], keys[3 * t + j]);
                let (edge, ends) = if a < b { ((a, b), (ka, kb)) } else { ((b, a), (kb, ka)) };
                match first.get(&edge) {
                    Some(&(other, k0, k1)) if (k0, k1) == ends => {
                        pairs.extend([(other, t as u32), (t as u32, other)])
                    }
                    Some(_) => {}
                    None => {
                        first.insert(edge, (t as u32, ends.0, ends.1));
                    }
                }
            }
        }
        pairs.sort_unstable();
        let mut start = vec![0u32; triangles.len() + 1];
        for &(t, _) in &pairs {
            start[t as usize + 1] += 1;
        }
        for t in 0..triangles.len() {
            start[t + 1] += start[t];
        }
        Self { start, list: pairs.into_iter().map(|(_, n)| n).collect() }
    }

    fn of(&self, t: u32) -> &[u32] {
        &self.list[self.start[t as usize] as usize..self.start[t as usize + 1] as usize]
    }
}

fn polygon<'t>(target: Target<'t>, f: usize) -> &'t [u32] {
    &target.corners[target.offsets[f] as usize..target.offsets[f + 1] as usize]
}

/// Face `f`'s normal, scaled by its area (Newell's method; any polygon).
fn newell(target: Target<'_>, f: usize) -> DVec3 {
    let ring = polygon(target, f);
    let mut n = DVec3::ZERO;
    for (i, &v) in ring.iter().enumerate() {
        let (a, b) = (target.positions[v as usize], target.positions[ring[(i + 1) % ring.len()] as usize]);
        n += DVec3::new((a.y - b.y) * (a.z + b.z), (a.z - b.z) * (a.x + b.x), (a.x - b.x) * (a.y + b.y));
    }
    0.5 * n
}

/// Weights of `q` (a point on triangle `abc`) at the three corners. A
/// degenerate triangle gives its nearest corner all the weight.
fn barycentric(q: DVec3, a: DVec3, b: DVec3, c: DVec3) -> [f32; 3] {
    let (ab, ac, aq) = (b - a, c - a, q - a);
    let (d00, d01, d11) = (ab.dot(ab), ab.dot(ac), ac.dot(ac));
    let (d20, d21) = (aq.dot(ab), aq.dot(ac));
    let denom = d00 * d11 - d01 * d01;
    if denom <= f64::EPSILON * d00 * d11 || !denom.is_finite() {
        let d = [a, b, c].map(|x| x.distance_squared(q));
        let nearest = (0..3).min_by(|&i, &j| d[i].total_cmp(&d[j])).unwrap_or(0);
        let mut w = [0.0; 3];
        w[nearest] = 1.0;
        return w;
    }
    let v = ((d11 * d20 - d01 * d21) / denom).clamp(0.0, 1.0);
    let w = ((d00 * d21 - d01 * d20) / denom).clamp(0.0, 1.0);
    let (v, w) = if v + w > 1.0 { (v / (v + w), w / (v + w)) } else { (v, w) };
    [(1.0 - v - w) as f32, v as f32, w as f32]
}

#[cfg(test)]
mod tests;
