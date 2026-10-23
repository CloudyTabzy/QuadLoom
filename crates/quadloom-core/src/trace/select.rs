//! Choosing the paths of a layout.
//!
//! A [`Region`] is traced on its own: a whole part, or one patch's faces cut
//! out as a sub-mesh (whose border then acts like a feature, so new paths end
//! on it as T-junctions). Each region runs the same rounds (the paper's
//! §5.2–5.4): convexity (concave and narrow vertices emit paths until each is
//! satisfied), loops from spread-out interior seeds, and border to border.
//! Loops and border paths are taken farthest-first and only where they split
//! a patch that still fails a condition. The whole part then gets sub-patch
//! passes, a clean-up that dissolves paths whose removal makes no patch
//! worse, and a recovery ladder for whatever is still invalid.
//!
//! A part that still comes out crowded (patches holding several
//! singularities, which no pattern can place) is laid out once more with
//! crowding ranked ahead of side counts, and the less crowded layout is kept
//! (see [`lay_out`]).

use hashbrown::{HashMap, HashSet};
use std::collections::BinaryHeap;

use glam::DVec3;
use rayon::prelude::*;

use super::TraceCost;
use super::cut::CutMesh;
use super::graph::{FieldGraph, Node, transport};
use super::patches::{Partition, PatchContext, PatchInfo, PatchKind, PathEdges};
use super::sample::{DistanceField, farthest_points};
use super::search::{Occupancy, SearchPool, TracePath, cut_ears, self_intersects};
use super::turns::{FaceCrosses, VertexClass, classify_boundary};
use crate::control::{Control, Stage};
use crate::error::Error;

/// Loop seeds per vertex, clamped to [`MIN_SEEDS`, `MAX_SEEDS`] (the oracle's).
/// A sub-region gets the same density but only [`MIN_SUB_SEEDS`] at least
/// (the oracle reuses its parent's seeds inside the patch).
const MIN_SEEDS: usize = 50;
const MIN_SUB_SEEDS: usize = 4;
const MAX_SEEDS: usize = 1000;

/// Narrow and concave paths cost at most this × √area (the oracle's
/// `MaxNarrowWeight` = √area · 0.05 · Drift at its Drift of 100).
const REACH: f64 = 5.0;

/// Loops cost at most this × √area.
const LOOP_REACH: f64 = 20.0;

/// Loops are searched within this many edge rings of their seed's streamline.
const LOOP_RINGS: usize = 3;

/// Clean-up sweeps over all paths.
const REMOVAL_SWEEPS: usize = 3;

/// Rounds of recovery seams.
const MAX_SEAM_ROUNDS: usize = 64;

/// A part is laid out again with crowding first when patches holding several
/// singularities cover at least this share of it. Below it, the second layout
/// mostly trades a few tiny crowded patches for slightly worse shapes.
const CROWDED_SHARE: f64 = 0.005;

/// How a layout is chosen (from [`super::TraceParams`]).
#[derive(Clone, Debug)]
pub struct Settings {
    pub max_sides: usize,
    pub match_valence: bool,
    pub cc_tolerance: f64,
    pub loop_density: f64,
    pub sub_passes: usize,
    pub cost: TraceCost,
    pub drift: f64,
    pub cut_ears: bool,
}

/// Counts for the report.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tally {
    pub candidates: usize,
    pub removed_paths: usize,
    pub sub_passes_run: usize,
    pub relaxed_patches: usize,
    pub seams: usize,
    pub redeclared: usize,
    /// Parts whose crowding-first layout was kept (see [`lay_out`]).
    pub uncrowded: usize,
}

/// A path that was chosen.
#[derive(Clone, Debug)]
pub struct Chosen {
    pub path: TracePath,
    /// Starts or ends at a concave or narrow vertex: never removed.
    pub anchored: bool,
    pub removed: bool,
}

/// What a boundary vertex offers paths.
#[derive(Clone, Debug, Default)]
struct Port {
    class: Option<VertexClass>,
    emit: Vec<Node>,
    receive: Vec<Node>,
}

/// One region traced on its own.
#[derive(Debug)]
pub struct Region {
    pub cut: CutMesh,
    pub directions: Vec<DVec3>,
    pub crosses: FaceCrosses,
    pub graph: FieldGraph,
    pub classes: Vec<Option<VertexClass>>,
    /// Index in quarters per cut vertex.
    pub singular: Vec<i32>,
    /// Vertices where the field is singular but which are not on a barrier
    /// (so they may lie on a region boundary): turns there are geometric.
    pub geometric_turns: Vec<bool>,
    /// Each cut vertex's id in the top-level uncut mesh.
    pub merge_id: Vec<u32>,
    /// Labels of boundary half-edges that don't run along their own edge
    /// (in a sub-region, the parent's path edges), by directed cut-vertex pair.
    labels: HashMap<(u32, u32), DVec3>,
    ports: Vec<Port>,
    pub area: f64,
    pub tolerance: f64,
}

/// What a region is built from (input vertex ids).
#[derive(Clone, Debug)]
pub struct RegionMesh<'a> {
    pub positions: &'a [DVec3],
    pub triangles: &'a [[u32; 3]],
    /// Edges to cut along (features, or a parent's paths inside a patch).
    pub barrier: &'a [[u32; 2]],
    /// One direction per triangle.
    pub directions: Vec<DVec3>,
    /// Index in quarters per vertex.
    pub singular: &'a [i32],
    /// Travel directions of boundary edges that don't run along themselves.
    pub labels: &'a HashMap<(u32, u32), DVec3>,
    /// The CC-ability tolerance (a parent's, for a sub-region).
    pub tolerance: f64,
    /// Each input vertex's id in the top-level uncut mesh (None: the input
    /// ids themselves).
    pub merge_id: Option<&'a [u32]>,
}

