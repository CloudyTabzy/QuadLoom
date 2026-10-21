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
/// No face may turn away from the input orientation it descends from (see
/// [`EditMesh::reference_normal`]). Each operation turns faces by less than
/// `MIN_NORMAL_DOT` allows, but on slivers, whose normals swing with tiny
/// moves, such turns can add up to a fold over several operations.
const MIN_REFERENCE_DOT: f64 = 0.0;
/// A flip's new faces must stay within ~25° of the faces they replace.
const FLIP_MIN_NORMAL_DOT: f64 = 0.9;
/// Tangential smoothing step (fraction of the way to the neighbour centroid).
const SMOOTH_STEP: f64 = 0.5;
/// Quality guard: an operation may not leave a face with a radius ratio below
/// `min(QUALITY_FLOOR, worst ratio among the faces it replaces)`. So it can
/// never make a neighbourhood's worst face worse, but it *can* clean up a
/// neighbourhood that's already bad (inputs made entirely of slivers).
const QUALITY_FLOOR: f64 = 0.1;
/// Relative slack on the quality guard. A collapse that leaves a neighbouring
/// sliver as it was still changes its ratio in the last digits; without slack
/// such rounding blocks every collapse on a sliver-only input.
const QUALITY_TOLERANCE: f64 = 1e-3;
/// A flip made for quality alone must improve the worse of its two faces by
/// at least this factor (strict improvement, so flips can't cycle).
const FLIP_QUALITY_GAIN: f64 = 1.25;
/// A collapse may lengthen an edge that's already over the split limit by up
/// to this factor (removing a sliver always stretches its long sides a bit).
const LONG_EDGE_SLACK: f64 = 1.1;

pub(super) struct Pass<'a> {
    pub mesh: &'a mut EditMesh,
    /// Desired edge length per vertex slot; grows with the mesh.
    pub sizes: Vec<f64>,
    pub projector: &'a Projector,
    /// Splitting stops above this many faces: a hard ceiling so no input can
    /// make the remesh explode.
    pub max_faces: usize,
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
                if self.mesh.face_count() >= self.max_faces {
                    return total + splits;
                }
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
        // No new edge over the split limit, unless the edge it replaces was
        // already about that long (slivers' long sides): the splitter deals
        // with those next pass.
        let limit = SPLIT_ABOVE * size;
        let (pk, pg) = (self.mesh.position(keep), self.mesh.position(gone));
        let fits = |n: u32, old_end: DVec3| {
            let q = self.mesh.position(n);
            n == keep
                || n == gone
                || (q - target).length() <= limit.max(LONG_EDGE_SLACK * (q - old_end).length())
        };
        let ring_ok =
            self.mesh.one_ring(keep).all(|n| fits(n, pk)) && self.mesh.one_ring(gone).all(|n| fits(n, pg));
        if !ring_ok || !self.faces_survive(&[keep, gone], target, Some((keep, gone))) {
            return false;
        }
        self.mesh.collapse_prechecked(h, target)
    }

    /// Whether moving each of `movers` to `target` keeps every remaining face
    /// around them facing the same way and passes the quality guard. With
    /// `collapsing = Some((a, b))`, faces holding both disappear: they count
    /// towards the "before" quality but not the "after".
    fn faces_survive(&self, movers: &[u32], target: DVec3, collapsing: Option<(u32, u32)>) -> bool {
        let mut worst_before = 1.0f64;
        let mut worst_after = 1.0f64;
        let mut seen: Vec<u32> = Vec::with_capacity(16);
        for &v in movers {
            for f in self.mesh.faces_around(v) {
                if seen.contains(&f) {
                    continue;
                }
                seen.push(f);
                let corners = self.mesh.face_corners(f);
                let old = corners.map(|c| self.mesh.position(c));
                worst_before = worst_before.min(radius_ratio(old));
                if let Some((a, b)) = collapsing
                    && corners.contains(&a)
                    && corners.contains(&b)
                {
                    continue;
                }
                let new = corners.map(|c| if movers.contains(&c) { target } else { self.mesh.position(c) });
                let normal = (new[1] - new[0]).cross(new[2] - new[0]).normalize_or_zero();
                if normal.dot(self.mesh.face_normal(f)) < MIN_NORMAL_DOT
                    || normal.dot(self.mesh.reference_normal(f)) <= MIN_REFERENCE_DOT
                {
                    return false;
                }
                worst_after = worst_after.min(radius_ratio(new));
            }
        }
        worst_after > 0.0 && worst_after >= (1.0 - QUALITY_TOLERANCE) * QUALITY_FLOOR.min(worst_before)
    }

    /// Flips interior, non-feature edges when that brings the four vertices
    /// closer to the ideal valence (6 inside, 4 on the boundary), or when it
    /// clearly improves the worse of the two triangles, without bending the
    /// surface.
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
            let [pa, pb, pc, pd] = [a, b, c, d].map(|v| self.mesh.position(v));
            let quality_before = radius_ratio([pa, pb, pc]).min(radius_ratio([pb, pa, pd]));
            let quality_after = radius_ratio([pc, pa, pd]).min(radius_ratio([pc, pd, pb]));
            let better_valence = after < before && quality_after >= QUALITY_FLOOR.min(quality_before);
            let better_shape = quality_after > FLIP_QUALITY_GAIN * quality_before;
            if !better_valence && !better_shape {
                continue;
            }
            let (f, g) = (self.mesh.face(h), self.mesh.face(t));
            let old = (self.mesh.face_normal(f) + self.mesh.face_normal(g)).normalize_or_zero();
            let reference = self.mesh.reference_normal(f) + self.mesh.reference_normal(g);
            let n1 = (pa - pc).cross(pd - pc).normalize_or_zero();
            let n2 = (pd - pc).cross(pb - pc).normalize_or_zero();
            if [n1, n2]
                .iter()
                .any(|n| n.dot(old) < FLIP_MIN_NORMAL_DOT || n.dot(reference) <= MIN_REFERENCE_DOT)
            {
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
            if self.faces_survive(&[v], target, None) {
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
