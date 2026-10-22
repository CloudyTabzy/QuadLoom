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

## Open items

- Large layouts: BiMDF, or larger region repair, to remove armadillo's fans.
- A few inverted quads remain (minimum SJ near −1) on organic models. Untangling during smoothing is the next step.
- Feature edges of the quad mesh aren't marked Sharp yet.
- With Quad Shape on, the frame's per-face scales set the side targets (anisotropic lengths). Its preview attributes aren't carried onto the quads yet.
