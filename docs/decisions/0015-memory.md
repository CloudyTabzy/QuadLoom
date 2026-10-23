# 0015 — Peak memory: exact sizes, flat layouts, free early

**Status:** accepted 2026-10-23.

## Context

The owner asked for better memory efficiency without losing quality and with a sensible speed trade, in safe Rust only. Large inputs peak during the repair and the prep remesh, and a host that runs out of memory takes Blender down with it.

A scratch profiler tracked live bytes per stage and, at each stage's peak, every live block over 32 KB with its call site. It is a counting `#[global_allocator]` linked against `quadloom-core`.

The 1M-triangle sphere peaked at 580 MB live, about 560 bytes per input triangle. Most of that was not essential data:
- Vecs grown by doubling (up to 2× slack, plus the old copy while moving);
- per-element heap lists;
- maps over every edge where a flat array or a subset would do;
- intermediates that outlived their use.

## Decision

Every change keeps the output byte-identical.

- **Exact sizes.**
  - The BVH computes its leaf and node counts first (`leaf_count`), so its packets, ids and nodes are allocated once at their final size.
  - Edge lists collected from `flat_map` (features, manifold split, orientation, half-edges, components) are reserved up front.
  - The feature list is shrunk after dropping non-features.
- **Growth by a quarter, not doubling** (`edit::reserve_quarter`), for the remesher's face and vertex arrays. They start at the input's exact size, so the first split used to double them.
- **In-place face compaction** (`EditMesh::compact_faces`).
  - A decimating pass appends replacement faces for every collapse. On the sphere that was about 2M slots in one pass, before the between-iteration rebuild could reclaim them.
  - Passes now snapshot edges by their endpoints rather than ids, and compact once dead slots outnumber live faces.
  - Compaction keeps live faces in order and keeps vertex ids, so every id comparison and fan start is unchanged.
  - `waste()` still counts compacted slots, so full rebuilds happen exactly when they did before.
- **Flat layouts instead of a heap list per element:**
  - manifold split: face links as `[u32; 3]` per face, not a map keyed by (face, edge); vertex→faces as compressed rows;
  - orientation: inline `TinyVec<[Neighbour; 3]>`;
  - feature salience: `[u32; 3]` neighbours per face.
- **Smaller structures:**
  - The projector maps only feature edges to their faces, not every edge.
  - The four-wide BVH node no longer stores its halves' boxes, which are exactly the min/max union of their children's: 304 → 208 bytes.
  - The BVH is built straight from positions and indices, without a per-triangle corner copy.
  - The collapse sort is unstable (its keys are distinct), so there is no scratch buffer.
- **Free early.**
  - The input mesh is dropped after repair, and the repair's vertex/face maps and the feature set after feature detection.
  - `remesh::prepare`/`Prepared::run` split the remesh, so the cleaned geometry is freed once the edit mesh and projector have their own copies. A single-part mesh is no longer copied per part.
  - The surface BVH is built before the edit mesh, so its build buffers are gone before the edit mesh exists.

**Not changed, by choice:**
- The SIMD BVH packets (about 77 B per triangle, about what the old binary tree used): they are the query speed.
- The projector's per-triangle f64 normals: recomputing them would slow the hottest path, and `f32` would change facing decisions near 90°.
- The quantization ILP's memory: it is inside `microlp`'s branch and bound, and shrinking it would change the quads.

## Evidence

Interleaved A/B runs, best of 3, against the previous commit. Output is byte-identical on all 11 models, and whole runs are 3.5% faster (46.40 → 44.79 s).

| Peak process memory (pagefile) | Before | After |
|---|---|---|
| sphere, 1M triangles | 619 MB | 291 MB |
| igea, 269k triangles | 200 MB | 84 MB |
| max-planck | 78 MB | 52 MB |
| bunny | 55 MB | 46 MB |
| armadillo | 88 MB | 77 MB |

On mid-size models the peak is now the quantization ILP (in `microlp`), not our data.

`compacting_faces_changes_nothing_but_memory` and `arrays_are_allocated_at_their_final_size` guard the two subtle parts.
