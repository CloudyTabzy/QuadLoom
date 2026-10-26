# 0011 — hashbrown maps, and a HashTable for the remesher's edges

**Status:** accepted 2026-10-23.

## Context

hashbrown 0.17.1 was chosen for its richer API. All maps were `std::collections` ones with SipHash, except the remesher's, which had its own FxHash-style hasher.

## Decision

- **Every `HashMap`/`HashSet` is hashbrown's**, with its default foldhash hasher (`default-features = false`, features `default-hasher` and `inline-more`). foldhash seeds each map separately, which avoids the slowdown fixed-seed hashers get when one map is copied into another in iteration order. Results don't depend on iteration order: the same mesh gave byte-identical output in repeated runs before and after the switch.
- **The remesher's directed-edge map is a `HashTable<u32>`** of half-edge ids (`edit::EdgeTable`). The `(from, to)` key is read back from the face corners, which never change once a face exists (faces are only appended), so the table can rehash at any time. Each entry is 4 bytes, not 12. The custom `EdgeHasher` is gone.
- **`entry_ref`** looks up the shared-patch region map by a borrowed slice, so the key is copied only when it's new.
- **Unsafe:** `unsafe_code = "forbid"` still covers our crates. hashbrown's internal `unsafe` is that crate's responsibility (a deliberate exception). Its `unsafe fn` APIs (`*_unchecked`) are not used, because calling them needs `unsafe` in our code.

## Evidence

Interleaved A/B runs over the 10 stress models plus a 1M-triangle sphere, best of 3:
- All outputs are byte-identical to the previous build.
- Totals are 1–5% faster per model. Before the remesher change, quantize was 13% and trace 14% faster summed.
- The `HashTable` makes the prep remesh about 5% faster and lowers the 1M-triangle peak memory from 613 to 581 MB.
- The foldhash hasher ties with the old FxHash-style one (within 1–2%).
