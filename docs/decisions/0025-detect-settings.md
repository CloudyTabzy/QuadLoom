# 0025 — Detect Settings: a model's kind from its crease lines

**Status:** implemented, 2026-11-27. It is version **0.4.3**: full gate green, scored on all 70 inputs (below), production zip built and smoke-tested, merged to `main`. The benchmark through the add-on was not re-run: at default settings the pipeline differs from 0.4.2 on one model. First published in 0.4.6.

## The request

Decision 0024 ended with a setting that helps some models and harms others. The request was a detector: a button that reads the selected mesh and picks the settings that suit it, telling a "bunny" from a "cheburashka" from a "refrigerator", with the numbers shown in the panel and a second button to apply the result.

## What tells the kinds apart

The angle test's own output. Edges it alone found, joined at shared vertices, are a model's **crease lines** (`features::CreaseLine`: length in nominal edges, mean fold). A line is **form** if it is 10 nominal edges long or folded 80° on average; the rest are **scraps**. Over the 70 benchmark models, the share of a model's crease length in form lines separates the three kinds the request named:

| Model | Lines | Form share | Scraps |
|---|---|---|---|
| fandisk | 2 | 1.00 | none |
| CommercialRefrigerator | 340 | 0.95 | 100, folded 64° |
| cheburashka | 26 | 0.87 | 16, folded 50° |
| bunny | 203 | 0.17 | 200, folded 46° |
| armadillo | 680 | 0.01 | 676, folded 44° |
| rocker arm | 229 | 0.00 | 229, folded 45° |

Reading the lines takes validation, cleanup and the angle test: no remesh, a fraction of a second on most meshes (`advise::crease_lines`).

## The rule, and what it was set against

All 70 models were run four ways: every crease kept, Ignore Faint Creases at 4 and at 9, Sharp Edges off. `advise::decide`:

| Form share | Kind | Suggestion | Why |
|---|---|---|---|
| under 5 nominal edges of crease in all | Smooth | none | nothing for Sharp Edges to follow |
| 0.9 and over | Hard surface (32 models) | Sharp Edges on, every crease kept | dropping the few scraps gains nothing on most and cost two their layout |
| 0.1 to 0.9 | Sculpt with creases (28) | Ignore Faint Creases 9 | below |
| under 0.1 | Surface detail only (6) | none | no setting did better than another across them (armadillo and homer worse without their scraps, igea and DragonAttenuation better) |

**Sharp Edges off is never suggested.** By quad shape alone it was the best of the four on most models, hard-surface ones included, because a mesh that ignores its hard edges has nothing to conform to. The condition was that hard edges are not softened.

### The proof run

Each model with its detected settings against the defaults, all 70 (`quad_eval.py --advise` (a development tool, not in the public repository)):

- The 42 models it does not change are unchanged.
- **Of the 28 it changes: mean scaled Jacobian better on 19, worse on 9; the 5th percentile better on 19, worse on 9 (summed +0.86).** Irregular vertices fewer on 14, more on 13.
- Hausdorff distance better on 10, worse on 17, by little except the pig: its tail is thinner than a quad, and without the scraps round it the quads there are larger (0.95% to 2.0% from the input). Mean distance worse on 22 of 28, summed 0.06 points.
- Medians over all 70: mean scaled Jacobian 0.946 to 0.952, 5th percentile 0.768 to 0.804.

So where it suggests a change it is right about two times in three, and it leaves hard-surface models alone. That is a suggestion to show with its numbers, not a default to apply silently, which is how it ships.

What would raise the two in three: telling a broken soft crease (collinear pieces) from noise, a layout that does not lean on scraps, and smaller quads on thin parts (the pig).

## In the add-on

- **Detect Settings** (`quadloom.detect`), in the main panel under Sharp Edges. Runs `bridge.advise` on a snapshot of the active mesh. Changes no setting.
- A **Detection** box: the object, the kind, crease lines, form lines and their share, scraps and their fold; the suggested Sharp Edges and Ignore Faint Creases beside the values now; one or two sentences of why (`wording.KINDS`).
- **Apply Suggested Settings** (`quadloom.apply_detected`): sets the two settings, as one undo step. Disabled when nothing would change.
- **Stale detections are refused.** The detection stores the object's name and a fingerprint (vertex, edge and face counts, and the angle). If any differs the box says to detect again and Apply is disabled.
- State lives in `Scene.quadloom_detection`. Add-on API 11 (`advise`).

**Ruling: detection runs in Blender's own process**, not the memory-limited worker (decision 0021). It allocates a cleaned copy of the mesh and an edge list, is bounded by the face limit checked before anything is copied, and runs none of the stages that have aborted on memory. A worker round trip would cost more than the work.

Not done: detecting on every Remesh. It is a possible later option, once the rule has proven itself on real models.

## A guard found on the way: quads that leave the surface

SpecularSilkPouf laid out without its faint creases has a seam strip 0.7 long and almost no width, five-sided. Its pattern joined points far apart: quads a third of the model long, cutting through it, the result 9.4% of the diagonal off the input, no error.

`quad::astray_patches`: a patch with a quad edge over three times the quad size whose midpoint is more than one quad size off the layout's mesh keeps its triangles (through the same retry as clashing patches). A long edge that stays on the surface, a flat face with large quads, is not refused. At default settings one benchmark model is affected; SpecularSilkPouf with faint creases ignored: 9.36% to 0.55%.

## Tools

`quadloom lines <mesh>` (every crease line) and `quadloom advise <mesh>` (the kind and suggestion) print JSON. `quad_eval.py --advise` runs each case with its suggested settings. The development probe takes `QL_MIN_FOLD=<x>`.
