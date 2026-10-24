# Open and deferred items

Everything known to be unfinished, in one place. Last updated 2026-10-24 (0.3.0 and 0.4.0 releases, decisions 0021/0022). The details live in the decision records named here. When you finish an item, move it to "Done" with its decision number.

## Next up

1. **Improve coarse target accuracy and quality.** 0.4.0 ships approximate target count. Five coarse real-asset requests missed tolerance (bunny 2,000 → 1,560; worst 500 → 1,660 on armadillo), with poor coarse element quality. Existing layout/fixed patches and quantization discontinuities need further work; see decision 0022 and `../benchmark/reports/2026-10-24-target-count.md`.
2. **Validate crash containment on larger real assets.** Decision 0021 isolates the backend and attribute mapping, with OS memory limits. Profile IPC/startup overhead and host allocations on dense meshes; Blender modifier evaluation and output allocation still run in Blender.
3. **More real attribute assets.** Test additional glTF characters, posed rigs and Smart UV Project maps beyond the automated synthetic rig/UV/material scenarios.
4. **M9: benchmark and GitHub release.** A full benchmark comparison against QRemeshify on quad output, then the hosted release (no Git remote configured yet).

## Output quality

| Item | Notes | Where |
|---|---|---|
| Quad Shape vs Follow UV Islands | UV seams as feature lines can make the opt-in Quad Shape solver fall back (the torus primitive does). Workaround: turn Follow UV Islands off. | 0019, 0007 |
| Crowded patches, remainder | The Organic armadillo hand keeps a few patches with 2–4 singularities (3.7% of the area). Under-resolved bumpy meshes stay crowded. The second layout costs about 3 s on the armadillo. | 0018 |
| High-index singularities | UV-sphere poles and similar; open since M4. | M4 rulings |
| Leftover non-convex patches, handle recovery | 141 concave vertices across the benchmark, plus patches with a handle. | M5 rulings |
| Large layouts | Over 2,500 free segments the global ILP is skipped and only regional repair runs. | 0008 |
| Known hard inputs | crane/oloid quality; heavy-noise input takes 59 s. | M3b rulings |
| Automatic cone placement for Quad Shape | Organic models with borders fall back to the regular field. | 0007 |

## Features

| Item | Notes | Where |
|---|---|---|
| Shape keys | Not transferred: their deltas need the basis shape, not the evaluated mix. | 0019 |
| Edge attributes | Crease and bevel weights aren't transferred (the edges don't survive). Custom normals are recomputed. | 0019 |
| Anisotropy | Long quads on tubes and cylinders. | external ideas |
| Size field | One Poisson solve for better local size targets. | external ideas |
| More symmetry | Planes through any point or orientation; radial symmetry. | 0009 |
| Best-of-N layouts | Try a few seeds and keep the best by score. | |
| Voxel fallback (opt-in) | For input that repair can't make manifold. | |

## Robustness, platforms, polish

| Item | Notes |
|---|---|
| Linux and macOS wheels, CI | Plus a Blender 4.2–5.1 test matrix (design §11). |
| Progress across parallel parts | Not monotonic. |
| Worker portability | Windows tested; validate Unix memory limits and Blender bundled-Python discovery when packaging Linux/macOS wheels. |
| Scene renamed during a run | Discard still stops the worker and drops the result, but the renamed scene's last-run status can remain "Running". |
| Main-thread freeze | 1–2 s marking sharp edges at 1M triangles. |
| Memory on 87355 / 105762 | +3 MB since 0.1.7; not profiled (performance journal). |
| Target-mode hints | Some inherited fallback/error hints still say Density even though Target mode ignores it. |
| Direct native grid/center cancellation | Large directly fillable patterns check before/after construction rather than within grid loops. Blender hard cancellation still terminates the worker. |

## Done

- 0.4.0 approximate target quad count: decision 0022; research, resource/accuracy/cache/symmetry tests, 117 Python tests, 49 Blender scenarios, production ZIP and local tag. Coarse constrained cases remain above.
- 0.3.0 OOM-hardening release: decisions 0019/0021; production ZIP and local tag, full gate plus seven production smoke scenarios passed. See `releases/0.3.0.md`.
- Crowded patches: 0.2.0, decision 0018.
- Attribute transfer (UVs, materials, vertex groups, colours, rig): decision 0019, released in 0.3.0.
- Starved and spur-blocked patches: 0.1.10, decision 0017.
