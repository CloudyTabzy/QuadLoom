//! A triangle mesh that can be edited in place: split, collapse and flip, with
//! feature edges and vertex kinds carried through every operation.
//!
//! Face `f` owns half-edges `3f..3f + 3`, so `next`, `prev` and `face` are
//! arithmetic; twins are found through a directed-edge map. Every operation is
//! expressed as "remove these faces, add those" and *validated before anything
//! changes* (no repeated corners, no edge used twice in the same direction), so
//! a refused operation leaves the mesh exactly as it was.

use std::collections::{HashMap, HashSet};

pub use glam::DVec3;

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
    corners: Vec<[u32; 3]>,
    face_alive: Vec<bool>,
    live_faces: usize,
    /// `(from, to)` → half-edge.
    directed: HashMap<(u32, u32), u32>,
    /// Feature edges as sorted vertex pairs.
    features: HashSet<(u32, u32)>,
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
            corners: Vec::with_capacity(triangles.len()),
            face_alive: Vec::with_capacity(triangles.len()),
            live_faces: 0,
            directed: HashMap::with_capacity(triangles.len() * 3),
            features: HashSet::new(),
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
            .keys()
            .filter(|&&(a, b)| !mesh.directed.contains_key(&(b, a)))
            .map(|&(a, b)| key(a, b))
            .collect();
        mesh.features.extend(boundary);
        for &[a, b] in feature_edges {
            if mesh.directed.contains_key(&(a, b)) || mesh.directed.contains_key(&(b, a)) {
                mesh.features.insert(key(a, b));
            }
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
        self.directed.get(&(self.to(h), self.from(h))).map(|&t| HalfEdge(t))
    }

    pub fn find_half_edge(&self, a: u32, b: u32) -> Option<HalfEdge> {
        self.directed.get(&(a, b)).map(|&h| HalfEdge(h))
    }

    pub fn is_feature(&self, a: u32, b: u32) -> bool {
        self.features.contains(&key(a, b))
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

    pub fn valence(&self, v: u32) -> usize {
        self.one_ring(v).len()
    }

    pub fn is_boundary_vertex(&self, v: u32) -> bool {
        self.fan(v).first().is_some_and(|&h| self.twin(h).is_none())
    }

    /// The vertex's feature neighbours.
    pub fn feature_neighbours(&self, v: u32) -> Vec<u32> {
        self.one_ring(v).filter(|&n| self.is_feature(v, n)).collect()
    }

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
    fn replace_faces(&mut self, remove: &[u32], add: &[[u32; 3]]) -> Option<Vec<u32>> {
        let removed: HashSet<u32> = remove.iter().copied().collect();
        let mut seen: HashSet<(u32, u32)> = HashSet::new();
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
                let taken = self.directed.get(&e).is_some_and(|h| !removed.contains(&(h / 3)));
                if taken || !seen.insert(e) {
                    return None;
                }
            }
        }

        // Vertices that lose faces and get none back need a surviving face to
        // point at; find it before anything is removed.
        let in_added: HashSet<u32> = add.iter().flatten().copied().collect();
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
                self.directed.remove(&(t[i], t[(i + 1) % 3]));
            }
            self.face_alive[f as usize] = false;
            self.live_faces -= 1;
        }
        let mut ids = Vec::with_capacity(add.len());
        for t in add {
            let f = self.corners.len() as u32;
            self.corners.push(*t);
            self.face_alive.push(true);
            self.live_faces += 1;
            for i in 0..3 {
                self.directed.insert((t[i], t[(i + 1) % 3]), 3 * f + i as u32);
                self.out[t[i] as usize] = 3 * f + i as u32;
            }
            ids.push(f);
        }
        for (v, h) in relink {
            self.out[v as usize] = h;
        }
        Some(ids)
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
            return None;
        }
        if self.features.remove(&key(a, b)) {
            self.features.insert(key(a, m));
            self.features.insert(key(m, b));
            self.kinds[m as usize] = VertexKind::Feature;
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
        let ring_a: HashSet<u32> = self.one_ring(a).collect();
        let shared: HashSet<u32> = self.one_ring(b).filter(|v| ring_a.contains(v)).collect();
        let expected: HashSet<u32> = std::iter::once(c).chain(d).collect();
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
        if !self.can_collapse(h) {
            return false;
        }
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
            self.features.remove(&key(b, x));
            if x != a {
                self.features.insert(key(a, x));
            }
        }
        self.vertex_alive[b as usize] = false;
        self.out[b as usize] = NONE;
        self.positions[a as usize] = keep_position;
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

    /// Live vertices (renumbered in index order), faces, and feature edges.
    pub fn compact(&self) -> (Vec<DVec3>, Vec<[u32; 3]>, Vec<[u32; 2]>) {
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
        (positions, triangles, features)
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
                if self.directed.get(&(t[i], t[(i + 1) % 3])) != Some(&(3 * f + i as u32)) {
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
        }
        for &(a, b) in &self.features {
            if !self.directed.contains_key(&(a, b)) && !self.directed.contains_key(&(b, a)) {
                return Err(format!("feature edge ({a}, {b}) is not a mesh edge"));
            }
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
