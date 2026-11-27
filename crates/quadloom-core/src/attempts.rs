//! Laying a part out more than once and keeping its best quads.
//!
//! The layout is built greedily and what it comes to is chaotic: a change
//! that means nothing (the prep remesh's tolerance a twentieth of a percent
//! larger) gives nineteen models in twenty another result, and three in ten
//! one whose worst quads differ by more than 0.05 in scaled Jacobian
//! (decisions 0026 and 0027). So one layout is one draw. This module draws
//! again: each part is laid out with other seeds
//! ([`trace::compute_parts`]), every layout gets its quads, the quads of
//! each part are scored against the surface they stand for ([`Score`]), and
//! each part keeps the quads that scored best. The first draw is the
//! unseeded layout, so a result is never rated worse than the one a single
//! attempt gives.
//!
//! The unit is the island: a connected part, or several that touch at a
//! vertex (their quads share that vertex, so they are kept together). On a
//! model of many parts each one gets its own best draw, which a choice
//! between whole results cannot give: with thirty parts, some draw is
//! unlucky in every result.
//!
//! The idea is old in fields whose heuristics behave this way. Chip layout
//! tools run place-and-route under several seeds and keep the best pass;
//! randomised restarts take the heavy tail off a search whose run time
//! depends on early choices (Gomes, Selman & Kautz 1998); a multi-start
//! greedy with a local search after each start is GRASP (Feo & Resende
//! 1995), which is this pipeline's shape: a greedy layout, then a relaxer.
//! Decision 0027 has the sources and the measurements.

use std::sync::atomic::{AtomicUsize, Ordering};

use glam::DVec3;
use rayon::prelude::*;

use crate::control::{Control, Stage};
use crate::error::{Error, ErrorKind};
use crate::field::FieldOutput;
use crate::mesh::PolyMesh;
use crate::metrics::quality::quad_quality;
use crate::quad::{PatchNote, QuadOutput};
use crate::spatial::TriangleBvh;
use crate::trace::{self, TraceOutput, TraceParams};
use crate::unionfind::UnionFind;

/// The most layouts a part can be given.
pub const MOST: usize = 8;

/// How many attempts build their quads at once. Each holds a layout and a
/// quad mesh of the parts it tries, so the number bounds the memory; past
/// it the attempts go in rounds.
const SIDE_BY_SIDE: usize = 4;

/// The later attempts' share of the count solver's effort (its node limits).
///
/// The count solver's time depends on the layout far more than on the
/// model: one layout of a part is solved in a second and the next takes
/// seven, where the search runs to its limit. The attempts run side by
/// side, so a run waits for the slowest. This is the cutoff that goes with
/// restarts (Gomes, Selman & Kautz 1998): a later attempt is one draw of
/// several, and a draw that needs a long search is dropped for the next
/// rather than waited for. The first attempt keeps the whole effort, so one
/// attempt is still the result it always was.
///
/// At a quarter, the 70 benchmark models score as they do at the full
/// effort (better than the oracle on 55, 53 and 54 of 60 by mean scaled
/// Jacobian, its 5th percentile and Hausdorff, against 54, 53 and 52; ten
/// more irregular vertices a model), and a single organic part, which is
/// where a long search happens, takes a third less time (the pig 18 s to
/// 11 s, the armadillo 35 s to 27 s). Models of many small parts are
/// unchanged: their solves are short.
pub(crate) const LATER_EFFORT: f64 = 0.25;

/// What the layouts are made from and how their quads are built.
pub(crate) struct Job<'a> {
    /// The prep mesh.
    pub positions: &'a [DVec3],
    pub triangles: &'a [[u32; 3]],
    pub feature_edges: &'a [[u32; 2]],
    /// The field the layouts follow.
    pub field: &'a FieldOutput,
    pub trace: &'a TraceParams,
    /// Builds the quads of a layout of some of the parts, given how many
    /// quads the first attempt gave those parts.
    pub build: &'a (dyn Fn(&TraceOutput, usize) -> Result<QuadOutput, Error> + Sync),
    /// Whether a result of this many quads in all will do (a run that was
    /// asked for a number of quads keeps its first result otherwise).
    pub fits: &'a (dyn Fn(usize) -> bool + Sync),
    /// The quad edge length.
    pub edge: f64,
}

