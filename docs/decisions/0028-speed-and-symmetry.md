# 0028 — A faster count solver and prep remesh; Symmetry at the whole model's scale

2026-11-27. Version 0.4.6.

## The problem

Two requests.

**Speed, in the count solver and elsewhere.** 0.4.5's four layout attempts took the 70 benchmark models from 630 s to 964 s through the add-on, and [0027](0027-best-of-n-layouts.md) ended on "a faster count solver would make every attempt cheaper, and nothing else would". Where a run's time goes was not known beyond that.

**Mirrored results.** "Models like Suzanne would benefit if the mesh is mirrored, so one side of the remesh is like the other: see if this is already done, or needs revision." It is done (Symmetry, [0009](0009-symmetry.md): X, Y, Z or Auto Detect; one side is remeshed and mirrored, exactly). It is off by default, and trying it on the benchmark's Suzanne found two bugs and one thing missing.

## What others do

**Quantization as a flow problem.** Heistermann, Warnett and Bommes, [Min-Deviation-Flow in Bi-directed Graphs for T-Mesh Quantization](https://cgg.unibe.ch/publication/011282/) (SIGGRAPH 2023; [libSatsuma](https://www.github.com/cgg-bern/libSatsuma)): the counts of a quad layout are a minimum-deviation flow in a bidirected network, solved exactly through a minimum-cost matching. On QuadWild's own inputs their exact solver takes 0.49% of the time QuadWild's integer program does. Campen, Bommes and Kobbelt, [Quantized Global Parametrization](https://www.graphics.rwth-aachen.de/publication/03251) (2015), is where the problem's structure was first used instead of a general solver. Behind both: a separable convex cost over network constraints is solvable in polynomial time ([Ahuja, Hochbaum and Orlin](https://hochbaum.ieor.berkeley.edu/html/pub/AHO-cut-based-Algorithmica-2004.pdf); [Karzanov and McCormick](https://www.osti.gov/biblio/36273)).

**What a general solver does before it searches.** A program whose constraints fall into blocks that share no variable is solved block by block: SCIP's components presolver ([Gamrath et al. 2015](https://link.springer.com/10.1007/s12532-015-0083-5)) and, when the blocks are only nearly apart, [decomposition branching](https://optimization-online.org/wp-content/uploads/2018/08/6788.pdf). And a convex piecewise-linear cost is written as bounded columns, one per piece, not as one row per piece: the simplex method carries a column's bounds for nothing, and every row is a row of the basis.

**Long triangles in a bounding-volume hierarchy.** Ernst and Greiner, Early Split Clipping (2007), and Stich et al., Spatial Splits in Bounding Volume Hierarchies (2009): a box round a long thin triangle is mostly empty space, so the triangle is entered into the tree as several pieces, each with a box its own size.

**Symmetry in other remeshers** is something the artist sets: Blender's QuadriFlow follows the mesh's own symmetry option, Quad Remesher has a toggle per axis. None looks for a plane.

Taken: solving block by block, the cost as bounded columns, and the pieces. The flow formulation is the larger step and is not taken here (see "What it does not do").

## Where the time went

All 70 benchmark inputs through the command line at one layout attempt, with timing notes behind an environment switch (taken out again):

| Share of 574 s | 0.4.5 |
|---|---|
| Relaxing the quads | 29% |
| Prep remesh | 28% |
| The count solver's whole-model program | 22% |
| Tracing the layout | 8% |
| The count solver's region programs | 6% |

Each of the first three turned out to be one thing.

## The design

### 1. A search that widens (`spatial/mod.rs`)

The relaxer's time was all in one call: the nearest triangle *of a vertex's own patch* (`Cover::pull`), a search with a predicate. A search that has found nothing yet has no bound, so with a choosy predicate it went through every triangle of each subtree it entered. 0.4.4 had met the same thing in the repair pass and bounded that one call from the vertex's position; here there is no such bound to hand.

`TriangleBvh::closest_triangle_where` now looks within two mean triangle edges first, then four times as far, and again, and only then everywhere. What it finds in a bounded pass is what the unbounded pass would have found (nothing nearer exists), so the result is the same to the bit: 70 of 70 benchmark results byte-identical, and the test `a_widening_search_is_the_one_pass_search` holds it there for near and far points, five predicates and both SIMD levels. CommercialRefrigerator 79 s to 9.2 s, ChronographWatch 74 s to 26 s, AntiqueCamera 33 s to 9.6 s; all 70, 738 s to 569 s.

### 2. One count program per connected part (`quad/ilp.rs`)

Two patches bear on each other only through a free segment they share. So the count problem of a layout falls into independent blocks, its connected parts, and the optimum of the whole is the parts' optima put together. `ilp::parts` finds them (a union over each patch's free segments; a fixed segment joins nothing, being a constant to both its patches) and `ilp::refine` solves each part's program on its own, side by side, each with the node limit the whole used to have.

