# 0001 — The native module is `quadloom_native`

**Date:** 2026-10-21
**Status:** Accepted

## Context

The Blender add-on package is `quadloom` (extension id `quadloom`). In development and in headless tests, the add-on's parent folder is often on `sys.path`. A compiled module also named `quadloom` would then be shadowed by the add-on package (or the reverse), depending on path order. That's a confusing class of import bugs.

## Decision

The PyO3 module (crate `quadloom-py`, `[lib] name`) is **`quadloom_native`**. Only `addon/quadloom/bridge.py` imports it.

## Consequences

- `import quadloom_native` is unambiguous everywhere: in Blender, the test venv and the CLI-free pytest suite.
- The wheel is named `quadloom_native-<ver>-cp311-abi3-<plat>.whl`.
