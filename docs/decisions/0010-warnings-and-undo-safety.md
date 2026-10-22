# 0010 — Warnings, edge cases and undo safety

**Status:** accepted 2026-10-23 (0.1.5).

## Context

The owner asked for fallbacks and warnings for edge cases ("symmetrizing a mesh where it doesn't make sense should warn"), and for safe undo/redo. They remembered QuadWild's add-on lagging or crashing on undo.

## Decision

**Warnings.** A run that succeeds can still carry warnings. The pipeline collects `RunOutput.warnings`: each has a stage, a message and an optional hint. The binding returns them as `result["warnings"]`, and the add-on shows them in a "Warnings" box under the last-run summary. The run reports at WARNING level ("QuadLoom: <first warning> (+N more, see the panel)").

| When | Warning (hint) |
|---|---|
| Explicit symmetry axis, mesh asymmetric beyond tolerance | "the mesh isn't symmetric across X (N%): the other side was replaced by a mirror of this one" (turn the axis off, or use Auto Detect) |
| Kept side holds less than 25% of the surface | "only N% of the surface is on the kept side" (Keep Negative Side) |
| Mesh doesn't reach the plane | "the mirrored copy is a separate piece" (move the origin, or use Auto Detect) |
| Nothing on the kept side, or no plane detected | "symmetry skipped: …" (Keep Negative Side, or Tolerance) |
| Symmetry asked but no quads built | "the result is one side only" |
| Quad stage failed | "the quads could not be built (…); the result shows the patch layout" |
| Fallback or kept patches above 2% | "N of M patches used a fallback pattern" (higher Density, more Adaptive Detail) |
| Quad Shape fell back | "kept the regular field on N part(s)" |
| Several objects selected *(add-on)* | "only the active object is remeshed" |
| Symmetry on a negatively or non-uniformly scaled object *(add-on)* | "symmetry uses its own (local) axes" (apply the scale) |

**Edit Mode.** The active object's edits are flushed (`update_from_editmode`) before the snapshot, so a run never works on stale data.

**Undo and redo:**
- **The source is never touched.** The result is a new object, built in one go on the main thread when the modal operator finishes. Its `UNDO` flag makes that exactly one undo step. The job holds only copied arrays and names, never `bpy` references, so an undo can't leave it pointing at freed data.
- **Undo during a run.** The `undo_pre` and `redo_pre` handlers drop the job: it's cancelled, and `Job.discarded` guarantees its result never lands, in both `modal` and `finish_job`. The add-on is free for the next run straight away.
- **No `REGISTER` flag.** Without it, "Adjust Last Operation" can't re-run the background job synchronously.

## Evidence

- Rust tests: lopsided torus with explicit X warns; symmetric torus has no warnings; shifted torus warns about the kept share; a torus away from the plane warns about the separate piece; the opposite side is skipped with a hint.
- pytest: the shape of `result["warnings"]`.
- Blender scenarios:
  - `undo_redo_result_is_safe`: three real Ctrl+Z / Ctrl+Shift+Z cycles; the result disappears and comes back with the same faces.
  - `undo_mid_run_drops_the_job`: no result lands, and the next run works.
  - `symmetry_mismatch_warns`: WARNING report, with both the asymmetry and selection warnings.
  - `remesh_is_undoable`: `UNDO` and not `REGISTER`.
