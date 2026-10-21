# 0002 — Stage comparisons use the oracle DLL's own files; the instrumented C++ build waits until M6

**Date:** 2026-10-21
**Status:** Accepted

## Context

Design §9.1 planned a dev-only, instrumented C++ build of quadwild-bimdf so QuadLoom's stages could be compared one at a time with the oracle. The M2 spike showed that QRemeshify 1.5.0 (built from quadwild-bimdf) already writes every intermediate stage to Blender's temp folder before importing its result:

| File | Stage | Use for QuadLoom |
|---|---|---|
| `<name>.obj` | the triangulated input it actually used | sanity check of input normalization |
| `<name>_rem.obj` | stages 1–2: cleaned, isotropically remeshed mesh | M3: compare remesh density, quality, feature preservation |
| `<name>_rem.sharp` | stage 1: detected sharp features | M3: compare feature detection |
| `<name>_rem.rosy` | stage 3: per-face cross field | M4: field smoothness, singularity count and placement |
| `<name>_rem_p0.obj`, `.patch`, `.corners`, `.feature`, `.c_feature` | stages 4–5: traced mesh, per-face patch labels, patch corners | M5: patch count, patch valence, corner placement |
| `*_quadrangulation(_smooth).obj` | stages 6–9: quads before and after smoothing | M7–M8: final comparison |

What's *not* in those files: the candidate traces before selection, and the BiMDF quantization problem (per-subside quad counts, objective value).

## Decision

- M2–M5 compare stages against the DLL's stage files, collected automatically by `benchmark/blender/job.py` into `runs/<id>/oracle/<case>/stages/`.
- The instrumented C++ build (still time-boxed) moves to **M6**, where BiMDF internals are actually needed.

## Consequences

- No C++ toolchain work before M6, and the M3–M5 comparisons use exactly the binary users run today.
- Readers for `.sharp`, `.rosy` and `.patch` are needed in `quadloom-cli` (added in the milestones that use them).
- If M6's build turns out impractical, the fallback stays as designed: hand-built cases plus final-mesh metrics.
