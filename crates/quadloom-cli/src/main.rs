//! `quadloom`: run the QuadLoom core on mesh files.
//!
//! ```text
//! quadloom info <mesh.obj|stl>          print validation, repair and feature reports as JSON
//! quadloom run  <in.obj|stl> <out.obj> [--rosy <out.rosy>] [--sharp <out.sharp>] [--layout <prefix>] [--frame <json>]
//!                                       run the pipeline and write the result
//!                                       (and its cross field and patch layout,
//!                                       in QuadWild's formats; with --frame,
//!                                       the frame's rotated directions)
//! quadloom layout <mesh.obj> <field.rosy> <features.sharp> <prefix> [--params <json>]
//!                                       trace a given mesh, field and features
//!                                       (e.g. QuadWild's `_rem` files); writes
//!                                       `<prefix>_p0.*` and prints the report
//! quadloom layout-stats <mesh.obj> <file.patch> <file.corners>
//!                                       measure any layout the same way, as JSON
//! quadloom field-index <mesh.obj> <field.rosy>
//!                                       singularities of a per-face cross field as JSON
//! quadloom metrics <result.obj> [--reference <input.obj|stl>]
//!                                       quad quality (+ surface distance) as JSON
//! ```
//!
//! Exit codes: 0 ok, 2 invalid input, 3 internal error or cancelled,
//! 4 file I/O error, 64 usage error. Errors are printed as JSON on stderr.

#![forbid(unsafe_code)]

mod io;

use std::path::Path;
use std::process::ExitCode;

use quadloom_core::{Control, Error, ErrorKind, Params, RunOutput};
use serde_json::json;

const USAGE: &str = "usage: quadloom info <mesh>\n       quadloom run <in> <out.obj> [--rosy <out.rosy>] [--sharp <out.sharp>] [--layout <prefix>] [--frame <json>]\n       quadloom field-index <mesh.obj> <field.rosy>\n       quadloom layout <mesh.obj> <field.rosy> <features.sharp> <prefix> [--params <json>]\n       quadloom layout-stats <mesh.obj> <file.patch> <file.corners>";

enum Failure {
    Usage,
    Io(io::IoError),
    Core(Error),
}

impl From<io::IoError> for Failure {
    fn from(e: io::IoError) -> Self {
        Self::Io(e)
    }
}

