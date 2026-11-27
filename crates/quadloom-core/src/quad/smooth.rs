//! Relaxing the quad mesh on the surface.
//!
//! Patterns are placed patch by patch, so quads kink where patches meet and
//! vary in shape inside them. A round of relaxing moves every vertex toward
//! a blend of two targets:
//!
//! - **shape**: for each quad around it, the corner of the square that best
//!   fits that quad (same centre and size, turned to match, wound the way
//!   the surface faces). Squares are what every quad wants to be, and a quad
//!   that has folded over is pushed back the right way round. A quad with
//!   two opposite sides on feature lines (one of a strip between the two)
//!   fits a rectangle instead, laid along the lines with the quad's own
//!   length and width: the lines decide how wide it is, and right angles
//!   are all that is left to want.
//! - **spacing**: the average of its neighbours, which evens sizes out.
//!
//! The quads round a vertex do not pull alike ([`Fit::firm`]). A corner
//! moved by a given length along a quad's side leans the quad by that length
//! over the quad's *other* dimension, so a small quad is leant more than a
//! large one, and a thin strip quad much more than the wide quads beside it.
//! Where a strip one quad thick runs between two feature lines, with the
//! vertices of one line held where other lines meet it, this is what lines
//! the other line's vertices up across the strip: counted alike, the strip's
//! two quads lose to the two wide ones on the far side and to spacing, and
//! every quad of the strip leans.
//!
//! How a vertex may follow that pull depends on where it is ([`Hold`]):
//!
//! - on the open surface it moves **within the surface**: the part of the
//!   pull along the quad mesh's own normal at the vertex is dropped before
//!   the result is put back on the surface. Averaging in space and snapping
//!   back, as this stage once did, wears a tip or a thin rim down a little
//!   every round. The mesh's normal, not the surface's: at the tip of a cone
//!   the surface has only the slanted normal of whichever triangle the
//!   vertex is on, and a pull straight down the axis slides along it.
//! - on a feature line or border it **slides along the line**, and its
//!   spacing target is its two neighbours on the line.
//! - at a corner or junction of feature lines it stays.
//!
//! A vertex is put back on the triangles of its own patches and their
//! neighbours that face the way it does, so it can't jump through a thin part.
//!
//! Staying on the surface is not the same as covering it. Quads can slide
//! down off the end of a thin tail and every vertex is still on the surface.
//! So a free vertex gets a third pull, **cover**: every vertex of the
//! surface's own mesh finds the nearest point of the quad mesh, and what
//! separates the two is shared out over the corners of the quad it found. A
//! tip the quads have left pulls the nearest of them back out to it.
//!
//! Relaxing moves all vertices at once, toward where an average says they
//! should be. That leaves a few quads folded or badly pinched, usually
//! where the surface turns sharply under one quad. [`repair`] then takes the
//! vertices of those quads one at a time and tries a handful of positions for
//! each, keeping one only if the worst quad around the vertex gets better.

use hashbrown::HashSet;

use glam::{DMat3, DVec2, DVec3};
use rayon::prelude::*;

use super::curves::Curves;
use crate::control::{Control, Stage};
use crate::mesh::PolyMesh;
use crate::spatial::TriangleBvh;
use crate::trace::TraceOutput;

/// What relaxing may do with a vertex.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Hold {
    /// Moves within the surface.
    Free,
    /// Stays where it is.
    Fixed,
    /// Slides along feature chain `chain`; it is at arc length `at`.
    Curve { chain: u32, at: f64 },
}

/// Share of the spacing target in a free vertex's pull (the rest is shape).
/// Shape alone leaves sizes uneven; spacing alone lets quads skew.
const SPACING: f64 = 0.3;
/// The same for a vertex on a feature line, where spacing is along the line.
const SPACING_ON_CURVE: f64 = 0.5;
/// Fraction of the way each round moves toward the target (a full step lets
/// a checkerboard wobble survive or grow).
const STEP: f64 = 0.6;
/// A vertex on a line moves at most this share of the gap to the neighbour
/// it moves toward in one round, so vertices never pass each other.
const CURVE_REACH: f64 = 0.4;
/// A triangle is one the vertex may land on if its normal is within about
/// 120° of the vertex's own: the far side of a thin part faces away.
const FACING: f64 = -0.5;
/// The cover pull moves a vertex at most this share of its mean edge length
/// in one round.
const COVER_REACH: f64 = 0.5;
/// A quad whose worst corner is below this (the sine of its angle, so about
/// 9°) is repaired after relaxing.
const PINCHED: f64 = 0.15;
/// Repair stops caring about a vertex's worst quad once it is this good
/// (about 17°), and picks among such positions by the average instead.
const SOUND: f64 = 0.3;
/// Passes of repair, and how far along the way to each target a vertex is tried.
const REPAIR_PASSES: usize = 12;
const REPAIR_STEPS: [f64; 3] = [0.35, 0.7, 1.0];

/// A strip quad counts as at most this many times thinner than the quads
/// round it when its hold on a corner is weighed ([`Fit::firm`]).
const SLENDEREST: f64 = 10.0;
/// A free quad counts as at most this many times smaller than the largest
/// quad round a vertex when its hold on the vertex is weighed.
const SMALLEST: f64 = 2.0;

/// The corners of the unit square as directions from its centre, in order.
const SQUARE: [DVec2; 4] =
    [DVec2::new(1.0, 0.0), DVec2::new(0.0, 1.0), DVec2::new(-1.0, 0.0), DVec2::new(0.0, -1.0)];

