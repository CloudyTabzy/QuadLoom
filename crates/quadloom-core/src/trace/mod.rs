//! Stage 4: field tracing and patch layout.
//!
//! The prep mesh is cut along its feature lines, and field-aligned paths are
//! traced on a graph of (vertex, field direction) nodes until every patch is
//! a disk with 3 to `max_sides` convex corners (Pietroni et al., *Reliable
//! Feature-Line Driven Quad-Remeshing*, SIGGRAPH 2021, §5). See
//! `docs/superpowers/plans/2026-10-24-m5-trace-layout.md`.

pub mod cut;
pub mod graph;
pub mod patches;
pub mod refine;
pub mod sample;
pub mod search;
pub mod select;
pub mod stats;
pub mod turns;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};

use glam::DVec3;
use rayon::prelude::*;

pub use patches::PatchKind;
pub use stats::{LayoutStats, layout_stats};

use crate::control::{Control, Stage};
use crate::error::Error;
use crate::field::{FaceFrames, FieldOutput, ROSY, vertex_indices};

/// How a path's cost grows as it drifts from the field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum TraceCost {
    /// Anisotropic length √((d·f)² + drift²·(d·f⊥)²) of each arc's net
    /// displacement d against the field f (Campen, Heistermann & Kobbelt,
    /// *Practical Anisotropic Geodesy*, 2013).
    Anisotropic,
    /// The oracle's ℓ·(1 + drift·(α/45°)²), α the arc's angle to the field.
    AnglePenalty,
}

impl TraceCost {
    pub fn name(self) -> &'static str {
        match self {
            Self::Anisotropic => "anisotropic",
            Self::AnglePenalty => "angle_penalty",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        [Self::Anisotropic, Self::AnglePenalty].into_iter().find(|c| c.name() == name)
    }
}

/// The prep mesh and its field, as tracing sees them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TraceMesh {
    pub positions: Vec<DVec3>,
    pub triangles: Vec<[u32; 3]>,
    /// Sorted vertex pairs.
    pub feature_edges: Vec<[u32; 2]>,
    /// One unit direction per triangle.
    pub directions: Vec<DVec3>,
    /// `(vertex, index in quarters)`, by vertex id.
    pub singularities: Vec<(u32, i32)>,
}

/// Stage 4 settings. Defaults follow the oracle where it has one.
#[derive(Clone, Debug, PartialEq)]
pub struct TraceParams {
    pub enabled: bool,
    /// Most sides a patch may have (the oracle's `MaxVal`).
    pub max_sides: usize,
    pub cost: TraceCost,
    /// How strongly the cost punishes drifting from the field.
    pub drift: f64,
    /// CC-ability tolerance, in mean edge lengths (the oracle's `CClarkability`).
    pub cc_tolerance: f64,
    /// Patches should hold no singularity (4 sides) or one matching their sides.
    pub match_valence: bool,
    /// Loop seeds per vertex (the oracle's `sample_ratio`).
    pub loop_density: f64,
    /// Passes of tracing inside failing patches.
    pub sub_passes: usize,
}

impl Default for TraceParams {
    fn default() -> Self {
        Self {
            enabled: true,
            max_sides: 5,
            cost: TraceCost::Anisotropic,
            drift: 10.0,
            cc_tolerance: 1.0,
            match_valence: true,
            loop_density: 0.01,
            sub_passes: 3,
        }
    }
}

impl TraceParams {
    fn settings(&self) -> select::Settings {
        select::Settings {
            max_sides: self.max_sides,
            match_valence: self.match_valence,
            cc_tolerance: self.cc_tolerance,
            loop_density: self.loop_density,
            sub_passes: self.sub_passes,
            cost: self.cost,
            drift: self.drift,
            cut_ears: true,
        }
    }
}

/// One side of a patch.
#[derive(Clone, Debug, PartialEq)]
pub struct Side {
    /// Corner to corner, inclusive (mesh vertex ids).
    pub vertices: Vec<u32>,
    /// Length along the field.
    pub length: f64,
}

/// One patch of the layout.
#[derive(Clone, Debug, PartialEq)]
pub struct Patch {
    /// Corners, counter-clockwise (mesh vertex ids).
    pub corners: Vec<u32>,
    pub sides: Vec<Side>,
    /// Singular vertices inside, `(vertex, quarters)`.
    pub singularities: Vec<(u32, i32)>,
    pub kind: PatchKind,
    pub cc_margin: f64,
}

/// Stage 4 totals.
#[derive(Clone, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct TraceReport {
    pub parts: usize,
    pub patches: usize,
    /// Patches by side count (index 8: 8 or more).
    pub sides_histogram: [usize; 9],
    pub non_disk: usize,
    pub not_convex: usize,
    pub low_sides: usize,
    pub high_sides: usize,
    pub not_cc_able: usize,
    pub valence_mismatch: usize,
    pub ok: usize,
    /// Candidate paths traced.
    pub candidates: usize,
    /// Paths in the layout (feature lines excluded).
    pub kept_paths: usize,
    /// Paths dissolved by the clean-up.
    pub removed_paths: usize,
    pub sub_passes_run: usize,
    /// Patches retraced with every border node receiving.
    pub relaxed_patches: usize,
    /// Non-field-aligned paths added to make a patch a disk.
    pub seams: usize,
    /// Patches that are still invalid (non-disk, not convex, or a side count out of range).
    pub unsolved: usize,
}

