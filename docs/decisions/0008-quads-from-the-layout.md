# 0008 — Quads from the layout: segments, integer quantization, closed-form patterns

**Status:** accepted 2026-10-23. This is the first all-quad output, and it covers what the roadmap planned as M6 (quantization) and M7 (patterns, map and stitch). It's built directly, without subagents, because the owner wanted quads as soon as possible.

## Decision

`crates/quadloom-core/src/quad/` turns the patch layout into a quad mesh. It's on by default.

1. **Segments.** Patch sides are cut at every layout corner, T-junctions included. Each segment is shared by at most two patches and gets one integer count.
2. **Quantization**, in steps; each candidate is kept only if it lowers the combined cost (deviation plus weighted pattern cost):
   1. A least-squares fit with 4-sided balance as a penalty.
   2. Rounding, then integer moves on single segments and on whole chords (strips through 4-sided patches).
   3. Parity fixed along chords, a T-join on the chord graph, with a plain path walk as the last resort.
   4. The whole integer program (`ilp.rs`, `microlp`), for up to 2,500 free segments.
   5. Parallel region repair: disjoint 1-rings around problem patches, each solved exactly.

   The program:
   - integer counts, with the quadratic deviation exact at integers via chords;
   - 4-sided balance as a soft term;
   - for 3-, 5- and 6-sided patches, Tarini's closed form taken exactly: an integer split variable per step-2 cycle, which also makes the total even;
   - seeded with the heuristic counts, deterministic node limits.
3. **Patterns** (`pattern.rs`), best first:
   - a regular grid for balanced quads;
   - Tarini's one-singularity closed form for 3, 5 or 6 sides;
   - a chord split into two such pieces;
   - a single quad for 4 boundary edges, else a fan (always valid, last resort).

   Pattern points carry identities, so sub-patterns share their edges exactly.
4. **Embedding** (`embed.rs`). A Tutte map, with clamped positive cotangent weights, of each patch's triangles onto its polygon. Border samples sit at shared arc-length positions, so neighbours stitch exactly. Patches that can't be mapped keep their triangles, and the segments around them are sampled at their mesh vertices, so the mesh stays watertight.
5. **Smoothing** (`smooth.rs`). 8 Laplacian rounds, each projected back onto the triangles of the vertex's own patches. Feature and border vertices stay fixed.

## Why not BiMDF first (as the design said)

The design called for a port of BiMDF: a bidirected min-cost flow plus matching, with no integer solver. Rounding plus local moves left far too many patches infeasible: 111 fallback patterns on the bunny. A small integer program over the closed-form conditions fixed that outright: 0 fallbacks on the bunny, under a second for the solve. `microlp` is pure Rust and Apache-2.0, deterministic under node limits, and can only improve on its seed.

BiMDF stays the plan for very large layouts, where the global program stalls. Armadillo (1,073 patches) still has 240 fan fallbacks out of 1,073 patches.

## Evidence

Five oracle `_rem` inputs through `quadloom run`, release build, with defaults (quad size 0.75× the prep edge, smoothing 8). SJ is the scaled Jacobian.

| Model | Quads (oracle) | Time | Fans | p5 SJ | Median SJ | Watertight |
|---|---|---|---|---|---|---|
| fandisk | 6,381 (4,542) | 1.1 s | 0 | 0.86 | 0.99 | yes |
| stanford-bunny | 11,610 (12,805) | 5.3 s | 0 | 0.64 | 0.97 | yes (open borders kept) |
| rocker-arm | 9,002 (10,813) | 4.6 s | 0 | 0.58 | 0.98 | yes |
| CesiumMilkTruck | 9,759 (12,078) | 1.8 s | 0 | 0.15 | 0.96 | yes (open borders kept) |
| armadillo | 10,413 (23,399) | 7.3 s | 240 | 0.22 | 0.92 | 4 patches kept as triangles |

Every output is manifold (no edge in more than two faces).

## Update 2026-10-23 (0.1.2)

- **Smoothing.** 20 under-relaxed rounds (step 0.6). Each blends the neighbour average with each incident quad's parallelogram corner (weight 0.3). Full steps let a checkerboard wobble survive, and the parallelogram target alone amplifies it about 3×.
- **Parity absorbed by kept triangles.** Segments along a patch kept as triangles have frozen counts, which could leave a region with odd parity and an unfillable patch, and a crack (rocker-arm had 26 border edges). Such a segment can now gain one extra sample, the midpoint of its longest edge, and the kept triangle on that edge becomes a polygon through it. Output is watertight wherever the input is.
- **Results (fandisk / bunny / rocker-arm / MilkTruck / armadillo), p5 SJ:**
  - before: 0.86 / 0.64 / 0.58 / 0.15 / 0.22
  - now: 0.85 / 0.63 / 0.77 / 0.13 / 0.37
  - fandisk's worst quad: −0.62 before, +0.12 now
  - rocker-arm's fallback patterns: 29 before, 1 now (with the salience test in features)

## Update 2026-10-23 (0.1.3): adaptive, all-quad, hardened

- **Adaptive quad size.** Each segment's target counts edges against the *local* quad size: the global size × (prep edge length around the face / global prep length)^adaptivity. The panel's Adaptivity slider defaults to 0.5. The prep remesh is curvature-adaptive, so quads concentrate on detail. Test: `adaptivity_spends_quads_on_detail`.
- **Always all-quad.** A patch no pattern fills (not a disk, unmappable) splits each triangle into 3 quads through its edge midpoints and centroid. The segments around it are sampled at mesh vertices and edge midpoints, so their counts are even: the parity trick of 0.1.2 is gone.
- **Never fails a run.** A non-cancellation error in the quad stages makes the pipeline return the layout, with `quad_error` explaining why ("quads failed (...); showing the patch layout" in the panel). A final guard (`check_output`) refuses non-finite, malformed or non-manifold quad meshes.
- **Untangling.** After smoothing, quads with a reflex or flipped corner against the surface get full Laplacian rounds on their free vertices, projected (up to 30 rounds). Optimising each vertex for its worst corner was tried and rejected: it disagreed with the measured quality.
- **Patterns.** The chord split tries every chord length and judges proportions in count space: a 4-sided patch is laid out as a rectangle of its mean side counts, so long thin patches split correctly. For example, (3,47,1,47) becomes a 1×47 grid plus the closed-form triangle (47,2,47).
- **Quantization.** The global program has a 2,000-node budget (was 400). Armadillo's fallback patterns drop from 78 to 10, for about 3 s.
- **Stress harness:** raw input to quads through the full pipeline. 10 detailed models:
  - every one 100% quads, watertight where the input is, no non-manifold edges;
  - 1–10 s each (igea, 269k triangles in, takes 6.5 s);
  - armadillo: fallbacks 86 → 10, p5 SJ 0.22 → 0.56, inverted 206 → 81.

## Open items

- Large layouts: BiMDF, or larger region repair, to remove armadillo's fans.
- A few inverted quads remain (minimum SJ near −1) on organic models. Untangling during smoothing is the next step.
- Feature edges of the quad mesh aren't marked Sharp yet.
- With Quad Shape on, the frame's per-face scales set the side targets (anisotropic lengths). Its preview attributes aren't carried onto the quads yet.
