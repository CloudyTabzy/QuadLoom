//! Path search on the field graph, and the rules for how paths may meet.
//!
//! Paths may cross each other only at right angles and only by actually
//! crossing (their neighbours alternate around the shared vertex); they may
//! never share an edge, run tangentially through a vertex, or touch without
//! crossing. A path may end on another one at right angles (a T-junction),
//! or where the other one ends, continuing it.

use hashbrown::HashMap;
use std::cmp::Ordering;
use std::collections::BinaryHeap;

use super::cut::CutMesh;
use super::graph::{DIRECT, FieldGraph, Node, TWIN, transport};
use crate::control::{Control, Stage};
use crate::error::Error;

const NONE: u32 = u32::MAX;

/// Dijkstra pops between cancel checks.
const CANCEL_INTERVAL: usize = 256;

/// A path through the graph, expanded so that consecutive nodes always share
/// a mesh edge or are copies of one vertex (a twin step across a cut).
#[derive(Clone, Debug, PartialEq)]
pub struct TracePath {
    pub nodes: Vec<Node>,
    /// A loop: the last node connects back to the first (not repeated).
    pub closed: bool,
    pub cost: f64,
}

impl TracePath {
    pub fn vertices(&self) -> impl Iterator<Item = u32> + '_ {
        self.nodes.iter().map(|n| n.vertex())
    }

    /// Consecutive vertex pairs, including the closing pair of a loop.
    pub fn steps(&self) -> impl Iterator<Item = (Node, Node)> + '_ {
        let n = self.nodes.len();
        let count = if self.closed { n } else { n.saturating_sub(1) };
        (0..count).map(move |i| (self.nodes[i], self.nodes[(i + 1) % n]))
    }

    /// Mesh edges (twin steps excluded), as sorted pairs.
    pub fn edges<'a>(&'a self, cut: &'a CutMesh) -> impl Iterator<Item = (u32, u32)> + 'a {
        self.steps().filter(move |(a, b)| !is_twin_step(cut, a.vertex(), b.vertex())).map(|(a, b)| {
            let (x, y) = (a.vertex(), b.vertex());
            (x.min(y), x.max(y))
        })
    }
}

fn is_twin_step(cut: &CutMesh, a: u32, b: u32) -> bool {
    cut.twins[a as usize].contains(&b)
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Entry {
    cost: f64,
    node: u32,
}

impl Eq for Entry {}

impl Ord for Entry {
    // A min-heap on cost, ties by node id.
    fn cmp(&self, other: &Self) -> Ordering {
        other.cost.total_cmp(&self.cost).then(other.node.cmp(&self.node))
    }
}

impl PartialOrd for Entry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Reusable Dijkstra state over one graph.
#[derive(Debug)]
pub struct Search<'g> {
    graph: &'g FieldGraph,
    dist: Vec<f64>,
    parent: Vec<u32>,
    via: Vec<u32>,
    touched: Vec<u32>,
    heap: BinaryHeap<Entry>,
    /// Per vertex: the stamp of the corridor it is in (when `restrict`).
    corridor: Vec<u32>,
    stamp: u32,
    restrict: bool,
}

impl<'g> Search<'g> {
    pub fn new(graph: &'g FieldGraph) -> Self {
        let n = graph.node_count();
        Self {
            graph,
            dist: vec![f64::INFINITY; n],
            parent: vec![NONE; n],
            via: vec![DIRECT; n],
            touched: Vec::new(),
            heap: BinaryHeap::new(),
            corridor: vec![0; graph.vertex_count()],
            stamp: 0,
            restrict: false,
        }
    }

    fn allowed(&self, v: u32) -> bool {
        !self.restrict || self.corridor[v as usize] == self.stamp
    }

    /// Whether a 2-hop arc's intermediate vertex may be passed: inside the
    /// corridor, and through an active node (a chosen path turns a vertex's
    /// nodes in its axis off; only then is the exact node worth computing).
    fn via_usable(&self, from: Node, via: u32) -> bool {
        if via == DIRECT || via == TWIN {
            return true;
        }
        if !self.allowed(via) {
            return false;
        }
        let g = self.graph;
        (0..4).all(|d| g.active(Node::new(via, d))) || g.active(self.via_node(from, via))
    }

