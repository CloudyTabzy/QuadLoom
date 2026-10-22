//! The vertex field graph: four nodes per vertex, one per field direction.
//!
//! A node is a vertex plus one of its cross's four directions, so a path
//! through the graph follows one consistent field direction (the M₄
//! stratification of Kälberer et al. 2007, as in Campen et al. 2012 and the
//! oracle's `VertexFieldGraph`). Arcs run along non-boundary mesh edges within
//! 45° of the node's direction; virtual 2-hop arcs measure the net
//! displacement of two steps, so a path zig-zagging along a field direction
//! between the mesh's edges isn't charged twice for drift that cancels.
//! Singular vertices have no field direction and hence no nodes: no path
//! passes through them.

use std::collections::HashMap;
use std::f64::consts::FRAC_PI_4;

use glam::{DQuat, DVec3};

use super::TraceCost;
use super::cut::CutMesh;
use crate::control::{Control, Stage};
use crate::error::Error;
use crate::field::Cx;

/// A vertex and one of its four field directions: `4·vertex + direction`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Node(pub u32);

impl Node {
    pub fn new(vertex: u32, dir: u8) -> Self {
        Self(4 * vertex + u32::from(dir % 4))
    }

    pub fn vertex(self) -> u32 {
        self.0 / 4
    }

    pub fn dir(self) -> u8 {
        (self.0 % 4) as u8
    }

    /// The same vertex, the opposite direction.
    pub fn opposite(self) -> Self {
        Self::new(self.vertex(), self.dir() + 2)
    }

    /// The same vertex, the two directions at right angles.
    pub fn orthogonal(self) -> [Self; 2] {
        [Self::new(self.vertex(), self.dir() + 1), Self::new(self.vertex(), self.dir() + 3)]
    }
}

/// No intermediate vertex: a 1-hop arc.
pub const DIRECT: u32 = u32::MAX;

/// A twin arc: between copies of one vertex across a cut.
pub const TWIN: u32 = u32::MAX - 1;

/// A graph arc. 2-hop arcs pass through `via`; otherwise `via` is
/// [`DIRECT`] or [`TWIN`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Arc {
    pub to: Node,
    pub cost: f32,
    pub via: u32,
}

/// Bytes per arc, for the memory estimate.
const ARC_BYTES: usize = std::mem::size_of::<Arc>();

/// Vertices (or faces) between cancel checks in setup loops.
pub const CANCEL_EVERY: u32 = 4096;

/// Largest graph built for one part (its arcs only); above it the part is
/// refused. Parts are built in parallel and search buffers come on top, so
/// peak memory is bounded by the input face limit, not by this alone.
pub const MAX_GRAPH_BYTES: usize = 1_500_000_000;

/// The arcs of every node, plus each vertex's cross.
#[derive(Clone, Debug)]
pub struct FieldGraph {
    positions: Vec<DVec3>,
    boundary: Vec<bool>,
    normal: Vec<DVec3>,
    /// Direction 0 of each vertex's cross, or zero where it has none.
    dir0: Vec<DVec3>,
    offsets: Vec<u32>,
    arcs: Vec<Arc>,
    twin_arcs: HashMap<Node, Vec<Arc>>,
    active: Vec<bool>,
}

