//! Measuring any layout the same way: ours, or the oracle's `_p0` files.

use hashbrown::HashMap;

use glam::DVec3;

use super::patches::cc_margin;

/// Layout measurements that need only the mesh, per-face patch ids and each
/// patch's corners (no field), so two layouts compare like for like.
#[derive(Clone, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct LayoutStats {
    pub faces: usize,
    pub patches: usize,
    /// Patches by corner count (index 8: 8 or more).
    pub sides_histogram: [usize; 9],
    /// Patches that aren't disks (more than one boundary loop, or χ ≠ 1).
    pub non_disk: usize,
    /// Patches with fewer than 3 corners.
    pub low_sides: usize,
    /// Patches with more than `max_sides` corners.
    pub high_sides: usize,
    /// Disks with 3 to `max_sides` corners.
    pub valid: usize,
    /// Valid patches whose straight side lengths pass the CC-ability test
    /// (4-sided ones always do, as when matching valence).
    pub cc_able: usize,
    /// Faces with no patch (patch id out of range).
    pub unassigned_faces: usize,
}

/// Measures a layout. `corners[p]` lists patch `p`'s corners (any order);
/// side lengths are the boundary polyline lengths between consecutive
/// corners; the CC-ability tolerance is `cc_tolerance` × the mean edge.
pub fn layout_stats(
    positions: &[DVec3],
    triangles: &[[u32; 3]],
    patch_of_face: &[u32],
    corners: &[Vec<u32>],
    max_sides: usize,
    cc_tolerance: f64,
) -> LayoutStats {
    let mut stats = LayoutStats { faces: triangles.len(), patches: corners.len(), ..LayoutStats::default() };
    let (mut sum, mut count) = (0.0, 0usize);
    for t in triangles {
        for i in 0..3 {
            sum += (positions[t[i] as usize] - positions[t[(i + 1) % 3] as usize]).length();
            count += 1;
        }
    }
    let tolerance = cc_tolerance * if count > 0 { sum / count as f64 } else { 0.0 };
    let mut faces_of: Vec<Vec<u32>> = vec![Vec::new(); corners.len()];
    for (f, &p) in patch_of_face.iter().enumerate().take(triangles.len()) {
        match faces_of.get_mut(p as usize) {
            Some(list) => list.push(f as u32),
            None => stats.unassigned_faces += 1,
        }
    }
    // Which faces use each directed edge, to tell patch borders.
    let mut owner: HashMap<(u32, u32), u32> = HashMap::with_capacity(3 * triangles.len());
    for (f, t) in triangles.iter().enumerate() {
        for i in 0..3 {
            owner.insert((t[i], t[(i + 1) % 3]), patch_of_face.get(f).copied().unwrap_or(u32::MAX));
        }
    }
    for (p, faces) in faces_of.iter().enumerate() {
        let p = p as u32;
        let n = corners[p as usize].len();
        stats.sides_histogram[n.min(8)] += 1;
        // Border half-edges: the patch on their left, something else on the right.
        let mut border: HashMap<u32, Vec<u32>> = HashMap::new();
        let mut border_count = 0usize;
        let mut vertices: Vec<u32> = Vec::new();
        for &f in faces {
            let t = triangles[f as usize];
            vertices.extend_from_slice(&t);
            for i in 0..3 {
                let (a, b) = (t[i], t[(i + 1) % 3]);
                if owner.get(&(b, a)) != Some(&p) {
                    border.entry(a).or_default().push(b);
                    border_count += 1;
                }
            }
        }
        vertices.sort_unstable();
        vertices.dedup();
        let mut edges: Vec<(u32, u32)> = faces
            .iter()
            .flat_map(|&f| {
                let t = triangles[f as usize];
                (0..3).map(move |i| (t[i].min(t[(i + 1) % 3]), t[i].max(t[(i + 1) % 3])))
            })
            .collect();
        edges.sort_unstable();
        edges.dedup();
        let chi = vertices.len() as i64 - edges.len() as i64 + faces.len() as i64;
        // Walk the loops.
        let mut used: HashMap<(u32, u32), bool> = HashMap::new();
        let mut loops: Vec<Vec<u32>> = Vec::new();
        let starts: Vec<(u32, u32)> =
            border.iter().flat_map(|(&a, bs)| bs.iter().map(move |&b| (a, b))).collect::<Vec<_>>();
        let mut starts = starts;
        starts.sort_unstable();
        for (a0, b0) in starts {
            if used.contains_key(&(a0, b0)) {
                continue;
            }
            let mut ring = vec![a0];
            let (mut a, mut b) = (a0, b0);
            for _ in 0..=border_count {
                used.insert((a, b), true);
                if b == a0 {
                    break;
                }
                ring.push(b);
                let next =
                    border.get(&b).and_then(|cs| cs.iter().copied().find(|&c| !used.contains_key(&(b, c))));
                match next {
                    Some(c) => {
                        a = b;
                        b = c;
                    }
                    None => break,
                }
            }
            loops.push(ring);
        }
        let disk = loops.len() == 1 && chi == 1;
        if !disk {
            stats.non_disk += 1;
            continue;
        }
        if n < 3 {
            stats.low_sides += 1;
            continue;
        }
        if n > max_sides {
            stats.high_sides += 1;
            continue;
        }
        stats.valid += 1;
        if n == 4 {
            stats.cc_able += 1;
            continue;
        }
        let ring = &loops[0];
        let is_corner = |v: u32| corners[p as usize].contains(&v);
        let Some(first) = ring.iter().position(|&v| is_corner(v)) else { continue };
        let m = ring.len();
        let mut lengths = Vec::with_capacity(n);
        let mut length = 0.0;
        for k in 1..=m {
            let (a, b) = (ring[(first + k - 1) % m], ring[(first + k) % m]);
            length += (positions[b as usize] - positions[a as usize]).length();
            if is_corner(b) {
                lengths.push(length);
                length = 0.0;
            }
        }
        let shortest = lengths.iter().copied().fold(f64::INFINITY, f64::min);
        if lengths.len() == n && cc_margin(&lengths) + shortest + tolerance > 0.0 {
            stats.cc_able += 1;
        }
    }
    stats
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testmesh::grid;

    #[test]
    fn two_quads_measure_as_valid() {
        let (p, t) = grid(4, 4);
        // Left half (x < 2) is patch 0, right half patch 1.
        let patch: Vec<u32> = t
            .iter()
            .map(|tri| u32::from(tri.iter().map(|&v| p[v as usize].x).sum::<f64>() / 3.0 > 2.0))
            .collect();
        let corners = vec![vec![0, 2, 22, 20], vec![2, 4, 24, 22]];
        let s = layout_stats(&p, &t, &patch, &corners, 5, 1.0);
        assert_eq!(s.patches, 2);
        assert_eq!(s.valid, 2);
        assert_eq!(s.cc_able, 2);
        assert_eq!(s.non_disk, 0);
        assert_eq!(s.sides_histogram[4], 2);
    }

    #[test]
    fn a_ring_is_not_a_disk() {
        let (p, t) = grid(6, 6);
        let centre = |tri: &[u32; 3]| {
            tri.iter()
                .all(|&v| (2.0..=4.0).contains(&p[v as usize].x) && (2.0..=4.0).contains(&p[v as usize].y))
        };
        let patch: Vec<u32> = t.iter().map(|tri| u32::from(centre(tri))).collect();
        let s = layout_stats(&p, &t, &patch, &[vec![0, 6, 48, 42], vec![16, 18, 32, 30]], 5, 1.0);
        assert_eq!(s.non_disk, 1);
        assert_eq!(s.valid, 1);
    }
}
