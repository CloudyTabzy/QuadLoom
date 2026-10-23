//! Quantization as a small integer program, solved with `microlp` and seeded
//! with the heuristic counts (so it can only improve on them).
//!
//! Variables and terms:
//! - `n_s ≥ 1` is an integer per free segment, with L1 deviation `d_s ≥ |n_s − t_s|`
//!   weighted by `1 / max(t_s, 1)`.
//! - A 4-sided patch has a soft balance per opposite pair, `e⁺ − e⁻ = Σa − Σc`,
//!   weighted by `regularity`, and an even total (integer half-sum).
//! - A patch with 3, 5 or 6 sides has the closed form exactly: integer
//!   `x_start` per step-2 cycle with `2·x_start` = the cycle's alternating sum
//!   (this also makes the total even). The rest follow as `x_{j+2} = l_j − x_j`,
//!   and every split is `≥ 1 − σ_p`, with the slack `σ_p` weighted by
//!   `4·regularity`.
//!
//! The search runs with a deterministic node limit. If it fails or finds
//! nothing better, the heuristic counts stand.

use microlp::{ComparisonOp, OptimizationDirection, Problem, SolveOptions, Variable};

use rayon::prelude::*;

use super::segments::Segments;
use crate::control::{Control, Stage};
use crate::error::Error;

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

/// A side's count: variable terms, a constant, and its value in the warm start.
type SideTerms = (Vec<(Variable, f64)>, f64, f64);

/// Branch-and-bound nodes per solve.
pub const NODE_LIMIT: u64 = 2000;

/// Counts from the whole integer program, seeded with `start` (so never
/// worse by its own measure), for problems up to [`MAX_GLOBAL_SEGMENTS`] free
/// segments; larger ones go straight to [`neighbourhoods`].
pub fn refine(spec: Spec<'_>, start: &[u32]) -> Option<Vec<u32>> {
    if spec.fixed.iter().filter(|f| f.is_none()).count() > MAX_GLOBAL_SEGMENTS {
        return None;
    }
    let full = build(spec, start, Mode::Integer(None));
    run(&full.problem, &full.count, Some(full.warm), start, scaled(NODE_LIMIT, spec.effort))
}

/// Largest problem (free segments) solved as one integer program.
pub const MAX_GLOBAL_SEGMENTS: usize = 2500;

/// Counts improved region by region: around each patch that `bad` flags, the
/// segments of its 2-ring of patches are freed and the integer program solved
/// exactly with everything else fixed. Each round picks regions whose
/// affected patches (every patch touching a freed segment) don't overlap,
/// solves them in parallel, and applies the ones `accept` keeps. They compose
/// exactly because no patch sees two regions. Rounds repeat while some region
/// improves.
pub fn neighbourhoods(
    spec: Spec<'_>,
    n: &mut Vec<u32>,
    bad: &(dyn Fn(&[u32], usize) -> bool + Sync),
    accept: &mut dyn FnMut(&[u32], &[u32]) -> bool,
    control: &Control,
) -> Result<(), Error> {
    let (segments, fixed) = (spec.segments, spec.fixed);
    let patches = segments.sides.len();
    let m = n.len();
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
        let mut regions: Vec<Vec<usize>> = Vec::new(); // free segments per region
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
            let mut free: Vec<usize> = region.iter().flat_map(|&q| segments_of(q)).collect();
            free.sort_unstable();
            free.dedup();
            let mut affected: Vec<usize> = free
                .iter()
                .flat_map(|&s| segments.segments[s].uses.iter().map(|u| u.patch as usize))
                .collect();
            affected.sort_unstable();
            affected.dedup();
            if affected.iter().any(|&q| taken[q]) {
                continue;
            }
            for &q in &affected {
                taken[q] = true;
            }
            regions.push(free);
        }
        if regions.is_empty() {
            break;
        }
        let current: &[u32] = n;
        let solved: Vec<Option<Vec<u32>>> = regions
            .par_iter()
            .map(|free| {
                let mut local_fixed: Vec<Option<u32>> =
                    (0..m).map(|s| Some(fixed[s].unwrap_or(current[s]))).collect();
                for &s in free {
                    local_fixed[s] = None;
                }
                let local = build(Spec { fixed: &local_fixed, ..spec }, current, Mode::Integer(None));
                run(
                    &local.problem,
                    &local.count,
                    Some(local.warm),
                    current,
                    scaled(LOCAL_NODE_LIMIT, spec.effort),
                )
            })
            .collect();
        let mut improved = false;
        for (free, candidate) in regions.iter().zip(solved) {
            let Some(candidate) = candidate else { continue };
            let mut next = n.clone();
            for &s in free {
                next[s] = candidate[s];
            }
            if accept(n, &next) {
                *n = next;
                improved = true;
            }
        }
        if !improved {
            break;
        }
    }
    Ok(())
}