/// Relaxes `mesh` in place for `iterations` rounds. `holds` says how each
/// vertex may move (and is updated as vertices slide), `feature_edges` are the
/// mesh's edges along feature lines, `patch_of_face` names the layout patch
/// of each face (to limit where a vertex may land).
#[allow(clippy::too_many_arguments)]
pub fn smooth(
    mesh: &mut PolyMesh,
    holds: &mut [Hold],
    feature_edges: &[[u32; 2]],
    curves: &Curves,
    patch_of_face: &[u32],
    trace: &TraceOutput,
    iterations: usize,
    control: &Control,
) {
    if iterations == 0 || mesh.positions.is_empty() {
        return;
    }
    let n = mesh.positions.len();
    let mut neighbours: Vec<Vec<u32>> = vec![Vec::new(); n];
    let mut patches: Vec<Vec<u32>> = vec![Vec::new(); n];
    // Per vertex, the quads around it as (face, which corner it is).
    let mut corners: Vec<Vec<(u32, u8)>> = vec![Vec::new(); n];
    let faces: Vec<&[u32]> = mesh
        .face_offsets
        .windows(2)
        .map(|w| mesh.face_indices.get(w[0] as usize..w[1] as usize).unwrap_or_default())
        .collect();
    for (f, face) in faces.iter().enumerate() {
        let patch = patch_of_face.get(f).copied().unwrap_or(u32::MAX);
        for (i, &v) in face.iter().enumerate() {
            let next = face[(i + 1) % face.len()];
            let prev = face[(i + face.len() - 1) % face.len()];
            if face.len() == 4
                && let Some(list) = corners.get_mut(v as usize)
            {
                list.push((f as u32, i as u8));
            }
            if let Some(list) = neighbours.get_mut(v as usize) {
                for u in [next, prev] {
                    if !list.contains(&u) {
                        list.push(u);
                    }
                }
            }
            if let Some(list) = patches.get_mut(v as usize)
                && !list.contains(&patch)
            {
                list.push(patch);
            }
        }
    }
    // Per vertex, its neighbours along a feature line.
    let mut along: Vec<Vec<u32>> = vec![Vec::new(); n];
    for &[a, b] in feature_edges {
        if (a as usize) < n && (b as usize) < n && a != b {
            along[a as usize].push(b);
            along[b as usize].push(a);
        }
    }
    let on_line = |a: u32, b: u32| along.get(a as usize).is_some_and(|list| list.contains(&b));
    let tm = &trace.mesh;
    let surface = TriangleBvh::new(&tm.positions, &tm.triangles);
    let face_normal: Vec<DVec3> = tm
        .triangles
        .iter()
        .map(|t| {
            let [a, b, c] = t.map(|v| tm.positions.get(v as usize).copied().unwrap_or(DVec3::ZERO));
            (b - a).cross(c - a).normalize_or_zero()
        })
        .collect();
    // A vertex lands on its own patches and their neighbours: its own patches
    // alone would clamp a drifting vertex onto a shared patch corner.
    let regions = projection_regions(&patches, trace);
    let world = Surface { bvh: &surface, face_normal: &face_normal, regions: &regions, trace };
    let cover = Cover::new(trace, &face_normal);
    // The way the surface faces at each vertex, kept up as vertices move.
    let mut normals: Vec<DVec3> = {
        let positions = &mesh.positions;
        (0..n)
            .into_par_iter()
            .map(|v| world.land(v, positions[v], DVec3::ZERO, positions[v]).map_or(DVec3::ZERO, |(_, m)| m))
            .collect()
    };

    for round in 0..iterations {
        control.set_progress(Stage::Smooth, round as f32 / iterations as f32);
        let current = &mesh.positions;
        // The shape every quad should take, and the quad's area vector
        // turned the way the surface faces.
        let fits: Vec<(Option<Fit>, DVec3)> = faces
            .par_iter()
            .map(|face| {
                let &[a, b, c, d] = *face else { return (None, DVec3::ZERO) };
                let ids = [a, b, c, d];
                let facing: DVec3 =
                    ids.iter().map(|&v| normals.get(v as usize).copied().unwrap_or(DVec3::ZERO)).sum();
                let p = ids.map(|v| current.get(v as usize).copied().unwrap_or(DVec3::ZERO));
                let area = (p[2] - p[0]).cross(p[3] - p[1]);
                let lined = [0, 1, 2, 3].map(|k| on_line(ids[k], ids[(k + 1) % 4]));
                (fit_quad(p, facing, lined), if area.dot(facing) < 0.0 { -area } else { area })
            })
            .collect();
        let uncovered = cover.pull(current, &faces, patch_of_face, &regions);
        let holds_now: &[Hold] = holds;
        let moved: Vec<(DVec3, DVec3, Hold)> = (0..n)
            .into_par_iter()
            .map(|v| {
                let (p, hold) = (current[v], holds_now.get(v).copied().unwrap_or(Hold::Fixed));
                let stay = (p, normals[v], hold);
                if hold == Hold::Fixed || neighbours[v].is_empty() {
                    return stay;
                }
                // What the quads round the vertex ask of it, as a least-squares
                // problem: `firm` is the sum of their holds (a matrix: each
                // holds differently along its two sides), `wanted` the holds
                // applied to the moves to their shapes' corners, `loose` what
                // spacing is weighed against (a free quad's hold, or 1 for a
                // strip quad).
                let (mut firm, mut wanted) = (DMat3::ZERO, DVec3::ZERO);
                let mut loose = 0.0;
                let mut own = DVec3::ZERO;
                let around = || corners[v].iter().filter_map(|&(f, k)| Some((fits.get(f as usize)?, k)));
                let largest = around()
                    .filter_map(|((fit, _), _)| fit.as_ref())
                    .fold(0.0, |most: f64, fit| most.max(fit.half[0]).max(fit.half[1]));
                for ((fit, area), k) in around() {
                    own += *area;
                    if let Some(fit) = fit {
                        let to = fit.corners[k as usize] - p;
                        let holds = fit.firm(largest);
                        for (axis, hold) in fit.axes.into_iter().zip(holds) {
                            firm += hold * outer(axis);
                            wanted += axis * (hold * axis.dot(to));
                        }
                        // A strip quad's hold along the strip is not spacing's
                        // to match: lining up across the strip comes first.
                        loose += if fit.strip { 1.0 } else { holds[0] };
                    }
                }
                let mean = |list: &[u32]| {
                    list.iter().map(|&u| current[u as usize]).sum::<DVec3>() / list.len() as f64
                };
                match hold {
                    Hold::Curve { chain, at } => {
                        let Some(line) = curves.chain(chain) else { return stay };
                        // Spacing on a line is between its two neighbours on the line.
                        let &[a, b] = along[v].as_slice() else { return stay };
                        let tangent = line.tangent(at);
                        let to_spacing = (mean(&[a, b]) - p).dot(tangent);
                        // Along the line only: one unknown.
                        let spacing = loose * SPACING_ON_CURVE / (1.0 - SPACING_ON_CURVE);
                        let hold_along = tangent.dot(firm * tangent) + spacing;
                        let pull = if hold_along > 0.0 && hold_along.is_finite() {
                            STEP * (wanted.dot(tangent) + spacing * to_spacing) / hold_along
                        } else {
                            STEP * to_spacing
                        };
                        let (back, ahead) = reach_along(p, tangent, current[a as usize], current[b as usize]);
                        let slide = if pull.is_finite() { pull.clamp(-back, ahead) } else { 0.0 };
                        let at = line.bound(at + slide);
                        (line.point(at), normals[v], Hold::Curve { chain, at })
                    }
                    _ => {
                        let to_spacing = mean(&neighbours[v]) - p;
                        let spacing = loose * SPACING / (1.0 - SPACING);
                        let solved = (firm + DMat3::from_diagonal(DVec3::splat(spacing))).inverse()
                            * (wanted + spacing * to_spacing);
                        let pull =
                            STEP * if spacing > 0.0 && solved.is_finite() { solved } else { to_spacing };
                        // Within the surface only: what points along the normal would
                        // carry the vertex off a tip or a rim before it is put back.
                        let own = own.try_normalize().unwrap_or(normals[v]);
                        let within = pull - own * own.dot(pull);
                        // Toward the surface the quads here don't reach, by no
                        // more than a share of the vertex's own edge length.
                        let reach = COVER_REACH
                            * neighbours[v].iter().map(|&u| (current[u as usize] - p).length()).sum::<f64>()
                            / neighbours[v].len() as f64;
                        let out = uncovered.get(v).copied().unwrap_or(DVec3::ZERO).clamp_length_max(reach);
                        match world.land(v, p + within + out, normals[v], p) {
                            Some((q, m)) if q.is_finite() => (q, m, hold),
                            _ => stay,
                        }
                    }
                }
            })
            .collect();
        let next: Vec<DVec3> = moved.iter().map(|m| m.0).collect();
        let kept = guard_collapse(&mesh.positions, next, &neighbours);
        for (v, (to, normal, hold)) in moved.into_iter().enumerate() {
            // A move the guard refused leaves the vertex, and where it is on its line, as they were.
            if kept[v] == to {
                normals[v] = normal;
                if let Some(slot) = holds.get_mut(v) {
                    *slot = hold;
                }
            }
        }
        mesh.positions = kept;
    }
    repair(&mut mesh.positions, holds, &faces, &corners, &neighbours, &along, &mut normals, curves, &world);
}

