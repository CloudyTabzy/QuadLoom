//! Mesh file I/O for the CLI. The Blender add-on never goes through files.

use std::fmt::Write as _;
use std::path::Path;

use quadloom_core::PolyMesh;
use quadloom_core::mesh::DVec3;

/// Why a mesh file couldn't be loaded or written.
#[derive(Debug)]
pub enum IoError {
    /// The file couldn't be opened, read or written.
    Io(String),
    /// The file was read but its contents aren't a usable mesh.
    Invalid(String),
}

/// Flat `xyz` positions and flat triangle indices, ready for `quadloom_core::run`.
pub type Buffers = (Vec<f32>, Vec<u32>);

/// Loads a triangulated mesh from `.obj` or `.stl`. Multiple OBJ objects are merged.
pub fn load(path: &Path) -> Result<Buffers, IoError> {
    let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
    match ext.as_deref() {
        Some("obj") => load_obj(path),
        Some("stl") => load_stl(path),
        _ => Err(IoError::Invalid(format!(
            "{}: unsupported file type (expected .obj or .stl)",
            path.display()
        ))),
    }
}

fn load_obj(path: &Path) -> Result<Buffers, IoError> {
    // `single_index: false` keeps `indices` as pure position indices. With
    // `true`, tobj splits vertices at every UV/normal seam, which turns seams
    // into holes in the topology.
    let options =
        tobj::LoadOptions { single_index: false, triangulate: true, ignore_points: true, ignore_lines: true };
    let (models, _materials) = tobj::load_obj(path, &options).map_err(|e| match e {
        tobj::LoadError::OpenFileFailed | tobj::LoadError::ReadError => {
            IoError::Io(format!("{}: {e}", path.display()))
        }
        other => IoError::Invalid(format!("{}: {other}", path.display())),
    })?;

    let mut positions = Vec::new();
    let mut indices = Vec::new();
    for model in models {
        let offset =
            u32::try_from(positions.len() / 3).map_err(|_| IoError::Invalid("too many vertices".into()))?;
        positions.extend_from_slice(&model.mesh.positions);
        indices.extend(model.mesh.indices.iter().map(|i| i + offset));
    }
    Ok((positions, indices))
}

/// Loads an OBJ keeping its polygons (no triangulation), for measuring results.
pub fn load_poly(path: &Path) -> Result<PolyMesh, IoError> {
    let options = tobj::LoadOptions {
        single_index: false,
        triangulate: false,
        ignore_points: true,
        ignore_lines: true,
    };
    let (models, _materials) = tobj::load_obj(path, &options).map_err(|e| match e {
        tobj::LoadError::OpenFileFailed | tobj::LoadError::ReadError => {
            IoError::Io(format!("{}: {e}", path.display()))
        }
        other => IoError::Invalid(format!("{}: {other}", path.display())),
    })?;

    let mut mesh = PolyMesh { face_offsets: vec![0], ..PolyMesh::default() };
    for model in models {
        let m = &model.mesh;
        let offset =
            u32::try_from(mesh.positions.len()).map_err(|_| IoError::Invalid("too many vertices".into()))?;
        mesh.positions.extend(
            m.positions
                .chunks_exact(3)
                .map(|p| DVec3::new(f64::from(p[0]), f64::from(p[1]), f64::from(p[2]))),
        );
        // tobj leaves `face_arities` empty when every face is a triangle.
        let arities: Vec<u32> =
            if m.face_arities.is_empty() { vec![3; m.indices.len() / 3] } else { m.face_arities.clone() };
        let mut cursor = 0usize;
        for arity in arities {
            let end = cursor + arity as usize;
            let face =
                m.indices.get(cursor..end).ok_or_else(|| IoError::Invalid("truncated face list".into()))?;
            mesh.face_indices.extend(face.iter().map(|&i| i + offset));
            let total = u32::try_from(mesh.face_indices.len())
                .map_err(|_| IoError::Invalid("too many face corners".into()))?;
            mesh.face_offsets.push(total);
            cursor = end;
        }
    }
    let vertex_count = mesh.positions.len();
    if let Some(bad) = mesh.face_indices.iter().find(|&&i| i as usize >= vertex_count) {
        return Err(IoError::Invalid(format!("{}: face uses missing vertex {bad}", path.display())));
    }
    Ok(mesh)
}

fn load_stl(path: &Path) -> Result<Buffers, IoError> {
    let mut file = std::fs::File::open(path).map_err(|e| IoError::Io(format!("{}: {e}", path.display())))?;
    let mesh =
        stl_io::read_stl(&mut file).map_err(|e| IoError::Invalid(format!("{}: {e}", path.display())))?;
    let positions = mesh.vertices.iter().flat_map(|v| v.0).collect();
    let indices = mesh
        .faces
        .iter()
        .flat_map(|f| f.vertices)
        .map(|i| u32::try_from(i).map_err(|_| IoError::Invalid("vertex index exceeds u32".into())))
        .collect::<Result<_, _>>()?;
    Ok((positions, indices))
}

/// Writes a polygon mesh as OBJ (1-based indices).
pub fn write_obj(path: &Path, mesh: &PolyMesh) -> Result<(), IoError> {
    let mut text = String::with_capacity(mesh.positions.len() * 32 + mesh.face_indices.len() * 8);
    for p in &mesh.positions {
        let _ = writeln!(text, "v {} {} {}", p.x, p.y, p.z);
    }
    for face in mesh.face_offsets.windows(2) {
        let (start, end) = (face[0] as usize, face[1] as usize);
        text.push('f');
        for &v in mesh.face_indices.get(start..end).unwrap_or_default() {
            let _ = write!(text, " {}", v + 1);
        }
        text.push('\n');
    }
    std::fs::write(path, text).map_err(|e| IoError::Io(format!("{}: {e}", path.display())))
}
