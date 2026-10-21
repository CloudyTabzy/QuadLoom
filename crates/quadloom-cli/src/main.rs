//! `quadloom`: run the QuadLoom core on mesh files.
//!
//! ```text
//! quadloom info <mesh.obj|stl>          print validation, repair and feature reports as JSON
//! quadloom run  <in.obj|stl> <out.obj> [--rosy <out.rosy>]
//!                                       run the pipeline and write the result
//!                                       (and its cross field, in QuadWild's format)
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

const USAGE: &str = "usage: quadloom info <mesh>\n       quadloom run <in> <out.obj> [--rosy <out.rosy>]\n       quadloom field-index <mesh.obj> <field.rosy>";

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
            let output = run_file(Path::new(input))?;
            println!(
                "{}",
                json!({ "validation": output.validation, "repair": output.repair, "features": output.features })
            );
            Ok(())
        }
        ["run", input, output_path, rest @ ..] => {
            let rosy = match rest {
                [] => None,
                ["--rosy", path] => Some(Path::new(*path)),
                _ => return Err(Failure::Usage),
            };
            let output = run_file(Path::new(input))?;
            io::write_obj(Path::new(output_path), &output.mesh)?;
            if let (Some(path), Some(field)) = (rosy, &output.field) {
                io::write_rosy(path, &field.directions)?;
            }
            let timings: serde_json::Map<String, serde_json::Value> =
                output.timings_ms.iter().map(|(stage, ms)| (stage.name().to_owned(), json!(ms))).collect();
            println!(
                "{}",
                json!({
                    "input_faces": output.validation.faces,
                    "output_faces": output.mesh.face_count(),
                    "output_vertices": output.mesh.positions.len(),
                    "validation": output.validation,
                    "repair": output.repair,
                    "features": output.features,
                    "components": output.components,
                    "remesh": output.remesh,
                    "field": output.field.as_ref().map(|f| &f.report),
                    "timings_ms": timings,
                })
            );
            Ok(())
        }
        ["field-index", mesh, rosy] => field_index(Path::new(mesh), Path::new(rosy)),
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
    let singular: Vec<i32> = indices.into_iter().filter(|&k| k != 0).collect();
    println!(
        "{}",
        json!({
            "faces": triangles.len(),
            "singularities": singular.len(),
            "positive_quarters": singular.iter().filter(|&&k| k > 0).sum::<i32>(),
            "negative_quarters": singular.iter().filter(|&&k| k < 0).sum::<i32>(),
        })
    );
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

fn run_file(path: &Path) -> Result<RunOutput, Failure> {
    let (positions, indices) = io::load(path)?;
    let features = quadloom_core::features::FeatureInput::default();
    Ok(quadloom_core::run(&positions, &indices, &features, &Params::default(), &Control::default())?)
}
