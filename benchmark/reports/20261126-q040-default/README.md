# QuadLoom 0.4.0 against QRemeshify 1.5.0: quad output on the 70-model set

2026-11-26, Windows / Blender 5.2. The released 0.4.0 extension zip, run through
the add-on at its defaults (Balanced preset, Density 1), one Blender process per
mesh. The oracle numbers are the `20261021-oracle-baseline` run of QRemeshify
1.5.0 at its defaults. Both sides were measured by `quadloom metrics` against
the same input. This is the first comparison of the two tools' finished quads.

| File | What |
|---|---|
| `quads_vs_oracle.md` | Counts, medians, wins per mesh source and one row per case (`benchmark/compare_quads.py`) |
| `report.md`, `results.json` | Every metric per case (`benchmark/report.py`) |

## Result

| | QuadLoom 0.4.0 | QRemeshify 1.5.0 |
|---|---|---|
| Completed | 70 of 70 (see "Reliability") | 60 of 70 (4 errors, 6 timeouts at 900 s) |
| Median time | 3.0 s | 13.0 s |
| Median quads | 12,742 | 9,784 |
| Mean scaled Jacobian, median | 0.907 | 0.922 |
| 5th-percentile scaled Jacobian, median | 0.555 | 0.650 |
| Hausdorff distance, median | 0.759% | 0.491% |
| Mean distance, median | 0.024% | 0.037% |
| Results further than 1% of the diagonal from the input | 27 of 69 | 10 of 60 |
| Results with an inverted or degenerate quad | 65 of 69 | 52 of 60 |

Medians are over the 60 cases both tools completed with quads; distances are
fractions of the bounding-box diagonal.

**QuadLoom is more reliable and about four times faster, and its quads are worse
on most meshes.** It has the better mean scaled Jacobian on 21 of 60 cases, the
better 5th percentile on 20, the better Hausdorff distance on 17, and it uses
1.4 times the quads to get there. The design's bar ("equal or better than
QRemeshify on most meshes") is met for reliability, speed and mean distance, and
not for element quality or worst-case distance.

The split by mesh source says where:

| Source | Cases | Better mean SJ | Better 5th-percentile SJ | Better Hausdorff |
|---|---|---|---|---|
| common3d (organic classics) | 12 | 9 | 8 | 1 |
| crane (clean organic) | 7 | 3 | 3 | 4 |
| khronos (game assets) | 19 | 6 | 6 | 8 |
| thingi10k (mechanical, printable) | 22 | 3 | 3 | 4 |

Organic models are QuadLoom's strength, as the stress set (ten models, seven
organic) has shown since 0.1.8. Mechanical parts and game assets, which that set
barely covers, are where it loses.

## What goes wrong

Each class below was seen in a render, not only in a number.

1. **Sheared patch interiors.** On `thingi10k/49382` (a dome, 12 patches in both
   tools) the oracle's quads are a near-orthogonal grid, mean scaled Jacobian
   0.992; QuadLoom's are parallelograms twisted across each patch, 0.868. The
   layouts are comparable (decision 0006 measured that), so the loss is after
   the layout: how a pattern is placed in its patch and relaxed.
2. **Thin parts flattened.** `common3d/cheburashka`'s ears come out as faceted
   slabs: Hausdorff 5.8% against the oracle's 0.7%. The layout mesh of the same
   run is within 0.11% of the input, so the quad stage does it, not the prep
   remesh. Eleven of twelve common3d results are further from the input than
   the oracle's.
3. **Patches kept as triangles on mechanical parts.** `common3d/fandisk` keeps
   one curved strip as triangles split into quads: 486 irregular vertices
   against the oracle's 34. Next to it a tapered fillet patch is filled with
   quads squeezed to slivers. `thingi10k/472206`: 880 irregular against 48.
4. **The default size runs away.** Ten results have more than 100,000 quads at
   Density 1, where most have about 12,000. On the three looked at (alligator,
   650187, TrafficCone) the automatic prep-remesh size is the cause, each
   reaching 125,000 to 155,000 prep triangles. `common3d/alligator`, a flat
   5,981-triangle outline, becomes 140,802 quads; `common3d/woody` (1,267
   triangles) 136,867.
5. **Dense feature networks.** Game assets arrive with thousands of sharp edges
   from split normals (`khronos/Corset`: 8,570 feature edges, 1,906 corners,
   1,022 patches, 171 of them filled with the fan fallback). The 5th-percentile
   scaled Jacobian is 0.21 against the oracle's 0.72. Turning Follow UV Islands
   off changes nothing on the 26 Khronos assets (identical output), so that
   default is not the cause.
6. **A failed output check.** On `khronos/ChronographWatch` (124 parts, 6,208
   patches) the quad stage fails its own last check, "edge in more than two
   faces". The run falls back to the triangle patch layout and says so in a
   warning, as designed, but the check failing is a bug in the stitching.
7. **Inverted quads** in 65 of 69 results. The oracle has them in 52 of 60, so
   this is a shared weakness and a chance to lead.

## Reliability

The first pass lost 11 of 70 runs to one fault: the add-on's worker refused with
`Permission denied ... progress.json`. Run again one at a time, 2 of those 11
failed the same way. The cause and the fix are in decision 0021's addendum. The
table above uses the 68 results from the released zip plus the last 2 from a
build with only that fix (the backend is the same), so its "70 of 70" is the
engine's reliability, not the released add-on's: 0.4.0 as shipped fails roughly
one run in six on this machine.

No run crashed Blender.

## Method notes

- Times are from a machine that was also compiling, two jobs at a time: read
  them as upper bounds. The oracle's are from its own run in October.
- The tools choose their own quad count at their defaults, so quality is not
  compared at equal counts. QuadLoom uses more quads, which favours it on
  curved surfaces. A matched-count run (Target Quads set to the oracle's count)
  has not been done.
- The oracle treats UV-seam splits as holes (decision 0003) and reads sharp
  edges differently, so the Khronos rows compare the tools as a user meets
  them, not the same feature lines.
- Benchmark meshes keep their own licenses and are not part of this report.