impl Region {
    pub fn new(mesh: RegionMesh<'_>, settings: &Settings, control: &Control) -> Result<Self, Error> {
        let RegionMesh { positions, triangles, barrier, directions, singular, labels, tolerance, merge_id } =
            mesh;
        let cut = CutMesh::new(positions, triangles, barrier);
        let singular: Vec<i32> = (0..cut.vertex_count())
            .map(|v| {
                if cut.boundary[v] { 0 } else { singular.get(cut.original[v] as usize).copied().unwrap_or(0) }
            })
            .collect();
        let mut on_barrier = vec![false; positions.len()];
        for &[a, b] in barrier {
            for v in [a, b] {
                if let Some(x) = on_barrier.get_mut(v as usize) {
                    *x = true;
                }
            }
        }
        let geometric_turns: Vec<bool> = cut
            .original
            .iter()
            .map(|&o| singular.get(o as usize).is_some_and(|&k| k != 0) && !on_barrier[o as usize])
            .collect();
        let crosses = FaceCrosses::new(&cut, &directions);
        let is_singular: Vec<bool> = singular.iter().map(|&k| k != 0).collect();
        let graph = FieldGraph::new(&cut, &directions, &is_singular, settings.cost, settings.drift, control)?;
        let mut cut_labels = HashMap::new();
        if !labels.is_empty() {
            for h in cut.boundary_half_edges() {
                let (a, b) = (cut.tail(h), cut.head(h));
                let (oa, ob) = (cut.original[a as usize], cut.original[b as usize]);
                let label = labels.get(&(oa, ob)).copied().or_else(|| labels.get(&(ob, oa)).map(|&d| -d));
                if let Some(d) = label {
                    cut_labels.insert((a, b), d);
                }
            }
        }
        let area = cut
            .triangles
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|v| cut.positions[v as usize]);
                0.5 * (b - a).cross(c - a).length()
            })
            .sum();
        let mut region = Self {
            cut,
            directions,
            crosses,
            graph,
            classes: Vec::new(),
            singular,
            geometric_turns,
            merge_id: Vec::new(),
            labels: cut_labels,
            ports: Vec::new(),
            area,
            tolerance,
        };
        region.merge_id = region
            .cut
            .original
            .iter()
            .map(|&o| merge_id.and_then(|m| m.get(o as usize).copied()).unwrap_or(o))
            .collect();
        region.classes =
            classify_boundary(&region.cut, &region.crosses, &region.geometric_turns, &|h| region.label(h));
        region.init_ports();
        Ok(region)
    }

    /// Whether the region is a topological disk (one boundary loop, χ = 1).
    pub fn is_disk(&self) -> bool {
        let cut = &self.cut;
        let used: Vec<u32> = (0..cut.vertex_count() as u32).filter(|&v| !cut.fan(v).is_empty()).collect();
        let boundary: Vec<u32> = cut.boundary_half_edges().collect();
        let mut seen = vec![false; 3 * cut.face_count()];
        let mut loops = 0;
        for &h in &boundary {
            if seen[h as usize] {
                continue;
            }
            loops += 1;
            let mut e = h;
            for _ in 0..boundary.len() {
                if seen[e as usize] {
                    break;
                }
                seen[e as usize] = true;
                e = cut.next_on_boundary(e);
            }
        }
        let edges = (3 * cut.face_count() + boundary.len()) / 2;
        let chi = used.len() as i64 - edges as i64 + cut.face_count() as i64;
        loops == 1 && chi == 1
    }

    /// A boundary half-edge's travel direction.
    pub fn label(&self, h: u32) -> DVec3 {
        let (a, b) = (self.cut.tail(h), self.cut.head(h));
        self.labels
            .get(&(a, b))
            .copied()
            .unwrap_or(self.cut.positions[b as usize] - self.cut.positions[a as usize])
    }

    fn reach(&self) -> f64 {
        REACH * self.area.sqrt()
    }

    /// Emitters, receivers and twin arcs of the boundary vertices, and the
    /// boundary nodes that start inactive.
    fn init_ports(&mut self) {
        let n = self.cut.vertex_count();
        let mut ports = vec![Port::default(); n];
        let mut inward: Vec<Option<Node>> = vec![None; n];
        for v in 0..n as u32 {
            let Some(class) = self.classes[v as usize] else { continue };
            let fan = self.cut.fan(v);
            let (Some(&out), Some(&last)) = (fan.first(), fan.last()) else { continue };
            let (lin, lout) =
                (self.label(CutMesh::prev(last)).normalize_or_zero(), self.label(out).normalize_or_zero());
            let g = &self.graph;
            let port = &mut ports[v as usize];
            port.class = Some(class);
            if g.has_cross(v) {
                let node = |d: DVec3| Node::new(v, g.closest_dir(v, d));
                match class {
                    VertexClass::Flat => {
                        let e = node(g.normal(v).cross(lin + lout));
                        port.emit.push(e);
                        port.receive.push(e.opposite());
                        inward[v as usize] = Some(e);
                    }
                    VertexClass::Concave => {
                        for e in [node(lin), node(-lout)] {
                            if !port.emit.contains(&e) {
                                port.emit.push(e);
                                port.receive.push(e.opposite());
                            }
                        }
                    }
                    VertexClass::Narrow => {
                        let e = node(lin - lout);
                        port.emit.push(e);
                        port.receive.push(e.opposite());
                    }
                    VertexClass::Convex | VertexClass::Cusp => {}
                }
            }
            for d in 0..4 {
                self.graph.set_active(Node::new(v, d), false);
            }
        }
        for port in &ports {
            for &x in port.emit.iter().chain(&port.receive) {
                self.graph.set_active(x, true);
            }
        }
        // Twin arcs: from one flat copy's outward node to another flat copy's
        // inward node, when they point the same way across the cut.
        let mut pairs = Vec::new();
        for v in 0..n as u32 {
            let Some(e) = inward[v as usize] else { continue };
            let out = e.opposite();
            let dout = self.graph.direction(v, out.dir()).unwrap_or_default();
            for &w in &self.cut.twins[v as usize] {
                if let Some(ew) = inward[w as usize] {
                    let din = self.graph.direction(w, ew.dir()).unwrap_or_default();
                    if dout.dot(din) > 0.5 {
                        pairs.push((out, ew));
                    }
                }
            }
        }
        self.graph.add_twin_arcs(&pairs);
        self.ports = ports;
    }

    fn ctx(&self, settings: &Settings) -> PatchContext<'_> {
        PatchContext {
            cut: &self.cut,
            crosses: &self.crosses,
            singular: &self.singular,
            geometric_turns: &self.geometric_turns,
            merge_id: &self.merge_id,
            max_sides: settings.max_sides,
            match_valence: settings.match_valence,
            tolerance: self.tolerance,
        }
    }
}

/// The path search mode of a join.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Direct,
    Dijkstra,
}

/// The paths chosen in a region so far.
#[derive(Debug)]
pub struct Layout<'s> {
    pub region: Region,
    settings: &'s Settings,
    /// Checked inside setup work that can't return an error itself.
    control: &'s Control,
    pub chosen: Vec<Chosen>,
    occupancy: Occupancy,
    pub edges: PathEdges,
    partition: Option<Partition>,
    /// Remaining paths each concave or narrow vertex needs.
    needs: Vec<u8>,
    distances: Option<DistanceField>,
    /// Lets convex corners and border tangents receive (the oracle's `AllReceivers`).
    relaxed: bool,
    reach_scale: f64,
    min_seeds: usize,
    pub tally: Tally,
    /// Search arrays reused across rounds and parallel jobs.
    searches: SearchPool,
    /// Rank crowding ahead of side counts (see [`layout_score`]).
    crowd_first: bool,
}

impl<'s> Layout<'s> {
    pub fn new(region: Region, settings: &'s Settings, control: &'s Control) -> Self {
        let needs = region
            .ports
            .iter()
            .map(|p| {
                u8::from(
                    matches!(p.class, Some(VertexClass::Concave | VertexClass::Narrow)) && !p.emit.is_empty(),
                )
            })
            .collect();
        let mut edges = PathEdges::for_mesh(&region.cut);
        for (&(a, b), &d) in &region.labels {
            edges.insert_edge_on(&region.cut, a, b, d);
        }
        Self {
            region,
            settings,
            control,
            chosen: Vec::new(),
            occupancy: Occupancy::new(),
            edges,
            partition: None,
            needs,
            distances: None,
            relaxed: false,
            reach_scale: 1.0,
            min_seeds: MIN_SEEDS,
            tally: Tally::default(),
            searches: SearchPool::default(),
            crowd_first: false,
        }
    }

    fn reach(&self) -> f64 {
        self.region.reach() * self.reach_scale
    }

    /// The current partition (built on first use, then kept up to date).
    pub fn partition(&mut self) -> &Partition {
        let (region, settings, edges) = (&self.region, self.settings, &self.edges);
        self.partition.get_or_insert_with(|| Partition::new(&region.ctx(settings), edges))
    }

    /// Faces on either side of `path`'s mesh edges.
    fn touched(&self, path: &TracePath) -> Vec<u32> {
        let cut = &self.region.cut;
        let mut faces: Vec<u32> = path
            .edges(cut)
            .flat_map(|(a, b)| [cut.half_edge(a, b), cut.half_edge(b, a)])
            .flatten()
            .map(|h| h / 3)
            .collect();
        faces.sort_unstable();
        faces.dedup();
        faces
    }

