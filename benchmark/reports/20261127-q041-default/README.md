# 0.4.1 through the add-on, against QRemeshify 1.5.0

Run 2026-11-27: `dist/releases/quadloom-0.4.1.zip` (production) through the add-on on all 70 models at the default settings, three Blender processes at once. The oracle numbers are the 2026-10-21 baseline.

```
python benchmark/run_benchmark.py --tool quadloom --zip dist/releases/quadloom-0.4.1.zip --run-id 20261127-q041-default --jobs 3 --timeout 900
python benchmark/report.py 20261127-q041-default --tool quadloom
python benchmark/compare_quads.py 20261127-q041-default 20261021-oracle-baseline
```

## Result

- **70 of 70 completed** (the oracle: 60). No run failed on `progress.json`: released 0.4.0 lost about one in six to that race on a run of this length.
- Over the 60 models both finished, QuadLoom is better on:

| Metric | QuadLoom | Oracle | Better on | 0.4.0 was better on |
|---|---|---|---|---|
| Scaled Jacobian, mean | 0.950 | 0.922 | 45 of 60 | 21 |
| Scaled Jacobian, 5th percentile | 0.759 | 0.650 | 42 of 60 | |
| Irregular vertices | 125 | 248 | 42 of 60 | |
| Hausdorff, % of the diagonal | 0.328 | 0.491 | 45 of 60 | 17 |
| Mean distance, % of the diagonal | 0.0195 | 0.0367 | 55 of 60 | |
| Tool time, s | 3.1 | 13.0 | 60 of 60 | |

- Further than 1% from the input: 3 results (the oracle: 10). With an inverted or degenerate quad: 47 (the oracle: 52).
- By source, the mean scaled Jacobian is better on 12 of 12 common models, 6 of 7 of Crane's, 14 of 19 Khronos assets and 13 of 22 Thingi10k parts.

## Where the oracle is still ahead

Mechanical parts and game assets where features are denser than the quads (`thingi10k/271869` 0.876 against 0.920, `thingi10k/94990` 0.897 against 0.951, WaterBottle 0.930 against 0.959), and thin steps at the default size (`thingi10k/75117`: 0.99% from the input against 0.14%). Decision 0023 lists what is left.

Files: [`quads_vs_oracle.md`](quads_vs_oracle.md) (every model), [`report.md`](report.md), `results.json`.
