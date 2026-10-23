//! Quantization: an integer quad count per segment.
//!
//! The counts should match each segment's target (its length over the quad
//! size). Every patch also needs an even boundary count, or no quad mesh fills
//! it. And a pattern of good quality must exist: opposite sides equal for a
//! 4-sided patch (a regular grid), the closed-form split for 3, 5 and 6 sides.
//!
//! Steps:
//! 1. A continuous least-squares fit, with the 4-sided regularity as a strong
//!    penalty (one sparse solve).
//! 2. Rounding.
//! 3. Integer moves along quad strips (chords), kept while they lower the
//!    combined deviation-plus-pattern cost.
//! 4. A parity fix that always succeeds where a path exists: ±1 along a
//!    shortest path between two odd patches, or to a border segment.

use std::collections::VecDeque;

use faer::linalg::solvers::Solve;
use faer::sparse::{SparseColMat, Triplet};
use faer::{Mat, Side};
use tinyvec::TinyVec;

use super::pattern::center_deficit;
use super::segments::Segments;
use crate::control::{Control, Stage};
use crate::error::Error;

/// Weight of a unit of 4-sided imbalance, or of an infeasible closed form,
/// against a unit of deviation from the target.
pub const DEFAULT_REGULARITY: f64 = 50.0;
/// No segment gets fewer quads than this share of its target (at least 1).
/// Without a floor, a patch that can't take a valid pattern at sensible
/// counts (a sliver, a non-convex patch) is "fixed" by shrinking a whole
/// strip of segments around it, starving large patches of quads; with it,
/// that patch gets a fallback pattern and its neighbours keep their size.
const FLOOR_SHARE: f64 = 0.5;

/// The fewest quads a segment with target `t` may get.
pub(crate) fn count_floor(t: f64) -> u32 {
    if t.is_finite() { (t * FLOOR_SHARE).floor().max(1.0).min(u32::MAX as f64) as u32 } else { 1 }
}

/// Passes of integer moves.
const MAX_PASSES: usize = 12;
/// Longest chord walked through 4-sided patches.
const MAX_CHORD: usize = 4096;

/// Integer counts for every segment. `targets[s]` is the ideal count of
/// segment s; polyline segments are fixed at their edge count.
pub fn quantize(
    segments: &Segments,
    targets: &[f64],
    regularity: f64,
    effort: f64,
    control: &Control,
) -> Result<Vec<u32>, Error> {
    let stage = Stage::Quantize;
    let q = Problem::new(segments, targets, regularity);
    control.checkpoint(stage)?;
    let mut n = q.relaxed()?;
    control.checkpoint(stage)?;
    control.set_progress(stage, 0.1);
    q.improve(&mut n, control)?;
    q.fix_parity_by_chords(&mut n, control)?;
    q.improve(&mut n, control)?;
    q.fix_parity_by_chords(&mut n, control)?;
    q.fix_parity(&mut n);
    control.checkpoint(stage)?;
    control.set_progress(stage, 0.2);
    // The integer program, seeded with these counts, keeps only real improvements.
    let spec = super::ilp::Spec { segments, targets, fixed: &q.fixed, regularity, effort };
    if let Some(candidate) = super::ilp::refine(spec, &n)
        && q.all_even(&candidate)
        && q.total(&candidate) < q.total(&n) - 1e-9
    {
        n = candidate;
    }
    // Then region by region where problems remain.
    control.checkpoint(stage)?;
    control.set_progress(stage, 0.6);
    let bad = |n: &[u32], p: usize| q.patch_cost(n, p) > 0.0;
    let mut accept = |old: &[u32], new: &[u32]| q.all_even(new) && q.total(new) < q.total(old) - 1e-9;
    super::ilp::neighbourhoods(spec, &mut n, &bad, &mut accept, control)?;
    Ok(n)
}

struct Problem<'a> {
    segments: &'a Segments,
    targets: &'a [f64],
    regularity: f64,
    fixed: Vec<Option<u32>>,
    weight: Vec<f64>,
    /// [`count_floor`] of each segment's target.
    floor: Vec<u32>,
}