/// The layout of a mesh.
#[derive(Clone, Debug, Default)]
pub struct TraceOutput {
    /// The refined mesh the layout lives on.
    pub mesh: TraceMesh,
    /// One patch id per triangle of `mesh`.
    pub patch_of_face: Vec<u32>,
    pub patches: Vec<Patch>,
    /// Each traced path as a chain of `mesh` vertices (a loop repeats its first vertex).
    pub paths: Vec<Vec<u32>>,
    pub report: TraceReport,
}

/// Lays out `triangles` (each connected part on its own, in parallel): cuts
/// along `feature_edges` and traces field-aligned paths until every patch is
/// valid, as far as possible (see the module docs).
pub fn compute(
    positions: &[DVec3],
    triangles: &[[u32; 3]],
    feature_edges: &[[u32; 2]],
    field: &FieldOutput,
    params: &TraceParams,
    control: &Control,
) -> Result<TraceOutput, Error> {
    let stage = Stage::Trace;
    control.checkpoint(stage)?;
    let settings = params.settings();
    let parts = crate::components::split(triangles, positions.len());
    let done = AtomicUsize::new(0);
    let results: Vec<Result<PartLayout, Error>> = parts
        .par_iter()
        .map(|part| {
            control.checkpoint(stage)?;
            let layout =
                layout_part(positions, triangles, feature_edges, field, &part.faces, &settings, control)?;
            let finished = done.fetch_add(1, Ordering::Relaxed) + 1;
            control.set_progress(stage, finished as f32 / parts.len().max(1) as f32);
            Ok(layout)
        })
        .collect();
    let mut out = TraceOutput {
        mesh: TraceMesh { positions: positions.to_vec(), ..TraceMesh::default() },
        ..TraceOutput::default()
    };
    out.report.parts = parts.len();
    for result in results {
        merge(&mut out, result?);
    }
    out.mesh.feature_edges.sort_unstable();
    out.mesh.feature_edges.dedup();
    out.mesh.singularities.sort_unstable();
    let r = &mut out.report;
    for p in &out.patches {
        r.sides_histogram[p.sides.len().min(8)] += 1;
        match p.kind {
            PatchKind::NonDisk => r.non_disk += 1,
            PatchKind::NotConvex => r.not_convex += 1,
            PatchKind::LowSides => r.low_sides += 1,
            PatchKind::HighSides => r.high_sides += 1,
            PatchKind::NotCcAble => r.not_cc_able += 1,
            PatchKind::ValenceMismatch => r.valence_mismatch += 1,
            PatchKind::Ok => r.ok += 1,
        }
        if !p.kind.is_valid() {
            r.unsolved += 1;
        }
    }
    r.patches = out.patches.len();
    r.kept_paths = out.paths.len();
    control.set_progress(stage, 1.0);
    Ok(out)
}

/// One part's layout, in its own vertex ids (`global` maps the part's
/// original vertices; ids past its end are new, from refinement).
struct PartLayout {
    global: Vec<u32>,
    mesh: TraceMesh,
    patch_of_face: Vec<u32>,
    patches: Vec<Patch>,
    paths: Vec<Vec<u32>>,
    tally: select::Tally,
}

