//! Target edge length and the per-vertex sizing field.

use hashbrown::{HashMap, HashSet};

use glam::{DMat4, DVec3, DVec4};
use rayon::prelude::*;

use super::RemeshParams;
use crate::edit::{EditMesh, VertexKind};
use crate::unionfind::UnionFind;

/// Faces the automatic length aims at for a compact shape (the oracle's 10,000).
const NOMINAL_FACES: f64 = 10_000.0;
/// The most a thin closed part multiplies its share of the faces by: edges a
/// quarter of the nominal length, as the oracle's rule ends up with. A limit
/// of four was tried and cost the thin-walled parts and many-part assets it
/// touched their quality (AntiqueCamera's mean scaled Jacobian 0.93 to 0.85):
/// their features lie closer together than the longer quads.
const DENSEST: f64 = 16.0;
/// Curvature is measured over vertices within this fraction of the target
/// length, so tessellation noise finer than the target averages out.
const BALL_RADIUS: f64 = 0.5;
/// At most this many samples per curvature ball: bounds the cost on input
/// much denser than the target (later, coarser iterations see the full ball).
const MAX_BALL: usize = 64;
/// Below this agreement (|Σ normals| / Σ |normals|) the ball isn't a height
/// field over its mean plane (the surface wraps within it), so the one-ring
/// estimate is used instead.
const MIN_COHERENCE: f64 = 0.5;
/// At this agreement or better the ball is fitted as it is. Under it the
/// surface turns too far within the ball for the fit to read its curvature
/// (about 60° to either side of the middle), and a smaller ball is tried.
const GENTLE: f64 = 0.85;
/// A ball is halved at most this many times.
const MAX_HALVINGS: usize = 4;

/// Triangle edges round a full turn of the surface, at least: a tube gets
/// twelve round it however thin it is, enough for a layout to be traced on
/// it and for quads to go round it in six or more.
pub const EDGES_PER_TURN: f64 = 12.0;
/// The turn rule shortens edges to no less than this share of the target.
/// It may go under `min_mult`, which bounds the deviation rule: that one
/// trades edge length against a tolerance, this one keeps a thin part a
/// shape at all.
const TURN_FLOOR: f64 = 0.1;
/// The turn rule goes by the net curvature over a ball of this many target
/// lengths ([`Ball::net_curvature`]). At 1.25 a tube up to 3.6 target
/// lengths round is shortened, and the rounded rim of a thin plate, which
/// turns half way and stops, is not: its net curvature is about 2 / radius
/// of the ball, twelve edges round which are a third of the target, the
/// deviation rule's own shortest.
const TURN_RADIUS: f64 = 1.25;
/// At most this many vertices in that ball: all of it at the turn rule's
/// shortest edges.
const MAX_TURN_BALL: usize = 512;

/// The longest edge that still gives `per_turn` edges round a full turn of
/// a surface of curvature `k`: `2π / (|k| · per_turn)`. Infinite where it's flat.
///
/// The deviation rule ([`error_bounded_length`]) allows an edge as long as
/// the tolerance lets it stray, which on a thin tube is most of the way
/// round: at a tolerance of 1/2500 of the diagonal, a tail 1/300 of it thick
/// may have five edges round it, and the quads laid on those have three. A
/// count round the turn is what meshing practice uses where shape matters
/// more than distance (Gmsh's "elements per 2π", Bawin et al. 2020).
fn turn_length(k: f64, per_turn: f64) -> f64 {
    let k = k.abs();
    if k.is_nan() || k <= 1e-12 { f64::INFINITY } else { std::f64::consts::TAU / (k * per_turn) }
}

/// One target edge length for a whole mesh: explicit (`target_edge_rel` ×
/// diagonal), or the length that gives [`NOMINAL_FACES`] equilateral
/// triangles over the whole area, shortened for thin closed parts. (The
/// pipeline gives each part its own length and also looks at how close its
/// feature lines are: [`part_measure`], [`feature_density`].)
///
/// The shortening is the oracle's `ExpectedEdgeL` idea, applied where it
/// means something. The oracle reads how thin a shape is from its sphericity
/// `S = π^⅓(6V)^⅔ / A` and asks for `0.2 / S²` times the faces (a sphere and
/// a cube get none, a plate a hundred times wider than thick gets ten). But
/// a volume only exists for a closed part, and the sphericity of several
/// parts taken as one body falls with their number however compact each is.
/// So each connected part is judged alone: an open one counts as compact, a
/// closed one multiplies its own area's faces by `0.2 / S²`, between 1 and
/// [`DENSEST`]. The parts' multipliers, weighted by area, give the whole
/// mesh's, and the length shrinks by its square root.
pub fn target_length(positions: &[DVec3], triangles: &[[u32; 3]], params: &RemeshParams) -> f64 {
    if params.target_edge_rel > 0.0 {
        return params.target_edge_rel * super::bbox_diagonal(positions);
    }
    let mut parts = UnionFind::new(positions.len());
    // Per directed edge's sorted ends: uses, to tell closed parts from open.
    let mut uses: HashMap<(u32, u32), u32> = HashMap::with_capacity(triangles.len() * 3 / 2);
    for &[a, b, c] in triangles {
        parts.union(a, b);
        parts.union(a, c);
        for (u, v) in [(a, b), (b, c), (c, a)] {
            *uses.entry((u.min(v), u.max(v))).or_default() += 1;
        }
    }
    /// A connected part's area, six times its signed volume, and whether an
    /// edge of it has other than two faces.
    #[derive(Default, Clone, Copy)]
    struct Part {
        area: f64,
        volume6: f64,
        open: bool,
    }
    let mut measured: HashMap<u32, Part> = HashMap::new();
    for &[a, b, c] in triangles {
        let [pa, pb, pc] = [a, b, c].map(|v| positions[v as usize]);
        let part = measured.entry(parts.find(a)).or_default();
        part.area += 0.5 * (pb - pa).cross(pc - pa).length();
        part.volume6 += pa.dot(pb.cross(pc));
    }
    for (&(a, _), &count) in &uses {
        if count != 2
            && let Some(part) = measured.get_mut(&parts.find(a))
        {
            part.open = true;
        }
    }
    drop(uses);
    // Summed in a fixed order: the result must not depend on the map's.
    let mut measured: Vec<(u32, Part)> = measured.into_iter().collect();
    measured.sort_unstable_by_key(|&(root, _)| root);
    let (mut area, mut faces) = (0.0, 0.0);
    for &(_, part) in &measured {
        area += part.area;
        faces += part.area * denser(part.area, part.volume6, part.open);
    }
    if area <= 0.0 || faces <= 0.0 {
        return super::bbox_diagonal(positions) * 0.01;
    }
    // Faces per area rise by `faces / area`, so the length falls by its root.
    nominal_length(area) / (faces / area).sqrt()
}