impl FieldGraph {
    /// Builds the graph of `cut` for the per-face `directions`; vertices with
    /// `singular[v]` (cut ids) get no nodes. Errors when the graph would
    /// exceed [`MAX_GRAPH_BYTES`] or 2³⁰ vertices.
    pub fn new(
        cut: &CutMesh,
        directions: &[DVec3],
        singular: &[bool],
        cost: TraceCost,
        drift: f64,
        control: &Control,
    ) -> Result<Self, Error> {
        let n = cut.vertex_count();
        if n >= 1 << 30 {
            return Err(Error::invalid_input(Stage::Trace, format!("{n} vertices are too many to trace")));
        }
        let estimate: usize = (0..n as u32)
            .map(|v| {
                let d = cut.fan(v).len();
                d + d * d
            })
            .sum::<usize>()
            .saturating_mul(ARC_BYTES);
        if estimate > MAX_GRAPH_BYTES {
            return Err(Error::invalid_input(
                Stage::Trace,
                format!("the field graph would need about {} MB", estimate / 1_000_000),
            )
            .with_hint("lower the remesh resolution"));
        }
        let (normal, dir0) = vertex_crosses(cut, directions, singular, control)?;
        let mut graph = Self {
            positions: cut.positions.clone(),
            boundary: cut.boundary.clone(),
            normal,
            dir0,
            offsets: Vec::with_capacity(4 * n + 1),
            arcs: Vec::new(),
            twin_arcs: HashMap::new(),
            active: Vec::new(),
        };
        graph.active = (0..4 * n).map(|k| graph.has_cross((k / 4) as u32)).collect();
        // Interior-edge neighbours of each vertex (paths never run along a boundary).
        let neighbours: Vec<Vec<u32>> = (0..n as u32)
            .map(|v| cut.fan(v).iter().filter(|&&h| cut.twin(h).is_some()).map(|&h| cut.head(h)).collect())
            .collect();
        graph.offsets.push(0);
        let mut steps: Vec<(u32, u32)> = Vec::new();
        let mut node_arcs: Vec<Arc> = Vec::new();
        for v in 0..n as u32 {
            if v % CANCEL_EVERY == 0 {
                control.checkpoint(Stage::Trace)?;
            }
            steps.clear();
            if graph.has_cross(v) {
                let ring = &neighbours[v as usize];
                steps.extend(ring.iter().filter(|&&w| graph.has_cross(w)).map(|&w| (w, DIRECT)));
                if !cut.boundary[v as usize] {
                    for &u in ring {
                        if cut.boundary[u as usize] || !graph.has_cross(u) {
                            continue;
                        }
                        for &w in &neighbours[u as usize] {
                            if w == v || cut.boundary[w as usize] || !graph.has_cross(w) || ring.contains(&w)
                            {
                                continue;
                            }
                            steps.push((w, u));
                        }
                    }
                }
                // One 2-hop arc per target: through the intermediate closest to
                // the middle of the step (ties: lower id).
                steps.sort_by(|x, y| {
                    let key = |&(w, u): &(u32, u32)| {
                        let middle = 0.5 * (graph.positions[v as usize] + graph.positions[w as usize]);
                        let off =
                            if u == DIRECT { 0.0 } else { (graph.positions[u as usize] - middle).length() };
                        (w, off, u)
                    };
                    let (a, b) = (key(x), key(y));
                    a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)).then(a.2.cmp(&b.2))
                });
                steps.dedup_by_key(|s| s.0);
            }
            for d in 0..4u8 {
                node_arcs.clear();
                if let Some(f) = graph.direction(v, d) {
                    for &(w, via) in &steps {
                        if let Some(arc) = graph.arc(v, f, w, via, cost, drift) {
                            node_arcs.push(arc);
                        }
                    }
                    node_arcs.sort_by(|a, b| a.cost.total_cmp(&b.cost).then(a.to.cmp(&b.to)));
                }
                graph.arcs.extend_from_slice(&node_arcs);
                graph.offsets.push(graph.arcs.len() as u32);
            }
        }
        Ok(graph)
    }

    /// The arc from `v` (direction `f`) to `w`, if within 45° of the field.
    fn arc(&self, v: u32, f: DVec3, w: u32, via: u32, cost: TraceCost, drift: f64) -> Option<Arc> {
        let n = self.normal[v as usize];
        let e = self.positions[w as usize] - self.positions[v as usize];
        let tangent = e - n * e.dot(n);
        let along = tangent.dot(f);
        let across = tangent.dot(n.cross(f));
        let length = tangent.length();
        if length <= 0.0 || along <= 0.0 {
            return None;
        }
        let angle = across.abs().atan2(along);
        if angle >= FRAC_PI_4 {
            return None;
        }
        let value = match cost {
            TraceCost::Anisotropic => along.hypot(drift * across),
            TraceCost::AnglePenalty => e.length() * (1.0 + drift * (angle / FRAC_PI_4).powi(2)),
        };
        let to = Node::new(w, self.closest_dir(w, transport(f, n, self.normal[w as usize])));
        Some(Arc { to, cost: value as f32, via })
    }

    pub fn vertex_count(&self) -> usize {
        self.positions.len()
    }

    pub fn node_count(&self) -> usize {
        4 * self.positions.len()
    }

    /// Whether `v` lies on a boundary of the cut mesh.
    pub fn is_boundary(&self, v: u32) -> bool {
        self.boundary[v as usize]
    }

    pub fn has_cross(&self, v: u32) -> bool {
        self.dir0[v as usize] != DVec3::ZERO
    }

    pub fn normal(&self, v: u32) -> DVec3 {
        self.normal[v as usize]
    }

    /// Direction `d` (0..4, counter-clockwise about the normal) of `v`'s cross.
    pub fn direction(&self, v: u32, d: u8) -> Option<DVec3> {
        let d0 = self.dir0[v as usize];
        if d0 == DVec3::ZERO {
            return None;
        }
        let d1 = self.normal[v as usize].cross(d0);
        Some(match d % 4 {
            0 => d0,
            1 => d1,
            2 => -d0,
            _ => -d1,
        })
    }

    pub fn cross(&self, v: u32) -> Option<[DVec3; 4]> {
        Some([self.direction(v, 0)?, self.direction(v, 1)?, self.direction(v, 2)?, self.direction(v, 3)?])
    }

    /// The direction of `v`'s cross closest to `d` (ties: lower index; 0
    /// when `v` has no cross).
    pub fn closest_dir(&self, v: u32, d: DVec3) -> u8 {
        let mut best = (0u8, f64::NEG_INFINITY);
        for k in 0..4u8 {
            if let Some(x) = self.direction(v, k) {
                let dot = x.dot(d);
                if dot > best.1 {
                    best = (k, dot);
                }
            }
        }
        best.0
    }

    /// The node's arcs, cheapest first, then its twin arcs.
    pub fn arcs(&self, node: Node) -> impl Iterator<Item = &Arc> {
        let k = node.0 as usize;
        let (a, b) = (self.offsets[k] as usize, self.offsets[k + 1] as usize);
        self.arcs[a..b].iter().chain(self.twin_arcs.get(&node).into_iter().flatten())
    }

    pub fn active(&self, node: Node) -> bool {
        self.active.get(node.0 as usize).copied().unwrap_or(false)
    }

    /// Enables or disables a node (only nodes of vertices with a cross can be enabled).
    pub fn set_active(&mut self, node: Node, on: bool) {
        let has = self.has_cross(node.vertex());
        if let Some(a) = self.active.get_mut(node.0 as usize) {
            *a = on && has;
        }
    }

    /// Adds zero-cost arcs `from → to` (copies of one vertex across a cut).
    pub fn add_twin_arcs(&mut self, pairs: &[(Node, Node)]) {
        for &(from, to) in pairs {
            self.twin_arcs.entry(from).or_default().push(Arc { to, cost: 0.0, via: TWIN });
        }
    }

    /// How far edge `a → b` runs along the field: |(p_b − p_a)·f| with f
    /// the direction of `a`'s cross closest to the edge (or `b`'s, or the
    /// plain length where neither has a cross).
    pub fn field_length(&self, a: u32, b: u32) -> f64 {
        let e = self.positions[b as usize] - self.positions[a as usize];
        for v in [a, b] {
            if self.has_cross(v) {
                let f = self.direction(v, self.closest_dir(v, e)).unwrap_or(DVec3::ZERO);
                return e.dot(f).abs();
            }
        }
        e.length()
    }

    pub fn position(&self, v: u32) -> DVec3 {
        self.positions[v as usize]
    }
}

