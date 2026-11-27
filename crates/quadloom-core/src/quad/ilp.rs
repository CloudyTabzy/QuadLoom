//! Quantization as small integer programs, solved with `microlp` and seeded
//! with the heuristic counts (so they can only improve on them).
//!
//! Variables and terms:
//! - `n_s ≥ 1` is an integer per free segment. Its deviation
//!   `(n_s − t_s)² / max(t_s, 1)` is a convex piecewise-linear cost, exact at
//!   the integers round the target: `n_s = lo + Σ z_k`, one bounded `z_k` per
//!   unit step, each with that step's slope ([`deviation_pieces`]).
//! - A 4-sided patch has a soft balance per opposite pair, `e⁺ − e⁻ = Σa − Σc`,
//!   weighted by `regularity`, and an even total (integer half-sum).
//! - A patch with 3, 5 or 6 sides has the closed form exactly: integer
//!   `x_start` per step-2 cycle with `2·x_start` = the cycle's alternating sum
//!   (this also makes the total even). The rest follow as `x_{j+2} = l_j − x_j`,
//!   and every split is `≥ 1 − σ_p`, with the slack `σ_p` weighted by
//!   `4·regularity`.
//!
//! **One program per part.** Two patches only bear on each other through a
//! free segment they share, so the counts of a layout fall into independent
//! blocks: its connected parts ([`parts`]). Each is solved on its own, side
//! by side ([`refine`]). The optimum of the whole is the parts' optima put
//! together, and a search of bounded length finds far more of it in forty
//! small programs than in one large one, which a model of many parts used
//! to be too large to get at all. (MIP solvers do the same before they
//! search: SCIP's components presolver, Gamrath et al. 2015.)
//!
//! Every search runs with a deterministic node limit. If it fails or finds
//! nothing better, the counts it started from stand.

use microlp::{ComparisonOp, OptimizationDirection, Problem, SolveOptions, Variable};
use tinyvec::TinyVec;

use rayon::prelude::*;

use super::segments::Segments;
use crate::control::{Control, Stage};
use crate::error::Error;
use crate::unionfind::UnionFind;

/// The quantization problem shared by every solve.
#[derive(Clone, Copy, Debug)]
pub struct Spec<'a> {
    pub segments: &'a Segments,
    /// Ideal count per segment.
    pub targets: &'a [f64],
    /// Segments whose count is fixed (polylines).
    pub fixed: &'a [Option<u32>],
    pub regularity: f64,
    /// Multiple of the node limits ([`NODE_LIMIT`], [`LOCAL_NODE_LIMIT`]).
    pub effort: f64,
}

/// What one program decides: some free segments (ascending), and the patches
/// they are sides of (ascending). Every other segment keeps its count.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Scope {
    pub free: Vec<usize>,
    pub patches: Vec<usize>,
}

impl Scope {
    /// The scope that decides `free` (in any order, repeats allowed).
    fn of(segments: &Segments, mut free: Vec<usize>) -> Self {
        free.sort_unstable();
        free.dedup();
        let mut patches: Vec<usize> =
            free.iter().flat_map(|&s| segments.segments[s].uses.iter().map(|u| u.patch as usize)).collect();
        patches.sort_unstable();
        patches.dedup();
        Self { free, patches }
    }
}

/// Whether to take the counts `new` for `old`; they differ in the segments
/// listed (a [`Scope::free`]) and nowhere else.
pub type Accept<'a> = dyn FnMut(&[u32], &[u32], &[usize]) -> bool + 'a;

/// A side's count: variable terms, a constant, and its value in the warm start.
type SideTerms = (Vec<(Variable, f64)>, f64, f64);

/// Branch-and-bound nodes per part.
pub const NODE_LIMIT: u64 = 2000;

/// Largest part (free segments) solved as one integer program; a larger one
/// is left to [`neighbourhoods`].
pub const MAX_PART_SEGMENTS: usize = 2500;