/// The edge length that gives [`NOMINAL_FACES`] equilateral triangles over `area`.
pub fn nominal_length(area: f64) -> f64 {
    (area * 2.309 / NOMINAL_FACES).sqrt()
}

/// Faces a part gets at least, however small its share of the object: as
/// many as it came with, within these bounds.
const LEAST_FACES: (f64, f64) = (100.0, 400.0);

/// The longest edge a part of this area, which came with `faces` triangles,
/// may be remeshed at: the one that gives it as many again, at least 100 and
/// at most 400 ([`LEAST_FACES`]).
///
/// An object of many parts shares its faces out by area, and a small part's
/// share can be a handful: an edge as long as the part is thick. BrainStem's
/// 400 parts at the nominal length left one, 7% of the object long, with
/// its whole body inside a single patch of 8 quads, drawn flat across what
/// was a solid piece. A few hundred triangles are what a field and a layout
/// need to mean anything on a part with any shape to it. A part that came
/// as a dozen triangles has none to speak of, and 400 for each of those
/// took CommercialRefrigerator's 376 parts from 57,000 quads to 172,000.
///
/// Not below an eighth of the `nominal` length, though. A speck a
/// thousandth of the object across is not a part to give a hundred
/// triangles to (ABeautifulGame has one four thousand times under the
/// nominal length, and the run that tried took over ten minutes where
/// twenty seconds had done).
pub fn longest_length(area: f64, faces: usize, nominal: f64) -> f64 {
    let wanted = (area * 2.309 / (faces as f64).clamp(LEAST_FACES.0, LEAST_FACES.1)).sqrt();
    wanted.max(nominal / SHORTEST_FOR_A_SPECK)
}

/// A part's least-resolution length is never under the nominal length over this.
const SHORTEST_FOR_A_SPECK: f64 = 8.0;

/// How many times the nominal faces per area a part with this area, six
/// times this signed volume and these open edges should get: 1 for an open
/// or compact part, up to [`DENSEST`] for a thin closed one (see [`target_length`]).
fn denser(area: f64, volume6: f64, open: bool) -> f64 {
    let sphericity = std::f64::consts::PI.cbrt() * volume6.abs().powf(2.0 / 3.0) / area;
    if open || area.is_nan() || area <= 0.0 || sphericity.is_nan() {
        1.0
    } else {
        (0.2 / (sphericity * sphericity)).clamp(1.0, DENSEST)
    }
}

/// What one connected part's own edge length is worked out from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PartMeasure {
    /// Its triangles.
    pub faces: usize,
    pub area: f64,
    /// How many times the nominal faces per area the part should get for
    /// being thin and closed ([`target_length`]'s measure): 1 to [`DENSEST`].
    pub thin: f64,
    /// The length of its open edges (those with other than two faces).
    pub open_length: f64,
}

/// Measures one connected part, for a caller that remeshes each part at its
/// own length.
pub fn part_measure(positions: &[DVec3], triangles: impl Iterator<Item = [u32; 3]>) -> PartMeasure {
    side_measure(positions, triangles, DVec3::ZERO, 1.0, |_, _| false)
}

