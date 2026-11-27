//! Reading a model's kind from its crease lines, to suggest settings for it.
//!
//! The angle test turns every edge folded past the threshold into a feature.
//! What those edges are differs by model: the edges of a machined part, the
//! creases a sculpted figure's limbs meet at, or scraps of fold in fur, pores
//! and scan noise. The first two should steer the quads; the third bends the
//! flow of a smooth surface to no purpose. No one setting suits all three
//! (decision 0024), but the lines themselves say which a model has: their
//! lengths and how sharply they fold ([`crate::features::CreaseLine`]).

use crate::control::{Control, Stage};
use crate::error::Error;
use crate::features::{self, CreaseLine, FeatureInput};
use crate::mesh::InputMesh;
use crate::params::Params;
use crate::repair::{RepairedMesh, repair};
use crate::symmetry::{self, SymmetryParams, SymmetryReport};
use crate::validate::validate;

/// A line this long (in nominal edges) is part of a model's form whatever
/// its fold: ten quads or so of crease.
pub const LONG_LINE: f64 = 10.0;
/// A line folded this sharply on average is part of a model's form whatever
/// its length (the same bar as [`crate::features`]' hard fold).
pub const HARD_FOLD_DEG: f64 = 80.0;

/// What a model's crease lines say it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// No crease lines to speak of.
    Smooth,
    /// Nearly all its crease is long or sharp lines: a machined part.
    HardSurface,
    /// Long or sharp lines, and scraps beside them: a figure with real creases.
    CreasedSculpt,
    /// Scraps only: fur, pores, scan noise, the rims of a cast part.
    SurfaceDetail,
    /// The lines don't say.
    Unsure,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Smooth => "smooth",
            Self::HardSurface => "hard_surface",
            Self::CreasedSculpt => "creased_sculpt",
            Self::SurfaceDetail => "surface_detail",
            Self::Unsure => "unsure",
        }
    }
}

/// A model's kind, the numbers it was read from, and the feature settings
/// that suit it.
#[derive(Clone, Debug, PartialEq)]
pub struct Advice {
    pub kind: Kind,
    /// Angle-only crease lines, and their total length in nominal edges.
    pub lines: usize,
    pub length: f64,
    /// Lines that are long or sharp ([`LONG_LINE`], [`HARD_FOLD_DEG`]), and
    /// their share of the total length.
    pub form_lines: usize,
    pub form_share: f64,
    /// The other lines, their total length in nominal edges and their mean
    /// fold in degrees.
    pub scraps: usize,
    pub scrap_length: f64,
    pub scrap_fold_deg: f64,
    /// Suggested [`features::FeatureParams::use_angle`] and
    /// [`features::FeatureParams::min_fold`].
    pub use_angle: bool,
    pub min_fold: f64,
    /// The mirror planes the model is symmetric about, as Symmetry's Auto
    /// Detect finds them at the caller's tolerance: per axis whether there
    /// is one, where it lies, and how far from symmetric the model is across
    /// it (see [`SymmetryReport`]). All off for a bare [`classify`].
    pub mirror: SymmetryReport,
    /// Whether Symmetry's Auto Detect is suggested: for a model with a
    /// mirror plane. Never a suggestion to turn Symmetry off.
    pub symmetry: bool,
}

/// Reads a model's kind from its crease lines and picks the feature settings
/// for it. `use_angle` and `min_fold` are the caller's current settings,
/// which an unsure reading leaves as they are.
pub fn classify(lines: &[CreaseLine], use_angle: bool, min_fold: f64) -> Advice {
    let hard = HARD_FOLD_DEG.to_radians();
    let is_form = |l: &CreaseLine| l.length >= LONG_LINE || l.fold >= hard;
    let length: f64 = lines.iter().map(|l| l.length).sum();
    let form: Vec<&CreaseLine> = lines.iter().filter(|l| is_form(l)).collect();
    let form_length: f64 = form.iter().map(|l| l.length).sum();
    let scraps = lines.len() - form.len();
    let scrap_length = length - form_length;
    let scrap_fold: f64 = lines.iter().filter(|l| !is_form(l)).map(|l| l.length * l.fold).sum();
    let mut advice = Advice {
        kind: Kind::Unsure,
        lines: lines.len(),
        length,
        form_lines: form.len(),
        form_share: if length > 0.0 { form_length / length } else { 0.0 },
        scraps,
        scrap_length: scrap_length.max(0.0),
        scrap_fold_deg: if scrap_length > 0.0 { (scrap_fold / scrap_length).to_degrees() } else { 0.0 },
        use_angle,
        min_fold,
        mirror: SymmetryReport::default(),
        symmetry: false,
    };
    let (kind, suggestion) = decide(&advice);
    advice.kind = kind;
    if let Some((use_angle, min_fold)) = suggestion {
        advice.use_angle = use_angle;
        advice.min_fold = min_fold;
    }
    advice
}