/// The connected parts of the count problem: free segments that are sides
/// of one patch belong together, and so on from patch to patch. A fixed
/// segment joins nothing: its count is a constant to both its patches.
/// In order of each part's first segment.
pub fn parts(segments: &Segments, fixed: &[Option<u32>]) -> Vec<Scope> {
    let m = segments.segments.len();
    let is_free = |s: u32| fixed.get(s as usize).is_some_and(Option::is_none);
    let mut sets = UnionFind::new(m);
    for sides in segments.sides.iter().flatten() {
        let mut first = None;
        for &(s, _) in sides.iter().flatten().filter(|&&(s, _)| is_free(s)) {
            match first {
                None => first = Some(s),
                Some(f) => sets.union(f, s),
            }
        }
    }
    // Roots are each set's lowest segment, so a part's slot is found before
    // any of its other segments is.
    let mut slot = vec![usize::MAX; m];
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for s in (0..m as u32).filter(|&s| is_free(s)) {
        let root = sets.find(s) as usize;
        if slot[root] == usize::MAX {
            slot[root] = groups.len();
            groups.push(Vec::new());
        }
        groups[slot[root]].push(s as usize);
    }
    groups.into_iter().map(|free| Scope::of(segments, free)).collect()
}

/// Counts improved part by part: the whole integer program of each
/// connected part of up to [`MAX_PART_SEGMENTS`] free segments, seeded with
/// the counts in `n` (so never worse by its own measure), the parts solved
/// side by side and each applied if `accept` keeps it.
pub fn refine(spec: Spec<'_>, n: &mut [u32], accept: &mut Accept<'_>) {
    let parts = parts(spec.segments, spec.fixed);
    let current: &[u32] = n;
    let solved: Vec<Option<Vec<u32>>> = parts
        .par_iter()
        .map(|part| {
            (part.free.len() <= MAX_PART_SEGMENTS)
                .then(|| solve(spec, current, part, scaled(NODE_LIMIT, spec.effort)))
                .flatten()
        })
        .collect();
    apply(n, &parts, solved, accept);
}

/// Writes each scope's solved counts into `n` if `accept` takes them.
/// Returns whether any was taken. The scopes must not share a patch: then
/// each is judged against counts the others have not touched where it looks.
fn apply(n: &mut [u32], scopes: &[Scope], solved: Vec<Option<Vec<u32>>>, accept: &mut Accept<'_>) -> bool {
    let mut next = n.to_vec();
    let mut any = false;
    for (scope, counts) in scopes.iter().zip(solved) {
        let Some(counts) = counts else { continue };
        for (&s, &count) in scope.free.iter().zip(&counts) {
            next[s] = count;
        }
        let taken = accept(n, &next, &scope.free);
        for &s in &scope.free {
            if taken {
                n[s] = next[s];
            } else {
                next[s] = n[s];
            }
        }
        any |= taken;
    }
    any
}

