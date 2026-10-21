# 0004 — Prep remesh: error-bounded, feature-aware isotropic remeshing

**Date:** 2026-10-23
**Status:** Accepted

The prep remesh turns the repaired input into the clean triangle mesh that the cross field and tracing (M4+) work on. The oracle uses VCG's `IsotropicRemeshing` (`AutoRemesher::RemeshAdapt`). QuadLoom implements its own Botsch–Kobbelt loop of split, collapse, flip and tangential smoothing with projection, in `crates/quadloom-core/src/remesh/` over `edit::EditMesh`. This record lists where it deliberately differs from the oracle and why.

## Decisions

1. **The deviation bound drives everything.**
   - The bound is the oracle's: 1/2500 of the bounding-box diagonal (`max_surface_distance_rel`, the add-on's *Max Deviation*).
   - Every coarsening operation must respect it:
     - **Collapses** keep both endpoints' old positions within the bound of the new faces. They also keep each new face's centroid within the bound of the input, which VCG checks too.
     - **Flips** may not move the edge further from the input than the bound, or than the edge they replace. That relative clause is QuadLoom's own: an absolute flip bound, as in VCG, stopped coarse curved inputs from ever becoming isotropic.
   - Vertices themselves are always projected onto the input; split points are projected too.
2. **Smoothing stays on the input's folds** (`SMOOTH_DEVIATION`).
   - A tangential move is applied only if projecting it back onto the input moves it by at most a quarter of the bound.
   - On a faceted, low-poly input this keeps vertices on facet corners, so faces follow the facets. The oracle's VCG Laplacian is gated the same way, which is why it keeps about 69% of a coarse input's vertices in place.
   - This was the largest single gain: the 10-case median Hausdorff ratio against the oracle went from 1.74 to 1.07.
3. **Adaptive sizing from the error bound** (Dunyach et al., *Adaptive Remeshing for Real-Time Mesh Deformation*, EG 2013).
   - h = √(6·ε·r − 3·ε²), with ε the deviation bound and r the **maximum** curvature radius, clamped to `[min_mult, max_mult] × target`.
   - The oracle instead adapts by vertex "radii" quality over 0.3–3× and relies on its distance checks.
4. **Curvature from a jet fit at target scale.**
   - At each free vertex, the samples within half a target length are fitted with h = a·x² + b·xy + c·y² + d. That is a degree-2 osculating jet (Cazals & Pouget, SGP 2003) plus a constant.
   - The flood never crosses a crease and takes at most 64 samples.
   - Principal curvatures are the eigenvalues of [[2a, b], [b, 2c]].
   - Zero-mean tessellation noise lands in the constant, so a finely corrugated plane reads as flat.
   - Coordinates are stretched to chord length, which makes the fit exact on spheres at any ball size.
   - Crease vertices take their free neighbours' size, capped by their feature line's own Menger curvature.
5. **`max_mult` defaults to 1** (refine only).
   - On 20 cases: 1× gives 1.18× the oracle's triangles at 0.75× its Hausdorff distance and 0.82× its mean distance. 2× gives 0.87× the triangles at 0.78× / 1.02×. 3× gives Hausdorff 0.83×.
   - Coarser flats cost fidelity on CAD parts (fandisk) and triangle quality.
6. **Soft features.**
   - Angle-only feature edges are re-judged after each of the first 3 iterations at target scale and released where the surface is smooth, like the oracle's `UpdateCoherentSharp`.
   - Each side is flooded from its own face without crossing the edge's own feature line. Faces are weighted by area × exp(−(3d/radius)²).
   - The earlier plane-through-the-edge split mixed the sides of a curving ridge. On `khronos/Duck`'s beak, a real 110° ridge measured 28° and was released.
7. **Corners at target scale with non-maximum suppression.** A feature vertex is a corner only if its line turns by more than 35° between points about target/2 away, and that turn is the largest along the line within that reach. Zigzag tessellation then doesn't pin feature lines.
8. **Guards QuadLoom adds.**
   - **Relative quality guard:** never leave a face worse than min(0.1, the worst face it replaces), with 0.1% rounding slack. That lets all-sliver inputs clean up.
   - **Per-face reference normals:** no face may turn away from the input orientation it descends from. This stops folds that build up over many small sliver operations.
   - **Face ceiling.**
   - **Sweeps look edges up by their vertices:** a stale half-edge id once left a 2.15-long edge unsplit through all 15 iterations.

## Evidence

Benchmark run `20261023-m3b5` against the oracle baseline `20261021-oracle-baseline`. The full table is in `benchmark/reports/20261023-m3b5/remesh_vs_oracle.md`.

- **Robustness:** QuadLoom completes 70/70; QRemeshify completes 60/70.
- **Medians over the 60 cases both completed:**

| Median | QuadLoom | QRemeshify |
|---|---|---|
| Triangles | 19,859 | 14,163 |
| Min angle, 5th percentile | 35.5° | 21.1° |
| Mean radius ratio | 0.941 | 0.855 |
| Mean distance (% of diagonal) | 0.0071 | 0.0100 |
| Hausdorff (% of diagonal) | 0.099 | 0.142 |

- **Per case:** Hausdorff is at or below the oracle's on 52/60 cases, and mean distance on 50/60. The median per-case ratios are 0.70 (Hausdorff), 0.77 (mean) and 1.27 (triangles).
- **Time:** median tool time is 2.1 s; the oracle's is 13 s.
- **Remaining worst ratio:** `common3d/suzanne`, Hausdorff 1.48% vs 0.95%. It's an open-surface model; the oracle is also far off on it.
- **How we got here:** before this work (run `20261022-m3b4`, uniform-ish sizing without deviation-gated smoothing), the medians were Hausdorff 0.238% and mean 0.0144%.

## Consequences

The prep mesh follows the input at least as closely as QRemeshify's on most of the benchmark, with clearly better triangles. It is somewhat denser by default. Users who want a lighter prep mesh can raise *Coarsest* or *Max Deviation*.

Open items:
- `EditMesh` is memory-bound on its directed-edge hash map; the 1M-triangle check takes about 22 s (budget 40 s).
- `crane/oloid`, whose faces are all needles, still gives low-quality triangles.
