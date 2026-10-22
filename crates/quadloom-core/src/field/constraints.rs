//! What the cross field aligns to: feature lines and borders exactly, and
//! principal curvature softly.

use hashbrown::{HashMap, HashSet};

use glam::DVec3;

use super::frames::{Cx, FaceFrames, dual_edges};
use super::solve::{Hard, ROSY, Soft};

/// The oracle's test (`TriangleMesh::SufficientFeatures`): features alone
/// guide the field when their total length exceeds `sharp_factor` × √area.
pub fn sufficient_features(feature_length: f64, area: f64, sharp_factor: f64) -> bool {
    area > 0.0 && feature_length / area.sqrt() > sharp_factor
}

/// Constrained edges of one face whose crosses lie within this angle (°) of
/// each other are averaged; otherwise the longest edge decides.
const AGREEMENT_DEG: f64 = 1.0;

/// Every face beside a feature edge or a border edge, aligned to it (the
/// oracle's feature constraints plus `align_borders`). A face with several
/// such edges follows the longest (ties: the first in corner order), so it
/// lies exactly along one of its features, as tracing needs; edges at right
/// angles (or within [`AGREEMENT_DEG`] of it) describe the same cross and are
/// averaged.
pub fn hard_constraints(
    positions: &[DVec3],
    triangles: &[[u32; 3]],
    feature_edges: &[[u32; 2]],
) -> Vec<Hard> {
    let features: HashSet<(u32, u32)> = feature_edges.iter().map(|&[a, b]| (a.min(b), a.max(b))).collect();
    let mut uses: HashMap<(u32, u32), u32> = HashMap::new();
    for t in triangles {
        for i in 0..3 {
            let (a, b) = (t[i], t[(i + 1) % 3]);
            *uses.entry((a.min(b), a.max(b))).or_default() += 1;
        }
    }
    let frames = FaceFrames::new(positions, triangles);
    let mut hard = Vec::new();
    for (f, t) in triangles.iter().enumerate() {
        let f = f as u32;
        if frames.is_degenerate(f) {
            continue;
        }
        let constrained: Vec<DVec3> = (0..3)
            .map(|i| (t[i], t[(i + 1) % 3]))
            .filter(|&(a, b)| {
                let key = (a.min(b), a.max(b));
                features.contains(&key) || uses.get(&key) == Some(&1)
            })
            .map(|(a, b)| positions[b as usize] - positions[a as usize])
            .collect();
        let Some(longest) =
            constrained.iter().copied().reduce(|x, y| if y.length() > x.length() { y } else { x })
        else {
            continue;
        };
        let reference = frames.power(f, longest, ROSY);
        let tolerance = (ROSY * AGREEMENT_DEG).to_radians().cos();
        let agreeing: Vec<DVec3> = constrained
            .iter()
            .copied()
            .filter(|&e| {
                let z = frames.power(f, e, ROSY);
                z.re * reference.re + z.im * reference.im >= tolerance
            })
            .collect();
        let sum = agreeing.iter().fold(Cx::ZERO, |acc, &e| acc + frames.power(f, e, ROSY).scale(e.length()));
        hard.push(Hard { face: f, direction: frames.direction(f, sum.arg() / ROSY) });
    }
    hard
}

