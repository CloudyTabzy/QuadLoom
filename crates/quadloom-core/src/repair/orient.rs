//! Consistent winding per component, outward for closed components.
//!
//! A breadth-first walk over faces decides, per face, whether to flip it so that
//! every shared edge is traversed in opposite directions by its two faces. If the
//! surface is non-orientable the walk meets an edge it can't satisfy; that edge is
//! reported in `cut_edges` for the caller to separate, instead of looping.

use std::collections::VecDeque;

pub use glam::DVec3;
use tinyvec::TinyVec;

#[derive(Clone, Debug, PartialEq)]
pub struct Orientation {
    pub triangles: Vec<[u32; 3]>,
    pub flipped: usize,
    /// Undirected edges that couldn't be made consistent (non-orientable surface).
    pub cut_edges: Vec<(u32, u32)>,
}

/// Six times the signed volume enclosed by `triangles` (positive = outward normals).
pub fn signed_volume(positions: &[DVec3], triangles: &[[u32; 3]]) -> f64 {
    triangles
        .iter()
        .map(|&[a, b, c]| positions[a as usize].dot(positions[b as usize].cross(positions[c as usize])))
        .sum()
}

/// A face across a manifold edge. `same_direction` means both faces traverse the
/// edge the same way, i.e. they disagree about orientation.
#[derive(Clone, Copy, Debug, Default)]
struct Neighbour {
    face: u32,
    same_direction: bool,
    edge: (u32, u32),
}

pub fn orient(positions: &[DVec3], triangles: &[[u32; 3]]) -> Orientation {
    let n = triangles.len();
    // At most one neighbour per edge, so three per face, kept inline (one
    // heap list per face cost more than the neighbours themselves).
    let mut neighbours: Vec<TinyVec<[Neighbour; 3]>> = vec![TinyVec::new(); n];
    let mut boundary = vec![false; n];
    let mut uses: Vec<(u32, u32, u32, u32)> = Vec::with_capacity(3 * n);
    uses.extend(triangles.iter().enumerate().flat_map(|(f, t)| {
        (0..3).map(move |i| {
            let (a, b) = (t[i], t[(i + 1) % 3]);
            (a.min(b), a.max(b), f as u32, a)
        })
    }));
    uses.sort_unstable();
    for group in uses.chunk_by(|x, y| (x.0, x.1) == (y.0, y.1)) {
        match group {
            [x, y] => {
                let (same_direction, edge) = (x.3 == y.3, (x.0, x.1));
                neighbours[x.2 as usize].push(Neighbour { face: y.2, same_direction, edge });
                neighbours[y.2 as usize].push(Neighbour { face: x.2, same_direction, edge });
            }
            _ => {
                for u in group {
                    boundary[u.2 as usize] = true;
                }
            }
        }
    }

    let mut flip: Vec<Option<bool>> = vec![None; n];
    let mut cut_edges = Vec::new();
    let mut queue = VecDeque::new();
    for start in 0..n {
        if flip[start].is_some() {
            continue;
        }
        flip[start] = Some(false);
        queue.push_back(start as u32);
        let mut component = Vec::new();
        let (mut closed, mut conflicted) = (true, false);
        while let Some(f) = queue.pop_front() {
            component.push(f);
            closed &= !boundary[f as usize];
            let flip_f = flip[f as usize].unwrap_or(false);
            for &Neighbour { face: g, same_direction, edge } in &neighbours[f as usize] {
                let wanted = flip_f ^ same_direction;
                match flip[g as usize] {
                    None => {
                        flip[g as usize] = Some(wanted);
                        queue.push_back(g);
                    }
                    Some(actual) if actual != wanted => {
                        conflicted = true;
                        cut_edges.push(edge);
                    }
                    Some(_) => {}
                }
            }
        }
        // Closed, orientable components should enclose positive volume.
        if closed && !conflicted {
            let oriented: Vec<[u32; 3]> = component
                .iter()
                .map(|&f| apply(triangles[f as usize], flip[f as usize].unwrap_or(false)))
                .collect();
            if signed_volume(positions, &oriented) < 0.0 {
                for &f in &component {
                    flip[f as usize] = flip[f as usize].map(|x| !x);
                }
            }
        }
    }
    cut_edges.sort_unstable();
    cut_edges.dedup();

    let mut flipped = 0;
    let triangles = triangles
        .iter()
        .zip(&flip)
        .map(|(&t, f)| {
            let f = f.unwrap_or(false);
            flipped += usize::from(f);
            apply(t, f)
        })
        .collect();
    Orientation { triangles, flipped, cut_edges }
}

