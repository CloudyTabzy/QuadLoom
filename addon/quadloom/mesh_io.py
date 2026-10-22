# SPDX-License-Identifier: GPL-3.0-or-later
"""Blender mesh data <-> flat buffers. Main thread only."""

import json
from array import array
from dataclasses import dataclass

import bpy
from mathutils import Matrix

RESULT_SUFFIX = "_QuadLoom"


class MeshTooLarge(ValueError):
    """The evaluated mesh is over the configured limits; refused before copying."""


class EmptyMesh(ValueError):
    """The evaluated mesh has no faces (even after modifiers)."""


@dataclass
class MeshSnapshot:
    """Everything a run needs, copied out of Blender so the source can change freely."""

    source_name: str
    collection_name: str | None
    matrix_world: Matrix
    positions: array  # float32 xyz, object space
    indices: array  # int32 triangle corners
    sharp_edges: array  # int32 vertex pairs of edges marked sharp
    seam_edges: array  # int32 vertex pairs of UV seams
    material: array  # int32 material index per triangle
    face_set: array | None  # int32 sculpt face set per triangle, if the mesh has them

    @property
    def face_count(self):
        return len(self.indices) // 3

    def feature_kwargs(self):
        kwargs = {"sharp_edges": self.sharp_edges, "seam_edges": self.seam_edges, "material": self.material}
        if self.face_set is not None:
            kwargs["face_set"] = self.face_set
        return kwargs


def _flagged_edge_pairs(mesh, prop):
    """Vertex pairs of the edges where boolean edge property `prop` is set."""
    import numpy as np

    count = len(mesh.edges)
    flags = np.zeros(count, dtype=bool)
    mesh.edges.foreach_get(prop, flags)
    vertices = np.zeros(count * 2, dtype=np.int32)
    mesh.edges.foreach_get("vertices", vertices)
    return array("i", vertices.reshape(-1, 2)[flags].ravel().tobytes())


def _per_triangle(mesh, polygon_values):
    """Spreads a per-polygon int array onto loop triangles."""
    import numpy as np

    polygon_of = np.zeros(len(mesh.loop_triangles), dtype=np.int32)
    mesh.loop_triangles.foreach_get("polygon_index", polygon_of)
    return array("i", np.asarray(polygon_values, dtype=np.int32)[polygon_of].tobytes())


def extract(obj, depsgraph, max_faces):
    """Snapshots the evaluated (modifiers applied) mesh of `obj` as triangles.

    Size limits are checked from counts alone, before triangulating or copying,
    so a huge modifier result is refused without freezing the UI.
    """
    evaluated = obj.evaluated_get(depsgraph)
    mesh = evaluated.to_mesh()
    try:
        if len(mesh.polygons) == 0:
            raise EmptyMesh(f"{obj.name} has no faces (after modifiers)")
        # Fan triangulation turns an n-gon into n - 2 triangles.
        triangles = len(mesh.loops) - 2 * len(mesh.polygons)
        if triangles > max_faces:
            raise MeshTooLarge(f"mesh has {triangles} faces, above the limit {max_faces}")
        if len(mesh.vertices) > 3 * max_faces:
            raise MeshTooLarge(f"mesh has {len(mesh.vertices)} vertices, above the limit {3 * max_faces}")
        mesh.calc_loop_triangles()
        positions = array("f", bytes(4 * 3 * len(mesh.vertices)))
        mesh.vertices.foreach_get("co", positions)
        indices = array("i", bytes(4 * 3 * len(mesh.loop_triangles)))
        mesh.loop_triangles.foreach_get("vertices", indices)

        sharp_edges = _flagged_edge_pairs(mesh, "use_edge_sharp")
        seam_edges = _flagged_edge_pairs(mesh, "use_seam")
        polygon_material = array("i", bytes(4 * len(mesh.polygons)))
        mesh.polygons.foreach_get("material_index", polygon_material)
        material = _per_triangle(mesh, polygon_material)
        face_set = None
        attr = mesh.attributes.get(".sculpt_face_set")
        if attr is not None and attr.domain == "FACE" and attr.data_type == "INT":
            polygon_set = array("i", bytes(4 * len(mesh.polygons)))
            attr.data.foreach_get("value", polygon_set)
            face_set = _per_triangle(mesh, polygon_set)
    finally:
        evaluated.to_mesh_clear()
    collections = obj.users_collection
    return MeshSnapshot(
        source_name=obj.name,
        collection_name=collections[0].name if collections else None,
        matrix_world=obj.matrix_world.copy(),
        positions=positions,
        indices=indices,
        sharp_edges=sharp_edges,
        seam_edges=seam_edges,
        material=material,
        face_set=face_set,
    )