    fn update_partition(&mut self, faces: &[u32]) {
        if let Some(mut partition) = self.partition.take() {
            let ctx = self.region.ctx(self.settings);
            partition.update(&ctx, &self.edges, faces);
            self.partition = Some(partition);
        }
    }

    /// Adds `path` to the layout.
    fn accept(&mut self, mut path: TracePath, anchored: bool) {
        if self.settings.cut_ears {
            let (occupancy, edges, cut) = (&self.occupancy, &self.edges, &self.region.cut);
            cut_ears(&mut path, cut, &|a, b| occupancy.has_edge(a, b) || edges.contains(a, b));
        }
        self.accept_with(path, anchored, true);
    }

    fn accept_with(&mut self, path: TracePath, anchored: bool, repartition: bool) {
        let id = self.chosen.len() as u32;
        self.occupancy.add(id, &path, &self.region.cut);
        self.edges.insert(&path, &self.region.cut, &self.region.graph);
        for node in &path.nodes {
            self.region.graph.set_active(*node, false);
            self.region.graph.set_active(node.opposite(), false);
        }
        if !path.closed {
            for v in [path.nodes[0].vertex(), path.nodes[path.nodes.len() - 1].vertex()] {
                if let Some(n) = self.needs.get_mut(v as usize) {
                    *n = n.saturating_sub(1);
                }
            }
        }
        if repartition {
            let touched = self.touched(&path);
            self.update_partition(&touched);
        }
        if let Some(d) = self.distances.as_mut() {
            d.add_sources(path.vertices(), self.control);
        }
        self.chosen.push(Chosen { path, anchored, removed: false });
    }

    /// Adds `path` if that leaves the patches it splits no worse (see
    /// [`Self::no_worse`]): a loop that only turns one annulus into two, say,
    /// is refused.
    fn try_accept(&mut self, path: TracePath) -> bool {
        self.try_accept_if(path, false, std::cmp::Ordering::is_le)
    }

    /// Adds `path` if comparing the patches it splits, after against before,
    /// passes `keep` (see [`Self::compare`]).
    fn try_accept_if(
        &mut self,
        mut path: TracePath,
        anchored: bool,
        keep: fn(std::cmp::Ordering) -> bool,
    ) -> bool {
        if self.settings.cut_ears {
            let (occupancy, edges, cut) = (&self.occupancy, &self.edges, &self.region.cut);
            cut_ears(&mut path, cut, &|a, b| occupancy.has_edge(a, b) || edges.contains(a, b));
        }
        let touched = self.touched(&path);
        self.partition();
        let Some(mut partition) = self.partition.take() else { return false };
        let ctx = self.region.ctx(self.settings);
        self.edges.insert(&path, &self.region.cut, &self.region.graph);
        let change = partition.update(&ctx, &self.edges, &touched);
        let after: Vec<&PatchInfo> = change.added.iter().filter_map(|&p| partition.info(p)).collect();
        if keep(self.compare(&change.removed, &after)) {
            self.partition = Some(partition);
            self.edges.remove(&path, &self.region.cut);
            // The partition already has the path: accept without updating it again.
            let saved = self.partition.take();
            self.accept_with(path, anchored, false);
            self.partition = saved;
            true
        } else {
            self.edges.remove(&path, &self.region.cut);
            partition.update(&ctx, &self.edges, &touched);
            self.partition = Some(partition);
            false
        }
    }

    /// Whether `v` is a concave or narrow vertex (one that paths fix).
    fn needs_path(&self, v: u32) -> bool {
        matches!(self.region.ports[v as usize].class, Some(VertexClass::Concave | VertexClass::Narrow))
    }

    fn anchored(&self, path: &TracePath) -> bool {
        !path.closed
            && [path.nodes[0].vertex(), path.nodes[path.nodes.len() - 1].vertex()].iter().any(|&v| {
                matches!(
                    self.region.ports[v as usize].class,
                    Some(VertexClass::Concave | VertexClass::Narrow)
                )
            })
    }

    /// Emitter nodes of `class` vertices (unsatisfied ones for concave and narrow).
    fn emitters(&self, class: VertexClass) -> Vec<Node> {
        let g = &self.region.graph;
        self.region
            .ports
            .iter()
            .enumerate()
            .filter(|(v, p)| p.class == Some(class) && (class == VertexClass::Flat || self.needs[*v] > 0))
            .flat_map(|(_, p)| p.emit.iter().copied())
            .filter(|&n| g.active(n))
            .collect()
    }

    /// Receiver nodes for paths ending at `class` vertices.
    fn receivers(&self, class: VertexClass) -> Vec<bool> {
        let g = &self.region.graph;
        let mut is = vec![false; g.node_count()];
        for (v, p) in self.region.ports.iter().enumerate() {
            let Some(c) = p.class else { continue };
            if c == class && (class == VertexClass::Flat || self.needs[v] > 0) {
                for &x in &p.receive {
                    is[x.0 as usize] = true;
                }
            }
            // Relaxed: every node of a convex corner or flat vertex receives.
            if self.relaxed
                && class == VertexClass::Flat
                && matches!(c, VertexClass::Convex | VertexClass::Flat)
            {
                for d in 0..4 {
                    is[Node::new(v as u32, d).0 as usize] = g.has_cross(v as u32);
                }
            }
        }
        is
    }

    /// Activates (or deactivates) the nodes only relaxed mode lets paths end at.
    fn set_relaxed(&mut self, on: bool) {
        self.relaxed = on;
        let g = &mut self.region.graph;
        for (v, p) in self.region.ports.iter().enumerate() {
            if matches!(p.class, Some(VertexClass::Convex | VertexClass::Flat)) {
                for d in 0..4 {
                    let x = Node::new(v as u32, d);
                    let own = p.emit.contains(&x) || p.receive.contains(&x);
                    if !own {
                        g.set_active(x, on);
                    }
                }
            }
        }
        // Chosen paths keep their nodes off.
        for c in self.chosen.iter().filter(|c| !c.removed) {
            for node in &c.path.nodes {
                g.set_active(*node, false);
                g.set_active(node.opposite(), false);
            }
        }
    }

    /// Traces one candidate from each start.
    fn trace(
        &self,
        starts: &[Node],
        mode: Mode,
        receivers: &[bool],
        reach: f64,
        control: &Control,
    ) -> Result<Vec<TracePath>, Error> {
        let g = &self.region.graph;
        let target = |n: Node| receivers[n.0 as usize];
        let found: Vec<Option<TracePath>> = starts
            .par_iter()
            .with_min_len(16)
            .map_init(
                || self.searches.search(g),
                |search, &s| match mode {
                    Mode::Dijkstra => search.shortest(&[s], &target, reach, control),
                    Mode::Direct => Ok(search.direct(s, &target, g.vertex_count())),
                },
            )
            .collect::<Result<_, _>>()?;
        Ok(self.keep_valid(found))
    }

    fn keep_valid(&self, found: Vec<Option<TracePath>>) -> Vec<TracePath> {
        let cut = &self.region.cut;
        found
            .into_iter()
            .flatten()
            .filter(|p| {
                p.nodes.len() >= 2 && (p.closed || p.nodes[0].vertex() != p.nodes[p.nodes.len() - 1].vertex())
            })
            .filter(|p| !self_intersects(p, cut))
            .collect()
    }