impl<'a> Problem<'a> {
    fn new(segments: &'a Segments, targets: &'a [f64], regularity: f64) -> Self {
        let fixed = segments
            .segments
            .iter()
            .map(|s| s.polyline.then_some(2 * s.vertices.len().saturating_sub(1).max(1) as u32))
            .collect();
        // Relative deviation: long segments tolerate an extra quad more easily.
        let weight = targets.iter().map(|&t| 1.0 / t.max(1.0)).collect();
        let floor = targets.iter().map(|&t| count_floor(t)).collect();
        Self { segments, targets, regularity, fixed, weight, floor }
    }

    /// Total cost: deviation plus weighted pattern cost.
    fn total(&self, n: &[u32]) -> f64 {
        let dev: f64 = (0..n.len()).map(|s| self.deviation(n, s)).sum();
        let pattern: f64 = (0..self.segments.sides.len()).map(|p| self.patch_cost(n, p)).sum();
        dev + self.regularity * pattern
    }

    fn all_even(&self, n: &[u32]) -> bool {
        (0..self.segments.sides.len())
            .all(|p| self.side_counts(n, p).is_none_or(|l| l.iter().sum::<u32>() % 2 == 0))
    }

    fn side_counts(&self, n: &[u32], p: usize) -> Option<TinyVec<[u32; 8]>> {
        let sides = self.segments.sides.get(p)?.as_ref()?;
        Some(sides.iter().map(|side| side.iter().map(|&(s, _)| n[s as usize]).sum()).collect())
    }

    /// The pattern cost of one patch at counts `n`: imbalance of a 4-sided
    /// patch, the closed form's deficit otherwise, 2 for an odd total.
    fn patch_cost(&self, n: &[u32], p: usize) -> f64 {
        let Some(l) = self.side_counts(n, p) else { return 0.0 };
        let total: u32 = l.iter().sum();
        if total % 2 == 1 {
            return 2.0;
        }
        if l.len() == 4 {
            let d02 = (l[0] as f64 - l[2] as f64).abs();
            let d13 = (l[1] as f64 - l[3] as f64).abs();
            return d02 + d13;
        }
        // Graded, so moves toward a valid closed form are rewarded step by step.
        center_deficit(&l).map_or(2.0, |d| d as f64)
    }

    fn deviation(&self, n: &[u32], s: usize) -> f64 {
        let d = n[s] as f64 - self.targets[s];
        self.weight[s] * d * d
    }

    /// Step 1–2: the penalized least-squares fit, rounded to counts ≥ 1.
    fn relaxed(&self) -> Result<Vec<u32>, Error> {
        let m = self.segments.segments.len();
        let mut n: Vec<u32> = (0..m).map(|s| self.fixed[s].unwrap_or(1)).collect();
        // Free segments get unknowns.
        let index: Vec<Option<usize>> = {
            let mut next = 0;
            self.fixed
                .iter()
                .map(|f| {
                    f.is_none().then(|| {
                        next += 1;
                        next - 1
                    })
                })
                .collect()
        };
        let unknowns = index.iter().flatten().count();
        if unknowns == 0 {
            return Ok(n);
        }
        let mut triplets = Vec::new();
        let mut rhs = vec![0.0; unknowns];
        for (s, slot) in index.iter().enumerate() {
            if let &Some(i) = slot {
                triplets.push(Triplet::new(i, i, self.weight[s]));
                rhs[i] += self.weight[s] * self.targets[s];
            }
        }
        // (Σ side_a − Σ side_c)² for both opposite pairs of every 4-sided patch.
        for sides in self.segments.sides.iter().flatten() {
            if sides.len() != 4 {
                continue;
            }
            for (a, c) in [(0, 2), (1, 3)] {
                let mut coef: Vec<(usize, f64)> = Vec::new();
                let mut constant = 0.0;
                for (side, sign) in [(a, 1.0), (c, -1.0)] {
                    for &(s, _) in &sides[side] {
                        match (index[s as usize], self.fixed[s as usize]) {
                            (Some(i), _) => coef.push((i, sign)),
                            (None, Some(f)) => constant += sign * f as f64,
                            (None, None) => {}
                        }
                    }
                }
                for &(i, ci) in &coef {
                    for &(j, cj) in &coef {
                        triplets.push(Triplet::new(i, j, self.regularity * ci * cj));
                    }
                    rhs[i] -= self.regularity * ci * constant;
                }
            }
        }
        let stage = Stage::Quantize;
        let matrix = SparseColMat::<usize, f64>::try_new_from_triplets(unknowns, unknowns, &triplets)
            .map_err(|e| Error::internal(stage, format!("quantization matrix: {e:?}")))?;
        let x = match matrix.sp_cholesky(Side::Lower) {
            Ok(llt) => {
                let mut b = Mat::<f64>::from_fn(unknowns, 1, |i, _| rhs[i]);
                llt.solve_in_place(b.as_mut());
                (0..unknowns).map(|i| b[(i, 0)]).collect::<Vec<f64>>()
            }
            // Not expected (the matrix is SPD); rounding the targets still works.
            Err(_) => vec![f64::NAN; unknowns],
        };
        for s in 0..m {
            if let Some(i) = index[s] {
                let v = if x[i].is_finite() && x[i] > 0.0 { x[i] } else { self.targets[s] };
                n[s] = (v.round().max(1.0) as u32).max(self.floor[s]);
            }
        }
        Ok(n)
    }