fn apply(t: [u32; 3], flip: bool) -> [u32; 3] {
    if flip { [t[0], t[2], t[1]] } else { t }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f64, y: f64, z: f64) -> DVec3 {
        DVec3::new(x, y, z)
    }

    fn cube() -> (Vec<DVec3>, Vec<[u32; 3]>) {
        let p = vec![
            v(0., 0., 0.),
            v(1., 0., 0.),
            v(1., 1., 0.),
            v(0., 1., 0.),
            v(0., 0., 1.),
            v(1., 0., 1.),
            v(1., 1., 1.),
            v(0., 1., 1.),
        ];
        let t = vec![
            [0, 3, 2],
            [0, 2, 1],
            [4, 5, 6],
            [4, 6, 7],
            [0, 1, 5],
            [0, 5, 4],
            [3, 7, 6],
            [3, 6, 2],
            [0, 4, 7],
            [0, 7, 3],
            [1, 2, 6],
            [1, 6, 5],
        ];
        (p, t)
    }

    fn consistent(triangles: &[[u32; 3]]) -> bool {
        let mut directed: Vec<(u32, u32)> =
            triangles.iter().flat_map(|t| (0..3).map(move |i| (t[i], t[(i + 1) % 3]))).collect();
        directed.sort_unstable();
        directed.windows(2).all(|w| w[0] != w[1])
    }

    #[test]
    fn inconsistent_winding_fixed() {
        let (p, mut t) = cube();
        t[3] = [t[3][0], t[3][2], t[3][1]];
        t[7] = [t[7][0], t[7][2], t[7][1]];
        let o = orient(&p, &t);
        assert!(consistent(&o.triangles));
        assert_eq!(o.flipped, 2);
        assert_eq!(o.cut_edges.len(), 0);
    }

    #[test]
    fn inside_out_cube_flipped() {
        let (p, t) = cube();
        let inverted: Vec<[u32; 3]> = t.iter().map(|f| [f[0], f[2], f[1]]).collect();
        let o = orient(&p, &inverted);
        assert_eq!(o.flipped, 12);
        assert!(signed_volume(&p, &o.triangles) > 0.0);
    }

    #[test]
    fn outward_cube_untouched() {
        let (p, t) = cube();
        let o = orient(&p, &t);
        assert_eq!(o.flipped, 0);
        assert_eq!(o.triangles, t);
    }

    #[test]
    fn mobius_strip_reports_cut_edges() {
        // A 6-quad strip whose ends are glued with a half twist.
        let n = 6u32;
        let mut p = Vec::new();
        for i in 0..n {
            let a = std::f64::consts::TAU * f64::from(i) / f64::from(n);
            let (r, h) = (2.0, 0.5 * (a / 2.0).cos());
            let w = 0.5 * (a / 2.0).sin();
            p.push(v((r + w) * a.cos(), (r + w) * a.sin(), h));
            p.push(v((r - w) * a.cos(), (r - w) * a.sin(), -h));
        }
        let mut t = Vec::new();
        for i in 0..n {
            let (a0, a1) = (2 * i, 2 * i + 1);
            let (b0, b1) = if i + 1 == n { (1, 0) } else { (2 * i + 2, 2 * i + 3) };
            t.push([a0, b0, a1]);
            t.push([a1, b0, b1]);
        }
        let o = orient(&p, &t);
        assert!(!o.cut_edges.is_empty(), "a Möbius strip can't be oriented without a cut");
    }
}
