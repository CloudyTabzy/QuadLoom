//! Global target-count sizing; shared segment quantization and bounded plans.

pub const MAX_TARGET_QUADS: usize = 2_000_000;

use super::{Prepared, QuadOutput, QuadParams, Sizing, pattern, quantize};
use crate::{Control, Error, ErrorKind, Stage, TraceOutput};

const MAX_ATTEMPTS: usize = 8;
const MAX_SEGMENT_TARGET: f64 = 32_768.0;

struct Plan {
    counts: Vec<u32>,
    patterns: Vec<Option<pattern::Pattern>>,
    quads: usize,
    edge: f64,
}

#[derive(Clone, Copy, Debug, Default)]
struct RefinementStats {
    visits: usize,
    probes: usize,
    accepted: usize,
    rebuilt: usize,
    quads_built: usize,
    selected: usize,
}

struct RefinementBudget {
    stats: RefinementStats,
    ceiling: RefinementStats,
}

impl Default for RefinementBudget {
    fn default() -> Self {
        Self {
            stats: RefinementStats::default(),
            ceiling: RefinementStats {
                visits: 8192,
                probes: 512,
                accepted: 64,
                rebuilt: 1024,
                quads_built: MAX_TARGET_QUADS,
                selected: 0,
            },
        }
    }
}

type PatternReplacements = (Vec<(usize, pattern::Pattern)>, usize);

fn pattern_rank(kind: pattern::PatternKind) -> u8 {
    match kind {
        pattern::PatternKind::Grid => 0,
        pattern::PatternKind::Center => 1,
        pattern::PatternKind::Split => 2,
        pattern::PatternKind::Fan => 3,
    }
}

fn refine(
    prepared: &Prepared,
    unit: &[f64],
    plan: &mut Plan,
    wanted: usize,
    mirror: usize,
    control: &Control,
) -> Result<RefinementStats, Error> {
    let mut budget = RefinementBudget::default();
    budget.stats.selected = refine_with_budget(prepared, unit, plan, wanted, mirror, control, &mut budget)?;
    Ok(budget.stats)
}