/// Per-face principal-curvature alignment targets (Knöppel et al. 2013,
/// §6.1.2, in dual form): the trace-free shape operator from the normal
/// cycle, q̃_f = Σ over f's interior edges of θ_e·|e|·e^{2iφ_e}, where θ_e is
/// the signed dihedral angle (positive where convex) and φ_e the edge's angle
/// in f's frame. q̃_f is a line field along a principal direction; its square
/// is the cross target (minimum and maximum directions give the same cross).
/// The weight is |q̃_f| (∝ (κ₁ − κ₂)²) over its 95th percentile, clamped to
/// [0, 1], so umbilic and flat regions are left to smoothness.
pub fn curvature_alignment(positions: &[DVec3], triangles: &[[u32; 3]]) -> Vec<Soft> {
    let frames = FaceFrames::new(positions, triangles);
    let mut q = vec![Cx::ZERO; triangles.len()];
    for e in dual_edges(triangles) {
        let (pa, pb) = (positions[e.a as usize], positions[e.b as usize]);
        let d = pb - pa;
        let (nf, ng) = (frames.normal[e.f as usize], frames.normal[e.g as usize]);
        if nf == DVec3::ZERO || ng == DVec3::ZERO {
            continue;
        }
        let opposite = triangles[e.g as usize].into_iter().find(|&v| v != e.a && v != e.b).unwrap_or(e.a);
        let convex = (positions[opposite as usize] - pa).dot(nf) < 0.0;
        let theta = nf.dot(ng).clamp(-1.0, 1.0).acos() * if convex { 1.0 } else { -1.0 };
        let weight = theta * d.length();
        for face in [e.f, e.g] {
            q[face as usize] = q[face as usize] + Cx::from_angle(2.0 * frames.angle(face, d)).scale(weight);
        }
    }
    let mut magnitudes: Vec<f64> = q.iter().map(|z| z.norm()).filter(|&m| m > 0.0).collect();
    magnitudes.sort_by(f64::total_cmp);
    let scale = magnitudes.get(magnitudes.len().saturating_sub(1) * 95 / 100).copied().unwrap_or(0.0);
    q.iter()
        .enumerate()
        .filter(|(_, z)| z.norm() > 0.0 && scale > 0.0)
        .map(|(f, &z)| {
            let line = z.scale(1.0 / z.norm());
            Soft { face: f as u32, target: line * line, weight: (z.norm() / scale).min(1.0) }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::field::smooth_field;

    /// An open cylinder, radius 1, height 2, `rings` × `segments` quads.
    fn cylinder(rings: u32, segments: u32) -> (Vec<DVec3>, Vec<[u32; 3]>) {
        let mut p = Vec::new();
        for i in 0..=rings {
            for j in 0..segments {
                let a = std::f64::consts::TAU * f64::from(j) / f64::from(segments);
                p.push(DVec3::new(a.cos(), a.sin(), 2.0 * f64::from(i) / f64::from(rings)));
            }
        }
        let mut t = Vec::new();
        for i in 0..rings {
            for j in 0..segments {
                let (a, b) = (i * segments + j, i * segments + (j + 1) % segments);
                t.extend([[a, b, b + segments], [a, b + segments, a + segments]]);
            }
        }
        (p, t)
    }

    /// A unit cube, each side an `n`×`n` grid, and its 12 edges as features.
    fn cube(n: u32) -> (Vec<DVec3>, Vec<[u32; 3]>, Vec<[u32; 2]>) {
        let mut p: Vec<DVec3> = Vec::new();
        let mut index: HashMap<[i64; 3], u32> = HashMap::new();
        let mut id = |q: DVec3, p: &mut Vec<DVec3>| {
            let key = (q * f64::from(n)).round().as_i64vec3().to_array();
            *index.entry(key).or_insert_with(|| {
                p.push(q);
                (p.len() - 1) as u32
            })
        };
        let mut t = Vec::new();
        // (origin, u, v): u × v points outward.
        let sides = [
            (DVec3::ZERO, DVec3::Y, DVec3::X),
            (DVec3::Z, DVec3::X, DVec3::Y),
            (DVec3::ZERO, DVec3::X, DVec3::Z),
            (DVec3::Y, DVec3::Z, DVec3::X),
            (DVec3::ZERO, DVec3::Z, DVec3::Y),
            (DVec3::X, DVec3::Y, DVec3::Z),
        ];
        for (o, u, v) in sides {
            let s = 1.0 / f64::from(n);
            for i in 0..n {
                for j in 0..n {
                    let corner = |a: u32, b: u32| o + u * (f64::from(a) * s) + v * (f64::from(b) * s);
                    let [q00, q10, q11, q01] =
                        [corner(i, j), corner(i + 1, j), corner(i + 1, j + 1), corner(i, j + 1)]
                            .map(|q| id(q, &mut p));
                    t.extend([[q00, q10, q11], [q00, q11, q01]]);
                }
            }
        }
        let on_edge = |q: DVec3| {
            q.to_array().iter().filter(|&&c| c.abs() < 1e-9 || (c - 1.0).abs() < 1e-9).count() >= 2
        };
        let mut features = Vec::new();
        for tri in &t {
            for i in 0..3 {
                let (a, b) = (tri[i], tri[(i + 1) % 3]);
                let (pa, pb) = (p[a as usize], p[b as usize]);
                let shared_axis = (0..3)
                    .filter(|&k| {
                        (pa[k] - pb[k]).abs() < 1e-9 && (pa[k].abs() < 1e-9 || (pa[k] - 1.0).abs() < 1e-9)
                    })
                    .count();
                if a < b && on_edge(pa) && on_edge(pb) && shared_axis == 2 {
                    features.push([a, b]);
                }
            }
        }
        (p, t, features)
    }

    /// Smallest angle between `d` and any of the three axes, in degrees,
    /// modulo the cross's 90° symmetry within the face plane.
    fn axis_misalignment(d: DVec3) -> f64 {
        let c = d.abs().max_element().clamp(-1.0, 1.0);
        c.acos().to_degrees()
    }

    #[test]
    fn cube_field_aligns_with_edges() {
        let (p, t, features) = cube(6);
        assert!(!features.is_empty());
        let hard = hard_constraints(&p, &t, &features);
        let field = smooth_field(&p, &t, &hard, &[], 0.0, &crate::control::Control::default()).unwrap();
        for d in &field.directions {
            assert!(axis_misalignment(*d) < 1.0, "{d:?}");
        }
    }

    #[test]
    fn cylinder_follows_curvature() {
        let (p, t) = cylinder(40, 48);
        let soft = curvature_alignment(&p, &t);
        let field = smooth_field(&p, &t, &[], &soft, 0.3, &crate::control::Control::default()).unwrap();
        for (f, d) in field.directions.iter().enumerate() {
            let [a, b, c] = t[f].map(|v| p[v as usize]);
            let z = (a.z + b.z + c.z) / 3.0;
            if z < 0.3 || z > 1.7 {
                continue; // near the open rims
            }
            // Along the axis, or around it (perpendicular to the axis).
            let along = d.z.abs();
            assert!(along > 5f64.to_radians().cos() || along < 5f64.to_radians().sin(), "face {f}: {d:?}");
        }
    }

    #[test]
    fn edges_at_right_angles_agree() {
        // A unit square of two triangles: each has two border sides at right
        // angles (and the interior diagonal), so one cross fits both sides.
        let p = [DVec3::ZERO, DVec3::X, DVec3::new(1.0, 1.0, 0.0), DVec3::Y];
        let hard = hard_constraints(&p, &[[0, 1, 2], [0, 2, 3]], &[]);
        assert_eq!(hard.len(), 2);
        for h in hard {
            let d = h.direction;
            assert!(d.x.abs() > 1.0 - 1e-9 || d.y.abs() > 1.0 - 1e-9, "{d:?}");
        }
    }

    #[test]
    fn conflicting_edges_follow_the_longest() {
        // Two border sides 20° apart: the face follows the longer one exactly
        // (averaging pinned it 10° off both).
        let a = 20f64.to_radians();
        let p = [DVec3::ZERO, DVec3::new(3.0, 0.0, 0.0), DVec3::new(a.cos(), a.sin(), 0.0)];
        let hard = hard_constraints(&p, &[[0, 1, 2]], &[[0, 1], [0, 2]]);
        assert_eq!(hard.len(), 1);
        let d = hard[0].direction;
        let misalignment = d.x.abs().max(d.y.abs()).acos().to_degrees();
        assert!(misalignment < 1e-6, "{d:?} is {misalignment}° off the longest edge");
    }

    #[test]
    fn auto_alignment_follows_the_oracle_rule() {
        assert!(sufficient_features(70.0, 100.0, 6.0));
        assert!(!sufficient_features(50.0, 100.0, 6.0));
        assert!(!sufficient_features(1.0, 0.0, 6.0));
    }
}
