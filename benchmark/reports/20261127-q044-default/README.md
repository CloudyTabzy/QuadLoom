# 0.4.4 through the add-on, against QRemeshify 1.5.0

Run 2026-11-27: `dist/releases/quadloom-0.4.4.zip` (production) through the add-on on all 70 models at the default settings, three Blender processes at once. The oracle numbers are the 2026-10-21 baseline; the 0.4.2 column is `../20261127-q042-default/` (0.4.3 changed no default result but one).

```
python benchmark/run_benchmark.py --tool quadloom --zip dist/releases/quadloom-0.4.4.zip --run-id 20261127-q044-default --jobs 3 --timeout 900
python benchmark/report.py 20261127-q044-default --tool quadloom
python benchmark/compare_quads.py 20261127-q044-default 20261021-oracle-baseline
```

## Result

**70 of 70 completed** (the oracle: 60). Over the 60 models both finished:

| Metric | QuadLoom 0.4.4 | Oracle | Better on | 0.4.2 was better on |
|---|---|---|---|---|
| Scaled Jacobian, mean | 0.956 | 0.922 | 49 of 60 | 51 |
| Scaled Jacobian, 5th percentile | 0.812 | 0.650 | 47 of 60 | 45 |
| Irregular vertices | 114 | 248 | 45 of 60 | 44 |
| Mean distance, % of the diagonal | 0.0169 | 0.0367 | 58 of 60 | 57 |
| Hausdorff, % of the diagonal | 0.324 | 0.491 | 46 of 60 | 42 |
| Tool time, s | 2.7 | 13.0 | 60 of 60 | 60 |

- Further than 1% from the input: 1 result, `common3d/suzanne` at 1.49% (0.4.2: 2; the oracle: 10). None over 2%.
- With an inverted or degenerate quad: 43 (0.4.2: 44; the oracle: 52).
- Over 100,000 quads: 3 results, as in 0.4.2. Quads over all 70 models: 1.81 million against 1.82 million.
- Tool time over all 70: 630 s against 690 s.

## How much of that is the version

**Read these counts with decision 0026 in hand: a single run moves by this much on its own.** 0.4.3 run twice from the command line, the second time with a tolerance changed by 0.05%, is better than the oracle on 51 and then 48 of 60 by mean scaled Jacobian, and on 45 and then 46 by Hausdorff. This run and 0.4.2's are one run each.

What holds beyond that, from the paired command-line runs (decision 0026) and from looking at the models:

- `thingi10k/906891` keeps its pin holes: 1.24% off its input to 0.37%.
- Model by model against 0.4.2, the Hausdorff distance is better on 37 and worse on 21 (median over the 70: 0.372% to 0.328%); quad shape is even (mean scaled Jacobian better on 31, worse on 28).
- `khronos/AntiqueCamera`: its worst quads from 0.03 to 0.33 at the 5th percentile. A thin disc that 0.4.2 left as patches kept as triangles gets patterns.
- `khronos/SunglassesKhronos`: the other way, 0.49 to 0.25. Its lens frames are bars thinner than a quad, laid out as strips one quad wide; in this run more of those strips are sheared and kept as triangles. It is the open "sheared strips" item, and which run gets it is chance.

## Found by this run

The first pass of this benchmark had AntiqueCamera at 270 s. The cause was a search in the relaxer with no bound, fixed before this run (decision 0026); the same model now takes 25 s here, and the run as a whole is 60 s shorter than 0.4.2's.

Files: [`quads_vs_oracle.md`](quads_vs_oracle.md) (every model), [`report.md`](report.md), `results.json`.
