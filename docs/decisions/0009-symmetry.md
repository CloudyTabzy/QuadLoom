# 0009 — Symmetry: cut, remesh one side, mirror and weld

**Status:** accepted 2026-10-23 (0.1.4).

## Context

The goal was symmetry "similar to Ksami's QuadWild add-on, evolved". QRemeshify bisects the mesh at the origin with Blender's operator, remeshes one half, then mirrors it with a Mirror modifier. The user must have centred the object, and the seam relies on the remesher treating the cut as a border.

## Decision

`crates/quadloom-core/src/symmetry.rs`, run inside the pipeline:

1. **Planes:**
   - X, Y and Z can be combined, for half, quarter or eighth symmetry. They're in object space and go through the origin, exactly like Blender's Mirror modifier.
   - **Auto Detect** tests each axis through the origin and through the bounding-box centre. It reflects up to 4,000 sample vertices across the plane and takes the root-mean-square distance to the surface (via the BVH) over the bbox diagonal. It keeps the planes within `tolerance` (default 0.5%) and reports the measured asymmetry per axis. The user doesn't need to centre the object or guess the axis.
2. **Cut**, right after repair, so every later stage sees one side:
   - Triangles are clipped to the kept side; vertices within 1e-9 of the diagonal are snapped onto the plane, to avoid slivers.
   - Crossing points are placed exactly on the plane, and shared between the two triangles of an edge.
   - Faces lying in the plane are dropped, because the mirror recreates them.
   - The input-vertex maps and face origins are carried through, so Blender's Sharp marks, seams, materials and face sets keep working.
3. **Seam.** The boundary edges of the cut that lie on a plane are always feature lines, even with open-boundary features off. As a feature and border, the seam aligns the cross field: edge loops meet the centre line squarely and run along it, the way artists retopologise. The remesh, layout and quads keep seam vertices exactly on the plane. Feature vertices are fixed during smoothing, and arc-length samples of on-plane segments stay on the plane.
4. **Mirror and weld.**
   - The quad mesh is reflected across each plane in turn, with reversed winding.
   - Vertices within 1e-6 of the diagonal from the plane are snapped and welded, so the result is one watertight, exactly symmetric mesh.
   - Patch ids and feature edges are mirrored too. Mirrored faces keep their patch id, so the patch view and the Quad Shape preview are symmetric.
5. **Fallbacks:**
   - Nothing on the kept side, or no plane detected: the remesh runs without symmetry, and the panel says why.
   - Keep Negative Side switches which half is kept.
   - The prep mesh and layout previews stay one side, because their per-face arrays index them.

## Evidence

- Torus with X symmetry: closed, manifold, all quads, and every vertex has its mirror image (1e-6).
- X+Y symmetry: the same, with all three images.
- Auto-detection finds all three planes of a centred torus, finds planes through the centre of a shifted torus, and rejects X on a lopsided torus while keeping Y.
- A centred cube cut at X keeps only x ≥ 0, with a closed seam square of length 4 on the plane.
- Blender scenario `symmetry_mirrors_quads`: Suzanne with Auto Detect gives "mirrored X", all quads, symmetric vertices.

## Open items

- A symmetry plane through an arbitrary point or orientation. It could come from the 3D cursor, or from a detected plane that isn't axis-aligned.
- Radial (rotational) symmetry.