    /// A loop through `start`, searched only near the field line: first the
    /// streamline from `start` (always the cheapest arc) is followed until it
    /// comes back next to `start`; then [`Self::closed_loop`] runs inside
    /// the vertices within `rings` edges of it. Seeds whose streamline
    /// doesn't close (it spirals, or runs into a boundary) give no loop.
    /// This costs about the loop's length instead of the surface's area.
    pub fn guided_loop(
        &mut self,
        cut: &CutMesh,
        start: Node,
        max_cost: f64,
        rings: usize,
        control: &Control,
    ) -> Result<Option<TracePath>, Error> {
        let g = self.graph;
        if !g.active(start) {
            return Ok(None);
        }
        let origin = start.vertex();
        let near_origin: Vec<u32> = cut.neighbours(origin);
        self.stamp = self.stamp.wrapping_add(1);
        if self.stamp == 0 {
            self.corridor.fill(0);
            self.stamp = 1;
        }
        let stamp = self.stamp;
        let mut line: Vec<u32> = vec![origin];
        self.corridor[origin as usize] = stamp;
        let (mut here, mut cost, mut closed, mut by_twin) = (start, 0.0, false, false);
        for step in 0..g.vertex_count() {
            // A boundary reached over a mesh edge only lets the line cross a
            // cut (by a twin arc); the copy reached that way continues freely.
            let ends = step > 0 && g.is_boundary(here.vertex()) && !by_twin;
            let Some(arc) = g.arcs(here).find(|a| g.active(a.to) && (!ends || a.via == TWIN)) else { break };
            by_twin = arc.via == TWIN;
            if arc.to == start {
                closed = true;
                break;
            }
            cost += f64::from(arc.cost);
            if cost > max_cost {
                break;
            }
            for v in [arc.via, arc.to.vertex()] {
                if v == DIRECT || v == TWIN {
                    continue;
                }
                if self.corridor[v as usize] == stamp {
                    if step >= 3 && (v == origin || near_origin.contains(&v)) {
                        closed = true;
                    } else if v != origin {
                        // A revisit away from the start: a spiral.
                        return Ok(None);
                    }
                } else {
                    self.corridor[v as usize] = stamp;
                    line.push(v);
                }
            }
            if closed {
                break;
            }
            if step >= 3 && near_origin.contains(&arc.to.vertex()) {
                closed = true;
                break;
            }
            here = arc.to;
        }
        if !closed {
            return Ok(None);
        }
        // Widen the line into a corridor.
        let mut frontier = line;
        for _ in 0..rings {
            let mut next = Vec::new();
            for &v in &frontier {
                for w in cut.neighbours(v).into_iter().chain(cut.twins[v as usize].iter().copied()) {
                    if self.corridor[w as usize] != stamp {
                        self.corridor[w as usize] = stamp;
                        next.push(w);
                    }
                }
            }
            frontier = next;
        }
        self.restrict = true;
        let found = self.closed_loop(start, max_cost, control);
        self.restrict = false;
        found
    }

    fn reset(&mut self) {
        for &k in &self.touched {
            self.dist[k as usize] = f64::INFINITY;
            self.parent[k as usize] = NONE;
            self.via[k as usize] = DIRECT;
        }
        self.touched.clear();
        self.heap.clear();
    }

    fn seed(&mut self, starts: &[Node]) {
        for &s in starts {
            if self.graph.active(s) && self.dist[s.0 as usize] > 0.0 {
                self.dist[s.0 as usize] = 0.0;
                self.parent[s.0 as usize] = s.0;
                self.touched.push(s.0);
                self.heap.push(Entry { cost: 0.0, node: s.0 });
            }
        }
    }

    /// The cheapest path from any of `starts` to a node (other than a start)
    /// for which `target` holds, costing at most `max_cost`.
    pub fn shortest(
        &mut self,
        starts: &[Node],
        target: &dyn Fn(Node) -> bool,
        max_cost: f64,
        control: &Control,
    ) -> Result<Option<TracePath>, Error> {
        self.reset();
        self.seed(starts);
        let mut pops = 0usize;
        while let Some(Entry { cost, node }) = self.heap.pop() {
            if cost > self.dist[node as usize] {
                continue;
            }
            pops += 1;
            if pops.is_multiple_of(CANCEL_INTERVAL) {
                control.checkpoint(Stage::Trace)?;
            }
            let here = Node(node);
            if self.parent[node as usize] != node && target(here) {
                return Ok(Some(self.path_to(here, false)));
            }
            let twins_only = self.ends_here(node);
            self.relax(here, cost, max_cost, twins_only);
        }
        control.checkpoint(Stage::Trace)?;
        Ok(None)
    }

