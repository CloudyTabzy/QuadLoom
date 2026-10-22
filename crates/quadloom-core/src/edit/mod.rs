//! A triangle mesh that can be edited in place: split, collapse and flip, with
//! feature edges and vertex kinds carried through every operation.
//!
//! Face `f` owns half-edges `3f..3f + 3`, so `next`, `prev` and `face` are
//! arithmetic; twins are found through a directed-edge map. Every operation is
//! expressed as "remove these faces, add those" and *validated before anything
//! changes* (no repeated corners, no edge used twice in the same direction), so
//! a refused operation leaves the mesh exactly as it was.

use hashbrown::{DefaultHashBuilder, HashMap, HashSet, HashTable};
use std::hash::BuildHasher;

pub use glam::DVec3;

/// Half-edges by `(from, to)`. Only the half-edge id is stored (4 bytes, not
/// a 12-byte key and value): its key is read back from the face corners.
/// That is sound because a face's corners never change once it exists (faces
/// are only appended), so the table can rehash any entry at any time.
#[derive(Clone, Debug, Default)]
struct EdgeTable {
    table: HashTable<u32>,
    hasher: DefaultHashBuilder,
}

fn ends(corners: &[[u32; 3]], h: u32) -> (u32, u32) {
    let t = corners[(h / 3) as usize];
    let i = (h % 3) as usize;
    (t[i], t[(i + 1) % 3])
}

impl EdgeTable {
    fn with_capacity(n: usize) -> Self {
        Self { table: HashTable::with_capacity(n), hasher: DefaultHashBuilder::default() }
    }

    fn len(&self) -> usize {
        self.table.len()
    }

    fn get(&self, corners: &[[u32; 3]], a: u32, b: u32) -> Option<u32> {
        self.table.find(self.hasher.hash_one((a, b)), |&h| ends(corners, h) == (a, b)).copied()
    }

    fn contains(&self, corners: &[[u32; 3]], a: u32, b: u32) -> bool {
        self.get(corners, a, b).is_some()
    }

    /// Adds half-edge `h`, whose `(from, to)` must not be in the table yet.
    fn insert(&mut self, corners: &[[u32; 3]], h: u32) {
        let hasher = &self.hasher;
        self.table
            .insert_unique(hasher.hash_one(ends(corners, h)), h, |&x| hasher.hash_one(ends(corners, x)));
    }

    fn remove(&mut self, corners: &[[u32; 3]], a: u32, b: u32) {
        if let Ok(entry) =
            self.table.find_entry(self.hasher.hash_one((a, b)), |&h| ends(corners, h) == (a, b))
        {
            entry.remove();
        }
    }

    fn keys<'a>(&'a self, corners: &'a [[u32; 3]]) -> impl Iterator<Item = (u32, u32)> + 'a {
        self.table.iter().map(move |&h| ends(corners, h))
    }
}

const NONE: u32 = u32::MAX;
/// A feature vertex whose two feature edges turn by more than this (from
/// straight) is a corner: it must never move, or smoothing would round it off.
pub const CORNER_TURN_DEG: f64 = 35.0;
/// Safety cap on fan walks, so a damaged structure can't loop forever.
const MAX_FAN: usize = 1 << 16;

/// How a vertex may move during remeshing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VertexKind {
    /// Anywhere on the surface.
    Free,
    /// Only along its feature line.
    Feature,
    /// Never (feature endpoints, junctions and sharp turns).
    Corner,
}

/// A half-edge id: half-edge `i` of face `f` is `3f + i`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HalfEdge(pub u32);

#[derive(Clone, Debug)]
pub struct EditMesh {
    positions: Vec<DVec3>,
    kinds: Vec<VertexKind>,
    vertex_alive: Vec<bool>,
    /// One outgoing half-edge per vertex (`NONE` if it has no faces).
    out: Vec<u32>,
    /// Faces using each vertex, maintained incrementally.
    face_degree: Vec<u32>,
    /// Boundary edges at each vertex, maintained incrementally.
    boundary_degree: Vec<u32>,
    corners: Vec<[u32; 3]>,
    /// Per face slot, the unit normal of the input surface the face descends
    /// from: input faces take their own; new faces inherit from the faces they
    /// replace (see [`replace_faces`](Self::replace_faces)). It never follows
    /// the current geometry, so guards can catch a fold that builds up over
    /// many small operations.
    references: Vec<DVec3>,
    face_alive: Vec<bool>,
    live_faces: usize,
    /// `(from, to)` → half-edge.
    directed: EdgeTable,
    /// Feature edges as sorted vertex pairs.
    features: HashSet<(u32, u32)>,
    /// The subset of `features` that came from the angle test alone and may be
    /// released once the surface is clean enough to judge them (see
    /// [`release_flat_soft_features`](Self::release_flat_soft_features)).
    soft: HashSet<(u32, u32)>,
}

fn key(a: u32, b: u32) -> (u32, u32) {
    (a.min(b), a.max(b))
}

impl EditMesh {
    /// Builds an editable mesh. Open boundary edges always become features;
    /// `feature_edges` that aren't mesh edges are ignored. Fails if the input
    /// isn't a manifold, consistently oriented triangle mesh.
    pub fn from_triangles(
        positions: &[DVec3],
        triangles: &[[u32; 3]],
        feature_edges: &[[u32; 2]],
    ) -> Result<Self, String> {
        let n = positions.len();
        let mut mesh = Self {
            positions: positions.to_vec(),
            kinds: vec![VertexKind::Free; n],
            vertex_alive: vec![true; n],
            out: vec![NONE; n],
            face_degree: vec![0; n],
            boundary_degree: vec![0; n],
            corners: Vec::with_capacity(triangles.len()),
            references: Vec::with_capacity(triangles.len()),
            face_alive: Vec::with_capacity(triangles.len()),
            live_faces: 0,
            directed: EdgeTable::with_capacity(triangles.len() * 3),
            features: HashSet::default(),
            soft: HashSet::default(),
        };
        for (f, t) in triangles.iter().enumerate() {
            if t.iter().any(|&v| v as usize >= n) {
                return Err(format!("face {f} references a missing vertex"));
            }
            if mesh.replace_faces(&[], &[*t]).is_none() {
                return Err(format!("face {f} {t:?} makes the mesh non-manifold or degenerate"));
            }
        }
        let boundary: Vec<(u32, u32)> = mesh
            .directed
            .keys(&mesh.corners)
            .filter(|&(a, b)| !mesh.directed.contains(&mesh.corners, b, a))
            .map(|(a, b)| key(a, b))
            .collect();
        mesh.features.extend(boundary);
        for &[a, b] in feature_edges {
            if mesh.directed.contains(&mesh.corners, a, b) || mesh.directed.contains(&mesh.corners, b, a) {
                mesh.features.insert(key(a, b));
            }
        }
        // A vertex whose faces form more than one fan (a "bowtie") would be
        // seen from only one of them by every fan walk.
        if let Some(v) =
            (0..n as u32).find(|&v| mesh.faces_around(v).len() != mesh.face_degree[v as usize] as usize)
        {
            return Err(format!("vertex {v} joins separate fans of faces (non-manifold vertex)"));
        }
        for v in 0..n as u32 {
            mesh.kinds[v as usize] = mesh.classify(v);
        }
        Ok(mesh)
    }

