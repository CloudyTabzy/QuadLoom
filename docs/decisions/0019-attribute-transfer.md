# 0019 — Attribute transfer: UVs, materials, weights, colours, rig

**Status:** accepted 2026-10-23.

## Context

The result was a bare mesh: no UV maps, materials, vertex groups, colour attributes or rig. QRemeshify users report the same gap ([ksami/QRemeshify#46](https://github.com/ksami/QRemeshify/issues/46): "Remesh removes lots of information (UV-Maps, Materials, etc.) … Please don't remove these!"). The owner asked for this to be treated as seriously as the user report does.

**The common fix and why it falls short.** A fork's fix for #46 copies data with Blender's Data Transfer modifier (`POLYINTERP_NEAREST`) and picks each face's material by its centre's nearest source face.
- **UVs across seams.** A UV map jumps at a seam. A result corner on the seam is interpolated from whichever side is nearest, so a quad along a seam stretches across the texture.
- **Thin parts.** Nearest-point lookups grab the opposite side of a thin part (an ear, a lip, a sheet).
- **Thin material strips.** A face-centre lookup can lose a thin strip to its neighbour.
- **Rig.** Its rig copy parents the result, but the remesh was of the posed mesh, so a posed character gets deformed twice.

## Decision

### What is kept

| Data | How |
|---|---|
| UV maps | every map, by name; the active and render flags are kept |
| Colour attributes | every one, any domain; active and render colours are kept |
| Other generic attributes | point, corner and face attributes of any type |
| Face sets | as a face attribute (`.sculpt_face_set`) |
| Flat or smooth shading | as `sharp_face` (a face attribute) |
| Materials | the slots in order; each face's index |
| Vertex groups | every group by name, weights interpolated |
| UV seams | marked on the result wherever its active UV map jumps |
| Rig | parent (with type, bone, parent inverse and local transform) and every Armature modifier with its settings |

**Not kept:**
- Shape keys (deferred: their deltas would need the basis shape, not what the viewport shows).
- Edge attributes other than the result's own feature lines (crease and bevel weights belong to edges that no longer exist).
- Custom normals (recomputed).

### How the mapping works: the core computes it, the add-on applies it

The core (`quadloom_core::transfer`) computes a mapping from the result back to the source surface and knows nothing about attribute types. The add-on gathers the source values through that mapping with NumPy, so any attribute type works the same way.

- **A sample** is a source triangle and two barycentric weights (the third is implied).
- **Per result vertex** (point attributes, vertex groups): the closest point on the source, accepting only triangles that face the same way as the vertex normal (dot > 0). That keeps thin parts from borrowing the far side. If nothing faces the same way, any triangle is accepted.
- **Per result corner** (UVs, corner colours): the closest source point reachable **from the face's own side** without crossing a seam.
  - **Corner keys.** The add-on sends a key per source triangle corner, built from the vertex and every corner attribute. Two source triangles are *continuous* across an edge when the keys at both of its ends match on both sides.
  - **The search.** It starts at the source triangle under a point inset 20% from the corner towards the face centre, which is inside the face and so on its side of any seam. It then walks over continuous edges, and only as far as could still beat the start (1.5× its distance, at most 256 triangles).
  - **Why not islands.** Restricting to the face's island isn't enough. A seam that doesn't split the mesh (a UV sphere's meridian, a cylinder's seam, a cut that stops inside an island) leaves one island, so a seam corner could still take the other side's value. The local walk can't cross the cut.
  - A face that straddles a seam is clamped to its own side rather than smeared across the texture.
- **Per result face** (materials, face sets, shading, face attributes): five samples (the centre, and halfway from it to each corner of a quad) vote by **face key** (the add-on's key of every face value of the source polygon). The winning key's sample nearest the centre is the face's source. A thin strip can't win a face by being nearest the centre alone.
- **Interpolation:** float-like types (float, float2/3/4, colours) are interpolated with the weights. Integer, boolean, quaternion and matrix values take the corner with the largest weight.
- **Weights** for vertex groups are interpolated per group, and weights below 10⁻⁶ are dropped.

### Quads follow UV islands

A seam that runs through a quad can only be approximated. **Follow UV Islands** (on by default) adds the render UV map's seams to the seam edges, so the layout puts patch borders on them and no quad spans a seam. With it, the per-corner mapping is exact.
- **What counts as a seam.** An edge whose UVs differ on its two sides at both ends is a seam. One that differs at only one end is a seam's last edge (where a cut stops inside an island) only if that vertex is on at most two such edges.
- **Pole fans.** A vertex on more such edges, like a UV sphere's pole with its fan of triangles, is one point mapped many ways, not a seam. Without this rule a UV sphere's poles became stars of feature lines.
- The result's own seams are marked by the same rule.
- It is governed by the existing "UV Seams" feature switch.
- It only uses the render map: a second map (a lightmap, say) often has many small islands, which would cut the layout into pieces.

### Rig: remesh the rest shape

With **Bind to Rig** on and an enabled Armature modifier on the source, the snapshot is taken with the Armature modifiers switched off, so the remesh is of the rest shape. They are switched back on at once, in a `finally`, and are copied as enabled (they are read while switched off). The result gets the parent and the modifiers, so it deforms with the rig like the source. Without a rig nothing changes. With Bind to Rig off, the posed shape is remeshed as seen, and no rig is copied.

### Safety

- **Off the main thread.** The mapping runs on the job's worker thread, after the pipeline, through `quadloom_native.transfer_map`. It releases the GIL, runs inside `catch_unwind`, reports the "Attributes" stage, and checks for cancel.
- **The result always lands.** If the mapping or applying an attribute fails, the result is still created, and a warning says what was lost.
- **Stale references.** The snapshot keeps names, not Blender references, so a material or armature deleted mid-run is skipped instead of crashing.
- **Memory.** The extra snapshot data is only copied when Keep Attributes is on.

### UI

**Keep Attributes** in the main panel (on by default). An **Attributes** sub-panel (collapsed) holds UV Maps, Materials, Vertex Groups, Colors and Other Attributes, Follow UV Islands and Bind to Rig. None of them is preset-controlled, as with symmetry. The native API goes to 8 (new function, new stage).

## Evidence

- **Core tests (`transfer/tests.rs`).** Samples reproduce the points they map. Corners stay on their side of a seam, including one that doesn't split the mesh. A thin part doesn't borrow its far side. A thin strip doesn't win a face by its centre. Bad input is refused, and cancel works.
- **Python tests.** The native contract (buffers, keys, errors, cancel), and the seam rule (a plain seam, a dead-end cut, a pole fan, −0.0 keys).
- **Blender scenario `attributes_are_kept`.** A posed, rigged UV sphere with two UV maps, two materials and a weight gradient is checked end to end:
  - the result is remeshed in the rest shape;
  - planar UVs are within 0.05;
  - no face away from the poles spans the UVMap seam;
  - seams are marked;
  - materials are right on every face clear of the equator;
  - weights are within 0.05;
  - the result is parented, with the Armature modifier copied;
  - the source's modifier is back on;
  - with Keep Attributes off, the result is a bare mesh.

## Limits

- Shape keys and edge attributes aren't kept (above).
- Follow UV Islands adds the render map's seams as feature lines. That changes the layout of UV-mapped meshes, and the opt-in Quad Shape solver can fall back with them (the torus primitive does). Turn Follow UV Islands off there; corners are still taken from the right side of each seam.

## Alternatives rejected

- **Blender's Data Transfer modifier.** Seam-unaware (above), and applying it needs `bpy.ops` with a context override from a modal operator.
- **Transfer inside the pipeline.** The pipeline would need every attribute as input, and the output mesh would have to be decided twice. Mapping the final mesh after the run keeps the core attribute-agnostic and Quick Re-run untouched.
- **Splitting result faces along seams.** That would break the all-quad result. Following islands in the layout gives the same exactness without it.
