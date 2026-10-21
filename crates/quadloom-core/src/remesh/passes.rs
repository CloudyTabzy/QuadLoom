//! The four remeshing passes over an [`EditMesh`].

use glam::DVec3;

use super::Projector;
use crate::edit::{EditMesh, HalfEdge, VertexKind};

const SPLIT_ABOVE: f64 = 4.0 / 3.0;
const COLLAPSE_BELOW: f64 = 4.0 / 5.0;
/// Splitting sweeps per pass. One, as in Botsch & Kobbelt: repeated sweeps
/// keep splitting the connectors to opposite vertices and build long fans;
/// the outer iterations converge instead.
const SPLIT_ROUNDS: usize = 1;
/// A move or collapse may not turn any face normal by more than ~78°.
const MIN_NORMAL_DOT: f64 = 0.2;
/// A flip's new faces must stay within ~25° of the faces they replace.
const FLIP_MIN_NORMAL_DOT: f64 = 0.9;
/// Tangential smoothing step (fraction of the way to the neighbour centroid).
const SMOOTH_STEP: f64 = 0.5;
/// No operation may create a face smaller than (collapse length)² / 1000, the
/// same floor vcglib uses; below it faces are numerically slivers.
const MIN_AREA_FACTOR: f64 = COLLAPSE_BELOW * COLLAPSE_BELOW / 1000.0;
/// Flips may not create triangles with a radius ratio below this.
const FLIP_MIN_RADIUS_RATIO: f64 = 0.1;

pub(super) struct Pass<'a> {
    pub mesh: &'a mut EditMesh,
    /// Desired edge length per vertex slot; grows with the mesh.
    pub sizes: Vec<f64>,
    pub projector: &'a Projector,
}