/// Counts improved region by region: around each patch that `bad` flags, the
/// segments of its ring of patches are freed and the integer program solved
/// exactly with everything else fixed. Each round picks regions whose
/// affected patches (every patch touching a freed segment) don't overlap,
/// solves them in parallel, and applies the ones `accept` keeps. They compose
/// exactly because no patch sees two regions. Rounds repeat while some region
/// improves.
pub fn neighbourhoods(
    spec: Spec<'_>,
    n: &mut [u32],
    bad: &(dyn Fn(&[u32], usize) -> bool + Sync),
    accept: &mut Accept<'_>,
    control: &Control,
) -> Result<(), Error> {
    let (segments, fixed) = (spec.segments, spec.fixed);
    let patches = segments.sides.len();
    let mut neighbours: Vec<Vec<usize>> = vec![Vec::new(); patches];
    for seg in &segments.segments {
        for a in &seg.uses {
            for b in &seg.uses {
                if a.patch != b.patch && !neighbours[a.patch as usize].contains(&(b.patch as usize)) {
                    neighbours[a.patch as usize].push(b.patch as usize);
                }
            }
        }
    }
    let segments_of = |q: usize| -> Vec<usize> {
        segments.sides.get(q).and_then(|s| s.as_ref()).map_or_else(Vec::new, |sides| {
            sides.iter().flatten().map(|&(s, _)| s as usize).filter(|&s| fixed[s].is_none()).collect()
        })
    };
    for round in 0..MAX_ROUNDS {
        control.checkpoint(Stage::Quantize)?;
        // The last 40% of the quantize stage's progress (rounds usually stop early).
        control.set_progress(Stage::Quantize, 0.6 + 0.4 * round as f32 / MAX_ROUNDS as f32);
        // Pick disjoint regions around problem patches.
        let mut taken = vec![false; patches];
        let mut regions: Vec<Scope> = Vec::new();
        for p in 0..patches {
            if taken[p] || !bad(n, p) {
                continue;
            }
            let mut region = vec![p];
            let mut in_region = vec![false; patches];
            in_region[p] = true;
            let mut frontier = vec![p];
            for _ in 0..REGION_RINGS {
                let mut next = Vec::new();
                for &q in &frontier {
                    for &r in &neighbours[q] {
                        if !in_region[r] && region.len() < MAX_REGION {
                            in_region[r] = true;
                            region.push(r);
                            next.push(r);
                        }
                    }
                }
                frontier = next;
            }
            let scope = Scope::of(segments, region.iter().flat_map(|&q| segments_of(q)).collect());
            if scope.patches.iter().any(|&q| taken[q]) {
                continue;
            }
            for &q in &scope.patches {
                taken[q] = true;
            }
            regions.push(scope);
        }
        if regions.is_empty() {
            break;
        }
        let current: &[u32] = n;
        let solved: Vec<Option<Vec<u32>>> = regions
            .par_iter()
            .map(|region| solve(spec, current, region, scaled(LOCAL_NODE_LIMIT, spec.effort)))
            .collect();
        if !apply(n, &regions, solved, accept) {
            break;
        }
    }
    Ok(())
}

/// Rounds of region repair.
const MAX_ROUNDS: usize = 30;

/// Unit steps on each side of the target that the deviation is exact over.
const DEVIATION_STEPS: i64 = 3;

/// Rings of patches freed around a problem patch, and the region size cap.
const REGION_RINGS: usize = 1;
const MAX_REGION: usize = 15;
/// Branch-and-bound nodes per region.
const LOCAL_NODE_LIMIT: u64 = 500;

/// A node limit times the effort, at least one node.
fn scaled(limit: u64, effort: f64) -> u64 {
    (limit as f64 * effort).round().max(1.0) as u64
}

/// The counts of `scope`'s free segments (in their order) from its integer
/// program, searched for `nodes` nodes from the counts in `start`; every
/// other segment stays at its count there. `None` if the search fails.
fn solve(spec: Spec<'_>, start: &[u32], scope: &Scope, nodes: u64) -> Option<Vec<u32>> {
    let Built { problem, count, warm } = build(spec, start, scope);
    let mut options = SolveOptions::default();
    options.node_limit = Some(nodes);
    options.mip_gap = 1e-3;
    options.warm_start = Some(warm);
    let outcome = problem.solve_with(options).ok()?;
    let solution = outcome.solution()?;
    scope
        .free
        .iter()
        .map(|&s| {
            let value = solution.var_value_raw(count[s]?).round();
            (value.is_finite() && value >= 1.0).then_some(value as u32)
        })
        .collect()
}