/// How far a vertex at `p` on a line with direction `tangent` may slide in
/// one round, backward and forward: [`CURVE_REACH`] of the gap to its
/// neighbour on that side. The nearer gap for both if its two neighbours
/// `a` and `b` are not one behind and one ahead (a line doubling back).
fn reach_along(p: DVec3, tangent: DVec3, a: DVec3, b: DVec3) -> (f64, f64) {
    let (to_a, to_b) = (a - p, b - p);
    let (gap_a, gap_b) = (CURVE_REACH * to_a.length(), CURVE_REACH * to_b.length());
    match (to_a.dot(tangent) > 0.0, to_b.dot(tangent) > 0.0) {
        (false, true) => (gap_a, gap_b),
        (true, false) => (gap_b, gap_a),
        _ => (gap_a.min(gap_b), gap_a.min(gap_b)),
    }
}

/// The worst corner of quad `p` seen from the side `facing` points to: the
/// sine of the corner's angle in the quad's own plane, so 1 for a square, 0
/// for a corner squeezed flat or opened straight, negative for a reflex
/// corner or a quad wound against the surface. −1 for a collapsed quad.
fn worst_corner(p: [DVec3; 4], facing: DVec3) -> f64 {
    let own = (p[2] - p[0]).cross(p[3] - p[1]);
    let Some(own) = own.try_normalize() else { return -1.0 };
    let normal = if own.dot(facing) < 0.0 { -own } else { own };
    let mut worst = f64::INFINITY;
    for k in 0..4 {
        let (next, prev) = (p[(k + 1) % 4] - p[k], p[(k + 3) % 4] - p[k]);
        let scale = next.length() * prev.length();
        if scale.is_nan() || scale <= 0.0 {
            return -1.0;
        }
        worst = worst.min(next.cross(prev).dot(normal) / scale);
    }
    worst
}

/// Unfolds what relaxing left folded or pinched: the vertices of every quad
/// whose worst corner is under [`PINCHED`], one at a time in a fixed order.
/// Each is tried part and all of the way to a few targets (the corner of
/// each neighbouring quad's fitted square, the corner that would make that
/// quad a parallelogram, the average of its neighbours), each put back on
/// the surface or its feature line, and it moves to the best of them only if
/// the worst quad around it improves. So a pass never makes a quad worse
/// than the worst it touches was, and passes stop when one changes nothing.
#[allow(clippy::too_many_arguments)]
fn repair(
    positions: &mut [DVec3],
    holds: &mut [Hold],
    faces: &[&[u32]],
    corners: &[Vec<(u32, u8)>],
    neighbours: &[Vec<u32>],
    along: &[Vec<u32>],
    normals: &mut [DVec3],
    curves: &Curves,
    world: &Surface<'_>,
) {
    let quad = |positions: &[DVec3], f: u32| -> Option<([u32; 4], [DVec3; 4])> {
        let &[a, b, c, d] = *faces.get(f as usize)? else { return None };
        let ids = [a, b, c, d];
        Some((ids, ids.map(|v| positions.get(v as usize).copied().unwrap_or(DVec3::ZERO))))
    };
    let facing = |normals: &[DVec3], ids: [u32; 4]| -> DVec3 {
        ids.iter().map(|&v| normals.get(v as usize).copied().unwrap_or(DVec3::ZERO)).sum()
    };
    // The quads around `v`, with `v` at `at`: their worst corner (capped at
    // SOUND, past which it stops mattering), then their mean worst corner.
    let around = |positions: &[DVec3], normals: &[DVec3], v: usize, at: DVec3| -> (f64, f64) {
        let (mut worst, mut sum, mut count) = (f64::INFINITY, 0.0, 0usize);
        for &(f, k) in &corners[v] {
            let Some((ids, mut p)) = quad(positions, f) else { continue };
            p[k as usize] = at;
            let q = worst_corner(p, facing(normals, ids));
            worst = worst.min(q);
            sum += q;
            count += 1;
        }
        (worst.min(SOUND), if count > 0 { sum / count as f64 } else { 0.0 })
    };
    for _ in 0..REPAIR_PASSES {
        let mut pinched: Vec<u32> = Vec::new();
        for f in 0..faces.len() as u32 {
            if let Some((ids, p)) = quad(positions, f)
                && worst_corner(p, facing(normals, ids)) < PINCHED
            {
                pinched.extend(ids);
            }
        }
        pinched.sort_unstable();
        pinched.dedup();
        let mut improved = false;
        for v in pinched {
            let v = v as usize;
            let hold = holds.get(v).copied().unwrap_or(Hold::Fixed);
            if hold == Hold::Fixed || neighbours[v].is_empty() {
                continue;
            }
            let p = positions[v];
            let mut targets: Vec<DVec3> = Vec::with_capacity(2 * corners[v].len() + 1);
            for &(f, k) in &corners[v] {
                let Some((ids, q)) = quad(positions, f) else { continue };
                let k = k as usize;
                targets.push(q[(k + 1) % 4] + q[(k + 3) % 4] - q[(k + 2) % 4]);
                let lined = [0, 1, 2, 3]
                    .map(|i| along.get(ids[i] as usize).is_some_and(|list| list.contains(&ids[(i + 1) % 4])));
                if let Some(fit) = fit_quad(q, facing(normals, ids), lined) {
                    targets.push(fit.corners[k]);
                }
            }
            targets.push(
                neighbours[v].iter().map(|&u| positions[u as usize]).sum::<DVec3>()
                    / neighbours[v].len() as f64,
            );
            let mut best = (around(positions, normals, v, p), p, normals[v], hold);
            let before = best.0;
            for target in targets {
                for step in REPAIR_STEPS {
                    let goal = p.lerp(target, step);
                    let candidate = match hold {
                        Hold::Curve { chain, at } => {
                            let (Some(line), &[a, b]) = (curves.chain(chain), along[v].as_slice()) else {
                                continue;
                            };
                            let tangent = line.tangent(at);
                            let (back, ahead) =
                                reach_along(p, tangent, positions[a as usize], positions[b as usize]);
                            let at = line.bound(at + (goal - p).dot(tangent).clamp(-back, ahead));
                            (line.point(at), normals[v], Hold::Curve { chain, at })
                        }
                        _ => match world.land(v, goal, normals[v], p) {
                            Some((q, m)) if q.is_finite() => (q, m, hold),
                            _ => continue,
                        },
                    };
                    let score = around(positions, normals, v, candidate.0);
                    if score > best.0 {
                        best = (score, candidate.0, candidate.1, candidate.2);
                    }
                }
            }
            if best.0 > (before.0 + 1e-9, before.1) {
                positions[v] = best.1;
                normals[v] = best.2;
                holds[v] = best.3;
                improved = true;
            }
        }
        if !improved {
            break;
        }
    }
}

/// The surface the quads lie on, and where each vertex may land on it.
struct Surface<'a> {
    bvh: &'a TriangleBvh,
    face_normal: &'a [DVec3],
    regions: &'a Regions,
    trace: &'a TraceOutput,
}

