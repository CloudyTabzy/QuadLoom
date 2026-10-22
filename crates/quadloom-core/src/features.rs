//! Stage 1: feature lines, the edges the quad layout must follow.
//!
//! Features come from six independently toggleable sources (QRemeshify's
//! defaults: all on, 35°). Auto-detected angle features are cleaned by pruning
//! short dangling spurs; anything the user marked or that comes from mesh
//! structure (seams, material/face-set/open boundaries) is never pruned.

use glam::DVec3;

use crate::repair::RepairedMesh;

/// Bit flags for where a feature edge came from.
pub mod source {
    pub const ANGLE: u8 = 1 << 0;
    pub const SHARP: u8 = 1 << 1;
    pub const SEAM: u8 = 1 << 2;
    pub const MATERIAL: u8 = 1 << 3;
    pub const FACE_SET: u8 = 1 << 4;
    pub const BOUNDARY: u8 = 1 << 5;
    /// In report order.
    pub const ALL: [u8; 6] = [ANGLE, SHARP, SEAM, MATERIAL, FACE_SET, BOUNDARY];
}

/// Feature hints from the host application, in *input* vertex/triangle ids.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FeatureInput {
    /// Edges the user marked sharp.
    pub sharp_edges: Vec<[u32; 2]>,
    /// UV seam edges.
    pub seam_edges: Vec<[u32; 2]>,
    /// Material index per input triangle.
    pub material: Option<Vec<i32>>,
    /// Face-set id per input triangle.
    pub face_set: Option<Vec<i32>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FeatureParams {
    /// Edges whose faces' normals differ by more than this become features.
    pub angle_deg: f64,
    pub use_angle: bool,
    pub use_marked_sharp: bool,
    pub use_seams: bool,
    pub use_material_boundaries: bool,
    pub use_face_set_boundaries: bool,
    pub use_open_boundaries: bool,
    /// Angle-only spurs up to this many edges (and under 5% of the bbox
    /// diagonal) that dangle from the feature network are removed. 0 disables.
    pub prune_steps: usize,
}

impl Default for FeatureParams {
    fn default() -> Self {
        Self {
            angle_deg: 35.0,
            use_angle: true,
            use_marked_sharp: true,
            use_seams: true,
            use_material_boundaries: true,
            use_face_set_boundaries: true,
            use_open_boundaries: true,
            prune_steps: 4,
        }
    }
}

/// One feature edge of the repaired mesh.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FeatureEdge {
    /// Sorted vertex pair.
    pub v: [u32; 2],
    /// [`source`] bit flags.
    pub sources: u8,
    /// Convex (ridge) rather than concave (valley). Boundaries count as convex.
    pub convex: bool,
}

/// Feature edges sorted by vertex pair.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FeatureSet {
    pub edges: Vec<FeatureEdge>,
}

impl FeatureSet {
    /// Vertices where feature lines end or meet (feature valence ≠ 2).
    pub fn corners(&self) -> Vec<u32> {
        let mut ends: Vec<u32> = self.edges.iter().flat_map(|e| e.v).collect();
        ends.sort_unstable();
        ends.chunk_by(|a, b| a == b).filter(|g| g.len() != 2).map(|g| g[0]).collect()
    }