What that changes:

- **A model of many parts gets a whole-model answer at all.** Over 2,500 free segments the single program was skipped and only fifteen-patch regions were repaired, which cannot move a count along a strip that leaves the region. AntiqueCamera has 6,899 free segments in 100 parts, the largest 628. Its count cost (deviation plus fifty times the pattern cost) went from 73,500 to 12,100, and the solve from 1.4 s to 0.6 s.
- **A search of bounded length finds more in forty small programs than in one large one.** SunglassesKhronos, 1,249 segments in 26 parts, under the old limit: cost 4,557 to 1,296, 1.0 s to 0.1 s.
- **A one-part model is as it was.** The same program, the same answer.

A part over 2,500 free segments is still left to the regions (ChronographWatch's largest has 6,095).

The region solver builds its program from its own patches now (`ilp::Scope`: the free segments and the patches they are sides of) and a candidate is judged on those alone (`quantize::Problem::improves`), where every region used to walk every patch of the model twice.

### 3. The deviation as bounded columns (`ilp::deviation_pieces`)

A count's deviation `(n − t)² / max(t, 1)` was seven rows per segment, one per unit step round the target (`d ≥` each step's chord): three quarters of the program's rows. It is now `n = lo + Σ z_k` with one bounded `z_k` per step, each with that step's slope. The slopes rise, so a minimum fills the pieces in order and the cost is the same function: exact at the integers within three of the target, straight outside them. One row per segment in place of seven; a search node costs about half what it did.

The answers are not the same answers: the relaxation has the same value but another vertex, so the search branches elsewhere. They are as good (below).

### 4. Long triangles in pieces (`spatial::cut_long`)

Two models spent 25 to 38 s in the prep remesh's collapse pass, on inputs of 8,000 triangles. The cause was the input: `crane/oloid` is a ruled surface triangulated along its rulings (median triangle 1,000 times as long as wide, every one 42% of the model's size) and `thingi10k/1514900` a CAD export with flats in slivers. Round such a triangle a box is a slab of empty space, and a surface made of them is boxes that all overlap: a nearest-point query cost 31 microseconds where the armadillo's costs one.

A triangle more than four times as long as the mesh's own size of triangle (the edge of an equilateral triangle of the mean area) is now entered into the tree as pieces of that size, halved across its longest edge until they are. A query names the triangle a piece is of, and the points of the surface are the same points. There is a budget (half a million pieces, or four a triangle), kept by making the pieces longer.

Only a mesh made of such triangles is cut: one whose long triangles, measured in pieces, come to four per triangle of the mesh or more (the oloid: 29; `1514900`: 8). The first version cut any mesh with one long triangle, and nearly every prep mesh and quad mesh has a handful: each tree became a copy of its mesh for a few hundred pieces. The second cut at a quarter of a piece per triangle, which takes in CAD exports with a tenth to a third of their triangles long (1 to 2.5 by that measure): their trees came out up to sixteen times the mesh, for no gain that showed. Halving a sliver makes three slivers of half the length, so the pieces grow faster than the length; slicing it across would not, and is left for when a mesh between the two kinds needs it. A mesh that is not cut is built as before, to the bit.

The oloid's prep remesh went from 38 s to 10 s and `1514900`'s from 36 s to 12 s.

### 5. Symmetry at the whole model's scale (`symmetry::Whole`, `pipeline::Side`)