impl Surface<'_> {
    /// The point of the surface nearest `goal` that vertex `v` may land on,
    /// and the surface normal there: on its own patches or their neighbours,
    /// and, when `facing` is not zero, on a triangle that faces that way
    /// (if none does, on any of them).
    ///
    /// `from` is where the vertex is now. It lies on such a triangle already
    /// (it was landed there), so the nearest one to `goal` is no farther
    /// than `from` is, and the search is told so: without a bound it goes
    /// through whole subtrees of other patches' triangles before it meets
    /// one of the vertex's own ([`TriangleBvh::closest_triangle_within`]).
    /// If the bound turns out wrong the search is done again without it, so
    /// the answer never depends on `from`.
    fn land(&self, v: usize, goal: DVec3, facing: DVec3, from: DVec3) -> Option<(DVec3, DVec3)> {
        let own =
            |t: u32| self.trace.patch_of_face.get(t as usize).is_some_and(|&p| self.regions.contains(v, p));
        let normal = |t: u32| self.face_normal.get(t as usize).copied().unwrap_or(DVec3::ZERO);
        let faces = |t: u32| own(t) && normal(t).dot(facing) > FACING;
        // A hair over the distance to `from`, so a vertex that hasn't moved finds its own triangle.
        let reach = (goal - from).length() * (1.0 + 1e-9) + 1e-9 * (1.0 + from.length());
        let found = if facing == DVec3::ZERO {
            None
        } else {
            self.bvh
                .closest_triangle_within(goal, reach, faces)
                .or_else(|| self.bvh.closest_triangle_where(goal, faces))
        };
        let (q, _, t) = found
            .or_else(|| self.bvh.closest_triangle_within(goal, reach, own))
            .or_else(|| self.bvh.closest_triangle_where(goal, own))?;
        Some((q, normal(t)))
    }
}

/// What separates one of the surface's vertices from the quad mesh: the
/// corners of the quad-mesh triangle nearest it, their weights at the
/// nearest point, and the vector from that point to the vertex.
type Gap = ([u32; 3], [f64; 3], DVec3);

/// The surface's own vertices, which the quad mesh should stay near.
struct Cover {
    /// Position, normal and patch of each vertex that takes part.
    points: Vec<(DVec3, DVec3, u32)>,
}

impl Cover {
    /// The layout mesh's vertices off its feature lines (a quad edge runs
    /// along a feature line, and the quads either side of it face apart).
    fn new(trace: &TraceOutput, face_normal: &[DVec3]) -> Self {
        let tm = &trace.mesh;
        let mut normal = vec![DVec3::ZERO; tm.positions.len()];
        let mut patch = vec![u32::MAX; tm.positions.len()];
        for (f, t) in tm.triangles.iter().enumerate() {
            for &v in t {
                if let (Some(sum), Some(&m)) = (normal.get_mut(v as usize), face_normal.get(f)) {
                    *sum += m;
                }
                if let (Some(slot), Some(&q)) = (patch.get_mut(v as usize), trace.patch_of_face.get(f))
                    && *slot == u32::MAX
                {
                    *slot = q;
                }
            }
        }
        let mut on_feature = vec![false; tm.positions.len()];
        for &[a, b] in &tm.feature_edges {
            for v in [a, b] {
                if let Some(flag) = on_feature.get_mut(v as usize) {
                    *flag = true;
                }
            }
        }
        let points = (0..tm.positions.len())
            .filter(|&v| !on_feature[v] && patch[v] != u32::MAX)
            .filter_map(|v| Some((tm.positions[v], normal[v].try_normalize()?, patch[v])))
            .collect();
        Self { points }
    }

    /// Per quad-mesh vertex, the mean of what separates the surface's
    /// vertices from the quad mesh near it: each surface vertex finds the
    /// nearest point of the quads of its own patch and the patches next to
    /// it that face its way, and shares the gap out over the corners of the
    /// triangle that point is in, by how near each corner is.
    fn pull(
        &self,
        positions: &[DVec3],
        faces: &[&[u32]],
        patch_of_face: &[u32],
        regions: &Regions,
    ) -> Vec<DVec3> {
        // Each quad as two triangles.
        let mut triangles: Vec<[u32; 3]> = Vec::with_capacity(faces.len() * 2);
        let mut face_of: Vec<u32> = Vec::with_capacity(faces.len() * 2);
        for (f, face) in faces.iter().enumerate() {
            if let &[a, b, c, d] = *face {
                triangles.push([a, b, c]);
                triangles.push([a, c, d]);
                face_of.extend([f as u32, f as u32]);
            }
        }
        let at = |v: u32| positions.get(v as usize).copied().unwrap_or(DVec3::ZERO);
        let normal: Vec<DVec3> = triangles
            .iter()
            .map(|&[a, b, c]| (at(b) - at(a)).cross(at(c) - at(a)).normalize_or_zero())
            .collect();
        let quads = TriangleBvh::new(positions, &triangles);
        let found: Vec<Option<Gap>> = self
            .points
            .par_iter()
            .map(|&(point, facing, patch)| {
                let (near, _, t) = quads.closest_triangle_where(point, |t| {
                    let near_patch = face_of
                        .get(t as usize)
                        .and_then(|&f| patch_of_face.get(f as usize))
                        .is_some_and(|&q| q == patch || regions.next_to(patch, q));
                    near_patch && normal.get(t as usize).is_some_and(|m| m.dot(facing) > FACING)
                })?;
                let corners = *triangles.get(t as usize)?;
                let weights = barycentric(near, corners.map(at))?;
                Some((corners, weights, point - near))
            })
            .collect();
        let mut gap = vec![DVec3::ZERO; positions.len()];
        let mut weight = vec![0.0f64; positions.len()];
        for (corners, weights, apart) in found.into_iter().flatten() {
            for (v, w) in corners.into_iter().zip(weights) {
                if let (Some(sum), Some(total)) = (gap.get_mut(v as usize), weight.get_mut(v as usize)) {
                    *sum += apart * w;
                    *total += w;
                }
            }
        }
        for (sum, total) in gap.iter_mut().zip(&weight) {
            if *total > 0.0 {
                *sum /= *total;
            }
        }
        gap
    }
}

/// The weights of triangle `t`'s corners at point `p` in its plane, clamped
/// to the triangle. `None` for a triangle with no area.
fn barycentric(p: DVec3, t: [DVec3; 3]) -> Option<[f64; 3]> {
    let (e0, e1, r) = (t[1] - t[0], t[2] - t[0], p - t[0]);
    let (d00, d01, d11, d20, d21) = (e0.dot(e0), e0.dot(e1), e1.dot(e1), r.dot(e0), r.dot(e1));
    let denominator = d00 * d11 - d01 * d01;
    if denominator.is_nan() || denominator <= 0.0 {
        return None;
    }
    let v = ((d11 * d20 - d01 * d21) / denominator).clamp(0.0, 1.0);
    let w = ((d00 * d21 - d01 * d20) / denominator).clamp(0.0, 1.0 - v);
    Some([1.0 - v - w, v, w])
}

/// The shape a quad should take (a square, or a rectangle for a quad of a
/// strip between two feature lines), and how firmly it holds a corner there.
#[derive(Clone, Copy, Debug)]
struct Fit {
    /// The shape's corners, in the quad's own order.
    corners: [DVec3; 4],
    /// Three directions at right angles (unit): two in the quad's plane (a
    /// rectangle's two sides) and the plane's normal.
    axes: [DVec3; 3],
    /// Half the shape's extent along the first two axes.
    half: [f64; 2],
    /// A quad of a strip between two feature lines (its shape a rectangle).
    strip: bool,
}

