//! Disjoint sets over `u32` ids: iterative, with path halving and lowest-id roots
//! (so results never depend on the order of `union` calls).

#[derive(Debug, Clone)]
pub(crate) struct UnionFind {
    parent: Vec<u32>,
}

impl UnionFind {
    pub(crate) fn new(n: usize) -> Self {
        Self { parent: (0..n as u32).collect() }
    }

    pub(crate) fn find(&mut self, mut x: u32) -> u32 {
        while self.parent[x as usize] != x {
            let grandparent = self.parent[self.parent[x as usize] as usize];
            self.parent[x as usize] = grandparent;
            x = grandparent;
        }
        x
    }

    pub(crate) fn union(&mut self, a: u32, b: u32) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            let (lo, hi) = (ra.min(rb), ra.max(rb));
            self.parent[hi as usize] = lo;
        }
    }
}