With Symmetry on, the benchmark's Suzanne came out with 19,038 quads where it gets 11,747 without. Every size the pipeline takes from the model was being taken from the kept half: the triangle budget over its area, the nominal edge length, the tolerance over its diagonal. The half got the whole's triangles on half the area, edges 0.7 times as long, and nearly twice the quads once mirrored.

`symmetry::Whole` is what the kept side stands for (its copies, and the diagonal of its bounding box with the mirror image), and `pipeline::Side` carries it with the planes and the seam to feature detection and to the per-part sizes: areas and face counts are times the copies, a part's volume is measured from a point on the planes (so the open side adds nothing), and the seam is not an open edge. Suzanne: 11,840 quads with Symmetry, 11,440 without, in 1.1 s against 2.5 s.

### 6. A centre line a rounding error off the plane

The same Suzanne had twelve quads of no size along the mirror plane. It is not centred on its origin, so Auto Detect takes the plane from the bounding box, and the box's centre and the model's own centre-line vertices differ by the rounding of their 32-bit coordinates. The cut went through every centre-line triangle a hair from its edge. Vertices were snapped to the plane within a billionth of the diagonal, which is nothing; it is now a millionth (the distance cleanup welds at), and never less than a few steps of a 32-bit coordinate as far out as the mesh lies.

### 7. Auto Detect leaves a one-sided detail alone (`symmetry::planes`)

Auto Detect took a plane when the root mean square distance of mirrored sample points from the surface was under the tolerance, 0.5% of the diagonal. One detail does not move a root mean square. Of the 70 benchmark models 44 passed, and a quarter of those have something on one side only: ChronographWatch's crown (3.4% of its size), the cow's tail (1.9%), a lug on `thingi10k/1514900` (6.7%). Mirrored, the kept side's detail appears twice or the other side's is gone.

A plane that passes that test is now checked at every vertex, and taken only if no vertex's mirror image lies farther from the surface than the tolerance. That is what the setting has always said it is ("largest asymmetry Auto Detect accepts"). The root mean square stays as the cheap first test. The report carries the figure (`SymmetryReport::worst`), a skipped run says which axis and by how much, and an axis chosen by hand is still mirrored as asked, with a warning that says what it costs.

Ten of the 44 are now left whole and two keep one plane fewer; of the 34 still mirrored the largest difference is 0.47% (homer) and most have none.

### 8. Detect Settings looks for mirror planes (`advise.rs`)

Symmetry being off by default, an artist has to know the model is symmetric about an axis of its own and turn it on. Detect Settings now runs Auto Detect's test as well, shows the planes it found and how far the sides differ, and suggests Symmetry's Auto Detect when there is one; Apply turns it on. It never suggests turning Symmetry off. Add-on API 13.

The default stays off. Turning Symmetry on by default would change every symmetric model's result, so it is left for a later decision.

## What it measured

### The count solver on its own: one attempt

All 70 inputs through the command line at one layout attempt, single runs, the relaxer's search already widened in both. A model counts as clearly better when it moves by more than a run and its twin usually differ (0.004 of mean scaled Jacobian, 0.02 of its 5th percentile, 0.03 points of distance).

| | Models whose result changes | Mean scaled Jacobian, better / worse | 5th percentile | Largest distance | Count solver, all 70 |
|---|---|---|---|---|---|
| One program per part | 23 | 8 / 1 | 8 / 2 | 5 / 2 | 191 s to 205 s |
| and the deviation as columns | 55 | 10 / 3 | 9 / 2 | 6 / 5 | 191 s to 108 s |

The 23 are the models of more than one part. Over them the irregular vertices fall by a fifth (2,001 a model to 1,601) and the quads rise 3.7%.

| Model | Mean scaled Jacobian | 5th percentile | Irregular vertices |
|---|---|---|---|
| ABeautifulGame | 0.882 to 0.905 | 0.35 to 0.58 | 4,192 to 2,835 |
| AntiqueCamera | 0.890 to 0.906 | 0.33 to 0.46 | 3,142 to 2,265 |
| DamagedHelmet | 0.905 to 0.930 | 0.45 to 0.66 | 984 to 707 |
| SunglassesKhronos | 0.876 to 0.928 | 0.46 to 0.64 | 2,489 to 360 |
| ChronographWatch | 0.763 to 0.774 | 0.09 to 0.10 | 10,872 to 8,829 |