/// [`part_measure`] of the whole a part is one side of: `copies` of the
/// part, joined along the edges `seam` names, which lie on mirror planes
/// through `origin`.
///
/// The whole has `copies` times the faces, the area and the open edges
/// (the seam is not one: its two sides meet there), and the same volume
/// each: measured from a point on the planes, the part's open side adds
/// nothing to its volume, since the faces that would close it lie in the
/// planes.
pub fn side_measure(
    positions: &[DVec3],
    triangles: impl Iterator<Item = [u32; 3]>,
    origin: DVec3,
    copies: f64,
    seam: impl Fn(u32, u32) -> bool,
) -> PartMeasure {
    let (mut area, mut volume6, mut faces) = (0.0, 0.0, 0usize);
    let mut uses: HashMap<(u32, u32), u32> = HashMap::new();
    let at = |v: u32| positions.get(v as usize).copied().unwrap_or(DVec3::ZERO);
    for [a, b, c] in triangles {
        faces += 1;
        let [pa, pb, pc] = [a, b, c].map(at);
        area += 0.5 * (pb - pa).cross(pc - pa).length();
        volume6 += (pa - origin).dot((pb - origin).cross(pc - origin));
        for (u, v) in [(a, b), (b, c), (c, a)] {
            *uses.entry((u.min(v), u.max(v))).or_default() += 1;
        }
    }
    // Summed in a fixed order: the result must not depend on the map's.
    let mut open: Vec<(u32, u32)> =
        uses.into_iter().filter(|&((u, v), count)| count != 2 && !seam(u, v)).map(|(edge, _)| edge).collect();
    open.sort_unstable();
    let open_length: f64 = open.iter().map(|&(u, v)| (at(u) - at(v)).length()).sum();
    PartMeasure {
        faces: (faces as f64 * copies).round() as usize,
        area: area * copies,
        thin: denser(area * copies, volume6 * copies, !open.is_empty()),
        open_length: open_length * copies,
    }
}

/// Edges of the nominal length that should fit in the mean spacing between
/// a part's feature lines.
const EDGES_BETWEEN_FEATURES: f64 = 2.0;
/// The most a part's feature lines multiply its share of the faces by.
/// Lower than [`DENSEST`]: a thin wall needs its triangles to be a shape at
/// all, close feature lines only to be meshed well, and a part covered in
/// them (stitching, a noisy scan) would otherwise always get the most.
const DENSEST_FOR_FEATURES: f64 = 4.0;

/// How many times the nominal faces per area a part should get for how
/// close together its feature lines are. `feature_length` is the length of
/// the part's feature and open edges, `nominal` the nominal edge length.
///
/// A part's area over its feature length is the mean spacing between its
/// lines (exactly, for parallel lines). Quads longer than that spacing can't
/// follow the lines: every strip between two lines still gets its row of
/// quads, far smaller than the quads beside it, and the patches between are
/// too small for a pattern (BrainStem at the nominal length: 5,119 patches
/// for 35,000 quads, a tenth of them poor). So the length is shortened until
/// [`EDGES_BETWEEN_FEATURES`] of it fit in the spacing, which multiplies the
/// faces by the square of the shortening, up to [`DENSEST_FOR_FEATURES`].
pub fn feature_density(area: f64, feature_length: f64, nominal: f64) -> f64 {
    let spacing = area / feature_length;
    let shorter = EDGES_BETWEEN_FEATURES * nominal / spacing;
    if shorter.is_finite() && shorter > 1.0 { (shorter * shorter).min(DENSEST_FOR_FEATURES) } else { 1.0 }
}

/// Desired edge length at every vertex slot of `mesh` (dead slots get `target`).
///
/// With adaptivity on, the length follows the allowed deviation `tolerance`:
/// an edge of length h on a surface of curvature radius r strays about
/// `h²/8r` from it, and Dunyach et al. (EG 2013) size edges as
/// `h = √(6·tolerance·r − 3·tolerance²)` so triangles stay within the tolerance.
/// r is the *maximum* curvature radius at each free vertex, from a local jet
/// fit (see [`Ball::curvature`]); h is clamped to `[min_mult, max_mult] ×
/// target`. Feature and corner vertices sit on creases, which are features
/// rather than curvature, so they take the mean of their free neighbours. The
/// field is then smoothed twice (decision 0004).
///
/// A second rule shortens edges on thin parts, under `min_mult` if need be:
/// [`EDGES_PER_TURN`] round a full turn ([`turn_length`]). It goes by the
/// *net* curvature over a wider ball ([`Ball::net_curvature`]), so it holds
/// where the surface goes on turning the same way (a tail, a finger, a
/// wire) and not where a tight turn stops or turns back (a plate's rounded
/// rim, a fold, ripples). The quad stage has the same rule for quads
/// (`quad::turn`). Decision 0026.
pub fn vertex_sizes(mesh: &EditMesh, target: f64, tolerance: f64, params: &RemeshParams) -> Vec<f64> {
    let slots = mesh.vertex_slots();
    if !params.adaptive {
        return vec![target; slots];
    }
    let (lo, hi) = (params.min_mult * target, params.max_mult * target);
    let normals = vertex_normals(mesh);
    let curvature: Vec<Option<f64>> = (0..slots as u32)
        .into_par_iter()
        .map_init(Ball::default, |ball, v| {
            (mesh.is_vertex_alive(v) && mesh.kind(v) == VertexKind::Free)
                .then(|| ball.curvature(mesh, &normals, v, BALL_RADIUS * target))
        })
        .collect();
    let areas = vertex_areas(mesh);
    let free_sizes: Vec<Option<f64>> = (0..slots as u32)
        .into_par_iter()
        .map_init(Ball::default, |ball, v| {
            let k = curvature[v as usize]?;
            let by_deviation = error_bounded_length(k.abs(), tolerance).clamp(lo, hi);
            // Only where the turn is tight here is the wider ball looked at.
            if turn_length(k, EDGES_PER_TURN) >= by_deviation {
                return Some(by_deviation);
            }
            let net = ball.net_curvature(mesh, &curvature, &areas, v, TURN_RADIUS * target);
            Some(by_deviation.min(turn_length(net, EDGES_PER_TURN).max(TURN_FLOOR * target)))
        })
        .collect();
    let mut sizes: Vec<f64> = free_sizes.iter().map(|s| s.unwrap_or(target)).collect();
    let measured: Vec<bool> = free_sizes.iter().map(Option::is_some).collect();
    // Crease vertices borrow their free neighbours' sizes, but no more than
    // their own feature line's curvature allows: a long chord would cut a
    // curved crease just as it cuts a curved surface.
    for v in 0..slots as u32 {
        if !mesh.is_vertex_alive(v) || measured[v as usize] {
            continue;
        }
        let free: Vec<f64> =
            mesh.one_ring(v).filter(|&n| measured[n as usize]).map(|n| sizes[n as usize]).collect();
        let borrowed =
            if free.is_empty() { target.clamp(lo, hi) } else { free.iter().sum::<f64>() / free.len() as f64 };
        let line = match mesh.feature_neighbours(v).as_slice() {
            &[a, b] => {
                let reach = BALL_RADIUS * target;
                let k = menger_curvature(
                    mesh.walk_feature(v, a, reach),
                    mesh.position(v),
                    mesh.walk_feature(v, b, reach),
                );
                error_bounded_length(k, tolerance).clamp(lo, hi)
            }
            _ => hi,
        };
        sizes[v as usize] = borrowed.min(line);
    }
    for _ in 0..2 {
        let previous = sizes.clone();
        for v in 0..slots as u32 {
            if !mesh.is_vertex_alive(v) {
                continue;
            }
            let (sum, count) = mesh
                .one_ring(v)
                .fold((0.0, 0usize), |(sum, count), n| (sum + previous[n as usize], count + 1));
            if count == 0 {
                continue;
            }
            let neighbours = sum / count as f64;
            sizes[v as usize] = 0.5 * (previous[v as usize] + neighbours);
        }
    }
    sizes
}