fn layout_part(
    positions: &[DVec3],
    triangles: &[[u32; 3]],
    feature_edges: &[[u32; 2]],
    field: &FieldOutput,
    faces: &[u32],
    settings: &select::Settings,
    control: &Control,
) -> Result<PartLayout, Error> {
    let mut global: Vec<u32> = faces.iter().flat_map(|&f| triangles[f as usize]).collect();
    global.sort_unstable();
    global.dedup();
    let local = |v: u32| global.binary_search(&v).ok().map(|i| i as u32);
    let mut part = TraceMesh {
        positions: global.iter().map(|&v| positions[v as usize]).collect(),
        triangles: faces.iter().map(|&f| triangles[f as usize].map(|v| local(v).unwrap_or(0))).collect(),
        feature_edges: feature_edges.iter().filter_map(|&[a, b]| Some([local(a)?, local(b)?])).collect(),
        directions: faces
            .iter()
            .map(|&f| field.directions.get(f as usize).copied().unwrap_or(DVec3::ZERO))
            .collect(),
        singularities: Vec::new(),
    };
    // The part's own singularities (the field's merge sums parts that touch).
    let frames = FaceFrames::new(&part.positions, &part.triangles);
    let powers: Vec<_> = (0..part.triangles.len() as u32)
        .map(|f| frames.power(f, part.directions[f as usize], ROSY))
        .collect();
    part.singularities = vertex_indices(&part.positions, &part.triangles, &powers)
        .into_iter()
        .enumerate()
        .filter(|&(_, k)| k != 0)
        .map(|(v, k)| (v as u32, k))
        .collect();
    let mesh = refine::for_tracing(&part);
    let mut singular = vec![0i32; mesh.positions.len()];
    for &(v, k) in &mesh.singularities {
        singular[v as usize] = k;
    }
    let (mut sum, mut count) = (0.0, 0usize);
    for t in &mesh.triangles {
        for i in 0..3 {
            sum += (mesh.positions[t[i] as usize] - mesh.positions[t[(i + 1) % 3] as usize]).length();
            count += 1;
        }
    }
    let mean_edge = if count > 0 { sum / count as f64 } else { 0.0 };
    let labels = HashMap::new();
    let region = select::Region::new(
        select::RegionMesh {
            positions: &mesh.positions,
            triangles: &mesh.triangles,
            barrier: &mesh.feature_edges,
            directions: mesh.directions.clone(),
            singular: &singular,
            labels: &labels,
            tolerance: settings.cc_tolerance * mean_edge,
        },
        settings,
    )?;
    let mut layout = select::Layout::new(region, settings);
    layout.run(control)?;
    let partition = layout.partition().clone();
    let cut = &layout.region.cut;
    let original = |v: u32| cut.original[v as usize];
    // Patches renumbered by first face.
    let mut ids: HashMap<u32, u32> = HashMap::new();
    let mut patches = Vec::new();
    let mut patch_of_face = vec![0u32; mesh.triangles.len()];
    for (f, &p) in partition.patch_of_face().iter().enumerate() {
        let next = ids.len() as u32;
        let id = *ids.entry(p).or_insert(next);
        if id == next {
            if let Some(info) = partition.info(p) {
                patches.push(Patch {
                    corners: info.corners.iter().map(|&v| original(v)).collect(),
                    sides: info
                        .sides
                        .iter()
                        .map(|s| Side {
                            vertices: s.vertices.iter().map(|&v| original(v)).collect(),
                            length: s.length,
                        })
                        .collect(),
                    singularities: info.singularities.iter().map(|&(v, k)| (original(v), k)).collect(),
                    kind: info.kind,
                    cc_margin: info.cc_margin,
                });
            }
        }
        patch_of_face[f] = id;
    }
    let paths = layout
        .chosen
        .iter()
        .filter(|c| !c.removed)
        .map(|c| {
            let mut chain: Vec<u32> = Vec::with_capacity(c.path.nodes.len() + 1);
            for v in c.path.vertices().map(original) {
                if chain.last() != Some(&v) {
                    chain.push(v);
                }
            }
            if c.path.closed {
                if let Some(&first) = chain.first() {
                    chain.push(first);
                }
            }
            chain
        })
        .collect();
    let tally = layout.tally.clone();
    Ok(PartLayout { global, mesh, patch_of_face, patches, paths, tally })
}

/// Appends a part's layout to `out`, renumbering vertices and patches.
fn merge(out: &mut TraceOutput, part: PartLayout) {
    let original = part.global.len();
    let base_new = out.mesh.positions.len() as u32;
    let map = |v: u32| {
        if (v as usize) < original { part.global[v as usize] } else { base_new + v - original as u32 }
    };
    out.mesh.positions.extend_from_slice(part.mesh.positions.get(original..).unwrap_or(&[]));
    out.mesh.triangles.extend(part.mesh.triangles.iter().map(|t| t.map(map)));
    out.mesh.directions.extend_from_slice(&part.mesh.directions);
    out.mesh.feature_edges.extend(part.mesh.feature_edges.iter().map(|&[a, b]| {
        let (x, y) = (map(a), map(b));
        [x.min(y), x.max(y)]
    }));
    out.mesh.singularities.extend(part.mesh.singularities.iter().map(|&(v, k)| (map(v), k)));
    let patch_base = out.patches.len() as u32;
    out.patch_of_face.extend(part.patch_of_face.iter().map(|&p| p + patch_base));
    out.patches.extend(part.patches.into_iter().map(|p| {
        Patch {
            corners: p.corners.into_iter().map(map).collect(),
            sides: p
                .sides
                .into_iter()
                .map(|s| Side { vertices: s.vertices.into_iter().map(map).collect(), length: s.length })
                .collect(),
            singularities: p.singularities.into_iter().map(|(v, k)| (map(v), k)).collect(),
            ..p
        }
    }));
    out.paths.extend(part.paths.into_iter().map(|c| c.into_iter().map(map).collect()));
    let r = &mut out.report;
    r.candidates += part.tally.candidates;
    r.removed_paths += part.tally.removed_paths;
    r.sub_passes_run += part.tally.sub_passes_run;
    r.relaxed_patches += part.tally.relaxed_patches;
    r.seams += part.tally.seams;
}