    pub fn total_length(&self, positions: &[DVec3]) -> f64 {
        self.edges.iter().map(|e| (positions[e.v[0] as usize] - positions[e.v[1] as usize]).length()).sum()
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct FeatureReport {
    pub edges: usize,
    /// Edges carrying each source, in [`source::ALL`] order.
    pub by_source: [usize; 6],
    pub pruned_edges: usize,
    pub corners: usize,
    /// Total feature length over √area (QuadWild's "sharp factor" measure).
    pub length_ratio: f64,
}

/// Fraction of the bbox diagonal below which a dangling spur may be pruned.
const PRUNE_MAX_LENGTH_RATIO: f64 = 0.05;

pub fn detect(
    mesh: &RepairedMesh,
    input: &FeatureInput,
    params: &FeatureParams,
) -> (FeatureSet, FeatureReport) {
    let positions = &mesh.positions;
    let normals: Vec<DVec3> = mesh
        .triangles
        .iter()
        .map(|&[a, b, c]| {
            let (pa, pb, pc) = (positions[a as usize], positions[b as usize], positions[c as usize]);
            (pb - pa).cross(pc - pa).normalize_or_zero()
        })
        .collect();

    // (min, max, face, apex) for every face edge, grouped by undirected edge.
    let mut uses: Vec<(u32, u32, u32, u32)> = mesh
        .triangles
        .iter()
        .enumerate()
        .flat_map(|(f, t)| {
            (0..3).map(move |i| {
                let (a, b) = (t[i], t[(i + 1) % 3]);
                (a.min(b), a.max(b), f as u32, t[(i + 2) % 3])
            })
        })
        .collect();
    uses.sort_unstable();

    let group_of = |groups: &Option<Vec<i32>>| -> Option<Vec<i32>> {
        let groups = groups.as_ref()?;
        mesh.face_origin.iter().map(|&o| groups.get(o as usize).copied()).collect()
    };
    let material = group_of(&input.material);
    let face_set = group_of(&input.face_set);
    let cos_threshold = params.angle_deg.to_radians().cos();

    let region = Region::new(mesh, &normals, &uses);
    let mut edges = Vec::new();
    let mut on_boundary = vec![false; positions.len()];
    for group in uses.chunk_by(|x, y| (x.0, x.1) == (y.0, y.1)) {
        let v = [group[0].0, group[0].1];
        let (mut sources, mut convex) = (0u8, true);
        match group {
            [_] => {
                on_boundary[v[0] as usize] = true;
                on_boundary[v[1] as usize] = true;
                if params.use_open_boundaries {
                    sources |= source::BOUNDARY;
                }
            }
            [x, y] => {
                let (fx, fy) = (x.2 as usize, y.2 as usize);
                let raw_sharp = normals[fx].dot(normals[fy]) < cos_threshold;
                if params.use_angle && raw_sharp && region.salient(v, fx, fy, cos_threshold) {
                    sources |= source::ANGLE;
                    let apex_height = (positions[y.3 as usize] - positions[v[0] as usize]).dot(normals[fx]);
                    convex = apex_height <= 0.0;
                }
                let differs = |g: &Option<Vec<i32>>| g.as_ref().is_some_and(|g| g[fx] != g[fy]);
                if params.use_material_boundaries && differs(&material) {
                    sources |= source::MATERIAL;
                }
                if params.use_face_set_boundaries && differs(&face_set) {
                    sources |= source::FACE_SET;
                }
            }
            _ => {} // repaired meshes are manifold
        }
        edges.push(FeatureEdge { v, sources, convex });
    }

    let mut mark = |list: &[[u32; 2]], flag: u8| {
        for &[a, b] in list {
            let copies = |x: u32| mesh.vertex_copies.get(x as usize).map(Vec::as_slice).unwrap_or_default();
            for &ca in copies(a) {
                for &cb in copies(b) {
                    let key = [ca.min(cb), ca.max(cb)];
                    if let Ok(i) = edges.binary_search_by_key(&key, |e| e.v) {
                        edges[i].sources |= flag;
                    }
                }
            }
        }
    };
    if params.use_marked_sharp {
        mark(&input.sharp_edges, source::SHARP);
    }
    if params.use_seams {
        mark(&input.seam_edges, source::SEAM);
    }
    edges.retain(|e| e.sources != 0);

    let diagonal = {
        let (lo, hi) = positions
            .iter()
            .fold((DVec3::INFINITY, DVec3::NEG_INFINITY), |(lo, hi), &p| (lo.min(p), hi.max(p)));
        if positions.is_empty() { 0.0 } else { (hi - lo).length() }
    };
    let pruned_edges =
        prune(&mut edges, positions, &on_boundary, params.prune_steps, PRUNE_MAX_LENGTH_RATIO * diagonal);

    let features = FeatureSet { edges };
    let area: f64 = mesh
        .triangles
        .iter()
        .map(|&[a, b, c]| {
            let (pa, pb, pc) = (positions[a as usize], positions[b as usize], positions[c as usize]);
            0.5 * (pb - pa).cross(pc - pa).length()
        })
        .sum();
    let mut by_source = [0usize; 6];
    for e in &features.edges {
        for (count, flag) in by_source.iter_mut().zip(source::ALL) {
            *count += usize::from(e.sources & flag != 0);
        }
    }
    let report = FeatureReport {
        edges: features.edges.len(),
        by_source,
        pruned_edges,
        corners: features.corners().len(),
        length_ratio: if area > 0.0 { features.total_length(positions) / area.sqrt() } else { 0.0 },
    };
    (features, report)
}

/// Rings of faces around an edge that the salience test looks at.
const SALIENCE_RINGS: usize = 3;

/// The salience test for angle features. An edge that is sharp by its own two
/// faces is kept only if it is still sharp between the two *sides* of its
/// neighbourhood: the faces within [`SALIENCE_RINGS`] rings, split by the plane
/// that contains the edge and bisects its normals, each side averaged by area.
/// Along a real crease each side is the crease's own surface, so the angle
/// survives. Sculpting or scanning noise averages out over the neighbourhood.
struct Region<'a> {
    positions: &'a [DVec3],
    triangles: &'a [[u32; 3]],
    normals: &'a [DVec3],
    area: Vec<f64>,
    centroid: Vec<DVec3>,
    neighbours: Vec<Vec<u32>>,
}

impl<'a> Region<'a> {
    fn new(mesh: &'a RepairedMesh, normals: &'a [DVec3], uses: &[(u32, u32, u32, u32)]) -> Self {
        let (positions, triangles) = (&mesh.positions, &mesh.triangles);
        let corners = |t: &[u32; 3]| t.map(|v| positions[v as usize]);
        let area = triangles.iter().map(|t| {
            let [a, b, c] = corners(t);
            0.5 * (b - a).cross(c - a).length()
        });
        let centroid = triangles.iter().map(|t| corners(t).iter().sum::<DVec3>() / 3.0).collect();
        let mut neighbours: Vec<Vec<u32>> = vec![Vec::new(); triangles.len()];
        for group in uses.chunk_by(|x, y| (x.0, x.1) == (y.0, y.1)) {
            if let [x, y] = group {
                neighbours[x.2 as usize].push(y.2);
                neighbours[y.2 as usize].push(x.2);
            }
        }
        Self { positions, triangles, normals, area: area.collect(), centroid, neighbours }
    }

