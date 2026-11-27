# 0024 — Size per part, strip relaxing, bulging patches

**Status:** implemented, 2026-11-27. It is version **0.4.2**: full gate green, scored on all 70 inputs (below) and through the add-on (`benchmark/reports/20261127-q042-default/`), merged to `main`. First published in 0.4.6.

Work on the open list of decision 0023, and on a question about warped edge flow on smooth surfaces. Three changes went in, one new setting, and two things were tried and taken out.

## Result on all 70 inputs

`benchmark/quad_eval.py` (a development tool, not in the public repository), default settings, against QRemeshify on the 60 it finishes and against 0.4.1. Medians are over those 60; the last two rows count all 70.

| | 0.4.1 | 0.4.2 | QRemeshify |
|---|---|---|---|
| Mean scaled Jacobian, median | 0.942 | 0.951 | 0.922 |
| — better than QRemeshify on | 47 of 60 | 51 of 60 | |
| 5th percentile, median | 0.757 | 0.791 | 0.650 |
| — better than QRemeshify on | 47 of 60 | 50 of 60 | |
| Irregular vertices, median | 120 | 122 | 248 |
| Hausdorff distance, median | 0.332% | 0.344% | 0.491% |
| — better than QRemeshify on | 46 of 60 | 45 of 60 | |
| Mean distance better on | 57 of 60 | 58 of 60 | |
| Further than 1% from the input | 3 of 70 | 1 of 70 | |
| Over 100,000 quads | 10 of 70 | 2 of 70 | |

Against 0.4.1 model by model: mean scaled Jacobian better on 55 of 70, its 5th percentile on 53, mean distance on 42; the Hausdorff distance better on 29 and worse on 41 (by little: the medians above).

## 1. Each part gets its own size

### What was wrong

