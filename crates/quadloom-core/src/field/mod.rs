//! Stage 3: a smooth per-face cross field (4-RoSy) on the prep mesh.
//!
//! Complex power representation (Knöppel, Crane, Pinkall & Schröder,
//! *Globally Optimal Direction Fields*, SIGGRAPH 2013) in dual, per-face form:
//! each face stores `z = e^{4iθ}` in its own frame, so the four directions of
//! a cross share one value, and smoothness is a quadratic energy over the
//! faces' shared edges. See `docs/superpowers/plans/2026-10-23-m4-cross-field.md`.

pub mod constraints;
pub mod frames;
pub mod singular;
pub mod solve;

pub use constraints::{curvature_alignment, hard_constraints, sufficient_features};
pub use frames::{Cx, FaceFrames};
pub use singular::vertex_indices;
pub use solve::{Field, Hard, ROSY, Soft, smooth_field};

use glam::DVec3;
use rayon::prelude::*;

use crate::components;
use crate::control::{Control, Stage};
use crate::error::Error;

/// When principal curvature guides the field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum CurvatureAlignment {
    /// When features are too sparse to guide the field (the oracle's rule:
    /// feature length ≤ `sharp_factor` × √area).
    Auto,
    On,
    Off,
}

impl CurvatureAlignment {
    pub fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::On => "on",
            Self::Off => "off",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        [Self::Auto, Self::On, Self::Off].into_iter().find(|m| m.name() == name)
    }
}

/// Stage 3 settings. Defaults follow the oracle.
#[derive(Clone, Debug, PartialEq)]
pub struct FieldParams {
    pub enabled: bool,
    pub curvature_alignment: CurvatureAlignment,
    /// Strength α of curvature alignment against smoothness (the oracle's
    /// `alpha_curv`).
    pub alignment_weight: f64,
    /// The oracle's `SharpFactor` for [`CurvatureAlignment::Auto`].
    pub sharp_factor: f64,
}

impl Default for FieldParams {
    fn default() -> Self {
        Self {
            enabled: true,
            curvature_alignment: CurvatureAlignment::Auto,
            alignment_weight: 0.02,
            sharp_factor: 6.0,
        }
    }
}

/// Stage 3 totals.
#[derive(Clone, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct FieldReport {
    /// Vertices with a non-zero index.
    pub singularities: usize,
    /// Sum of positive indices, in quarters.
    pub positive_quarters: i64,
    /// Sum of negative indices, in quarters (≤ 0).
    pub negative_quarters: i64,
    pub hard_faces: usize,
    pub curvature_aligned: bool,
    /// Parts whose solve failed and got the propagated constraint field instead.
    pub fallback_parts: usize,
}

/// The cross field of a mesh.
#[derive(Clone, Debug, Default)]
pub struct FieldOutput {
    /// One unit direction per triangle (one of its cross's four).
    pub directions: Vec<DVec3>,
    /// `(vertex, index in quarters)` for every singular vertex, by vertex id
    /// (a vertex shared by parts that only touch there sums their indices).
    pub singularities: Vec<(u32, i32)>,
    pub report: FieldReport,
}

/// Computes the cross field of `triangles`, each connected part on its own
/// (in parallel), aligned to `feature_edges` and borders, and to curvature
/// per [`FieldParams::curvature_alignment`].
pub fn compute(
    positions: &[DVec3],
    triangles: &[[u32; 3]],
    feature_edges: &[[u32; 2]],
    params: &FieldParams,
    control: &Control,
) -> Result<FieldOutput, Error> {
    let stage = Stage::Field;
    control.checkpoint(stage)?;
    let area: f64 = triangles
        .iter()
        .map(|t| {
            let [a, b, c] = t.map(|v| positions[v as usize]);
            0.5 * (b - a).cross(c - a).length()
        })
        .sum();
    let feature_length: f64 =
        feature_edges.iter().map(|&[a, b]| (positions[a as usize] - positions[b as usize]).length()).sum();
    let aligned = match params.curvature_alignment {
        CurvatureAlignment::On => true,
        CurvatureAlignment::Off => false,
        CurvatureAlignment::Auto => !sufficient_features(feature_length, area, params.sharp_factor),
    };
    let parts = components::split(triangles, positions.len());
    // Each feature edge, once, under the part its first vertex belongs to.
    let mut part_of_vertex = vec![usize::MAX; positions.len()];
    for (k, part) in parts.iter().enumerate() {
        for &f in &part.faces {
            for v in triangles[f as usize] {
                part_of_vertex[v as usize] = k;
            }
        }
    }
    let mut part_features: Vec<Vec<[u32; 2]>> = vec![Vec::new(); parts.len()];
    for &[a, b] in feature_edges {
        if let Some(bucket) = part_features.get_mut(part_of_vertex[a as usize]) {
            bucket.push([a, b]);
        }
    }
    let done = std::sync::atomic::AtomicUsize::new(0);
    let results: Vec<Result<PartField, Error>> = parts
        .par_iter()
        .zip(&part_features)
        .map(|(part, features)| {
            control.checkpoint(stage)?;
            let field = part_field(
                positions,
                triangles,
                features,
                &part.faces,
                aligned,
                params.alignment_weight,
                control,
            )?;
            let finished = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            control.set_progress(stage, finished as f32 / parts.len().max(1) as f32);
            Ok(field)
        })
        .collect();
    let mut output = FieldOutput { directions: vec![DVec3::ZERO; triangles.len()], ..FieldOutput::default() };
    output.report.curvature_aligned = aligned;
    for (part, result) in parts.iter().zip(results) {
        let field = result?;
        for (&f, d) in part.faces.iter().zip(&field.directions) {
            output.directions[f as usize] = *d;
        }
        output.singularities.extend(field.singularities);
        output.report.hard_faces += field.hard_faces;
        output.report.fallback_parts += usize::from(field.fallback);
    }
    output.singularities.sort_unstable();
    // A vertex where parts only touch appears once per part: merge.
    let mut merged: Vec<(u32, i32)> = Vec::with_capacity(output.singularities.len());
    for (v, k) in output.singularities.drain(..) {
        match merged.last_mut() {
            Some(last) if last.0 == v => last.1 += k,
            _ => merged.push((v, k)),
        }
    }
    merged.retain(|&(_, k)| k != 0);
    output.singularities = merged;
    for &(_, k) in &output.singularities {
        if k > 0 {
            output.report.positive_quarters += i64::from(k);
        } else {
            output.report.negative_quarters += i64::from(k);
        }
    }
    output.report.singularities = output.singularities.len();
    control.set_progress(stage, 1.0);
    Ok(output)
}

