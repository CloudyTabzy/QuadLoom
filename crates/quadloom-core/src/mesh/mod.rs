//! Mesh types: the validated triangle input and the polygon output.

pub mod halfedge;

pub use glam::DVec3;

use crate::control::Stage;
use crate::error::Error;

/// A triangle mesh whose buffers have been checked: every index is in range
/// and every position is finite. Topological problems (non-manifold edges,
/// degenerate faces, …) are *not* rejected here; later stages report or repair them.
#[derive(Clone, Debug, PartialEq)]
pub struct InputMesh {
    positions: Vec<DVec3>,
    triangles: Vec<[u32; 3]>,
}

impl InputMesh {
    /// Builds a mesh from flat `xyz` positions and flat triangle indices.
    pub fn from_buffers(positions: &[f32], indices: &[u32]) -> Result<Self, Error> {
        let stage = Stage::Validate;
        if !positions.len().is_multiple_of(3) {
            return Err(Error::invalid_input(
                stage,
                format!("positions length {} is not a multiple of 3", positions.len()),
            ));
        }
        if !indices.len().is_multiple_of(3) {
            return Err(Error::invalid_input(
                stage,
                format!("indices length {} is not a multiple of 3", indices.len()),
            ));
        }
        if indices.is_empty() {
            return Err(Error::invalid_input(stage, "mesh has no faces"));
        }

        let positions: Vec<DVec3> = positions
            .as_chunks::<3>()
            .0
            .iter()
            .map(|&[x, y, z]| DVec3::new(f64::from(x), f64::from(y), f64::from(z)))
            .collect();
        if let Some(v) = positions.iter().position(|p| !p.is_finite()) {
            return Err(Error::invalid_input(stage, format!("vertex {v} has a non-finite position"))
                .with_hint("check the mesh for NaN or infinite coordinates"));
        }

        let vertex_count = positions.len();
        let triangles: Vec<[u32; 3]> = indices.as_chunks::<3>().0.to_vec();
        for (face, tri) in triangles.iter().enumerate() {
            if let Some(&bad) = tri.iter().find(|&&i| i as usize >= vertex_count) {
                return Err(Error::invalid_input(
                    stage,
                    format!("face {face} uses index {bad}, but the mesh has {vertex_count} vertices"),
                ));
            }
        }

        Ok(Self { positions, triangles })
    }

    pub fn positions(&self) -> &[DVec3] {
        &self.positions
    }

    pub fn triangles(&self) -> &[[u32; 3]] {
        &self.triangles
    }

    pub fn vertex_count(&self) -> usize {
        self.positions.len()
    }

    pub fn face_count(&self) -> usize {
        self.triangles.len()
    }
}

/// A polygon mesh in compressed-row form: face `f` uses
/// `face_indices[face_offsets[f]..face_offsets[f + 1]]`. This maps directly onto
/// Blender's polygon/loop arrays.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PolyMesh {
    pub positions: Vec<DVec3>,
    pub face_offsets: Vec<u32>,
    pub face_indices: Vec<u32>,
}

impl PolyMesh {
    /// Re-emits a triangle mesh as polygons (one triangle per face).
    pub fn from_triangles(mesh: &InputMesh) -> Self {
        let face_indices: Vec<u32> = mesh.triangles.iter().flatten().copied().collect();
        let face_offsets: Vec<u32> = (0..=mesh.triangles.len() as u32).map(|f| f * 3).collect();
        Self { positions: mesh.positions.clone(), face_offsets, face_indices }
    }

    pub fn face_count(&self) -> usize {
        self.face_offsets.len().saturating_sub(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorKind;

    fn tri() -> (Vec<f32>, Vec<u32>) {
        (vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0], vec![0, 1, 2])
    }

    #[test]
    fn from_buffers_rejects_position_length_not_multiple_of_3() {
        let err = InputMesh::from_buffers(&[0.0; 4], &[]).unwrap_err();
        assert_eq!(err.kind, ErrorKind::InvalidInput);
        assert!(err.message.contains("positions"), "{}", err.message);
    }

    #[test]
    fn from_buffers_rejects_index_length_not_multiple_of_3() {
        let (p, _) = tri();
        let err = InputMesh::from_buffers(&p, &[0, 1]).unwrap_err();
        assert_eq!(err.kind, ErrorKind::InvalidInput);
        assert!(err.message.contains("indices"), "{}", err.message);
    }

    #[test]
    fn from_buffers_rejects_out_of_range_index() {
        let (p, _) = tri();
        let err = InputMesh::from_buffers(&p, &[0, 1, 3]).unwrap_err();
        assert_eq!(err.kind, ErrorKind::InvalidInput);
        assert!(err.message.contains("index 3"), "{}", err.message);
        assert!(err.message.contains("face 0"), "{}", err.message);
    }

    #[test]
    fn from_buffers_rejects_non_finite_position() {
        let (mut p, i) = tri();
        p[4] = f32::NAN;
        let err = InputMesh::from_buffers(&p, &i).unwrap_err();
        assert_eq!(err.kind, ErrorKind::InvalidInput);
        assert!(err.message.contains("vertex 1"), "{}", err.message);
    }

    #[test]
    fn from_buffers_rejects_empty() {
        let (p, _) = tri();
        let err = InputMesh::from_buffers(&p, &[]).unwrap_err();
        assert_eq!(err.kind, ErrorKind::InvalidInput);
        assert_eq!(err.message, "mesh has no faces");
    }

    #[test]
    fn from_buffers_accepts_single_triangle() {
        let (p, i) = tri();
        let mesh = InputMesh::from_buffers(&p, &i).unwrap();
        assert_eq!(mesh.vertex_count(), 3);
        assert_eq!(mesh.face_count(), 1);
        assert_eq!(mesh.positions()[1], DVec3::new(1.0, 0.0, 0.0));
        assert_eq!(mesh.triangles(), &[[0, 1, 2]]);
    }

    #[test]
    fn polymesh_from_triangles_roundtrips() {
        let p = [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0];
        let i = [0, 1, 2, 1, 3, 2];
        let mesh = InputMesh::from_buffers(&p, &i).unwrap();
        let poly = PolyMesh::from_triangles(&mesh);
        assert_eq!(poly.face_offsets, vec![0, 3, 6]);
        assert_eq!(poly.face_indices, vec![0, 1, 2, 1, 3, 2]);
        assert_eq!(poly.face_count(), 2);
        assert_eq!(poly.positions.len(), 4);
    }
}
