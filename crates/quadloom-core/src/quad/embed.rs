//! Maps a patch's triangles onto its polygon domain, so pattern points can be
//! placed on the surface.
//!
//! The boundary is pinned to the polygon; interior vertices solve a Laplace
//! equation with positive (clamped cotangent) weights. With a convex polygon
//! and positive weights the map is a bijection (Tutte's theorem), so every
//! domain point lies in exactly one mapped triangle.

use std::collections::HashMap;

use faer::linalg::solvers::Solve;
use faer::sparse::{SparseColMat, Triplet};
use faer::{Mat, Side};
use glam::{DVec2, DVec3};

/// Clamp range for edge weights (degenerate triangles have huge cotangents).
const MIN_WEIGHT: f64 = 1e-3;
const MAX_WEIGHT: f64 = 1e3;

/// A patch's triangles with domain coordinates for each of their vertices.
#[derive(Debug)]
pub struct PatchMap {
    triangles: Vec<[u32; 3]>,
    uv: HashMap<u32, DVec2>,
    grid: Grid,
}

impl PatchMap {
    /// Maps `faces` (indices into `triangles`) with the boundary vertices pinned
    /// at `boundary`. `None` if a patch vertex is neither pinned nor connected
    /// to the boundary, or the solve fails.
    pub fn new(
        positions: &[DVec3],
        triangles: &[[u32; 3]],
        faces: &[u32],
        boundary: &HashMap<u32, DVec2>,
    ) -> Option<Self> {
        let tris: Vec<[u32; 3]> = faces.iter().filter_map(|&f| triangles.get(f as usize).copied()).collect();
        let mut interior: Vec<u32> =
            tris.iter().flatten().copied().filter(|v| !boundary.contains_key(v)).collect();
        interior.sort_unstable();
        interior.dedup();
        let index: HashMap<u32, usize> = interior.iter().enumerate().map(|(i, &v)| (v, i)).collect();
        let mut uv: HashMap<u32, DVec2> =
            tris.iter().flatten().filter_map(|v| boundary.get(v).map(|&p| (*v, p))).collect();
        if !interior.is_empty() {
            let n = interior.len();
            let mut triplets = Vec::new();
            let mut rhs = vec![DVec2::ZERO; n];
            let mut diagonal = vec![0.0; n];
            for t in &tris {
                let p = t.map(|v| positions.get(v as usize).copied().unwrap_or(DVec3::ZERO));
                for c in 0..3 {
                    // Edge (a, b) opposite corner c gets half the cotangent at c.
                    let (a, b) = (t[(c + 1) % 3], t[(c + 2) % 3]);
                    let (x, y) = (p[(c + 1) % 3] - p[c], p[(c + 2) % 3] - p[c]);
                    let cot = x.dot(y) / x.cross(y).length().max(1e-300);
                    let w = (0.5 * cot).clamp(MIN_WEIGHT, MAX_WEIGHT);
                    for (i, j) in [(a, b), (b, a)] {
                        let Some(&ii) = index.get(&i) else { continue };
                        diagonal[ii] += w;
                        match (index.get(&j), boundary.get(&j)) {
                            (Some(&jj), _) => triplets.push(Triplet::new(ii, jj, -w)),
                            (None, Some(&q)) => rhs[ii] += q * w,
                            (None, None) => return None,
                        }
                    }
                }
            }
            for (i, d) in diagonal.iter().enumerate() {
                triplets.push(Triplet::new(i, i, *d));
            }
            let matrix = SparseColMat::<usize, f64>::try_new_from_triplets(n, n, &triplets).ok()?;
            let llt = matrix.sp_cholesky(Side::Lower).ok()?;
            let mut b = Mat::<f64>::from_fn(n, 2, |i, k| if k == 0 { rhs[i].x } else { rhs[i].y });
            llt.solve_in_place(b.as_mut());
            for (i, &v) in interior.iter().enumerate() {
                let q = DVec2::new(b[(i, 0)], b[(i, 1)]);
                if !q.is_finite() {
                    return None;
                }
                uv.insert(v, q);
            }
        }
        let grid = Grid::new(&tris, &uv);
        Some(Self { triangles: tris, uv, grid })
    }