/// Dunyach et al.'s edge length for curvature `k` and deviation `tolerance`
/// (infinite on flat surfaces; the tolerance itself where r < tolerance).
fn error_bounded_length(k: f64, tolerance: f64) -> f64 {
    if k <= 1e-12 {
        return f64::INFINITY;
    }
    let r = 1.0 / k;
    let squared = 6.0 * tolerance * r - 3.0 * tolerance * tolerance;
    if squared > 0.0 { squared.sqrt() } else { tolerance }
}

/// Curvature of the circle through three points (0 if they're collinear).
fn menger_curvature(a: DVec3, b: DVec3, c: DVec3) -> f64 {
    let product = (b - a).length() * (c - b).length() * (c - a).length();
    if product <= 0.0 { 0.0 } else { 2.0 * (b - a).cross(c - a).length() / product }
}

/// Area-weighted (unnormalized) vertex normals.
fn vertex_normals(mesh: &EditMesh) -> Vec<DVec3> {
    let mut normals = vec![DVec3::ZERO; mesh.vertex_slots()];
    for f in mesh.faces() {
        let corners = mesh.face_corners(f);
        let [a, b, c] = corners.map(|v| mesh.position(v));
        let n = (b - a).cross(c - a);
        for v in corners {
            normals[v as usize] += n;
        }
    }
    normals
}

/// A third of the area of the faces round each vertex slot.
fn vertex_areas(mesh: &EditMesh) -> Vec<f64> {
    let mut areas = vec![0.0; mesh.vertex_slots()];
    for f in mesh.faces() {
        let corners = mesh.face_corners(f);
        let [a, b, c] = corners.map(|v| mesh.position(v));
        let third = (b - a).cross(c - a).length() / 6.0;
        for v in corners {
            areas[v as usize] += third;
        }
    }
    areas
}

/// Reusable scratch for [`Ball::curvature`].
#[derive(Default)]
struct Ball {
    seen: HashSet<u32>,
    queue: Vec<u32>,
    samples: Vec<u32>,
    offsets: Vec<DVec3>,
}

