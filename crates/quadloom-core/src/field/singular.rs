//! Singularities of a per-face cross field: the index at each interior vertex.

use std::collections::HashMap;
use std::f64::consts::TAU;

use glam::DVec3;

use super::frames::{Cx, FaceFrames};
use super::solve::ROSY;

/// Each vertex's index in quarters (+1 = a +¼ singularity, as at a cube
/// corner; −1 = −¼), or 0 for regular and boundary vertices.
///
/// Walking around vertex v through its faces in orientation order, the field
/// turns by ω across each shared edge relative to parallel transport
/// (ω = arg(z_g / (r_fg·z_f)) ∈ (−π, π]). Around the loop, Σω plus 4× the
/// angle defect K_v is a multiple of 2π: that multiple is the index in
/// quarters (the dual form of Knöppel et al. 2013, eq. 19). Summed over a
/// closed mesh the ω cancel pairwise, leaving 4·ΣK/2π = 4χ (Poincaré–Hopf).
/// Each edge's ω is computed once, from its lower face to its higher one, and
/// negated when a loop crosses it the other way: evaluated twice, a turn of
/// exactly π (the cross turning 45°, common between faces constrained to
/// edges 45° apart) would come out +π both times and break the cancellation.
pub fn vertex_indices(positions: &[DVec3], triangles: &[[u32; 3]], powers: &[Cx]) -> Vec<i32> {
    let frames = FaceFrames::new(positions, triangles);
    let mut by_edge: HashMap<(u32, u32), u32> = HashMap::new();
    let mut corner_of: Vec<Option<u32>> = vec![None; positions.len()];
    for (f, t) in triangles.iter().enumerate() {
        for i in 0..3 {
            by_edge.insert((t[i], t[(i + 1) % 3]), f as u32);
            corner_of[t[i] as usize].get_or_insert(f as u32);
        }
    }
    let unit = |z: Cx| if z.norm() > 0.0 { z.scale(1.0 / z.norm()) } else { Cx::ONE };
    let mut omega: HashMap<(u32, u32), f64> = HashMap::new();
    for e in super::frames::dual_edges(triangles) {
        let (lo, hi) = (e.f.min(e.g), e.f.max(e.g));
        let d = positions[e.b as usize] - positions[e.a as usize];
        let r = Cx::from_angle(ROSY * (frames.angle(hi, d) - frames.angle(lo, d)));
        let (zl, zh) = (unit(powers[lo as usize]), unit(powers[hi as usize]));
        omega.insert((lo, hi), (zh * (r * zl).conj()).arg());
    }
    (0..positions.len() as u32)
        .map(|v| {
            let Some(start) = corner_of[v as usize] else { return 0 };
            let p = positions[v as usize];
            let (mut f, mut turn, mut angles) = (start, 0.0, 0.0);
            for _ in 0..triangles.len() {
                let t = triangles[f as usize];
                let k = t.iter().position(|&x| x == v).unwrap_or(0);
                let (a, b) = (t[(k + 1) % 3], t[(k + 2) % 3]);
                let (da, db) = (positions[a as usize] - p, positions[b as usize] - p);
                angles += da.angle_between(db);
                // The next face around v shares edge v–b, traversed v → b there.
                let Some(&g) = by_edge.get(&(v, b)) else { return 0 }; // boundary
                let Some(&w) = omega.get(&(f.min(g), f.max(g))) else { return 0 };
                turn += if f < g { w } else { -w };
                f = g;
                if f == start {
                    let defect = TAU - angles;
                    return ((turn + ROSY * defect) / TAU).round() as i32;
                }
            }
            0
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::field::smooth_field;

    fn torus(rings: u32, segments: u32) -> (Vec<DVec3>, Vec<[u32; 3]>) {
        let (big, small) = (1.0, 0.4);
        let mut p = Vec::new();
        for i in 0..rings {
            let u = TAU * f64::from(i) / f64::from(rings);
            for j in 0..segments {
                let w = TAU * f64::from(j) / f64::from(segments);
                let r = big + small * w.cos();
                p.push(DVec3::new(r * u.cos(), r * u.sin(), small * w.sin()));
            }
        }
        let id = |i: u32, j: u32| (i % rings) * segments + (j % segments);
        let mut t = Vec::new();
        for i in 0..rings {
            for j in 0..segments {
                t.extend([
                    [id(i, j), id(i + 1, j), id(i + 1, j + 1)],
                    [id(i, j), id(i + 1, j + 1), id(i, j + 1)],
                ]);
            }
        }
        (p, t)
    }

    #[test]
    fn sphere_and_torus_obey_poincare_hopf() {
        let (p, t) = crate::remesh::tests::sphere_mesh(12, 24);
        let field = smooth_field(&p, &t, &[], &[], 0.0, &crate::control::Control::default()).unwrap();
        let indices = vertex_indices(&p, &t, &field.powers);
        assert_eq!(indices.iter().sum::<i32>(), 8, "sphere: χ = 2 → 8 quarters");
        // The smoothest cross field on a sphere has the cube's eight +¼ singularities.
        assert_eq!(
            indices.iter().filter(|&&k| k != 0).count(),
            8,
            "{:?}",
            indices.iter().filter(|&&k| k != 0).collect::<Vec<_>>()
        );
        let (p, t) = torus(32, 16);
        let field = smooth_field(&p, &t, &[], &[], 0.0, &crate::control::Control::default()).unwrap();
        let indices = vertex_indices(&p, &t, &field.powers);
        // Only the total is fixed: on a coarse torus the relaxed energy (|z| may
        // dip near singularities) admits ± pairs; Task 4 compares counts with
        // the oracle on real models.
        assert_eq!(indices.iter().sum::<i32>(), 0, "torus: χ = 0");
    }
}
