# 0027 — Layout Attempts: several layouts of each part, the best quads kept

**Status:** implemented, 2026-11-27. It is version **0.4.5**: full gate green, scored on all 70 inputs as pairs of runs (below), production zip built and smoke-tested, benchmarked through the add-on, merged to `main`. First published in 0.4.6.

## The problem

Decision 0026 found that the layout is chaotic: the same build with a change that means nothing (the prep remesh's tolerance a twentieth of a percent larger) gives another result, and moves the count against the oracle from 51 to 48 of 60. Six such twins of 0.4.4 put numbers on it: of any two runs of one model, 94% differ, 30% differ by more than 0.05 in the scaled Jacobian of the worst twentieth of the quads, and 45% by that or by a tenth of a percent of the diagonal in the largest distance from the input.

So a result is one draw from a spread, and the spread is wider than any rule added since 0.4.1. Two ways to gain from that were written down: make the fragile steps stable, one case at a time, or draw several times and keep the best. This is the second.

## What others do

Read before designing (ideas only, no code taken):

- **Blender's own QuadriFlow remesher has a Seed setting**, and its [manual](https://docs.blender.org/manual/en/dev/modeling/meshes/retopology.html) says why: "different seeds will cause the remesher to generate different quad layouts on the mesh". The user tries seeds by hand and judges by eye.
- **Chip layout tools sweep seeds.** Place-and-route is a heuristic with the same character, and the vendors' tools run it under several seeds and keep the best pass: [Intel's Design Space Explorer](https://www.intel.com/content/www/us/en/docs/programmable/836192/current/run-design-space-explorer-ii.html) and its [fitter seed notes](https://www.intel.com/content/www/us/en/docs/programmable/683641/21-3/fitter-seed.html) (a seed changes the result by about ±5%, and no seed is best for every design), and [Microchip's Multiple Pass Layout](https://onlinedocs.microchip.com/oxy/GUID-6BACC19D-0959-4B51-87AC-AD97B0E07E35-en-US-1/GUID-F2324A8F-DE5C-46D4-8AD4-F16D84A51956.html) (five passes recommended, the best picked by a stated criterion, an option to stop at the first pass with no violations). Intel adds the warning decision 0026 arrived at on its own: judge a change over several seeds, not by one.
- **Randomised restarts.** [Gomes, Selman and Kautz 1998](https://www.cs.cornell.edu/gomes/pdf/1998_gomes_aaai_iaai_boosting.pdf) randomise a deterministic search where it breaks ties by a fixed rule ("select the lowest-numbered variable"), widen the ties with a "heuristic equivalence" parameter (choices within H percent of the best count as equally good), and restart. Their point is the tail: a search whose cost depends on its early choices has a heavy tail of bad runs, and restarts cut it off ([Gomes et al. 2000](https://www.cs.cornell.edu/gomes/pdf/2000_gomes_jar_phenomena.pdf)).
- **GRASP** ([Feo and Resende 1995](https://link.springer.com/doi/10.1007/BF01096763)): a greedy construction randomised among its near-best candidates, a local search after it, many starts, the best kept. This pipeline has that shape already: a greedy layout, then a relaxer.
- **How much the best of N is worth.** The expected best of N draws from a normal spread is 0.56, 0.85, 1.03, 1.16 and 1.27 standard deviations above the mean for N = 2, 3, 4, 5 and 6 ([table](https://faculty.washington.edu/heagerty/Books/Biostatistics/TABLES/NormalOrder/index.html)): most of the gain is in the first three or four.
- **Racing** (successive halving, [Li et al.](https://arxiv.org/pdf/1810.05934v3)): rate many candidates cheaply and spend the full cost on the promising few. Looked at for pruning layouts before their quads are built (below).

Taken: the seed where the greedy rule breaks ties, the equivalence band, a stated criterion for "best", and the count of attempts as the one setting. Added to them: the choice is made per connected part and not per result, and a part whose layout does not change with the seed is not built again.

## The design

### A seed where the layout breaks ties (`trace/select.rs`)

The layout is greedy, and where two choices are as good as each other a fixed rule picks one. Three such places take a seed:

| Where | Fixed rule (seed 0) | With a seed |
|---|---|---|
| The loop seeds (`sample::farthest_points`) | start from the lowest eligible vertex | start from another one |
| Joining concave and narrow vertices (`join_needs`) | cheapest path first, the lower id on a tie | paths within a tenth of each other in cost count as tied, and the seed orders them |
| Loops and border paths (`choose_farthest`) | farthest from the paths so far first | the same band on the distance |

Seed 0 is the layout as it always was, bit for bit. Any other seed gives a layout the same rounds rate as highly: nothing is relaxed, only the order among near-equals changes. A seed is not a random draw: the same seed gives the same layout on every run, so results stay repeatable.

Two things were measured about it, each over the 70 models:

- **A seed in the layout alone gives nearly the whole spread.** Five seeds against six of 0.4.4's noise twins (which change the prep mesh, the field and everything after): the spread between draws is 85 to 90% as wide (a model's worst quads vary by 0.023 against 0.026 at the median) and the best of N gains as much. So an attempt need not repeat the prep remesh or the field, which are a quarter of a run.
- **The width of the band hardly matters.** At 5%, 10% and 20% the best of three gains 0.0070, 0.0064 and 0.0070 in mean scaled Jacobian and the other measures move as little. It is 10%.

### Each part keeps its own best (`attempts.rs`)

A model of thirty parts has an unlucky part in every draw, so picking one whole result gains little on it. The pick is per **island**: a connected part, or several that touch at a vertex (their quads share that vertex, so they stay together).

1. The first attempt is the run as it was: the unseeded layout and its quads.
2. Every other attempt lays all parts out with its seed, side by side. Laying out is 7% of a run.
3. **A layout an earlier attempt already made is dropped** (`trace::same_part`: the same triangles in the same patches with the same corners). A box, a flat panel or a tube has no near-ties to break and comes out the same from every seed, so it costs its tracing and nothing more. Over the 70 models, 73% of an island's later attempts are new layouts where any is; islands with none are not tried again at all.
4. Each attempt builds the quads of its new layouts only (`trace::compose` makes the layout of just those parts), a few attempts at a time.
5. Each island's quads are cut out of each attempt's mesh and scored against that island's part of the prep mesh.
6. Each island takes the attempt that scored lowest, the first on a tie. If none beat the first anywhere, the first result is returned untouched; otherwise the winners' layouts and quads are joined into one layout and one mesh, with the patch numbers the layout's report, the quads' `patch_of_face` and the per-patch notes all agree on.

One attempt (`quad.attempts = 1`) is byte-identical to 0.4.4. A later attempt that fails is one draw fewer, never a failed run; only a cancel ends it.

### The score

Less is better; the sum of five measures of an island's quads, each in the unit of a difference worth having:

| Measure | Unit |
|---|---|
| Mean scaled Jacobian, short of 1 | 0.01 |
| The scaled Jacobian of the worst twentieth, short of 1 | 0.05 |
| The worst quad's, short of 1 | 0.2 |
| Irregular vertices per thousand quads | 5 |
| The farthest the quads and the surface lie apart, either way, in quad sizes | 0.1 |

Found on the way:

- **It has to hold both shape and distance.** Picked by shape alone, the best of several results sits no better on its surface than one result does; picked by distance alone, its shape is worse than one result's. With both, every measure gains.
- **The weights matter less than that.** Fifteen weightings were scored on the logged attempts of all 70 models. Every one that holds all five measures gains on all five, and they differ by less than the models do. Leave distance out and the largest distance stops gaining; leave the worst quad out and more models lose on it than gain.
- **Irregular vertices weigh least.** They follow shape anyway (picking by shape alone already removes two thirds of what picking for them removes). At two per thousand they made one bad call that showed the principle: a part whose patches were kept as triangles (hundreds of irregular vertices, passable shape) was rated worse than the same part with its quads sheared.
- Distance is in quad sizes, measured on the island, so a small part is judged at its own scale and not the model's.

### Target Count, Symmetry, Quick Re-run

- **Target Count.** Each later attempt is asked for as many quads as the first attempt gave the parts it tries, and if the winners together miss the number asked for, the first result stands.
- **Symmetry.** The attempts work on the kept side; the mirror is made from the winners.
- **Quick Re-run.** The attempts are part of the quad stage (`pipeline::finish`), so changing Layout Attempts reruns only that.

### Progress

The attempts run stages a second and third time, side by side, and each would report its own stage from zero. `Control::hold` hands progress to one reporter for the duration: the panel shows "Layout Attempts… 40%" and nothing jumps back. The stage is `Stage::Attempts`, with its own line in the timings.

### The setting

**Layout Attempts**, 1 to 8, in the main panel under Adaptive Detail. It is not part of the presets: how long to wait is the user's choice and a preset does not reset it. The result line says what happened: "Best of 4 Layouts: 5 of 12 Parts Took Another". Add-on API 12 (the setting and three report fields).

### The default is four

The best of N draws grows as order statistics say, and the logged attempts of the 70 models agree with the table: of what six attempts gain for an island, two keep 46%, three 67% and four 82%.

The later attempts run side by side, so time does not grow with their number until they no longer fit in one round (four at a time, which bounds the memory: each attempt holds a layout and a quad mesh of the parts it tries). Ten models timed one at a time, the better of two passes:

| Attempts | 1 | 3 | 4 |
|---|---|---|---|
| Ten models | 128.5 s | 191.5 s | 193.2 s |
| Against one | | +49% | +50% |

Three and four cost the same, so the default is four. (Six took 76% longer than one in an earlier single pass on a busier machine.) What a model pays depends on its parts: one that is a single part with many near-ties pays most (the pig, 4.4 s to 13.4 s), a hard-surface model of many simple parts little (CommercialRefrigerator, 35.1 s to 37.3 s). That table was taken with every attempt at the full solver effort; the next section is what was done about the pig.

### A cutoff for the later attempts

A run waits for its slowest attempt, and the count solver's time depends on the layout far more than on the model. One layout of the bunny is solved in 1.2 s and another takes 7 s; the pig's third layout met a clash and was solved twice. Where the search runs to its node limit it costs seconds, and which layout does that is a draw of its own.

Restarts come with a cutoff for this (Gomes, Selman and Kautz: stop a run that is taking long, start the next). Here: **the later attempts get a quarter of the solver effort** (`attempts::LATER_EFFORT`; the effort is a multiple of the count solver's node limits, so the result is still the same on every run). The first attempt keeps all of it, so one attempt is the result it always was, and a later attempt whose search is cut short is rated on what it has.

| Later attempts' effort | Better than the oracle by mean scaled Jacobian / 5th percentile / Hausdorff, of 60 | Four attempts against one, ten models | The pig | The armadillo |
|---|---|---|---|---|
| 1 | 54 / 53 / 52 | +57% | 5.4 s to 18.3 s | 19.1 s to 35.2 s |
| 0.5 | | +48% | to 12.7 s | to 29.4 s |
| 0.25 | 55 / 53 / 54 | +44% | to 11.0 s | to 27.2 s |

(One sitting, one pass each, on a busier machine than the table above: compare the rows, not the tables.) The quality is the same within what a run varies by: against the full effort, 55 of 70 models are identical in mean scaled Jacobian, 5 better and 10 worse, with eleven more irregular vertices a model. A model of many small parts gains nothing from the command line, since its solves are short either way (ToyCar, BrainStem).

What found it was the benchmark through the add-on, again. With every attempt at the full effort that run took 1,338 s, more than twice 0.4.4's 630 s, and eight models one at a time took 87% longer than on 0.4.4 (the bunny 2.9 s to 11.1 s), where the ten command-line models above had said half as long again. The add-on's input is another draw (32-bit positions), and on it one of the bunny's later layouts ran its count solve to the node limit. With the cutoff the same run takes 964 s and the eight models 46% longer.

## What it measured

### Before it was built: how much there is to gain

Six noise twins of 0.4.4 over the 70 models (`quad_eval.py` (a development tool, not in the public repository) with the remesh tolerance times 1 + 0.0005 k), and what picking the best whole result of N of them would give, over every choice of N of the six:

| N | Better than the oracle by mean scaled Jacobian, of 60 | by its 5th percentile | by Hausdorff | Results over 1% from the input |
|---|---|---|---|---|
| 1 | 50.3 | 48.0 | 47.2 | 1.3 |
| 2 | 52.6 | 50.1 | 48.3 | 0.6 |
| 3 | 53.2 | 50.6 | 48.7 | 0.2 |
| 4 | 53.6 | 50.7 | 49.1 | 0.1 |
| 6 | 54.0 | 51.0 | 49.0 | 0 |

The gain flattens as the order statistics say it should. That was for whole results, which is the least a pick can do; the per-part pick below does better on models of many parts.

### After: 0.4.4 against 0.4.5, as pairs

Default settings, all 70 models, each build run as built and with its noise twin (`quad_pairs.py` (a development tool, not in the public repository)). A model counts only when both new runs beat (or lose to) both old ones, which happens for about 12 of 70 either way by chance:

| Measure | Clearly better | Clearly worse | Median, 0.4.4 to 0.4.5 |
|---|---|---|---|
| Mean scaled Jacobian | 29 | 3 | 0.941 to 0.954 |
| Scaled Jacobian, 5th percentile | 23 | 2 | 0.755 to 0.794 |
| Largest distance from the input, % of the diagonal | 24 | 1 | 0.355 to 0.315 |
| Mean distance from the input | 11 | 11 | 0.0167 to 0.0162 |

The "worse" column is as telling as the "better": a dozen models should lose both ways by chance, and one to three do. (With every attempt at the full solver effort, before the cutoff above, the same table read 29 and 3, 25 and 1, 23 and 1.)

| | 0.4.4 | its twin | 0.4.5 | its twin |
|---|---|---|---|---|
| Better than the oracle by mean scaled Jacobian, of 60 | 49 | 50 | 55 | 51 |
| by its 5th percentile | 47 | 47 | 53 | 51 |
| by Hausdorff | 48 | 46 | 54 | 51 |
| Results with an inverted quad, of 70 | 44 | 44 | 41 | 39 |
| Results over 1% from the input | 1 | 1 | 0 | 0 |
| Irregular vertices, all models | 70,530 | 71,798 | 67,747 | 67,266 |
| Quads, all models | 1,821,808 | 1,816,337 | 1,833,190 | 1,838,736 |

**The bad draws are what goes.** Between a run and its twin, the median difference in a model's numbers is what it was (0.003 in mean scaled Jacobian, 0.02 in the 5th percentile, 0.03 points of distance): the twin still changes the prep mesh, and each run picks among its own four layouts. The large differences shrink: the mean difference in the 5th percentile goes from 0.052 to 0.031 and the largest from 0.54 to 0.20; in distance the largest goes from 0.49 to 0.34 points. This is what restarts do to a heavy tail.

Models, as run / twin:

- `thingi10k/94990`: 5th percentile 0.318 / 0.853 to 0.892 / 0.889 (the half-disc patch that flipped on a two-triangle difference; one of its four layouts does not have it).
- ABeautifulGame: 0.353 / 0.354 to 0.647 / 0.631. ToyCar: 0.321 / 0.138 to 0.421 / 0.471. DamagedHelmet: 0.447 / 0.439 to 0.694 / 0.654. These are models of many parts, each of which now has its own best draw.
- `thingi10k/650187`: mean 0.919 to 0.976, 5th percentile 0.75 to 0.95.
- `thingi10k/1706457`: largest distance 0.65% / 1.10% to 0.60% / 0.62%. The pig: 0.41% / 0.61% to 0.36% / 0.37%.
- Worse on both runs, each by little: `thingi10k/40994` (mean 0.961 / 0.962 to 0.958 / 0.960), the ogre (0.944 / 0.942 to 0.941 / 0.929), DragonAttenuation (0.775 / 0.780 to 0.770 / 0.762). The score trades one measure for another, and on these the trade went against the mean.

**The number of quads is not in the score**, and it varies between layouts of the same part at the same size: counts are whole numbers, and a tube takes six quads round in one layout and seven in the next. Over the 70 models there are 0.9% more quads. Single models move both ways: `thingi10k/271869` 28% fewer, the oloid 23% fewer, `thingi10k/94990` 22% fewer, `thingi10k/650187` (a thin coil: seven round where the first layout had six) 39% more, the rest within 9%.

### Through the add-on

One run of the 70 with the production zip ([report](../../benchmark/reports/20261127-q045-default/README.md)): 70 of 70; against the oracle, mean scaled Jacobian better on 53 of 60, its 5th percentile on 50, Hausdorff on 50 (0.4.4: 49, 47 and 46). Against 0.4.4's run model by model: mean scaled Jacobian better on 51 and worse on 3, Hausdorff 31 and 12. One result over 1% from its input, the same one at the same figure as in 0.4.4 (`common3d/suzanne`, 1.49%).

Tool time is 964 s against 630 s (+53%; this run with two Blender processes at once, 0.4.4's with three). Eight models one at a time: 205 s to 299 s (+46%), from 1.26 times (the armadillo) to 2.11 times (the bunny).

## What it does not do

- **Attempts differ in the layout and in nothing before it.** A defect that is already in the prep mesh is in every attempt. 0.4.4's six noise twins, which do change the prep mesh, put the pig between 0.4% and 5.8% of the diagonal off its input (its tail) and `thingi10k/1706457` over 1% in three of six; over five layout seeds on one prep mesh no model went past 1.2%. Those are the prep remesh's own unlucky draws, and no layout seed reaches them.
- **A part whose layout has no near-ties is not tried again**, and its quads can still be an unlucky draw of the count solver. SunglassesKhronos' large frame is the case: the same layout from every seed, and quads that differ with what else is solved beside it. A seed in the count solver would reach those parts.
- **The pick sees what the score sees:** shape, irregular vertices and distance. It does not see edge flow (how the quads follow the features and the curvature), so it cannot pick for it.
- **Inverted quads mostly survive.** Of the islands tried again, 184 hold an inverted quad after one attempt and 159 after three: most come from something every layout shares.
- **A layout cannot be rated before its quads are built.** Tried for pruning (trace eight, build the promising three): six features of a layout (patches, patches not Ok, not four-sided, crowded, short of CC-ability, path length) each correlate with the score its quads get at 0.03 to 0.17 by rank, and picking which layouts to build by them keeps exactly as much of the gain as taking the same number in order (46%, 68%, 82% of the best of six for 2, 3 and 4 built). So every new layout is built.
- **An attempt costs a quad stage, and the quad stage is mostly the count solver** (95% of it on the pig). The attempts run side by side, so a run takes as long as its slowest attempt; the cutoff bounds the search, and a layout that needs a second count solve (a clash) still pays twice. A run is about half as long again as with one attempt, and up to twice as long for a model whose later layouts are hard to count. The count solver's cost was an open item; it now counts once per attempt.