impl Ball {
    /// Maximum absolute curvature at free vertex `v`, judged over the free
    /// surface within `radius` (flooding through free vertices only, so it
    /// never crosses a crease; at least the one-ring, at most [`MAX_BALL`]
    /// samples).
    ///
    /// The samples' heights over the tangent plane (ball-mean normal) are fitted
    /// by least squares with `h = a·x² + b·xy + c·y² + d`, a degree-2 osculating
    /// jet (Cazals & Pouget, SGP 2003) plus a constant. The principal
    /// curvatures are the eigenvalues of `[[2a, b], [b, 2c]]`. Zero-mean noise
    /// finer than the ball doesn't correlate with the quadratic terms, so it
    /// ends up in `d` rather than in the curvature. When the samples can't
    /// determine a quadric (e.g. all on a line) the sagitta estimate
    /// `2·Σ|height| / Σ d²` is used instead.
    ///
    /// The value is signed: positive where the surface bulges the way its
    /// normals point, negative where it is hollow. Its size is the largest
    /// principal curvature.
    fn curvature(&mut self, mesh: &EditMesh, normals: &[DVec3], v: u32, radius: f64) -> f64 {
        let p = mesh.position(v);
        // The ball is halved while the surface turns too far within it to be
        // a height field: a tube thinner than the ball is measured at its
        // own scale. Fitted whole, a ball that reaches half round a tube
        // reads a third of its curvature.
        let mut radius = radius;
        let (mut normal, mut magnitude);
        let mut halvings = 0;
        loop {
            self.seen.clear();
            self.queue.clear();
            self.samples.clear();
            self.seen.insert(v);
            self.queue.push(v);
            (normal, magnitude) = (DVec3::ZERO, 0.0);
            let mut head = 0;
            while let Some(&u) = self.queue.get(head) {
                head += 1;
                normal += normals[u as usize];
                magnitude += normals[u as usize].length();
                for n in mesh.one_ring(u) {
                    if self.samples.len() >= MAX_BALL {
                        break;
                    }
                    if !self.seen.insert(n) {
                        continue;
                    }
                    self.samples.push(n);
                    if mesh.kind(n) == VertexKind::Free && (mesh.position(n) - p).length() < radius {
                        self.queue.push(n);
                    }
                }
            }
            // Down to the vertex's own ring, there is nothing smaller to look at.
            let only_the_ring = self.queue.len() <= 1;
            if magnitude <= 0.0
                || normal.length() >= GENTLE * magnitude
                || only_the_ring
                || halvings >= MAX_HALVINGS
            {
                break;
            }
            radius *= 0.5;
            halvings += 1;
        }
        if magnitude <= 0.0 || normal.length() < MIN_COHERENCE * magnitude {
            return ring_curvature(mesh, v, normals[v as usize]);
        }
        let n = normal.normalize();
        self.offsets.clear();
        self.offsets.extend(self.samples.iter().map(|&u| mesh.position(u) - p));
        jet_curvature(&self.offsets, n).unwrap_or_else(|| {
            let (height, lean, d2) = self.offsets.iter().fold((0.0, 0.0, 0.0), |(h, lean, d2), e| {
                (h + e.dot(n).abs(), lean + e.dot(n), d2 + e.length_squared())
            });
            let size = if d2 > 0.0 { 2.0 * height / d2 } else { 0.0 };
            // Neighbours under the tangent plane: the surface bulges.
            if lean > 0.0 { -size } else { size }
        })
    }

    /// How far the surface turns one way within `radius` of `v`: the
    /// area-weighted mean of the signed `curvature` over the free vertices
    /// there (flooding as [`curvature`](Self::curvature) does, through at
    /// most [`MAX_TURN_BALL`] of them), as a size.
    ///
    /// Round a tube thinner than the ball it is the tube's own curvature,
    /// whatever the tube's section: all the turning there is, a full turn,
    /// over all the surface there is. Where a tight turn stops (the rim of
    /// a plate, a rounded corner) the flat beside it counts as well, and
    /// where it turns back (a fold between two bulges, ripples) the two
    /// signs cancel.
    fn net_curvature(
        &mut self,
        mesh: &EditMesh,
        curvature: &[Option<f64>],
        areas: &[f64],
        v: u32,
        radius: f64,
    ) -> f64 {
        let p = mesh.position(v);
        self.seen.clear();
        self.queue.clear();
        self.seen.insert(v);
        self.queue.push(v);
        let (mut turning, mut area) = (0.0, 0.0);
        let mut head = 0;
        while let Some(&u) = self.queue.get(head) {
            head += 1;
            if let Some(k) = curvature[u as usize].filter(|k| k.is_finite()) {
                turning += areas[u as usize] * k;
                area += areas[u as usize];
            }
            for n in mesh.one_ring(u) {
                if self.queue.len() >= MAX_TURN_BALL {
                    break;
                }
                if mesh.kind(n) == VertexKind::Free
                    && (mesh.position(n) - p).length() < radius
                    && self.seen.insert(n)
                {
                    self.queue.push(n);
                }
            }
        }
        if area > 0.0 { (turning / area).abs() } else { 0.0 }
    }
}

/// The principal curvature largest in size of the quadric `a·x² + b·xy + c·y² + d`
/// fitted to `offsets` (sample − centre) over the plane with normal `n`, or
/// `None` if the samples don't determine it. Positive where the quadric
/// falls away from `n` (the surface bulges). Coordinates are scaled to the
/// sample spread so the normal equations stay well conditioned.
fn jet_curvature(offsets: &[DVec3], n: DVec3) -> Option<f64> {
    if offsets.len() < 5 {
        return None;
    }
    let (t1, t2) = n.any_orthonormal_pair();
    let scale = offsets.iter().map(|e| e.length()).fold(0.0, f64::max);
    if scale <= 0.0 {
        return None;
    }
    let (mut m, mut rhs) = (DMat4::ZERO, DVec4::ZERO);
    for e in offsets {
        // Tangent direction stretched to the chord length: on a sphere a chord
        // d long rises exactly d²/2R, so the quadric is exact at any ball size
        // (the plain projection is only exact to second order).
        let (px, py) = (e.dot(t1), e.dot(t2));
        let stretch = e.length() / px.hypot(py).max(f64::MIN_POSITIVE);
        let (x, y, h) = (px * stretch / scale, py * stretch / scale, e.dot(n) / scale);
        let row = DVec4::new(x * x, x * y, y * y, 1.0);
        m += DMat4::from_cols(row * row.x, row * row.y, row * row.z, row * row.w);
        rhs += row * h;
    }
    // Determinant of the (scaled, so O(1)-sized) normal matrix: near zero when
    // the samples don't span both tangent directions.
    if m.determinant().abs() < 1e-9 * (offsets.len() as f64).powi(4) {
        return None;
    }
    let [a, b, c, _] = (m.inverse() * rhs).to_array();
    let (mean, half_gap) = (a + c, ((a - c) * (a - c) + b * b).sqrt());
    let (first, second) = (mean + half_gap, mean - half_gap);
    let largest = if first.abs() >= second.abs() { first } else { second };
    Some(-largest / scale)
}