    /// The cheapest loop from `start` back to `start`, costing at most `max_cost`.
    pub fn closed_loop(
        &mut self,
        start: Node,
        max_cost: f64,
        control: &Control,
    ) -> Result<Option<TracePath>, Error> {
        self.reset();
        self.seed(&[start]);
        let (mut best, mut best_cost) = (None, max_cost);
        let mut pops = 0usize;
        while let Some(Entry { cost, node }) = self.heap.pop() {
            if cost > self.dist[node as usize] {
                continue;
            }
            if cost >= best_cost {
                break;
            }
            pops += 1;
            if pops.is_multiple_of(CANCEL_INTERVAL) {
                control.checkpoint(Stage::Trace)?;
            }
            let here = Node(node);
            let twins_only = self.ends_here(node);
            for arc in self.graph.arcs(here) {
                if twins_only {
                    break;
                }
                if arc.to == start && arc.via != TWIN && self.via_usable(here, arc.via) {
                    let total = cost + f64::from(arc.cost);
                    if total < best_cost && self.parent[node as usize] != node {
                        best_cost = total;
                        best = Some((here, arc.via));
                    }
                }
            }
            self.relax(here, cost, best_cost, twins_only);
        }
        control.checkpoint(Stage::Trace)?;
        Ok(best.map(|(last, via)| {
            let mut path = self.path_to(last, true);
            if via != DIRECT {
                path.nodes.push(self.via_node(last, via));
            }
            path.cost = best_cost;
            path
        }))
    }

    /// A path can't run along or through a boundary: a boundary node it
    /// reached over a mesh edge is an end (it may only cross a cut, by a twin
    /// arc, after arriving by one... or start there).
    fn ends_here(&self, node: u32) -> bool {
        let start = self.parent[node as usize] == node;
        let by_twin = self.via[node as usize] == TWIN;
        self.graph.is_boundary(Node(node).vertex()) && !start && !by_twin
    }

    fn relax(&mut self, here: Node, cost: f64, max_cost: f64, twins_only: bool) {
        let graph = self.graph;
        for arc in graph.arcs(here) {
            if twins_only && arc.via != TWIN {
                continue;
            }
            if !graph.active(arc.to) || !self.allowed(arc.to.vertex()) {
                continue;
            }
            if !self.via_usable(here, arc.via) {
                continue;
            }
            let next = cost + f64::from(arc.cost);
            if next > max_cost || next >= self.dist[arc.to.0 as usize] {
                continue;
            }
            let k = arc.to.0 as usize;
            if self.dist[k].is_infinite() {
                self.touched.push(arc.to.0);
            }
            self.dist[k] = next;
            self.parent[k] = here.0;
            self.via[k] = arc.via;
            self.heap.push(Entry { cost: next, node: arc.to.0 });
        }
    }

    /// The node at `via` continuing `from`'s direction.
    fn via_node(&self, from: Node, via: u32) -> Node {
        let g = self.graph;
        let d = g.direction(from.vertex(), from.dir()).unwrap_or_default();
        let carried = transport(d, g.normal(from.vertex()), g.normal(via));
        Node::new(via, g.closest_dir(via, carried))
    }

    fn path_to(&self, end: Node, closed: bool) -> TracePath {
        let mut reversed = vec![end];
        let mut k = end.0;
        for _ in 0..self.dist.len() {
            let p = self.parent[k as usize];
            if p == k || p == NONE {
                break;
            }
            let via = self.via[k as usize];
            if via != DIRECT && via != TWIN {
                reversed.push(self.via_node(Node(p), via));
            }
            reversed.push(Node(p));
            k = p;
        }
        reversed.reverse();
        TracePath { nodes: reversed, closed, cost: self.dist[end.0 as usize] }
    }

