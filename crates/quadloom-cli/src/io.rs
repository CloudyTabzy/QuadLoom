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

/// Writes per-face cross directions in QuadWild's `.rosy` format: the face
/// count, the symmetry (4), then one direction per line.
pub fn write_rosy(path: &Path, directions: &[DVec3]) -> Result<(), IoError> {
    let mut text = format!("{}\n4\n", directions.len());
    for d in directions {
        let _ = writeln!(text, "{} {} {}", d.x, d.y, d.z);
    }
    std::fs::write(path, text).map_err(|e| IoError::Io(format!("{}: {e}", path.display())))
}

/// Writes feature edges of a triangle mesh in QuadWild's `.sharp` format:
/// the count, then `kind,face,edge` per feature edge side (kind 0).
pub fn write_sharp(path: &Path, mesh: &PolyMesh, features: &[[u32; 2]]) -> Result<(), IoError> {
    let wanted: hashbrown::HashSet<[u32; 2]> = features.iter().map(|&[a, b]| [a.min(b), a.max(b)]).collect();
    let mut lines = Vec::new();
    for (f, face) in mesh.face_offsets.windows(2).enumerate() {
        let corners = mesh.face_indices.get(face[0] as usize..face[1] as usize).unwrap_or_default();
        if corners.len() != 3 {
            continue;
        }
        for e in 0..3 {
            let (a, b) = (corners[e], corners[(e + 1) % 3]);
            if wanted.contains(&[a.min(b), a.max(b)]) {
                lines.push(format!("0,{f},{e}"));
            }
        }
    }
    let text = format!(
        "{}
{}",
        lines.len(),
        lines
            .iter()
            .map(|l| format!(
                "{l}
"
            ))
            .collect::<String>()
    );
    std::fs::write(path, text).map_err(|e| IoError::Io(format!("{}: {e}", path.display())))
}

/// Reads a `.rosy` file for a mesh with `faces` triangles: an integer face
/// count, the symmetry (must be 4), then three finite numbers per face.
pub fn read_rosy(path: &Path, faces: usize) -> Result<Vec<DVec3>, IoError> {
    let invalid = |what: &str| IoError::Invalid(format!("{}: {what}", path.display()));
    let text = std::fs::read_to_string(path).map_err(|e| IoError::Io(format!("{}: {e}", path.display())))?;
    let mut tokens = text.split_whitespace();
    let count: usize = tokens
        .next()
        .and_then(|t| t.parse().ok())
        .ok_or_else(|| invalid("expected an integer face count"))?;
    let symmetry: u32 =
        tokens.next().and_then(|t| t.parse().ok()).ok_or_else(|| invalid("expected the symmetry"))?;
    if symmetry != 4 {
        return Err(invalid(&format!("a {symmetry}-RoSy field; only cross fields (4) are supported")));
    }
    if count != faces {
        return Err(invalid(&format!("{count} directions for {faces} faces")));
    }
    let mut number = || -> Result<f64, IoError> {
        tokens
            .next()
            .and_then(|t| t.parse::<f64>().ok())
            .filter(|x| x.is_finite())
            .ok_or_else(|| invalid("expected a finite direction component"))
    };
    (0..count).map(|_| Ok(DVec3::new(number()?, number()?, number()?))).collect()
}

/// Loads an OBJ's vertices in file order (polygons fan-triangulated), so
/// vertex ids match files that refer to them (e.g. QuadWild's `.corners`).
/// `tobj` renumbers vertices by first use, which breaks that.
pub fn load_obj_ordered(path: &Path) -> Result<(Vec<DVec3>, Vec<[u32; 3]>), IoError> {
    let invalid = |what: String| IoError::Invalid(format!("{}: {what}", path.display()));
    let text = read_tokens(path)?;
    let (mut positions, mut triangles) = (Vec::new(), Vec::new());
    for (n, line) in text.lines().enumerate() {
        let mut fields = line.split_whitespace();
        match fields.next() {
            Some("v") => {
                let c: Vec<f64> = fields.take(3).filter_map(|t| t.parse().ok()).collect();
                let [x, y, z] = c[..] else { return Err(invalid(format!("line {}: bad vertex", n + 1))) };
                positions.push(DVec3::new(x, y, z));
            }
            Some("f") => {
                let count = positions.len() as i64;
                let ids: Vec<u32> = fields
                    .map(|t| {
                        let i: i64 = t.split('/').next()?.parse().ok()?;
                        let i = if i < 0 { count + i } else { i - 1 };
                        u32::try_from(i).ok().filter(|&i| (i as usize) < positions.len())
                    })
                    .collect::<Option<_>>()
                    .ok_or_else(|| invalid(format!("line {}: bad face", n + 1)))?;
                for k in 1..ids.len().saturating_sub(1) {
                    triangles.push([ids[0], ids[k], ids[k + 1]]);
                }
            }
            _ => {}
        }
    }
    Ok((positions, triangles))
}

fn read_tokens(path: &Path) -> Result<String, IoError> {
    std::fs::read_to_string(path).map_err(|e| IoError::Io(format!("{}: {e}", path.display())))
}