/// How good one island's quads are: less is better.
///
/// The sum of what a result is judged by, each in the unit of a difference
/// worth having: the mean scaled Jacobian (0.01), the worst twentieth of
/// the quads (0.05), the worst quad (0.2), irregular vertices (five per
/// thousand quads) and the farthest the quads and the surface lie apart
/// (a tenth of a quad's size).
///
/// It has to be a sum. Picked by shape alone, the best of several results
/// sits no better on its surface than a single one, and picked by distance
/// alone its shape is worse (measured over six draws of the 70 benchmark
/// models). The units matter less than that all five are in it: every
/// weighting that held all five gained on every measure. The irregular vertices weigh
/// least because they follow shape anyway, and at two per thousand they
/// took a part whose patches were kept as triangles (hundreds of irregular
/// vertices, passable shape) for worse than the same part with its quads
/// sheared, which it is not.
#[derive(Clone, Copy, Debug)]
struct Score {
    /// The scaled Jacobian's mean, fifth percentile and least.
    mean: f64,
    p05: f64,
    min: f64,
    /// Irregular vertices per thousand quads.
    poles: f64,
    /// The largest distance between the quads and the surface, in quad sizes.
    far: f64,
    quads: usize,
}

const MEAN_UNIT: f64 = 0.01;
const P05_UNIT: f64 = 0.05;
const MIN_UNIT: f64 = 0.2;
const POLES_UNIT: f64 = 5.0;
const FAR_UNIT: f64 = 0.1;

/// The mesh's islands: its connected parts, those that share a vertex joined.
struct Islands {
    /// Per part (in [`crate::components::split`]'s order), its island.
    of_part: Vec<u32>,
    count: usize,
}

impl Islands {
    fn new(parts: &[crate::components::Component], triangles: &[[u32; 3]], vertices: usize) -> Self {
        let mut sets = UnionFind::new(parts.len());
        let mut owner = vec![u32::MAX; vertices];
        for (i, part) in parts.iter().enumerate() {
            for &f in &part.faces {
                for &v in triangles.get(f as usize).into_iter().flatten() {
                    let Some(slot) = owner.get_mut(v as usize) else { continue };
                    if *slot == u32::MAX {
                        *slot = i as u32;
                    } else {
                        sets.union(*slot, i as u32);
                    }
                }
            }
        }
        // Numbered by their lowest part.
        let mut id = vec![u32::MAX; parts.len()];
        let mut count = 0;
        let of_part = (0..parts.len() as u32)
            .map(|i| {
                let root = sets.find(i) as usize;
                if id[root] == u32::MAX {
                    id[root] = count as u32;
                    count += 1;
                }
                id[root]
            })
            .collect();
        Self { of_part, count }
    }
}

/// The surface an island's quads stand for.
struct Reference {
    surface: TriangleBvh,
    /// Its vertices and triangle centres.
    samples: Vec<DVec3>,
    area: f64,
}

impl Reference {
    fn new(positions: &[DVec3], triangles: &[[u32; 3]]) -> Self {
        let at = |v: u32| positions.get(v as usize).copied().unwrap_or(DVec3::ZERO);
        let mut vertices: Vec<u32> = triangles.iter().flatten().copied().collect();
        vertices.sort_unstable();
        vertices.dedup();
        let mut samples: Vec<DVec3> = vertices.into_iter().map(at).collect();
        let mut area = 0.0;
        for t in triangles {
            let [a, b, c] = t.map(at);
            samples.push((a + b + c) / 3.0);
            area += 0.5 * (b - a).cross(c - a).length();
        }
        Self { surface: TriangleBvh::new(positions, triangles), samples, area }
    }
}

/// One island's quads, cut out of a quad mesh.
#[derive(Clone, Debug, Default)]
struct Piece {
    mesh: PolyMesh,
    /// Per face, its patch in the layout the quads were built on.
    patch_of_face: Vec<u32>,
    feature_edges: Vec<[u32; 2]>,
}

/// Cuts `quads` into its islands' pieces: `island_of_span` gives the island
/// of each part of `trace`, the layout the quads were built on.
fn pieces(
    trace: &TraceOutput,
    quads: &QuadOutput,
    island_of_span: &[u32],
    islands: usize,
) -> Vec<Option<Piece>> {
    let mut island_of_patch = vec![u32::MAX; trace.patches.len()];
    for (span, &island) in trace.parts.iter().zip(island_of_span) {
        for slot in island_of_patch
            .get_mut(span.patches.start as usize..span.patches.end as usize)
            .into_iter()
            .flatten()
        {
            *slot = island;
        }
    }
    let mut out: Vec<Option<Piece>> = (0..islands).map(|_| None).collect();
    let mesh = &quads.mesh;
    let mut local = vec![u32::MAX; mesh.positions.len()];
    let mut island_of_vertex = vec![u32::MAX; mesh.positions.len()];
    for (f, w) in mesh.face_offsets.windows(2).enumerate() {
        let face = mesh.face_indices.get(w[0] as usize..w[1] as usize).unwrap_or_default();
        let Some(&patch) = quads.patch_of_face.get(f) else { continue };
        let Some(&island) = island_of_patch.get(patch as usize) else { continue };
        let Some(slot) = out.get_mut(island as usize) else { continue };
        let piece = slot.get_or_insert_with(|| Piece {
            mesh: PolyMesh { positions: Vec::new(), face_offsets: vec![0], face_indices: Vec::new() },
            ..Piece::default()
        });
        for &v in face {
            let (Some(id), Some(&p)) = (local.get_mut(v as usize), mesh.positions.get(v as usize)) else {
                continue;
            };
            if *id == u32::MAX {
                *id = piece.mesh.positions.len() as u32;
                piece.mesh.positions.push(p);
                island_of_vertex[v as usize] = island;
            }
            piece.mesh.face_indices.push(*id);
        }
        piece.mesh.face_offsets.push(piece.mesh.face_indices.len() as u32);
        piece.patch_of_face.push(patch);
    }
    for &[a, b] in &quads.feature_edges {
        let (Some(&ia), Some(&ib)) = (island_of_vertex.get(a as usize), island_of_vertex.get(b as usize))
        else {
            continue;
        };
        if ia == ib
            && let Some(Some(piece)) = out.get_mut(ia as usize)
        {
            piece.feature_edges.push([local[a as usize], local[b as usize]]);
        }
    }
    out
}