    // --- Accessors ---------------------------------------------------------

    pub fn position(&self, v: u32) -> DVec3 {
        self.positions[v as usize]
    }

    pub fn set_position(&mut self, v: u32, p: DVec3) {
        self.positions[v as usize] = p;
    }

    pub fn kind(&self, v: u32) -> VertexKind {
        self.kinds[v as usize]
    }

    pub fn vertex_slots(&self) -> usize {
        self.positions.len()
    }

    pub fn is_vertex_alive(&self, v: u32) -> bool {
        self.vertex_alive[v as usize] && self.out[v as usize] != NONE
    }

    pub fn face_count(&self) -> usize {
        self.live_faces
    }

    /// Live faces, in index order.
    pub fn faces(&self) -> impl Iterator<Item = u32> + '_ {
        (0..self.corners.len() as u32).filter(|&f| self.face_alive[f as usize])
    }

    pub fn face_corners(&self, f: u32) -> [u32; 3] {
        self.corners[f as usize]
    }

    pub fn face_normal(&self, f: u32) -> DVec3 {
        let [a, b, c] = self.corners[f as usize].map(|v| self.positions[v as usize]);
        (b - a).cross(c - a).normalize_or_zero()
    }

    /// The input surface's orientation that face `f` descends from (unit, or
    /// zero for a degenerate input face).
    pub fn reference_normal(&self, f: u32) -> DVec3 {
        self.references[f as usize]
    }

    /// The summed reference normals of the faces around `v`: the side of the
    /// input that `v` belongs to.
    pub fn vertex_reference(&self, v: u32) -> DVec3 {
        self.faces_around(v).iter().map(|&f| self.references[f as usize]).sum()
    }

    pub fn from(&self, h: HalfEdge) -> u32 {
        self.corners[(h.0 / 3) as usize][(h.0 % 3) as usize]
    }

    pub fn to(&self, h: HalfEdge) -> u32 {
        self.corners[(h.0 / 3) as usize][((h.0 + 1) % 3) as usize]
    }

    pub fn next(&self, h: HalfEdge) -> HalfEdge {
        HalfEdge(h.0 - h.0 % 3 + (h.0 + 1) % 3)
    }

    pub fn prev(&self, h: HalfEdge) -> HalfEdge {
        HalfEdge(h.0 - h.0 % 3 + (h.0 + 2) % 3)
    }

    pub fn face(&self, h: HalfEdge) -> u32 {
        h.0 / 3
    }

    pub fn twin(&self, h: HalfEdge) -> Option<HalfEdge> {
        self.directed.get(&self.corners, self.to(h), self.from(h)).map(HalfEdge)
    }

    pub fn find_half_edge(&self, a: u32, b: u32) -> Option<HalfEdge> {
        self.directed.get(&self.corners, a, b).map(HalfEdge)
    }

    pub fn is_feature(&self, a: u32, b: u32) -> bool {
        self.features.contains(&key(a, b))
    }

    /// Feature edges (including open boundaries) as sorted vertex pairs, in
    /// this mesh's vertex ids, sorted.
    pub fn feature_edges(&self) -> Vec<[u32; 2]> {
        let mut edges: Vec<[u32; 2]> = self.features.iter().map(|&(a, b)| [a, b]).collect();
        edges.sort_unstable();
        edges
    }

    /// Marks existing feature edges as soft (angle-detected, releasable).
    pub fn mark_soft(&mut self, edges: &[[u32; 2]]) {
        for &[a, b] in edges {
            if self.features.contains(&key(a, b)) {
                self.soft.insert(key(a, b));
            }
        }
    }

    /// Releases soft feature edges whose two sides, judged at scale `radius`,
    /// meet at less than `angle_deg` (the surface is smooth there after all),
    /// then re-derives every vertex's kind at that scale (a corner may move to
    /// a new peak). Boundary edges are never released. Returns how many edges
    /// were released.
    ///
    /// The sides are judged by the area-weighted normals of all faces within
    /// `radius` of the edge on either side, not by the edge's two faces alone:
    /// tessellation noise averages out over that neighbourhood, while a real
    /// crease (whose sides are each smooth) keeps its angle.
    pub fn release_flat_soft_features(&mut self, angle_deg: f64, radius: f64) -> usize {
        let cos_limit = angle_deg.to_radians().cos();
        let mut candidates: Vec<(u32, u32)> = self.soft.iter().copied().collect();
        candidates.sort_unstable();
        let mut released = 0;
        for (a, b) in candidates {
            let Some(h) = self.find_half_edge(a, b).or_else(|| self.find_half_edge(b, a)) else { continue };
            let Some(t) = self.twin(h) else { continue };
            if self.side_normals_cos(h, t, radius) > cos_limit {
                self.features.remove(&(a, b));
                self.soft.remove(&(a, b));
                released += 1;
            }
        }
        if released > 0 {
            self.reclassify(radius);
        }
        released
    }

    /// One half-edge per undirected edge, in index order.
    pub fn half_edges(&self) -> Vec<HalfEdge> {
        self.faces()
            .flat_map(|f| (0..3).map(move |i| HalfEdge(3 * f + i)))
            .filter(|&h| match self.twin(h) {
                None => true,
                Some(t) => h < t,
            })
            .collect()
    }

    // --- Neighbourhoods ----------------------------------------------------

    /// Outgoing half-edges of `v` in fan order; for a boundary vertex the fan
    /// starts at its outgoing boundary half-edge.
    fn fan(&self, v: u32) -> Vec<HalfEdge> {
        let Some(&start) = self.out.get(v as usize).filter(|&&h| h != NONE) else { return Vec::new() };
        let mut first = HalfEdge(start);
        for _ in 0..MAX_FAN {
            let Some(t) = self.twin(first) else { break };
            let back = self.next(t);
            if back.0 == start {
                break;
            }
            first = back;
        }
        let mut fan = vec![first];
        let mut h = first;
        while fan.len() < MAX_FAN {
            let Some(t) = self.twin(self.prev(h)) else { break };
            if t == first {
                break;
            }
            fan.push(t);
            h = t;
        }
        fan
    }

    /// Neighbouring vertices of `v`, in fan order.
    pub fn one_ring(&self, v: u32) -> std::vec::IntoIter<u32> {
        let fan = self.fan(v);
        let mut ring: Vec<u32> = fan.iter().map(|&h| self.to(h)).collect();
        if let Some(&last) = fan.last()
            && self.twin(self.prev(last)).is_none()
        {
            ring.push(self.from(self.prev(last)));
        }
        ring.into_iter()
    }

    pub fn faces_around(&self, v: u32) -> Vec<u32> {
        self.fan(v).iter().map(|&h| self.face(h)).collect()
    }

    /// Number of neighbours. On a manifold mesh that's the face count, plus one
    /// for boundary vertices (whose fan is open).
    pub fn valence(&self, v: u32) -> usize {
        (self.face_degree[v as usize] + self.boundary_degree[v as usize] / 2) as usize
    }

    pub fn is_boundary_vertex(&self, v: u32) -> bool {
        self.boundary_degree[v as usize] > 0
    }

    /// The vertex's feature neighbours.
    pub fn feature_neighbours(&self, v: u32) -> Vec<u32> {
        self.one_ring(v).filter(|&n| self.is_feature(v, n)).collect()
    }

    /// The point reached by walking from `from` through `next` along its
    /// feature line for at least `reach` (or until a junction or end).
    pub fn walk_feature(&self, from: u32, next: u32, reach: f64) -> DVec3 {
        let (mut prev, mut at) = (from, next);
        let mut travelled = (self.positions[at as usize] - self.positions[prev as usize]).length();
        for _ in 0..MAX_FAN {
            if travelled >= reach {
                break;
            }
            let ahead: Vec<u32> = self.feature_neighbours(at).into_iter().filter(|&n| n != prev).collect();
            let [step] = ahead.as_slice() else { break };
            travelled += (self.positions[*step as usize] - self.positions[at as usize]).length();
            (prev, at) = (at, *step);
        }
        self.positions[at as usize]
    }

    /// Cosine of the angle between the two sides of edge `h`/`t`, each side's
    /// normal being the mean over faces within `radius` of the edge, weighted
    /// by area × exp(−(3·distance/radius)²): noise averages out over the
    /// neighbourhood, while a side that curls away from the crease is still
    /// judged near the crease.
    ///
    /// Each side is flooded from its own face of the edge without crossing the
    /// edge's feature line (the chain of feature edges through it, out to
    /// `radius` each way), so a curving ridge keeps its two sides apart.
    /// Other feature lines are crossed: parallel tessellation-noise lines then
    /// average out. A face both floods reach (the line ends within `radius`)
    /// goes to the side of the plane through the edge it lies on.
    fn side_normals_cos(&self, h: HalfEdge, t: HalfEdge, radius: f64) -> f64 {
        const MAX_FACES: usize = 256;
        let (a, b) = (self.from(h), self.to(h));
        let (pa, pb) = (self.position(a), self.position(b));
        let (f0, f1) = (self.face(h), self.face(t));
        let mut line: HashSet<(u32, u32)> = HashSet::default();
        line.insert(key(a, b));
        for (from, start) in [(b, a), (a, b)] {
            let (mut prev, mut at, mut travelled) = (from, start, 0.0);
            for _ in 0..MAX_FAN {
                let ahead: Vec<u32> =
                    self.feature_neighbours(at).into_iter().filter(|&n| n != prev).collect();
                let [step] = ahead.as_slice() else { break };
                if !line.insert(key(at, *step)) || travelled > radius {
                    break;
                }
                travelled += (self.position(*step) - self.position(at)).length();
                (prev, at) = (at, *step);
            }
        }
        let distance = |c: DVec3| {
            let e = pb - pa;
            let s = ((c - pa).dot(e) / e.length_squared().max(f64::MIN_POSITIVE)).clamp(0.0, 1.0);
            (c - (pa + e * s)).length()
        };
        let near = |c: DVec3| distance(c) <= radius;
        let flood = |start: u32| {
            let mut seen: HashSet<u32> = HashSet::default();
            seen.insert(start);
            let mut queue = vec![start];
            while let Some(f) = queue.pop() {
                if seen.len() >= MAX_FACES {
                    break;
                }
                for i in 0..3 {
                    let e = HalfEdge(3 * f + i);
                    let Some(o) = self.twin(e) else { continue };
                    let g = self.face(o);
                    if !line.contains(&key(self.from(e), self.to(e)))
                        && near(self.centroid(g))
                        && seen.insert(g)
                    {
                        queue.push(g);
                    }
                }
            }
            seen
        };
        let (side0, side1) = (flood(f0), flood(f1));
        let axis = (pb - pa).cross(self.face_normal(f0) + self.face_normal(f1)).normalize_or_zero();
        let reference = (self.centroid(f0) - pa).dot(axis).signum();
        let mut sides = [DVec3::ZERO; 2];
        for &f in side0.iter().chain(side1.iter().filter(|f| !side0.contains(*f))) {
            let side = match (side0.contains(&f), side1.contains(&f)) {
                (true, false) => 0,
                (false, true) => 1,
                _ => usize::from((self.centroid(f) - pa).dot(axis).signum() != reference),
            };
            let [x, y, z] = self.corners[f as usize].map(|v| self.positions[v as usize]);
            let d = distance(self.centroid(f)) / (radius / 3.0).max(f64::MIN_POSITIVE);
            sides[side] += (y - x).cross(z - x) * (-d * d).exp().max(1e-12);
        }
        sides[0].normalize_or_zero().dot(sides[1].normalize_or_zero())
    }

    fn centroid(&self, f: u32) -> DVec3 {
        self.corners[f as usize].iter().map(|&v| self.positions[v as usize]).sum::<DVec3>() / 3.0
    }

    /// Re-derives every vertex's kind, judging feature turns at scale `radius`.
    ///
    /// A vertex with two feature edges is a corner when its line turns by more
    /// than [`CORNER_TURN_DEG`] between points about `radius / 2` away in each
    /// direction (so zigzag tessellation noise along a straight feature doesn't
    /// count), *and* that turn is the largest among the feature vertices within
    /// reach along the line (so only the actual corner vertex is fixed, not its
    /// neighbours whose wide view also wraps around the corner). The reach is
    /// at most a quarter of the vertex's feature line, so a loop or chain
    /// shorter than the radius is judged at its own scale.
    pub fn reclassify(&mut self, radius: f64) {
        let n = self.positions.len();
        let line_length = self.feature_line_lengths();
        let reach_of = |v: u32| (0.5 * radius).min(0.25 * line_length[v as usize]);
        let mut turn = vec![0.0f64; n];
        for v in 0..n as u32 {
            if !self.is_vertex_alive(v) {
                continue;
            }
            if let &[a, b] = self.feature_neighbours(v).as_slice() {
                let (p, reach) = (self.positions[v as usize], reach_of(v));
                let (da, db) = (self.walk_feature(v, a, reach) - p, self.walk_feature(v, b, reach) - p);
                let cos = da.normalize_or_zero().dot(db.normalize_or_zero()).clamp(-1.0, 1.0);
                turn[v as usize] = 180.0 - cos.acos().to_degrees();
            }
        }
        for v in 0..n as u32 {
            if !self.is_vertex_alive(v) {
                continue;
            }
            let ends = self.feature_neighbours(v);
            self.kinds[v as usize] = match ends.as_slice() {
                [] => VertexKind::Free,
                &[a, b] => {
                    let t = turn[v as usize];
                    let is_peak = [a, b].into_iter().all(|start| {
                        self.chain_within(v, start, reach_of(v)).iter().all(|&u| {
                            let tu = turn[u as usize];
                            tu < t || (tu == t && u > v)
                        })
                    });
                    if t > CORNER_TURN_DEG && is_peak { VertexKind::Corner } else { VertexKind::Feature }
                }
                _ => VertexKind::Corner,
            };
        }
    }

    /// For each vertex inside a feature line (feature valence 2), the length of
    /// that line between its junctions or ends, or around it if it's a loop;
    /// infinite elsewhere. Each line is walked once.
    fn feature_line_lengths(&self) -> Vec<f64> {
        let n = self.positions.len();
        let mut length = vec![f64::INFINITY; n];
        let mut done = vec![false; n];
        for v in 0..n as u32 {
            if done[v as usize] || !self.is_vertex_alive(v) {
                continue;
            }
            let &[a, b] = self.feature_neighbours(v).as_slice() else { continue };
            let mut members = vec![v];
            let mut total = 0.0;
            for start in [a, b] {
                let (mut prev, mut at) = (v, start);
                for _ in 0..MAX_FAN {
                    total += (self.positions[at as usize] - self.positions[prev as usize]).length();
                    if at == v {
                        break; // around a loop
                    }
                    let ahead: Vec<u32> =
                        self.feature_neighbours(at).into_iter().filter(|&x| x != prev).collect();
                    let [step] = ahead.as_slice() else { break };
                    members.push(at);
                    (prev, at) = (at, *step);
                }
                if at == v {
                    total *= 0.5; // both walks went all the way round
                }
            }
            for u in members {
                length[u as usize] = total;
                done[u as usize] = true;
            }
        }
        length
    }

    /// Feature-valence-2 vertices met walking from `from` through `next` for up
    /// to `reach` (stopping at junctions and ends).
    fn chain_within(&self, from: u32, next: u32, reach: f64) -> Vec<u32> {
        let (mut prev, mut at) = (from, next);
        let mut travelled = (self.positions[at as usize] - self.positions[prev as usize]).length();
        let mut chain = Vec::new();
        for _ in 0..MAX_FAN {
            if travelled > reach || at == from {
                break;
            }
            let ahead: Vec<u32> = self.feature_neighbours(at).into_iter().filter(|&n| n != prev).collect();
            let [step] = ahead.as_slice() else { break };
            chain.push(at);
            travelled += (self.positions[*step as usize] - self.positions[at as usize]).length();
            (prev, at) = (at, *step);
        }
        chain
    }

    /// Kind of `v` judged at vertex scale (its immediate feature neighbours);
    /// [`reclassify`](Self::reclassify) judges at a chosen scale.
    fn classify(&self, v: u32) -> VertexKind {
        let ends = self.feature_neighbours(v);
        match ends.as_slice() {
            [] => VertexKind::Free,
            &[a, b] => {
                let p = self.positions[v as usize];
                let (da, db) = (self.positions[a as usize] - p, self.positions[b as usize] - p);
                let cos = da.normalize_or_zero().dot(db.normalize_or_zero()).clamp(-1.0, 1.0);
                let turn = 180.0 - cos.acos().to_degrees();
                if turn > CORNER_TURN_DEG { VertexKind::Corner } else { VertexKind::Feature }
            }
            _ => VertexKind::Corner,
        }
    }

    // --- Operations ---------------------------------------------------------

    /// Replaces faces atomically. Returns the new face ids, or `None` (and
    /// changes nothing) if the result would have a repeated corner, a dead
    /// vertex, or a directed edge used twice.
    ///
    /// Each new face inherits the summed reference normals of the removed
    /// faces sharing the most corners with it (the face it was cut from, or
    /// both faces of a flip); with nothing removed it takes its own normal.
    fn replace_faces(&mut self, remove: &[u32], add: &[[u32; 3]]) -> Option<Vec<u32>> {
        // Neighbourhoods are tiny: linear scans beat hashing here.
        let removed = remove;
        let mut seen: Vec<(u32, u32)> = Vec::with_capacity(add.len() * 3);
        for t in add {
            if t[0] == t[1]
                || t[1] == t[2]
                || t[0] == t[2]
                || t.iter().any(|&v| !self.vertex_alive[v as usize])
            {
                return None;
            }
            for i in 0..3 {
                let e = (t[i], t[(i + 1) % 3]);
                let taken =
                    self.directed.get(&self.corners, e.0, e.1).is_some_and(|h| !removed.contains(&(h / 3)));
                if taken || seen.contains(&e) {
                    return None;
                }
                seen.push(e);
            }
        }

        // Vertices that lose faces and get none back need a surviving face to
        // point at; find it before anything is removed.
        let in_added: Vec<u32> = add.iter().flatten().copied().collect();
        let mut relink: Vec<(u32, u32)> = Vec::new();
        for &f in remove {
            for v in self.corners[f as usize] {
                if in_added.contains(&v) || relink.iter().any(|&(w, _)| w == v) {
                    continue;
                }
                let survivor = self.faces_around(v).into_iter().find(|g| !removed.contains(g));
                let h = survivor.map_or(NONE, |g| {
                    let i = self.corners[g as usize].iter().position(|&c| c == v).unwrap_or(0) as u32;
                    3 * g + i
                });
                relink.push((v, h));
            }
        }

        for &f in remove {
            let t = self.corners[f as usize];
            for i in 0..3 {
                let (a, b) = (t[i], t[(i + 1) % 3]);
                self.directed.remove(&self.corners, a, b);
                self.face_degree[a as usize] -= 1;
                let alone = !self.directed.contains(&self.corners, b, a);
                self.bump_boundary(a, b, alone, false);
            }
            self.face_alive[f as usize] = false;
            self.live_faces -= 1;
        }
        let mut ids = Vec::with_capacity(add.len());
        for t in add {
            let shared = |g: u32| self.corners[g as usize].iter().filter(|v| t.contains(v)).count();
            let most = remove.iter().map(|&g| shared(g)).max().unwrap_or(0);
            let reference = if most == 0 {
                let [a, b, c] = t.map(|v| self.positions[v as usize]);
                (b - a).cross(c - a)
            } else {
                remove.iter().filter(|&&g| shared(g) == most).map(|&g| self.references[g as usize]).sum()
            };
            let f = self.corners.len() as u32;
            self.corners.push(*t);
            self.references.push(reference.normalize_or_zero());
            self.face_alive.push(true);
            self.live_faces += 1;
            for i in 0..3 {
                let (a, b) = (t[i], t[(i + 1) % 3]);
                self.directed.insert(&self.corners, 3 * f + i as u32);
                self.out[a as usize] = 3 * f + i as u32;
                self.face_degree[a as usize] += 1;
                let alone = !self.directed.contains(&self.corners, b, a);
                self.bump_boundary(a, b, alone, true);
            }
            ids.push(f);
        }
        for (v, h) in relink {
            self.out[v as usize] = h;
        }
        Some(ids)
    }

    /// Keeps `boundary_degree` current when half-edge `a -> b` is added or
    /// removed; `alone` means the opposite half-edge `b -> a` doesn't exist.
    fn bump_boundary(&mut self, a: u32, b: u32, alone: bool, adding: bool) {
        let (a, b) = (a as usize, b as usize);
        // Adding a lone half-edge, or removing one side of an interior edge,
        // creates a boundary edge; the other two cases remove one.
        if adding == alone {
            self.boundary_degree[a] += 1;
            self.boundary_degree[b] += 1;
        } else {
            self.boundary_degree[a] -= 1;
            self.boundary_degree[b] -= 1;
        }
    }

    /// Splits the edge of `h` at `at`. Returns the new vertex.
    pub fn split(&mut self, h: HalfEdge, at: DVec3) -> Option<u32> {
        let (a, b, c) = (self.from(h), self.to(h), self.from(self.prev(h)));
        let twin = self.twin(h);
        let m = self.positions.len() as u32;
        self.positions.push(at);
        self.kinds.push(VertexKind::Free);
        self.vertex_alive.push(true);
        self.out.push(NONE);
        self.face_degree.push(0);
        self.boundary_degree.push(0);

        let mut remove = vec![self.face(h)];
        let mut add = vec![[a, m, c], [m, b, c]];
        if let Some(t) = twin {
            let d = self.from(self.prev(t));
            remove.push(self.face(t));
            add.extend([[b, m, d], [m, a, d]]);
        }
        if self.replace_faces(&remove, &add).is_none() {
            self.positions.pop();
            self.kinds.pop();
            self.vertex_alive.pop();
            self.out.pop();
            self.face_degree.pop();
            self.boundary_degree.pop();
            return None;
        }
        if self.features.remove(&key(a, b)) {
            self.features.insert(key(a, m));
            self.features.insert(key(m, b));
            self.kinds[m as usize] = VertexKind::Feature;
            if self.soft.remove(&key(a, b)) {
                self.soft.insert(key(a, m));
                self.soft.insert(key(m, b));
            }
        }
        Some(m)
    }

    /// Whether collapsing `h` (removing `to(h)` into `from(h)`) keeps the mesh
    /// manifold and the feature network intact.
    pub fn can_collapse(&self, h: HalfEdge) -> bool {
        let (a, b) = (self.from(h), self.to(h));
        match self.kind(b) {
            VertexKind::Corner => return false,
            VertexKind::Feature if !self.is_feature(a, b) => return false,
            _ => {}
        }
        let twin = self.twin(h);
        if twin.is_some() && self.is_boundary_vertex(a) && self.is_boundary_vertex(b) {
            return false; // would pinch two boundary stretches together
        }
        let c = self.from(self.prev(h));
        let d = twin.map(|t| self.from(self.prev(t)));
        // Link condition: the only shared neighbours are the two opposite vertices.
        let ring_a: Vec<u32> = self.one_ring(a).collect();
        let mut shared: Vec<u32> = self.one_ring(b).filter(|v| ring_a.contains(v)).collect();
        let mut expected: Vec<u32> = std::iter::once(c).chain(d).collect();
        shared.sort_unstable();
        shared.dedup();
        expected.sort_unstable();
        expected.dedup();
        if shared != expected {
            return false;
        }
        // Opposite vertices must keep enough faces to stay valid.
        [Some(c), d]
            .into_iter()
            .flatten()
            .all(|v| self.valence(v) > if self.is_boundary_vertex(v) { 2 } else { 3 })
    }

    /// Removes `to(h)` into `from(h)`, moving the survivor to `keep_position`.
    pub fn collapse(&mut self, h: HalfEdge, keep_position: DVec3) -> bool {
        self.can_collapse(h) && self.collapse_prechecked(h, keep_position)
    }

    /// [`collapse`](Self::collapse) for callers that just checked
    /// [`can_collapse`](Self::can_collapse). Topology is still validated.
    pub fn collapse_prechecked(&mut self, h: HalfEdge, keep_position: DVec3) -> bool {
        let (a, b) = (self.from(h), self.to(h));
        let around = self.faces_around(b);
        let add: Vec<[u32; 3]> = around
            .iter()
            .map(|&f| self.corners[f as usize])
            .filter(|t| !t.contains(&a))
            .map(|t| t.map(|v| if v == b { a } else { v }))
            .collect();
        let feature_ring: Vec<u32> = self.feature_neighbours(b);
        if self.replace_faces(&around, &add).is_none() {
            return false;
        }
        for x in feature_ring {
            let was_soft = self.soft.remove(&key(b, x));
            self.features.remove(&key(b, x));
            if x == a {
                continue;
            }
            // (a, x) stays soft only if every feature merged into it was soft.
            let already = self.features.contains(&key(a, x));
            let already_soft = self.soft.contains(&key(a, x));
            self.features.insert(key(a, x));
            if was_soft && (!already || already_soft) {
                self.soft.insert(key(a, x));
            } else {
                self.soft.remove(&key(a, x));
            }
        }
        self.soft.remove(&key(a, b));
        self.vertex_alive[b as usize] = false;
        self.out[b as usize] = NONE;
        self.positions[a as usize] = keep_position;
        // The survivor's line may have changed shape: a line end or junction
        // is a corner, a vertex off every line is free. Other kinds (decided at
        // scale by `reclassify`) stay.
        for v in std::iter::once(a).chain(self.feature_neighbours(a)) {
            let (valence, kind) = (self.feature_neighbours(v).len(), self.kinds[v as usize]);
            self.kinds[v as usize] = match valence {
                0 => VertexKind::Free,
                2 if kind == VertexKind::Free => VertexKind::Feature,
                2 => kind,
                _ => VertexKind::Corner,
            };
        }
        true
    }

    /// Flips the (interior, non-feature) edge of `h`.
    pub fn flip(&mut self, h: HalfEdge) -> bool {
        let Some(t) = self.twin(h) else { return false };
        let (a, b) = (self.from(h), self.to(h));
        if self.is_feature(a, b) {
            return false;
        }
        let (c, d) = (self.from(self.prev(h)), self.from(self.prev(t)));
        if c == d {
            return false;
        }
        let keeps_valid = |v: u32| self.valence(v) > if self.is_boundary_vertex(v) { 2 } else { 3 };
        if !keeps_valid(a) || !keeps_valid(b) {
            return false;
        }
        self.replace_faces(&[self.face(h), self.face(t)], &[[c, a, d], [c, d, b]]).is_some()
    }

    // --- Output and checks --------------------------------------------------

    /// Dead face slots (left behind by operations) per live face.
    pub fn waste(&self) -> f64 {
        (self.corners.len() - self.live_faces) as f64 / self.live_faces.max(1) as f64
    }

    /// A fresh copy without dead slots. Vertex kinds are carried over as they
    /// are (not re-derived), so a corner stays a corner. Returns the copy and
    /// `old vertex id → new vertex id` (`None` for dropped vertices).
    pub fn rebuilt(&self) -> Result<(EditMesh, Vec<Option<u32>>), String> {
        let (positions, triangles, features, map) = self.compact_with_map();
        let mut mesh = EditMesh::from_triangles(&positions, &triangles, &features)?;
        for (old, new) in map.iter().enumerate() {
            if let Some(new) = new {
                mesh.kinds[*new as usize] = self.kinds[old];
            }
        }
        let soft: Vec<[u32; 2]> =
            self.soft.iter().filter_map(|&(a, b)| Some([map[a as usize]?, map[b as usize]?])).collect();
        mesh.mark_soft(&soft);
        // `compact_with_map` lists faces in index order, as `from_triangles` numbered them.
        mesh.references = self.faces().map(|f| self.references[f as usize]).collect();
        Ok((mesh, map))
    }

    /// Live vertices (renumbered in index order), faces, and feature edges.
    pub fn compact(&self) -> (Vec<DVec3>, Vec<[u32; 3]>, Vec<[u32; 2]>) {
        let (positions, triangles, features, _) = self.compact_with_map();
        (positions, triangles, features)
    }

    #[allow(clippy::type_complexity)]
    fn compact_with_map(&self) -> (Vec<DVec3>, Vec<[u32; 3]>, Vec<[u32; 2]>, Vec<Option<u32>>) {
        let mut new_index = vec![NONE; self.positions.len()];
        let mut positions = Vec::new();
        for f in self.faces() {
            for v in self.corners[f as usize] {
                new_index[v as usize] = 0;
            }
        }
        for (v, slot) in new_index.iter_mut().enumerate() {
            if *slot == 0 {
                *slot = positions.len() as u32;
                positions.push(self.positions[v]);
            }
        }
        let triangles =
            self.faces().map(|f| self.corners[f as usize].map(|v| new_index[v as usize])).collect();
        let mut features: Vec<[u32; 2]> = self
            .features
            .iter()
            .filter(|&&(a, b)| new_index[a as usize] != NONE && new_index[b as usize] != NONE)
            .map(|&(a, b)| {
                let (x, y) = (new_index[a as usize], new_index[b as usize]);
                [x.min(y), x.max(y)]
            })
            .collect();
        features.sort_unstable();
        let map = new_index.iter().map(|&i| (i != NONE).then_some(i)).collect();
        (positions, triangles, features, map)
    }

    /// Verifies the structure; used by tests and debug assertions.
    pub fn check_invariants(&self) -> Result<(), String> {
        let mut faces_of: HashMap<u32, usize> = HashMap::new();
        for f in self.faces() {
            let t = self.corners[f as usize];
            if t[0] == t[1] || t[1] == t[2] || t[0] == t[2] {
                return Err(format!("face {f} repeats a corner: {t:?}"));
            }
            for i in 0..3 {
                let v = t[i];
                if !self.vertex_alive[v as usize] {
                    return Err(format!("face {f} uses dead vertex {v}"));
                }
                *faces_of.entry(v).or_default() += 1;
                if self.directed.get(&self.corners, t[i], t[(i + 1) % 3]) != Some(3 * f + i as u32) {
                    return Err(format!(
                        "half-edge {} of face {f} missing from the edge map",
                        3 * f + i as u32
                    ));
                }
            }
        }
        if self.directed.len() != 3 * self.live_faces {
            return Err(format!(
                "edge map has {} entries for {} faces",
                self.directed.len(),
                self.live_faces
            ));
        }
        for (&v, &count) in &faces_of {
            let h = self.out[v as usize];
            if h == NONE || !self.face_alive[(h / 3) as usize] || self.from(HalfEdge(h)) != v {
                return Err(format!("vertex {v} has a stale outgoing half-edge"));
            }
            let fan = self.faces_around(v).len();
            if fan != count {
                return Err(format!("vertex {v} has {count} faces but a fan of {fan} (not a single fan)"));
            }
            if self.face_degree[v as usize] as usize != count {
                return Err(format!(
                    "vertex {v}: face_degree {} but {count} faces",
                    self.face_degree[v as usize]
                ));
            }
            let ring = self.one_ring(v).len();
            if self.valence(v) != ring {
                return Err(format!("vertex {v}: valence {} but one-ring has {ring}", self.valence(v)));
            }
        }
        for &(a, b) in &self.features {
            if !self.directed.contains(&self.corners, a, b) && !self.directed.contains(&self.corners, b, a) {
                return Err(format!("feature edge ({a}, {b}) is not a mesh edge"));
            }
        }
        if let Some(&(a, b)) = self.soft.iter().find(|e| !self.features.contains(*e)) {
            return Err(format!("soft edge ({a}, {b}) is not a feature"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// An `n`×`n` grid of unit squares on z = 0, as triangles.
    fn grid(n: u32) -> (Vec<DVec3>, Vec<[u32; 3]>) {
        let mut p = Vec::new();
        for y in 0..=n {
            for x in 0..=n {
                p.push(DVec3::new(f64::from(x), f64::from(y), 0.0));
            }
        }
        let w = n + 1;
        let mut t = Vec::new();
        for y in 0..n {
            for x in 0..n {
                let a = y * w + x;
                t.push([a, a + 1, a + w + 1]);
                t.push([a, a + w + 1, a + w]);
            }
        }
        (p, t)
    }

    fn tetrahedron() -> (Vec<DVec3>, Vec<[u32; 3]>) {
        let p = vec![DVec3::ZERO, DVec3::X, DVec3::Y, DVec3::Z];
        (p, vec![[0, 2, 1], [0, 1, 3], [1, 2, 3], [2, 0, 3]])
    }

    fn half_edge_between(mesh: &EditMesh, a: u32, b: u32) -> HalfEdge {
        mesh.find_half_edge(a, b).expect("edge exists")
    }

    #[test]
    fn from_triangles_roundtrip() {
        let (p, t) = grid(2);
        let mesh = EditMesh::from_triangles(&p, &t, &[]).unwrap();
        mesh.check_invariants().unwrap();
        assert_eq!(mesh.face_count(), 8);
        let (positions, triangles, features) = mesh.compact();
        assert_eq!(positions.len(), 9);
        assert_eq!(triangles.len(), 8);
        assert_eq!(features.len(), 8, "the 8 boundary edges are features");
    }

    #[test]
    fn vertex_kinds_from_features() {
        let (p, t) = grid(2);
        let mesh = EditMesh::from_triangles(&p, &t, &[]).unwrap();
        // The square's corners are 90° turns of the boundary loop → Corner (else
        // smoothing would round them off); straight boundary vertices → Feature.
        assert_eq!(mesh.kind(4), VertexKind::Free);
        assert_eq!(mesh.kind(0), VertexKind::Corner);
        assert_eq!(mesh.kind(1), VertexKind::Feature);
        // A feature line ending inside the grid: its tip is a Corner.
        let mesh = EditMesh::from_triangles(&p, &t, &[[1, 4]]).unwrap();
        assert_eq!(mesh.kind(4), VertexKind::Corner);
        assert_eq!(mesh.kind(1), VertexKind::Corner, "boundary vertex with a third feature edge");
    }

    #[test]
    fn one_ring_and_valence() {
        let (p, t) = grid(2);
        let mesh = EditMesh::from_triangles(&p, &t, &[]).unwrap();
        let mut ring: Vec<u32> = mesh.one_ring(4).collect();
        ring.sort_unstable();
        assert_eq!(ring, vec![0, 1, 3, 5, 7, 8]);
        assert_eq!(mesh.valence(4), 6);
        assert!(!mesh.is_boundary_vertex(4));
        assert!(mesh.is_boundary_vertex(0));
        assert_eq!(mesh.valence(0), 3);
    }

    #[test]
    fn split_interior_edge_keeps_manifold() {
        let (p, t) = grid(2);
        let mut mesh = EditMesh::from_triangles(&p, &t, &[]).unwrap();
        let h = half_edge_between(&mesh, 0, 4);
        let m = mesh.split(h, DVec3::new(0.5, 0.5, 0.0)).unwrap();
        mesh.check_invariants().unwrap();
        assert_eq!(mesh.face_count(), 10);
        assert_eq!(mesh.valence(m), 4);
        assert_eq!(mesh.kind(m), VertexKind::Free);
    }

    #[test]
    fn split_feature_edge_tags_both_halves() {
        let (p, t) = grid(2);
        let mut mesh = EditMesh::from_triangles(&p, &t, &[]).unwrap();
        let h = half_edge_between(&mesh, 0, 1); // boundary → feature
        let m = mesh.split(h, DVec3::new(0.5, 0.0, 0.0)).unwrap();
        mesh.check_invariants().unwrap();
        assert!(mesh.is_feature(0, m) && mesh.is_feature(m, 1));
        assert!(!mesh.is_feature(0, 1));
        assert_eq!(mesh.kind(m), VertexKind::Feature);
    }

    #[test]
    fn collapse_moves_and_removes() {
        let (p, t) = grid(2);
        let mut mesh = EditMesh::from_triangles(&p, &t, &[]).unwrap();
        // A boundary feature vertex (5) may not leave its line along the
        // interior edge 4-5 ...
        assert!(!mesh.can_collapse(half_edge_between(&mesh, 4, 5)));
        // ... but the free centre (4) may be removed into it.
        let h = half_edge_between(&mesh, 5, 4);
        assert!(mesh.collapse(h, mesh.position(5)));
        mesh.check_invariants().unwrap();
        assert_eq!(mesh.face_count(), 6);
        assert!(!mesh.is_vertex_alive(4));
    }

    #[test]
    fn collapse_respects_link_condition() {
        let (p, t) = tetrahedron();
        let mut mesh = EditMesh::from_triangles(&p, &t, &[]).unwrap();
        let h = half_edge_between(&mesh, 0, 1);
        assert!(!mesh.collapse(h, DVec3::ZERO));
        mesh.check_invariants().unwrap();
        assert_eq!(mesh.face_count(), 4);
    }

    #[test]
    fn collapse_refuses_corner() {
        let (p, t) = grid(2);
        let mut mesh = EditMesh::from_triangles(&p, &t, &[[1, 4]]).unwrap();
        // 4 is a Corner (feature tip): it may not be removed.
        let h = half_edge_between(&mesh, 5, 4);
        assert!(!mesh.collapse(h, mesh.position(5)));
        mesh.check_invariants().unwrap();
    }

    #[test]
    fn collapse_refuses_pinching_two_boundaries() {
        // In a 1×2 strip, the middle edge joins two boundary vertices across the interior.
        let (p, t) = grid(1);
        let mut mesh = EditMesh::from_triangles(&p, &t, &[]).unwrap();
        let h = half_edge_between(&mesh, 0, 3);
        assert!(!mesh.collapse(h, mesh.position(0)));
    }

    #[test]
    fn flip_changes_diagonal() {
        let (p, t) = grid(1);
        let mut mesh = EditMesh::from_triangles(&p, &t, &[]).unwrap();
        let h = half_edge_between(&mesh, 0, 3);
        assert!(mesh.flip(h));
        mesh.check_invariants().unwrap();
        assert!(mesh.find_half_edge(0, 3).is_none() && mesh.find_half_edge(3, 0).is_none());
        assert!(mesh.find_half_edge(1, 2).is_some() || mesh.find_half_edge(2, 1).is_some());
    }

    #[test]
    fn flip_refuses_feature_and_boundary() {
        let (p, t) = grid(1);
        let mut mesh = EditMesh::from_triangles(&p, &t, &[[0, 3]]).unwrap();
        assert!(!mesh.flip(half_edge_between(&mesh, 0, 3)));
        assert!(!mesh.flip(half_edge_between(&mesh, 0, 1)));
    }

    #[test]
    fn soft_features_follow_splits_and_release_when_flat() {
        let (p, t) = grid(2);
        let mut mesh = EditMesh::from_triangles(&p, &t, &[[0, 4], [4, 8]]).unwrap();
        mesh.mark_soft(&[[0, 4]]);
        let m = mesh.split(half_edge_between(&mesh, 0, 4), DVec3::new(0.5, 0.5, 0.0)).unwrap();
        assert!(mesh.is_feature(0, m) && mesh.is_feature(m, 4));
        assert_eq!(mesh.kind(m), VertexKind::Feature);
        // The grid is flat: both soft halves go; the hard edge 4-8 stays.
        assert_eq!(mesh.release_flat_soft_features(35.0, 1.0), 2);
        assert!(!mesh.is_feature(0, m) && !mesh.is_feature(m, 4));
        assert!(mesh.is_feature(4, 8));
        assert_eq!(mesh.kind(m), VertexKind::Free);
        assert_eq!(mesh.kind(4), VertexKind::Corner, "4 is now the tip of the remaining line");
        mesh.check_invariants().unwrap();
    }

    #[test]
    fn zigzag_feature_is_not_a_corner_at_scale() {
        // A boundary that zigzags by ±0.01 every 0.1 turns ~22° per vertex up and
        // down; at scale 1.0 the line is straight, while the strip's 4 real
        // corners still turn 90°.
        let n = 10u32;
        let mut p = Vec::new();
        for y in 0..2 {
            for i in 0..=n {
                let z = if i % 2 == 0 { 0.0 } else { 0.05 };
                p.push(DVec3::new(f64::from(i) / f64::from(n), f64::from(y), z));
            }
        }
        let top = n + 1;
        let mut t = Vec::new();
        for i in 0..n {
            t.push([i, i + 1, top + i + 1]);
            t.push([i, top + i + 1, top + i]);
        }
        let mut mesh = EditMesh::from_triangles(&p, &t, &[]).unwrap();
        assert_eq!(mesh.kind(5), VertexKind::Corner, "at vertex scale the zigzag turns sharply");
        mesh.reclassify(1.0);
        assert_eq!(mesh.kind(5), VertexKind::Feature);
        for corner in [0, n, top, top + n] {
            assert_eq!(mesh.kind(corner), VertexKind::Corner);
        }
    }

    #[test]
    fn faces_inherit_reference_normals() {
        let (p, t) = grid(2);
        let mut mesh = EditMesh::from_triangles(&p, &t, &[]).unwrap();
        let up = DVec3::Z;
        assert!(mesh.faces().all(|f| mesh.reference_normal(f).dot(up) > 0.999));
        // Tilt the centre vertex: face normals change, references don't.
        mesh.set_position(4, DVec3::new(0.5, 0.5, 0.4));
        let m = mesh.split(half_edge_between(&mesh, 0, 4), DVec3::new(0.25, 0.25, 0.2)).unwrap();
        let fan = mesh.faces_around(m);
        assert!(fan.iter().all(|&f| mesh.reference_normal(f).dot(up) > 0.999));
        assert!(fan.iter().any(|&f| mesh.face_normal(f).dot(up) < 0.99));
        let flippable = mesh.half_edges().into_iter().find(|&h| mesh.twin(h).is_some()).unwrap();
        assert!(mesh.flip(flippable));
        let (rebuilt, _) = mesh.rebuilt().unwrap();
        assert!(rebuilt.faces().all(|f| rebuilt.reference_normal(f).dot(up) > 0.999));
        assert!(rebuilt.faces().any(|f| rebuilt.face_normal(f).dot(up) < 0.99));
    }

    #[test]
    fn ridge_with_a_curling_side_is_not_released() {
        // A 110° crease along x. Side A leaves the crease horizontally and
        // curls up through 120° on a radius-0.5 arc; side B runs flat, 70°
        // down. Judged within 1.0 of an edge, the plane through the edge put
        // A's curled-up faces on B's side, mixing the sides into "smooth".
        // Flooding each side from its own face, up to the crease, keeps them
        // apart. (Found on khronos/Duck's beak: 28° by the plane, 55° by sides.)
        let (columns, arc, flat) = (21u32, 12u32, 8u32);
        let mut profile = Vec::new();
        for k in (1..=flat).rev() {
            let s = 0.1 * f64::from(k);
            let down = 70f64.to_radians();
            profile.push((-s * down.cos(), -s * down.sin()));
        }
        for k in 0..=arc {
            let phi = 120f64.to_radians() * f64::from(k) / f64::from(arc);
            profile.push((0.5 * phi.sin(), 0.5 * (1.0 - phi.cos())));
        }
        let width = profile.len() as u32;
        let mut p = Vec::new();
        for i in 0..columns {
            for &(y, z) in &profile {
                p.push(DVec3::new(0.1 * f64::from(i), y, z));
            }
        }
        let mut t = Vec::new();
        for i in 0..columns - 1 {
            for j in 0..width - 1 {
                let (a, b) = (i * width + j, i * width + j + 1);
                t.extend([[a, b + width, b], [a, a + width, b + width]]);
            }
        }
        let crease: Vec<[u32; 2]> =
            (0..columns - 1).map(|i| [i * width + flat, (i + 1) * width + flat]).collect();
        let mut mesh = EditMesh::from_triangles(&p, &t, &crease).unwrap();
        mesh.mark_soft(&crease);
        assert_eq!(mesh.release_flat_soft_features(35.0, 1.0), 0);
    }

    #[test]
    fn bowtie_vertex_is_rejected() {
        // Two triangles sharing only vertex 0: two fans around one vertex.
        let p = [DVec3::ZERO, DVec3::X, DVec3::Y, -DVec3::X, -DVec3::Y];
        let err = EditMesh::from_triangles(&p, &[[0, 1, 2], [0, 3, 4]], &[]).unwrap_err();
        assert!(err.contains("non-manifold vertex"), "{err}");
    }

    #[test]
    fn soft_marks_survive_rebuild() {
        let (p, t) = grid(2);
        let mut mesh = EditMesh::from_triangles(&p, &t, &[[0, 4]]).unwrap();
        mesh.mark_soft(&[[0, 4]]);
        let (mut rebuilt, map) = mesh.rebuilt().unwrap();
        let (a, b) = (map[0].unwrap(), map[4].unwrap());
        assert!(rebuilt.is_feature(a, b));
        assert_eq!(rebuilt.release_flat_soft_features(35.0, 1.0), 1);
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(48))]
        #[test]
        fn random_op_sequence_keeps_invariants(ops in proptest::collection::vec((0u8..3, 0usize..10_000, 0.0..1.0f64), 200)) {
            let (p, t) = grid(6);
            let mut mesh = EditMesh::from_triangles(&p, &t, &[[8, 9], [9, 10]]).unwrap();
            for (op, pick, s) in ops {
                let edges = mesh.half_edges();
                if edges.is_empty() {
                    break;
                }
                let h = edges[pick % edges.len()];
                let (a, b) = (mesh.from(h), mesh.to(h));
                let at = mesh.position(a).lerp(mesh.position(b), 0.25 + 0.5 * s);
                match op {
                    0 => { mesh.split(h, at); }
                    1 => { mesh.collapse(h, mesh.position(a)); }
                    _ => { mesh.flip(h); }
                }
                prop_assert!(mesh.check_invariants().is_ok(), "{:?}", mesh.check_invariants());
            }
        }
    }
}
