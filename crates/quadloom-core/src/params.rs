//! User-facing parameters for a pipeline run.

use crate::control::Stage;
use crate::error::Error;
#[cfg(feature = "fault-injection")]
use crate::fault::Fault;
use crate::features::FeatureParams;
use crate::field::FieldParams;
use crate::frame::FrameParams;
use crate::quad::QuadParams;
use crate::remesh::RemeshParams;
use crate::repair::RepairParams;
use crate::symmetry::SymmetryParams;
use crate::trace::TraceParams;

/// Largest allowed weld distance, as a fraction of the bbox diagonal. Above
/// this, welding stops being "merge split seams" and starts destroying detail
/// (and degrades to quadratic time as most vertices share a grid cell).
pub const MAX_WELD_EPSILON_REL: f64 = 1e-2;

/// Default refusal threshold: inputs above this many triangles are rejected up front.
pub const DEFAULT_MAX_FACES: usize = 2_000_000;

/// Parameters for [`crate::run`]. M1 only carries safety limits; every later
/// stage adds its own section.
#[derive(Clone, Debug, PartialEq)]
pub struct Params {
    /// Inputs with more triangles than this are refused before any per-face work.
    pub max_faces: usize,
    /// Stage 0: cleanup.
    pub repair: RepairParams,
    /// Stage 1: feature lines.
    pub features: FeatureParams,
    /// Stage 2: isotropic prep remesh.
    pub remesh: RemeshParams,
    /// Stage 3: cross field.
    pub field: FieldParams,
    /// Stage 3b: rectangular frame (off by default).
    pub frame: FrameParams,
    /// Stage 4: field tracing and patch layout.
    pub trace: TraceParams,
    /// Stages 5–6: quantization and quad patterns.
    pub quad: QuadParams,
    /// Remesh one side of mirror planes and mirror the result.
    pub symmetry: SymmetryParams,
    /// Test-only fault to inject (see [`crate::fault`]).
    #[cfg(feature = "fault-injection")]
    pub fault: Option<Fault>,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            max_faces: DEFAULT_MAX_FACES,
            repair: RepairParams::default(),
            features: FeatureParams::default(),
            remesh: RemeshParams::default(),
            field: FieldParams::default(),
            frame: FrameParams::default(),
            trace: TraceParams::default(),
            quad: QuadParams::default(),
            symmetry: SymmetryParams::default(),
            #[cfg(feature = "fault-injection")]
            fault: None,
        }
    }
}