struct PartField {
    directions: Vec<DVec3>,
    singularities: Vec<(u32, i32)>,
    hard_faces: usize,
    fallback: bool,
}

/// One connected part: its own vertex numbering, solve, and singularities.
/// A failed solve falls back to the propagated field; cancellation is passed on.
fn part_field(
    positions: &[DVec3],
    triangles: &[[u32; 3]],
    feature_edges: &[[u32; 2]],
    faces: &[u32],
    aligned: bool,
    alignment_weight: f64,
    control: &Control,
) -> Result<PartField, Error> {
    let mut vertices: Vec<u32> = faces.iter().flat_map(|&f| triangles[f as usize]).collect();
    vertices.sort_unstable();
    vertices.dedup();
    let local = |v: u32| vertices.binary_search(&v).ok().map(|i| i as u32);
    let part_positions: Vec<DVec3> = vertices.iter().map(|&v| positions[v as usize]).collect();
    let part_triangles: Vec<[u32; 3]> =
        faces.iter().map(|&f| triangles[f as usize].map(|v| local(v).unwrap_or(0))).collect();
    let part_features: Vec<[u32; 2]> =
        feature_edges.iter().filter_map(|&[a, b]| Some([local(a)?, local(b)?])).collect();
    let hard = hard_constraints(&part_positions, &part_triangles, &part_features);
    let soft = if aligned { curvature_alignment(&part_positions, &part_triangles) } else { Vec::new() };
    let (directions, powers, fallback) =
        match smooth_field(&part_positions, &part_triangles, &hard, &soft, alignment_weight, control) {
            Ok(field) => (field.directions, field.powers, false),
            Err(e) if e.kind == crate::error::ErrorKind::Cancelled => return Err(e),
            Err(_) => {
                let (d, z) = propagated(&part_positions, &part_triangles, &hard);
                (d, z, true)
            }
        };
    let singularities = vertex_indices(&part_positions, &part_triangles, &powers)
        .into_iter()
        .enumerate()
        .filter(|&(_, k)| k != 0)
        .map(|(v, k)| (vertices[v], k))
        .collect();
    Ok(PartField { directions, singularities, hard_faces: hard.len(), fallback })
}

/// The recovery field when a solve fails: constraint directions spread
/// breadth-first across faces, projected into each face's plane; faces no
/// constraint reaches use their frame's first axis.
fn propagated(positions: &[DVec3], triangles: &[[u32; 3]], hard: &[Hard]) -> (Vec<DVec3>, Vec<Cx>) {
    let frames = FaceFrames::new(positions, triangles);
    let n = triangles.len();
    let mut neighbours: Vec<Vec<u32>> = vec![Vec::new(); n];
    for e in frames::dual_edges(triangles) {
        neighbours[e.f as usize].push(e.g);
        neighbours[e.g as usize].push(e.f);
    }
    let mut directions: Vec<Option<DVec3>> = vec![None; n];
    let mut queue = std::collections::VecDeque::new();
    for h in hard {
        if directions[h.face as usize].is_none() {
            directions[h.face as usize] = Some(h.direction);
            queue.push_back(h.face);
        }
    }
    while let Some(f) = queue.pop_front() {
        let d = directions[f as usize].unwrap_or(DVec3::ZERO);
        for &g in &neighbours[f as usize] {
            if directions[g as usize].is_none() {
                let normal = frames.normal[g as usize];
                directions[g as usize] = Some((d - normal * d.dot(normal)).normalize_or_zero());
                queue.push_back(g);
            }
        }
    }
    let directions: Vec<DVec3> =
        (0..n).map(|f| directions[f].filter(|d| *d != DVec3::ZERO).unwrap_or(frames.e1[f])).collect();
    let powers = (0..n as u32).map(|f| frames.power(f, directions[f as usize], ROSY)).collect();
    (directions, powers)
}