impl From<Error> for Failure {
    fn from(e: Error) -> Self {
        Self::Core(e)
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match dispatch(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(Failure::Usage) => {
            eprintln!("{USAGE}");
            ExitCode::from(64)
        }
        Err(Failure::Io(io::IoError::Io(message))) => {
            eprintln!(
                "{}",
                json!({ "error": { "kind": "io", "stage": null, "message": message, "hint": null } })
            );
            ExitCode::from(4)
        }
        Err(Failure::Io(io::IoError::Invalid(message))) => {
            eprintln!(
                "{}",
                json!({ "error": { "kind": "invalid_input", "stage": "validate", "message": message, "hint": null } })
            );
            ExitCode::from(2)
        }
        Err(Failure::Core(error)) => {
            eprintln!("{}", json!({ "error": error }));
            ExitCode::from(match error.kind {
                ErrorKind::InvalidInput => 2,
                ErrorKind::Cancelled | ErrorKind::Internal => 3,
            })
        }
    }
}

fn dispatch(args: &[&str]) -> Result<(), Failure> {
    match args {
        ["info", input] => {
            let output = run_file(Path::new(input), &Params::default())?;
            println!(
                "{}",
                json!({ "validation": output.validation, "repair": output.repair, "features": output.features })
            );
            Ok(())
        }
        ["run", input, output_path, rest @ ..] => {
            let (mut rosy, mut layout, mut sharp, mut frame) = (None, None, None, None);
            let mut options = rest.chunks(2);
            for pair in &mut options {
                match pair {
                    ["--rosy", path] => rosy = Some(Path::new(*path)),
                    ["--layout", path] => layout = Some(Path::new(*path)),
                    ["--sharp", path] => sharp = Some(Path::new(*path)),
                    ["--frame", json] => frame = Some(*json),
                    _ => return Err(Failure::Usage),
                }
            }
            let params = run_params(frame)?;
            let output = run_file(Path::new(input), &params)?;
            // The quad mesh when the quad stages ran, else the prep triangles.
            let written = output.quads.as_ref().map_or(&output.mesh, |q| &q.mesh);
            io::write_obj(Path::new(output_path), written)?;
            // With a frame, the rosy file carries the frame's rotated directions.
            let directions = match (&output.frame, &output.field) {
                (Some(frame), _) => Some(&frame.directions),
                (None, Some(field)) => Some(&field.directions),
                (None, None) => None,
            };
            if let (Some(path), Some(directions)) = (rosy, directions) {
                io::write_rosy(path, directions)?;
            }
            if let Some(path) = sharp {
                io::write_sharp(path, &output.mesh, &output.feature_edges)?;
            }
            if let (Some(prefix), Some(trace)) = (layout, &output.trace) {
                io::write_layout(prefix, trace)?;
            }
            let timings: serde_json::Map<String, serde_json::Value> =
                output.timings_ms.iter().map(|(stage, ms)| (stage.name().to_owned(), json!(ms))).collect();
            println!(
                "{}",
                json!({
                    "input_faces": output.validation.faces,
                    "output_faces": written.face_count(),
                    "output_vertices": written.positions.len(),
                    "prep_faces": output.mesh.face_count(),
                    "validation": output.validation,
                    "repair": output.repair,
                    "features": output.features,
                    "components": output.components,
                    "remesh": output.remesh,
                    "field": output.field.as_ref().map(|f| &f.report),
                    "frame": output.frame.as_ref().map(|f| &f.report),
                    "trace": output.trace.as_ref().map(|t| &t.report),
                    "quads": output.quads.as_ref().map(|q| &q.report),
                    "timings_ms": timings,
                })
            );
            Ok(())
        }
        ["field-index", mesh, rosy] => field_index(Path::new(mesh), Path::new(rosy)),
        ["layout", mesh, rosy, sharp, prefix, rest @ ..] => {
            let params = match rest {
                [] => quadloom_core::TraceParams::default(),
                ["--params", json] => trace_params(json)?,
                _ => return Err(Failure::Usage),
            };
            layout(Path::new(mesh), Path::new(rosy), Path::new(sharp), Path::new(prefix), &params)
        }
        ["layout-stats", mesh, patch, corners] => {
            layout_stats(Path::new(mesh), Path::new(patch), Path::new(corners))
        }
        ["metrics", result] => metrics(Path::new(result), None),
        ["metrics", result, "--reference", reference] => {
            metrics(Path::new(result), Some(Path::new(reference)))
        }
        _ => Err(Failure::Usage),
    }
}

/// Singularities of a QuadWild-format field on a triangle mesh, computed with
/// QuadLoom's own index routine (so two fields are compared the same way).
fn field_index(mesh: &Path, rosy: &Path) -> Result<(), Failure> {
    let (positions, indices) = io::load(mesh)?;
    let positions: Vec<quadloom_core::mesh::DVec3> = positions
        .chunks_exact(3)
        .map(|c| quadloom_core::mesh::DVec3::new(f64::from(c[0]), f64::from(c[1]), f64::from(c[2])))
        .collect();
    let triangles: Vec<[u32; 3]> = indices.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect();
    let directions = io::read_rosy(rosy, triangles.len())?;
    let frames = quadloom_core::field::FaceFrames::new(&positions, &triangles);
    let powers: Vec<_> = (0..triangles.len() as u32)
        .map(|f| frames.power(f, directions[f as usize], quadloom_core::field::ROSY))
        .collect();
    let indices = quadloom_core::field::vertex_indices(&positions, &triangles, &powers);
    let listed: Vec<[i64; 2]> = indices
        .iter()
        .enumerate()
        .filter(|(_, k)| **k != 0)
        .map(|(v, &k)| [v as i64, i64::from(k)])
        .collect();
    let singular: Vec<i32> = indices.into_iter().filter(|&k| k != 0).collect();
    println!(
        "{}",
        json!({
            "faces": triangles.len(),
            "singularities": singular.len(),
            "positive_quarters": singular.iter().filter(|&&k| k > 0).sum::<i32>(),
            "negative_quarters": singular.iter().filter(|&&k| k < 0).sum::<i32>(),
            "vertices": listed,
        })
    );
    Ok(())
}

type Triangles = (Vec<quadloom_core::mesh::DVec3>, Vec<[u32; 3]>);

fn load_triangles(mesh: &Path) -> Result<Triangles, Failure> {
    if mesh.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("obj")) {
        return Ok(io::load_obj_ordered(mesh)?);
    }
    let (positions, indices) = io::load(mesh)?;
    let positions = positions
        .chunks_exact(3)
        .map(|c| quadloom_core::mesh::DVec3::new(f64::from(c[0]), f64::from(c[1]), f64::from(c[2])))
        .collect();
    Ok((positions, indices.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect()))
}

