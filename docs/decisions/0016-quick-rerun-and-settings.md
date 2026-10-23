# 0016 — Quick Re-run, Regularity, Solver Effort, Show Intermediate Steps

**Status:** accepted 2026-10-23.

## Context

Before calling QuadLoom production-ready, the owner asked which settings from Ksami's QRemeshify and from their QRemeshify-rs fork we could expand on.
- **Already covered:** Preprocess, Smoothing, sharp detection and angle, symmetry, density, the feature sources, presets.
- **Not worth copying:** the QuadWild solver internals (ILP method, flow/Satsuma configs, parity and "repeat losing constraints" toggles, callback limits). Our quantizer is our own deterministic code.
- **Four ideas carried over,** and the owner chose all four:
  - Use Cache → Quick Re-run;
  - Alpha → Regularity;
  - Time/Gap Limit → Solver Effort;
  - Debug Mode → Show Intermediate Steps.

The native API is now **7**.

## Decision

- **Quick Re-run.**
  - The pipeline is split into `run_upstream` (validate through the patch layout, returning an `Upstream`) and `finish` (quad stage, mirror, output). `run` is the two in sequence, so nothing else changed.
  - The binding's `Cache` keeps one `Upstream` with its key: a hash of the input buffers and feature hints, plus the `Params` with `quad` reset. A run given the cache with a matching key starts at `finish`.
  - The core decides whether the kept work matches, not the add-on. Changing the mesh, sharp marks, or any non-quad setting runs everything again. The input check is a 64-bit hash plus exact settings equality, so wrongly reusing work would take a hash collision.
  - The add-on keeps one cache per session. It is on by default (Advanced > System) and freed on file load, on unregister, and when turned off.
  - The old entry is dropped before a new upstream is computed, so at most one is held.
  - `finishing_a_kept_upstream_matches_a_full_run` and `test_cache.py` check that a reused run's output is identical to a full run's.
- **Regularity** (Advanced > Quads) exposes the quantizer's existing weight, mapped `25·4ˣ` for x in 0–1, so 0.5 is the default 50 exactly.

  | Weight | Measured effect |
  |---|---|
  | below 25 | split/fallback patterns appear (bunny at 12.5: 58 split, 48 fallback) |
  | 25–100 | the real trade-off, from matching size to regular grids |
  | above 100 | saturated: fully regular, with 3–8% more quads |

  A new **Regular Quads** preset sets it to 1.
- **Solver Effort** (Fast / Balanced / Thorough) scales the integer program's node limits by 0.25 / 1 / 4 (`quad.solver_effort`). These are node counts, not time, so results stay deterministic.

  | Model | Fast | Balanced | Thorough |
  |---|---|---|---|
  | bunny, quantize | 1.2 s, almost the same result | 3.0 s | 10 s, no better |
  | armadillo, quantize | 2.6 s, same result | 9.4 s | 36 s, no better |

  The tooltips say so: Fast is for previews, Thorough rarely helps.
- **Show Intermediate Steps** (Advanced > System, off by default) builds two hidden objects in a "QuadLoom Steps" collection from data the result already carried:
  - the prep remesh, with sharp edges;
  - the patch layout, with a per-patch color attribute (corner domain; Blender lists color attributes only on points and corners).

  A re-run replaces the previous steps of the same object.

**Presets.**
- Presets set every result-changing setting on the main panel plus Regularity and Solver Effort. Regular Quads sets Regularity to 1; every other preset uses 0.5 and Balanced effort.
- Editing any of them by hand switches the preset to Custom.
- Symmetry and the System toggles (Quick Re-run, Show Intermediate Steps) don't change the result, so presets never touch them.
- The scenario `presets_set_plain_settings` checks all of this.

## Evidence

Quick Re-run, full run vs. the same mesh at Density 2:

| Model | Full run | Quick re-run |
|---|---|---|
| igea | 6.8 s | 0.5 s |
| bunny | 8.5 s | 0.8 s |
| armadillo | 24.0 s | 3.8 s |
| fandisk | 0.9 s | 0.1 s |

Dev wheel on a busy machine, so the ratios are what count. Default-settings output is unchanged.
