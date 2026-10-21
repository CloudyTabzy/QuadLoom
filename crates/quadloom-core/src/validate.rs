//! Stage 0a: measure what's wrong with the input before anything touches it.

use crate::control::{Control, Stage};
use crate::error::Error;
use crate::mesh::InputMesh;
use crate::mesh::halfedge::HalfEdges;
use crate::unionfind::UnionFind;

/// Relative area below which a triangle counts as degenerate, as a fraction of
/// the squared bounding-box diagonal.
const DEGENERATE_AREA_RATIO: f64 = 1e-12;

/// What the validate stage found. Nothing here is fatal on its own; later
/// stages decide what to repair and what to refuse.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct ValidationReport {
    pub vertices: usize,
    pub faces: usize,
    pub boundary_edges: usize,
    pub non_manifold_edges: usize,
    /// Faces with (near-)zero area or a repeated vertex.
    pub degenerate_faces: usize,
    /// Connected pieces, counting vertices that share faces.
    pub components: usize,
}

pub fn validate(mesh: &InputMesh, control: &Control) -> Result<ValidationReport, Error> {
    let stage = Stage::Validate;
    let topology = HalfEdges::build(mesh.triangles(), mesh.vertex_count());
    control.checkpoint(stage)?;

    let threshold = DEGENERATE_AREA_RATIO * bbox_diagonal_squared(mesh);
    let positions = mesh.positions();
    let mut components = UnionFind::new(mesh.vertex_count());
    let mut degenerate_faces = 0;
    let mut ticker = control.ticker(stage, mesh.face_count());
    for (f, &[a, b, c]) in mesh.triangles().iter().enumerate() {
        ticker.tick(f)?;
        let (pa, pb, pc) = (positions[a as usize], positions[b as usize], positions[c as usize]);
        let doubled_area = (pb - pa).cross(pc - pa).length();
        if a == b || b == c || a == c || 0.5 * doubled_area <= threshold {
            degenerate_faces += 1;
        }
        components.union(a, b);
        components.union(b, c);
    }

    let mut used = vec![false; mesh.vertex_count()];
    for &v in mesh.triangles().iter().flatten() {
        used[v as usize] = true;
    }
    let component_count =
        (0..mesh.vertex_count() as u32).filter(|&v| used[v as usize] && components.find(v) == v).count();

    control.set_progress(stage, 1.0);
    Ok(ValidationReport {
        vertices: mesh.vertex_count(),
        faces: mesh.face_count(),
        boundary_edges: topology.boundary_edge_count(),
        non_manifold_edges: topology.non_manifold_edge_count(),
        degenerate_faces,
        components: component_count,
    })
}

fn bbox_diagonal_squared(mesh: &InputMesh) -> f64 {
    let mut points = mesh.positions().iter().copied();
    let Some(first) = points.next() else { return 0.0 };
    let (min, max) = points.fold((first, first), |(lo, hi), p| (lo.min(p), hi.max(p)));
    (max - min).length_squared()
}