def decode_result(result):
    """Unpacks and checks a native result. Raises ValueError if it's inconsistent.

    This is the last line of defence before data reaches Blender's C code:
    a malformed index here would otherwise be a native crash, not an error.
    """
    positions = array("f")
    positions.frombytes(result["positions"])
    offsets = array("I")
    offsets.frombytes(result["face_offsets"])
    corners = array("I")
    corners.frombytes(result["face_indices"])
    if len(positions) % 3:
        raise ValueError("result positions are not xyz triples")
    vertex_count = len(positions) // 3
    if len(offsets) < 2 or offsets[0] != 0 or offsets[-1] != len(corners):
        raise ValueError("result face offsets don't cover the face indices")
    if any(b - a < 3 for a, b in zip(offsets, offsets[1:])):
        raise ValueError("result has a face with fewer than 3 corners")
    if corners and max(corners) >= vertex_count:
        raise ValueError("result references a vertex that doesn't exist")
    if _has_repeated_corner(offsets, corners, vertex_count):
        raise ValueError("result has a face that uses the same vertex twice")
    return positions, offsets, corners


def decode_layout(trace):
    """Unpacks and checks a result's patch layout (on its own refined mesh).

    Returns (positions, face offsets, face corners, patch ids, feature edges);
    raises ValueError if it's inconsistent.
    """
    positions = array("f")
    positions.frombytes(trace["positions"])
    corners = array("I")
    corners.frombytes(trace["triangles"])
    patch = array("I")
    patch.frombytes(trace["patch_of_face"])
    if len(positions) % 3 or len(corners) % 3:
        raise ValueError("layout mesh buffers are not triples")
    face_count = len(corners) // 3
    if len(patch) != face_count:
        raise ValueError("layout has a patch id count different from its face count")
    offsets = array("I", range(0, 3 * face_count + 1, 3))
    vertex_count = len(positions) // 3
    if corners and max(corners) >= vertex_count:
        raise ValueError("layout references a vertex that doesn't exist")
    if _has_repeated_corner(offsets, corners, vertex_count):
        raise ValueError("layout has a face that uses the same vertex twice")
    return positions, offsets, corners, patch, trace.get("feature_edges", b"")


def _has_repeated_corner(offsets, corners, vertex_count):
    import numpy as np

    sizes = np.diff(np.frombuffer(offsets, dtype=np.uint32).astype(np.int64))
    face_of = np.repeat(np.arange(len(sizes), dtype=np.int64), sizes)
    keys = face_of * max(vertex_count, 1) + np.frombuffer(corners, dtype=np.uint32).astype(np.int64)
    return len(np.unique(keys)) != len(keys)


def _mark_feature_edges(mesh, feature_edges):
    """Shows the detected feature lines as the result's Sharp edges."""
    import numpy as np

    pairs = np.frombuffer(feature_edges, dtype="<u4").reshape(-1, 2)
    if len(pairs) == 0:
        return
    vertices = np.zeros(len(mesh.edges) * 2, dtype=np.int32)
    mesh.edges.foreach_get("vertices", vertices)
    edges = np.sort(vertices.reshape(-1, 2), axis=1)
    wanted = {(int(a), int(b)) for a, b in np.sort(pairs.astype(np.int64), axis=1)}
    flags = np.fromiter(((int(a), int(b)) in wanted for a, b in edges), dtype=bool, count=len(edges))
    attr = mesh.attributes.get("sharp_edge") or mesh.attributes.new("sharp_edge", "BOOLEAN", "EDGE")
    attr.data.foreach_set("value", flags)


def _target_collection(snapshot, scene):
    """The source's collection if it's still part of this scene and editable,
    else the scene's master collection."""
    collection = bpy.data.collections.get(snapshot.collection_name or "")
    if collection is None or collection.library is not None:
        return scene.collection
    if collection not in scene.collection.children_recursive:
        return scene.collection
    return collection


def build_result(snapshot, result, scene):
    """Creates a new object from a run result. Never touches the source object.

    Raises ValueError (without leaving partial data behind) if the result is malformed.
    """
    positions, offsets, corners = decode_result(result)
    feature_edges, patch = result.get("feature_edges", b""), None
    if result.get("trace"):
        # Until quads exist, the result shows the layout: its (refined) mesh,
        # with each triangle's patch as the integer face attribute "quadloom_patch".
        positions, offsets, corners, patch, feature_edges = decode_layout(result["trace"])
    name = snapshot.source_name + RESULT_SUFFIX
    mesh = bpy.data.meshes.new(name)
    obj = None
    try:
        mesh.vertices.add(len(positions) // 3)
        mesh.vertices.foreach_set("co", positions)
        mesh.loops.add(len(corners))
        mesh.loops.foreach_set("vertex_index", array("i", corners))
        mesh.polygons.add(len(offsets) - 1)
        mesh.polygons.foreach_set("loop_start", array("i", offsets[:-1]))
        mesh.update(calc_edges=True)
        _mark_feature_edges(mesh, feature_edges)
        if patch is not None:
            attr = mesh.attributes.new("quadloom_patch", "INT", "FACE")
            attr.data.foreach_set("value", array("i", patch))

        obj = bpy.data.objects.new(name, mesh)
        obj.matrix_world = snapshot.matrix_world
        _target_collection(snapshot, scene).objects.link(obj)
        obj["quadloom_report"] = json.dumps(
            {key: dict(result[key]) for key in ("validation", "repair", "features") if key in result}
            | ({"trace": dict(result["trace"]["report"])} if result.get("trace") else {})
        )
    except Exception:
        if obj is not None:
            bpy.data.objects.remove(obj)
        bpy.data.meshes.remove(mesh)
        raise
    return obj