    /// Patches touched by segment `s`.
    fn patches_of(&self, s: usize) -> impl Iterator<Item = usize> + '_ {
        self.segments.segments[s].uses.iter().map(|u| u.patch as usize)
    }

    /// The chord through `s`: segments reached by stepping across 4-sided
    /// patches to the opposite side, in both directions, until a patch of
    /// another kind, a border, a fixed segment, or the start again.
    fn chord(&self, n: &[u32], start: usize) -> Vec<usize> {
        self.chord_with_ends(n, start).0
    }

    /// The chord through `start` and how each of its two ends terminates.
    fn chord_with_ends(&self, n: &[u32], start: usize) -> (Vec<usize>, [End; 2]) {
        let mut chord = vec![start];
        let uses = &self.segments.segments[start].uses;
        let mut ends = [End::Border, End::Border];
        for (use_index, end) in ends.iter_mut().enumerate().take(uses.len().min(2)) {
            let mut s = start;
            let mut u = uses[use_index];
            *end = End::Blocked;
            for _ in 0..MAX_CHORD {
                let Some(Some(sides)) = self.segments.sides.get(u.patch as usize) else { break };
                if sides.len() != 4 {
                    *end = End::Patch(u.patch as usize);
                    break;
                }
                let Some(next) = opposite(sides, u.side as usize, s as u32, n) else { break };
                let next = next as usize;
                if next == start || chord.contains(&next) {
                    *end = End::Closed;
                    break;
                }
                if self.fixed[next].is_some() {
                    break;
                }
                chord.push(next);
                // Continue into the patch on the other side of `next`.
                let opposite_side = (u.side as usize + 2) % 4;
                let other = self.segments.segments[next]
                    .uses
                    .iter()
                    .find(|w| w.patch != u.patch || w.side as usize != opposite_side);
                let Some(&other) = other else {
                    *end = End::Border;
                    break;
                };
                s = next;
                u = other;
            }
        }
        (chord, ends)
    }

    fn cost_of(&self, n: &[u32], segs: &[usize]) -> f64 {
        let mut patches: Vec<usize> = segs.iter().flat_map(|&s| self.patches_of(s)).collect();
        patches.sort_unstable();
        patches.dedup();
        segs.iter().map(|&s| self.deviation(n, s)).sum::<f64>()
            + self.regularity * patches.iter().map(|&p| self.patch_cost(n, p)).sum::<f64>()
    }

    /// Step 3: ±1 on single segments and on whole chords while that helps.
    fn improve(&self, n: &mut [u32], control: &Control) -> Result<(), Error> {
        let m = n.len();
        for _ in 0..MAX_PASSES {
            control.checkpoint(Stage::Quantize)?;
            let mut changed = false;
            for s in 0..m {
                if self.fixed[s].is_some() {
                    continue;
                }
                let chord = self.chord(n, s);
                for moves in [vec![s], chord] {
                    for delta in [1i64, -1] {
                        if moves.iter().any(|&t| n[t] as i64 + delta < self.floor[t] as i64) {
                            continue;
                        }
                        let before = self.cost_of(n, &moves);
                        for &t in &moves {
                            n[t] = (n[t] as i64 + delta) as u32;
                        }
                        if self.cost_of(n, &moves) < before - 1e-9 {
                            changed = true;
                        } else {
                            for &t in &moves {
                                n[t] = (n[t] as i64 - delta) as u32;
                            }
                        }
                    }
                }
            }
            if !changed {
                break;
            }
        }
        Ok(())
    }

    /// Step 4a: parity along chords. Changing a whole chord by ±1 keeps every
    /// 4-sided patch it crosses balanced and flips the parity of its two end
    /// patches only, so odd patches are paired through chains of chords
    /// (a T-join on the chord graph, by Dijkstra from each odd patch to the
    /// nearest other odd patch or a border).
    fn fix_parity_by_chords(&self, n: &mut [u32], control: &Control) -> Result<(), Error> {
        let patches = self.segments.sides.len();
        let sink = patches;
        // Open chords between patches (or the border sink), each listed once.
        let mut seen = vec![false; n.len()];
        let mut chords: Vec<(Vec<usize>, usize, usize)> = Vec::new();
        for s in 0..n.len() {
            if seen[s] || self.fixed[s].is_some() {
                continue;
            }
            let (segs, ends) = self.chord_with_ends(n, s);
            for &t in &segs {
                seen[t] = true;
            }
            let node = |e: End| match e {
                End::Patch(p) => Some(p),
                End::Border => Some(sink),
                End::Closed | End::Blocked => None,
            };
            if let (Some(a), Some(b)) = (node(ends[0]), node(ends[1]))
                && a != b
            {
                chords.push((segs, a, b));
            }
        }
        let mut adjacency: Vec<Vec<usize>> = vec![Vec::new(); patches + 1];
        for (c, &(_, a, b)) in chords.iter().enumerate() {
            adjacency[a].push(c);
            adjacency[b].push(c);
        }
        let odd = |n: &[u32], p: usize| {
            self.side_counts(n, p).is_some_and(|l| l.len() != 4 && l.iter().sum::<u32>() % 2 == 1)
        };
        for start in 0..patches {
            if start % 256 == 0 {
                control.checkpoint(Stage::Quantize)?;
            }
            if !odd(n, start) {
                continue;
            }
            // Dijkstra over patches; edge cost = deviation change of moving the chord.
            let mut dist = vec![f64::INFINITY; patches + 1];
            let mut via: Vec<Option<(usize, usize, i64)>> = vec![None; patches + 1];
            let mut heap = std::collections::BinaryHeap::new();
            dist[start] = 0.0;
            heap.push(Entry(0.0, start));
            let mut goal = None;
            while let Some(Entry(d, p)) = heap.pop() {
                if d > dist[p] {
                    continue;
                }
                if p != start && (p == sink || odd(n, p)) {
                    goal = Some(p);
                    break;
                }
                if p == sink {
                    continue;
                }
                for &c in &adjacency[p] {
                    let (segs, a, b) = &chords[c];
                    let q = if *a == p { *b } else { *a };
                    let Some((delta, cost)) = self.chord_move(n, segs) else { continue };
                    if d + cost < dist[q] {
                        dist[q] = d + cost;
                        via[q] = Some((c, p, delta));
                        heap.push(Entry(d + cost, q));
                    }
                }
            }
            let Some(mut p) = goal else { continue };
            while p != start {
                let Some((c, prev, delta)) = via[p] else { break };
                for &t in &chords[c].0 {
                    n[t] = (n[t] as i64 + delta).max(self.floor[t] as i64) as u32;
                }
                p = prev;
            }
        }
        Ok(())
    }

    /// The cheaper of ±1 on every segment of a chord: `(delta, deviation increase)`.
    fn chord_move(&self, n: &[u32], segs: &[usize]) -> Option<(i64, f64)> {
        [1i64, -1]
            .into_iter()
            .filter(|&d| segs.iter().all(|&t| n[t] as i64 + d >= self.floor[t] as i64))
            .map(|d| {
                let cost: f64 = segs
                    .iter()
                    .map(|&t| {
                        let after = n[t] as f64 + d as f64 - self.targets[t];
                        self.weight[t] * after * after - self.deviation(n, t)
                    })
                    .sum();
                (d, cost.max(0.0) + 1e-6)
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
    }

    /// Step 4: every odd patch is paired with the nearest other odd patch (or
    /// a border segment) and the segments of the path between get ±1. Patches
    /// along the path change by 0 or ±2, so only the two ends flip parity.
    fn fix_parity(&self, n: &mut [u32]) {
        let patches = self.segments.sides.len();
        let odd =
            |n: &[u32], p: usize| self.side_counts(n, p).is_some_and(|l| l.iter().sum::<u32>() % 2 == 1);
        for start in 0..patches {
            if !odd(n, start) {
                continue;
            }
            // BFS over patches through free segments; `via[p]` = (segment, previous patch).
            let mut via: Vec<Option<(usize, usize)>> = vec![None; patches];
            let mut seen = vec![false; patches];
            seen[start] = true;
            let mut queue = VecDeque::from([start]);
            let mut end: Option<(usize, Option<usize>)> = None; // (patch, border segment)
            'search: while let Some(p) = queue.pop_front() {
                let Some(Some(sides)) = self.segments.sides.get(p) else { continue };
                for &(s, _) in sides.iter().flatten() {
                    let s = s as usize;
                    if self.fixed[s].is_some() {
                        continue;
                    }
                    let uses = &self.segments.segments[s].uses;
                    if uses.len() == 1 {
                        end = Some((p, Some(s)));
                        break 'search;
                    }
                    for u in uses {
                        let q = u.patch as usize;
                        if q != p && !seen[q] {
                            seen[q] = true;
                            via[q] = Some((s, p));
                            if odd(n, q) {
                                end = Some((q, None));
                                break 'search;
                            }
                            queue.push_back(q);
                        }
                    }
                }
            }
            let Some((mut p, border)) = end else { continue };
            let mut path: Vec<usize> = border.into_iter().collect();
            while p != start {
                let Some((s, prev)) = via[p] else { break };
                path.push(s);
                p = prev;
            }
            for s in path {
                // Toward the target where possible, never below the floor.
                if n[s] > self.floor[s] && (n[s] as f64) > self.targets[s] {
                    n[s] -= 1;
                } else {
                    n[s] += 1;
                }
            }
        }
    }
}

