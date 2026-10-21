//! Removing faces that carry no surface: repeated corners, zero area, duplicates.

pub use glam::DVec3;

/// Surviving faces (in their original order) and what was removed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Degenerate {
    pub triangles: Vec<[u32; 3]>,
    /// Input index of each surviving face.
    pub kept: Vec<u32>,
    pub removed_repeated: usize,
    pub removed_zero_area: usize,
    pub removed_duplicates: usize,
}

/// Drops faces with a repeated corner, faces with area `<= area_eps`, and every
/// later copy of a face that uses the same three vertices (in any order).
pub fn remove(positions: &[DVec3], triangles: &[[u32; 3]], area_eps: f64) -> Degenerate {
    let mut result = Degenerate {
        triangles: Vec::with_capacity(triangles.len()),
        kept: Vec::with_capacity(triangles.len()),
        removed_repeated: 0,
        removed_zero_area: 0,
        removed_duplicates: 0,
    };

    // The first face of each vertex set survives; later copies are duplicates.
    let mut keyed: Vec<([u32; 3], usize)> = triangles
        .iter()
        .enumerate()
        .map(|(f, t)| {
            let mut key = *t;
            key.sort_unstable();
            (key, f)
        })
        .collect();
    keyed.sort_unstable();
    let mut duplicate = vec![false; triangles.len()];
    for group in keyed.chunk_by(|a, b| a.0 == b.0) {
        for &(_, f) in &group[1..] {
            duplicate[f] = true;
        }
    }

    for (f, &[a, b, c]) in triangles.iter().enumerate() {
        if a == b || b == c || a == c {
            result.removed_repeated += 1;
        } else if duplicate[f] {
            result.removed_duplicates += 1;
        } else {
            let (pa, pb, pc) = (positions[a as usize], positions[b as usize], positions[c as usize]);
            if 0.5 * (pb - pa).cross(pc - pa).length() <= area_eps {
                result.removed_zero_area += 1;
            } else {
                result.triangles.push([a, b, c]);
                result.kept.push(f as u32);
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f64, y: f64, z: f64) -> DVec3 {
        DVec3::new(x, y, z)
    }

    #[test]
    fn degenerate_removes_repeated_and_zero_area() {
        let positions = [v(0., 0., 0.), v(1., 0., 0.), v(0., 1., 0.), v(2., 0., 0.)];
        let triangles = [[0, 1, 2], [0, 0, 1], [0, 1, 3]];
        let d = remove(&positions, &triangles, 1e-12);
        assert_eq!(d.triangles, vec![[0, 1, 2]]);
        assert_eq!(d.removed_repeated, 1);
        assert_eq!(d.removed_zero_area, 1);
        assert_eq!(d.removed_duplicates, 0);
    }

    #[test]
    fn duplicate_faces_removed_regardless_of_rotation_or_winding() {
        let positions = [v(0., 0., 0.), v(1., 0., 0.), v(0., 1., 0.)];
        let triangles = [[0, 1, 2], [1, 2, 0], [2, 1, 0]];
        let d = remove(&positions, &triangles, 1e-12);
        assert_eq!(d.triangles, vec![[0, 1, 2]]);
        assert_eq!(d.removed_duplicates, 2);
    }

    #[test]
    fn healthy_mesh_is_untouched() {
        let positions = [v(0., 0., 0.), v(1., 0., 0.), v(0., 1., 0.), v(1., 1., 0.)];
        let triangles = [[0, 1, 2], [1, 3, 2]];
        let d = remove(&positions, &triangles, 1e-12);
        assert_eq!(d.triangles, triangles.to_vec());
        assert_eq!(d.removed_repeated + d.removed_zero_area + d.removed_duplicates, 0);
    }
}