One program per part costs time where there was no whole-model search before (SheenChair's count solve 0.5 s to 9.9 s, ToyCar's 1.6 s to 4.6 s) and saves it where one large search becomes several small ones (SunglassesKhronos 2.0 s to 0.2 s); the columns halve what is left.

### At the default four attempts, as pairs

Each build run twice on all 70 (as built, and with the remesh tolerance times 1.0005), a model counted only when both new runs beat or lose to both old ones; chance gives about 12 each way. The old build is 0.4.5 with the widened search (the same results as 0.4.5 to the bit).

| Measure | Clearly better | Clearly worse | Median, old to new |
|---|---|---|---|
| Mean scaled Jacobian | 10 | 11 | 0.954 to 0.957 |
| Scaled Jacobian, 5th percentile | 12 | 6 | 0.794 to 0.802 |
| Largest distance from the input | 9 | 10 | 0.315% to 0.324% of the diagonal |
| Mean distance from the input | 16 | 8 | 0.0162% to 0.0159% |

Within chance, taken over all 70. Four layout attempts were already picking their way round a poor count solve, so most of what one attempt gains is not there to gain a second time. What is left is on the models named above and their kind: SunglassesKhronos (mean scaled Jacobian 0.878 and 0.858 in the old runs, 0.918 and 0.921 in the new; its 5th percentile 0.46 and 0.26 to 0.54 and 0.54), ChronographWatch (0.767 and 0.769 to 0.788 and 0.796), CommercialRefrigerator, BrainStem, AntiqueCamera's worst quads. Quads over all models: 1.9% more. One of the two new runs has a model over 1% from its input (`thingi10k/62287`, 1.07%), where neither old run has one. Against the oracle the two builds are level: better by mean scaled Jacobian on 55 and 53 of 60 (the old: 55 and 51), by its 5th percentile on 52 and 51 (53 and 51), by Hausdorff on 51 and 49 (54 and 51).

### Time at the default settings

All 70 through the command line with four layout attempts, three models at a time on an otherwise idle machine (the build before the last change to the cut rule; see "Through the add-on" for the final one):

| Build | All 70 |
|---|---|
| 0.4.5 with the widened search | 911 s |
| 0.4.6 | 753 s |

That is what the count solver and the pieces add to the widened search: 17% off. (What the widened search took off 0.4.5 itself is in the design above; through the add-on, below, the two are measured together.) By stage, summed over the models: prep remesh 196 s to 167 s, the first attempt's quad stage 288 s to 192 s, the later attempts 345 s to 297 s, tracing 62 s to 74 s (which nothing here touches: three at a time, a stage's time moves by that much with what runs beside it).

Model by model the three-at-a-time figures are not to be trusted either way: `thingi10k/650187` read 29 s and 45 s where, run alone, it takes 29 s in both builds. Nine models that read slower, run alone: 151 s to 138 s in all, and one of them really is slower, `thingi10k/1505036`, whose count search takes 11 s where it took 6 s. The largest gains are `thingi10k/271869` (91 s to 13 s; its count solve alone was 52 s of a one-attempt run), the armadillo (32 s to 21 s), AntiqueCamera (29 s to 21 s) and the oloid (31 s to 21 s).

### Through the add-on

The production zip on all 70 models at the default settings, two Blender processes at once ([report](../../benchmark/reports/20261127-q046-default/README.md)): 70 of 70, and **683 s against 0.4.5's 964 s**, 29% less, the widened search and the rest together. A dozen heavy models pay for it (CommercialRefrigerator 73 s to 11 s, ChronographWatch 136 s to 80 s, AntiqueCamera 45 s to 15 s); the median model takes 0.97 times what it did. Against the oracle over the 60 it finishes: better by mean scaled Jacobian on 53 (0.4.5: 53), by its 5th percentile on 50 (50), by Hausdorff on 52 (50). One result over 1% from its input, Suzanne at 1.49% as before.