/// How one end of a chord terminates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum End {
    /// In a patch that is not 4-sided.
    Patch(usize),
    /// On the mesh border.
    Border,
    /// Back at its start (a closed strip).
    Closed,
    /// At a fixed segment or a walk limit.
    Blocked,
}

/// A min-heap entry for Dijkstra (`f64` distance, node).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Entry(f64, usize);

impl Eq for Entry {}

impl PartialOrd for Entry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Entry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other.0.total_cmp(&self.0).then_with(|| other.1.cmp(&self.1))
    }
}

/// The segment on the side opposite `side` that lies across from segment `s`,
/// by relative position along the sides (opposite sides run antiparallel).
fn opposite(sides: &[Vec<(u32, bool)>], side: usize, s: u32, n: &[u32]) -> Option<u32> {
    let here = &sides[side];
    let there = &sides[(side + 2) % 4];
    let total: u32 = here.iter().map(|&(t, _)| n[t as usize]).sum();
    let mut before = 0;
    let mut mid = None;
    for &(t, _) in here {
        let c = n[t as usize];
        if t == s {
            mid = Some((before as f64 + 0.5 * c as f64) / total.max(1) as f64);
            break;
        }
        before += c;
    }
    let target = 1.0 - mid?;
    let total_there: u32 = there.iter().map(|&(t, _)| n[t as usize]).sum();
    let mut acc = 0.0;
    for &(t, _) in there {
        acc += n[t as usize] as f64 / total_there.max(1) as f64;
        if acc >= target - 1e-12 {
            return Some(t);
        }
    }
    there.last().map(|&(t, _)| t)
}