    /// Whether edge `v` (between faces `fx` and `fy`) is sharp at the scale of its neighbourhood.
    fn salient(&self, v: [u32; 2], fx: usize, fy: usize, cos_threshold: f64) -> bool {
        let (a, b) = (self.positions[v[0] as usize], self.positions[v[1] as usize]);
        let mid = 0.5 * (a + b);
        let along = (b - a).normalize_or_zero();
        let bisector = (self.normals[fx] + self.normals[fy]).normalize_or_zero();
        let across = along.cross(bisector);
        // Orient `across` toward fx's side.
        let across = if (self.centroid[fx] - mid).dot(across) >= 0.0 { across } else { -across };
        // Faces within a few rings of the edge, breadth first from both of its faces.
        let mut seen = vec![fx as u32, fy as u32];
        let mut frontier = seen.clone();
        for _ in 0..SALIENCE_RINGS {
            let mut next = Vec::new();
            for &f in &frontier {
                for &g in &self.neighbours[f as usize] {
                    if !seen.contains(&g) {
                        seen.push(g);
                        next.push(g);
                    }
                }
            }
            frontier = next;
        }
        let (mut side_x, mut side_y) = (DVec3::ZERO, DVec3::ZERO);
        for &f in &seen {
            let f = f as usize;
            let weighted = self.normals[f] * self.area[f];
            if f == fx || (f != fy && (self.centroid[f] - mid).dot(across) > 0.0) {
                side_x += weighted;
            } else {
                side_y += weighted;
            }
        }
        let (nx, ny) = (side_x.normalize_or_zero(), side_y.normalize_or_zero());
        // Degenerate neighbourhoods fall back to the edge's own faces.
        if nx == DVec3::ZERO || ny == DVec3::ZERO || self.triangles.is_empty() {
            return true;
        }
        nx.dot(ny) < cos_threshold
    }
}

/// Removes angle-only chains that dangle from the network: a chain starting at
/// a feature endpoint that isn't on an open boundary, running through
/// valence-2 vertices, at most `steps` edges and shorter than `max_length`.
/// Repeats until nothing changes (at most `steps` passes). Returns the count removed.
pub(crate) fn prune(
    edges: &mut Vec<FeatureEdge>,
    positions: &[DVec3],
    on_boundary: &[bool],
    steps: usize,
    max_length: f64,
) -> usize {
    if steps == 0 {
        return 0;
    }
    let mut incident: Vec<Vec<usize>> = vec![Vec::new(); positions.len()];
    for (i, e) in edges.iter().enumerate() {
        incident[e.v[0] as usize].push(i);
        incident[e.v[1] as usize].push(i);
    }
    let mut removed = vec![false; edges.len()];
    let live = |v: usize, removed: &[bool]| incident[v].iter().filter(|&&i| !removed[i]).count();

    for _ in 0..steps {
        let mut changed = false;
        for start in 0..positions.len() {
            if on_boundary.get(start).copied().unwrap_or(false) || live(start, &removed) != 1 {
                continue;
            }
            let (mut chain, mut length, mut at, mut came) = (Vec::new(), 0.0, start, usize::MAX);
            let prunable = loop {
                let Some(&e) = incident[at].iter().find(|&&i| !removed[i] && i != came) else { break true };
                if edges[e].sources != source::ANGLE || chain.len() == steps {
                    break false;
                }
                let next = if edges[e].v[0] as usize == at { edges[e].v[1] } else { edges[e].v[0] } as usize;
                length += (positions[next] - positions[at]).length();
                chain.push(e);
                (at, came) = (next, e);
                if live(at, &removed) != 2 || on_boundary.get(at).copied().unwrap_or(false) {
                    // Reached a junction, another loose end, or the boundary: complete.
                    break true;
                }
            };
            if prunable && length < max_length && !chain.is_empty() {
                for e in chain {
                    removed[e] = true;
                }
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let count = removed.iter().filter(|&&r| r).count();
    let mut keep = removed.iter().map(|r| !r);
    edges.retain(|_| keep.next().unwrap_or(true));
    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::Control;
    use crate::mesh::InputMesh;
    use crate::repair::{RepairParams, repair};

    const CUBE_P: [f32; 24] =
        [0., 0., 0., 1., 0., 0., 1., 1., 0., 0., 1., 0., 0., 0., 1., 1., 0., 1., 1., 1., 1., 0., 1., 1.];
    const CUBE_T: [u32; 36] = [
        0, 3, 2, 0, 2, 1, 4, 5, 6, 4, 6, 7, 0, 1, 5, 0, 5, 4, 3, 7, 6, 3, 6, 2, 0, 4, 7, 0, 7, 3, 1, 2, 6, 1,
        6, 5,
    ];

    fn repaired(p: &[f32], t: &[u32]) -> RepairedMesh {
        let params = RepairParams { min_component_faces: 1, ..RepairParams::default() };
        repair(&InputMesh::from_buffers(p, t).unwrap(), &params, &Control::default()).unwrap().0
    }

    fn only(source: u8) -> FeatureParams {
        FeatureParams {
            use_angle: source == source::ANGLE,
            use_marked_sharp: source == source::SHARP,
            use_seams: source == source::SEAM,
            use_material_boundaries: source == source::MATERIAL,
            use_face_set_boundaries: source == source::FACE_SET,
            use_open_boundaries: source == source::BOUNDARY,
            ..FeatureParams::default()
        }
    }

    /// A 2×1 strip of quads (6 vertices, 4 triangles) folded 90° along x = 1.
    fn folded_strip(angle_deg: f64) -> RepairedMesh {
        let (c, s) = (angle_deg.to_radians().cos(), angle_deg.to_radians().sin());
        let p: Vec<f32> = [0., 0., 0., 1., 0., 0., 1. + c, 0., s, 0., 1., 0., 1., 1., 0., 1. + c, 1., s]
            .into_iter()
            .map(|x: f64| x as f32)
            .collect();
        repaired(&p, &[0, 1, 4, 0, 4, 3, 1, 2, 5, 1, 5, 4])
    }

    #[test]
    fn cube_has_12_convex_feature_edges_and_8_corners() {
        let mesh = repaired(&CUBE_P, &CUBE_T);
        let (features, report) = detect(&mesh, &FeatureInput::default(), &only(source::ANGLE));
        assert_eq!(features.edges.len(), 12);
        assert!(features.edges.iter().all(|e| e.convex && e.sources == source::ANGLE));
        assert_eq!(features.corners().len(), 8);
        assert_eq!(report.corners, 8);
        assert_eq!(report.by_source[0], 12);
    }

    /// Edges sharper than `deg` by their two faces' raw normals.
    fn raw_sharp_edges(p: &[DVec3], t: &[[u32; 3]], deg: f64) -> usize {
        let normal = |f: &[u32; 3]| {
            (p[f[1] as usize] - p[f[0] as usize]).cross(p[f[2] as usize] - p[f[0] as usize]).normalize()
        };
        let mut faces: std::collections::HashMap<(u32, u32), Vec<usize>> = std::collections::HashMap::new();
        for (i, f) in t.iter().enumerate() {
            for k in 0..3 {
                let (a, b) = (f[k], f[(k + 1) % 3]);
                faces.entry((a.min(b), a.max(b))).or_default().push(i);
            }
        }
        faces
            .values()
            .filter(|fs| fs.len() == 2 && normal(&t[fs[0]]).dot(normal(&t[fs[1]])) < deg.to_radians().cos())
            .count()
    }

    fn flat(p: &[DVec3], t: &[[u32; 3]]) -> (Vec<f32>, Vec<u32>) {
        (
            p.iter().flat_map(|v| [v.x as f32, v.y as f32, v.z as f32]).collect(),
            t.iter().flatten().copied().collect(),
        )
    }

    #[test]
    fn sculpting_noise_is_not_a_feature() {
        // A lumpy sphere: radial noise makes scattered edges sharper than 35°.
        let (mut p, t) = crate::testmesh::icosphere(3);
        let mut seed = 7u64;
        for v in &mut p {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let r = (seed >> 11) as f64 / (1u64 << 53) as f64;
            *v *= 1.0 + 0.12 * (r - 0.5);
        }
        let raw = raw_sharp_edges(&p, &t, 35.0);
        assert!(raw > 20, "the test needs noisy edges ({raw})");
        let (fp, ft) = flat(&p, &t);
        let (features, _) = detect(&repaired(&fp, &ft), &FeatureInput::default(), &only(source::ANGLE));
        assert!(features.edges.len() * 10 < raw, "{} of {raw} noisy edges kept", features.edges.len());
    }

    #[test]
    fn real_creases_survive_the_salience_test() {
        // A subdivided cube: every one of its 12 edges, 6 segments each, stays.
        let (p, t, expected) = crate::testmesh::cube(6);
        let (fp, ft) = flat(&p, &t);
        let (features, _) = detect(&repaired(&fp, &ft), &FeatureInput::default(), &only(source::ANGLE));
        assert_eq!(features.edges.len(), expected.len());
        assert_eq!(features.corners().len(), 8);
    }

    #[test]
    fn angle_threshold_respected() {
        // Folding by 40° deviates the normals by 40°: above 35, below 45.
        let mesh = folded_strip(40.0);
        let at35 = FeatureParams { angle_deg: 35.0, ..only(source::ANGLE) };
        let at45 = FeatureParams { angle_deg: 45.0, ..only(source::ANGLE) };
        assert_eq!(detect(&mesh, &FeatureInput::default(), &at35).0.edges.len(), 1);
        assert_eq!(detect(&mesh, &FeatureInput::default(), &at45).0.edges.len(), 0);
    }

    #[test]
    fn concave_edge_marked_concave() {
        let up = detect(&folded_strip(60.0), &FeatureInput::default(), &only(source::ANGLE)).0;
        let down = detect(&folded_strip(-60.0), &FeatureInput::default(), &only(source::ANGLE)).0;
        assert_eq!(up.edges.len(), 1);
        assert_eq!(down.edges.len(), 1);
        assert_ne!(up.edges[0].convex, down.edges[0].convex);
    }

    #[test]
    fn open_boundary_source() {
        let (features, _) = detect(&folded_strip(0.0), &FeatureInput::default(), &only(source::BOUNDARY));
        assert_eq!(features.edges.len(), 6);
        assert!(features.edges.iter().all(|e| e.sources == source::BOUNDARY));
    }

    #[test]
    fn marked_sharp_and_seam_sources() {
        let mesh = folded_strip(0.0);
        let input =
            FeatureInput { sharp_edges: vec![[1, 4]], seam_edges: vec![[0, 4]], ..FeatureInput::default() };
        let sharp = detect(&mesh, &input, &only(source::SHARP)).0;
        assert_eq!(sharp.edges.iter().map(|e| e.v).collect::<Vec<_>>(), vec![[1, 4]]);
        let seam = detect(&mesh, &input, &only(source::SEAM)).0;
        assert_eq!(seam.edges.iter().map(|e| e.v).collect::<Vec<_>>(), vec![[0, 4]]);
    }

    #[test]
    fn user_edges_that_are_not_mesh_edges_are_ignored() {
        let input = FeatureInput { sharp_edges: vec![[0, 5], [2, 2], [0, 99]], ..FeatureInput::default() };
        assert!(detect(&folded_strip(0.0), &input, &only(source::SHARP)).0.edges.is_empty());
    }

    #[test]
    fn material_and_face_set_boundaries() {
        let mesh = folded_strip(0.0);
        let groups = Some(vec![0, 0, 1, 1]);
        let material = FeatureInput { material: groups.clone(), ..FeatureInput::default() };
        let face_set = FeatureInput { face_set: groups, ..FeatureInput::default() };
        assert_eq!(detect(&mesh, &material, &only(source::MATERIAL)).0.edges.len(), 1);
        assert_eq!(detect(&mesh, &face_set, &only(source::FACE_SET)).0.edges.len(), 1);
        assert!(detect(&mesh, &material, &only(source::FACE_SET)).0.edges.is_empty());
    }

    #[test]
    fn group_arrays_of_wrong_length_are_ignored_not_fatal() {
        let input = FeatureInput { material: Some(vec![0, 1]), ..FeatureInput::default() };
        assert!(detect(&folded_strip(0.0), &input, &only(source::MATERIAL)).0.edges.is_empty());
    }

    #[test]
    fn user_edges_remapped_through_weld() {
        // Each triangle of a square has its own copy of the shared diagonal.
        let p = [0., 0., 0., 1., 0., 0., 1., 1., 0., 0., 0., 0., 1., 1., 0., 0., 1., 0.];
        let mesh = repaired(&p, &[0, 1, 2, 3, 4, 5]);
        // Mark the diagonal using the *second* triangle's (welded-away) copies.
        let input = FeatureInput { sharp_edges: vec![[3, 4]], ..FeatureInput::default() };
        let features = detect(&mesh, &input, &only(source::SHARP)).0;
        assert_eq!(features.edges.len(), 1);
    }

    #[test]
    fn short_dangling_spur_pruned_but_user_marks_kept() {
        // A long chain 0..6 between two boundary vertices, an angle-only spur
        // 3-7, a user-marked spur 3-8-9, and an isolated angle-only segment 10-11.
        let positions: Vec<DVec3> = (0..12).map(|i| DVec3::new(f64::from(i), 0.0, 0.0)).collect();
        let edge =
            |a: u32, b: u32, sources: u8| FeatureEdge { v: [a.min(b), a.max(b)], sources, convex: true };
        let mut edges: Vec<FeatureEdge> = (0..6).map(|i| edge(i, i + 1, source::ANGLE)).collect();
        edges.push(edge(3, 7, source::ANGLE));
        edges.push(edge(3, 8, source::ANGLE | source::SHARP));
        edges.push(edge(8, 9, source::SHARP));
        edges.push(edge(10, 11, source::ANGLE));
        edges.sort_by_key(|e| e.v);
        let mut on_boundary = vec![false; 12];
        on_boundary[0] = true;
        on_boundary[6] = true;
        let removed = prune(&mut edges, &positions, &on_boundary, 4, 100.0);
        assert_eq!(removed, 2);
        let kept: Vec<[u32; 2]> = edges.iter().map(|e| e.v).collect();
        assert!(!kept.contains(&[3, 7]) && !kept.contains(&[10, 11]));
        assert!(kept.contains(&[3, 8]) && kept.contains(&[8, 9]));
        assert_eq!(kept.len(), 8);
    }

    #[test]
    fn long_spurs_are_kept() {
        let positions: Vec<DVec3> = (0..8).map(|i| DVec3::new(f64::from(i), 0.0, 0.0)).collect();
        let mut edges: Vec<FeatureEdge> =
            (0..7).map(|i| FeatureEdge { v: [i, i + 1], sources: source::ANGLE, convex: true }).collect();
        // 7 edges, both ends dangling, but longer than `steps` → kept.
        assert_eq!(prune(&mut edges, &positions, &[false; 8], 4, 100.0), 0);
        // Short in edge count but longer than `max_length` → kept too.
        let mut short: Vec<FeatureEdge> = edges[..2].to_vec();
        assert_eq!(prune(&mut short, &positions, &[false; 8], 4, 1.5), 0);
    }

    #[test]
    fn split_vertex_keeps_feature_on_all_copies() {
        // Bow-tie: vertex 0 is split into one copy per fan; marks on edges in
        // both fans must survive.
        let p = [0., 0., 0., 1., 0., 0., 1., 1., 0., -1., 0., 0., -1., -1., 0.];
        let mesh = repaired(&p, &[0, 1, 2, 0, 3, 4]);
        assert_eq!(mesh.vertex_copies[0].len(), 2);
        let input = FeatureInput { sharp_edges: vec![[0, 1], [0, 3]], ..FeatureInput::default() };
        assert_eq!(detect(&mesh, &input, &only(source::SHARP)).0.edges.len(), 2);
    }

    #[test]
    fn closed_feature_loop_is_never_pruned() {
        let mesh = repaired(&CUBE_P, &CUBE_T);
        let (features, report) = detect(
            &mesh,
            &FeatureInput::default(),
            &FeatureParams { prune_steps: 50, ..only(source::ANGLE) },
        );
        assert_eq!(features.edges.len(), 12);
        assert_eq!(report.pruned_edges, 0);
    }
}