/// The farthest `piece` and `reference` lie from each other, either way.
fn farthest(piece: &PolyMesh, reference: &Reference) -> f64 {
    let faces = || {
        piece
            .face_offsets
            .windows(2)
            .map(|w| piece.face_indices.get(w[0] as usize..w[1] as usize).unwrap_or_default())
    };
    let at = |v: u32| piece.positions.get(v as usize).copied().unwrap_or(DVec3::ZERO);
    let mut samples = piece.positions.clone();
    samples.extend(
        faces().filter(|f| !f.is_empty()).map(|f| f.iter().map(|&v| at(v)).sum::<DVec3>() / f.len() as f64),
    );
    let triangles: Vec<[u32; 3]> =
        faces().flat_map(|f| (1..f.len().saturating_sub(1)).map(move |i| [f[0], f[i], f[i + 1]])).collect();
    let own = TriangleBvh::new(&piece.positions, &triangles);
    let most = |surface: &TriangleBvh, samples: &[DVec3]| {
        samples
            .par_iter()
            .with_min_len(256)
            .map(|&p| surface.closest_point(p).map_or(f64::INFINITY, |(_, d)| d))
            .reduce(|| 0.0, f64::max)
    };
    most(&reference.surface, &samples).max(most(&own, &reference.samples))
}

impl Score {
    /// `size` is the length of a quad's edge on this island.
    fn of(piece: &Piece, reference: &Reference, size: f64) -> Self {
        let quality = quad_quality(&piece.mesh);
        let jacobian = &quality.scaled_jacobian;
        let (Some(mean), Some(p05), Some(min)) = (jacobian.mean, jacobian.p05, jacobian.min) else {
            return Self { mean: 0.0, p05: 0.0, min: 0.0, poles: 0.0, far: 0.0, quads: 0 };
        };
        let poles = (quality.irregular_interior + quality.irregular_boundary) as f64 * 1000.0
            / quality.quads.max(1) as f64;
        let far = farthest(&piece.mesh, reference) / size.max(f64::MIN_POSITIVE);
        Self { mean, p05, min, poles, far, quads: quality.quads }
    }

    /// The sum; infinite (worse than any) for no quads at all.
    fn total(&self) -> f64 {
        let total = (1.0 - self.mean) / MEAN_UNIT
            + (1.0 - self.p05) / P05_UNIT
            + (1.0 - self.min) / MIN_UNIT
            + self.poles / POLES_UNIT
            + self.far / FAR_UNIT;
        if self.quads > 0 && total.is_finite() { total } else { f64::INFINITY }
    }
}

/// One more layout of the parts: those whose layout no earlier attempt
/// made, with their quads cut into pieces and scored.
struct Attempt {
    /// The layout of the islands tried, and each of its parts' place in the mesh's parts.
    trace: TraceOutput,
    parts: Vec<u32>,
    notes: Vec<PatchNote>,
    /// Per island (None: not tried in this attempt).
    pieces: Vec<Option<Piece>>,
    scores: Vec<Option<Score>>,
}

/// A failed attempt is one draw fewer, unless the run was cancelled.
fn failed<T>(e: Error) -> Result<Option<T>, Error> {
    if e.kind == ErrorKind::Cancelled { Err(e) } else { Ok(None) }
}

/// What [`improve`] did, for the report.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Tally {
    /// Layouts made, the first included.
    pub attempts: usize,
    /// Islands that got a layout the first attempt did not give them.
    pub retried: usize,
    pub improved: usize,
}