/// The deviation `(n − t)² / max(t, 1)` of a count `n` from `lo` to `hi`, as
/// a convex piecewise-linear cost: its value at `lo`, and the `(slope,
/// length)` of each piece from there on, in order. `n = lo + Σ z_k` with
/// `0 ≤ z_k ≤ length_k` then costs `value + Σ slope_k · z_k` at the least,
/// because the slopes rise: a minimum fills the pieces in order.
///
/// It is the quadratic exactly at every integer within [`DEVIATION_STEPS`]
/// of the target, one piece per unit step there, and goes on straight
/// outside them (the first and last piece reach to `lo` and `hi`).
///
/// As rows this was seven per segment (`d ≥` each step's chord), three
/// quarters of the whole program. As bounded columns it is none: the
/// simplex method carries bounds for nothing, and a search node costs half
/// what it did.
fn deviation_pieces(t: f64, lo: i32, hi: i32) -> (f64, TinyVec<[(f64, f64); 8]>) {
    let w = 1.0 / t.max(1.0);
    let f = |i: f64| w * (i - t) * (i - t);
    let slope = |i: i64| f(i as f64 + 1.0) - f(i as f64);
    let base = t.floor() as i64;
    // Where each unit step starts, in order and once each.
    let mut steps: TinyVec<[i64; 8]> = TinyVec::new();
    for i in ((base - DEVIATION_STEPS)..=(base + DEVIATION_STEPS)).map(|i| i.max(1)) {
        if steps.last() != Some(&i) {
            steps.push(i);
        }
    }
    let (lo_f, hi_f) = (f64::from(lo), f64::from(hi));
    // The cost at `lo`: the highest chord there.
    let at_lo =
        steps.iter().map(|&i| f(i as f64) + slope(i) * (lo_f - i as f64)).fold(f64::NEG_INFINITY, f64::max);
    let last = steps.len() - 1;
    let pieces = steps
        .iter()
        .enumerate()
        .filter_map(|(k, &i)| {
            let from = if k == 0 { lo_f } else { (i as f64).max(lo_f) };
            let to = if k == last { hi_f } else { (i as f64 + 1.0).min(hi_f) };
            (to > from).then(|| (slope(i), to - from))
        })
        .collect();
    (at_lo, pieces)
}