/// Reads QuadWild's `.sharp` features of a triangle mesh: a count, then one
/// `kind,face,edge` line per feature edge (edge `e` of a face runs from its
/// vertex `e` to vertex `e + 1`). Returns sorted vertex pairs.
pub fn read_sharp(path: &Path, triangles: &[[u32; 3]]) -> Result<Vec<[u32; 2]>, IoError> {
    let invalid = |what: &str| IoError::Invalid(format!("{}: {what}", path.display()));
    let text = read_tokens(path)?;
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    let count: usize =
        lines.next().and_then(|t| t.parse().ok()).ok_or_else(|| invalid("expected an integer edge count"))?;
    let mut edges = Vec::with_capacity(count);
    for _ in 0..count {
        let line = lines.next().ok_or_else(|| invalid("fewer edges than the count says"))?;
        let fields: Vec<usize> = line
            .split(',')
            .map(|t| t.trim().parse().ok())
            .collect::<Option<_>>()
            .ok_or_else(|| invalid("expected kind,face,edge"))?;
        let [_, face, edge] = fields[..] else { return Err(invalid("expected kind,face,edge")) };
        let t =
            triangles.get(face).filter(|_| edge < 3).ok_or_else(|| invalid("face or edge out of range"))?;
        let (a, b) = (t[edge], t[(edge + 1) % 3]);
        edges.push([a.min(b), a.max(b)]);
    }
    edges.sort_unstable();
    edges.dedup();
    Ok(edges)
}

/// Reads a `.patch` file (a face count, then one patch id per face).
pub fn read_patch(path: &Path, faces: usize) -> Result<Vec<u32>, IoError> {
    let invalid = |what: &str| IoError::Invalid(format!("{}: {what}", path.display()));
    let text = read_tokens(path)?;
    let mut tokens = text.split_whitespace().map(|t| t.parse::<i64>().ok());
    let count = tokens.next().flatten().ok_or_else(|| invalid("expected a face count"))?;
    if count as usize != faces {
        return Err(invalid(&format!("{count} patch ids for {faces} faces")));
    }
    (0..faces)
        .map(|_| {
            tokens
                .next()
                .flatten()
                .and_then(|p| u32::try_from(p).ok())
                .ok_or_else(|| invalid("expected a patch id"))
        })
        .collect()
}

/// Reads a `.corners` file (a patch count, then per patch its corner count
/// and corner vertex ids).
pub fn read_corners(path: &Path) -> Result<Vec<Vec<u32>>, IoError> {
    let invalid = |what: &str| IoError::Invalid(format!("{}: {what}", path.display()));
    let text = read_tokens(path)?;
    let mut tokens = text.split_whitespace().map(|t| t.parse::<u32>().ok());
    let mut next = || tokens.next().flatten().ok_or_else(|| invalid("expected an integer"));
    let patches = next()?;
    (0..patches)
        .map(|_| {
            let n = next()?;
            (0..n).map(|_| next()).collect()
        })
        .collect()
}

/// Writes a layout in QuadWild's stage formats: `<prefix>_p0.obj` (the
/// mesh), `.patch` (patch id per face), `.corners`, `.feature` (`face,edge`
/// per feature edge) and `.c_feature` (feature vertices that aren't on a
/// single feature line: ends and junctions).
pub fn write_layout(prefix: &Path, layout: &quadloom_core::TraceOutput) -> Result<(), IoError> {
    let path = |ext: &str| {
        let mut name = prefix.as_os_str().to_owned();
        name.push(format!("_p0.{ext}"));
        std::path::PathBuf::from(name)
    };
    let write = |p: std::path::PathBuf, text: String| {
        std::fs::write(&p, text).map_err(|e| IoError::Io(format!("{}: {e}", p.display())))
    };
    let mesh = &layout.mesh;
    let poly = PolyMesh {
        positions: mesh.positions.clone(),
        face_offsets: (0..=mesh.triangles.len() as u32).map(|f| 3 * f).collect(),
        face_indices: mesh.triangles.iter().flatten().copied().collect(),
    };
    write_obj(&path("obj"), &poly)?;
    let mut text = format!("{}\n", layout.patch_of_face.len());
    for p in &layout.patch_of_face {
        let _ = writeln!(text, "{p}");
    }
    write(path("patch"), text)?;
    let mut text = format!("{}\n", layout.patches.len());
    for patch in &layout.patches {
        let _ = writeln!(text, "{}", patch.corners.len());
        for c in &patch.corners {
            let _ = writeln!(text, "{c}");
        }
    }
    write(path("corners"), text)?;
    let features: hashbrown::HashSet<[u32; 2]> = mesh.feature_edges.iter().copied().collect();
    let mut lines = Vec::new();
    for (f, t) in mesh.triangles.iter().enumerate() {
        for e in 0..3 {
            let (a, b) = (t[e], t[(e + 1) % 3]);
            if features.contains(&[a.min(b), a.max(b)]) {
                lines.push(format!("{f},{e}"));
            }
        }
    }
    write(
        path("feature"),
        format!("{}\n{}", lines.len(), lines.iter().map(|l| format!("{l}\n")).collect::<String>()),
    )?;
    let mut degree = vec![0u32; mesh.positions.len()];
    for &[a, b] in &mesh.feature_edges {
        degree[a as usize] += 1;
        degree[b as usize] += 1;
    }
    let corners: Vec<usize> =
        degree.iter().enumerate().filter(|&(_, &d)| d != 0 && d != 2).map(|(v, _)| v).collect();
    write(
        path("c_feature"),
        format!("{}\n{}", corners.len(), corners.iter().map(|v| format!("{v}\n")).collect::<String>()),
    )
}