/// One-ring curvature: a vertex whose incident face normals spread by angle θ
/// (from their mean) over edges of mean length ℓ has κ ≈ 2θ/ℓ. Signed as
/// [`Ball::curvature`] is, going by which side of `normal` the ring lies on.
fn ring_curvature(mesh: &EditMesh, v: u32, normal: DVec3) -> f64 {
    let normals: Vec<DVec3> = mesh.faces_around(v).iter().map(|&f| mesh.face_normal(f)).collect();
    let mean = normals.iter().copied().sum::<DVec3>().normalize_or_zero();
    let spread = normals.iter().map(|n| n.dot(mean).clamp(-1.0, 1.0).acos()).fold(0.0, f64::max);
    let p = mesh.position(v);
    let ring: Vec<u32> = mesh.one_ring(v).collect();
    let mean_len =
        ring.iter().map(|&n| (mesh.position(n) - p).length()).sum::<f64>() / ring.len().max(1) as f64;
    let size = if mean_len > 0.0 { 2.0 * spread / mean_len } else { 0.0 };
    let lean: f64 = ring.iter().map(|&n| (mesh.position(n) - p).dot(normal)).sum();
    if lean > 0.0 { -size } else { size }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sphere(radius: f64) -> EditMesh {
        let (p, t) = crate::remesh::tests::sphere_mesh(8, 16);
        let p: Vec<DVec3> = p.into_iter().map(|q| q * radius).collect();
        EditMesh::from_triangles(&p, &t, &[]).unwrap()
    }

    #[test]
    fn adaptive_sizes_shrink_on_curved_regions() {
        let params = RemeshParams::default();
        let mean = |m: &EditMesh, target: f64| {
            let s = vertex_sizes(m, target, 0.01, &params);
            s.iter().sum::<f64>() / s.len() as f64
        };
        // Same target, tighter curvature → shorter edges.
        let (tight, loose) = (sphere(1.0), sphere(4.0));
        assert!(mean(&tight, 1.0) < mean(&loose, 1.0));
        let off = RemeshParams { adaptive: false, ..params.clone() };
        assert!(vertex_sizes(&tight, 1.0, 0.01, &off).iter().all(|&s| s == 1.0));
    }

    #[test]
    fn ball_curvature_is_exact_on_a_sphere() {
        for radius in [0.5, 2.0] {
            let mesh = sphere(radius);
            let normals = vertex_normals(&mesh);
            let mut ball = Ball::default();
            // A vertex away from the poles; the ball spans a few rings.
            let k = ball.curvature(&mesh, &normals, 40, 0.6 * radius);
            assert!((k * radius - 1.0).abs() < 0.1, "κ·R = {} at R = {radius}", k * radius);
        }
    }

    #[test]
    fn ball_curvature_finds_the_maximum_on_a_cylinder() {
        // Radius 0.5, length 2: principal curvatures 2 (around) and 0 (along).
        let (rings, segments) = (40u32, 48u32);
        let mut p = Vec::new();
        for i in 0..=rings {
            for j in 0..segments {
                let a = std::f64::consts::TAU * f64::from(j) / f64::from(segments);
                p.push(DVec3::new(0.5 * a.cos(), 0.5 * a.sin(), 2.0 * f64::from(i) / f64::from(rings)));
            }
        }
        let mut t = Vec::new();
        for i in 0..rings {
            for j in 0..segments {
                let (a, b) = (i * segments + j, i * segments + (j + 1) % segments);
                t.extend([[a, b, b + segments], [a, b + segments, a + segments]]);
            }
        }
        let mesh = EditMesh::from_triangles(&p, &t, &[]).unwrap();
        let normals = vertex_normals(&mesh);
        let k = Ball::default().curvature(&mesh, &normals, (rings / 2) * segments, 0.3);
        assert!((k - 2.0).abs() < 0.2, "κ_max = {k}, expected 2");
    }

    #[test]
    fn menger_curvature_of_a_circle() {
        let on = |a: f64| DVec3::new(2.0 * a.cos(), 2.0 * a.sin(), 0.0);
        assert!((menger_curvature(on(0.0), on(0.3), on(1.1)) - 0.5).abs() < 1e-12);
        assert_eq!(menger_curvature(DVec3::ZERO, DVec3::X, 2.0 * DVec3::X), 0.0);
    }

    #[test]
    fn crease_vertices_follow_their_line_curvature() {
        // A cylinder's rim circle (radius 0.5) is a feature line on a surface
        // that is flat along the rim's band only when far from it; sizes at the
        // rim may not exceed what the circle's curvature allows.
        let (rings, segments) = (6u32, 96u32);
        let mut p = Vec::new();
        for i in 0..=rings {
            for j in 0..segments {
                let a = std::f64::consts::TAU * f64::from(j) / f64::from(segments);
                p.push(DVec3::new(0.5 * a.cos(), 0.5 * a.sin(), 0.05 * f64::from(i)));
            }
        }
        let mut t = Vec::new();
        for i in 0..rings {
            for j in 0..segments {
                let (a, b) = (i * segments + j, i * segments + (j + 1) % segments);
                t.extend([[a, b, b + segments], [a, b + segments, a + segments]]);
            }
        }
        let mesh = EditMesh::from_triangles(&p, &t, &[]).unwrap();
        let params = RemeshParams { max_mult: 10.0, min_mult: 0.01, ..RemeshParams::default() };
        let tolerance = 0.001;
        let sizes = vertex_sizes(&mesh, 1.0, tolerance, &params);
        let allowed = error_bounded_length(2.0, tolerance);
        assert!(sizes[0] <= 1.05 * allowed, "rim size {} vs {allowed}", sizes[0]);
    }

    #[test]
    fn one_side_of_a_closed_part_measures_as_the_whole() {
        // A slab a twentieth as thick as wide, centred on the origin, and the
        // half of it with x >= 0: open where the plane cut it, which is not
        // an open edge of the whole.
        let (p, t, _) = crate::testmesh::cube(4);
        let slab: Vec<DVec3> =
            p.iter().map(|&v| (v - DVec3::splat(0.5)) * DVec3::new(1.0, 1.0, 0.05)).collect();
        let whole = part_measure(&slab, t.iter().copied());
        assert!(whole.thin > 2.0 && whole.thin < 2.5, "thin, and not at either limit: {whole:?}");
        let kept: Vec<[u32; 3]> =
            t.iter().copied().filter(|tri| tri.iter().all(|&v| slab[v as usize].x >= 0.0)).collect();
        assert_eq!(kept.len(), t.len() / 2);
        let on_plane = |u: u32, v: u32| slab[u as usize].x == 0.0 && slab[v as usize].x == 0.0;
        let side = side_measure(&slab, kept.iter().copied(), DVec3::ZERO, 2.0, on_plane);
        assert_eq!((side.faces, side.open_length), (whole.faces, 0.0));
        assert!((side.area - whole.area).abs() < 1e-12 && (side.thin - whole.thin).abs() < 1e-9, "{side:?}");
        // On its own the half is an open shell of half the area, which is
        // never thin.
        let alone = part_measure(&slab, kept.iter().copied());
        assert!((alone.area - 1.1).abs() < 1e-12 && (alone.open_length - 2.1).abs() < 1e-12, "{alone:?}");
        assert_eq!(alone.thin, 1.0);
        // From a point off the plane the open side counts as volume, and the
        // slab reads as thicker than it is: the origin has to be on the plane.
        let off = side_measure(&slab, kept.iter().copied(), DVec3::new(-1.0, 0.0, 0.0), 2.0, on_plane);
        assert!(off.thin < 0.6 * whole.thin, "{off:?} {whole:?}");
    }

    #[test]
    fn a_thin_tube_gets_its_edges_round_the_turn() {
        // A tube of radius 0.05 at a target of 0.2 and a loose tolerance: the
        // deviation rule alone settles for the shortest it may ask, 0.06,
        // five edges round. Twelve round is 2π · 0.05 / 12 = 0.026.
        let (p, t) = crate::testmesh::cylinder(24, 16);
        let p: Vec<DVec3> = p.into_iter().map(|q| DVec3::new(0.05 * q.x, 0.05 * q.y, q.z)).collect();
        let mesh = EditMesh::from_triangles(&p, &t, &[]).unwrap();
        let params = RemeshParams::default();
        let sizes = vertex_sizes(&mesh, 0.2, 0.01, &params);
        let wanted = turn_length(1.0 / 0.05, EDGES_PER_TURN);
        assert!((wanted - 0.02618).abs() < 1e-4);
        let middle: Vec<f64> = (0..mesh.vertex_slots() as u32)
            .filter(|&v| mesh.kind(v) == VertexKind::Free && mesh.position(v).z.abs() < 0.5)
            .map(|v| sizes[v as usize])
            .collect();
        assert!(!middle.is_empty());
        for &s in &middle {
            assert!(s < 0.045 && s > 0.6 * wanted, "size {s} against {wanted} round the turn");
        }
        // A wide tube at the same target is left to the deviation rule.
        assert_eq!(turn_length(1.0, EDGES_PER_TURN).max(0.02), std::f64::consts::TAU / 12.0);
        assert_eq!(turn_length(0.0, EDGES_PER_TURN), f64::INFINITY);
        // And the rule stops at a tenth of the target however tight the turn.
        let hair: Vec<DVec3> = p.iter().map(|q| DVec3::new(0.02 * q.x, 0.02 * q.y, q.z)).collect();
        let mesh = EditMesh::from_triangles(&hair, &t, &[]).unwrap();
        let sizes = vertex_sizes(&mesh, 0.2, 0.01, &params);
        assert!(sizes.iter().all(|&s| s >= 0.02 - 1e-12), "under the floor: {sizes:?}");
    }

    #[test]
    fn the_rounded_rim_of_a_plate_is_not_a_turn() {
        // A plate 0.1 thick and 2 wide whose rims are half circles of radius
        // 0.05: the same curvature as the thin tube's above, at the same
        // target of 0.2. A tube turns all the way round and gets edges of
        // 0.026. A rim turns half way and stops: its edges stay at the
        // deviation rule's shortest, 0.06.
        let (radius, flat) = (0.05, 2.0);
        let outline = |j: u32| {
            // 8 steps round the right rim, 16 along the top, 8 round the left rim, 16 along the bottom.
            let turn = |step: u32| std::f64::consts::PI * f64::from(step) / 8.0 - std::f64::consts::FRAC_PI_2;
            match j {
                0..8 => (flat / 2.0 + radius * turn(j).cos(), radius * turn(j).sin()),
                8..24 => (flat / 2.0 - flat * f64::from(j - 8) / 16.0, radius),
                24..32 => (-flat / 2.0 - radius * turn(j - 24).cos(), -radius * turn(j - 24).sin()),
                _ => (-flat / 2.0 + flat * f64::from(j - 32) / 16.0, -radius),
            }
        };
        let (p, t) = crate::testmesh::extrusion(48, 64, 2.0, outline);
        let mesh = EditMesh::from_triangles(&p, &t, &[]).unwrap();
        let normals = vertex_normals(&mesh);
        // The middle of the right rim, half way along.
        let apex = 32 * 48 + 4;
        assert!((mesh.position(apex) - DVec3::new(1.05, 0.0, 1.0)).length() < 1e-9);
        let k = Ball::default().curvature(&mesh, &normals, apex, BALL_RADIUS * 0.2);
        assert!(k > 12.0 && k < 30.0, "the rim's own curvature, 1 / 0.05, read as {k}");
        assert!(turn_length(k, EDGES_PER_TURN) < 0.06, "tight enough to be a turn if it went on");
        let sizes = vertex_sizes(&mesh, 0.2, 0.01, &RemeshParams::default());
        for v in 0..mesh.vertex_slots() as u32 {
            let z = mesh.position(v).z;
            if mesh.kind(v) == VertexKind::Free && z > 0.5 && z < 1.5 {
                assert!(
                    sizes[v as usize] >= 0.06 - 1e-9,
                    "size {} at {}",
                    sizes[v as usize],
                    mesh.position(v)
                );
            }
        }
    }

    #[test]
    fn ripples_are_not_a_turn() {
        // A 6 × 6 sheet rippled along x, one wave to a target length of 1
        // and 0.15 high: every crest and trough is tight enough for the turn
        // rule on its own, but they turn opposite ways and no part of the
        // sheet goes round. Its edges stay at the deviation rule's shortest.
        let (p, t) = crate::testmesh::grid(60, 60);
        let p: Vec<DVec3> = p
            .into_iter()
            .map(|q| DVec3::new(0.1 * q.x, 0.1 * q.y, 0.15 * (std::f64::consts::TAU * 0.1 * q.x).cos()))
            .collect();
        let mesh = EditMesh::from_triangles(&p, &t, &[]).unwrap();
        let normals = vertex_normals(&mesh);
        let at = |x: u32, y: u32| y * 61 + x;
        let crest = Ball::default().curvature(&mesh, &normals, at(30, 30), BALL_RADIUS);
        let trough = Ball::default().curvature(&mesh, &normals, at(35, 30), BALL_RADIUS);
        assert!(crest > 0.0 && trough < 0.0, "a crest {crest} bulges and a trough {trough} is hollow");
        assert!(turn_length(crest, EDGES_PER_TURN) < 0.3 && turn_length(trough, EDGES_PER_TURN) < 0.3);
        let sizes = vertex_sizes(&mesh, 1.0, 0.01, &RemeshParams::default());
        for v in 0..mesh.vertex_slots() as u32 {
            let q = mesh.position(v);
            if q.x > 1.5 && q.x < 4.5 && q.y > 1.5 && q.y < 4.5 {
                assert!(sizes[v as usize] >= 0.3 - 1e-9, "size {} at {q}", sizes[v as usize]);
            }
        }
    }

    #[test]
    fn error_bounded_length_follows_dunyach() {
        // r = 1, ε = 0.01: h = √(0.06 − 0.0003).
        assert!((error_bounded_length(1.0, 0.01) - 0.0597_f64.sqrt()).abs() < 1e-12);
        assert_eq!(error_bounded_length(0.0, 0.01), f64::INFINITY);
        assert_eq!(error_bounded_length(1000.0, 0.01), 0.01, "r below the tolerance");
    }

    #[test]
    fn fine_corrugation_is_not_curvature() {
        // A flat 1×1 grid of 100×100 cells whose rows alternate z = ±0.001:
        // neighbouring faces meet at a sharp angle, yet at a 0.1 target the
        // surface is flat, so sizes stay above the target.
        let n = 100u32;
        let mut p = Vec::new();
        for y in 0..=n {
            for x in 0..=n {
                let z = if y % 2 == 0 { 0.001 } else { -0.001 };
                p.push(DVec3::new(f64::from(x) / f64::from(n), f64::from(y) / f64::from(n), z));
            }
        }
        let w = n + 1;
        let mut t = Vec::new();
        for y in 0..n {
            for x in 0..n {
                let a = y * w + x;
                t.extend([[a, a + 1, a + w + 1], [a, a + w + 1, a + w]]);
            }
        }
        let mesh = EditMesh::from_triangles(&p, &t, &[]).unwrap();
        let params = RemeshParams { max_mult: 3.0, ..RemeshParams::default() };
        let sizes = vertex_sizes(&mesh, 0.1, 1.4 / 2500.0, &params);
        let centre = (n / 2) * w + n / 2;
        assert!(sizes[centre as usize] > 0.1, "noise shrank the size to {}", sizes[centre as usize]);
        assert!(
            ring_curvature(&mesh, centre, DVec3::Z).abs() > 10.0,
            "the one-ring alone sees the noise as curvature"
        );
    }
}