/// The program that decides `scope`, with a warm start from `start`.
fn build(spec: Spec<'_>, start: &[u32], scope: &Scope) -> Built {
    let Spec { segments, targets, fixed, regularity, .. } = spec;
    let m = segments.segments.len();
    let mut problem = Problem::new(OptimizationDirection::Minimize);
    let mut warm: Vec<(Variable, f64)> = Vec::new();
    let int_var = |problem: &mut Problem, lo: i32, hi: i32| problem.add_integer_var(0.0, (lo, hi));

    // Segment counts and deviations.
    let mut count: Vec<Option<Variable>> = vec![None; m];
    let mut at_bounds = 0.0;
    for &s in scope.free.iter().filter(|&&s| fixed[s].is_none()) {
        let t = targets[s];
        let hi = (4.0 * t.ceil() + 8.0).min(i32::MAX as f64) as i32;
        let lo = super::quantize::floor_of(segments, s, t).min(hi as u32) as i32;
        let n = int_var(&mut problem, lo, hi);
        // n = lo + Σ z, and the deviation is the pieces' cost over its value at lo.
        let (at_lo, pieces) = deviation_pieces(t, lo, hi);
        at_bounds += at_lo;
        let v = start[s] as f64;
        let mut row: Vec<(Variable, f64)> = vec![(n, 1.0)];
        let mut filled = f64::from(lo);
        for (slope, length) in pieces {
            let z = problem.add_var(slope, (0.0, length));
            row.push((z, -1.0));
            let part = (v - filled).clamp(0.0, length);
            warm.push((z, part));
            filled += part;
        }
        problem.add_constraint(row, ComparisonOp::Eq, f64::from(lo));
        warm.push((n, v));
        count[s] = Some(n);
    }
    if at_bounds != 0.0 {
        // What the deviations come to with every count at its lower bound: a
        // constant, kept in the objective so that the gap the search stops
        // at is a share of the real cost.
        let one = problem.add_var(at_bounds, (1.0, 1.0));
        warm.push((one, 1.0));
    }

    // A side's count as a linear expression plus a constant (the segments
    // this program doesn't decide, at their fixed or present counts).
    let side = |side: &[(u32, bool)]| -> SideTerms {
        let mut terms = Vec::new();
        let (mut constant, mut warm_value) = (0.0, 0.0);
        for &(s, _) in side {
            let s = s as usize;
            match count[s] {
                Some(v) => terms.push((v, 1.0)),
                None => constant += fixed[s].unwrap_or(start[s]) as f64,
            }
            warm_value += start[s] as f64;
        }
        (terms, constant, warm_value)
    };

    for sides in scope.patches.iter().filter_map(|&p| segments.sides.get(p)?.as_ref()) {
        let k = sides.len();
        let lens: Vec<SideTerms> = sides.iter().map(|s| side(s)).collect();
        if lens.iter().all(|(terms, _, _)| terms.is_empty()) {
            continue; // nothing to decide
        }
        if k == 4 {
            // Even total: Σ l = 2 h.
            let total_warm: f64 = lens.iter().map(|l| l.2).sum();
            let h = int_var(&mut problem, 0, i32::MAX / 4);
            let mut expr: Vec<(Variable, f64)> = lens.iter().flat_map(|l| l.0.iter().copied()).collect();
            expr.push((h, -2.0));
            let constant: f64 = lens.iter().map(|l| l.1).sum();
            problem.add_constraint(expr, ComparisonOp::Eq, -constant);
            warm.push((h, (total_warm / 2.0).floor()));
            for (a, c) in [(0, 2), (1, 3)] {
                let ep = problem.add_var(regularity, (0.0, f64::INFINITY));
                let em = problem.add_var(regularity, (0.0, f64::INFINITY));
                let mut expr: Vec<(Variable, f64)> = vec![(ep, 1.0), (em, -1.0)];
                expr.extend(lens[a].0.iter().map(|&(v, c)| (v, -c)));
                expr.extend(lens[c].0.iter().copied());
                problem.add_constraint(expr, ComparisonOp::Eq, lens[a].1 - lens[c].1);
                let diff = lens[a].2 - lens[c].2;
                warm.push((ep, diff.max(0.0)));
                warm.push((em, (-diff).max(0.0)));
            }
            continue;
        }
        if !matches!(k, 3 | 5 | 6) {
            continue;
        }
        let sigma = problem.add_var(4.0 * regularity, (0.0, f64::INFINITY));
        let mut sigma_warm = 0.0f64;
        let cycles = if k % 2 == 1 { 1 } else { 2 };
        let mut x_warm_ok = true;
        let mut x_warm: Vec<(Variable, f64)> = Vec::new();
        for start_index in 0..cycles {
            // The cycle start_index, start_index+2, … and its alternating signs.
            let mut order = Vec::new();
            let mut j = start_index;
            loop {
                order.push(j);
                j = (j + 2) % k;
                if j == start_index {
                    break;
                }
            }
            let x0 = int_var(&mut problem, i32::MIN / 4, i32::MAX / 4);
            // 2·x0 = Σ (−1)^i l_{order[i]}
            let mut expr: Vec<(Variable, f64)> = vec![(x0, -2.0)];
            let (mut constant, mut alt_warm) = (0.0, 0.0);
            for (i, &j) in order.iter().enumerate() {
                let sign = if i % 2 == 0 { 1.0 } else { -1.0 };
                expr.extend(lens[j].0.iter().map(|&(v, c)| (v, sign * c)));
                constant += sign * lens[j].1;
                alt_warm += sign * lens[j].2;
            }
            problem.add_constraint(expr, ComparisonOp::Eq, -constant);
            if alt_warm.rem_euclid(2.0) != 0.0 {
                x_warm_ok = false;
            }
            x_warm.push((x0, (alt_warm / 2.0).floor()));
            // x_j as (coefficient of x0, terms, constant), walking the cycle.
            let mut x_coef = 1.0;
            let mut x_terms: Vec<(Variable, f64)> = Vec::new();
            let mut x_const = 0.0;
            let mut x_val = (alt_warm / 2.0).floor();
            for (i, &j) in order.iter().enumerate() {
                // x_j ≥ 1 − σ  and  l_j − x_j ≥ 1 − σ.
                let mut lower: Vec<(Variable, f64)> = vec![(x0, x_coef), (sigma, 1.0)];
                lower.extend(x_terms.iter().copied());
                problem.add_constraint(lower, ComparisonOp::Ge, 1.0 - x_const);
                let mut upper: Vec<(Variable, f64)> = vec![(x0, -x_coef), (sigma, 1.0)];
                upper.extend(x_terms.iter().map(|&(v, c)| (v, -c)));
                upper.extend(lens[j].0.iter().copied());
                problem.add_constraint(upper, ComparisonOp::Ge, 1.0 - lens[j].1 + x_const);
                sigma_warm = sigma_warm.max(1.0 - x_val).max(1.0 - (lens[j].2 - x_val));
                if i + 1 < order.len() {
                    // x_next = l_j − x_j
                    x_coef = -x_coef;
                    x_terms =
                        x_terms.iter().map(|&(v, c)| (v, -c)).chain(lens[j].0.iter().copied()).collect();
                    x_const = lens[j].1 - x_const;
                    x_val = lens[j].2 - x_val;
                }
            }
        }
        if x_warm_ok {
            warm.extend(x_warm);
            warm.push((sigma, sigma_warm.max(0.0)));
        }
    }

    Built { problem, count, warm }
}