impl Fit {
    /// How firmly the quad holds a corner along each of its axes, in units
    /// of the hold of the largest quad round the vertex. `largest` is the
    /// largest half-extent of any quad round the vertex.
    ///
    /// Moving a corner a length d along one side of a quad leans the quad by
    /// d over its *other* side's length, and a lean counts by its square.
    ///
    /// So a strip quad holds its corners along the strip by the square of
    /// how many times thinner it is than what surrounds it: a strip a tenth
    /// as high as the quads next to it are large is leant ten times as far
    /// as they are by the same slide. Up to [`SLENDEREST`] times, so a sliver
    /// doesn't outweigh everything.
    ///
    /// A free quad holds alike every way, by the square of how many times
    /// smaller it is than the largest, up to [`SMALLEST`] times only. A free
    /// quad can also shrink, which a strip quad's lines don't let it, and
    /// the smaller it gets the more a full measure would let it have its
    /// way: with ten times, slivers at a tapered patch pulled their
    /// neighbours in and grew longer.
    fn firm(&self, largest: f64) -> [f64; 3] {
        let times = |half: f64, most: f64| if half > 0.0 { (largest / half).clamp(1.0, most) } else { most };
        if !self.strip {
            return [times(self.half[0], SMALLEST).powi(2); 3];
        }
        [times(self.half[1], SLENDEREST).powi(2), times(self.half[0], SLENDEREST).powi(2), 1.0]
    }
}

/// `a aᵀ`.
fn outer(a: DVec3) -> DMat3 {
    DMat3::from_cols(a * a.x, a * a.y, a * a.z)
}

/// The shape quad `p` should take. `lined[k]` says side `k` (corner `k` to
/// the next) lies on a feature line.
///
/// It is the square that best fits the quad: same centre and mean radius,
/// turned to match the corners as closely as a turn can. A square is what
/// keeps a quad's proportions even, and nothing else here does: a row of
/// quads along a border, asked only for right angles, was squeezed flat
/// against the border by the rows behind it.
///
/// Unless two opposite sides are on lines: a quad of a strip between two
/// lines. Then the lines decide how wide it is, its vertices can only slide
/// along them, and what it can still get wrong is to lean. Its shape is a
/// rectangle that takes its turn from the lined sides alone (the main axis
/// of their directions, the 1-2 and 3-0 ones turned a quarter back), and the
/// length and width nearest the quad for that turn: with `m₁` the vector
/// between the midpoints of sides 3-0 and 1-2 and `m₂` between those of
/// sides 0-1 and 2-3, half-lengths `m₁·u / 2` and `m₂·v / 2` along its side
/// directions `u ⊥ v`. Taking the turn from the quad as well would go wrong
/// where it matters: a strip quad that leans by more than its own length is
/// nearest a rectangle laid along the lean, and the one it should become
/// lies along the strip.
///
/// Either shape has the quad's centre and lies in the quad's plane (each
/// corner keeps its own height off that plane, so a quad bent over a curved
/// surface stays bent). It is wound the way `facing` points, whatever the
/// quad's own winding, which is what unfolds a folded quad. `None` for a
/// quad too collapsed to fit.
fn fit_quad(p: [DVec3; 4], facing: DVec3, lined: [bool; 4]) -> Option<Fit> {
    let centre = (p[0] + p[1] + p[2] + p[3]) / 4.0;
    let (d0, d1) = (p[2] - p[0], p[3] - p[1]);
    let own = d0.cross(d1);
    let size = d0.length_squared().max(d1.length_squared());
    if size.is_nan() || size <= 0.0 {
        return None;
    }
    // The quad's own plane, turned to face the surface's way. A bow tie has
    // no plane of its own: use the surface's.
    let normal = if own.length() > 1e-6 * size {
        let own = own.normalize();
        if own.dot(facing) < 0.0 { -own } else { own }
    } else {
        facing.normalize_or_zero()
    };
    if normal == DVec3::ZERO {
        return None;
    }
    let e1 = normal.any_orthonormal_vector();
    let e2 = normal.cross(e1);
    let flat = p.map(|q| DVec2::new((q - centre).dot(e1), (q - centre).dot(e2)));
    let height = p.map(|q| (q - centre).dot(normal));
    if !((lined[0] && lined[2]) || (lined[1] && lined[3])) {
        // The turn that brings the square's corners nearest the quad's.
        let (mut cos, mut sin, mut radius) = (0.0, 0.0, 0.0);
        for k in 0..4 {
            cos += SQUARE[k].dot(flat[k]);
            sin += SQUARE[k].perp_dot(flat[k]);
            radius += flat[k].length();
        }
        if radius.is_nan() || radius <= 0.0 {
            return None;
        }
        let turn = cos.hypot(sin);
        let (cos, sin) = if turn > 1e-9 * radius {
            (cos / turn, sin / turn)
        } else {
            // A quad wound exactly backwards fits every turn equally badly: keep
            // its first corner's direction, so the choice is at least a steady one.
            let first = flat[0].try_normalize()?;
            (first.x, first.y)
        };
        let radius = radius / 4.0;
        let mut corners = [DVec3::ZERO; 4];
        for k in 0..4 {
            let s = SQUARE[k];
            let turned = DVec2::new(cos * s.x - sin * s.y, sin * s.x + cos * s.y) * radius;
            corners[k] = centre + e1 * turned.x + e2 * turned.y + normal * height[k];
        }
        let half = radius * std::f64::consts::FRAC_1_SQRT_2;
        return Some(Fit { corners, axes: [e1, e2, normal], half: [half, half], strip: false });
    }
    let m1 = (flat[1] + flat[2] - flat[0] - flat[3]) / 2.0;
    let m2 = (flat[2] + flat[3] - flat[0] - flat[1]) / 2.0;
    // `m2` a quarter turn back: along `m1` for a rectangle wound with the surface.
    let n = DVec2::new(m2.y, -m2.x);
    if (m1.length_squared() + n.length_squared()).is_nan()
        || m1.length_squared() + n.length_squared() <= 1e-12 * size
    {
        return None;
    }
    // The directions the rectangle's first side should follow: the lined
    // sides' own, all turned to run the way side 0-1 does (the two midlines
    // if those have no length).
    let quarter_back = |d: DVec2| DVec2::new(d.y, -d.x);
    let sides = [
        flat[1] - flat[0],
        quarter_back(flat[2] - flat[1]),
        flat[2] - flat[3],
        quarter_back(flat[3] - flat[0]),
    ];
    let mut guides: tinyvec::ArrayVec<[DVec2; 4]> = (0..4).filter(|&k| lined[k]).map(|k| sides[k]).collect();
    if guides.iter().map(|d| d.length_squared()).sum::<f64>() <= 1e-12 * size {
        guides.clear();
        guides.extend([m1, n]);
    }
    let (mut xx, mut xy, mut yy) = (0.0, 0.0, 0.0);
    for d in &guides {
        xx += d.x * d.x;
        xy += d.x * d.y;
        yy += d.y * d.y;
    }
    let turn = 0.5 * (2.0 * xy).atan2(xx - yy);
    let mut u = DVec2::new(turn.cos(), turn.sin());
    // The axis has two ends: take the one the longest guide points along.
    // For a quad wound against the surface the midlines disagree, and
    // following the longer one unfolds it across its long axis, the shorter way.
    let longest = guides
        .iter()
        .copied()
        .fold(DVec2::ZERO, |best, d| if u.dot(d).abs() > u.dot(best).abs() { d } else { best });
    if u.dot(longest) < 0.0 {
        u = -u;
    }
    let v = DVec2::new(-u.y, u.x);
    let (half_u, half_v) = (u.dot(m1).abs() / 2.0, v.dot(m2).abs() / 2.0);
    let longer = half_u.max(half_v);
    if longer.is_nan() || longer <= 0.0 {
        return None;
    }
    let (signs_u, signs_v) = ([-1.0, 1.0, 1.0, -1.0], [-1.0, -1.0, 1.0, 1.0]);
    let mut corners = [DVec3::ZERO; 4];
    for k in 0..4 {
        let at = u * (signs_u[k] * half_u) + v * (signs_v[k] * half_v);
        corners[k] = centre + e1 * at.x + e2 * at.y + normal * height[k];
    }
    Some(Fit {
        corners,
        axes: [e1 * u.x + e2 * u.y, e1 * v.x + e2 * v.y, normal],
        half: [half_u, half_v],
        strip: true,
    })
}