    /// Follows the cheapest active arc from `start` until `target` holds,
    /// for at most `max_steps` arcs; None on a dead end or a revisit.
    pub fn direct(&self, start: Node, target: &dyn Fn(Node) -> bool, max_steps: usize) -> Option<TracePath> {
        let g = self.graph;
        if !g.active(start) {
            return None;
        }
        let mut nodes = vec![start];
        let mut visited: HashMap<u32, ()> = HashMap::new();
        visited.insert(start.vertex(), ());
        let (mut here, mut cost, mut by_twin) = (start, 0.0, false);
        for step in 0..max_steps {
            let ends = step > 0 && g.is_boundary(here.vertex()) && !by_twin;
            let arc = g.arcs(here).find(|a| g.active(a.to) && (!ends || a.via == TWIN))?;
            by_twin = arc.via == TWIN;
            if arc.via != DIRECT && arc.via != TWIN {
                let mid = self.via_node(here, arc.via);
                if visited.insert(mid.vertex(), ()).is_some() {
                    return None;
                }
                nodes.push(mid);
            }
            if visited.insert(arc.to.vertex(), ()).is_some() {
                return None;
            }
            nodes.push(arc.to);
            cost += f64::from(arc.cost);
            here = arc.to;
            if target(here) {
                return Some(TracePath { nodes, closed: false, cost });
            }
        }
        None
    }
}

/// Shortcuts `a → b → c` to `a → c` wherever a, b and c are one face and
/// a–c is an interior edge not on `blocked` (nor a twin step): the path covers
/// the same field distance with fewer zig-zags.
pub fn cut_ears(path: &mut TracePath, cut: &CutMesh, blocked: &dyn Fn(u32, u32) -> bool) {
    let mut i = 0;
    while path.nodes.len() > 3 && i + 2 < path.nodes.len() {
        let [a, b, c] = [path.nodes[i], path.nodes[i + 1], path.nodes[i + 2]].map(|n| n.vertex());
        let same_face = cut.half_edge(a, b).is_some_and(|h| cut.head(CutMesh::next(h)) == c)
            || cut.half_edge(b, a).is_some_and(|h| cut.head(CutMesh::next(h)) == c)
            || cut.half_edge(a, c).is_some_and(|h| cut.head(CutMesh::next(h)) == b)
            || cut.half_edge(c, a).is_some_and(|h| cut.head(CutMesh::next(h)) == b);
        if same_face
            && !is_twin_step(cut, a, b)
            && !is_twin_step(cut, b, c)
            && cut.is_interior_edge(a, c)
            && !blocked(a, c)
            && !cut.boundary[b as usize]
        {
            path.nodes.remove(i + 1);
            i = i.saturating_sub(1);
        } else {
            i += 1;
        }
    }
}

/// A path's visit to a vertex: its axis there and its neighbours on the path.
#[derive(Clone, Copy, Debug)]
struct Visit {
    path: u32,
    axis: u8,
    prev: u32,
    next: u32,
}

impl Visit {
    fn is_end(&self) -> bool {
        self.prev == NONE || self.next == NONE
    }
}

fn visits(path: &TracePath, id: u32, cut: &CutMesh) -> Vec<(u32, Visit)> {
    let n = path.nodes.len();
    (0..n)
        .map(|i| {
            let node = path.nodes[i];
            let v = node.vertex();
            let neighbour = |j: Option<usize>| {
                j.map(|j| path.nodes[j].vertex()).filter(|&w| !is_twin_step(cut, v, w)).unwrap_or(NONE)
            };
            let prev = neighbour(if i > 0 {
                Some(i - 1)
            } else if path.closed {
                Some(n - 1)
            } else {
                None
            });
            let next = neighbour(if i + 1 < n {
                Some(i + 1)
            } else if path.closed {
                Some(0)
            } else {
                None
            });
            (v, Visit { path: id, axis: node.dir() % 2, prev, next })
        })
        .collect()
}

/// Whether two visits to `v` conflict (see the module docs).
fn conflict(cut: &CutMesh, v: u32, a: &Visit, b: &Visit) -> bool {
    if a.axis == b.axis {
        if a.is_end() && b.is_end() {
            // Allowed only when they continue each other (their path
            // neighbours lie on opposite sides of v).
            let side = |x: &Visit| if x.prev != NONE { x.prev } else { x.next };
            let (sa, sb) = (side(a), side(b));
            if sa == NONE || sb == NONE {
                return true;
            }
            let p = cut.positions[v as usize];
            return (cut.positions[sa as usize] - p).dot(cut.positions[sb as usize] - p) >= 0.0;
        }
        return true;
    }
    if a.is_end() || b.is_end() {
        return false; // a T-junction
    }
    !interleaved(cut, v, [a.prev, a.next], [b.prev, b.next])
}