/// A built program.
struct Built {
    problem: Problem,
    /// The count variable of each free segment.
    count: Vec<Option<Variable>>,
    /// A warm start from the heuristic counts.
    warm: Vec<(Variable, f64)>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quad::segments::{Segment, SegmentUse};

    /// A layout from each patch's sides, each a list of segment ids.
    fn layout(patches: &[&[&[u32]]]) -> Segments {
        let count = patches.iter().flat_map(|p| p.iter().flat_map(|s| s.iter())).max().map_or(0, |&s| s + 1);
        let mut segments =
            vec![Segment { vertices: Vec::new(), uses: Vec::new(), polyline: false }; count as usize];
        for (p, sides) in patches.iter().enumerate() {
            for (k, side) in sides.iter().enumerate() {
                for &s in side.iter() {
                    segments[s as usize].uses.push(SegmentUse {
                        patch: p as u32,
                        side: k as u8,
                        reversed: false,
                    });
                }
            }
        }
        let sides = patches
            .iter()
            .map(|p| Some(p.iter().map(|side| side.iter().map(|&s| (s, false)).collect()).collect()))
            .collect();
        Segments { segments, sides, twins: Vec::new() }
    }

    /// Two quads that share segment 2, and a third quad apart from them.
    fn three_quads() -> Segments {
        layout(&[&[&[0], &[1], &[2], &[3]], &[&[2], &[4], &[5], &[6]], &[&[7], &[8], &[9], &[10]]])
    }