    /// A join that fixes vertex needs: candidates by ascending cost.
    fn join_needs(
        &mut self,
        from: VertexClass,
        to: VertexClass,
        mode: Mode,
        control: &Control,
    ) -> Result<usize, Error> {
        let starts = self.emitters(from);
        if starts.is_empty() {
            return Ok(0);
        }
        let receivers = self.receivers(to);
        let mut candidates = self.trace(&starts, mode, &receivers, self.reach(), control)?;
        self.tally.candidates += candidates.len();
        candidates.sort_by(|a, b| a.cost.total_cmp(&b.cost).then(a.nodes[0].cmp(&b.nodes[0])));
        let mut added = 0;
        for path in candidates {
            control.checkpoint(Stage::Trace)?;
            let (s, e) = (path.nodes[0].vertex(), path.nodes[path.nodes.len() - 1].vertex());
            // The start must still need a path, and a path may only end at a
            // concave or narrow vertex that still needs one: candidates were
            // traced together, so an earlier one may have satisfied it.
            if self.needs[s as usize] == 0 || (self.needs_path(e) && self.needs[e as usize] == 0) {
                continue;
            }
            if self.occupancy.collides(&path, &self.region.cut) {
                continue;
            }
            let anchored = self.anchored(&path);
            self.accept(path, anchored);
            added += 1;
        }
        Ok(added)
    }

    /// The paper's convexity round: narrow joins until none is added, then
    /// concave joins likewise.
    fn convexity(&mut self, control: &Control) -> Result<usize, Error> {
        use VertexClass::{Concave, Flat, Narrow};
        let mut total = 0;
        for steps in [
            &[
                (Narrow, Narrow, Mode::Dijkstra),
                (Narrow, Concave, Mode::Dijkstra),
                (Narrow, Flat, Mode::Direct),
                (Narrow, Flat, Mode::Dijkstra),
            ][..],
            &[
                (Concave, Concave, Mode::Dijkstra),
                (Concave, Flat, Mode::Direct),
                (Concave, Flat, Mode::Dijkstra),
            ][..],
        ] {
            loop {
                let mut added = 0;
                for &(from, to, mode) in steps {
                    added += self.join_needs(from, to, mode, control)?;
                }
                total += added;
                if added == 0 {
                    break;
                }
            }
        }
        Ok(total)
    }

    /// Whether `path` runs through a patch that doesn't yet meet every condition.
    fn splits_failing_patch(&mut self, path: &TracePath) -> bool {
        let cut = &self.region.cut;
        let edges: Vec<(u32, u32)> = path.edges(cut).collect();
        let sides: Vec<(u32, u32)> = edges
            .iter()
            .filter_map(|&(a, b)| Some((cut.half_edge(a, b)? / 3, cut.half_edge(b, a)? / 3)))
            .collect();
        let partition = self.partition();
        sides.iter().any(|&(f, g)| {
            let (p, q) = (partition.patch_of_face()[f as usize], partition.patch_of_face()[g as usize]);
            p == q && partition.info(p).is_some_and(|i| i.kind != PatchKind::Ok)
        })
    }

    fn distance_field(&mut self) -> &mut DistanceField {
        let (cut, chosen, singular, control) =
            (&self.region.cut, &self.chosen, &self.region.singular, self.control);
        self.distances.get_or_insert_with(|| {
            let mut field = DistanceField::new(cut);
            // Boundaries and singularities repel paths (the oracle's `away_from_singular`).
            field.add_sources(
                (0..cut.vertex_count() as u32)
                    .filter(|&v| cut.boundary[v as usize] || singular[v as usize] != 0),
                control,
            );
            for c in chosen.iter().filter(|c| !c.removed) {
                field.add_sources(c.path.vertices(), control);
            }
            field
        })
    }

    fn mean_distance(&mut self, path: &TracePath) -> f64 {
        let field = self.distance_field();
        path.vertices().map(|v| field.get(v)).filter(|d| d.is_finite()).sum::<f64>()
            / path.nodes.len().max(1) as f64
    }

