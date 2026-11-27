# 0023 — Quad geometry: placement, relaxation, boundary cuts, bridged patterns

**Status:** implemented, 2026-11-26. It is version **0.4.1** (2026-11-27). 0.4.1 was not released on its own; it is first published in 0.4.6.

The first benchmark of finished quads against QRemeshify 1.5.0 (`benchmark/reports/20261126-q040-default/`) showed the layouts on par and the quads behind: the better mean scaled Jacobian on 21 of 60 models, the better Hausdorff distance on 17. The goal was a quad stage that matches or passes the oracle. This records what was wrong, what replaced it, and what is still open.

## How it was measured

`benchmark/quad_eval.py` (a development tool, not in the public repository) runs `quadloom run` and `quadloom metrics` on the benchmark's saved inputs without Blender (about ten minutes for all 70) and compares with the oracle's recorded numbers and with an earlier build (`--baseline`). `quadloom run --quad <json>` passes quad settings from the command line. Every change below was kept only if this A/B improved. The add-on benchmark stays the release gate.

## What was wrong and what replaced it

1. **Pattern points placed by formula folded.** The closed forms place a pattern's inner points in its polygon. With lopsided counts, (20, 20, 2) say, quads came out concave before any geometry was involved. `Pattern::place` now solves for every inner point as the mean of its neighbours with the boundary fixed (one sparse Cholesky solve per pattern). On a convex polygon that cannot fold. Grids keep their exact positions.

2. **The smoother wore the shape down.** It moved each vertex to its neighbours' mean and projected back, held every feature vertex still, and so flattened ears, fingers and tips (cheburashka's ears: 5.7% of the diagonal) while leaving feature lines jagged. `quad/smooth.rs` is rewritten:
   - a vertex moves toward the corners of the best-fitting squares of its quads (shape matching), only along the surface (the move loses its part along the quad mesh's own normal);
   - feature vertices slide along their feature line (`quad/curves.rs`: chains between corners and junctions; a `Hold` per vertex is `Free`, `Fixed` or `Curve`);
   - a cover term pulls the quad mesh back over input vertices it has drifted off, which is what keeps thin parts;
   - a repair pass moves the vertices of pinched or folded quads to whichever of a few candidate places gives the best worst corner.

   The oracle's `MultiCostraintSmooth` was read for what it constrains. The method and code are our own.

3. **Sides were not cut where the layout branches.** After corners are re-declared, a junction of boundary lines can be no patch's corner. Segments ran through it, two patches disagreed about where the side's points lie, and the stitch tore. `segments::cuts` now cuts at every patch corner and every vertex where the boundary lines branch.

4. **Twin segments.** Two segments joining the same two vertices both got one quad, which is the same edge twice: an edge in four faces, and the whole quad stage failed. All but the shortest of such a group now take at least two (`Segments::least`, `quantize::floor_of`).

5. **A clash cost the whole mesh.** A tube two quads round still makes doubled edges. `retrying_clashes` now keeps just the clashing patches as triangles and builds again (up to four rounds) instead of failing the run. Two of 70 benchmark models had no quads at all; all 70 have them now.

6. **Fans on long boundaries.** A boundary no grid, closed form or single chord fills fell back to a fan: one centre joined to every second boundary point. On a strip of (1, 147, 1, 145) that is one point with 147 quads round it. New pattern **Bridge** (`PatternKind::Bridge`): the boundary read as four sides and filled with a grid whose longer sides are reached through rows of widening cells (one edge below, three above). The four sides need not start at the patch's corners: one may move an edge along to mend the parity, a triangle is given a fourth, a pentagon or hexagon is read with a corner as a point of a side. Fans remain for boundaries of ten edges or fewer (`FAN_EDGES`), where they are fair. The run report gains `bridge_patches`; the add-on reads it with `.get`, so `API_VERSION` is unchanged.

7. **A crease that stops an edge short of a path.** fandisk's fading crease ends one edge inside a patch, with the patch's own boundary straight ahead of it. The recovery seam only accepted an end on the far half of the boundary, found none, and the strip stayed triangles (287 of them, 486 irregular vertices). The seam's first step now is the whole seam when it lands on the patch's boundary, wherever it lands (`select::concave_seam`). The acceptance test (strictly better) still decides.

Also: `PatchMap::new` refuses a map whose pins are not the rim of its faces, rather than returning a folded one.

## Results

`quad_eval.py` on all 70 inputs, against the oracle on the 60 it finished. "Before" is released 0.4.0 measured the same way.

| | Before | Now | Oracle |
|---|---|---|---|
| Models with quads | 68 of 70 | 70 of 70 | 60 of 70 |
| Mean scaled Jacobian, median | 0.911 | 0.942 | 0.922 |
| — better than the oracle on | 21 of 60 | 47 of 60 | |
| 5th percentile, median | 0.569 | 0.757 | 0.650 |
| — better than the oracle on | 21 of 60 | 47 of 60 | |
| Irregular vertices, median | — | 120 | 248 |
| Hausdorff distance, median | 0.802% | 0.332% | 0.491% |
| — better than the oracle on | 15 of 60 | 46 of 60 | |
| Mean distance better on | — | 57 of 60 | |
| Further than 1% from the input | 28 of 68 | 3 of 70 | |
| With an inverted quad | 64 of 68 | 46 of 70 | 52 of 60 |

Single models: fandisk 486 → 34 irregular vertices (the oracle's count), 5th percentile 0.81 → 0.93; `thingi10k/756803` 11.9% → 0.89% from the input; SpecularSilkPouf 8.4% → 0.55%; cheburashka 5.7% → about 0.3%; `thingi10k/49382` mean 0.845 → 0.992 (the oracle's).

These numbers come from the CLI, which passes no marked edges, seams or materials. Game assets differ from their add-on runs.

## Still open

- **Tiny patches.** 6,157 fans remain over the 70 models, nearly all on boundaries of ten edges or fewer, on models where features or noise are denser than the quads (756803, ChronographWatch, AntiqueCamera, Corset). Each segment takes at least one quad, so quads there are far under the target size. This is where the oracle still wins (Corset 0.885 against 0.938, at 2.6 times fewer quads than it uses). The cause is in sizing and layout, not the patterns: slivers between nearly parallel lines should be merged, or the quads sized to the features.
- **131 patches kept as triangles** (40 on 756803). `thingi10k/99836` keeps one and loses to the oracle for it.
- **Thin features at default size**: `thingi10k/75117`'s one-unit step is chamfered (1.19% against 0.14%), the pig's tail (0.92% against 0.55%), `thingi10k/105762`, `thingi10k/906891`.
- **Inverted quads** in 46 of 70 results (the oracle: 52 of 60). Most are single quads at tiny patches.
- **Quantization time** on many-patch models: 26 to 38 s on AntiqueCamera, ChronographWatch, CommercialRefrigerator and ABeautifulGame. A clash retry quantizes again.
- **The default size** still runs away on flat inputs (open item 3, untouched).
- BrainStem is 2.1% from its input; not yet diagnosed.

## Rejected

- Keeping big-fan patches as triangles and quantizing again: quantization already costs up to 38 s on the models that have them.
- A multi-chord split (peeling Center triangles off the long side): it has no room on strips one quad thick, which are most of the worst cases. Bridge covers both.
- Dropping short sharp spurs from the boundary instead of cutting from them: simpler, and it lets quads straddle the crease's last edge.