    #[test]
    fn the_deviation_s_pieces_are_the_quadratic_at_the_integers() {
        let cases = [
            (5.3, 2, 28),
            (1.2, 1, 16),
            (0.4, 1, 12),
            (7.0, 3, 36),
            (5.3, 5, 6),
            (5.3, 9, 30),
            (5.3, 1, 3),
            (2.5, 2, 2),
        ];
        for (t, lo, hi) in cases {
            let (at_lo, pieces) = deviation_pieces(t, lo, hi);
            // The pieces cover the range, and their slopes rise.
            assert_eq!(pieces.iter().map(|p| p.1).sum::<f64>(), f64::from(hi - lo), "{t} {lo} {hi}");
            assert!(pieces.windows(2).all(|w| w[0].0 < w[1].0), "{t} {lo} {hi}: {pieces:?}");
            let quadratic = |n: f64| (n - t) * (n - t) / t.max(1.0);
            let exact = (t.floor() - 3.0).max(1.0)..=(t.floor() + 4.0);
            for n in lo..=hi {
                // Filled in order, as a minimum fills them.
                let mut left = f64::from(n - lo);
                let mut cost = at_lo;
                for &(slope, length) in &pieces {
                    let z = left.min(length);
                    cost += slope * z;
                    left -= z;
                }
                let n = f64::from(n);
                if exact.contains(&n) {
                    assert!((cost - quadratic(n)).abs() < 1e-9, "{t} {lo} {hi} at {n}: {cost}");
                } else {
                    // Straight on from there: under the quadratic, never over it.
                    assert!(cost <= quadratic(n) + 1e-9 && cost > 0.0, "{t} {lo} {hi} at {n}: {cost}");
                }
            }
        }
    }

    #[test]
    fn free_segments_fall_into_parts_by_the_patches_they_share() {
        let segments = three_quads();
        let free = vec![None; 11];
        assert_eq!(
            parts(&segments, &free),
            [
                Scope { free: (0..7).collect(), patches: vec![0, 1] },
                Scope { free: (7..11).collect(), patches: vec![2] }
            ]
        );
        // With the shared segment fixed the two quads no longer bear on each
        // other: its count is a constant to both.
        let mut fixed = free;
        fixed[2] = Some(4);
        assert_eq!(
            parts(&segments, &fixed),
            [
                Scope { free: vec![0, 1, 3], patches: vec![0] },
                Scope { free: vec![4, 5, 6], patches: vec![1] },
                Scope { free: (7..11).collect(), patches: vec![2] }
            ]
        );
        assert!(parts(&segments, &[Some(2); 11]).is_empty());
        assert!(parts(&Segments::default(), &[]).is_empty());
    }

    #[test]
    fn each_part_s_program_gives_what_the_whole_program_gives() {
        let segments = three_quads();
        let targets = [3.2, 5.7, 4.1, 5.2, 6.6, 2.9, 6.3, 2.2, 8.4, 2.4, 7.7];
        let fixed = vec![None; 11];
        let spec =
            Spec { segments: &segments, targets: &targets, fixed: &fixed, regularity: 50.0, effort: 1.0 };
        let start = vec![4u32; 11];
        let everything = Scope::of(&segments, (0..11).collect());
        let whole = solve(spec, &start, &everything, NODE_LIMIT).unwrap();
        // Opposite sides agree, at the count nearest their targets together.
        assert_eq!(whole, [3, 5, 3, 5, 6, 3, 6, 2, 8, 2, 8]);
        let mut n = start.clone();
        let mut asked: Vec<Vec<usize>> = Vec::new();
        refine(spec, &mut n, &mut |old: &[u32], new: &[u32], changed: &[usize]| {
            // A candidate differs from the counts so far in its own part only.
            assert!((0..11).all(|s| old[s] == new[s] || changed.contains(&s)));
            asked.push(changed.to_vec());
            true
        });
        assert_eq!(n, whole);
        assert_eq!(asked, [(0..7).collect::<Vec<_>>(), (7..11).collect()]);
        // A part that is refused stays as it was; the other is still taken.
        let mut kept = start.clone();
        refine(spec, &mut kept, &mut |_: &[u32], _: &[u32], changed: &[usize]| changed[0] != 0);
        assert_eq!((&kept[..7], &kept[7..]), (&start[..7], &whole[7..]));
        // A segment the program doesn't decide keeps its count, and counts
        // as that to the patches beside it: segment 2 at 4 holds its
        // opposite side at 4 as well.
        let one_quad = Scope::of(&segments, vec![4, 5, 6]);
        assert_eq!(one_quad.patches, [1]);
        assert_eq!(solve(spec, &start, &one_quad, NODE_LIMIT).unwrap(), [6, 4, 6]);
    }
}