/// Lays the parts of `first` (the layout `quads` was built on) out again,
/// up to `attempts` layouts each in all, and returns each part's best quads
/// and, if any part changed, the layout they belong to.
///
/// Laying out is a small share of a run and building quads most of it, so
/// every attempt is laid out first and quads are built only for layouts
/// not seen before: a part with no near-ties to break (a flat panel, a box)
/// comes out the same from every seed and costs no more than its tracing.
///
/// Only a cancelled run is an error: a later attempt that fails is one draw
/// fewer.
pub(crate) fn improve(
    job: &Job<'_>,
    first: &TraceOutput,
    quads: QuadOutput,
    attempts: usize,
    control: &Control,
) -> Result<(Option<TraceOutput>, QuadOutput, Tally), Error> {
    improve_with(job, first, quads, attempts, control, &lowest)
}

/// The attempt with the lowest score, the first (0) unless a later one is
/// lower: `later[k]` is attempt `k + 1`'s, if it tried the island.
fn lowest(first: f64, later: &[Option<f64>]) -> usize {
    let (mut best, mut least) = (0, first);
    for (k, score) in later.iter().enumerate() {
        if let Some(score) = *score
            && score < least
        {
            (best, least) = (k + 1, score);
        }
    }
    best
}

/// [`improve`] with the rule that picks an island's attempt from the scores.
fn improve_with(
    job: &Job<'_>,
    first: &TraceOutput,
    quads: QuadOutput,
    attempts: usize,
    control: &Control,
    pick: &dyn Fn(f64, &[Option<f64>]) -> usize,
) -> Result<(Option<TraceOutput>, QuadOutput, Tally), Error> {
    let mut tally = Tally { attempts: 1, ..Tally::default() };
    let parts = crate::components::split(job.triangles, job.positions.len());
    if attempts < 2 || parts.is_empty() || parts.len() != first.parts.len() {
        return Ok((None, quads, tally));
    }
    let held = control.hold(Stage::Attempts);
    let steps = 3 * (attempts - 1);
    let done = AtomicUsize::new(0);
    let step =
        |count: usize| held.set((done.fetch_add(count, Ordering::Relaxed) + count) as f32 / steps as f32);

    // Every attempt's layout of every part.
    let faces: Vec<&[u32]> = parts.iter().map(|part| part.faces.as_slice()).collect();
    let layouts: Vec<Option<TraceOutput>> = (1..attempts)
        .into_par_iter()
        .map(|seed| {
            let layout = trace::compute_parts(
                job.positions,
                job.triangles,
                job.feature_edges,
                job.field,
                &faces,
                job.trace,
                seed as u64,
                control,
            );
            step(1);
            match layout {
                Ok(layout) if layout.parts.len() == parts.len() => Ok(Some(layout)),
                Ok(_) => Ok(None),
                Err(e) => failed(e),
            }
        })
        .collect::<Result<_, Error>>()?;
    control.checkpoint(Stage::Attempts)?;
    tally.attempts += layouts.iter().flatten().count();

    // Which of them are new: an island's layout that no earlier attempt made.
    let islands = Islands::new(&parts, job.triangles, job.positions.len());
    let mut parts_of: Vec<Vec<u32>> = vec![Vec::new(); islands.count];
    for (p, &island) in islands.of_part.iter().enumerate() {
        parts_of[island as usize].push(p as u32);
    }
    let all: Vec<Option<&TraceOutput>> =
        std::iter::once(Some(first)).chain(layouts.iter().map(Option::as_ref)).collect();
    let new: Vec<Vec<bool>> = (1..attempts)
        .into_par_iter()
        .map(|k| {
            let Some(layout) = all[k] else { return vec![false; islands.count] };
            parts_of
                .iter()
                .map(|parts| {
                    !all[..k].iter().flatten().any(|earlier| {
                        parts.iter().all(|&p| trace::same_part(layout, p as usize, earlier, p as usize))
                    })
                })
                .collect()
        })
        .collect();
    let retried: Vec<bool> = (0..islands.count).map(|i| new.iter().any(|n| n[i])).collect();
    tally.retried = retried.iter().filter(|&&r| r).count();
    if tally.retried == 0 {
        return Ok((None, quads, tally));
    }

    // The first attempt's quads, by island, and what they are measured against.
    let mut triangles_of: Vec<Vec<[u32; 3]>> = vec![Vec::new(); islands.count];
    for (part, &island) in parts.iter().zip(&islands.of_part) {
        if retried[island as usize] {
            triangles_of[island as usize]
                .extend(part.faces.iter().filter_map(|&f| job.triangles.get(f as usize).copied()));
        }
    }
    let references: Vec<Option<Reference>> = triangles_of
        .par_iter()
        .zip(&retried)
        .map(|(triangles, &retried)| retried.then(|| Reference::new(job.positions, triangles)))
        .collect();
    drop(triangles_of);
    let first_pieces = pieces(first, &quads, &islands.of_part, islands.count);
    // A quad's size on each island, from the first quads: the unit of its distances.
    let size: Vec<f64> = first_pieces
        .iter()
        .zip(&references)
        .map(|(piece, reference)| {
            let count = piece.as_ref().map_or(0, |p| p.patch_of_face.len());
            match reference {
                Some(reference) if count > 0 && reference.area > 0.0 => {
                    (reference.area / count as f64).sqrt()
                }
                _ => job.edge,
            }
        })
        .collect();
    let rate = |piece: &Option<Piece>, i: usize| match (piece, &references[i]) {
        (Some(piece), Some(reference)) => Some(Score::of(piece, reference, size[i])),
        _ => None,
    };
    let first_scores: Vec<Option<Score>> =
        first_pieces.par_iter().enumerate().map(|(i, piece)| rate(piece, i)).collect();
    control.checkpoint(Stage::Attempts)?;

    // The new layouts' quads, a few attempts side by side.
    let build = |(layout, new): (Option<TraceOutput>, &Vec<bool>)| -> Result<Option<Attempt>, Error> {
        let tried: Vec<u32> =
            (0..parts.len() as u32).filter(|&p| new[islands.of_part[p as usize] as usize]).collect();
        let Some(layout) = layout.filter(|_| !tried.is_empty()) else {
            step(2);
            return Ok(None);
        };
        let picks: Vec<(usize, usize)> = tried.iter().map(|&p| (0, p as usize)).collect();
        let trace = trace::compose(job.positions.len(), &[&layout], &picks);
        drop(layout);
        let wanted: usize = first_pieces
            .iter()
            .zip(new)
            .filter(|&(_, &new)| new)
            .map(|(piece, _)| piece.as_ref().map_or(0, |p| p.patch_of_face.len()))
            .sum();
        let built = match (job.build)(&trace, wanted) {
            Ok(built) => built,
            Err(e) => {
                step(2);
                return failed(e);
            }
        };
        step(1);
        let island_of_span: Vec<u32> = tried.iter().map(|&p| islands.of_part[p as usize]).collect();
        let pieces = pieces(&trace, &built, &island_of_span, islands.count);
        let scores = pieces.par_iter().enumerate().map(|(i, piece)| rate(piece, i)).collect();
        step(1);
        Ok(Some(Attempt { trace, parts: tried, notes: built.patches, pieces, scores }))
    };
    let mut later: Vec<Option<Attempt>> = Vec::with_capacity(attempts - 1);
    let mut waiting = layouts.into_iter().zip(&new);
    loop {
        let round: Vec<(Option<TraceOutput>, &Vec<bool>)> = waiting.by_ref().take(SIDE_BY_SIDE).collect();
        if round.is_empty() {
            break;
        }
        later.extend(round.into_par_iter().map(build).collect::<Result<Vec<_>, Error>>()?);
        control.checkpoint(Stage::Attempts)?;
    }

    // Each island's best: the first attempt unless a later one scored lower.
    let mut winner = vec![0usize; islands.count];
    for (i, best) in winner.iter_mut().enumerate() {
        let Some(first) = first_scores[i].map(|s| s.total()) else { continue };
        let others: Vec<Option<f64>> = later
            .iter()
            .map(|attempt| Some(attempt.as_ref()?.scores.get(i).copied().flatten()?.total()))
            .collect();
        *best = pick(first, &others).min(others.len());
        // An attempt that didn't try the island can't be its pick.
        if *best > 0 && others[*best - 1].is_none() {
            *best = 0;
        }
    }
    tally.improved = winner.iter().filter(|&&w| w != 0).count();
    let total: usize = winner
        .iter()
        .enumerate()
        .map(|(i, &source)| {
            let piece = match source.checked_sub(1) {
                Some(k) => later.get(k).and_then(Option::as_ref).and_then(|a| a.pieces.get(i)?.as_ref()),
                None => first_pieces.get(i).and_then(Option::as_ref),
            };
            piece.map_or(0, |p| p.patch_of_face.len())
        })
        .sum();
    if tally.improved == 0 || !(job.fits)(total) {
        tally.improved = 0;
        return Ok((None, quads, tally));
    }

    let mut first_pieces = first_pieces;
    let mut later_pieces: Vec<Vec<Option<Piece>>> = later
        .iter_mut()
        .map(|attempt| attempt.as_mut().map(|a| std::mem::take(&mut a.pieces)).unwrap_or_default())
        .collect();
    // The layout: each part from its island's winner. (Source 0 is the first
    // layout, with every part; source k is attempt k's, with the parts it tried.)
    let mut sources: Vec<&TraceOutput> = vec![first];
    sources.extend(later.iter().map(|attempt| attempt.as_ref().map_or(first, |a| &a.trace)));
    let part_of_span = |source: usize, span: usize| -> usize {
        match source.checked_sub(1).and_then(|k| later.get(k)).and_then(Option::as_ref) {
            Some(attempt) => attempt.parts.get(span).copied().unwrap_or(0) as usize,
            None => span,
        }
    };
    let picks: Vec<(usize, usize)> = (0..parts.len())
        .map(|p| {
            let source = winner[islands.of_part[p] as usize];
            let span = match source.checked_sub(1).and_then(|k| later.get(k)).and_then(Option::as_ref) {
                Some(attempt) => attempt.parts.binary_search(&(p as u32)).unwrap_or(0),
                None => p,
            };
            (source, span)
        })
        .collect();
    let layout = trace::compose(job.positions.len(), &sources, &picks);

    // The quads: each island's winning piece, its patches renumbered to the composed layout's.
    // (A part's patches are one run in every layout, so a patch moves by its part's offset.)
    let shift: Vec<Vec<(u32, i64)>> = sources
        .iter()
        .enumerate()
        .map(|(s, source)| {
            source
                .parts
                .iter()
                .enumerate()
                .map(|(j, span)| {
                    let to = layout.parts.get(part_of_span(s, j)).map_or(0, |span| span.patches.start);
                    (span.patches.start, i64::from(to) - i64::from(span.patches.start))
                })
                .collect()
        })
        .collect();
    let moved = |source: usize, patch: u32| -> u32 {
        let runs = &shift[source];
        let at = runs.partition_point(|&(start, _)| start <= patch).saturating_sub(1);
        runs.get(at).map_or(patch, |&(_, by)| (i64::from(patch) + by) as u32)
    };
    let mut out = QuadOutput {
        mesh: PolyMesh { positions: Vec::new(), face_offsets: vec![0], face_indices: Vec::new() },
        patches: vec![PatchNote::default(); layout.patches.len()],
        report: quads.report.clone(),
        ..QuadOutput::default()
    };
    for (i, &source) in winner.iter().enumerate() {
        let piece = match source {
            0 => first_pieces.get_mut(i).and_then(Option::take),
            k => later_pieces.get_mut(k - 1).and_then(|pieces| pieces.get_mut(i)?.take()),
        };
        let Some(piece) = piece else { continue };
        let base = out.mesh.positions.len() as u32;
        out.mesh.positions.extend_from_slice(&piece.mesh.positions);
        let offset = out.mesh.face_indices.len() as u32;
        out.mesh.face_indices.extend(piece.mesh.face_indices.iter().map(|&v| v + base));
        out.mesh.face_offsets.extend(piece.mesh.face_offsets.iter().skip(1).map(|&o| o + offset));
        out.patch_of_face.extend(piece.patch_of_face.iter().map(|&p| moved(source, p)));
        out.feature_edges.extend(piece.feature_edges.iter().map(|&[a, b]| [a + base, b + base]));
    }
    for (s, source) in sources.iter().enumerate() {
        let notes = match s.checked_sub(1).and_then(|k| later.get(k)).and_then(Option::as_ref) {
            Some(attempt) => &attempt.notes,
            None if s == 0 => &quads.patches,
            None => continue,
        };
        for (j, span) in source.parts.iter().enumerate() {
            if winner[islands.of_part[part_of_span(s, j)] as usize] != s {
                continue;
            }
            for patch in span.patches.clone() {
                if let (Some(to), Some(&note)) =
                    (out.patches.get_mut(moved(s, patch) as usize), notes.get(patch as usize))
                {
                    *to = note;
                }
            }
        }
    }
    let notes = std::mem::take(&mut out.patches);
    out.report.tally(&notes);
    out.patches = notes;
    held.set(1.0);
    Ok((Some(layout), out, tally))
}

