# 0.4.0 target-count measurements

Windows / Blender 5.2.2, native API 9. Existing benchmark meshes stay untracked.

**Density:** geometry and feature-edge bytes are identical to 0.3.0 on all six models.

**Accuracy:** 13/18 requests met max(4 quads, 5%); median error 2.8%. All nine synthetic requests and all six 8,000 requests met tolerance. Five coarse real-asset requests missed; the worst was 500 requested / 1,660 achieved (232%). Misses produce panel warnings. This is approximate count control, not an exact budget guarantee.

| Model | Requested | Achieved | Error | p5 scaled Jacobian | Inverted/concave | Seconds |
|---|---:|---:|---:|---:|---:|---:|
| cube | 500 | 486 | 2.80% | 1.000 | 0 | 0.02 |
| cube | 2,000 | 1,944 | 2.80% | 1.000 | 0 | 0.03 |
| cube | 8,000 | 7,776 | 2.80% | 1.000 | 0 | 0.04 |
| torus | 500 | 492 | 1.60% | 0.993 | 0 | 0.04 |
| torus | 2,000 | 1,968 | 1.60% | 0.986 | 0 | 0.04 |
| torus | 8,000 | 8,036 | 0.45% | 0.978 | 0 | 0.05 |
| plane | 500 | 512 | 2.40% | 1.000 | 0 | 0.89 |
| plane | 2,000 | 1,953 | 2.35% | 1.000 | 0 | 0.92 |
| plane | 8,000 | 8,001 | 0.01% | 1.000 | 0 | 0.87 |
| fandisk | 500 | 1,459 | 191.80% | 0.000 | 57 | 0.17 |
| fandisk | 2,000 | 2,205 | 10.25% | 0.146 | 55 | 0.58 |
| fandisk | 8,000 | 8,322 | 4.03% | 0.571 | 2 | 0.11 |
| stanford-bunny | 500 | 1,178 | 135.60% | 0.002 | 52 | 36.71 |
| stanford-bunny | 2,000 | 1,560 | 22.00% | 0.099 | 47 | 39.91 |
| stanford-bunny | 8,000 | 8,259 | 3.24% | 0.517 | 38 | 6.38 |
| armadillo | 500 | 1,660 | 232.00% | -0.002 | 95 | 83.05 |
| armadillo | 2,000 | 1,901 | 4.95% | 0.004 | 87 | 82.57 |
| armadillo | 8,000 | 7,979 | 0.26% | 0.503 | 48 | 131.03 |

All measured meshes were finite, all-quad, consistently oriented topologically, and had zero non-manifold edges. Those checks do **not** establish good geometric element quality: coarse real outputs include concave/inverted quads and near-zero Jacobians, as the table shows. The original Density baseline also has 45 such quads on the bunny and 26 on the armadillo.

Fixed fallback patches, existing layout complexity, pattern-class constraints and discontinuous integer quantization limit coarse targets. For example, the bunny requested at 2,000 produces 1,560; fandisk at 2,000 produces 2,205. The retained plan respects features, fixed counts and size floors rather than forcing these numbers with a geometry-changing post-process. Further low-count layout/quantization work remains open.

Comparisons at the selected edge length are in the JSON. Most representable scalar cases match Density exactly. Local count refinement intentionally changes some outputs. On armadillo at 2,000, reconstructing the Density edge via edge_scale changes the last floating-point bit (57.682229421970106 versus 57.68222942197011), selecting 5,677 instead of 1,901 quads; that pair is not a matched-count quality comparison.

Timings are single-pass measurements under concurrent release validation. Difficult adaptive organic targets can be slow (up to 131 s here); they are not evidence that speed is unchanged. The usual cache avoids repeating upstream stages; solver/pattern search still adds work. No controlled worker-overhead or cross-version speed claim is made.

Primary-source research and the design rationale are linked in [decision 0022](../../docs/decisions/0022-target-quad-count.md). Machine-readable measurements: [JSON](2026-10-24-target-count.json).