/// Whether exactly one of `b`'s neighbours lies between `a`'s two around `v`.
fn interleaved(cut: &CutMesh, v: u32, a: [u32; 2], b: [u32; 2]) -> bool {
    let ring = cut.neighbours(v);
    let pos = |w: u32| ring.iter().position(|&x| x == w);
    let (Some(a0), Some(a1), Some(b0), Some(b1)) = (pos(a[0]), pos(a[1]), pos(b[0]), pos(b[1])) else {
        return false;
    };
    let (lo, hi) = (a0.min(a1), a0.max(a1));
    let inside = |x: usize| lo < x && x < hi;
    inside(b0) != inside(b1) && ![b0, b1].contains(&a0) && ![b0, b1].contains(&a1)
}

/// Whether `path` meets itself in a way two separate paths may not (a
/// repeated edge, a vertex revisited in the same axis, or touching).
pub fn self_intersects(path: &TracePath, cut: &CutMesh) -> bool {
    let mut edges: Vec<(u32, u32)> = path.edges(cut).collect();
    let count = edges.len();
    edges.sort_unstable();
    edges.dedup();
    if edges.len() != count {
        return true;
    }
    let mut by_vertex: HashMap<u32, Vec<Visit>> = HashMap::new();
    for (v, visit) in visits(path, 0, cut) {
        let list = by_vertex.entry(v).or_default();
        if list.iter().any(|other| conflict(cut, v, other, &visit) || other.axis == visit.axis) {
            return true;
        }
        list.push(visit);
    }
    false
}

/// Where chosen paths run: per vertex, their visits; per edge, their ids.
#[derive(Clone, Debug, Default)]
pub struct Occupancy {
    visits: HashMap<u32, Vec<Visit>>,
    edges: HashMap<(u32, u32), u32>,
}

impl Occupancy {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether `path` may not be added next to the paths already here.
    pub fn collides(&self, path: &TracePath, cut: &CutMesh) -> bool {
        if path.edges(cut).any(|e| self.edges.contains_key(&e)) {
            return true;
        }
        visits(path, NONE, cut).iter().any(|(v, visit)| {
            self.visits.get(v).is_some_and(|list| list.iter().any(|other| conflict(cut, *v, other, visit)))
        })
    }

    pub fn add(&mut self, id: u32, path: &TracePath, cut: &CutMesh) {
        for e in path.edges(cut) {
            self.edges.insert(e, id);
        }
        for (v, visit) in visits(path, id, cut) {
            self.visits.entry(v).or_default().push(visit);
        }
    }

    pub fn remove(&mut self, id: u32, path: &TracePath, cut: &CutMesh) {
        for e in path.edges(cut) {
            if self.edges.get(&e) == Some(&id) {
                self.edges.remove(&e);
            }
        }
        for v in path.vertices() {
            if let Some(list) = self.visits.get_mut(&v) {
                list.retain(|x| x.path != id);
            }
        }
    }

    /// Whether any chosen path uses edge `a`–`b`.
    pub fn has_edge(&self, a: u32, b: u32) -> bool {
        self.edges.contains_key(&(a.min(b), a.max(b)))
    }

    /// Ids of the paths through `v`.
    pub fn paths_at(&self, v: u32) -> impl Iterator<Item = u32> + '_ {
        self.visits.get(&v).into_iter().flatten().map(|x| x.path)
    }

    /// Whether some path other than `id` ends on `id`'s interior (a
    /// T-junction it supports).
    pub fn supports_junction(&self, id: u32, path: &TracePath) -> bool {
        let n = path.nodes.len();
        path.nodes.iter().enumerate().any(|(i, node)| {
            let interior = path.closed || (i > 0 && i + 1 < n);
            interior
                && self
                    .visits
                    .get(&node.vertex())
                    .is_some_and(|list| list.iter().any(|x| x.path != id && x.is_end()))
        })
    }
}