    /// Candidates farthest-first (lazily re-scored as paths are added), each
    /// taken if it splits a failing patch without colliding.
    fn choose_farthest(&mut self, candidates: Vec<TracePath>, control: &Control) -> Result<usize, Error> {
        self.tally.candidates += candidates.len();
        #[derive(PartialEq)]
        struct Scored(f64, usize);
        impl Eq for Scored {}
        impl Ord for Scored {
            fn cmp(&self, o: &Self) -> std::cmp::Ordering {
                self.0.total_cmp(&o.0).then(o.1.cmp(&self.1))
            }
        }
        impl PartialOrd for Scored {
            fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
                Some(self.cmp(o))
            }
        }
        let mut heap: BinaryHeap<Scored> = BinaryHeap::with_capacity(candidates.len());
        for (i, p) in candidates.iter().enumerate() {
            let s = self.mean_distance(p);
            heap.push(Scored(s, i));
        }
        let mut slots: Vec<Option<TracePath>> = candidates.into_iter().map(Some).collect();
        let mut added = 0;
        while let Some(Scored(score, i)) = heap.pop() {
            control.checkpoint(Stage::Trace)?;
            let Some(path) = slots[i].take() else { continue };
            let fresh = self.mean_distance(&path);
            if fresh < score && heap.peek().is_some_and(|top| top.0 > fresh) {
                slots[i] = Some(path);
                heap.push(Scored(fresh, i));
                continue;
            }
            if self.occupancy.collides(&path, &self.region.cut) || !self.splits_failing_patch(&path) {
                continue;
            }
            if self.try_accept(path) {
                added += 1;
            }
        }
        Ok(added)
    }

    /// Loops from spread-out interior seeds, two directions each.
    fn loops(&mut self, control: &Control) -> Result<usize, Error> {
        let cut = &self.region.cut;
        let g = &self.region.graph;
        let eligible = |v: u32| !cut.boundary[v as usize] && g.has_cross(v) && g.active(Node::new(v, 0));
        let interior = (0..cut.vertex_count() as u32).filter(|&v| eligible(v)).count();
        let want = ((cut.vertex_count() as f64 * self.settings.loop_density).round() as usize)
            .clamp(self.min_seeds, MAX_SEEDS)
            .min(interior);
        let seeds = farthest_points(cut, &eligible, want, control)?;
        let starts: Vec<Node> =
            seeds.iter().flat_map(|&v| [Node::new(v, 0), Node::new(v, 1)]).filter(|&n| g.active(n)).collect();
        let reach = LOOP_REACH * self.region.area.sqrt();
        let found: Vec<Option<TracePath>> = starts
            .par_iter()
            .with_min_len(8)
            .map_init(
                || self.searches.search(g),
                |search, &s| search.guided_loop(cut, s, reach, LOOP_RINGS, control),
            )
            .collect::<Result<_, _>>()?;
        let candidates = self.keep_valid(found);
        let added = self.choose_farthest(candidates, control)?;
        // A small closed part can have no seed whose streamline closes; with
        // no boundary, nothing else can cut it, so search loops in full there.
        let closed = !self.region.cut.boundary.iter().any(|&b| b);
        if added > 0 || !closed || self.chosen.iter().any(|c| !c.removed) {
            return Ok(added);
        }
        let g = &self.region.graph;
        let found: Vec<Option<TracePath>> = starts
            .par_iter()
            .with_min_len(8)
            .map_init(|| self.searches.search(g), |search, &s| search.closed_loop(s, reach, control))
            .collect::<Result<_, _>>()?;
        let candidates = self.keep_valid(found);
        self.choose_farthest(candidates, control)
    }

    /// Border-to-border paths, direct then shortest, until none is added.
    fn borders(&mut self, control: &Control) -> Result<usize, Error> {
        let mut total = 0;
        loop {
            let mut added = 0;
            for mode in [Mode::Direct, Mode::Dijkstra] {
                let starts = self.emitters(VertexClass::Flat);
                if starts.is_empty() {
                    continue;
                }
                let receivers = self.receivers(VertexClass::Flat);
                let candidates = self.trace(&starts, mode, &receivers, self.reach(), control)?;
                added += self.choose_farthest(candidates, control)?;
            }
            total += added;
            if added == 0 {
                return Ok(total);
            }
        }
    }

    /// One batch of rounds (the oracle's `BatchAddLoops`). `force` first runs
    /// the convexity round with every border node receiving, over short
    /// reach; `relaxed` keeps that for the whole batch, without a cost bound.
    pub fn run_rounds(&mut self, force: bool, relaxed: bool, control: &Control) -> Result<usize, Error> {
        let mut added = 0;
        if force {
            self.set_relaxed(true);
            let scale = self.reach_scale;
            self.reach_scale = if relaxed { f64::INFINITY } else { scale / 100.0 };
            added += self.convexity(control)?;
            self.reach_scale = scale;
            self.set_relaxed(relaxed);
        }
        if relaxed {
            self.reach_scale = f64::INFINITY;
        }
        added += self.convexity(control)?;
        // In a disk, a loop that crosses no path only cuts out an annulus,
        // which the acceptance test always refuses: skip the round.
        if !self.region.is_disk() {
            added += self.loops(control)?;
        }
        added += self.borders(control)?;
        Ok(added)
    }

    /// The patches still failing, with their faces.
    fn failing(&mut self, only_non_disk: bool, only_invalid: bool) -> Vec<(u32, Vec<u32>)> {
        self.partition()
            .patches()
            .filter(|(_, i)| match (only_non_disk, only_invalid) {
                (true, _) => i.kind == PatchKind::NonDisk,
                (false, true) => !i.kind.is_valid(),
                (false, false) => i.kind != PatchKind::Ok,
            })
            .map(|(p, i)| (p, i.faces.clone()))
            .collect()
    }

    /// Traces inside each failing patch on its own until nothing changes
    /// (the oracle's `SolveSubPatches`), at most `sub_passes` times.
    pub fn sub_patch_passes(
        &mut self,
        only_non_disk: bool,
        relaxed: bool,
        control: &Control,
    ) -> Result<(), Error> {
        let mut exhausted: HashSet<u32> = HashSet::new();
        let mut boosted = false;
        for _ in 0..self.settings.sub_passes {
            let todo: Vec<(u32, Vec<u32>)> = self
                .failing(only_non_disk, false)
                .into_iter()
                .filter(|(_, f)| !exhausted.contains(&f[0]))
                .collect();
            if todo.is_empty() {
                break;
            }
            self.tally.sub_passes_run += 1;
            let reach = self.reach() * if boosted { 100.0 } else { 1.0 };
            let this = &*self;
            let results: Vec<Result<(Vec<TracePath>, usize), Error>> =
                todo.par_iter().map(|(_, faces)| this.solve_patch(faces, relaxed, reach, control)).collect();
            let mut added = 0;
            for ((_, faces), result) in todo.iter().zip(results) {
                let (paths, candidates) = result?;
                self.tally.candidates += candidates;
                if relaxed {
                    self.tally.relaxed_patches += 1;
                }
                let mut any = false;
                for path in paths {
                    if !self.occupancy.collides(&path, &self.region.cut)
                        && !self_intersects(&path, &self.region.cut)
                    {
                        // Judged again here: a path that is fine inside its
                        // patch can still pinch the patch's surroundings.
                        let anchored = self.anchored(&path);
                        if self.try_accept_if(path, anchored, std::cmp::Ordering::is_le) {
                            any = true;
                            added += 1;
                        }
                    }
                }
                if !any {
                    exhausted.insert(faces[0]);
                }
            }
            if added == 0 {
                let unconvex = self.partition().patches().any(|(_, i)| i.kind == PatchKind::NotConvex);
                if !only_non_disk && !boosted && unconvex {
                    boosted = true;
                    exhausted.clear();
                    continue;
                }
                break;
            }
        }
        Ok(())
    }

    /// Paths traced inside the patch made of `faces`, in this region's ids.
    fn solve_patch(
        &self,
        faces: &[u32],
        relaxed: bool,
        reach: f64,
        control: &Control,
    ) -> Result<(Vec<TracePath>, usize), Error> {
        let cut = &self.region.cut;
        let mut local: HashMap<u32, u32> = HashMap::new();
        let mut back: Vec<u32> = Vec::new();
        let mut triangles = Vec::with_capacity(faces.len());
        let mut directions = Vec::with_capacity(faces.len());
        for &f in faces {
            let t = cut.triangles[f as usize].map(|v| {
                *local.entry(v).or_insert_with(|| {
                    back.push(v);
                    (back.len() - 1) as u32
                })
            });
            triangles.push(t);
            directions.push(self.region.directions[f as usize]);
        }
        let positions: Vec<DVec3> = back.iter().map(|&v| cut.positions[v as usize]).collect();
        let singular: Vec<i32> = back.iter().map(|&v| self.region.singular[v as usize]).collect();
        let in_patch: HashSet<u32> = faces.iter().copied().collect();
        let mut barrier = Vec::new();
        let mut labels = HashMap::new();
        for &f in faces {
            for h in 3 * f..3 * f + 3 {
                let (a, b) = (cut.tail(h), cut.head(h));
                let (Some(&la), Some(&lb)) = (local.get(&a), local.get(&b)) else { continue };
                match cut.twin(h) {
                    Some(t) if self.edges.contains(a, b) => {
                        if in_patch.contains(&(t / 3)) && a < b {
                            barrier.push([la, lb]);
                        }
                        if let Some(d) = self.edges.label(a, b) {
                            labels.insert((la, lb), d);
                        }
                    }
                    None => {
                        labels.insert((la, lb), self.region.label(h));
                    }
                    _ => {}
                }
            }
        }
        let merge: Vec<u32> = back.iter().map(|&v| self.region.merge_id[v as usize]).collect();
        let mesh = RegionMesh {
            positions: &positions,
            triangles: &triangles,
            barrier: &barrier,
            directions,
            singular: &singular,
            labels: &labels,
            tolerance: self.region.tolerance,
            merge_id: Some(&merge),
        };
        let sub = Region::new(mesh, self.settings, control)?;
        let mut layout = Layout::new(sub, self.settings, control);
        layout.min_seeds = MIN_SUB_SEEDS;
        layout.crowd_first = self.crowd_first;
        layout.reach_scale = reach / layout.region.reach().max(f64::MIN_POSITIVE);
        layout.run_rounds(true, relaxed, control)?;
        let g = &self.region.graph;
        let sub_g = &layout.region.graph;
        let mut paths = Vec::new();
        for c in layout.chosen.iter().filter(|c| !c.removed) {
            let mut nodes: Vec<Node> = Vec::with_capacity(c.path.nodes.len());
            for node in &c.path.nodes {
                let v = back[layout.region.cut.original[node.vertex() as usize] as usize];
                let d = sub_g.direction(node.vertex(), node.dir()).unwrap_or_default();
                let mapped = Node::new(v, g.closest_dir(v, d));
                if nodes.last().is_none_or(|last: &Node| last.vertex() != v) {
                    nodes.push(mapped);
                }
            }
            paths.push(TracePath { nodes, closed: c.path.closed, cost: c.path.cost });
        }
        Ok((paths, layout.tally.candidates))
    }

    /// Whether `after` is no worse than `before` (see [`layout_score`]).
    fn no_worse(&self, before: &[PatchInfo], after: &[&PatchInfo]) -> bool {
        self.compare(before, after).is_le()
    }

    /// How `after` compares with `before` (less is better; see [`layout_score`]).
    fn compare(&self, before: &[PatchInfo], after: &[&PatchInfo]) -> std::cmp::Ordering {
        let (max, crowd_first) = (self.settings.max_sides, self.crowd_first);
        let b = layout_score(before.iter(), max, crowd_first);
        let a = layout_score(after.iter().copied(), max, crowd_first);
        a.cmp(&b)
    }

    /// The share of the region's area in patches holding several singularities.
    fn crowded_share(&mut self) -> f64 {
        self.partition();
        let (Some(partition), cut) = (&self.partition, &self.region.cut) else { return 0.0 };
        let crowded: f64 = partition
            .patches()
            .filter(|(_, i)| crowding(i) > 0)
            .flat_map(|(_, i)| &i.faces)
            .map(|&f| {
                let [a, b, c] = cut.triangles[f as usize].map(|v| cut.positions[v as usize]);
                0.5 * (b - a).cross(c - a).length()
            })
            .sum();
        crowded / self.region.area.max(f64::MIN_POSITIVE)
    }

    /// The finished layout's score with crowding ranked first, for choosing
    /// between two layouts of the same part.
    fn final_score(&mut self) -> [i64; 8] {
        let max = self.settings.max_sides;
        layout_score(self.partition().patches().map(|(_, i)| i), max, true)
    }

    /// Whether removing chosen path `k` would leave another path dangling.
    fn would_dangle(&self, k: usize) -> bool {
        let path = &self.chosen[k].path;
        let cut = &self.region.cut;
        let mut own: HashMap<u32, u32> = HashMap::new();
        for (a, b) in path.edges(cut) {
            *own.entry(a).or_default() += 1;
            *own.entry(b).or_default() += 1;
        }
        own.iter()
            .any(|(&v, &mine)| !cut.boundary[v as usize] && self.edges.degree(v).saturating_sub(mine) == 1)
    }

    /// Dissolves paths, newest first, whenever that leaves the affected
    /// patches no worse (the paper's clean-up of redundant paths).
    pub fn clean_up(&mut self, control: &Control) -> Result<(), Error> {
        self.partition();
        for _ in 0..REMOVAL_SWEEPS {
            let mut removed_any = false;
            for k in (0..self.chosen.len()).rev() {
                control.checkpoint(Stage::Trace)?;
                if self.chosen[k].removed || self.chosen[k].anchored || self.would_dangle(k) {
                    continue;
                }
                let path = self.chosen[k].path.clone();
                let touched = self.touched(&path);
                self.edges.remove(&path, &self.region.cut);
                self.occupancy.remove(k as u32, &path, &self.region.cut);
                let Some(mut partition) = self.partition.take() else { break };
                let ctx = self.region.ctx(self.settings);
                let change = partition.update(&ctx, &self.edges, &touched);
                let after: Vec<&PatchInfo> = change.added.iter().filter_map(|&p| partition.info(p)).collect();
                let keep_removed = self.no_worse(&change.removed, &after);
                if keep_removed {
                    self.partition = Some(partition);
                    self.chosen[k].removed = true;
                    self.tally.removed_paths += 1;
                    removed_any = true;
                } else {
                    self.edges.insert(&path, &self.region.cut, &self.region.graph);
                    self.occupancy.add(k as u32, &path, &self.region.cut);
                    partition.update(&ctx, &self.edges, &touched);
                    self.partition = Some(partition);
                }
            }
            if !removed_any {
                break;
            }
        }
        self.distances = None;
        Ok(())
    }

    /// The recovery ladder for patches that are still invalid: a relaxed
    /// sub-pass, then a seam for each remaining non-disk patch.
    pub fn recover(&mut self, control: &Control) -> Result<(), Error> {
        if !self.failing(false, true).is_empty() {
            let passes = self.settings.sub_passes.max(1);
            for _ in 0..passes {
                let todo = self.failing(false, true);
                if todo.is_empty() {
                    break;
                }
                let this = &*self;
                let results: Vec<Result<(Vec<TracePath>, usize), Error>> = todo
                    .par_iter()
                    .map(|(_, faces)| this.solve_patch(faces, true, f64::INFINITY, control))
                    .collect();
                let mut added = 0;
                for result in results {
                    let (paths, candidates) = result?;
                    self.tally.candidates += candidates;
                    self.tally.relaxed_patches += 1;
                    for path in paths {
                        if !self.occupancy.collides(&path, &self.region.cut)
                            && !self_intersects(&path, &self.region.cut)
                            && self.try_accept_if(path, true, std::cmp::Ordering::is_le)
                        {
                            added += 1;
                        }
                    }
                }
                if added == 0 {
                    break;
                }
            }
        }
        for _ in 0..MAX_SEAM_ROUNDS {
            control.checkpoint(Stage::Trace)?;
            let invalid: Vec<PatchInfo> = self
                .partition()
                .patches()
                .filter(|(_, i)| !i.kind.is_valid())
                .map(|(_, i)| i.clone())
                .collect();
            let mut added = 0;
            for info in &invalid {
                // A seam runs through the patch's interior, so it can only meet
                // other paths at its ends: no collision test. It must strictly
                // improve the patch, or seams could cut lenses into lenses forever.
                let seam_found = self.repair_seam(info);
                if let Some(seam) = seam_found
                    && self.try_accept_if(seam, true, std::cmp::Ordering::is_lt)
                {
                    self.tally.seams += 1;
                    added += 1;
                }
            }
            if added == 0 {
                break;
            }
        }
        Ok(())
    }

    /// The last resort for an invalid patch: a cut that is not field-aligned
    /// (and may pass through singular vertices) but always brings it closer
    /// to valid. A non-disk patch is cut between two of its boundary loops
    /// (merged loops, if it is a disk only cut open along a feature); a
    /// non-convex one from the offending vertex to the far half of its
    /// boundary. Side counts out of range are fixed by re-declaring corners
    /// instead, which moves no geometry.
    fn repair_seam(&self, info: &PatchInfo) -> Option<TracePath> {
        let loops = &info.loop_vertices;
        let (sources, targets): (Vec<u32>, Vec<u32>) = match info.kind {
            PatchKind::NonDisk if loops.len() >= 2 => {
                let (first, rest) = loops.split_first()?;
                (first.clone(), rest.iter().flatten().copied().collect())
            }
            PatchKind::NonDisk => match self.merged_loops(info) {
                Some(split) => split,
                None => {
                    // Pinched at a vertex P its loop passes twice: like an
                    // annulus whose two borders touch at P. A cut from the
                    // loop's arc on one side of P to the arc on the other,
                    // with the contact at P, splits it into two disks.
                    let ring = loops.first()?;
                    let merged = |v: u32| self.region.merge_id[v as usize];
                    let m = ring.len();
                    let mut first_seen: HashMap<u32, usize> = HashMap::with_capacity(m);
                    let (i, j) =
                        (0..m).find_map(|j| first_seen.insert(merged(ring[j]), j).map(|i| (i, j)))?;
                    let pinch = merged(ring[i]);
                    let arc = |from: usize, to: usize| -> Vec<u32> {
                        (from + 1..to).map(|k| ring[k % m]).filter(|&v| merged(v) != pinch).collect()
                    };
                    (arc(i, j), arc(j, i + m))
                }
            },
            PatchKind::NotConvex => return self.concave_seam(info),
            _ => return None,
        };
        if sources.is_empty() || targets.is_empty() {
            return None;
        }
        self.seam_path(&info.faces, &sources, &targets)
    }

    /// For a patch that is a disk only cut open along a feature: its boundary
    /// vertices on the first merged boundary loop, and those on the others.
    fn merged_loops(&self, info: &PatchInfo) -> Option<(Vec<u32>, Vec<u32>)> {
        let cut = &self.region.cut;
        let merged = |v: u32| self.region.merge_id[v as usize];
        let key = |a: u32, b: u32| (merged(a).min(merged(b)), merged(a).max(merged(b)));
        let mut uses: HashMap<(u32, u32), u32> = HashMap::new();
        for &f in &info.faces {
            for h in 3 * f..3 * f + 3 {
                *uses.entry(key(cut.tail(h), cut.head(h))).or_default() += 1;
            }
        }
        // Merged boundary edges are used once; group their vertices into loops.
        let mut sets = crate::unionfind::UnionFind::new(cut.vertex_count());
        let mut on: Vec<u32> = Vec::new();
        let mut by_merged: HashMap<u32, u32> = HashMap::new();
        for &f in &info.faces {
            for h in 3 * f..3 * f + 3 {
                let (a, b) = (cut.tail(h), cut.head(h));
                if uses.get(&key(a, b)) == Some(&1) {
                    sets.union(a, b);
                    on.extend([a, b]);
                }
            }
        }
        // Copies of one vertex are on the same merged loop.
        for &v in &on {
            match by_merged.get(&merged(v)) {
                Some(&w) => sets.union(v, w),
                None => {
                    by_merged.insert(merged(v), v);
                }
            }
        }
        on.sort_unstable();
        on.dedup();
        let root = sets.find(*on.first()?);
        let (first, rest): (Vec<u32>, Vec<u32>) = on.iter().partition(|&&v| sets.find(v) == root);
        (!rest.is_empty()).then_some((first, rest))
    }

    /// A seam out of a non-convex boundary vertex, leaving along the
    /// continuation of its incoming boundary edge (so a 270° corner splits
    /// into 180° and 90°), then to the far half of the boundary.
    fn concave_seam(&self, info: &PatchInfo) -> Option<TracePath> {
        let cut = &self.region.cut;
        let ring = info.loop_vertices.first()?;
        let &(v, class) = info.not_convex.iter().find(|(_, c)| *c != VertexClass::Cusp)?;
        let _ = class;
        let i = ring.iter().position(|&x| x == v)?;
        let a = ring[(i + ring.len() - 1) % ring.len()];
        let ahead = (cut.positions[v as usize] - cut.positions[a as usize]).normalize_or_zero();
        let in_patch: HashSet<u32> = info.faces.iter().copied().collect();
        let first = cut
            .fan(v)
            .iter()
            .filter(|&&h| {
                let w = cut.head(h);
                cut.twin(h).is_some_and(|t| in_patch.contains(&(h / 3)) && in_patch.contains(&(t / 3)))
                    && !self.edges.contains(v, w)
            })
            .map(|&h| cut.head(h))
            .max_by(|&x, &y| {
                let d = |w: u32| {
                    (cut.positions[w as usize] - cut.positions[v as usize]).normalize_or_zero().dot(ahead)
                };
                d(x).total_cmp(&d(y)).then(y.cmp(&x))
            })?;
        let targets = far_side(ring, v, 4);
        let along = |x: u32, y: u32| cut.positions[y as usize] - cut.positions[x as usize];
        if cut.boundary[first as usize] || ring.contains(&first) {
            if !targets.contains(&first) {
                return None;
            }
            let nodes = self.axis_nodes(&[v, first], ahead);
            return Some(TracePath { nodes, closed: false, cost: along(v, first).length() });
        }
        let rest = self.seam_path(&info.faces, &[first], &targets)?;
        // The whole seam follows the continued boundary edge's field axis.
        let chain: Vec<u32> = std::iter::once(v).chain(rest.vertices()).collect();
        let nodes = self.axis_nodes(&chain, ahead);
        Some(TracePath { nodes, closed: false, cost: rest.cost })
    }

    /// Nodes along `chain` that all follow one field axis: `first` snapped
    /// at the first vertex, then carried from vertex to vertex like a traced
    /// path's direction, so the chain has no corners of its own.
    fn axis_nodes(&self, chain: &[u32], first: DVec3) -> Vec<Node> {
        let g = &self.region.graph;
        let mut nodes = Vec::with_capacity(chain.len());
        let mut d = first;
        for (i, &v) in chain.iter().enumerate() {
            if i > 0 {
                d = transport(d, g.normal(chain[i - 1]), g.normal(v));
            }
            let k = g.closest_dir(v, d);
            if let Some(snapped) = g.direction(v, k) {
                d = snapped;
            }
            nodes.push(Node::new(v, k));
        }
        nodes
    }

    /// The shortest path from `sources` to `targets` over edges with both
    /// faces in the patch made of `faces` and on no path, passing only
    /// through the patch's interior.
    fn seam_path(&self, faces: &[u32], sources: &[u32], targets: &[u32]) -> Option<TracePath> {
        let cut = &self.region.cut;
        let g = &self.region.graph;
        let in_patch: HashSet<u32> = faces.iter().copied().collect();
        let target: HashSet<u32> = targets.iter().copied().collect();
        let source: HashSet<u32> = sources.iter().copied().collect();
        let is_cut = |h: u32| cut.twin(h).is_none() || self.edges.contains(cut.tail(h), cut.head(h));
        let on_boundary: HashSet<u32> = faces
            .iter()
            .flat_map(|&f| 3 * f..3 * f + 3)
            .filter(|&h| is_cut(h))
            .map(|h| cut.tail(h))
            .collect();
        let mut dist: HashMap<u32, f64> = HashMap::new();
        let mut parent: HashMap<u32, u32> = HashMap::new();
        let mut heap = BinaryHeap::new();
        #[derive(PartialEq)]
        struct E(f64, u32);
        impl Eq for E {}
        impl Ord for E {
            fn cmp(&self, o: &Self) -> std::cmp::Ordering {
                o.0.total_cmp(&self.0).then(o.1.cmp(&self.1))
            }
        }
        impl PartialOrd for E {
            fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
                Some(self.cmp(o))
            }
        }
        for &s in sources {
            dist.insert(s, 0.0);
            heap.push(E(0.0, s));
        }
        let mut end = None;
        while let Some(E(d, v)) = heap.pop() {
            if d > dist.get(&v).copied().unwrap_or(f64::INFINITY) {
                continue;
            }
            if target.contains(&v) && !source.contains(&v) {
                end = Some(v);
                break;
            }
            if on_boundary.contains(&v) && !source.contains(&v) {
                continue;
            }
            for &h in cut.fan(v) {
                let w = cut.head(h);
                let inside =
                    cut.twin(h).is_some_and(|t| in_patch.contains(&(h / 3)) && in_patch.contains(&(t / 3)));
                if !inside || self.edges.contains(v, w) || source.contains(&w) {
                    continue;
                }
                let nd = d + (cut.positions[w as usize] - cut.positions[v as usize]).length();
                if nd < dist.get(&w).copied().unwrap_or(f64::INFINITY) {
                    dist.insert(w, nd);
                    parent.insert(w, v);
                    heap.push(E(nd, w));
                }
            }
        }
        let last = end?;
        let mut chain = vec![last];
        while let Some(&p) = parent.get(chain.last()?) {
            chain.push(p);
            if chain.len() > cut.vertex_count() {
                return None;
            }
        }
        chain.reverse();
        if chain.len() < 2 {
            return None;
        }
        // One field axis along the whole seam (no corners of its own), starting
        // orthogonal to the boundary it leaves, so its ends are corners of the
        // patches it makes however it runs.
        let start_vertex = chain[0];
        let leaving = faces
            .iter()
            .flat_map(|&f| 3 * f..3 * f + 3)
            .find(|&h| is_cut(h) && cut.tail(h) == start_vertex)
            .map(|h| self.edges.label(start_vertex, cut.head(h)).unwrap_or_else(|| self.region.label(h)));
        let first_dir = match leaving {
            Some(label) => g.normal(start_vertex).cross(label),
            None => cut.positions[chain[1] as usize] - cut.positions[start_vertex as usize],
        };
        let nodes = self.axis_nodes(&chain, first_dir);
        Some(TracePath { nodes, closed: false, cost: dist.get(&last).copied().unwrap_or(0.0) })
    }

    /// The full schedule for a part: rounds, sub-patch passes (failing, then
    /// non-disk, then non-disk relaxed), clean-up, recovery, and the final
    /// re-declaration of corners.
    pub fn run(&mut self, control: &Control) -> Result<(), Error> {
        self.run_rounds(false, false, control)?;
        self.sub_patch_passes(false, false, control)?;
        self.sub_patch_passes(true, false, control)?;
        self.sub_patch_passes(true, true, control)?;
        self.clean_up(control)?;
        self.recover(control)?;
        self.partition();
        if let Some(mut partition) = self.partition.take() {
            let ctx = self.region.ctx(self.settings);
            self.tally.redeclared += partition.redeclare_corners(&ctx);
            self.partition = Some(partition);
        }
        Ok(())
    }
}

