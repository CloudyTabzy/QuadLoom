# 0003 — Repair ladder and feature detection: where QuadLoom deliberately differs from the oracle

**Date:** 2026-10-21
**Status:** Accepted

## Decisions

1. **Weld before anything else** (epsilon `1e-6 ×` bbox diagonal, configurable, negative disables).
   - QRemeshify passes Blender's mesh as-is. glTF/FBX assets are split at UV seams and hard normals, so the oracle sees every seam as an **open boundary**: a hole in the topology that it then also marks as a feature line.
   - QuadLoom welds those seams first. On the benchmark this joined 1,438 vertices on `khronos/Fox`, 3,371 on `khronos/CesiumMilkTruck` and 291 on `khronos/Duck`.
   - It's the main reason QuadLoom reports less feature length than the oracle on game assets: the oracle's extra "features" there are seam artifacts.
2. **Zero-area faces are removed, not flipped or collapsed** (M3a).
   - The oracle collapses, flips or removes them inside `SolveGeometricArtifacts`.
   - Removal is simpler and always safe. Any hole it leaves is re-triangulated by the M3b remesher. Every removal is counted in `RepairReport`.
3. **Non-manifold edges keep one consistently wound pair attached.** A detached face that's still fan-connected around both endpoints gets its own endpoint copies, and the fan pass repeats until stable. The oracle's equivalent (`RemoveNonManifolds` / `openNonManifoldEdges`) moves vertices slightly instead; QuadLoom never moves geometry during repair.
4. **Tiny parts are skipped only next to something bigger:** the largest part always survives, so a small but complete mesh is never discarded. The oracle drops small components unconditionally.
5. **Spur pruning instead of erode/dilate, and only for angle-detected edges.**
   - The oracle's `ErodeDilate` erodes every feature tip by N steps, then dilates back from the originals. In QRemeshify's flow it never runs at all, because features come from Blender and `UpdateSharp` is false.
   - QuadLoom removes only **angle-only** dangling chains (≤ `prune_steps` edges and < 5% of the diagonal). Anything the user marked, and anything from mesh structure (seams, material/face-set/open boundaries), is never pruned.
6. **Six feature sources, each a toggle.** Defaults equal QRemeshify's (angle 35°, marked sharp, seams, material boundaries, face-set boundaries, open boundaries), but each can be switched off. QRemeshify always uses all of them.

## Evidence (benchmark run `20261021-m3a` vs oracle `20261021-oracle-baseline`)

- **Robustness:** 70/70 benchmark models pass through repair and feature detection without error. Their repairs include welds, duplicate removal, non-manifold splits on `common3d/beetle` (47 edges) and `common3d/suzanne`, and a fixed flipped face.
- **Feature fidelity on a clean CAD part:** `common3d/fandisk` has feature length/√area 8.91 (oracle 8.90) and 24 corners (oracle 24).
- **Open item for M3b:** `crane/oloid` (a smooth developable surface, coarse tessellation) gets 8.6× the oracle's feature length, because the 35° angle test fires along its coarsely tessellated high-curvature arcs. Feature detection after the prep remesh, or a curvature-aware angle test, should settle it. It's tracked for M3b.

## Consequences

Glue-split game assets produce clean, closed topology and far fewer spurious feature lines than in QRemeshify. Differences in feature-length medians against the oracle are expected and explained above; benchmark comparisons should look at closed-surface CAD parts for a like-for-like check.

## Update 2026-10-23: salience test for angle features

**Problem.** The owner's dyntopo sculpt produced 59 "feature edges". The raw per-edge dihedral test (35°) also fires on sculpting and scanning noise. Those edges were then forced into the layout, marked Sharp (hard shading) and held still during smoothing, which left visible creases.

**Fix.** An edge that's sharp by its own two faces stays a feature only if it's still sharp between the two **sides of its neighbourhood**:
1. Take the faces within 3 rings of the edge.
2. Split them by the plane that contains the edge and bisects its normals.
3. Average each side's normals by area and test the angle between the two averages.

A real crease keeps its angle; noise averages out.

**Effect:**
- fandisk is unchanged: 610 edges vs 613.
- Organic models shed noise: armadillo drops from 1,016 to 467 edges, bunny from 525 to 388, rocker-arm from 248 to 193.
- A ±6% lumpy sphere keeps 10 of its 388 raw-sharp edges.