/// Traces a mesh with a given field and features, as the pipeline's stage 4
/// would, and writes the layout in QuadWild's formats.
/// Trace settings from a JSON object of `TraceParams` fields (for sweeps).
fn trace_params(text: &str) -> Result<quadloom_core::TraceParams, Failure> {
    let bad = |what: String| Failure::Io(io::IoError::Invalid(format!("--params: {what}")));
    let value: serde_json::Value = serde_json::from_str(text).map_err(|e| bad(e.to_string()))?;
    let object = value.as_object().ok_or_else(|| bad("expected a JSON object".into()))?;
    let mut params = quadloom_core::TraceParams::default();
    for (key, v) in object {
        let number = || v.as_f64().ok_or_else(|| bad(format!("{key} must be a number")));
        let count = || v.as_u64().map(|n| n as usize).ok_or_else(|| bad(format!("{key} must be a count")));
        match key.as_str() {
            "max_sides" => params.max_sides = count()?,
            "sub_passes" => params.sub_passes = count()?,
            "drift" => params.drift = number()?,
            "cc_tolerance" => params.cc_tolerance = number()?,
            "loop_density" => params.loop_density = number()?,
            "match_valence" => {
                params.match_valence = v.as_bool().ok_or_else(|| bad(format!("{key} must be a bool")))?
            }
            "cost" => {
                params.cost = v
                    .as_str()
                    .and_then(quadloom_core::TraceCost::from_name)
                    .ok_or_else(|| bad("cost must be \"anisotropic\" or \"angle_penalty\"".into()))?
            }
            other => return Err(bad(format!("unknown setting {other}"))),
        }
    }
    let checked = Params { trace: params.clone(), ..Params::default() };
    checked.check()?;
    Ok(params)
}

fn layout(
    mesh: &Path,
    rosy: &Path,
    sharp: &Path,
    prefix: &Path,
    params: &quadloom_core::TraceParams,
) -> Result<(), Failure> {
    let (positions, triangles) = load_triangles(mesh)?;
    let directions = io::read_rosy(rosy, triangles.len())?;
    let features = io::read_sharp(sharp, &triangles)?;
    let frames = quadloom_core::field::FaceFrames::new(&positions, &triangles);
    let powers: Vec<_> = (0..triangles.len() as u32)
        .map(|f| frames.power(f, directions[f as usize], quadloom_core::field::ROSY))
        .collect();
    let singularities = quadloom_core::field::vertex_indices(&positions, &triangles, &powers)
        .into_iter()
        .enumerate()
        .filter(|&(_, k)| k != 0)
        .map(|(v, k)| (v as u32, k))
        .collect();
    let field = quadloom_core::FieldOutput { directions, singularities, ..Default::default() };
    let started = std::time::Instant::now();
    let trace = quadloom_core::trace::compute(
        &positions,
        &triangles,
        &features,
        &field,
        params,
        &Control::default(),
    )?;
    let ms = started.elapsed().as_secs_f64() * 1000.0;
    io::write_layout(prefix, &trace)?;
    let corners: Vec<Vec<u32>> = trace.patches.iter().map(|p| p.corners.clone()).collect();
    let stats = quadloom_core::trace::layout_stats(
        &trace.mesh.positions,
        &trace.mesh.triangles,
        &trace.patch_of_face,
        &corners,
        5,
        1.0,
    );
    println!(
        "{}",
        json!({ "report": trace.report, "stats": stats, "trace_ms": ms, "faces": trace.mesh.triangles.len() })
    );
    Ok(())
}