impl Params {
    /// Rejects settings that are out of range, naming the setting.
    pub fn check(&self) -> Result<(), Error> {
        let bad = |message: String| Err(Error::invalid_input(Stage::Validate, message));
        let weld = self.repair.weld_epsilon_rel;
        if !weld.is_finite() || weld > MAX_WELD_EPSILON_REL {
            return bad(format!(
                "repair.weld_epsilon_rel must be at most {MAX_WELD_EPSILON_REL} (got {weld})"
            ));
        }
        let angle = self.features.angle_deg;
        if !(0.0..=180.0).contains(&angle) {
            return bad(format!("features.angle_deg must be between 0 and 180 (got {angle})"));
        }
        let r = &self.remesh;
        if !(0.0..=0.5).contains(&r.target_edge_rel) {
            return bad(format!(
                "remesh.target_edge_rel must be between 0 (auto) and 0.5 (got {})",
                r.target_edge_rel
            ));
        }
        if r.iterations > 100 {
            return bad(format!("remesh.iterations must be at most 100 (got {})", r.iterations));
        }
        if !(r.min_mult > 0.0 && r.min_mult <= 1.0 && r.max_mult >= 1.0 && r.max_mult <= 10.0) {
            return bad(format!(
                "remesh.min_mult must be in (0, 1] and remesh.max_mult in [1, 10] (got {} and {})",
                r.min_mult, r.max_mult
            ));
        }
        if !(r.max_surface_distance_rel > 0.0 && r.max_surface_distance_rel <= 0.1) {
            return bad(format!(
                "remesh.max_surface_distance_rel must be in (0, 0.1] (got {})",
                r.max_surface_distance_rel
            ));
        }
        let f = &self.field;
        if !(0.0..=100.0).contains(&f.alignment_weight) {
            return bad(format!(
                "field.alignment_weight must be between 0 and 100 (got {})",
                f.alignment_weight
            ));
        }
        if !(0.0..=1000.0).contains(&f.sharp_factor) {
            return bad(format!("field.sharp_factor must be between 0 and 1000 (got {})", f.sharp_factor));
        }
        let fr = &self.frame;
        if !(0.0..=1.0).contains(&fr.balance) {
            return bad(format!("frame.balance must be between 0 and 1 (got {})", fr.balance));
        }
        if !(1.0..=20.0).contains(&fr.max_stretch) {
            return bad(format!("frame.max_stretch must be between 1 and 20 (got {})", fr.max_stretch));
        }
        if !(0.0..=100.0).contains(&fr.stretch_weight) {
            return bad(format!(
                "frame.stretch_weight must be between 0 and 100 (got {})",
                fr.stretch_weight
            ));
        }
        if !(0.0..=100.0).contains(&fr.align_weight) {
            return bad(format!("frame.align_weight must be between 0 and 100 (got {})", fr.align_weight));
        }
        if !(1e-6..=10.0).contains(&fr.smoothness) {
            return bad(format!("frame.smoothness must be between 1e-6 and 10 (got {})", fr.smoothness));
        }
        if !(1..=500).contains(&fr.max_iterations) {
            return bad(format!(
                "frame.max_iterations must be between 1 and 500 (got {})",
                fr.max_iterations
            ));
        }
        if !(1_000..=2_000_000).contains(&fr.max_part_faces) {
            return bad(format!(
                "frame.max_part_faces must be between 1000 and 2000000 (got {})",
                fr.max_part_faces
            ));
        }
        let t = &self.trace;
        if !(4..=6).contains(&t.max_sides) {
            return bad(format!("trace.max_sides must be between 4 and 6 (got {})", t.max_sides));
        }
        if !(0.0..=1000.0).contains(&t.drift) {
            return bad(format!("trace.drift must be between 0 and 1000 (got {})", t.drift));
        }
        if !(0.0..=10.0).contains(&t.cc_tolerance) {
            return bad(format!("trace.cc_tolerance must be between 0 and 10 (got {})", t.cc_tolerance));
        }
        if !(0.001..=0.1).contains(&t.loop_density) {
            return bad(format!("trace.loop_density must be between 0.001 and 0.1 (got {})", t.loop_density));
        }
        if t.sub_passes > 10 {
            return bad(format!("trace.sub_passes must be at most 10 (got {})", t.sub_passes));
        }
        let q = &self.quad;
        if !(q.edge_scale.is_finite() && (0.25..=20.0).contains(&q.edge_scale)) {
            return bad(format!("quad.edge_scale must be between 0.25 and 20 (got {})", q.edge_scale));
        }
        if !(q.regularity.is_finite() && (0.0..=1000.0).contains(&q.regularity)) {
            return bad(format!("quad.regularity must be between 0 and 1000 (got {})", q.regularity));
        }
        if !(q.adaptivity.is_finite() && (0.0..=1.0).contains(&q.adaptivity)) {
            return bad(format!("quad.adaptivity must be between 0 and 1 (got {})", q.adaptivity));
        }
        let sym = &self.symmetry;
        if !(sym.tolerance.is_finite() && (0.0..=0.1).contains(&sym.tolerance)) {
            return bad(format!("symmetry.tolerance must be between 0 and 0.1 (got {})", sym.tolerance));
        }
        if q.smoothing > 100 {
            return bad(format!("quad.smoothing must be at most 100 (got {})", q.smoothing));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Default params with one frame setting edited.
    fn with(edit: impl FnOnce(&mut FrameParams)) -> Params {
        let mut params = Params::default();
        edit(&mut params.frame);
        params
    }

    #[test]
    fn defaults_pass_the_check() {
        assert!(Params::default().check().is_ok());
    }

    #[test]
    fn frame_params_are_range_checked() {
        let bad: Vec<(&str, Params)> = vec![
            ("balance", with(|f| f.balance = 1.5)),
            ("balance", with(|f| f.balance = f64::NAN)),
            ("max_stretch", with(|f| f.max_stretch = 0.5)),
            ("max_stretch", with(|f| f.max_stretch = 25.0)),
            ("stretch_weight", with(|f| f.stretch_weight = -1.0)),
            ("align_weight", with(|f| f.align_weight = 101.0)),
            ("align_weight", with(|f| f.align_weight = f64::INFINITY)),
            ("smoothness", with(|f| f.smoothness = 0.0)),
            ("smoothness", with(|f| f.smoothness = 11.0)),
            ("max_iterations", with(|f| f.max_iterations = 0)),
            ("max_iterations", with(|f| f.max_iterations = 501)),
            ("max_part_faces", with(|f| f.max_part_faces = 999)),
            ("max_part_faces", with(|f| f.max_part_faces = 2_000_001)),
        ];
        for (name, params) in bad {
            let err = params.check().expect_err(name);
            assert!(err.message.contains(&format!("frame.{name}")), "{name}: {}", err.message);
        }
    }
}
