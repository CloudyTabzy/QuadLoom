//! Distances along the mesh, and evenly spread seed vertices for loops.

use std::collections::BinaryHeap;

use super::cut::CutMesh;
use super::graph::CANCEL_EVERY;
use crate::control::{Control, Stage};
use crate::error::Error;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Entry {
    dist: f64,
    v: u32,
}

impl Eq for Entry {}

impl Ord for Entry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other.dist.total_cmp(&self.dist).then(other.v.cmp(&self.v))
    }
}

impl PartialOrd for Entry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Edge-length distances from a growing set of source vertices. Copies of a
/// vertex across a cut are at distance 0 from each other.
#[derive(Clone, Debug)]
pub struct DistanceField {
    offsets: Vec<u32>,
    neighbours: Vec<(u32, f64)>,
    dist: Vec<f64>,
}

impl DistanceField {
    pub fn new(cut: &CutMesh) -> Self {
        let n = cut.vertex_count();
        let (mut offsets, mut neighbours) = (Vec::with_capacity(n + 1), Vec::new());
        offsets.push(0);
        for v in 0..n as u32 {
            let p = cut.positions[v as usize];
            for w in cut.neighbours(v) {
                neighbours.push((w, (cut.positions[w as usize] - p).length()));
            }
            neighbours.extend(cut.twins[v as usize].iter().map(|&w| (w, 0.0)));
            offsets.push(neighbours.len() as u32);
        }
        Self { offsets, neighbours, dist: vec![f64::INFINITY; n] }
    }

    pub fn get(&self, v: u32) -> f64 {
        self.dist[v as usize]
    }

    /// Makes `sources` distance 0 and lowers every distance they shorten.
    /// Stops early once `control` is cancelled (the caller's next
    /// checkpoint then returns, so the partial field is never used).
    pub fn add_sources(&mut self, sources: impl IntoIterator<Item = u32>, control: &Control) {
        let mut heap = BinaryHeap::new();
        for s in sources {
            if self.dist[s as usize] > 0.0 {
                self.dist[s as usize] = 0.0;
                heap.push(Entry { dist: 0.0, v: s });
            }
        }
        let mut pops = 0u32;
        while let Some(Entry { dist, v }) = heap.pop() {
            pops = pops.wrapping_add(1);
            if pops % CANCEL_EVERY == 0 && control.is_cancelled() {
                return;
            }
            if dist > self.dist[v as usize] {
                continue;
            }
            let (a, b) = (self.offsets[v as usize] as usize, self.offsets[v as usize + 1] as usize);
            for k in a..b {
                let (w, len) = self.neighbours[k];
                let d = dist + len;
                if d < self.dist[w as usize] {
                    self.dist[w as usize] = d;
                    heap.push(Entry { dist: d, v: w });
                }
            }
        }
    }
}

/// Up to `count` vertices for which `eligible` holds, spread out: the first
/// is the lowest eligible id, each next one the eligible vertex farthest from
/// those chosen (ties: lower id; unreachable vertices count as farthest).
pub fn farthest_points(
    cut: &CutMesh,
    eligible: &dyn Fn(u32) -> bool,
    count: usize,
    control: &Control,
) -> Result<Vec<u32>, Error> {
    let candidates: Vec<u32> = (0..cut.vertex_count() as u32).filter(|&v| eligible(v)).collect();
    let mut chosen = Vec::with_capacity(count.min(candidates.len()));
    let Some(&first) = candidates.first() else { return Ok(chosen) };
    let mut field = DistanceField::new(cut);
    let mut next = first;
    while chosen.len() < count {
        control.checkpoint(Stage::Trace)?;
        chosen.push(next);
        field.add_sources([next], control);
        let best =
            candidates.iter().copied().max_by(|&a, &b| field.get(a).total_cmp(&field.get(b)).then(b.cmp(&a)));
        match best {
            Some(v) if field.get(v) > 0.0 => next = v,
            _ => break,
        }
    }
    Ok(chosen)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testmesh::grid;

    #[test]
    fn farthest_points_spread_out_and_are_deterministic() {
        let (p, t) = grid(10, 10);
        let cut = CutMesh::new(&p, &t, &[]);
        let interior = |v: u32| !cut.boundary[v as usize];
        let a = farthest_points(&cut, &interior, 4, &Control::default()).unwrap();
        assert_eq!(a, farthest_points(&cut, &interior, 4, &Control::default()).unwrap());
        assert_eq!(a.len(), 4);
        assert_eq!(a[0], 12, "lowest interior id first");
        // The second seed is the far interior corner.
        assert_eq!(cut.positions[a[1] as usize], glam::DVec3::new(9.0, 9.0, 0.0));
        let all = farthest_points(&cut, &interior, 1000, &Control::default()).unwrap();
        assert_eq!(all.len(), 81, "stops once every eligible vertex is chosen");
    }

    #[test]
    fn distances_are_incremental() {
        let (p, t) = grid(4, 1);
        let cut = CutMesh::new(&p, &t, &[]);
        let mut field = DistanceField::new(&cut);
        field.add_sources([0], &Control::default());
        assert_eq!(field.get(4), 4.0);
        field.add_sources([4], &Control::default());
        assert_eq!(field.get(3), 1.0);
        assert_eq!(field.get(1), 1.0);
    }
}
