# 0013 — SIMD closest-point queries (fearless_simd)

**Status:** accepted 2026-10-23.

## Context

The question was where `fearless_simd` 1.1.0 helps. Instrumenting the stages showed one float-heavy hot spot that is ours: `TriangleBvh` closest-point queries.
- They take about 27% of the prep remesh's wall time: every split, collapse and smoothing step projects onto the input surface or a feature line, or measures a distance to it.
- The quad smoothing runs them too.
- The other float work is either already SIMD inside a library (the field's faer solves), dominated by graph walks (curvature sizing, tracing), or not ours (the microlp ILP).

## Decision

- **Four-wide BVH.** The binary median-split tree is still built, then flattened two levels at a time into nodes of four children, each with its child boxes stored lane by lane. A query measures all four boxes in one `f64x4` step.
- **SIMD leaves.** A leaf (at most 4 triangles) is one packet. `closest4` runs Ericson's closest-point routine on all four at once: every region's point is computed with the scalar routine's operations in the same order, and the first region that applies is selected. Each lane is therefore bit for bit the scalar result. Zero-area triangles are flagged and recomputed by the scalar routine.
- **Same visiting order as the binary tree.** Each flat node keeps the boxes of the two binary halves its children came from. Children are visited nearer half first, then the nearer child within each half, which is exactly the binary tree's depth-first order. Ties between equally near triangles (shared edges and vertices) therefore resolve the same way, and the pipeline output is byte-identical to before. Without this, about 0.2% of queries named a different, equally near triangle.
- **Runtime dispatch.** `Level::new()` picks the best level the CPU has (SSE2 through AVX-512 on x86, NEON on ARM). There is no FMA contraction and every lane computes the same IEEE operations, so the result is the same at every level; a test checks that SSE2 and the detected level match.
- **Unsafe.** Our code stays `forbid(unsafe_code)`. `fearless_simd` keeps its small amount of `unsafe` internal: the `target_feature` call inside `vectorize`, and the intrinsic wrappers. This crate was chosen, so it joins hashbrown as an accepted exception.
- **MSRV 1.89** (fearless_simd's). Raising it enabled clippy lints that the old 1.85 MSRV held back (`is_multiple_of`, `if let` chains, `as_chunks`). They are fixed throughout.

## Evidence

- **Microbenchmark** (52k–75k queries near the bunny, armadillo and rocker-arm surfaces): 1.26–1.35× faster per query, with identical results.
  - SIMD leaves alone gave 1.10–1.20×.
  - The four-wide nodes gave the rest.
- **Pipeline A/B** (interleaved, best of 3, 10 stress models plus a 1M-triangle sphere):
  - prep remesh 7.4% faster summed (30.55 → 28.30 s), faster on every model;
  - whole runs 5.2% faster (47.29 → 44.81 s);
  - output byte-identical on all 11 models.
- **Cost:** peak memory up 0–6% on most models and 20% on igea (166 → 200 MB), because four-wide nodes and padded packets are larger than the binary nodes.
- **Tests:**
  - `simd_lanes_match_the_scalar_routine_bit_for_bit` covers random, segment, collinear and point triangles.
  - `flat_search_matches_the_binary_tree_and_every_simd_level` runs a grid full of ties and fails if the visiting order is changed.
