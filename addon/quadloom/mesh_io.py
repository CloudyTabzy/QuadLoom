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

    @property
    def face_count(self):
        return len(self.indices) // 3


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
    finally:
        evaluated.to_mesh_clear()
    collections = obj.users_collection
    return MeshSnapshot(
        source_name=obj.name,
        collection_name=collections[0].name if collections else None,
        matrix_world=obj.matrix_world.copy(),
        positions=positions,
        indices=indices,
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
    return positions, offsets, corners


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

        obj = bpy.data.objects.new(name, mesh)
        obj.matrix_world = snapshot.matrix_world
        _target_collection(snapshot, scene).objects.link(obj)
        obj["quadloom_report"] = json.dumps(dict(result["validation"]))
    except Exception:
        if obj is not None:
            bpy.data.objects.remove(obj)
        bpy.data.meshes.remove(mesh)
        raise
    return obj
