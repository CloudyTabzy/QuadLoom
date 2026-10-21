//! Stage 2: feature-preserving isotropic remeshing of one component.
//!
//! Each iteration (Botsch & Kobbelt 2004; the family the oracle's vcglib
//! `IsotropicRemeshing` belongs to):
//! 1. split edges longer than 4/3 of the local target
//! 2. collapse edges shorter than 4/5
//! 3. flip edges toward valence 6 (4 on boundaries)
//! 4. smooth tangentially and project back onto the input
//!
//! Feature vertices only slide along their feature line, corners never move,
//! and no operation may move the surface further than `max_surface_distance_rel`
//! from the input or flip a face.

mod passes;
mod project;
mod sizing;
#[cfg(test)]
pub(crate) mod tests;

use glam::DVec3;

use crate::control::{Control, Stage};
use crate::edit::EditMesh;
use crate::error::Error;

pub use project::Projector;
pub use sizing::target_length;

#[derive(Clone, Debug, PartialEq)]
pub struct RemeshParams {
    /// Run the prep remesh at all.
    pub enabled: bool,
    /// Target edge length as a fraction of the bbox diagonal; 0 = automatic
    /// (the oracle's `ExpectedEdgeL`, floored for open surfaces).
    pub target_edge_rel: f64,
    pub iterations: usize,
    /// Shorter edges where the surface curves, longer where it's flat.
    pub adaptive: bool,
    pub min_mult: f64,
    pub max_mult: f64,
    /// Furthest the remeshed surface may pull away from the input where a
    /// vertex is collapsed, × bbox diagonal.
    pub max_surface_distance_rel: f64,
}

impl Default for RemeshParams {
    fn default() -> Self {
        Self {
            enabled: true,
            target_edge_rel: 0.0,
            iterations: 15,
            adaptive: true,
            min_mult: 0.3,
            max_mult: 1.0,
            max_surface_distance_rel: 1.0 / 2500.0,
        }
    }
}

/// A remeshed component.
#[derive(Clone, Debug, PartialEq)]
pub struct RemeshedMesh {
    pub positions: Vec<DVec3>,
    pub triangles: Vec<[u32; 3]>,
    /// Sorted vertex pairs.
    pub feature_edges: Vec<[u32; 2]>,
}

#[derive(Clone, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct RemeshReport {
    pub input_faces: usize,
    pub output_faces: usize,
    pub target_edge_length: f64,
    pub splits: usize,
    pub collapses: usize,
    pub flips: usize,
    /// Largest distance from an output vertex or face centroid to the input
    /// surface (absolute; centroids measured to their own sheet).
    pub max_surface_distance: f64,
    pub min_angle_deg: f64,
    /// Mean of 2·inradius/circumradius (1 = equilateral).
    pub mean_radius_ratio: f64,
    /// Angle-detected feature edges released because the cleaned surface turned
    /// out smooth there (tessellation noise rather than real creases).
    pub released_features: usize,
}

/// The scale a part is remeshed at, shared by all parts of one object.
#[derive(Clone, Copy, Debug)]
pub struct Scale {
    /// Target edge length.
    pub target: f64,
    /// Largest allowed deviation from the input (absolute), from the whole
    /// object's diagonal so every part is held to the same bound.
    pub tolerance: f64,
    /// Splitting stops above this many faces.
    pub face_limit: usize,
}

impl Scale {
    /// The scale for a mesh remeshed on its own (`tolerance` from its diagonal).
    pub fn of(positions: &[DVec3], triangles: &[[u32; 3]], params: &RemeshParams, face_limit: usize) -> Self {
        Self {
            target: target_length(positions, triangles, params),
            tolerance: bbox_diagonal(positions) * params.max_surface_distance_rel,
            face_limit,
        }
    }
}

/// Early iterations after which soft features are re-checked (like the
/// oracle's `UpdateCoherentSharp`, which re-checks after the first pass).
const RELEASE_ITERATIONS: usize = 3;