/// The rule: a kind, and the `(use_angle, min_fold)` that suit it (`None`
/// to leave the settings as they are).
///
/// Set against all 70 benchmark models run four ways (every crease kept,
/// Ignore Faint Creases at 4 and at 9, Sharp Edges off), by the share of a
/// model's crease length in long or sharp lines:
///
/// - **0.9 and over** (fandisk, CommercialRefrigerator, 31 models): every
///   crease kept. Dropping the few scraps gains nothing on most and costs
///   two their layout; Sharp Edges off scores well on shape and gives up
///   the hard edges, which is not an option.
/// - **0.1 to 0.9** (cheburashka, the bunny, 28 models): scraps dropped at 9.
///   Quad shape better on 20 of them and worse on 8; 4 does less and harms
///   as often.
/// - **Under 0.1** (the armadillo, the rocker arm, 6 models with any crease):
///   no setting does better than another across them, so none is suggested.
fn decide(a: &Advice) -> (Kind, Option<(bool, f64)>) {
    if a.length < SMOOTH_LENGTH {
        return (Kind::Smooth, None);
    }
    if a.form_share >= HARD_SHARE {
        return (Kind::HardSurface, Some((true, 0.0)));
    }
    if a.form_share < DETAIL_SHARE {
        return (Kind::SurfaceDetail, None);
    }
    (Kind::CreasedSculpt, Some((true, SCRAP_FOLD)))
}

/// Under this much crease in all (nominal edges), a model is smooth.
const SMOOTH_LENGTH: f64 = 5.0;
/// With this share of its crease in long or sharp lines, a model is hard surface.
const HARD_SHARE: f64 = 0.9;
/// With less than this share, its creases are surface detail only.
const DETAIL_SHARE: f64 = 0.1;
/// The `min_fold` that drops scraps and keeps form.
const SCRAP_FOLD: f64 = 9.0;

/// A model's angle-only crease lines at the angle threshold in `params`,
/// with every line kept: validation, cleanup and the angle test, nothing
/// after. A fraction of a second on meshes the pipeline takes seconds for.
pub fn crease_lines(
    positions: &[f32],
    indices: &[u32],
    feature_input: &FeatureInput,
    params: &Params,
    control: &Control,
) -> Result<Vec<CreaseLine>, Error> {
    let repaired = cleaned(positions, indices, params, control)?;
    Ok(lines_of(&repaired, feature_input, params))
}

/// The mesh as the pipeline's later stages get it: validated and cleaned up.
fn cleaned(
    positions: &[f32],
    indices: &[u32],
    params: &Params,
    control: &Control,
) -> Result<RepairedMesh, Error> {
    control.checkpoint(Stage::Validate)?;
    params.check()?;
    crate::pipeline::check_limits(positions.len() / 3, indices.len(), params)?;
    let mesh = InputMesh::from_buffers(positions, indices)?;
    validate(&mesh, control)?;
    let (repaired, _, _) = repair(&mesh, &params.repair, control)?;
    control.checkpoint(Stage::Features)?;
    Ok(repaired)
}

/// Every angle-only crease line of a cleaned mesh.
fn lines_of(repaired: &RepairedMesh, feature_input: &FeatureInput, params: &Params) -> Vec<CreaseLine> {
    let every_line = features::FeatureParams { use_angle: true, min_fold: 0.0, ..params.features.clone() };
    let (_, report) = features::detect(repaired, feature_input, &every_line);
    report.lines
}

/// The mirror planes of a cleaned mesh, as Symmetry's Auto Detect finds
/// them at the tolerance in `params` (whatever axes are chosen there), and
/// whether to suggest Auto Detect.
///
/// It is suggested for every model with a plane: one side is then remeshed
/// and mirrored, so both sides get the same quads, and the run takes about
/// half the time.
fn mirror_of(repaired: &RepairedMesh, params: &SymmetryParams) -> (SymmetryReport, bool) {
    let detect = SymmetryParams { auto_detect: true, ..params.clone() };
    let (planes, report) = symmetry::planes(&repaired.positions, &repaired.triangles, &detect);
    (report, !planes.is_empty())
}

