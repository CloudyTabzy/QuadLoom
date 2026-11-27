# 0.4.2 through the add-on, against QRemeshify 1.5.0

Run 2026-11-27: `dist/releases/quadloom-0.4.2.zip` (production) through the add-on on all 70 models at the default settings, three Blender processes at once. The oracle numbers are the 2026-10-21 baseline; the 0.4.1 column is `../20261127-q041-default/`.

```
python benchmark/run_benchmark.py --tool quadloom --zip dist/releases/quadloom-0.4.2.zip --run-id 20261127-q042-default --jobs 3 --timeout 900
python benchmark/report.py 20261127-q042-default --tool quadloom
python benchmark/compare_quads.py 20261127-q042-default 20261021-oracle-baseline
```

## Result

**70 of 70 completed** (the oracle: 60). Over the 60 models both finished:

| Metric | QuadLoom 0.4.2 | Oracle | Better on | 0.4.1 was better on |
|---|---|---|---|---|
| Scaled Jacobian, mean | 0.955 | 0.922 | 51 of 60 | 45 |
| Scaled Jacobian, 5th percentile | 0.781 | 0.650 | 45 of 60 | 42 |
| Irregular vertices | 108 | 248 | 44 of 60 | 42 |
| Mean distance, % of the diagonal | 0.0172 | 0.0367 | 57 of 60 | 55 |
| Hausdorff, % of the diagonal | 0.365 | 0.491 | 42 of 60 | 45 |
| Tool time, s | 2.9 | 13.0 | 60 of 60 | 60 |

- Further than 1% from the input: 2 results (0.4.1: 3; the oracle: 10). None over 2%.
- With an inverted or degenerate quad: 44 (0.4.1: 47; the oracle: 52).
- Over 100,000 quads: 3 results (0.4.1: 11). Quads over all 70 models: 1.82 million against 2.52 million.

## Against 0.4.1, model by model (70)

Mean scaled Jacobian better on 53, its 5th percentile on 53, mean distance on 40. **The Hausdorff distance is better on 29 and worse on 41**, by little (median over the 60: 0.328% to 0.365%): small quads now hold their shape and the larger quads beside them give, and several models have fewer quads than before.

## Where it is still behind

`common3d/suzanne` (1.48% from its input) and `thingi10k/906891` (1.24%). `thingi10k/1514900` and `650187` still get about 128,000 quads as thin closed parts. Decision 0024 lists what is left.

Files: [`quads_vs_oracle.md`](quads_vs_oracle.md) (every model), [`report.md`](report.md), `results.json`.