/// Lays out a region: the usual schedule, then, if patches holding several
/// singularities still cover [`CROWDED_SHARE`] of it, once more from scratch
/// with crowding ranked ahead of side counts, keeping the second layout only
/// if it scores better with crowding first. A crowded patch gets no pattern
/// that places its singularities, so its quads flatten the shape there; a
/// missing or extra side, which the usual order ranks ahead, is mostly
/// temporary (later paths and the final corner re-declaration fix it). Applying crowding-first
/// everywhere costs quality on parts that were never crowded (the paths
/// taken early differ), so only crowded parts pay for the second layout.
pub fn lay_out<'s>(
    mesh: RegionMesh<'_>,
    settings: &'s Settings,
    control: &'s Control,
) -> Result<Layout<'s>, Error> {
    let mut layout = Layout::new(Region::new(mesh.clone(), settings, control)?, settings, control);
    layout.run(control)?;
    if layout.crowded_share() < CROWDED_SHARE {
        return Ok(layout);
    }
    let mut other = Layout::new(Region::new(mesh, settings, control)?, settings, control);
    other.crowd_first = true;
    other.run(control)?;
    if other.final_score() < layout.final_score() {
        other.tally.uncrowded = 1;
        other.tally.candidates += layout.tally.candidates;
        return Ok(other);
    }
    layout.tally.candidates += other.tally.candidates;
    Ok(layout)
}