/// Rounds of region repair.
const MAX_ROUNDS: usize = 30;

/// Chords on each side of the target in the quadratic deviation.
const DEVIATION_CHORDS: i64 = 3;

/// Rings of patches freed around a problem patch, and the region size cap.
const REGION_RINGS: usize = 1;
const MAX_REGION: usize = 15;
/// Branch-and-bound nodes per region.
const LOCAL_NODE_LIMIT: u64 = 500;

/// How variables are created.
#[derive(Clone, Copy)]
enum Mode<'a> {
    /// Integer counts, optionally with per-segment bounds.
    Integer(Option<&'a [Option<(i32, i32)>]>),
}

/// A node limit times the effort, at least one node.
fn scaled(limit: u64, effort: f64) -> u64 {
    (limit as f64 * effort).round().max(1.0) as u64
}

/// Solves `problem` within the node limit and reads the counts.
fn run(
    problem: &Problem,
    count: &[Option<Variable>],
    warm: Option<Vec<(Variable, f64)>>,
    start: &[u32],
    nodes: u64,
) -> Option<Vec<u32>> {
    let mut options = SolveOptions::default();
    options.node_limit = Some(nodes);
    options.mip_gap = 1e-3;
    options.warm_start = warm;
    let outcome = problem.solve_with(options).ok()?;
    let solution = outcome.solution()?;
    let mut n = start.to_vec();
    for (s, var) in count.iter().enumerate() {
        if let Some(v) = *var {
            let value = solution.var_value_raw(v).round();
            if !(value.is_finite() && value >= 1.0) {
                return None;
            }
            n[s] = value as u32;
        }
    }
    Some(n)
}

/// The program for `mode`, with a warm start from `start`.
fn build(spec: Spec<'_>, start: &[u32], mode: Mode<'_>) -> Built {
    let Spec { segments, targets, fixed, regularity, .. } = spec;
    let m = segments.segments.len();
    let mut problem = Problem::new(OptimizationDirection::Minimize);
    let mut warm: Vec<(Variable, f64)> = Vec::new();
    let int_var = |problem: &mut Problem, lo: i32, hi: i32| match mode {
        Mode::Integer(_) => problem.add_integer_var(0.0, (lo, hi)),
    };

    // Segment counts and deviations.
    let mut count: Vec<Option<Variable>> = vec![None; m];
    for s in 0..m {
        if fixed[s].is_some() {
            continue;
        }
        let t = targets[s];
        let upper = (4.0 * t.ceil() + 8.0).min(i32::MAX as f64) as i32;
        let floor = super::quantize::count_floor(t).min(upper as u32) as i32;
        let (lo, hi) = match mode {
            Mode::Integer(Some(bounds)) => bounds.get(s).copied().flatten().unwrap_or((floor, upper)),
            _ => (floor, upper),
        };
        let n = int_var(&mut problem, lo, hi);
        // Quadratic deviation (n − t)² / max(t, 1), exact at integers: chords
        // between consecutive integers around the target bound d from below.
        let w = 1.0 / t.max(1.0);
        let f = |i: f64| w * (i - t) * (i - t);
        let d = problem.add_var(1.0, (0.0, f64::INFINITY));
        let base = t.floor() as i64;
        for i in (base - DEVIATION_CHORDS)..=(base + DEVIATION_CHORDS) {
            let i = i.max(1) as f64;
            let slope = f(i + 1.0) - f(i);
            // d ≥ f(i) + slope·(n − i)
            problem.add_constraint([(d, 1.0), (n, -slope)], ComparisonOp::Ge, f(i) - slope * i);
        }
        let v = start[s] as f64;
        warm.push((n, v));
        warm.push((d, f(v)));
        count[s] = Some(n);
    }

    // A side's count as a linear expression plus a constant (fixed segments).
    let side = |side: &[(u32, bool)]| -> SideTerms {
        let mut terms = Vec::new();
        let (mut constant, mut warm_value) = (0.0, 0.0);
        for &(s, _) in side {
            let s = s as usize;
            match (count[s], fixed[s]) {
                (Some(v), _) => terms.push((v, 1.0)),
                (None, Some(f)) => constant += f as f64,
                (None, None) => {}
            }
            warm_value += start[s] as f64;
        }
        (terms, constant, warm_value)
    };

    for sides in segments.sides.iter().flatten() {
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
