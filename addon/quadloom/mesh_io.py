# SPDX-License-Identifier: GPL-3.0-or-later
"""Blender mesh data <-> flat buffers. Main thread only."""

import json
from array import array
from dataclasses import dataclass

import bpy
from mathutils import Matrix

RESULT_SUFFIX = "_QuadLoom"


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


def extract(obj, depsgraph):
    """Snapshots the evaluated (modifiers applied) mesh of `obj` as triangles."""
    evaluated = obj.evaluated_get(depsgraph)
    mesh = evaluated.to_mesh()
    try:
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


def build_result(snapshot, result, scene):
    """Creates a new object from a run result. Never touches the source object."""
    positions = array("f")
    positions.frombytes(result["positions"])
    offsets = array("I")
    offsets.frombytes(result["face_offsets"])
    corners = array("I")
    corners.frombytes(result["face_indices"])

    name = snapshot.source_name + RESULT_SUFFIX
    mesh = bpy.data.meshes.new(name)
    mesh.vertices.add(len(positions) // 3)
    mesh.vertices.foreach_set("co", positions)
    mesh.loops.add(len(corners))
    mesh.loops.foreach_set("vertex_index", array("i", corners))
    mesh.polygons.add(max(len(offsets) - 1, 0))
    mesh.polygons.foreach_set("loop_start", array("i", offsets[:-1]))
    mesh.update(calc_edges=True)

    obj = bpy.data.objects.new(name, mesh)
    obj.matrix_world = snapshot.matrix_world
    # The source's collection, unless it was the scene's master collection
    # (not in bpy.data) or has been deleted since the snapshot.
    collection = bpy.data.collections.get(snapshot.collection_name or "") or scene.collection
    collection.objects.link(obj)
    obj["quadloom_report"] = json.dumps(dict(result["validation"]))
    return obj