/// Singularities beyond the first in a patch (patterns place at most one).
fn crowding(info: &PatchInfo) -> i64 {
    info.singularities.len().saturating_sub(1) as i64
}

/// A layout's score, compared lexicographically (less is better), after the
/// oracle's `BetterConfiguration`. Topology first: cut-open excess, glued
/// edges (which fall as a self-glued band is cut into disks), excess with
/// features glued back (which catches pinches). Then the paper's conditions:
/// non-convex boundary vertices, missing or extra sides, the most
/// singularities in one patch, singularities matching their patch (more is
/// better), and patches failing CC-ability. Counting vertices and sides
/// rather than patches (as the oracle does) makes splitting a 0-sided patch
/// into two 2-sided ones count as progress, and merging two back as a loss.
///
/// With `crowd_first`, total crowding (see [`crowding`]) takes the place of
/// the most singularities and moves ahead of the side counts: a path that
/// separates singularities but leaves a patch with too many sides is then
/// progress. Convexity stays ahead of it, since a reflex corner folds the
/// pattern however few singularities the patch holds.
fn layout_score<'a>(
    infos: impl Iterator<Item = &'a PatchInfo>,
    max_sides: usize,
    crowd_first: bool,
) -> [i64; 8] {
    let mut s = [0i64; 8];
    let (mut most, mut crowded) = (1i64, 0i64);
    let mut sides = 0i64;
    for i in infos {
        let n = i.sides.len();
        s[0] += i.excess.max(0);
        s[1] += i.glued as i64;
        s[2] += i.merged_excess.max(0);
        s[3] += i.not_convex.len() as i64;
        if i.kind != PatchKind::NonDisk {
            sides += 3usize.saturating_sub(n) as i64 + n.saturating_sub(max_sides) as i64;
        }
        most = most.max(i.singularities.len() as i64);
        crowded += crowding(i);
        s[6] -= i64::from(i.matches_singularity());
        s[7] += i64::from(!i.cc_able && (3..=6).contains(&n));
    }
    (s[4], s[5]) = if crowd_first { (crowded, sides) } else { (sides, most) };
    s
}

