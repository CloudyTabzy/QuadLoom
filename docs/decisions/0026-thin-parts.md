# 0026 — Thin parts: elements round a full turn

**Status:** implemented, 2026-11-27. It is version **0.4.4**: full gate green, scored on all 70 inputs as pairs of runs (below), production zip built and smoke-tested, benchmarked through the add-on, merged to `main`. First published in 0.4.6.

## The problem

No stage knew how thick a part is. The prep remesh sizes edges by how far they may stray from the surface, down to 0.3 of the target length and no further; quads take the model's size and, by Adaptive Detail, the square root of their triangles' size. On a part thinner than a quad that came to:

- as few as five triangle edges round a tube thinner than the target length (the tip of a tail, a wire), which is little for a layout to be traced on;
- small holes closed over by the remesh (`thingi10k/906891`: pin holes 3 units deep in a 230-unit part, gone, 1.31% of the diagonal off the input);
- at a low Density or Adaptive Detail, two to four quads round a tail or a leg, so a block where there was a tube (the pig's tail at Low Poly's size).

## What others do

Read before designing (ideas only, no code taken):

- [Bawin, Henrotte, Remacle 2020](https://arxiv.org/abs/2009.03984), the size field behind Gmsh: a curvature size `h = 2πr / n` (n elements round a full circle, 20 by default), a feature size from the medial axis for thickness, the smallest of them taken, and the result graded so it grows by at most a fixed ratio from one element to the next.
- [Reberol et al. 2021](https://arxiv.org/abs/2103.04652), quasi-structured quad meshing in Gmsh: the same size map, limited edge by edge as `s(b) ≤ s(a) + g · |ab|`, and curve counts taken as the integral of 1/size.
- [Persson 2006](https://dx.doi.org/10.1007/s00366-006-0014-1): gradient-limited size functions, the grading both use.
- [Shapira et al. 2008](https://link.springer.com/doi/10.1007/s00371-007-0197-5), the shape diameter function: thickness by casting rays inward.

Taken: a count of elements round a full turn, and a graded size. Not taken: thickness by rays or a medial axis. A thin plate is thin by both and needs no more elements for it, and a count round a turn already says "this many across the part" where the part is round.

## The rule

**Where the surface turns all the way round within reach, at least N elements go round it:** twelve triangle edges in the prep remesh, six quads in the quad stage. It has to tell a tube from three things that are as tightly curved at a point and are not thin parts:

| Shape | Turns | Wanted |
|---|---|---|
| A tail, a finger, a wire, the wall of a small hole | all the way round | elements round it |
| The rounded rim of a plate | half way, then stops | nothing: one quad may span the rim |
| A fold between two bulges, ripples | one way, then back | nothing |
| A bar with sharp edges | at its creases only | nothing: one quad a face is exact |

A rule on curvature alone (the first version tried) refined all four, and cost quad shape on every model with folds or rims for it.

### In the prep remesh (`remesh/sizing.rs`)

1. **Curvature is read at the part's own scale.** The jet fit measured curvature over a ball half a target length across. Round a tube thinner than that, the ball wraps the tube and the fit reads a third of its curvature, so the thinnest parts were sized as if they were gently curved. The ball is now halved while its normals disagree (`GENTLE`), up to four times.
2. **The turn rule.** Where a vertex's own curvature asks for more than the deviation rule gives, the *net* curvature is taken: the area-weighted mean of the signed curvature over a ball of 1.25 target lengths (`Ball::net_curvature`), and the edge length is `2π / (12 · net)`, no shorter than a tenth of the target. Round a tube the net curvature is the tube's. On a plate's rim the flats beside it count as well, and it comes to about 2 over the ball's radius, which asks for no more than the deviation rule's shortest edge. Across a fold or ripples the signs cancel. For that the curvature is signed: positive where the surface bulges the way its normals point.

It may go under Finest (`min_mult`), which bounds the deviation rule only: that one trades edge length against a tolerance, this one keeps a thin part a shape at all.

### In the quad stage (`quad/turn.rs`)

Quads then follow the triangles by Adaptive Detail's square root, which is enough at the default settings and not below them: at Low Poly's size a tail got four quads round it. `turn::cap` lowers the quad-size multiplier where the surface turns all the way round within two quad edges, so six quads go round, and lets the size grow back by 0.3 of a quad edge per quad edge of distance (gradient limiting, swept outward from the capped vertices).

It reads the turn from the normals in a ball, not from curvature at a point. The ball is sized by the quads and may be many triangles wide; a low-poly tube is flats and creases at any one point, and the remesh's readings on it mean nothing at that scale (the first version went by them and read a 16-sided torus as a fifth of its curvature).

- **How round:** the covariance of the normals about their mean, weighted by area. Its eigenvalues are (½, ½, 0) round a tube, (⅓, ⅓, ⅓) on a ball, (1, 0, 0) on a plate seen from both sides. The sum of their products in pairs (0.25, 0.33, 0) needs no eigenvalues; 0.2 and over is a full turn or nearly. Half a tube has 0.05, a dome 0.17: a stud is detail, not a thin part.
- **How thick:** on a tube or a ball of radius r about a centre c, a point is `c + r · normal`. So r is the slope of position against normal over the ball: exact for a cylinder of any number of sides, for part of one, and for a hole (negative; its size is taken).
- **Where:** balls from a quarter of a quad edge up to two, smallest first. The smallest round one holds one section of the part and little of the body it grows from or the curl it bends into.
- Creases are not crossed, so a bar with sharp edges is flat faces and keeps its one quad a face.
- The surface is read at vertices a quarter of a quad edge apart and each vertex goes by the nearest, so the cost does not grow with the square of the quad size.

In Target Count mode the quad edge is not known beforehand; the cap goes by the edge the count comes to on the surface's area.

## How it was judged: every run has a twin

The first scores looked like a clear win (the pig's tail from 0.95% to 0.45% off its input) and a clear loss (SunglassesKhronos' worst quads from 0.72 to 0.24). Neither was the rule. Looked at part by part, the sunglasses' loss was one lens frame whose prep mesh differed by two triangles in 2,796 and whose layout came out differently.

So 0.4.3 was run again with a change that means nothing: the remesh's deviation tolerance times 1.0005.

| 0.4.3, all 70 models | as built | tolerance × 1.0005 |
|---|---|---|
| Mean scaled Jacobian better than the oracle | 51 of 60 | 48 of 60 |
| 5th percentile better | 50 of 60 | 47 of 60 |
| Hausdorff better | 45 of 60 | 46 of 60 |
| max-planck | 11,942 quads, 349 irregular vertices | 9,497 quads, 57 |
| pig, largest distance from the input | 0.95% | 0.54% |
| cheburashka, largest distance | 0.33% | 1.01% |

Between the twins the 5th-percentile scaled Jacobian of a model moves by 0.023 at the median and by up to 0.15, and the largest distance by 0.03 points at the median and up to 0.67. **A single run cannot judge a change that touches the prep mesh.** The tuned build also carries about three models of luck in its counts against the oracle: it was tuned on these runs.

From here on a build is scored as a pair (as built, and with the nudge), and a model counts as better or worse only if both new runs beat, or lose to, both old ones. By chance alone that happens for about one model in six each way: 12 of 70.

## Results

All 70 models at default settings, 0.4.3's pair against 0.4.4's (`quad_eval.py` (a development tool, not in the public repository)):

| Measure | Clearly better | Clearly worse | By chance |
|---|---|---|---|
| Mean scaled Jacobian | 12 | 15 | 12 each |
| 5th percentile | 9 | 16 | 12 each |
| Largest distance from the input | 17 | 16 | 12 each |
| Mean distance from the input | 24 | 9 | 12 each |

- Quad shape and the largest distance are within chance. The mean distance is better on twice as many models as chance gives.
- `thingi10k/906891`: 1.31% off its input in both old runs, 0.24% and 0.39% in the new. Its pin holes are kept.
- Against the oracle, the two new runs: mean scaled Jacobian better on 49 and 50 of 60, Hausdorff on 48 and 46 (0.4.3: 51 and 48, 45 and 46).
- 2.8% more quads over the 70 models. Ten models timed back to back, with the relaxer's search fixed (below): 142 s to 118 s.
- Further than 1% from the input: `906891` in both old runs (and two more in the nudged one); ChronographWatch (1.2%) in one new run and `1706457` (1.1%) in the other, both of which the old pair also moves by that much.
- The quad stage's cap changes 16 of the 70 results at default settings, none by more than 5% in quads. It is for sizes below the default.

Below the default, on the pig at Low Poly's quad size (1.5 target lengths, Adaptive Detail 0.3):

| | 0.4.3's quad sizing | with the cap |
|---|---|---|
| Quads | 4,076 | 5,345 |
| Largest distance from the input | 0.77% | 0.60% |
| Quads round the tail | 4, a square bar | 8 |

A torus whose tube is 2.5 round, under quads 1 long: 36 quads to 152.

So at the default settings this version is a fix for holes and the thinnest tubes and otherwise a wash, and at coarser settings it is what keeps a limb round.

Through the add-on, one run of the 70 ([report](../../benchmark/reports/20261127-q044-default/README.md)): 70 of 70; against the oracle, mean scaled Jacobian better on 49 of 60 and Hausdorff on 46 (0.4.2: 51 and 42); one result over 1% from its input (0.4.2: two); 630 s of tool time against 690 s.

## Found by the benchmark through the add-on: a search without a bound

The first run of the 70 models through the add-on had AntiqueCamera at 270 s (0.4.3: 30 s). Nothing in this decision's code was slow. Timed stage by stage and then step by step, all of it was the relaxer's repair pass: 12 passes over about 4,000 pinched vertices, each trying some 15 positions, each position one search for the nearest triangle on the vertex's own patches.

That search (`TriangleBvh::closest_triangle_where` with a predicate) took 134 µs where a plain nearest search takes 1 µs. Until it has found a triangle it has no bound, and it goes through every triangle of each subtree it enters; when the nearest triangles to a point belong to other patches, that is thousands of them. 0.4.3 has the same cost per search and was spared by chance: its pinched quads on this model sat in patches kept as triangles, whose vertices don't move, and in 0.4.4 the same part gets real patterns (fewer pinched quads: 2,114 against 5,588, and all of them free to move).

The fix is the bound. A vertex already lies on one of its own triangles, so the nearest one to where it is going is no farther than it is now. `closest_triangle_within` takes that distance; if it finds nothing the unbounded search runs as before, so the answer cannot depend on it. Results are byte-identical on the ten models compared, AntiqueCamera through the add-on takes 15.5 s, and every model with many patches relaxes faster (BrainStem 11.8 s to 5.6 s, ToyCar 8.4 s to 5.6 s).

Kept from it: the benchmark through the add-on is the only run that passes materials, seams and marked edges, and it found what 70 command-line runs did not. And a cost that is "bounded in practice" by something unrelated (here, by broken patches happening to be kept) is not bounded.

## What it does not do

- **Under Symmetry, a part cut by the mirror plane** (a tail or a nose on the plane) is half a tube to the quad stage's rule and is not capped. The remesh rule still applies to it.
- **Flat thin things** (a strap, a blade, an ear) are not refined, by design.
- **A sharp-edged bar thinner than a quad** is laid out as long strips one quad wide. When the strips' two sides are counted differently along their length the quads shear, and 0.4.3's guard keeps such a strip as triangles (SunglassesKhronos' lens frame). That is the open "slender patches" item, not this rule.