fn refine_with_budget(
    prepared: &Prepared,
    unit: &[f64],
    plan: &mut Plan,
    wanted: usize,
    mirror: usize,
    control: &Control,
    budget: &mut RefinementBudget,
) -> Result<usize, Error> {
    control.checkpoint(Stage::Quantize)?;
    let ideals: Vec<f64> = unit.iter().map(|v| v / plan.edge).collect();
    let upper: Vec<u32> = ideals
        .iter()
        .zip(&plan.counts)
        .map(|(&t, &n)| n.max((2.0 * t).ceil().min(u32::MAX as f64) as u32))
        .collect();
    let ceiling = budget.ceiling;
    let stats = &mut budget.stats;
    let accepted_before = stats.accepted;
    let limit = MAX_TARGET_QUADS / mirror;
    let mut next_start = 0usize;
    while !within_tolerance(wanted, plan.quads * mirror)
        && stats.probes < ceiling.probes
        && stats.visits < ceiling.visits
        && stats.accepted < ceiling.accepted
        && stats.rebuilt < ceiling.rebuilt
        && stats.quads_built < ceiling.quads_built
    {
        let mut moves = quantize::target_moves(&prepared.segs, &plan.counts, &ideals, control)?;
        if moves.is_empty() {
            break;
        }
        let move_count = moves.len();
        let rotation = next_start % move_count;
        moves.rotate_left(rotation);
        let mut accepted = false;
        'candidates: for (offset, moved) in moves.into_iter().enumerate() {
            for delta in [1i64, 2, -1, -2] {
                control.checkpoint(Stage::Quantize)?;
                if stats.probes >= ceiling.probes
                    || stats.rebuilt >= ceiling.rebuilt
                    || stats.quads_built >= ceiling.quads_built
                    || stats.visits >= ceiling.visits
                {
                    break 'candidates;
                }
                stats.visits += 1;
                if moved.iter().any(|&s| {
                    let after = i64::from(plan.counts[s]) + delta;
                    prepared.segs.segments[s].polyline
                        || after < i64::from(quantize::count_floor(ideals[s]))
                        || after > i64::from(upper[s])
                }) {
                    continue;
                }
                let mut affected: Vec<usize> = moved
                    .iter()
                    .flat_map(|&s| prepared.segs.segments[s].uses.iter().map(|u| u.patch as usize))
                    .collect();
                affected.sort_unstable();
                affected.dedup();
                if affected.iter().any(|&p| plan.patterns.get(p).is_none_or(Option::is_none)) {
                    continue;
                }
                stats.probes += 1;
                for &s in &moved {
                    plan.counts[s] = (i64::from(plan.counts[s]) + delta) as u32;
                }
                let replacements = (|| -> Result<Option<PatternReplacements>, Error> {
                    let mut changed = Vec::new();
                    let mut total = plan.quads;
                    for &p in &affected {
                        if stats.rebuilt >= ceiling.rebuilt || stats.quads_built >= ceiling.quads_built {
                            return Ok(None);
                        }
                        control.checkpoint(Stage::Patterns)?;
                        let Some(sides) = prepared.segs.sides[p].as_ref() else { return Ok(None) };
                        let sizes: Option<Vec<u32>> = sides
                            .iter()
                            .map(|side| {
                                side.iter()
                                    .try_fold(0u32, |sum, &(s, _)| sum.checked_add(plan.counts[s as usize]))
                            })
                            .collect();
                        let Some(sizes) = sizes else { return Ok(None) };
                        if sizes.iter().map(|&n| u64::from(n)).sum::<u64>() % 2 != 0 {
                            return Ok(None);
                        }
                        stats.rebuilt += 1;
                        let Some(old) = plan.patterns[p].as_ref() else { return Ok(None) };
                        let Some(others) = total.checked_sub(old.quads.len()) else { return Ok(None) };
                        let allowed = limit.saturating_sub(others);
                        let remaining = ceiling.quads_built - stats.quads_built;
                        let built = match pattern::build_limited(&sizes, allowed.min(remaining), control) {
                            Ok(Some(pattern)) => pattern,
                            Err(error) if error.kind == ErrorKind::Cancelled => return Err(error),
                            _ => return Ok(None),
                        };
                        stats.quads_built += built.quads.len();
                        if pattern_rank(built.kind) > pattern_rank(old.kind) {
                            return Ok(None);
                        }
                        let Some(next) =
                            total.checked_sub(old.quads.len()).and_then(|q| q.checked_add(built.quads.len()))
                        else {
                            return Ok(None);
                        };
                        total = next;
                        changed.push((p, built));
                    }
                    if total > limit
                        || (total * mirror).abs_diff(wanted) >= (plan.quads * mirror).abs_diff(wanted)
                    {
                        return Ok(None);
                    }
                    Ok(Some((changed, total)))
                })();
                match replacements {
                    Ok(Some((changed, total))) => {
                        for (p, pattern) in changed {
                            plan.patterns[p] = Some(pattern);
                        }
                        plan.quads = total;
                        stats.accepted += 1;
                        next_start = (rotation + offset + 1) % move_count;
                        accepted = true;
                        break 'candidates;
                    }
                    other => {
                        for &s in &moved {
                            plan.counts[s] = (i64::from(plan.counts[s]) - delta) as u32;
                        }
                        let _ = other?;
                    }
                }
            }
        }
        if !accepted {
            break;
        }
    }
    Ok(stats.accepted - accepted_before)
}

fn refine_best(
    prepared: &Prepared,
    unit: &[f64],
    mut plans: Vec<Plan>,
    wanted: usize,
    mirror: usize,
    control: &Control,
) -> Result<(Plan, RefinementStats), Error> {
    plans.sort_by_key(|p| (p.quads * mirror).abs_diff(wanted));
    let mut plans = plans.into_iter();
    let Some(mut best) = plans.next() else {
        return Err(Error::invalid_input(Stage::Quantize, "no valid target plan could be built"));
    };
    let Some(mut alternative) = plans.next() else {
        let stats = refine(prepared, unit, &mut best, wanted, mirror, control)?;
        return Ok((best, stats));
    };
    // Reserve half of the single global budget for the opposite-side start.
    let mut budget = RefinementBudget {
        ceiling: RefinementStats {
            visits: 4096,
            probes: 256,
            accepted: 32,
            rebuilt: 512,
            quads_built: MAX_TARGET_QUADS / 2,
            selected: 0,
        },
        ..Default::default()
    };
    let mut selected = refine_with_budget(prepared, unit, &mut best, wanted, mirror, control, &mut budget)?;
    // The nearest scalar plan can sit in a fan plateau or at its size envelope.
    // At most one start from the opposite side explores a different valid basin.
    if !within_tolerance(wanted, best.quads * mirror) {
        budget.ceiling = RefinementBudget::default().ceiling;
        let other =
            refine_with_budget(prepared, unit, &mut alternative, wanted, mirror, control, &mut budget)?;
        if (alternative.quads * mirror).abs_diff(wanted) < (best.quads * mirror).abs_diff(wanted) {
            best = alternative;
            selected = other;
        }
    }
    budget.stats.selected = selected;
    Ok((best, budget.stats))
}