#[cfg(test)]
mod tests {
    use hashbrown::HashMap;

    use super::*;
    use crate::field::{FieldParams, compute as field_compute};
    use crate::quad::{self, QuadParams, Sizing};
    use crate::testmesh::{cube, grid, torus};

    /// One mesh and what a single attempt makes of it.
    struct Made {
        positions: Vec<DVec3>,
        triangles: Vec<[u32; 3]>,
        features: Vec<[u32; 2]>,
        field: FieldOutput,
        layout: TraceOutput,
        quads: QuadOutput,
    }

    const EDGE: f64 = 0.12;

    fn build(layout: &TraceOutput, _wanted: usize) -> Result<QuadOutput, Error> {
        quad::compute(layout, Sizing::default(), EDGE, &QuadParams::default(), &Control::default())
    }

    fn made(positions: Vec<DVec3>, triangles: Vec<[u32; 3]>, features: Vec<[u32; 2]>) -> Made {
        let control = Control::default();
        let field =
            field_compute(&positions, &triangles, &features, &FieldParams::default(), &control).unwrap();
        let layout =
            trace::compute(&positions, &triangles, &features, &field, &TraceParams::default(), &control)
                .unwrap();
        let quads = build(&layout, 0).unwrap();
        Made { positions, triangles, features, field, layout, quads }
    }