#[cfg(test)]
mod tests {
    use glam::DVec3;

    use super::*;
    use crate::field::{FieldParams, compute};
    use crate::testmesh::{cylinder, grid};
    use crate::trace::TraceCost;

    fn grid_graph(n: u32, barrier: &[[u32; 2]]) -> (CutMesh, FieldGraph) {
        let (p, t) = grid(n, n);
        let cut = CutMesh::new(&p, &t, barrier);
        let g = FieldGraph::new(
            &cut,
            &vec![DVec3::X; t.len()],
            &vec![false; cut.vertex_count()],
            TraceCost::Anisotropic,
            10.0,
            &Control::default(),
        )
        .unwrap();
        (cut, g)
    }

    fn manual(g: &FieldGraph, vertices: &[u32], d: DVec3) -> TracePath {
        let nodes = vertices.iter().map(|&v| Node::new(v, g.closest_dir(v, d))).collect();
        TracePath { nodes, closed: false, cost: 0.0 }
    }

    #[test]
    fn straight_trace_on_grid_is_a_grid_line() {
        let (cut, g) = grid_graph(20, &[]);
        let id = |i: u32, j: u32| j * 21 + i;
        let start = Node::new(id(0, 10), g.closest_dir(id(0, 10), DVec3::X));
        let target = |n: Node| g.position(n.vertex()).x == 20.0;
        let path =
            Search::new(&g).shortest(&[start], &target, f64::INFINITY, &Control::default()).unwrap().unwrap();
        let ys: Vec<f64> = path.vertices().map(|v| g.position(v).y).collect();
        assert!(ys.iter().all(|&y| y == 10.0), "{ys:?}");
        assert_eq!(path.nodes.len(), 21);
        assert!((path.cost - 20.0).abs() < 1e-6);
        assert!(!self_intersects(&path, &cut));
    }

    #[test]
    fn loop_around_cylinder_closes() {
        let (p, t) = cylinder(10, 32);
        let field = compute(&p, &t, &[], &FieldParams::default(), &Control::default()).unwrap();
        let cut = CutMesh::new(&p, &t, &[]);
        let g = FieldGraph::new(
            &cut,
            &field.directions,
            &vec![false; p.len()],
            TraceCost::Anisotropic,
            10.0,
            &Control::default(),
        )
        .unwrap();
        let v = 5 * 32;
        let start = Node::new(v, g.closest_dir(v, DVec3::Y));
        let path =
            Search::new(&g).closed_loop(start, f64::INFINITY, &Control::default()).unwrap().expect("a loop");
        assert!(path.closed);
        let length: f64 = path.steps().map(|(a, b)| g.field_length(a.vertex(), b.vertex())).sum();
        let circumference = std::f64::consts::TAU;
        assert!((length - circumference).abs() < 0.02 * circumference, "length {length}");
        assert!(path.vertices().all(|w| (g.position(w).z - 1.0).abs() < 1e-9), "stays on its ring");
        assert!(!self_intersects(&path, &cut));
    }

    #[test]
    fn trace_respects_cost_bound() {
        let (_, g) = grid_graph(20, &[]);
        let start = Node::new(10 * 21, g.closest_dir(10 * 21, DVec3::X));
        let target = |n: Node| g.position(n.vertex()).x == 20.0;
        let found = Search::new(&g).shortest(&[start], &target, 5.0, &Control::default()).unwrap();
        assert!(found.is_none());
    }