/// Reporting tolerance only: topology/feature constraints may prevent meeting it.
pub(crate) fn within_tolerance(wanted: usize, actual: usize) -> bool {
    actual.abs_diff(wanted) as f64 <= (wanted as f64 * 0.05).max(4.0)
}

fn effective_area(
    trace: &TraceOutput,
    sizing: Sizing<'_>,
    control: &Control,
    included: Option<&[bool]>,
) -> Result<f64, Error> {
    let mut area = 0.0;
    for (f, triangle) in trace.mesh.triangles.iter().enumerate() {
        if f % 1024 == 0 {
            control.checkpoint(Stage::Quantize)?;
        }
        if included.is_some_and(|mask| !mask.get(f).copied().unwrap_or(false)) {
            continue;
        }
        let [a, b, c] = triangle.map(|v| trace.mesh.positions.get(v as usize).copied().unwrap_or_default());
        let source = trace.source_face.get(f).copied().unwrap_or(f as u32) as usize;
        let local = sizing
            .local
            .and_then(|local| local.get(source))
            .copied()
            .filter(|v| v.is_finite() && *v > 0.0)
            .unwrap_or(1.0);
        let [sx, sy] = sizing
            .scales
            .and_then(|scales| scales.get(source))
            .copied()
            .filter(|v| v.iter().all(|v| v.is_finite() && *v > 0.0))
            .unwrap_or([1.0, 1.0]);
        area += (b - a).cross(c - a).length() * 0.5 * sx * sy / (local * local);
    }
    if !area.is_finite() || area < 0.0 || (area == 0.0 && included.is_none()) {
        return Err(Error::invalid_input(
            Stage::Quantize,
            "target sizing needs positive finite surface area",
        ));
    }
    Ok(area)
}

fn variable_correction(actual: usize, wanted: usize, fixed: usize) -> f64 {
    (actual.saturating_sub(fixed).max(1) as f64 / wanted.saturating_sub(fixed).max(1) as f64).sqrt()
}

fn candidate(
    prepared: &Prepared,
    unit: &[f64],
    edge: f64,
    params: &QuadParams,
    limit: usize,
    control: &Control,
) -> Result<Plan, Error> {
    let targets: Vec<f64> = unit.iter().map(|length| (length / edge).max(1e-6)).collect();
    if targets.iter().any(|v| !v.is_finite() || *v > MAX_SEGMENT_TARGET) {
        return Err(Error::invalid_input(Stage::Quantize, "target segment subdivision limit exceeded"));
    }
    let counts =
        quantize::quantize(&prepared.segs, &targets, params.regularity, params.solver_effort, control)?;
    let mut patterns = Vec::with_capacity(prepared.faces_of.len());
    let mut quads = 0usize;
    for (p, faces) in prepared.faces_of.iter().enumerate() {
        control.checkpoint(Stage::Patterns)?;
        let pattern = match prepared.segs.sides.get(p).and_then(Option::as_ref) {
            Some(sides) => {
                let sizes: Option<Vec<u32>> = sides
                    .iter()
                    .map(|side| {
                        side.iter().try_fold(0u32, |sum, &(s, _)| sum.checked_add(counts[s as usize]))
                    })
                    .collect();
                let sizes = sizes
                    .ok_or_else(|| Error::invalid_input(Stage::Patterns, "target side count overflow"))?;
                pattern::build_limited(&sizes, limit.saturating_sub(quads), control)?
            }
            None => None,
        };
        let count = match &pattern {
            Some(pattern) => pattern.quads.len(),
            None => faces
                .len()
                .checked_mul(3)
                .ok_or_else(|| Error::invalid_input(Stage::Patterns, "target fallback count overflow"))?,
        };
        quads = quads.checked_add(count).filter(|total| *total <= limit).ok_or_else(|| {
            Error::invalid_input(Stage::Patterns, "target plan exceeds the quad output limit")
        })?;
        patterns.push(pattern);
    }
    // Every usable patch has a plan; unusable patches are the existing 3-quads/triangle fallback.
    Ok(Plan { counts, patterns, quads, edge })
}

