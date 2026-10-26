# QuadLoom

Feature-aligned quad remeshing for Blender, written from scratch in safe Rust.

QuadLoom turns a triangle mesh into a quad-dominant mesh while following sharp
features, UV seams and open borders. The remesher runs as a native module
(`quadloom_native`) behind a Python Blender extension, so the viewport stays
responsive and a backend failure can be contained in a worker process.

> **Status:** 0.4.0, Windows x64, Blender 5.2. This is a pre-1.0 release; the
> target-count mode is approximate. See [Known limitations](#known-limitations).

## Install

1. Download `quadloom-0.4.0.zip` from the [Releases](https://github.com/CloudyTabzy/QuadLoom/releases) page.
2. In Blender: **Edit → Preferences → Extensions → Install from Disk...**, choose the zip.
3. Enable **QuadLoom** if it is not enabled automatically.

The extension is currently built for **Windows x64**. Linux and macOS wheels are not
published yet.

## Quick start

1. Select a mesh object.
2. Open the 3D Viewport sidebar (`N`) and choose the **QuadLoom** tab.
3. Pick a preset (Balanced, Organic, Hard Surface, Regular Quads, Low Poly, High Detail).
4. Choose **Density** for a relative quad size, or **Target Quads** for an approximate
   total count.
5. Press **Remesh**. The result is a new object; the original is left untouched.

The panel also exposes sharp-feature detection, symmetry, optional UV-seam handling,
solver effort, smoothing and attribute transfer under **Advanced**.

## Features

- Feature-line-driven tracing and patch layout, with the cross field solved on the
  input surface.
- Quad-dominant output with closed-form patch patterns, stitching and smoothing;
  patches that cannot be filled safely fall back to triangles instead of forcing a
  folded quad.
- Sharp edges, open borders, symmetry (X/Y/Z, auto-detect) and per-part fallback for
  meshes that cannot be repaired.
- UVs, materials, vertex groups, colour attributes and armature deformation are
  transferred to the result.
- Remeshing runs in an isolated, memory-limited worker; a backend panic or abort
  does not take Blender down.
- Pure safe Rust (`#![forbid(unsafe_code)]`) with no C/C++ build dependencies.
- Deterministic output for the same input and settings.

## Build from source

Requirements: Rust 1.89+, Python 3.11+, and Blender 5.2 (for the extension zip).

```powershell
python tools/build.py
# -> dist/quadloom-<version>.zip
```

`python tools/build.py --dev` builds a fault-injection wheel for the test suite;
never distribute that zip.

The build script uses `QUADLOOM_BLENDER` if set, then looks for `blender` on
`PATH`, then common install locations.

### Tests

```powershell
cargo test --workspace --all-features
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
.venv\Scripts\python -m pytest tests/python -q
```

The Python tests need the development wheel (`python tools/build.py --dev`) and
NumPy. The public repository contains the Rust and Python unit tests; the headless
Blender integration suite is maintained separately and is not included yet.

### CLI

```powershell
cargo run -p quadloom-cli -- info mesh.obj
cargo run -p quadloom-cli -- run mesh.obj remeshed.obj
```

## Benchmark

`benchmark/` contains the model inventory (`sources.json`), a downloader
(`fetch_meshes.py`) and the published measurement reports. The meshes themselves are
not committed; their licenses are recorded as they are fetched. See
[`benchmark/README.md`](benchmark/README.md).

## Known limitations

The full, current list is in [`docs/open-items.md`](docs/open-items.md). The main
user-facing ones for 0.4.0:

- **Target Quads is approximate.** Coarse requests can miss substantially (for
  example 500 requested → 1,660 on the armadillo); constrained feature/layout cases
  are the hardest.
- Crowded organic regions can still keep a few patches with multiple singularities.
- UV-sphere poles and similar high-index singularities are not fully handled.
- Layouts with more than 2,500 free segments skip the global ILP and use regional
  repair.
- Shape keys and edge attributes (crease/bevel weights) are not transferred; custom
  normals are recomputed.
- Windows x64 only for now.
- Heavy-noise inputs and very large meshes remain slow; marking sharp edges on a 1M
  triangle mesh can freeze the UI for 1–2 seconds.

## License

QuadLoom is licensed under **GPL-3.0-or-later**. See [`LICENSE`](LICENSE).

The design is an independent Rust implementation of the QuadWild-BiMDF pipeline.
QuadWild-BiMDF and QRemeshify are referenced as oracles for comparison; no C++ code
is linked or shipped. Benchmark models keep their original licenses and are never
committed or redistributed.