/// [`classify`] for a mesh: its crease lines read at the angle threshold in
/// `params`, against the feature settings in `params`.
pub fn advise(
    positions: &[f32],
    indices: &[u32],
    feature_input: &FeatureInput,
    params: &Params,
    control: &Control,
) -> Result<Advice, Error> {
    let repaired = cleaned(positions, indices, params, control)?;
    let lines = lines_of(&repaired, feature_input, params);
    let mut advice = classify(&lines, params.features.use_angle, params.features.min_fold);
    (advice.mirror, advice.symmetry) = mirror_of(&repaired, &params.symmetry);
    Ok(advice)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(length: f64, fold_deg: f64) -> CreaseLine {
        CreaseLine { length, fold: fold_deg.to_radians(), edges: length.ceil() as u32 }
    }

    #[test]
    fn a_model_s_kind_is_read_from_its_crease_lines() {
        // Nothing, or next to nothing: smooth, and the settings stay.
        let smooth = classify(&[line(1.5, 40.0), line(1.5, 38.0)], true, 3.0);
        assert_eq!((smooth.kind, smooth.use_angle, smooth.min_fold), (Kind::Smooth, true, 3.0));
        assert_eq!(classify(&[], false, 0.0).kind, Kind::Smooth);
        // One line 572 edges long at 88 degrees and its faint tail (fandisk): every crease kept.
        let hard = classify(&[line(572.0, 88.0), line(11.5, 47.0)], true, 9.0);
        assert_eq!((hard.kind, hard.use_angle, hard.min_fold), (Kind::HardSurface, true, 0.0));
        assert_eq!((hard.lines, hard.form_lines, hard.scraps), (2, 2, 0));
        // Short lines at a right angle are form too (a part with small notches).
        let notched =
            classify(&[line(200.0, 90.0), line(1.5, 93.0), line(1.5, 95.0), line(4.0, 45.0)], true, 0.0);
        assert_eq!((notched.kind, notched.form_lines, notched.scraps), (Kind::HardSurface, 3, 1));
        // Ten creases of 10 to 33 edges and sixteen scraps (cheburashka): the scraps go.
        let mut lines: Vec<CreaseLine> = (0..10).map(|k| line(10.0 + 2.5 * f64::from(k), 95.0)).collect();
        lines.extend((0..16).map(|_| line(1.7, 50.0)));
        let creased = classify(&lines, true, 0.0);
        assert_eq!((creased.kind, creased.use_angle, creased.min_fold), (Kind::CreasedSculpt, true, 9.0));
        assert_eq!((creased.form_lines, creased.scraps), (10, 16));
        assert!((creased.form_share - 0.888).abs() < 0.01 && (creased.scrap_fold_deg - 50.0).abs() < 1e-6);
        // Scraps and nothing else (the armadillo): named, and left alone.
        let detail: Vec<CreaseLine> = (0..300).map(|_| line(1.2, 44.0)).collect();
        let only = classify(&detail, true, 2.0);
        assert_eq!((only.kind, only.use_angle, only.min_fold), (Kind::SurfaceDetail, true, 2.0));
        assert_eq!((only.form_lines, only.scraps), (0, 300));
    }

    #[test]
    fn a_cube_reads_as_hard_surface() {
        let (p, t, _) = crate::testmesh::cube(2);
        let positions: Vec<f32> = p.iter().flat_map(|v| [v.x as f32, v.y as f32, v.z as f32]).collect();
        let indices: Vec<u32> = t.iter().flatten().copied().collect();
        let advice =
            advise(&positions, &indices, &FeatureInput::default(), &Params::default(), &Control::default())
                .unwrap();
        assert_eq!((advice.kind, advice.lines, advice.form_lines), (Kind::HardSurface, 1, 1));
        assert_eq!((advice.use_angle, advice.min_fold), (true, 0.0));
        // Asking never needs Sharp Edges to be on: the lines are read either way.
        let mut off = Params::default();
        off.features.use_angle = false;
        let advice =
            advise(&positions, &indices, &FeatureInput::default(), &off, &Control::default()).unwrap();
        assert_eq!((advice.kind, advice.use_angle), (Kind::HardSurface, true));
    }
}
