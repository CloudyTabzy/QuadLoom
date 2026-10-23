# 0017 — No starved patches, no spur-blocked patches; clearer quick re-runs

**Status:** accepted 2026-10-23.

## Context

The owner tested 0.1.9 on the armadillo and reported three things.
1. A quick re-run stayed on "Checking" with no progress, then appeared; nothing said it had been quick.
2. At Density 2, one pec showed irregular hex-like quads instead of a grid.
3. Under Organic / Sculpt, the hand came out as a few huge faces.

**Reproducing.** The owner's numbers (53,960 and 15,938 quads) were reproduced exactly by running the real add-on headless in a throwaway Blender profile. A scratch probe linked against `quadloom-core` then gave the same results, once it read meshes in file order and used the add-on's f32-rounded settings. The results were then rendered (front, back, chest and hand) for visual checks.

## Findings and decisions

- **Progress.**
  - The quad stage reported almost nothing (quantize and smoothing not at all), and a reused run never left the first stage.
  - Now `finish` reports at once; quantization reports at its phase boundaries and per regional-repair round; smoothing reports per round.
  - The binding's `Cache.reusing()` lets the panel show "Quick Re-run: Quad Sizes… 60%".
  - The info notice starts "QuadLoom (Quick Re-run):".
- **Spurs (the pec).**
  - A traced path ending inside a patch is walked down one side and back up the other, so the patch's boundary loop visits a vertex twice (`a → x → a`). `segments::usable` then refused the patch, and it was kept as triangles split into quads: the hex look.
  - `trace::without_spurs` removes such spurs. It applies only when the spurs were all that made the patch non-convex, and not along a crease sharper than 60°: a hard crease ending inside a patch folds any pattern (fandisk had 31 inverted quads until this condition was added).
  - The patch's kind is then re-derived (`kind_without_spurs`).
- **Starvation (the hand, and much more).**
  - A patch that can't take a valid pattern at sensible counts (a sliver, a non-convex patch) was "fixed" by the quantizer shrinking whole strips of segments around it. Each unit of pattern infeasibility costs `regularity` (50); a segment's deviation costs only (n − t)²/t.
  - Measured at Density 2 (area in patches below 40% of their ideal quad count): bunny 26%, armadillo 16%, igea 8%. Density also under-delivered: 2× Density gave about 2.9× the quads instead of 4×.
  - Now no segment drops below half its target (`quantize::count_floor`): in the cost, the rounding, the ±1 moves, the parity fixes and the ILP's variable bounds.
- **Not fixed here: patches with many singularities.** Under Organic (no angle features), the armadillo's hand ends up in patches holding up to 15 fingertip singularities (20.6% of the area; at most 1.3% on every other model and setting).
  - A single pattern can't represent that, so the fingers flatten.
  - The layout's acceptance rules rank "no new concave corner" above "fewest singularities per patch", so every split is refused.
  - Keeping such patches as triangles preserved the fingers, but nearly tripled the quad count (19.6k → 52.7k), so it was dropped.
  - Reordering the acceptance rules is a layout change that needs its own evaluation.

## Evidence

**Floor sweep** (shares 0 / 0.33 / 0.5 across 7 models × Density 1 / Density 2 / Organic):
- At 0.5, starvation is gone everywhere.
- Quad shape is neutral or better nearly everywhere: the bunny's p5 scaled Jacobian at Density 2 goes 0.58 → 0.60.
- The exception is the armadillo under Organic, whose multi-singularity patches get worse.

**`benchmark/stress_quads.py`**, default settings, previous build → this one:

| Model | Previous | This |
|---|---|---|
| bunny | 2 kept, 23 inverted, p5 0.68 | 0 kept, 13 inverted, p5 0.73 |
| igea | 1 kept, 40 inverted, p5 0.56 | 0 kept, 22 inverted, p5 0.60 |
| armadillo | 10 fans, 1 kept, p5 0.59 | 1 fan, 0 kept, p5 0.54 |
| fandisk | 1 kept, 0 inverted | 1 kept, 0 inverted |
| rocker-arm | p5 0.78 | p5 0.81 |
| other 5 models | — | about the same |

**Renders:**
- The pec is a clean grid.
- At Density 1 the armadillo's back and arm have no blocky regions left.
- The Organic hand is still flattened (above).

Output now differs from 0.1.9 by design (more quads where patches were starved, patterns where patches were kept).