    #[test]
    fn tangent_paths_collide_orthogonal_paths_do_not() {
        let (cut, g) = grid_graph(10, &[]);
        let id = |i: u32, j: u32| j * 11 + i;
        let row: Vec<u32> = (0..=10).map(|i| id(i, 5)).collect();
        let column: Vec<u32> = (0..=10).map(|j| id(5, j)).collect();
        let mut occupied = Occupancy::new();
        occupied.add(0, &manual(&g, &row, DVec3::X), &cut);
        assert!(!occupied.collides(&manual(&g, &column, DVec3::Y), &cut), "a crossing");
        let overlap: Vec<u32> = (2..=8).map(|i| id(i, 5)).collect();
        assert!(occupied.collides(&manual(&g, &overlap, DVec3::X), &cut), "shared edges");
        let half: Vec<u32> = (0..=5).map(|j| id(5, j)).collect();
        assert!(!occupied.collides(&manual(&g, &half, DVec3::Y), &cut), "a T-junction");
        let touch = TracePath {
            nodes: vec![Node::new(id(4, 4), 1), Node::new(id(5, 5), 1), Node::new(id(5, 4), 1)],
            closed: false,
            cost: 0.0,
        };
        assert!(occupied.collides(&touch, &cut), "touching without crossing");
        let parallel: Vec<u32> = (0..=10).map(|i| id(i, 6)).collect();
        assert!(!occupied.collides(&manual(&g, &parallel, DVec3::X), &cut), "a parallel neighbour");
        occupied.remove(0, &manual(&g, &row, DVec3::X), &cut);
        assert!(!occupied.collides(&manual(&g, &overlap, DVec3::X), &cut), "after removal");
    }

    #[test]
    fn paths_cross_a_feature_through_twins() {
        let id = |i: u32, j: u32| j * 11 + i;
        let feature: Vec<[u32; 2]> = (0..10).map(|j| [id(5, j), id(5, j + 1)]).collect();
        let (cut, mut g) = grid_graph(10, &feature);
        let mut pairs = Vec::new();
        for j in 0..=10u32 {
            let copies: Vec<u32> =
                std::iter::once(id(5, j)).chain(cut.twins[id(5, j) as usize].iter().copied()).collect();
            assert_eq!(copies.len(), 2, "row {j}");
            // The copy whose faces lie at x < 5 is the left one.
            let left_side = |v: u32| {
                cut.fan(v).iter().any(|&h| {
                    cut.triangles[(h / 3) as usize].iter().any(|&w| cut.positions[w as usize].x < 5.0)
                })
            };
            let (l, r) = if left_side(copies[0]) { (copies[0], copies[1]) } else { (copies[1], copies[0]) };
            pairs.push((Node::new(l, g.closest_dir(l, DVec3::X)), Node::new(r, g.closest_dir(r, DVec3::X))));
            pairs
                .push((Node::new(r, g.closest_dir(r, -DVec3::X)), Node::new(l, g.closest_dir(l, -DVec3::X))));
        }
        g.add_twin_arcs(&pairs);
        let start = Node::new(id(0, 5), g.closest_dir(id(0, 5), DVec3::X));
        let target = |n: Node| g.position(n.vertex()).x == 10.0;
        let path =
            Search::new(&g).shortest(&[start], &target, f64::INFINITY, &Control::default()).unwrap().unwrap();
        let twin_steps = path.steps().filter(|(a, b)| is_twin_step(&cut, a.vertex(), b.vertex())).count();
        assert_eq!(twin_steps, 1);
        assert!(path.vertices().all(|v| g.position(v).y == 5.0), "crosses at right angles, straight");
        assert!(!self_intersects(&path, &cut));
    }

    #[test]
    fn self_intersecting_path_is_detected() {
        let (cut, g) = grid_graph(10, &[]);
        let id = |i: u32, j: u32| j * 11 + i;
        let back = manual(&g, &[id(2, 2), id(3, 2), id(4, 2), id(3, 2)], DVec3::X);
        assert!(self_intersects(&back, &cut));
    }

    #[test]
    fn cancel_stops_the_search() {
        let (_, g) = grid_graph(30, &[]);
        let control = Control::default();
        control.cancel();
        let start = Node::new(0, 0);
        let result = Search::new(&g).shortest(&[start], &|_| false, f64::INFINITY, &control);
        assert!(result.is_err());
    }

    #[test]
    fn ears_are_cut() {
        let (cut, g) = grid_graph(10, &[]);
        let id = |i: u32, j: u32| j * 11 + i;
        // (2,2) → (3,2) → (3,3) is one face; (2,2)–(3,3) is its diagonal.
        let mut path =
            manual(&g, &[id(1, 2), id(2, 2), id(3, 2), id(3, 3), id(4, 3)], DVec3::new(1.0, 0.5, 0.0));
        cut_ears(&mut path, &cut, &|_, _| false);
        let vertices: Vec<u32> = path.vertices().collect();
        assert_eq!(vertices, vec![id(1, 2), id(2, 2), id(3, 3), id(4, 3)]);
    }
}