/// `d` carried from the tangent plane of normal `from` to that of `to` by the
/// minimal rotation between the normals.
pub fn transport(d: DVec3, from: DVec3, to: DVec3) -> DVec3 {
    if from == DVec3::ZERO || to == DVec3::ZERO {
        return d;
    }
    DQuat::from_rotation_arc(from, to) * d
}

/// Each cut vertex's normal (angle-weighted over its fan) and the first
/// direction of its cross: the angle-weighted average of the fan faces'
/// crosses as fourth powers in a vertex frame (zero for singular vertices,
/// and where the fan is degenerate or its crosses cancel).
fn vertex_crosses(
    cut: &CutMesh,
    directions: &[DVec3],
    singular: &[bool],
    control: &Control,
) -> Result<(Vec<DVec3>, Vec<DVec3>), Error> {
    let n = cut.vertex_count();
    let (mut normals, mut dirs) = (vec![DVec3::ZERO; n], vec![DVec3::ZERO; n]);
    let mut corners: Vec<(f64, DVec3, DVec3)> = Vec::new();
    for v in 0..n as u32 {
        if v % CANCEL_EVERY == 0 {
            control.checkpoint(Stage::Trace)?;
        }
        corners.clear();
        let p = cut.positions[v as usize];
        for &h in cut.fan(v) {
            let f = (h / 3) as usize;
            let (b, c) = (cut.head(h), cut.tail(CutMesh::prev(h)));
            let (eb, ec) = (cut.positions[b as usize] - p, cut.positions[c as usize] - p);
            let normal = eb.cross(ec).normalize_or_zero();
            let u = directions.get(f).copied().unwrap_or(DVec3::ZERO);
            if normal == DVec3::ZERO || u == DVec3::ZERO {
                continue;
            }
            corners.push((eb.angle_between(ec), normal, u));
        }
        let normal = corners.iter().fold(DVec3::ZERO, |acc, &(w, nf, _)| acc + nf * w).normalize_or_zero();
        normals[v as usize] = normal;
        if normal == DVec3::ZERO || singular.get(v as usize).copied().unwrap_or(false) {
            continue;
        }
        let first = corners.first().map_or(DVec3::ZERO, |c| transport(c.2, c.1, normal));
        let mut e1 = (first - normal * first.dot(normal)).normalize_or_zero();
        if e1 == DVec3::ZERO {
            e1 = normal.any_orthonormal_vector();
        }
        let e2 = normal.cross(e1);
        let (mut sum, mut total) = (Cx::ZERO, 0.0);
        for &(w, nf, u) in &corners {
            let t = transport(u, nf, normal);
            sum = sum + Cx::from_angle(4.0 * t.dot(e2).atan2(t.dot(e1))).scale(w);
            total += w;
        }
        if total > 0.0 && sum.norm() > 1e-9 * total {
            let theta = sum.arg() / 4.0;
            dirs[v as usize] = e1 * theta.cos() + e2 * theta.sin();
        }
    }
    Ok((normals, dirs))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testmesh::grid;

    fn graph_of(p: &[DVec3], t: &[[u32; 3]], dir: DVec3, cost: TraceCost, drift: f64) -> FieldGraph {
        let cut = CutMesh::new(p, t, &[]);
        let directions = vec![dir; t.len()];
        FieldGraph::new(&cut, &directions, &vec![false; p.len()], cost, drift, &Control::default()).unwrap()
    }

    #[test]
    fn vertex_cross_follows_constant_field() {
        let (p, t) = grid(10, 10);
        let g = graph_of(&p, &t, DVec3::X, TraceCost::Anisotropic, 10.0);
        for v in 0..p.len() as u32 {
            let cross = g.cross(v).expect("every vertex has a cross");
            let best = cross.iter().map(|d| d.dot(DVec3::X)).fold(f64::MIN, f64::max);
            assert!((best - 1.0).abs() < 1e-9, "vertex {v}: {cross:?}");
        }
    }

    #[test]
    fn arcs_follow_field_within_45_degrees() {
        let (p, t) = grid(10, 10);
        let centre = 5 * 11 + 5;
        for cost in [TraceCost::Anisotropic, TraceCost::AnglePenalty] {
            let g = graph_of(&p, &t, DVec3::X, cost, 10.0);
            let node = Node::new(centre, g.closest_dir(centre, DVec3::X));
            let arcs: Vec<&Arc> = g.arcs(node).collect();
            assert!(!arcs.is_empty());
            for arc in &arcs {
                let d = g.position(arc.to.vertex()) - g.position(centre);
                assert!(d.x > 0.0 && d.y.abs() < d.x, "{cost:?}: arc to {d}");
                // The target's direction is +x too.
                let to = g.direction(arc.to.vertex(), arc.to.dir()).unwrap();
                assert!((to.dot(DVec3::X) - 1.0).abs() < 1e-9);
            }
            let straight =
                arcs.iter().find(|a| a.to.vertex() == centre + 1).expect("arc to the +x neighbour");
            assert!((f64::from(straight.cost) - 1.0).abs() < 1e-6, "{cost:?}: {}", straight.cost);
            assert_eq!(arcs[0].to.vertex(), centre + 1, "cheapest first");
        }
    }

    #[test]
    fn two_hop_arc_cancels_zigzag() {
        let (p, t) = grid(10, 10);
        let f = DVec3::new(30f64.to_radians().cos(), 30f64.to_radians().sin(), 0.0);
        let g = graph_of(&p, &t, f, TraceCost::Anisotropic, 10.0);
        let id = |i: u32, j: u32| j * 11 + i;
        let v = id(4, 4);
        let node = Node::new(v, g.closest_dir(v, f));
        let cost_to =
            |from: Node, w: u32| g.arcs(from).find(|a| a.to.vertex() == w).map(|a| (a.cost, a.to, a.via));
        let (two, _, via) = cost_to(node, id(6, 5)).expect("2-hop arc to (6, 5)");
        assert_eq!(via, id(5, 4));
        let (first, mid, _) = cost_to(node, id(5, 4)).expect("arc to (5, 4)");
        let (second, _, _) = cost_to(mid, id(6, 5)).expect("arc (5, 4) to (6, 5)");
        assert!(two < first + second, "2-hop {two} vs {first} + {second}");
    }

    #[test]
    fn singular_vertices_have_no_nodes() {
        let (p, t) = grid(6, 6);
        let cut = CutMesh::new(&p, &t, &[]);
        let mut singular = vec![false; p.len()];
        singular[3 * 7 + 3] = true;
        let g = FieldGraph::new(
            &cut,
            &vec![DVec3::X; t.len()],
            &singular,
            TraceCost::Anisotropic,
            10.0,
            &Control::default(),
        )
        .unwrap();
        assert!(g.cross(24).is_none());
        for d in 0..4 {
            assert!(!g.active(Node::new(24, d)));
            assert_eq!(g.arcs(Node::new(24, d)).count(), 0);
        }
        for k in 0..g.node_count() as u32 {
            assert!(g.arcs(Node(k)).all(|a| a.to.vertex() != 24 && a.via != 24));
        }
    }

    #[test]
    fn degenerate_fan_vertex_has_no_nodes() {
        // Vertex 0's two faces have zero area (1, 2, 3 are collinear with 0).
        let p = [
            DVec3::ZERO,
            DVec3::X,
            DVec3::new(2.0, 0.0, 0.0),
            DVec3::new(3.0, 0.0, 0.0),
            DVec3::new(1.0, 1.0, 0.0),
        ];
        let t = [[0, 1, 2], [0, 2, 3], [1, 4, 2], [2, 4, 3]];
        let cut = CutMesh::new(&p, &t, &[]);
        let g = FieldGraph::new(
            &cut,
            &[DVec3::X; 4],
            &[false; 5],
            TraceCost::Anisotropic,
            10.0,
            &Control::default(),
        )
        .unwrap();
        assert!(g.cross(0).is_none());
        for k in 0..g.node_count() as u32 {
            for a in g.arcs(Node(k)) {
                assert!(a.cost.is_finite());
            }
        }
        for v in 0..5 {
            assert!(g.normal(v).is_finite());
        }
    }
}