impl Pass<'_> {
    fn size(&self, a: u32, b: u32) -> f64 {
        let s = |v: u32| self.sizes.get(v as usize).copied().unwrap_or(f64::INFINITY);
        s(a).min(s(b))
    }

    fn length(&self, h: HalfEdge) -> f64 {
        (self.mesh.position(self.mesh.from(h)) - self.mesh.position(self.mesh.to(h))).length()
    }

    fn half_edge_alive(&self, h: HalfEdge) -> bool {
        // A half-edge is alive if the directed edge it names still maps to it.
        let (a, b) = (self.mesh.from(h), self.mesh.to(h));
        self.mesh.find_half_edge(a, b) == Some(h)
    }

    /// Splits every edge longer than 4/3 of its local target at its midpoint.
    pub fn split_long(&mut self) -> usize {
        let mut total = 0;
        for _ in 0..SPLIT_ROUNDS {
            let mut splits = 0;
            for h in self.mesh.half_edges() {
                if !self.half_edge_alive(h) {
                    continue;
                }
                let (a, b) = (self.mesh.from(h), self.mesh.to(h));
                if self.length(h) <= SPLIT_ABOVE * self.size(a, b) {
                    continue;
                }
                let mid = 0.5 * (self.mesh.position(a) + self.mesh.position(b));
                if let Some(m) = self.mesh.split(h, mid) {
                    let s = 0.5 * (self.sizes[a as usize] + self.sizes[b as usize]);
                    if self.sizes.len() <= m as usize {
                        self.sizes.resize(m as usize + 1, s);
                    }
                    self.sizes[m as usize] = s;
                    splits += 1;
                }
            }
            total += splits;
            if splits == 0 {
                break;
            }
        }
        total
    }

    /// Collapses edges shorter than 4/5 of their local target, when doing so
    /// keeps edges below the split threshold, faces unflipped, and the surface
    /// within the distance bound.
    pub fn collapse_short(&mut self) -> usize {
        let mut collapses = 0;
        // Shortest first (ties by id): the standard order, better quality and
        // faster convergence on dense input.
        let mut edges: Vec<(f64, HalfEdge)> =
            self.mesh.half_edges().into_iter().map(|h| (self.length(h), h)).collect();
        edges.sort_by(|x, y| x.0.total_cmp(&y.0).then(x.1.cmp(&y.1)));
        for (_, h) in edges {
            if !self.half_edge_alive(h) {
                continue;
            }
            let (a, b) = (self.mesh.from(h), self.mesh.to(h));
            let size = self.size(a, b);
            if self.length(h) >= COLLAPSE_BELOW * size {
                continue;
            }
            // Try removing the less constrained endpoint first.
            let reverse = self.mesh.find_half_edge(b, a);
            let mut candidates = vec![h];
            candidates.extend(reverse);
            candidates.sort_by_key(|&c| rank(self.mesh.kind(self.mesh.to(c))));
            for c in candidates {
                if self.try_collapse(c, size) {
                    collapses += 1;
                    break;
                }
            }
        }
        collapses
    }

    fn try_collapse(&mut self, h: HalfEdge, size: f64) -> bool {
        if !self.mesh.can_collapse(h) {
            return false;
        }
        let (keep, gone) = (self.mesh.from(h), self.mesh.to(h));
        let target = match (self.mesh.kind(keep), self.mesh.kind(gone)) {
            (VertexKind::Free, VertexKind::Free) => {
                self.projector.onto_surface(0.5 * (self.mesh.position(keep) + self.mesh.position(gone)))
            }
            _ => self.mesh.position(keep),
        };
        if !self.projector.within_bound(target) {
            return false;
        }
        let limit = SPLIT_ABOVE * size;
        let ring_ok = self
            .mesh
            .one_ring(keep)
            .chain(self.mesh.one_ring(gone))
            .filter(|&n| n != keep && n != gone)
            .all(|n| (self.mesh.position(n) - target).length() <= limit);
        if !ring_ok || !self.faces_survive(&[keep, gone], target, Some((keep, gone)), size) {
            return false;
        }
        self.mesh.collapse_prechecked(h, target)
    }

    /// Whether moving each of `movers` to `target` keeps every face around them
    /// facing the same way and above the minimum area for local size `size`.
    /// With `collapsing = Some((a, b))`, faces holding both are about to
    /// disappear and are ignored.
    fn faces_survive(
        &self,
        movers: &[u32],
        target: DVec3,
        collapsing: Option<(u32, u32)>,
        size: f64,
    ) -> bool {
        let min_double_area = 2.0 * MIN_AREA_FACTOR * size * size;
        for &v in movers {
            for f in self.mesh.faces_around(v) {
                let corners = self.mesh.face_corners(f);
                if let Some((a, b)) = collapsing
                    && corners.contains(&a)
                    && corners.contains(&b)
                {
                    continue;
                }
                let before = self.mesh.face_normal(f);
                let [p0, p1, p2] =
                    corners.map(|c| if movers.contains(&c) { target } else { self.mesh.position(c) });
                let after = (p1 - p0).cross(p2 - p0);
                if after.length() < min_double_area || after.normalize_or_zero().dot(before) < MIN_NORMAL_DOT
                {
                    return false;
                }
            }
        }
        true
    }

    /// Flips interior, non-feature edges when that brings the four vertices
    /// closer to the ideal valence (6 inside, 4 on the boundary) without
    /// bending the surface.
    pub fn equalize_valence(&mut self) -> usize {
        let mut flips = 0;
        for h in self.mesh.half_edges() {
            if !self.half_edge_alive(h) {
                continue;
            }
            let Some(t) = self.mesh.twin(h) else { continue };
            let (a, b) = (self.mesh.from(h), self.mesh.to(h));
            if self.mesh.is_feature(a, b) {
                continue;
            }
            let (c, d) = (self.mesh.from(self.mesh.prev(h)), self.mesh.from(self.mesh.prev(t)));
            let ideal = |v: u32| if self.mesh.is_boundary_vertex(v) { 4i64 } else { 6 };
            let val = |v: u32| self.mesh.valence(v) as i64;
            let before: i64 = [a, b, c, d].iter().map(|&v| (val(v) - ideal(v)).abs()).sum();
            let after = (val(a) - 1 - ideal(a)).abs()
                + (val(b) - 1 - ideal(b)).abs()
                + (val(c) + 1 - ideal(c)).abs()
                + (val(d) + 1 - ideal(d)).abs();
            if after >= before {
                continue;
            }
            let [pa, pb, pc, pd] = [a, b, c, d].map(|v| self.mesh.position(v));
            let old = (self.mesh.face_normal(self.mesh.face(h)) + self.mesh.face_normal(self.mesh.face(t)))
                .normalize_or_zero();
            let n1 = (pa - pc).cross(pd - pc).normalize_or_zero();
            let n2 = (pd - pc).cross(pb - pc).normalize_or_zero();
            if n1.dot(old) < FLIP_MIN_NORMAL_DOT || n2.dot(old) < FLIP_MIN_NORMAL_DOT {
                continue;
            }
            let min_area = MIN_AREA_FACTOR * self.size(c, d).powi(2);
            let healthy =
                |p: [DVec3; 3]| triangle_area(p) >= min_area && radius_ratio(p) >= FLIP_MIN_RADIUS_RATIO;
            if !healthy([pc, pa, pd]) || !healthy([pc, pd, pb]) {
                continue;
            }
            if self.mesh.flip(h) {
                flips += 1;
            }
        }
        flips
    }

    /// Moves each free vertex toward its neighbours' centroid within the
    /// tangent plane, and each feature vertex along its feature line, then
    /// projects them back; corners stay put.
    pub fn smooth_tangential(&mut self) {
        for v in 0..self.mesh.vertex_slots() as u32 {
            if !self.mesh.is_vertex_alive(v) {
                continue;
            }
            let p = self.mesh.position(v);
            let target = match self.mesh.kind(v) {
                VertexKind::Corner => continue,
                VertexKind::Free => {
                    let ring: Vec<u32> = self.mesh.one_ring(v).collect();
                    if ring.is_empty() {
                        continue;
                    }
                    let centroid =
                        ring.iter().map(|&n| self.mesh.position(n)).sum::<DVec3>() / ring.len() as f64;
                    let normal = self
                        .mesh
                        .faces_around(v)
                        .iter()
                        .map(|&f| self.mesh.face_normal(f))
                        .sum::<DVec3>()
                        .normalize_or_zero();
                    let delta = centroid - p;
                    self.projector.onto_surface(p + SMOOTH_STEP * (delta - normal * delta.dot(normal)))
                }
                VertexKind::Feature => {
                    let ends = self.mesh.feature_neighbours(v);
                    let [e0, e1] = ends.as_slice() else { continue };
                    let (q0, q1) = (self.mesh.position(*e0), self.mesh.position(*e1));
                    let tangent = (q1 - q0).normalize_or_zero();
                    let delta = 0.5 * (q0 + q1) - p;
                    self.projector.onto_features(p + SMOOTH_STEP * tangent * delta.dot(tangent))
                }
            };
            let size = self.sizes.get(v as usize).copied().unwrap_or(f64::INFINITY);
            if self.faces_survive(&[v], target, None, size) {
                self.mesh.set_position(v, target);
            }
        }
    }
}

/// Removal preference: free vertices first, then feature vertices; corners never.
fn rank(kind: VertexKind) -> u8 {
    match kind {
        VertexKind::Free => 0,
        VertexKind::Feature => 1,
        VertexKind::Corner => 2,
    }
}

fn triangle_area([a, b, c]: [DVec3; 3]) -> f64 {
    0.5 * (b - a).cross(c - a).length()
}

/// 2·inradius / circumradius: 1 for equilateral, 0 for degenerate.
fn radius_ratio(p: [DVec3; 3]) -> f64 {
    let [a, b, c] = p;
    let (la, lb, lc) = ((b - c).length(), (a - c).length(), (a - b).length());
    let product = la * lb * lc * (la + lb + lc);
    if product <= 0.0 {
        return 0.0;
    }
    let area = triangle_area(p);
    (16.0 * area * area / product).clamp(0.0, 1.0)
}
