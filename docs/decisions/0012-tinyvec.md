# 0012 — Inline small vectors in the remesher (tinyvec)

**Status:** accepted 2026-10-23.

## Context

The question was whether smallvec helps. The prep remesh is about 60% of a run. Each split, collapse and flip asked for a vertex's fan, ring or faces, and each request allocated a fresh `Vec` of about 6 items.

## Decision

- **`edit::Ring<T>` is `TinyVec<[T; 16]>`.** `fan`, `one_ring`, `faces_around` and `feature_neighbours` return it. The edit operations' scratch lists are inline too. `replace_faces` returns the new ids as a `Range`, because new faces are appended one after another.
- **tinyvec, not smallvec.** Both measured the same: smallvec was about 1% faster. tinyvec is itself `forbid(unsafe_code)`, while smallvec's internals use `unsafe` and have had soundness advisories. tinyvec was chosen, which keeps hashbrown (decision 0011) as the only dependency whose internal `unsafe` we accept by exception. tinyvec has no `dedup`, so `dedup_sorted` covers it.
- **Tried and dropped (no measurable gain):**
  - inline adjacency and sorted patch sets in quad smoothing: that stage's time is the integer solver;
  - inline per-vertex visits in the trace occupancy map.

## Evidence

Interleaved A/B runs, best of 3, over the 10 stress models plus a 1M-triangle sphere:
- The prep remesh is 17% faster summed (37.1 s → 30.9 s with smallvec, 31.2 s with tinyvec), and faster on every model.
- Whole runs are 11–12% faster.
- Output is byte-identical and peak memory unchanged.
