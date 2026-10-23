# QuadLoom benchmark

A fixed set of public meshes used to compare QuadLoom against the oracle (QRemeshify 1.5.0 / QuadWild-BiMDF) and against its own previous runs.

## Contents

| Path | What |
|---|---|
| `sources.json` | The curated list: source, model, tags, license notes. Edit this to change the set. |
| `fetch_meshes.py` | Downloads everything in `sources.json` into `meshes/` (stdlib only, idempotent; `--force` re-downloads). |
| `meshes/` | Downloaded files, **gitignored**. `meshes/MANIFEST.json` records URL, license, tags, size and SHA-256 per file. |
| `ab_compare.py` | Interleaved A/B of two `quadloom` builds on the stress models plus a 1M-triangle sphere: best stage and total times, peak memory, byte-identical outputs. See `docs/performance-journal.md`. |
| `stress_quads.py` | Full-pipeline quality check (quads, open edges, scaled Jacobian) on detailed models. |

## Fetch

```
python benchmark/fetch_meshes.py
```

Snapshot taken 2026-10-21: 70 models, 96 files, 272 MB, 0 failures.

| Source | Models | Why it's here |
|---|---|---|
| [Khronos glTF-Sample-Assets](https://github.com/KhronosGroup/glTF-Sample-Assets) (`.glb`, two as `.gltf`) | 26 | Real assets: UV seams, open boundaries, multi-part objects, game-ready low poly, garments (Corset, SheenCloth), soft goods |
| [Keenan Crane's 3D Model Repository](https://www.cs.cmu.edu/~kmcrane/Projects/ModelRepository/) | 7 | Clean organic shapes, a crumpled developable sheet (cloth-like, open boundary) |
| [common-3d-test-models](https://github.com/alecjacobson/common-3d-test-models) | 14 | Classic research meshes: fandisk and rocker-arm (mechanical), bunny, armadillo, teapot (non-manifold) |
| [Thingi10K](https://huggingface.co/datasets/Thingi10K/Thingi10K) subset | 23 | QuadWild's own reliability benchmark. Closed, manifold, single component, 3k–60k faces, permissive licenses, one or two per category |

## Licenses

Licenses vary per model and are recorded in `MANIFEST.json`. Most are CC0 or CC-BY. A few Khronos models carry other terms (CC-BY-NC, Poser EULA, SCEA, trademark notices). That's fine for local testing, but **never commit these meshes or ship them with the add-on.** Renders of them in public reports need attribution per their license.
