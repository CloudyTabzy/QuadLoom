# 0.4.5 through the add-on, against QRemeshify 1.5.0

Run 2026-11-27: `dist/releases/quadloom-0.4.5.zip` (production) through the add-on on all 70 models at the default settings, which now include Layout Attempts 4. Two Blender processes at once. The oracle numbers are the 2026-10-21 baseline; the 0.4.4 column is `../20261127-q044-default/`.

```
python benchmark/run_benchmark.py --tool quadloom --zip dist/quadloom-0.4.5.zip --run-id 20261127-q045-default --jobs 2
python benchmark/report.py 20261127-q045-default --tool quadloom
python benchmark/compare_quads.py 20261127-q045-default 20261021-oracle-baseline
```

## Result

**70 of 70 completed** (the oracle: 60). Over the 60 models both finished:

| Metric | QuadLoom 0.4.5 | Oracle | Better on | 0.4.4 was better on |
|---|---|---|---|---|
| Scaled Jacobian, mean | 0.959 | 0.922 | 53 of 60 | 49 |
| Scaled Jacobian, 5th percentile | 0.818 | 0.650 | 50 of 60 | 47 |
| Irregular vertices | 116 | 248 | 46 of 60 | 45 |
| Mean distance, % of the diagonal | 0.0174 | 0.0367 | 58 of 60 | 58 |
| Hausdorff, % of the diagonal | 0.292 | 0.491 | 50 of 60 | 46 |
| Tool time, s | 4.1 | 13.0 | 58 of 60 | 60 |

- Further than 1% from the input: 1 result, `common3d/suzanne` at 1.49%, as in 0.4.4 (the oracle: 10). The same figure in both versions: four layouts did not move it.
- With an inverted or degenerate quad: 40 (0.4.4: 43; the oracle: 52).
- Over 100,000 quads: 3 results, as in 0.4.4. Quads over all 70 models: 1.83 million against 1.81 million.

## Against 0.4.4, model by model

One run each, so read it with the paired runs of decision 0027 (which say the same): mean scaled Jacobian better on 51 models and worse on 3, its 5th percentile 43 and 8, Hausdorff 31 and 12. Medians over the 70: mean scaled Jacobian 0.953 to 0.957, 5th percentile 0.768 to 0.800, Hausdorff 0.328% to 0.299%.

| Model | 5th-percentile scaled Jacobian | Hausdorff | Irregular vertices |
|---|---|---|---|
| `common3d/igea` | 0.694 to 0.800 | 0.51% to 0.42% | 120 to 125 |
| `khronos/DamagedHelmet` | 0.701 to 0.737 | 0.67% to 0.37% | 982 to 743 |
| `khronos/ToyCar` | 0.448 to 0.516 | 0.49% to 0.43% | 3,511 to 2,442 |
| `khronos/ABeautifulGame` | 0.614 to 0.659 | 0.39% to 0.39% | 3,833 to 3,269 |
| `khronos/AntiqueCamera` | 0.329 to 0.382 | 0.24% to 0.22% | 3,255 to 3,072 |
| `crane/pig` | 0.648 to 0.699 | 0.46% to 0.43% | 162 to 166 |
| `thingi10k/94990` | 0.856 to 0.885 | 0.54% to 0.38% | 64 to 62 |
| `khronos/SunglassesKhronos` | 0.247 to 0.252 | 0.14% to 0.15% | 359 to 357 |

SunglassesKhronos is the model the attempts do not reach: its lens frames get the same layout from every seed, so they are not tried again, and their sheared strips stay (open items).

## Time

Tool time over all 70: **964 s against 0.4.4's 630 s (+53%)**, with a median model at 4.3 s against 3.1 s. Model by model the ratio has a median of 1.49 (quartiles 1.30 and 1.76, largest 2.25). The two runs are not like for like: this one had two Blender processes at once and 0.4.4's had three, and the attempts run side by side, so they need the cores.

Eight models one at a time, through the add-on, both versions:

| Model | 0.4.4 | 0.4.5 | |
|---|---|---|---|
| armadillo | 19.5 s | 24.7 s | 1.26× |
| stanford-bunny | 2.9 s | 6.1 s | 2.11× |
| pig | 5.3 s | 9.8 s | 1.86× |
| ABeautifulGame | 28.0 s | 36.6 s | 1.31× |
| AntiqueCamera | 21.6 s | 31.4 s | 1.45× |
| ChronographWatch | 42.1 s | 78.8 s | 1.87× |
| `thingi10k/1514900` | 53.3 s | 69.0 s | 1.29× |
| `thingi10k/271869` | 32.0 s | 42.1 s | 1.31× |
| Total | 204.7 s | 298.5 s | +46% |

## Found by this run

The first pass of this benchmark was taken with every attempt at the full solver effort and came to 1,338 s, more than twice 0.4.4's time, where ten models from the command line had said half as long again. The same eight models one at a time took 382 s (+87%), the bunny 11.1 s where the command line had 4.2 s. The cause was not the add-on: the add-on's input is another draw (32-bit positions), and on that draw one of the bunny's later layouts met a count solve that ran to its node limit, 7 s where the first layout took 1.2 s. A run waits for its slowest attempt, and the ten-model timing had happened to hold few slow ones.

Hence the cutoff in decision 0027 (the later attempts get a quarter of the solver effort), measured before this run: the same quality, 964 s in place of 1,338 s here, and +46% in place of +87% on the eight models.

Files: [`quads_vs_oracle.md`](quads_vs_oracle.md) (every model), [`report.md`](report.md), `results.json`.