/// Projection regions: a pool of patch sets, and per vertex an index into it.
/// Each patch's set is itself plus its neighbours; a vertex on several patches
/// gets the union (built once).
struct Regions {
    pool: Vec<HashSet<u32>>,
    of_vertex: Vec<usize>,
}

impl Regions {
    fn contains(&self, v: usize, patch: u32) -> bool {
        self.of_vertex.get(v).and_then(|&i| self.pool.get(i)).is_some_and(|set| set.contains(&patch))
    }

    /// Whether patches `a` and `b` share an edge (or are the same patch).
    fn next_to(&self, a: u32, b: u32) -> bool {
        self.pool.get(a as usize).is_some_and(|set| set.contains(&b))
    }
}

/// The patches each vertex may project onto: its own and their neighbours.
fn projection_regions(patches: &[Vec<u32>], trace: &TraceOutput) -> Regions {
    let tm = &trace.mesh;
    let count = trace.patches.len();
    let mut pool: Vec<HashSet<u32>> = (0..count as u32).map(|p| HashSet::from([p])).collect();
    let mut face_of: hashbrown::HashMap<(u32, u32), u32> = hashbrown::HashMap::new();
    for (f, t) in tm.triangles.iter().enumerate() {
        for i in 0..3 {
            let (a, b) = (t[i], t[(i + 1) % 3]);
            if let Some(&g) = face_of.get(&(b, a)) {
                let (pf, pg) =
                    (trace.patch_of_face.get(f).copied(), trace.patch_of_face.get(g as usize).copied());
                if let (Some(pf), Some(pg)) = (pf, pg)
                    && pf != pg
                    && (pf as usize) < count
                    && (pg as usize) < count
                {
                    pool[pf as usize].insert(pg);
                    pool[pg as usize].insert(pf);
                }
            }
            face_of.insert((a, b), f as u32);
        }
    }
    let mut merged: hashbrown::HashMap<Vec<u32>, usize> = hashbrown::HashMap::new();
    let of_vertex = patches
        .iter()
        .map(|own| match own.as_slice() {
            [p] if (*p as usize) < count => *p as usize,
            _ => {
                let mut key: Vec<u32> = own.iter().copied().filter(|&p| (p as usize) < count).collect();
                key.sort_unstable();
                // entry_ref: the key is copied only when it is new.
                *merged.entry_ref(key.as_slice()).or_insert_with(|| {
                    let union: HashSet<u32> =
                        key.iter().flat_map(|&p| pool[p as usize].iter().copied()).collect();
                    pool.push(union);
                    pool.len() - 1
                })
            }
        })
        .collect();
    Regions { pool, of_vertex }
}

/// Keeps a vertex where it was if its move would shrink one of its edges below
/// [`MIN_EDGE_RATIO`] of the edge's current length (projection can otherwise
/// pull neighbours onto the same point and collapse quads).
fn guard_collapse(current: &[DVec3], mut next: Vec<DVec3>, neighbours: &[Vec<u32>]) -> Vec<DVec3> {
    for v in 0..next.len() {
        if next[v] == current[v] {
            continue;
        }
        let collapses = neighbours[v].iter().any(|&u| {
            let u = u as usize;
            let before = (current[v] - current[u]).length();
            (next[v] - next[u]).length() < MIN_EDGE_RATIO * before
        });
        if collapses {
            next[v] = current[v];
        }
    }
    next
}