/// Vertices of `ring` at least `ring.len() / k` steps away from `v` along it.
fn far_side(ring: &[u32], v: u32, k: usize) -> Vec<u32> {
    let m = ring.len();
    let Some(i) = ring.iter().position(|&x| x == v) else { return Vec::new() };
    let reach = m.div_ceil(k.max(1));
    ring.iter()
        .enumerate()
        .filter(|&(j, _)| {
            let d = i.abs_diff(j);
            d.min(m - d) >= reach
        })
        .map(|(_, &x)| x)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trace::patches::SideInfo;

    /// A disk patch with `sides` one-edge sides, these singularities (+1
    /// quarter each) and these concave boundary vertices.
    fn patch(sides: usize, singularities: u32, concave: u32) -> PatchInfo {
        PatchInfo {
            faces: vec![0],
            loops: 1,
            loop_vertices: vec![(0..sides as u32).collect()],
            corners: (0..sides as u32).collect(),
            sides: (0..sides as u32).map(|v| SideInfo { vertices: vec![v, v + 1], length: 1.0 }).collect(),
            not_convex: (0..concave).map(|v| (100 + v, VertexClass::Concave)).collect(),
            singularities: (0..singularities).map(|v| (200 + v, 1)).collect(),
            kind: PatchKind::Ok,
            cc_margin: 0.0,
            cc_able: true,
            steps: Vec::new(),
            cut_disk: true,
            excess: 0,
            merged_excess: 0,
            glued: 0,
        }
    }

    fn compare(before: &[PatchInfo], after: &[PatchInfo], crowd_first: bool) -> std::cmp::Ordering {
        let a = layout_score(after.iter(), 6, crowd_first);
        a.cmp(&layout_score(before.iter(), 6, crowd_first))
    }

    #[test]
    fn crowding_counts_singularities_beyond_the_first() {
        let counts: Vec<i64> = (0..4).map(|k| crowding(&patch(4, k, 0))).collect();
        assert_eq!(counts, [0, 0, 1, 2]);
    }

    #[test]
    fn crowding_first_takes_a_split_that_leaves_too_many_sides() {
        // Three singularities separated, at the price of a 7-sided piece.
        let before = [patch(4, 3, 0)];
        let after = [patch(3, 1, 0), patch(7, 1, 0), patch(3, 1, 0)];
        assert!(compare(&before, &after, false).is_gt(), "the usual order refuses it");
        assert!(compare(&before, &after, true).is_lt(), "crowding first takes it");
    }

    #[test]
    fn convexity_still_ranks_ahead_of_crowding() {
        let before = [patch(4, 3, 0)];
        let after = [patch(3, 1, 1), patch(3, 1, 0), patch(3, 1, 0)];
        assert!(compare(&before, &after, true).is_gt());
    }

    #[test]
    fn crowding_first_still_ranks_side_counts_after_crowding() {
        // Equal crowding: the side counts decide, as in the usual order.
        let before = [patch(4, 2, 0), patch(4, 0, 0)];
        let after = [patch(4, 2, 0), patch(7, 0, 0)];
        assert!(compare(&before, &after, true).is_gt());
        assert!(compare(&before, &after, false).is_gt());
    }
}