The paired runs above were made with a build that cut more meshes for the tree than the final one does (a quarter of a piece per triangle where it is now four). The difference is which of two equally near triangles a query names, on the meshes between the two rules; the add-on run is of the final build.

### Symmetry

- The benchmark's Suzanne, Symmetry on against off: 19,038 quads against 11,747 before; 11,840 against 11,440 now, in 1.1 s against 2.5 s. No quad of no size on the plane (there were twelve).
- Mirror planes over the 70 benchmark models: 44 pass the first test. Ten are now left whole (the cow, BarramundiFish, ChronographWatch, CommercialRefrigerator, Corset, SpecularSilkPouf and `thingi10k/1514900`, `62287`, `650187`, `99836`) and two keep one plane fewer (`thingi10k/472206`, `94990`). Detect Settings suggests Symmetry for 34.
- Those 34 with Auto Detect on against off, single runs at the default settings:

  | | Off | On |
  |---|---|---|
  | Time, all 34 | 205 s | 63 s |
  | Mean scaled Jacobian, median | 0.964 | 0.967 |
  | Its 5th percentile, median | 0.849 | 0.855 |
  | Largest distance from the input, median | 0.289% | 0.303% |
  | Mean distance, median | 0.0163% | 0.0183% |
  | Quads, all 34 | 600,000 | 574,000 |

  Quad shape is level (mean scaled Jacobian clearly better on 12 and worse on 9). The result is a third of the time and exactly symmetric. It is a little farther from the input: the largest distance clearly worse on 16 and better on 8, none over 1%. Two causes were found. A face of the input that lies in the mirror plane (a fin, a membrane: Suzanne, the teapot's spout, WaterBottle) is on neither side and is left out. And a fault of the one side remeshed is a fault on both.
- The prep mesh is to scale exactly: a torus's triangles times its copies are within 2% of the whole's, for one, two and three planes, at the same edge length. The quad count follows less closely. A torus at one attempt gets 15% more quads with one plane, 27% more with two and 11% fewer with three, from whole counts along a few long patch sides. Over the 34 models it is within 15% of the count without Symmetry on 25. `thingi10k/906891` gets half the quads and the oloid 0.56 times; both have thin parts that the plane cuts, which the thin-part rules ([0026](0026-thin-parts.md)) take for half a tube, the likely cause and not yet confirmed.

## Tried and not taken

- **Stopping a search that has stopped improving.** The search can be run in stages (`resume_with`) and stopped after a stage with no better answer. In microlp a resumed search leaves its dive and starts from the open node with the best bound, so staging is another search, not the same one cut short: in stages of 50 nodes the armadillo's never reached a leaf in 2,000. In stages of 250 or 500 it saves 20% to 60% of the solver's time, and three models come out 11 to 24 times worse by the program's own cost (the pig, `thingi10k/271869`, `thingi10k/906891`): their first good answer comes late.
- **The regions before the whole, or instead of it.** The regions alone leave the pig at a cost of 7,700 where the whole program reaches 1,480; larger regions (two rings, forty patches; up to six rings, 300) are no better and often worse.
- **A band round the starting counts, or fewer unit steps in the deviation.** Faster on one model and worse on the next.
- **Region programs round every patch after the whole** ("polishing"): 20 s more over the 70 models for nothing that shows.
- **Timing notes left in the code.** They are out; `QL_TIME` is not a setting.

## What it does not do

- **The count solver is still a general integer program.** Of 58 part solves over the benchmark, 35 stop within a tenth of a percent of the relaxation's bound and 23 run to the node limit, one solve in ten more than 30% above the bound: the structure is there to be used. A flow solver of our own (the bidirected formulation, with a matching) would take the search away. That is a milestone, not a patch.
- **A part of more than 2,500 free segments** still gets regions only.
- **The prep remesh's size field** takes 107 s of processor time on ABeautifulGame (10 s of the run): it measures curvature over a ball at every vertex of the dense input, each iteration.
- **Tracing** (11% of a one-attempt run, and every attempt traces) was not looked into.
- **A thin part the mirror plane cuts** is still half a tube to the quad stage's six-quad rule ([0026](0026-thin-parts.md)).
- **Mirror planes that are not along the object's axes**, and radial symmetry.
