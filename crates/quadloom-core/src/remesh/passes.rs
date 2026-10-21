//! The four remeshing passes over an [`EditMesh`].

use glam::DVec3;
use rayon::prelude::*;

use super::Projector;
use crate::edit::{EditMesh, HalfEdge, VertexKind};
use crate::spatial::closest_point_on_triangle;

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

    /// The live half-edge between `h`'s endpoints, in either direction (`None`
    /// once the edge is gone). Sweeps snapshot half-edge ids, and an operation
    /// on a neighbour re-creates the faces around an edge (new ids) without
    /// removing the edge itself; looking it up again keeps such an edge in the
    /// sweep. Skipping stale ids instead left a 2.15-long panel edge unsplit
    /// through all 15 iterations on `khronos/CommercialRefrigerator`, since the
    /// fan beside it re-formed every iteration.
    fn current(&self, h: HalfEdge) -> Option<HalfEdge> {
        let (a, b) = (self.mesh.from(h), self.mesh.to(h));
        self.mesh.find_half_edge(a, b).or_else(|| self.mesh.find_half_edge(b, a))
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
                let Some(h) = self.current(h) else { continue };
                let (a, b) = (self.mesh.from(h), self.mesh.to(h));
                if self.length(h) <= SPLIT_ABOVE * self.size(a, b) {
                    continue;
                }
                // On the input (a feature edge's midpoint on its feature line),
                // not on the chord: smoothing may not get to move it there.
                let chord = 0.5 * (self.mesh.position(a) + self.mesh.position(b));
                let mid = if self.mesh.is_feature(a, b) {
                    self.projector.onto_features(chord)
                } else {
                    self.projector.onto_surface(chord)
                };
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
            let Some(h) = self.current(h) else { continue };
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
        // One walk around each endpoint serves every check below.
        let faces = self.faces_around_all(&[keep, gone]);
        let ring_ok = faces.iter().all(|&f| {
            let corners = self.mesh.face_corners(f);
            corners.iter().all(|&n| {
                (!corners.contains(&keep) || fits(n, pk)) && (!corners.contains(&gone) || fits(n, pg))
            })
        });
        if !ring_ok
            || !self.stays_close(&faces, keep, gone, target)
            || !self.faces_survive_among(&faces, &[keep, gone], target, Some((keep, gone)))
        {
            return false;
        }
        self.mesh.collapse_prechecked(h, target)
    }

    /// The faces around any of `vertices`, each once.
    fn faces_around_all(&self, vertices: &[u32]) -> Vec<u32> {
        let mut faces: Vec<u32> = Vec::with_capacity(16);
        for &v in vertices {
            for f in self.mesh.faces_around(v) {
                if !faces.contains(&f) {
                    faces.push(f);
                }
            }
        }
        faces
    }

    /// Whether the faces left by collapsing `gone` into `keep` at `target`
    /// pass within the deviation bound of both endpoints' old positions.
    /// Those points lie on the input surface, and coarsening is what pulls the
    /// surface away from it: projection keeps every *vertex* on the surface,
    /// but only this keeps the faces near where the removed vertex was.
    fn stays_close(&self, faces: &[u32], keep: u32, gone: u32, target: DVec3) -> bool {
        let moved = |v: u32| if v == keep || v == gone { target } else { self.mesh.position(v) };
        let remaining: Vec<[DVec3; 3]> = faces
            .iter()
            .map(|&f| self.mesh.face_corners(f))
            .filter(|c| !(c.contains(&keep) && c.contains(&gone)))
            .map(|c| c.map(moved))
            .collect();
        [self.mesh.position(keep), self.mesh.position(gone)].into_iter().all(|q| {
            remaining.iter().any(|&[a, b, c]| {
                (closest_point_on_triangle(q, a, b, c) - q).length() <= self.projector.max_distance
            })
        })
    }

    /// Whether moving each of `movers` to `target` keeps every remaining face
    /// around them facing the same way and passes the quality guard. With
    /// `collapsing = Some((a, b))`, faces holding both disappear: they count
    /// towards the "before" quality but not the "after".
    fn faces_survive(&self, movers: &[u32], target: DVec3, collapsing: Option<(u32, u32)>) -> bool {
        self.faces_survive_among(&self.faces_around_all(movers), movers, target, collapsing)
    }

    /// [`faces_survive`](Self::faces_survive) over `faces`, the faces around
    /// `movers` (as [`faces_around_all`](Self::faces_around_all) lists them).
    fn faces_survive_among(
        &self,
        faces: &[u32],
        movers: &[u32],
        target: DVec3,
        collapsing: Option<(u32, u32)>,
    ) -> bool {
        let mut worst_before = 1.0f64;
        let mut worst_after = 1.0f64;
        for &f in faces {
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
        worst_after > 0.0 && worst_after >= (1.0 - QUALITY_TOLERANCE) * QUALITY_FLOOR.min(worst_before)
    }

    /// Flips interior, non-feature edges when that brings the four vertices
    /// closer to the ideal valence (6 inside, 4 on the boundary), or when it
    /// clearly improves the worse of the two triangles, without bending the
    /// surface.
    pub fn equalize_valence(&mut self) -> usize {
        let mut flips = 0;
        for h in self.mesh.half_edges() {
            let Some(h) = self.current(h) else { continue };
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
    /// projects them back; corners stay put. Targets are computed in parallel
    /// from the current positions, then each move is checked and applied in
    /// vertex order (so the result doesn't depend on the thread count).
    pub fn smooth_tangential(&mut self) {
        let (mesh, projector) = (&*self.mesh, self.projector);
        let moves: Vec<(u32, DVec3)> = (0..mesh.vertex_slots() as u32)
            .into_par_iter()
            .filter_map(|v| smoothing_target(mesh, projector, v).map(|t| (v, t)))
            .collect();
        for (v, target) in moves {
            if self.faces_survive(&[v], target, None) {
                self.mesh.set_position(v, target);
            }
        }
    }
}

/// Where tangential smoothing would move `v` (`None` for corners, dead or
/// isolated vertices).
fn smoothing_target(mesh: &EditMesh, projector: &Projector, v: u32) -> Option<DVec3> {
    if !mesh.is_vertex_alive(v) {
        return None;
    }
    let p = mesh.position(v);
    match mesh.kind(v) {
        VertexKind::Corner => None,
        VertexKind::Free => {
            let ring: Vec<u32> = mesh.one_ring(v).collect();
            if ring.is_empty() {
                return None;
            }
            let centroid = ring.iter().map(|&n| mesh.position(n)).sum::<DVec3>() / ring.len() as f64;
            let normal =
                mesh.faces_around(v).iter().map(|&f| mesh.face_normal(f)).sum::<DVec3>().normalize_or_zero();
            let delta = centroid - p;
            Some(projector.onto_surface(p + SMOOTH_STEP * (delta - normal * delta.dot(normal))))
        }
        VertexKind::Feature => {
            let ends = mesh.feature_neighbours(v);
            let [e0, e1] = ends.as_slice() else { return None };
            let (q0, q1) = (mesh.position(*e0), mesh.position(*e1));
            let tangent = (q1 - q0).normalize_or_zero();
            let delta = 0.5 * (q0 + q1) - p;
            Some(projector.onto_features(p + SMOOTH_STEP * tangent * delta.dot(tangent)))
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