/// Remeshes one component at `scale`.
///
/// `soft_edges` (a subset of `feature_edges`) came from the angle test alone;
/// after each of the first iterations, those whose faces now meet below
/// `release_angle_deg` are released (0 disables).
#[allow(clippy::too_many_arguments)]
pub fn remesh(
    positions: &[DVec3],
    triangles: &[[u32; 3]],
    feature_edges: &[[u32; 2]],
    soft_edges: &[[u32; 2]],
    release_angle_deg: f64,
    scale: Scale,
    params: &RemeshParams,
    control: &Control,
) -> Result<(RemeshedMesh, RemeshReport), Error> {
    let stage = Stage::PrepRemesh;
    let target = scale.target;
    control.checkpoint(stage)?;
    let mut mesh = EditMesh::from_triangles(positions, triangles, feature_edges).map_err(|e| {
        Error::invalid_input(stage, format!("the remesh needs a clean manifold surface: {e}"))
            .with_hint("enable Split Non-Manifold and Fix Orientation, or turn the remesh off")
    })?;
    mesh.mark_soft(soft_edges);
    // Judge feature turns at the target scale, not the input's (possibly noisy) one.
    mesh.reclassify(target);
    let projector =
        Projector::new(positions, triangles, &mesh.feature_edges(), scale.tolerance, 0.5 * target);
    let mut report =
        RemeshReport { input_faces: triangles.len(), target_edge_length: target, ..Default::default() };
    let max_faces = face_ceiling(positions, triangles, target, params).min(scale.face_limit);

    for iteration in 0..params.iterations {
        control.set_progress(stage, iteration as f32 / params.iterations.max(1) as f32);
        // Operations leave dead face slots behind; once they outnumber live
        // faces, every scan pays for them, so start the iteration fresh.
        if mesh.waste() > 1.0 {
            mesh =
                mesh.rebuilt().map_err(|e| Error::internal(stage, format!("remesh rebuild failed: {e}")))?.0;
        }
        let sizes = sizing::vertex_sizes(&mesh, target, projector.max_distance, params);
        let mut pass = passes::Pass { mesh: &mut mesh, sizes, projector: &projector, max_faces, control };
        report.splits += pass.split_long();
        control.checkpoint(stage)?;
        report.collapses += pass.collapse_short();
        control.checkpoint(stage)?;
        report.flips += pass.equalize_valence();
        control.checkpoint(stage)?;
        pass.smooth_tangential();
        control.checkpoint(stage)?;
        if release_angle_deg > 0.0 && iteration < RELEASE_ITERATIONS {
            report.released_features += mesh.release_flat_soft_features(release_angle_deg, target);
        }
    }

    // Face centroids, each against its own sheet: vertices are always
    // projected, so only faces show where the result pulls away (or bridges).
    let centroid_gap = mesh
        .faces()
        .map(|f| {
            let [a, b, c] = mesh.face_corners(f).map(|v| mesh.position(v));
            projector.surface_distance_facing((a + b + c) / 3.0, mesh.reference_normal(f))
        })
        .fold(0.0, f64::max);
    let (positions, triangles, feature_edges) = mesh.compact();
    report.output_faces = triangles.len();
    report.max_surface_distance =
        positions.iter().map(|&p| projector.surface_distance(p)).fold(centroid_gap, f64::max);
    let (min_angle, mean_ratio) = triangle_quality(&positions, &triangles);
    report.min_angle_deg = min_angle;
    report.mean_radius_ratio = mean_ratio;
    control.set_progress(stage, 1.0);
    Ok((RemeshedMesh { positions, triangles, feature_edges }, report))
}

/// Four times the face count of an ideal mesh at the finest allowed size, plus
/// the input's own faces: far above anything a healthy remesh produces.
fn face_ceiling(positions: &[DVec3], triangles: &[[u32; 3]], target: f64, params: &RemeshParams) -> usize {
    let area: f64 = triangles
        .iter()
        .map(|&[a, b, c]| {
            let [pa, pb, pc] = [a, b, c].map(|v| positions[v as usize]);
            0.5 * (pb - pa).cross(pc - pa).length()
        })
        .sum();
    let finest = target * if params.adaptive { params.min_mult } else { 1.0 };
    let ideal = area / (0.433 * finest * finest).max(f64::MIN_POSITIVE);
    (4.0 * ideal).min(1e9) as usize + triangles.len()
}

pub(crate) fn bbox_diagonal(positions: &[DVec3]) -> f64 {
    let mut points = positions.iter().copied();
    let Some(first) = points.next() else { return 1.0 };
    let (lo, hi) = points.fold((first, first), |(lo, hi), p| (lo.min(p), hi.max(p)));
    let d = (hi - lo).length();
    if d > 0.0 { d } else { 1.0 }
}

/// Smallest angle (degrees) and mean radius ratio over all triangles.
fn triangle_quality(positions: &[DVec3], triangles: &[[u32; 3]]) -> (f64, f64) {
    let mut min_angle = 180.0f64;
    let mut ratio_sum = 0.0;
    for &[a, b, c] in triangles {
        let [pa, pb, pc] = [a, b, c].map(|v| positions[v as usize]);
        let (la, lb, lc) = ((pb - pc).length(), (pa - pc).length(), (pa - pb).length());
        let area = 0.5 * (pb - pa).cross(pc - pa).length();
        let product = la * lb * lc;
        if product > 0.0 {
            ratio_sum += (16.0 * area * area / (product * (la + lb + lc))).clamp(0.0, 1.0);
        }
        for (u, v) in [(pb - pa, pc - pa), (pa - pb, pc - pb), (pa - pc, pb - pc)] {
            min_angle = min_angle.min(u.angle_between(v).to_degrees());
        }
    }
    let mean = if triangles.is_empty() { 0.0 } else { ratio_sum / triangles.len() as f64 };
    (min_angle, mean)
}