    /// The surface point at domain point `q`: barycentric interpolation in the
    /// mapped triangle containing it (the nearest one if rounding put `q` just
    /// outside every triangle).
    pub fn locate(&self, positions: &[DVec3], q: DVec2) -> Option<DVec3> {
        let mut best: Option<(f64, usize, [f64; 3])> = None;
        for &t in self.grid.cell(q) {
            self.consider(q, t as usize, &mut best);
        }
        if best.is_none_or(|(o, _, _)| o > 1e-9) {
            for t in 0..self.triangles.len() {
                self.consider(q, t, &mut best);
            }
        }
        let (_, t, w) = best?;
        let w = w.map(|x| x.max(0.0));
        let sum: f64 = w.iter().sum();
        let tri = self.triangles.get(t)?;
        let p = tri.map(|v| positions.get(v as usize).copied().unwrap_or(DVec3::ZERO));
        Some((p[0] * w[0] + p[1] * w[1] + p[2] * w[2]) / sum.max(1e-300))
    }
}

impl PatchMap {
    /// Keeps triangle `t` in `best` if `q` is less outside it than the current best.
    fn consider(&self, q: DVec2, t: usize, best: &mut Option<(f64, usize, [f64; 3])>) {
        let Some(tri) = self.triangles.get(t) else { return };
        let [a, b, c] = tri.map(|v| self.uv.get(&v).copied().unwrap_or(DVec2::ZERO));
        let Some(w) = barycentric(q, a, b, c) else { return };
        let outside = -w.iter().copied().fold(f64::INFINITY, f64::min);
        if best.is_none_or(|(o, _, _)| outside < o) {
            *best = Some((outside, t, w));
        }
    }
}

fn barycentric(q: DVec2, a: DVec2, b: DVec2, c: DVec2) -> Option<[f64; 3]> {
    let d = (b - a).perp_dot(c - a);
    if d.abs() < 1e-300 {
        return None;
    }
    let wb = (q - a).perp_dot(c - a) / d;
    let wc = (b - a).perp_dot(q - a) / d;
    Some([1.0 - wb - wc, wb, wc])
}

/// A uniform grid over the domain [−1, 1]² listing the triangles that overlap each cell.
#[derive(Debug)]
struct Grid {
    size: usize,
    cells: Vec<Vec<u32>>,
}

impl Grid {
    fn new(tris: &[[u32; 3]], uv: &HashMap<u32, DVec2>) -> Self {
        let size = ((tris.len() as f64).sqrt().ceil() as usize).clamp(1, 256);
        let mut cells = vec![Vec::new(); size * size];
        for (t, tri) in tris.iter().enumerate() {
            let pts = tri.map(|v| uv.get(&v).copied().unwrap_or(DVec2::ZERO));
            let lo = pts[0].min(pts[1]).min(pts[2]);
            let hi = pts[0].max(pts[1]).max(pts[2]);
            let (x0, y0) = Self::coord(size, lo);
            let (x1, y1) = Self::coord(size, hi);
            for y in y0..=y1 {
                for x in x0..=x1 {
                    cells[y * size + x].push(t as u32);
                }
            }
        }
        Self { size, cells }
    }

    fn coord(size: usize, p: DVec2) -> (usize, usize) {
        let f = |v: f64| (((v + 1.0) * 0.5 * size as f64).floor().max(0.0) as usize).min(size - 1);
        (f(p.x), f(p.y))
    }

    fn cell(&self, q: DVec2) -> &[u32] {
        let (x, y) = Self::coord(self.size, q);
        &self.cells[y * self.size + x]
    }
}