    impl Made {
        fn job<'a>(
            &'a self,
            trace: &'a TraceParams,
            build: &'a (dyn Fn(&TraceOutput, usize) -> Result<QuadOutput, Error> + Sync),
        ) -> Job<'a> {
            Job {
                positions: &self.positions,
                triangles: &self.triangles,
                feature_edges: &self.features,
                field: &self.field,
                trace,
                build,
                fits: &|_| true,
                edge: EDGE,
            }
        }
    }

    /// Two tori side by side.
    fn two_tori() -> (Vec<DVec3>, Vec<[u32; 3]>) {
        let (mut p, mut t) = torus(24, 12);
        let base = p.len() as u32;
        let (q, u) = torus(20, 12);
        p.extend(q.iter().map(|&v| v + DVec3::new(4.0, 0.0, 0.0)));
        t.extend(u.iter().map(|tri| tri.map(|v| v + base)));
        (p, t)
    }

    /// How many faces use each edge.
    fn edge_uses(mesh: &PolyMesh) -> HashMap<(u32, u32), usize> {
        let mut uses = HashMap::new();
        for w in mesh.face_offsets.windows(2) {
            let face = &mesh.face_indices[w[0] as usize..w[1] as usize];
            for i in 0..face.len() {
                let (a, b) = (face[i], face[(i + 1) % face.len()]);
                *uses.entry((a.min(b), a.max(b))).or_default() += 1;
            }
        }
        uses
    }

    /// A 6 by 6 grid of unit quads, sheared and lifted off the plane it is measured against.
    fn square_piece(shear: f64, lift: f64) -> (Piece, Reference) {
        let (positions, triangles) = grid(6, 6);
        let reference = Reference::new(&positions, &triangles);
        let mut mesh = PolyMesh {
            positions: positions.iter().map(|p| DVec3::new(p.x + shear * p.y, p.y, lift)).collect(),
            face_offsets: vec![0],
            face_indices: Vec::new(),
        };
        for y in 0..6u32 {
            for x in 0..6u32 {
                let a = y * 7 + x;
                mesh.face_indices.extend([a, a + 1, a + 8, a + 7]);
                mesh.face_offsets.push(mesh.face_indices.len() as u32);
            }
        }
        let faces = mesh.face_offsets.len() - 1;
        (Piece { mesh, patch_of_face: vec![0; faces], feature_edges: Vec::new() }, reference)
    }

    /// Whatever the scores, the last attempt that tried the island.
    fn last(_: f64, later: &[Option<f64>]) -> usize {
        later.iter().rposition(Option::is_some).map_or(0, |k| k + 1)
    }

    #[test]
    fn parts_that_touch_at_a_vertex_are_one_island() {
        // Two triangles sharing vertex 2, and a third apart.
        let triangles = [[0, 1, 2], [2, 3, 4], [5, 6, 7]];
        let parts = crate::components::split(&triangles, 8);
        assert_eq!(parts.len(), 3);
        let islands = Islands::new(&parts, &triangles, 8);
        assert_eq!(islands.count, 2);
        assert_eq!(islands.of_part, [0, 0, 1]);
    }

    #[test]
    fn squarer_quads_and_quads_on_the_surface_score_lower() {
        let (square, reference) = square_piece(0.0, 0.0);
        let (sheared, _) = square_piece(0.4, 0.0);
        let (lifted, _) = square_piece(0.0, 0.3);
        let score = |piece: &Piece| Score::of(piece, &reference, 1.0);
        let best = score(&square);
        assert!(best.total() < 1e-9, "a grid of squares on its surface has nothing against it: {best:?}");
        assert!(score(&sheared).total() > best.total() + 1.0, "{:?}", score(&sheared));
        // Lifted 0.3 of a quad off the surface: three units of distance.
        let far = score(&lifted);
        assert!((far.far - 0.3).abs() < 1e-9 && (far.total() - 3.0).abs() < 1e-6, "{far:?}");
        // A piece with no quads is worse than any.
        assert!(Score::of(&Piece::default(), &reference, 1.0).total().is_infinite());
    }

    #[test]
    fn the_lowest_score_wins_and_the_first_attempt_wins_ties() {
        assert_eq!(lowest(5.0, &[Some(6.0), None, Some(4.0)]), 3);
        assert_eq!(lowest(5.0, &[Some(5.0), Some(5.0)]), 0);
        assert_eq!(lowest(5.0, &[Some(4.0), Some(4.0)]), 1);
        assert_eq!(lowest(5.0, &[]), 0);
        assert_eq!(lowest(f64::INFINITY, &[None, Some(9.0)]), 2);
    }

    #[test]
    fn a_quad_mesh_is_cut_into_its_islands_pieces() {
        let (p, t) = two_tori();
        let made = made(p, t, Vec::new());
        assert_eq!(made.layout.parts.len(), 2);
        let cut = pieces(&made.layout, &made.quads, &[0, 1], 2);
        let cut: Vec<&Piece> =
            cut.iter().map(|piece| piece.as_ref().expect("both islands have quads")).collect();
        let faces = |piece: &Piece| piece.mesh.face_offsets.len() - 1;
        assert_eq!(faces(cut[0]) + faces(cut[1]), made.quads.mesh.face_offsets.len() - 1);
        assert_eq!(
            cut[0].mesh.positions.len() + cut[1].mesh.positions.len(),
            made.quads.mesh.positions.len()
        );
        for piece in cut {
            assert_eq!(piece.patch_of_face.len(), faces(piece));
            // Each is a closed surface of its own.
            assert!(edge_uses(&piece.mesh).values().all(|&n| n == 2));
        }
    }

    #[test]
    fn the_pieces_of_other_attempts_join_into_one_mesh() {
        let (p, t) = two_tori();
        let made = made(p, t, Vec::new());
        let trace = TraceParams::default();
        let (layout, quads, tally) = improve_with(
            &made.job(&trace, &build),
            &made.layout,
            made.quads.clone(),
            4,
            &Control::default(),
            &last,
        )
        .unwrap();
        assert_eq!(tally.attempts, 4);
        assert!(tally.retried > 0 && tally.improved == tally.retried, "{tally:?}");
        let layout = layout.expect("a part took another layout");
        assert_eq!(layout.parts.len(), 2);
        assert_eq!(layout.patch_of_face.len(), layout.mesh.triangles.len());
        assert!(layout.patch_of_face.iter().all(|&p| (p as usize) < layout.patches.len()));
        let faces = quads.mesh.face_offsets.len() - 1;
        assert_eq!(quads.patch_of_face.len(), faces);
        assert!(quads.patch_of_face.iter().all(|&p| (p as usize) < layout.patches.len()));
        assert_eq!(quads.patches.len(), layout.patches.len());
        assert_eq!(quads.patches.iter().map(|note| note.quads as usize).sum::<usize>(), faces);
        assert_eq!(quads.report.quads, faces);
        // Every patch has as many faces as its note says.
        let mut used = vec![0usize; layout.patches.len()];
        for &p in &quads.patch_of_face {
            used[p as usize] += 1;
        }
        assert!(used.iter().zip(&quads.patches).all(|(&n, note)| n == note.quads as usize));
        // Still two closed surfaces of quads.
        assert!(quads.mesh.face_offsets.windows(2).all(|w| w[1] - w[0] == 4));
        assert!(edge_uses(&quads.mesh).values().all(|&n| n == 2));
        assert!(quads.mesh.face_indices.iter().all(|&v| (v as usize) < quads.mesh.positions.len()));
    }

    #[test]
    fn a_part_every_seed_lays_out_alike_is_left_alone() {
        // A cube's six faces are its layout whatever the seed.
        let (p, t, f) = cube(6);
        let made = made(p, t, f);
        let trace = TraceParams::default();
        let (layout, quads, tally) =
            improve(&made.job(&trace, &build), &made.layout, made.quads.clone(), 3, &Control::default())
                .unwrap();
        assert!(layout.is_none());
        assert_eq!(quads, made.quads);
        assert_eq!((tally.attempts, tally.retried, tally.improved), (3, 0, 0));
    }

    #[test]
    fn an_attempt_that_fails_is_one_draw_fewer_and_a_cancelled_one_ends_the_run() {
        let (p, t) = two_tori();
        let made = made(p, t, Vec::new());
        let trace = TraceParams::default();
        let fails = |_: &TraceOutput, _: usize| Err(Error::internal(Stage::Patterns, "no quads"));
        let (layout, quads, tally) =
            improve(&made.job(&trace, &fails), &made.layout, made.quads.clone(), 3, &Control::default())
                .unwrap();
        assert!(layout.is_none());
        assert_eq!(quads, made.quads);
        assert_eq!(tally.improved, 0);
        let cancelled = |_: &TraceOutput, _: usize| Err(Error::cancelled(Stage::Quantize));
        let control = Control::default();
        let error = improve(&made.job(&trace, &cancelled), &made.layout, made.quads.clone(), 3, &control)
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Cancelled);
        // Progress is handed back either way.
        control.set_progress(Stage::Output, 0.5);
        assert_eq!(control.progress(), (Stage::Output, 0.5));
    }

    #[test]
    fn a_result_that_misses_the_count_asked_for_keeps_the_first() {
        let (p, t) = two_tori();
        let made = made(p, t, Vec::new());
        let trace = TraceParams::default();
        let mut job = made.job(&trace, &build);
        job.fits = &|_| false;
        let (layout, quads, tally) =
            improve_with(&job, &made.layout, made.quads.clone(), 3, &Control::default(), &last).unwrap();
        assert!(layout.is_none());
        assert_eq!(quads, made.quads);
        assert_eq!(tally.improved, 0);
    }
}
