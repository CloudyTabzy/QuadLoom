# 0014 — No bump allocator; reuse the trace's search arrays instead

**Status:** accepted 2026-10-23.

## Context

The owner asked whether `bumpalo` 3.20.3 or `bump-scope` 2.4.1 helps, and where. A bump arena pays off where many short-lived allocations of varying size are made in one phase and freed together. So the first step was to find out how much time the allocator takes.

A scratch profiler linked `quadloom-core` with a counting global allocator: per-stage counts and bytes, timed `alloc`/`free` with the timer cost subtracted, and call sites sampled from 1 in 2,048 backtraces.

| Model | Allocations per run | Allocator time |
|---|---|---|
| bunny | 3.8M | about 3% of the prep remesh |
| 105762 | 5.7M | small in every stage but the trace |

The allocator time is small almost everywhere because the earlier tinyvec work (decision 0012) already keeps the remesher's per-edit lists inline.

**The one large cost was not an arena problem.** `trace::Search::new` allocated arrays sized to the whole graph (dist, parent, via, corridor) for every rayon job. `map_init` runs its init closure once per split job, not once per thread. On 105762 that came to about 3 GB, mostly blocks over 512 KB, which Windows serves straight from the OS with page faults.

## Decision

- **No bump allocator.** The most an arena could save is the remaining allocator time, about 1–2% of the prep remesh after the fixes below. Getting it would mean threading a `&Bump` lifetime through the remesh passes and quantizer. It would also add a dependency that is mostly `unsafe` internally: bump-scope has about 1,100 `unsafe` occurrences and bumpalo about 240, against tinyvec's none. That trade isn't worth it.
- **`SearchPool`** (`trace/search.rs`). A layout keeps spare search arrays, and each parallel job borrows a set and returns it when dropped. Every search already resets exactly the entries it touched (`touched`, plus the corridor stamps), so a reused search behaves as new. Arrays sized for a different graph are discarded.
- **Small temporaries inline or reused:**
  - the sizing ring is folded instead of collected;
  - the jet-fit offsets live in the reusable `Ball`;
  - collapse candidates are a `TinyVec<[HalfEdge; 2]>`;
  - quantization side counts are a `TinyVec<[u32; 8]>`.

## Evidence

Interleaved A/B runs, best of 3, 10 stress models plus a 1M-triangle sphere. Output is byte-identical everywhere.
- **Search pool:** the trace stage is 4.2% faster summed (4.74 → 4.54 s) and 20% faster on 105762 (0.45 → 0.36 s). Peak memory is up 0–7 MB, because each worker keeps a spare set of arrays.
- **Small temporaries:** the prep remesh is 1–3% faster on the 10 stress models.