/// Plans global shared segment counts, then maps and smooths the chosen plan once.
pub fn compute(
    trace: &TraceOutput,
    sizing: Sizing<'_>,
    params: &QuadParams,
    mirror_factor: usize,
    control: &Control,
) -> Result<QuadOutput, Error> {
    control.checkpoint(Stage::Quantize)?;
    let wanted = params.target_quads;
    if wanted == 0 || wanted > MAX_TARGET_QUADS || !matches!(mirror_factor, 1 | 2 | 4 | 8) {
        return Err(Error::invalid_input(
            Stage::Quantize,
            "target_quads or symmetry multiplier is out of range",
        ));
    }
    let prepared = Prepared::new(trace, control)?;
    let unit = super::targets(trace, &prepared.segs, sizing, 1.0);
    let fixed: usize = prepared
        .faces_of
        .iter()
        .enumerate()
        .filter(|(p, _)| prepared.segs.sides.get(*p).is_none_or(Option::is_none))
        .map(|(_, faces)| faces.len().saturating_mul(3))
        .sum();
    let fixed_total = fixed.saturating_mul(mirror_factor);
    let included: Vec<bool> = trace
        .patch_of_face
        .iter()
        .map(|&p| prepared.segs.sides.get(p as usize).is_some_and(Option::is_some))
        .collect();
    let area = effective_area(trace, sizing, control, Some(&included))?;
    let mut edge = if fixed_total >= wanted || area == 0.0 {
        (unit.iter().copied().fold(0.0, f64::max) * 4.0).max(1.0)
    } else {
        (area * mirror_factor as f64 / (wanted - fixed_total) as f64).sqrt()
    };
    let limit = MAX_TARGET_QUADS / mirror_factor;
    let mut plans: [Option<Plan>; 2] = [None, None];
    let mut history: Vec<(f64, usize)> = Vec::new();
    let mut attempts = 0;
    let mut last_error = None;
    for attempt in 0..MAX_ATTEMPTS {
        attempts = attempt + 1;
        control.checkpoint(Stage::Quantize)?;
        control.set_progress(Stage::Quantize, attempt as f32 / MAX_ATTEMPTS as f32);
        let plan = match candidate(&prepared, &unit, edge, params, limit, control) {
            Ok(plan) => plan,
            Err(error) if error.kind == ErrorKind::Cancelled => return Err(error),
            Err(error) => {
                last_error = Some(error);
                edge *= 2.0;
                continue;
            }
        };
        let actual = plan.quads * mirror_factor;
        let slot = usize::from(actual >= wanted);
        let old_difference = plans[slot].as_ref().map(|b| (b.quads * mirror_factor).abs_diff(wanted));
        let done = within_tolerance(wanted, actual);
        if old_difference.is_none_or(|old| actual.abs_diff(wanted) < old) {
            plans[slot] = Some(plan);
        }
        if done || fixed_total >= wanted {
            break;
        }
        let plateau = history.last().is_some_and(|&(_, previous)| previous == actual);
        history.push((edge, actual));
        let dense =
            history.iter().filter(|&&(_, count)| count > wanted).max_by(|a, b| a.0.total_cmp(&b.0)).copied();
        let sparse =
            history.iter().filter(|&&(_, count)| count < wanted).min_by(|a, b| a.0.total_cmp(&b.0)).copied();
        let correction = if plateau {
            if actual > wanted { 1.25 } else { 0.8 }
        } else {
            variable_correction(actual, wanted, fixed_total).clamp(0.25, 4.0)
        };
        let proposed = edge * correction;
        edge = if let (Some((a, _)), Some((b, _))) = (dense, sparse) {
            if a < b {
                let (lo, hi) = (a.ln(), b.ln());
                proposed.ln().clamp(lo + 0.1 * (hi - lo), hi - 0.1 * (hi - lo)).exp()
            } else {
                proposed
            }
        } else {
            proposed
        };
        if !edge.is_finite() || edge <= 0.0 {
            break;
        }
    }
    let plans: Vec<Plan> = plans.into_iter().flatten().collect();
    if plans.is_empty() {
        return Err(last_error.unwrap_or_else(|| {
            Error::invalid_input(Stage::Quantize, "no valid target plan could be built")
        }));
    }
    let (best, refinement) = refine_best(&prepared, &unit, plans, wanted, mirror_factor, control)?;
    let mut output =
        prepared.assemble(trace, &best.counts, best.edge, params, Some((&best.patterns, limit)), control)?;
    output.report.target_quads = wanted;
    output.report.target_attempts = attempts;
    output.report.target_refinements = refinement.selected;
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Control, TraceOutput};
    use glam::DVec3;

    #[test]
    fn metric_area_keeps_adaptive_and_frame_proportions() {
        let mut trace = TraceOutput::default();
        trace.mesh.positions = vec![DVec3::ZERO, DVec3::X * 2.0, DVec3::Y, DVec3::new(2.0, 1.0, 0.0)];
        trace.mesh.triangles = vec![[0, 1, 3], [0, 3, 2]];
        trace.source_face = vec![0, 1];
        let sizing = super::super::Sizing { scales: Some(&[[3.0, 2.0]; 2]), local: Some(&[2.0; 2]) };
        let area = effective_area(&trace, sizing, &Control::default(), None).unwrap();
        assert!((area - 3.0).abs() < 1e-12, "area {area}");
        for position in &mut trace.mesh.positions {
            *position *= 10.0;
        }
        let scaled = effective_area(&trace, sizing, &Control::default(), None).unwrap();
        assert!((scaled / area - 100.0).abs() < 1e-10);
    }

    #[test]
    fn feedback_subtracts_fixed_fallback_faces() {
        // 1,500 fixed faces plus 2,000 variable faces; the remaining budget is 500.
        // Doubling the edge makes variable faces 4x fewer, reaching 2,000 total.
        assert!((variable_correction(3500, 2000, 1500) - 2.0).abs() < 1e-12);
        assert!((variable_correction(3500, 2000, 0) - (1.75f64).sqrt()).abs() < 1e-12);
    }

    fn two_grid_plan() -> (Prepared, Plan) {
        use super::super::segments::{Segment, SegmentUse, Segments};
        let side_ids = [[0, 1, 2, 3], [4, 5, 6, 1]];
        let mut segments: Vec<Segment> =
            (0..7).map(|_| Segment { vertices: vec![0, 1], uses: Vec::new(), polyline: false }).collect();
        let sides = side_ids
            .iter()
            .enumerate()
            .map(|(p, ids)| {
                Some(
                    ids.iter()
                        .enumerate()
                        .map(|(i, &s)| {
                            segments[s].uses.push(SegmentUse {
                                patch: p as u32,
                                side: i as u8,
                                reversed: false,
                            });
                            vec![(s as u32, false)]
                        })
                        .collect(),
                )
            })
            .collect();
        let prepared = Prepared { faces_of: vec![vec![], vec![]], segs: Segments { segments, sides } };
        let plan =
            Plan { counts: vec![2; 7], patterns: vec![pattern::build(&[2; 4]); 2], quads: 8, edge: 1.0 };
        (prepared, plan)
    }

    #[test]
    fn shared_strip_refinement_improves_count_without_degrading_grids() {
        let (prepared, mut plan) = two_grid_plan();
        let stats = refine(&prepared, &[2.0; 7], &mut plan, 24, 1, &Control::default()).unwrap();
        assert!(plan.quads.abs_diff(24) < 16);
        assert!(plan.patterns.iter().all(|p| p.as_ref().unwrap().kind == pattern::PatternKind::Grid));
        assert!(plan.counts.iter().all(|&n| (1..=4).contains(&n)));
        assert!(stats.probes <= 512 && stats.accepted <= 64 && stats.rebuilt <= 1024);
        // Mirror-aware acceptance uses total output, not the kept half.
        let (_, mut mirrored) = two_grid_plan();
        refine(&prepared, &[2.0; 7], &mut mirrored, 48, 2, &Control::default()).unwrap();
        assert!((mirrored.quads * 2).abs_diff(48) < 32);
    }

    #[test]
    fn refinement_keeps_fixed_boundaries_and_propagates_cancel() {
        let (mut prepared, mut plan) = two_grid_plan();
        for segment in &mut prepared.segs.segments {
            segment.polyline = true;
        }
        let stats = refine(&prepared, &[2.0; 7], &mut plan, 24, 1, &Control::default()).unwrap();
        assert_eq!(stats.accepted, 0);
        assert_eq!(plan.counts, vec![2; 7]);
        let control = Control::default();
        control.cancel();
        let error = refine(&prepared, &[2.0; 7], &mut plan, 24, 1, &control).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Cancelled);
    }

    #[test]
    fn cheap_rejections_do_not_starve_later_pattern_probes() {
        use super::super::segments::{Segment, SegmentUse};
        let (base, original) = two_grid_plan();
        let mut prepared = Prepared { faces_of: Vec::new(), segs: Default::default() };
        let mut plan = Plan { counts: Vec::new(), patterns: Vec::new(), quads: 0, edge: 1.0 };
        let mut ideals = Vec::new();
        for p in 0..128 {
            let mut sides = Vec::new();
            for i in 0..4 {
                let id = prepared.segs.segments.len();
                prepared.segs.segments.push(Segment {
                    vertices: vec![0, 1],
                    uses: vec![SegmentUse { patch: p, side: i, reversed: false }],
                    polyline: false,
                });
                sides.push(vec![(id as u32, false)]);
                plan.counts.push(1);
                ideals.push(0.1);
            }
            prepared.segs.sides.push(Some(sides));
            prepared.faces_of.push(vec![]);
            plan.patterns.push(pattern::build(&[1; 4]));
            plan.quads += 1;
        }
        let offset = prepared.segs.segments.len() as u32;
        for mut segment in base.segs.segments {
            for use_ in &mut segment.uses {
                use_.patch += 128;
            }
            prepared.segs.segments.push(segment);
        }
        for sides in base.segs.sides {
            prepared.segs.sides.push(sides.map(|sides| {
                sides
                    .into_iter()
                    .map(|side| side.into_iter().map(|(s, r)| (s + offset, r)).collect())
                    .collect()
            }));
        }
        prepared.faces_of.extend([vec![], vec![]]);
        plan.counts.extend(original.counts);
        plan.patterns.extend(original.patterns);
        plan.quads += 8;
        ideals.extend([2.0; 7]);
        let stats = refine(&prepared, &ideals, &mut plan, 152, 1, &Control::default()).unwrap();
        assert!(within_tolerance(152, plan.quads), "{} {stats:?}", plan.quads);
    }

    #[test]
    fn nonquadrilateral_endpoint_keeps_even_parity_during_refinement() {
        use super::super::segments::{Segment, SegmentUse, Segments};
        let segments = (0..3)
            .map(|side| Segment {
                vertices: vec![0, 1],
                uses: vec![SegmentUse { patch: 0, side, reversed: false }],
                polyline: false,
            })
            .collect();
        let prepared = Prepared {
            faces_of: vec![vec![]],
            segs: Segments { segments, sides: vec![Some((0..3).map(|s| vec![(s, false)]).collect())] },
        };
        let mut plan =
            Plan { counts: vec![4; 3], patterns: vec![pattern::build(&[4; 3])], quads: 12, edge: 1.0 };
        let stats = refine(&prepared, &[4.0; 3], &mut plan, 24, 1, &Control::default()).unwrap();
        assert!(stats.accepted > 0);
        assert!(plan.counts.iter().all(|n| n % 2 == 0));
        assert_eq!(plan.patterns[0].as_ref().unwrap().kind, pattern::PatternKind::Center);
        assert!(plan.quads.abs_diff(24) < 12);
    }

    #[test]
    fn opposite_side_start_escapes_a_size_limited_nearest_plan() {
        let (prepared, mut below) = two_grid_plan();
        below.edge = 4.0; // its local upper envelope cannot grow it toward 18.
        let (_, mut above) = two_grid_plan();
        above.counts = vec![4; 7];
        above.patterns = vec![pattern::build(&[4; 4]); 2];
        above.quads = 32;
        let (best, _) =
            refine_best(&prepared, &[2.0; 7], vec![below, above], 18, 1, &Control::default()).unwrap();
        assert!(within_tolerance(18, best.quads), "{}", best.quads);
        assert!(best.patterns.iter().all(|p| p.as_ref().unwrap().kind == pattern::PatternKind::Grid));
    }

    #[test]
    fn refinement_starts_share_one_aggregate_work_budget() {
        let (prepared, mut first) = two_grid_plan();
        let (_, mut second) = two_grid_plan();
        let mut budget = RefinementBudget::default();
        budget.stats.visits = 8191;
        refine_with_budget(&prepared, &[2.0; 7], &mut first, 24, 1, &Control::default(), &mut budget)
            .unwrap();
        let after_first = budget.stats.visits;
        refine_with_budget(&prepared, &[2.0; 7], &mut second, 24, 1, &Control::default(), &mut budget)
            .unwrap();
        assert_eq!(after_first, 8192);
        assert_eq!(budget.stats.visits, after_first, "second start reset the work budget");
        assert!(
            budget.stats.probes <= 512
                && budget.stats.rebuilt <= 1024
                && budget.stats.quads_built <= MAX_TARGET_QUADS
        );
    }
}