/// Shortest an edge may become in one smoothing round, relative to before.
const MIN_EDGE_RATIO: f64 = 0.05;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_square_fits_itself() {
        let p = [
            DVec3::new(0.0, 0.0, 0.0),
            DVec3::new(2.0, 0.0, 0.0),
            DVec3::new(2.0, 2.0, 0.0),
            DVec3::new(0.0, 2.0, 0.0),
        ];
        let fitted = fit_quad(p, DVec3::Z, [false; 4]).unwrap().corners;
        for k in 0..4 {
            assert!((fitted[k] - p[k]).length() < 1e-12, "corner {k}: {:?}", fitted[k]);
        }
    }

    #[test]
    fn a_sheared_quad_fits_a_square_of_its_size_and_centre() {
        let p = [
            DVec3::new(0.0, 0.0, 0.0),
            DVec3::new(2.0, 0.0, 0.0),
            DVec3::new(3.5, 2.0, 0.0),
            DVec3::new(1.5, 2.0, 0.0),
        ];
        let fitted = fit_quad(p, DVec3::Z, [false; 4]).unwrap().corners;
        assert!(worst_corner(p, DVec3::Z) < 0.85);
        assert!(worst_corner(fitted, DVec3::Z) > 1.0 - 1e-12);
        let centre = |q: [DVec3; 4]| (q[0] + q[1] + q[2] + q[3]) / 4.0;
        assert!((centre(fitted) - centre(p)).length() < 1e-12);
        let radius = |q: [DVec3; 4]| q.iter().map(|x| (*x - centre(q)).length()).sum::<f64>() / 4.0;
        assert!((radius(fitted) - radius(p)).abs() < 1e-12);
    }

    #[test]
    fn a_quad_wound_against_the_surface_fits_a_square_wound_with_it() {
        // Clockwise seen from +z, on a surface facing +z.
        let p = [
            DVec3::new(0.0, 0.0, 0.0),
            DVec3::new(0.0, 2.0, 0.0),
            DVec3::new(2.0, 2.0, 0.0),
            DVec3::new(2.0, 0.0, 0.0),
        ];
        assert!(worst_corner(p, DVec3::Z) < 0.0);
        let fitted = fit_quad(p, DVec3::Z, [false; 4]).unwrap().corners;
        assert!(worst_corner(fitted, DVec3::Z) > 1.0 - 1e-12);
    }

    #[test]
    fn a_strip_quad_fits_itself_whatever_its_proportions() {
        for (w, h) in [(2.0, 2.0), (10.0, 0.5), (0.3, 4.0)] {
            let p = [
                DVec3::new(1.0, 1.0, 0.0),
                DVec3::new(1.0 + w, 1.0, 0.0),
                DVec3::new(1.0 + w, 1.0 + h, 0.0),
                DVec3::new(1.0, 1.0 + h, 0.0),
            ];
            for lined in [[true, false, true, false], [false, true, false, true], [true; 4]] {
                let fit = fit_quad(p, DVec3::Z, lined).unwrap();
                for (k, (fitted, corner)) in fit.corners.iter().zip(p).enumerate() {
                    assert!((*fitted - corner).length() < 1e-12, "{w} x {h}, corner {k}: {fitted:?}");
                }
            }
        }
    }

    #[test]
    fn a_strip_quad_holds_its_corners_by_how_thin_it_is_beside_its_neighbours() {
        // 4 x 1, its long sides on lines.
        let p = [
            DVec3::new(0.0, 0.0, 0.0),
            DVec3::new(4.0, 0.0, 0.0),
            DVec3::new(4.0, 1.0, 0.0),
            DVec3::new(0.0, 1.0, 0.0),
        ];
        let fit = fit_quad(p, DVec3::Z, [true, false, true, false]).unwrap();
        assert!((fit.axes[0] - DVec3::X).length() < 1e-9 && (fit.axes[1] - DVec3::Y).length() < 1e-9);
        assert!((fit.axes[2] - DVec3::Z).length() < 1e-9);
        assert!(fit.strip && (fit.half[0] - 2.0).abs() < 1e-9 && (fit.half[1] - 0.5).abs() < 1e-9);
        // Nothing larger round the vertex than its own length of 4 (half 2):
        // four times thinner than that along the strip, no thinner across.
        let near = |got: [f64; 3], want: [f64; 3]| (0..3).all(|i| (got[i] - want[i]).abs() < 1e-9);
        assert!(near(fit.firm(2.0), [16.0, 1.0, 1.0]), "{:?}", fit.firm(2.0));
        // Beside a quad 6 across (half 3): six times thinner, and a quarter
        // shorter, which counts across the strip.
        assert!(near(fit.firm(3.0), [36.0, 2.25, 1.0]), "{:?}", fit.firm(3.0));
        // Never more than ten times.
        assert!(near(fit.firm(50.0), [100.0, 100.0, 1.0]), "{:?}", fit.firm(50.0));
        // A free quad holds alike every way: by the square of how many times
        // smaller it is than the largest quad round the vertex, twice at most.
        let square = [p[0], DVec3::new(2.0, 0.0, 0.0), DVec3::new(2.0, 2.0, 0.0), DVec3::new(0.0, 2.0, 0.0)];
        let free = fit_quad(square, DVec3::Z, [false; 4]).unwrap();
        assert!(!free.strip && (free.half[0] - 1.0).abs() < 1e-9 && (free.half[1] - 1.0).abs() < 1e-9);
        for (largest, hold) in [(1.0, 1.0), (1.5, 2.25), (50.0, 4.0)] {
            assert!(
                free.firm(largest).iter().all(|&f| (f - hold).abs() < 1e-9),
                "{largest}: {:?}",
                free.firm(largest)
            );
        }
    }

    #[test]
    fn a_lined_quad_wound_against_the_surface_fits_a_rectangle_wound_with_it() {
        // Clockwise seen from +z, on a surface facing +z: 2 wide, 6 tall,
        // its two long sides (up and down the y axis) on lines.
        let p = [
            DVec3::new(0.0, 0.0, 0.0),
            DVec3::new(0.0, 6.0, 0.0),
            DVec3::new(2.0, 6.0, 0.0),
            DVec3::new(2.0, 0.0, 0.0),
        ];
        assert!(worst_corner(p, DVec3::Z) < 0.0);
        let fit = fit_quad(p, DVec3::Z, [true, false, true, false]).unwrap();
        assert!(worst_corner(fit.corners, DVec3::Z) > 1.0 - 1e-12);
        assert!(fit.axes[0].dot(DVec3::Y).abs() > 1.0 - 1e-9, "along its lined sides: {:?}", fit.axes);
    }

    #[test]
    fn sides_on_feature_lines_set_the_rectangle_s_turn() {
        // A strip quad 1 long and 0.2 high that leans by 2. With its two long
        // sides on lines its rectangle lies along the strip, as long as the
        // sides and as high as the strip (the rectangle nearest it, taken
        // freely, would lie along the lean).
        let p = [
            DVec3::new(0.0, 0.0, 0.0),
            DVec3::new(1.0, 0.0, 0.0),
            DVec3::new(3.0, 0.2, 0.0),
            DVec3::new(2.0, 0.2, 0.0),
        ];
        let lined = fit_quad(p, DVec3::Z, [true, false, true, false]).unwrap();
        assert!((lined.axes[0] - DVec3::X).length() < 1e-9, "{:?}", lined.axes);
        let (long, high) =
            ((lined.corners[1] - lined.corners[0]).length(), (lined.corners[3] - lined.corners[0]).length());
        assert!((long - 1.0).abs() < 1e-9 && (high - 0.2).abs() < 1e-9, "{long} x {high}");
        // The same strip with its corners named from the next one round.
        let p = [p[1], p[2], p[3], p[0]];
        let lined = fit_quad(p, DVec3::Z, [false, true, false, true]).unwrap();
        assert!((lined.axes[1] + DVec3::X).length() < 1e-9, "{:?}", lined.axes);
        // One side on a line is not a strip: a square, as for a free quad.
        let one = fit_quad(p, DVec3::Z, [false, true, false, false]).unwrap();
        let free = fit_quad(p, DVec3::Z, [false; 4]).unwrap();
        assert_eq!(one.corners, free.corners);
    }

    #[test]
    fn a_vertex_on_a_line_is_limited_by_the_neighbour_it_moves_toward() {
        // Neighbours 0.1 behind and 2 ahead: it may go 0.04 back and 0.8 forward.
        let (back, ahead) =
            reach_along(DVec3::ZERO, DVec3::X, DVec3::new(-0.1, 0.0, 0.0), DVec3::new(2.0, 0.0, 0.0));
        assert!((back - 0.04).abs() < 1e-12 && (ahead - 0.8).abs() < 1e-12, "{back} {ahead}");
        let (back, ahead) =
            reach_along(DVec3::ZERO, DVec3::X, DVec3::new(2.0, 0.0, 0.0), DVec3::new(-0.1, 0.0, 0.0));
        assert!((back - 0.04).abs() < 1e-12 && (ahead - 0.8).abs() < 1e-12, "{back} {ahead}");
        // Both on one side: the nearer gap, either way.
        let (back, ahead) =
            reach_along(DVec3::ZERO, DVec3::X, DVec3::new(0.1, 0.0, 0.0), DVec3::new(2.0, 0.0, 0.0));
        assert!((back - 0.04).abs() < 1e-12 && (ahead - 0.04).abs() < 1e-12, "{back} {ahead}");
    }

    #[test]
    fn a_bent_quad_keeps_its_bend() {
        // Two corners lifted: the fitted rectangle's corners keep those heights.
        let p = [
            DVec3::new(0.0, 0.0, 0.3),
            DVec3::new(2.0, 0.0, -0.3),
            DVec3::new(2.0, 2.0, 0.3),
            DVec3::new(0.0, 2.0, -0.3),
        ];
        let fit = fit_quad(p, DVec3::Z, [false; 4]).unwrap();
        for (fitted, corner) in fit.corners.iter().zip(p) {
            assert!((fitted.z - corner.z).abs() < 1e-12);
        }
    }

    /// A flat sheet `w` by `h` as a layout of one patch.
    fn sheet(w: f64, h: f64) -> TraceOutput {
        use crate::trace::{Patch, PatchKind, TraceMesh, TraceReport};
        let (nx, ny) = (20u32, 8u32);
        let (mut positions, mut triangles) = (Vec::new(), Vec::new());
        for j in 0..=ny {
            for i in 0..=nx {
                positions.push(DVec3::new(
                    w * f64::from(i) / f64::from(nx),
                    h * f64::from(j) / f64::from(ny),
                    0.0,
                ));
            }
        }
        let id = |i: u32, j: u32| j * (nx + 1) + i;
        for j in 0..ny {
            for i in 0..nx {
                triangles.push([id(i, j), id(i + 1, j), id(i + 1, j + 1)]);
                triangles.push([id(i, j), id(i + 1, j + 1), id(i, j + 1)]);
            }
        }
        let faces = triangles.len();
        TraceOutput {
            mesh: TraceMesh {
                positions,
                triangles,
                feature_edges: Vec::new(),
                directions: vec![DVec3::X; faces],
                singularities: Vec::new(),
            },
            patch_of_face: vec![0; faces],
            source_face: (0..faces as u32).collect(),
            patches: vec![Patch {
                corners: Vec::new(),
                sides: Vec::new(),
                singularities: Vec::new(),
                kind: PatchKind::Ok,
                cc_margin: 0.0,
            }],
            paths: Vec::new(),
            parts: Vec::new(),
            report: TraceReport::default(),
        }
    }

    #[test]
    fn a_thin_strip_lines_up_with_the_junctions_on_its_far_side() {
        // Three rows of quads on a flat sheet: a wide row, a strip a tenth as
        // thick, a wide row. The strip's upper line has its vertices where
        // other lines meet it (fixed, unevenly spaced); its lower line starts
        // with even spacing, as a segment sampled on its own does. Each strip
        // quad is then a parallelogram leaning several times its own height.
        // (AntiqueCamera's rails: rows of such quads, a fifth of them poor.)
        let upper = [0.0, 0.5, 1.2, 4.0, 4.4, 8.0, 10.0];
        let columns = upper.len();
        let even = |k: usize| 10.0 * k as f64 / (columns - 1) as f64;
        let rows = [0.0, 1.4, 1.6, 3.0];
        let mut positions = Vec::new();
        for (j, &y) in rows.iter().enumerate() {
            for (k, &x) in upper.iter().enumerate() {
                positions.push(DVec3::new(if j >= 2 { x } else { even(k) }, y, 0.0));
            }
        }
        let id = |k: usize, j: usize| (j * columns + k) as u32;
        let (mut face_indices, mut face_offsets) = (Vec::new(), vec![0u32]);
        for j in 0..3 {
            for k in 0..columns - 1 {
                face_indices.extend([id(k, j), id(k + 1, j), id(k + 1, j + 1), id(k, j + 1)]);
                face_offsets.push(face_indices.len() as u32);
            }
        }
        let mut mesh = PolyMesh { positions, face_offsets, face_indices };
        // Every row is a feature line; the two lower ones may slide along theirs.
        let feature_edges: Vec<[u32; 2]> =
            (0..4).flat_map(|j| (0..columns - 1).map(move |k| [id(k, j), id(k + 1, j)])).collect();
        let curves = Curves::new(&mesh.positions, feature_edges.iter().map(|&[a, b]| (a, b)));
        let mut holds: Vec<Hold> = (0..mesh.positions.len() as u32)
            .map(|v| match curves.at_vertex(v) {
                Some((chain, at)) if (v as usize) < 2 * columns => Hold::Curve { chain, at },
                _ => Hold::Fixed,
            })
            .collect();
        let strip = |mesh: &PolyMesh| -> f64 {
            (0..columns - 1)
                .map(|k| {
                    let quad =
                        [id(k, 1), id(k + 1, 1), id(k + 1, 2), id(k, 2)].map(|v| mesh.positions[v as usize]);
                    worst_corner(quad, DVec3::Z)
                })
                .fold(f64::INFINITY, f64::min)
        };
        assert!(strip(&mesh) < 0.1, "to begin with: {}", strip(&mesh));
        let trace = sheet(10.0, 3.0);
        let patch_of_face = vec![0u32; mesh.face_count()];
        // Twenty rounds: the default.
        smooth(
            &mut mesh,
            &mut holds,
            &feature_edges,
            &curves,
            &patch_of_face,
            &trace,
            20,
            &Control::default(),
        );
        assert!(strip(&mesh) > 0.9, "the strip's worst corner: {}", strip(&mesh));
        // The wide row under it pays little: its quads lean with the strip's.
        let under = (0..columns - 1)
            .map(|k| {
                let quad =
                    [id(k, 0), id(k + 1, 0), id(k + 1, 1), id(k, 1)].map(|v| mesh.positions[v as usize]);
                worst_corner(quad, DVec3::Z)
            })
            .fold(f64::INFINITY, f64::min);
        assert!(under > 0.9, "the row under it: {under}");
    }

    #[test]
    fn barycentric_weights_rebuild_the_point_and_clamp_outside() {
        let t = [DVec3::ZERO, DVec3::new(2.0, 0.0, 0.0), DVec3::new(0.0, 2.0, 0.0)];
        let p = DVec3::new(0.5, 1.0, 0.0);
        let w = barycentric(p, t).unwrap();
        assert!((t[0] * w[0] + t[1] * w[1] + t[2] * w[2] - p).length() < 1e-12);
        let outside = barycentric(DVec3::new(5.0, -1.0, 0.0), t).unwrap();
        assert!(outside.iter().all(|w| (0.0..=1.0).contains(w)));
        assert!((outside.iter().sum::<f64>() - 1.0).abs() < 1e-12);
        assert!(barycentric(p, [DVec3::ZERO, DVec3::X, DVec3::X * 2.0]).is_none());
    }

    #[test]
    fn worst_corner_reads_squares_reflex_corners_and_winding() {
        let square = [
            DVec3::new(0.0, 0.0, 0.0),
            DVec3::new(1.0, 0.0, 0.0),
            DVec3::new(1.0, 1.0, 0.0),
            DVec3::new(0.0, 1.0, 0.0),
        ];
        assert!((worst_corner(square, DVec3::Z) - 1.0).abs() < 1e-12);
        // The same square seen from below is wound the wrong way.
        assert!((worst_corner(square, -DVec3::Z) + 1.0).abs() < 1e-12);
        // An arrowhead: one corner pushed past the opposite diagonal.
        let arrow = [
            DVec3::new(0.0, 0.0, 0.0),
            DVec3::new(1.0, 0.0, 0.0),
            DVec3::new(0.2, 0.2, 0.0),
            DVec3::new(0.0, 1.0, 0.0),
        ];
        assert!(worst_corner(arrow, DVec3::Z) < 0.0);
        assert_eq!(worst_corner([DVec3::ONE; 4], DVec3::Z), -1.0);
    }

    #[test]
    fn a_collapsed_quad_has_no_rectangle() {
        assert!(fit_quad([DVec3::ONE; 4], DVec3::Z, [false; 4]).is_none());
    }
}
