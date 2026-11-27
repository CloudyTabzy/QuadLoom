# 0.4.6 through the add-on, against QRemeshify 1.5.0

Run 2026-11-27: `dist/releases/quadloom-0.4.6.zip` (production) through the add-on on all 70 models at the default settings (Layout Attempts 4, Symmetry off). Two Blender processes at once. The oracle numbers are the 2026-10-21 baseline; the 0.4.5 column is `../20261127-q045-default/`.

```
python benchmark/run_benchmark.py --tool quadloom --zip dist/releases/quadloom-0.4.6.zip --run-id 20261127-q046-default --jobs 2
python benchmark/report.py 20261127-q046-default --tool quadloom
python benchmark/compare_quads.py 20261127-q046-default 20261021-oracle-baseline
```

## Result

**70 of 70 completed** (the oracle: 60). Over the 60 models both finished:

| Metric | QuadLoom 0.4.6 | Oracle | Better on | 0.4.5 was better on |
|---|---|---|---|---|
| Scaled Jacobian, mean | 0.961 | 0.922 | 53 of 60 | 53 |
| Scaled Jacobian, 5th percentile | 0.828 | 0.650 | 50 of 60 | 50 |
| Irregular vertices | 117 | 248 | 46 of 60 | 46 |
| Mean distance, % of the diagonal | 0.0170 | 0.0367 | 58 of 60 | 58 |
| Hausdorff, % of the diagonal | 0.290 | 0.491 | 52 of 60 | 50 |
| Tool time, s | 3.7 | 13.0 | 60 of 60 | 58 |

- Further than 1% from the input: 1 result, `common3d/suzanne` at 1.49%, as in 0.4.5 and 0.4.4 (the oracle: 10).
- With an inverted or degenerate quad: 41 (0.4.5: 40; the oracle: 52).
- Over 100,000 quads: 3 results, as in 0.4.5. Quads over all 70 models: 1.87 million against 1.83 million.

## Time

**683 s for the 70 models, against 964 s in 0.4.5** (the add-on's own time, summed): 29% less.

It is a dozen heavy models that pay for it, not every model: CommercialRefrigerator 73 s to 11 s, ChronographWatch 136 s to 80 s, AntiqueCamera 45 s to 15 s, `thingi10k/1514900` 73 s to 48 s, DragonAttenuation 36 s to 15 s, ABeautifulGame 49 s to 38 s, the oloid 25 s to 15 s, the armadillo 28 s to 19 s. The median model takes 0.97 times what it did. With two Blender processes at once a small model's time moves by a second or two with what runs beside it, so the single-model figures at the small end mean little (the Avocado reads 1.1 s and 3.7 s; alone, through the command line, 1.2 s and 1.3 s).

## Against 0.4.5, model by model

One run each, so read it with the paired runs of decision 0028 (which say the same: within chance). Mean scaled Jacobian better on 35 models and worse on 24 (by more than 0.004: 9 and 5), its 5th percentile 34 and 24 (by more than 0.02: 8 and 7), Hausdorff 21 and 26 (by more than 0.03 points: 8 and 7). Medians over the 70: mean scaled Jacobian 0.957 to 0.959, 5th percentile 0.800 to 0.813, Hausdorff 0.299% to 0.293%. Eleven results are 0.4.5's to the last quad.

## What it does not show

Symmetry is off in this run, as it is by default. Decision 0028 has the 34 symmetric models with Auto Detect on against off.