The automatic edge length (the oracle's `ExpectedEdgeL`) reads how thin a shape is from its sphericity as one body, and gives a thin shape up to sixteen times the faces. That reading has no meaning in two cases, and both got the full sixteen:

- **an open surface** has no volume (a flat 1,267-triangle figure came back as 144,905 quads);
- **several parts taken together**: 27 cubes are as compact as one, with a sphericity of 0.27 as one body.

Ten of 70 benchmark results were over 100,000 quads for this reason.

### What it does now

`pipeline::part_targets` measures each connected part alone and remeshes it at its own length. The quads take their part's size through the per-face size multiplier (`Upstream::part_scale`, `local_size`). With the automatic length a part gets the shortest of:

| Rule | Length | Limit |
|---|---|---|
| Nominal | 10,000 faces over the whole object's area | |
| Thin and closed | shorter by the oracle's sphericity rule (`0.2 / S²` times the faces) | 16 times the faces, as before |
| Close feature lines | two edges in the mean spacing between its lines (area over feature and border length) | 4 times the faces |
| Least resolution | as many triangles as the part came with, between 100 and 400 | an eighth of the nominal length |

A length the user sets is one length for every part, as before. A single compact part is unchanged.

Why each rule:

- **Per part, not a mean.** One length for all parts was tried first (the area-weighted mean of what they wanted). It serves none of them: glasses with thin closed arms and open lenses got a length too long for the arms (quads sheared round a rod a few triangles thick, mean scaled Jacobian 0.785) and shorter than the lenses needed.
- **Feature spacing.** Without it the fix cost every hard-surface asset its quality: their features lie closer than the nominal quads (BrainStem: 5,119 patches for 35,000 quads). Area over feature length is the mean spacing between a part's lines. Measured per part it leaves a plain body alone and refines the grille beside it. As one number for a whole object it asked CommercialRefrigerator for sixteen times the faces because 5% of its area is a grille.
- **Limits.** A thin wall needs its triangles to be a shape at all, so its limit stays the oracle's. Close feature lines only need to be meshed well, and a part covered in them (stitching, a noisy scan) would otherwise always get the most. A limit of four on thin parts was tried and cost them their quality.
- **Least resolution.** An object of many parts shares its faces out by area. A small part's share can be a handful: an edge as long as the part is thick. A flat 400 for every part took CommercialRefrigerator's 376 parts (many of them two triangles) from 57,000 quads to 172,000, so a part gets what it came with, between 100 and 400. And never an edge under an eighth of the nominal length: ABeautifulGame has a speck four thousand times under it, and the run that gave it 100 triangles took over ten minutes where 18 seconds do. (Something downstream does not cope with a part at that scale. Not found; the limit keeps it from being reached.)

### Measured (one model at a time)

| Model | 0.4.1 quads | Now | Mean scaled Jacobian | Oracle |
|---|---|---|---|---|
| woody (flat, open) | 144,905 | 8,761 | 0.973 → 0.968 | fails |
| SunglassesKhronos | 153,980 | 26,091 | 0.942 → 0.936 | 20,180 at 0.889 |
| Corset | 18,422 | 43,852 | 0.882 → 0.948 | 48,059 at 0.938 |
| SheenChair | 24,110 | 33,765 | 0.921 → 0.955 | 26,652 at 0.681 |
| CesiumMilkTruck | 16,010 | 21,546 | 0.938 → 0.948 | 12,078 at 0.886 |
| AntiqueCamera | 144,862 | 87,260 | 0.929 → 0.919 | 90,887 at 0.935 |
| BrainStem (406 parts) | 125,529 | 154,177 | 0.944 → 0.953 | times out |

(Measured before the least-resolution rule was bounded; with it BrainStem is at 79,000 quads and CommercialRefrigerator at 104,000.) Counts now go where the parts need them: down a great deal on open and compact shapes, up on assets of many small detailed parts.

A first version remeshed a single part at the nominal length while giving its quads the part's finer size: quads four times finer than the mesh their layout was traced on (`thingi10k/1514900` lost 0.046 of mean scaled Jacobian). The scoring run on all 70 caught it.

## 2. The relaxer: strips, and quads of different sizes

### What was wrong

A strip one quad thick between two feature lines has the vertices of one line fixed where other lines meet it and the other line sampled evenly, as a segment sampled on its own is. Every quad of the strip then leans, by several times its own height (AntiqueCamera's rails: a fifth of their quads poor). The relaxer could not mend it:

- at a vertex of the free line, the strip's two quads lost to the two wide quads beyond the line and to the even-spacing pull;
- a quad that leans by more than its own length is nearest a square turned along the lean, so its pull was partly the wrong way;
- a vertex on a line could move only a share of the gap to its *nearer* neighbour, also when moving away from it.

### What it does now (`quad/smooth.rs`)

- **`fit_quad`**: a quad with two opposite sides on feature lines fits a rectangle laid along the lines, with the quad's own length and width. Every other quad keeps the square.
- **`Fit::firm`**: the quads round a vertex are weighed. A strip quad holds its corner along the strip by the square of how many times thinner it is than what surrounds it (ten at most). A free quad holds by the square of how many times smaller it is than the largest quad round the vertex (two at most). The vertex's move is the weighted least-squares answer (a 3 × 3 solve; one unknown along a line).
- **`reach_along`**: a vertex on a line is limited by the gap to the neighbour it moves toward.

### Tried and taken out

- **Rectangles for every quad.** Nothing then keeps a quad's proportions: slivers pulled their neighbours in. Inverted results rose from 47 to 55 of 70.
- **Rectangles for quads with one side on a line.** A row along a border was squeezed flat against it by the rows behind (alligator: quads 3.2 long and 0.003 wide).
- **Free quads weighed by their full size ratio (up to ten).** Shape improved, but slivers at tapered patches pulled harder and grew: Hausdorff worse by 2.5 points summed over the 70 models. Two at most keeps nine tenths of the gain for half that cost.

### Measured (70 inputs, same sizes, against the build before)

Mean scaled Jacobian better on 68 of 70, its 5th percentile on 64 (median 0.740 → 0.756). Hausdorff a little worse on 41 (median 0.347% → 0.351%; small quads now hold their shape and the large ones beside them give), better on 27, with `thingi10k/75117` 1.19% → 0.56%. Results over 1% from the input: 3 → 2.

## 3. A patch that bulges keeps its triangles

Quads are counted along a patch's boundary, so a pattern holds about as many as fit in the boundary laid flat. BrainStem has a solid part, 7% of the model long, whose layout left 330 of its triangles in one patch behind a boundary of a few edges. The pattern drew 8 quads across the mouth and the part was gone from the result: 3.3% of the diagonal off the input, no error, no warning. (0.4.1 has this too, at 2.1%.)

`quad::bulges`: a patch with more than four times the area of a square with its perimeter (a hemisphere on its rim has 2.5 times) keeps its triangles. It is the layout that failed there, and the cure belongs in the layout; this makes sure the shape survives meanwhile. One patch in the models checked; none on armadillo, homer, pig or AntiqueCamera. BrainStem: 3.17% → 0.31%.

## 4. Edge flow on smooth surfaces: a setting to ignore faint creases

The request was straighter edge flow on flat and smooth areas, as a default or a setting, with hard edges left alone.

### What bends the flow

Measured with a new scratch metric: at every regular vertex, how far the two edge lines through it are from straight, in the tangent plane. Our flow is already straighter than the oracle's on the same models (bunny: mean bend 6.5° against 10.2°; vertices bent over 20°: 5.0% against 11.6%). What was tried against the rest:

| Lever | Effect on the bunny |
|---|---|
| More relaxing rounds (20 to 100) | none: the relaxer has converged by 20 |
| Regularity (50 to 1,000) | none: every patch is already a grid or a one-singularity pattern |
| Curvature weight (0.1 to off) | singular vertices 147 to 95; bend 6.8° to 5.5° at best |
| The field solved on a smoothed copy of the surface | singular vertices unchanged (about 110) |
| Re-smoothing the unit field for some rounds | 115 to 93 |
| **Sharp Edges off** | **115 to 81; vertices bent over 20°: 5.0% to 1.6%** |

With Sharp Edges at 35°, a sculpt's angle test finds scraps of crease in fur, pores and scan noise: 549 feature edges on the bunny, 680 separate lines on the armadillo, none of them eight quads long. Each is a hard constraint on the cross field in a direction nothing round it shares.

### The setting

`FeatureParams::min_fold` (`features::drop_faint`), in the add-on as **Ignore Faint Creases** under Sharp Edges: a line of angle-only edges counts as a feature only if it carries enough crease, its length in nominal edges times its fold in radians. Never dropped: a line folded 80° or more on average, and any edge that is marked sharp, a seam, a border or a material or face-set boundary. On fandisk and `thingi10k/472206` the feature count is unchanged at every value tried.

### Why it is off by default

A layout also leans on those scraps to cut its patches, and soft real creases come out of the angle test in pieces.

| Value | On all 70, against off |
|---|---|
| 9 | bunny, cheburashka and the rocker arm clearly better (rocker arm 0.959 to 0.970, cheburashka's irregular vertices 268 to 114); six models clearly worse, SpecularSilkPouf 9.4% off its input |
| 4 | irregular vertices fewer on 27 and more on 13; mean scaled Jacobian better on 23, worse on 20; three much worse (`thingi10k/105762`, a vase whose ribs are such creases, lost 0.57 of its 5th percentile) |

No value is a default. It is a setting: 0 keeps every line, 4 to 9 suits a sculpt with surface noise. The add-on's API version is 10.

What would make it a default: joining the pieces of one broken crease before judging them (a rim's scraps are collinear, fur's are not), and a layout that does not need the scraps (the crowded patches of decision 0018).

## Tried and taken out: a limit on rows across a strip

CommercialRefrigerator has a strip a fifth of a quad wide and 43 long that got the nine rows of the wide patch it leads into (394 slivers where 43 quads fit, 306 poor): a quad strip crosses every 4-sided patch with one count, and the count fit has a floor per segment and no ceiling.

- **A hard ceiling** (no segment over twice its target, room for two always) fixed it and the other dense assets (AntiqueCamera +0.022) and broke grids everywhere else: irregular vertices rose on 59 of 70 models, `thingi10k/105762` lost 0.041.
- **A soft one** (a deviation costs more across the end of a slender patch) had the same pattern, smaller.
- **Soft, for patches that are not 4-sided only**: no net effect.

Rows of slivers in a straight strip are perfect by the scaled Jacobian, and the patterns that replace them are not. The strip is poor on the refrigerator because it is a five-sided patch with a singularity in it. That is where to look next, not at the counts.

## Still open

- Slender patches with an odd number of sides (above).
- Tapered patches: a fan of slivers into one boundary vertex (max-planck's base). The new weights make these a little worse.
- The layout failure behind the bulging patch.
- `thingi10k/650187` and `1514900` still get 13 to 15 times the faces as thin closed parts (the oracle's own rule; 128,000 quads).
- A part thousands of times under the nominal scale makes a run take minutes (see the least-resolution rule).
- Faint creases as a default: join broken crease lines first.
- Everything on decision 0023's list that is not named here.

## Tools

The development probe `patchprobe` prints `WHERE` (the poor quads by patch sides and size: this is what showed the loss sits in a few large patches, not the tiny ones), the segments per side under `QL_PATCH`, and `BULGE`. `quadloom run` takes `--field <json>` and `--features <json>` beside `--quad`, and `benchmark/quad_eval.py` passes all three on, so a setting can be scored on all 70 inputs without a rebuild.