/// Measures a layout from its files (ours or QuadWild's) with the same routine.
fn layout_stats(mesh: &Path, patch: &Path, corners: &Path) -> Result<(), Failure> {
    let (positions, triangles) = load_triangles(mesh)?;
    let patch_of_face = io::read_patch(patch, triangles.len())?;
    let corners = io::read_corners(corners)?;
    let stats = quadloom_core::trace::layout_stats(&positions, &triangles, &patch_of_face, &corners, 5, 1.0);
    println!("{}", json!(stats));
    Ok(())
}

fn metrics(result: &Path, reference: Option<&Path>) -> Result<(), Failure> {
    let mesh = io::load_poly(result)?;
    let quality = quadloom_core::metrics::quad_quality(&mesh);
    let distance = match reference {
        Some(path) if mesh.face_count() > 0 => {
            let (positions, indices) = io::load(path)?;
            let reference = quadloom_core::InputMesh::from_buffers(&positions, &indices)?;
            Some(quadloom_core::metrics::surface_distance(&mesh, &reference))
        }
        _ => None,
    };
    println!("{}", json!({ "quality": quality, "distance": distance }));
    Ok(())
}

/// Pipeline settings for `run`: the defaults, with `--frame <json>` applied.
fn run_params(frame: Option<&str>) -> Result<Params, Failure> {
    let mut params = Params::default();
    if let Some(text) = frame {
        params.frame = frame_params(text)?;
    }
    params.check()?;
    Ok(params)
}

/// Frame settings from a JSON object of `FrameParams` fields. The frame is
/// enabled first, so `{}` turns it on with the defaults; keys are applied after.
fn frame_params(text: &str) -> Result<quadloom_core::FrameParams, Failure> {
    let bad = |what: String| Failure::Io(io::IoError::Invalid(format!("--frame: {what}")));
    let value: serde_json::Value = serde_json::from_str(text).map_err(|e| bad(e.to_string()))?;
    let object = value.as_object().ok_or_else(|| bad("expected a JSON object".into()))?;
    let mut frame = quadloom_core::FrameParams { enabled: true, ..Default::default() };
    for (key, v) in object {
        let number = || v.as_f64().ok_or_else(|| bad(format!("params.frame.{key} must be a number")));
        let flag = || v.as_bool().ok_or_else(|| bad(format!("params.frame.{key} must be a bool")));
        let count = || {
            v.as_u64().map(|n| n as usize).ok_or_else(|| bad(format!("params.frame.{key} must be a count")))
        };
        match key.as_str() {
            "enabled" => frame.enabled = flag()?,
            "curvature_stretch" => frame.curvature_stretch = flag()?,
            "balance" => frame.balance = number()?,
            "max_stretch" => frame.max_stretch = number()?,
            "stretch_weight" => frame.stretch_weight = number()?,
            "align_weight" => frame.align_weight = number()?,
            "smoothness" => frame.smoothness = number()?,
            "max_iterations" => frame.max_iterations = count()?,
            "max_part_faces" => frame.max_part_faces = count()?,
            other => {
                return Err(bad(format!(
                    "params.frame.{other} is not a known setting (known: enabled, balance, curvature_stretch, max_stretch, stretch_weight, align_weight, smoothness, max_iterations, max_part_faces)"
                )));
            }
        }
    }
    Ok(frame)
}

fn run_file(path: &Path, params: &Params) -> Result<RunOutput, Failure> {
    let (positions, indices) = io::load(path)?;
    let features = quadloom_core::features::FeatureInput::default();
    Ok(quadloom_core::run(&positions, &indices, &features, params, &Control::default())?)
}
