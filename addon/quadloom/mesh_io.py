# SPDX-License-Identifier: GPL-3.0-or-later
"""Blender mesh data <-> flat buffers. Main thread only."""

import json
from array import array
from dataclasses import dataclass

import bpy
from mathutils import Matrix

from . import transfer, worker_limits

RESULT_SUFFIX = "_QuadLoom"
BLENDER_INDEX_MAX = 2**31 - 1


def check_blender_counts(vertices, corners, faces):
    if any(count < 0 or count > BLENDER_INDEX_MAX for count in (vertices, corners, faces)):
        raise ValueError("mesh exceeds Blender's signed 32-bit index limits")


def _check_positions(positions):
    import numpy as np

    if not np.isfinite(np.frombuffer(positions, dtype=np.float32)).all():
        raise ValueError("mesh positions must be finite (no NaN or infinity)")


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
    transfer: object = None  # transfer.TransferData, when attributes are kept

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


def extract(obj, depsgraph, max_faces, keep=None):
    """Snapshots the evaluated (modifiers applied) mesh of `obj` as triangles.

    Size limits are checked from counts alone, before triangulating or copying,
    so a huge modifier result is refused without freezing the UI. With `keep`
    (transfer.Options) the attributes to transfer are copied too; binding to a
    rig snapshots the rest shape (Armature modifiers off for the moment).
    """
    rest = transfer.armature_modifiers(obj) if keep is not None and keep.rig else []
    for modifier in rest:
        modifier.show_viewport = False
    try:
        if rest:
            depsgraph = bpy.context.evaluated_depsgraph_get()
        return _extract(obj, depsgraph, max_faces, keep, [m.name for m in rest])
    finally:
        for modifier in rest:
            modifier.show_viewport = True


def _extract(obj, depsgraph, max_faces, keep, resting):
    evaluated = obj.evaluated_get(depsgraph)
    mesh = evaluated.to_mesh()
    kept = None
    try:
        if len(mesh.polygons) == 0:
            raise EmptyMesh(f"{obj.name} has no faces (after modifiers)")
        # Fan triangulation turns an n-gon into n - 2 triangles.
        triangles = len(mesh.loops) - 2 * len(mesh.polygons)
        if triangles > max_faces:
            raise MeshTooLarge(f"mesh has {triangles} faces, above the limit {max_faces}")
        if len(mesh.vertices) > 3 * max_faces:
            raise MeshTooLarge(f"mesh has {len(mesh.vertices)} vertices, above the limit {3 * max_faces}")
        check_blender_counts(len(mesh.vertices), 3 * triangles, triangles)
        # Snapshot, triangulation, sharp flags, NumPy scratch, and optional
        # attributes coexist. Also account for attribute layers, not just faces.
        estimate = len(mesh.vertices) * 96 + triangles * 256 + len(mesh.edges) * 24
        estimate += sum(len(attr.data) * 32 for attr in mesh.attributes) if keep is not None else 0
        worker_limits.admit_host_memory(estimate)
        mesh.calc_loop_triangles()
        positions = array("f", [0.0]) * (3 * len(mesh.vertices))
        mesh.vertices.foreach_get("co", positions)
        _check_positions(positions)
        indices = array("i", [0]) * (3 * len(mesh.loop_triangles))
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
        if keep is not None:
            kept = transfer.collect(obj, mesh, keep, resting)
            if kept is not None and keep.follow_uv_islands and kept.uv_render:
                # Quads follow the render UV map's islands, so none spans two.
                seam_edges.extend(transfer.uv_island_borders(mesh, kept.uv_render))
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
        transfer=kept,
    )


def decode_result(result):
    """Unpacks and checks a native result. Raises ValueError if it's inconsistent.

    This is the last line of defence before data reaches Blender's C code:
    a malformed index here would otherwise be a native crash, not an error.
    """
    check_blender_counts(len(result["positions"]) // 12, len(result["face_indices"]) // 4,
                         len(result["face_offsets"]) // 4 - 1)
    worker_limits.admit_host_memory(sum(len(result[key]) for key in ("positions", "face_offsets", "face_indices")) * 8)
    positions = array("f")
    positions.frombytes(result["positions"])
    offsets = array("I")
    offsets.frombytes(result["face_offsets"])
    corners = array("I")
    corners.frombytes(result["face_indices"])
    if len(positions) % 3:
        raise ValueError("result positions are not xyz triples")
    _check_positions(positions)
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


def final_geometry(result):
    """The result object's mesh as (positions, face offsets, face corners):
    the quads if there are any, else the patch layout, else the remeshed mesh
    (the same choice `build_result` makes)."""
    if result.get("quads"):
        return decode_quads(result["quads"])[:3]
    if result.get("trace"):
        return decode_layout(result["trace"])[:3]
    return decode_result(result)


def decode_quads(quads):
    """Unpacks and checks a result's quad mesh. Returns (positions, face offsets,
    face corners, patch ids); raises ValueError if it's inconsistent."""
    positions, offsets, corners = decode_result(quads)
    patch = array("I")
    patch.frombytes(quads["patch_of_face"])
    if len(patch) != len(offsets) - 1:
        raise ValueError("quad mesh patch ids don't match its faces")
    return positions, offsets, corners, patch


def frame_per_quad(frame, field_faces, trace, quad_patch):
    """The frame's size and stretch per quad: each layout patch's mean over its
    triangles (mapped to field faces through the layout's source faces)."""
    import numpy as np

    size, stretch = decode_frame(frame, field_faces, source_face=trace.get("source_face"))
    tri_patch = np.frombuffer(trace["patch_of_face"], dtype="<u4").astype(np.int64)
    if len(tri_patch) != len(size):
        raise ValueError("layout patch ids don't match its faces")
    quad_patch = np.asarray(quad_patch, dtype=np.int64)
    # A layout cannot have more patches than faces. Check before bincount:
    # an invalid id must never turn into an unbounded host allocation.
    if (tri_patch.size and (tri_patch.min() < 0 or tri_patch.max() >= field_faces)
            or quad_patch.size and (quad_patch.min() < 0 or quad_patch.max() >= field_faces)):
        raise ValueError("layout or quad patch id exceeds its patch count")
    patches = int(max(tri_patch.max(initial=0), quad_patch.max(initial=0))) + 1
    counts = np.bincount(tri_patch, minlength=patches).astype(np.float64)
    counts[counts == 0] = 1.0
    mean = [np.bincount(tri_patch, weights=np.asarray(v, dtype=np.float64), minlength=patches) / counts for v in (size, stretch)]
    return tuple(array("f", m[quad_patch].astype(np.float32).tobytes()) for m in mean)


def decode_frame(frame, field_faces, source_face=None, output_faces=None):
    """Unpacks a result's frame scales and maps them onto the output faces.

    `frame` holds `size` and `stretch` (float32), one per field-mesh face
    (`field_faces` of them). `source_face` (uint32, one per output triangle) names
    the field-mesh face each output face was split from; None means the output is
    the field mesh itself. Returns (size, stretch) as float32 arrays, one per output
    face. Raises ValueError if the buffers don't match.
    """
    size, stretch = array("f"), array("f")
    if not isinstance(frame.get("size"), bytes) or not isinstance(frame.get("stretch"), bytes):
        raise ValueError("frame scales are not byte buffers")
    size.frombytes(frame["size"])
    stretch.frombytes(frame["stretch"])
    if len(size) != field_faces or len(stretch) != field_faces:
        raise ValueError("frame scales don't have one value per field face")
    if source_face is None:
        if output_faces is not None and output_faces != field_faces:
            raise ValueError("frame scales don't match the output face count")
        return size, stretch
    if not isinstance(source_face, bytes):
        raise ValueError("frame source faces are not a byte buffer")
    sources = array("I")
    sources.frombytes(source_face)
    if output_faces is not None and len(sources) != output_faces:
        raise ValueError("frame source faces don't match the layout's face count")
    if sources and max(sources) >= field_faces:
        raise ValueError("frame source face refers to a face that doesn't exist")
    return array("f", (size[s] for s in sources)), array("f", (stretch[s] for s in sources))


def decode_layout(trace):
    """Unpacks and checks a result's patch layout (on its own refined mesh).

    Returns (positions, face offsets, face corners, patch ids, feature edges);
    raises ValueError if it's inconsistent.
    """
    check_blender_counts(len(trace["positions"]) // 12, len(trace["triangles"]) // 4,
                         len(trace["triangles"]) // 12)
    worker_limits.admit_host_memory((len(trace["positions"]) + len(trace["triangles"])) * 8)
    positions = array("f")
    positions.frombytes(trace["positions"])
    corners = array("I")
    corners.frombytes(trace["triangles"])
    patch = array("I")
    patch.frombytes(trace["patch_of_face"])
    if len(positions) % 3 or len(corners) % 3:
        raise ValueError("layout mesh buffers are not triples")
    _check_positions(positions)
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


def _frame_report(frame):
    """The frame's report fields, without the per-face byte buffers."""
    return {key: value for key, value in frame.items() if key not in ("size", "stretch")}


STEPS_COLLECTION = "QuadLoom Steps"


def _patch_colors(patch, offsets):
    """A distinct, stable color per patch id (golden-ratio hues), as RGBA on
    each face corner (Blender shows color attributes on points or corners)."""
    import colorsys

    colors = array("f")
    for face, p in enumerate(patch):
        rgba = (*colorsys.hsv_to_rgb((p * 0.6180339887) % 1.0, 0.55, 0.9), 1.0)
        colors.extend(rgba * (offsets[face + 1] - offsets[face]))
    return colors


def _steps_collection(snapshot, scene):
    """The "QuadLoom Steps" collection, made if needed and placed beside the result."""
    steps = bpy.data.collections.get(STEPS_COLLECTION)
    if steps is None or steps.library is not None:
        steps = bpy.data.collections.new(STEPS_COLLECTION)
    if steps not in scene.collection.children_recursive:
        _target_collection(snapshot, scene).children.link(steps)
    return steps


def build_steps(snapshot, result, scene):
    """The run's intermediate meshes as hidden objects in the "QuadLoom Steps"
    collection: the prep remesh (feature lines as Sharp edges), and the patch
    layout with each patch in its own color. A previous run's steps of the
    same object are replaced. Returns the new objects.
    """
    positions, offsets, corners = decode_result(result)
    meshes = [("Remesh", positions, offsets, corners, result.get("feature_edges", b""), None)]
    if result.get("trace"):
        positions, offsets, corners, patch, feature_edges = decode_layout(result["trace"])
        meshes.append(("Layout", positions, offsets, corners, feature_edges, patch))
    steps = _steps_collection(snapshot, scene)
    made = []
    allocated = []
    try:
        for label, positions, offsets, corners, feature_edges, patch in meshes:
            name = f"{snapshot.source_name}{RESULT_SUFFIX}_{label}"
            old = bpy.data.objects.get(name)
            if old is not None and old.get("quadloom_step"):
                old_mesh = old.data
                bpy.data.objects.remove(old)
                if old_mesh is not None and old_mesh.users == 0:
                    bpy.data.meshes.remove(old_mesh)
            mesh = bpy.data.meshes.new(name)
            allocated.append(mesh)
            mesh.vertices.add(len(positions) // 3)
            mesh.vertices.foreach_set("co", positions)
            mesh.loops.add(len(corners))
            mesh.loops.foreach_set("vertex_index", array("i", corners))
            mesh.polygons.add(len(offsets) - 1)
            mesh.polygons.foreach_set("loop_start", array("i", offsets[:-1]))
            mesh.update(calc_edges=True)
            _mark_feature_edges(mesh, feature_edges)
            if patch is not None:
                mesh.attributes.new("quadloom_patch", "INT", "FACE").data.foreach_set("value", array("i", patch))
                color = mesh.color_attributes.new("quadloom_patch_color", "FLOAT_COLOR", "CORNER")
                color.data.foreach_set("color", _patch_colors(patch, offsets))
                mesh.color_attributes.active_color = color
            obj = bpy.data.objects.new(name, mesh)
            made.append(obj)
            obj["quadloom_step"] = label
            obj.matrix_world = snapshot.matrix_world
            steps.objects.link(obj)
            obj.hide_render = True
            try:
                obj.hide_set(True)
            except RuntimeError:  # not in the active view layer: hidden there anyway
                pass
    except Exception:
        for obj in made:
            bpy.data.objects.remove(obj, do_unlink=True)
        for mesh in allocated:
            if mesh.users == 0:
                bpy.data.meshes.remove(mesh)
        raise
    return made


def build_result(snapshot, result, scene):
    """Creates a new object from a run result. Never touches the source object.

    Raises ValueError (without leaving partial data behind) if the result is malformed.
    """
    positions, offsets, corners = decode_result(result)
    field_faces = len(offsets) - 1
    feature_edges, patch = result.get("feature_edges", b""), None
    frame = None
    if result.get("trace"):
        # Without quads the result shows the layout: its (refined) mesh, with
        # each triangle's patch as the integer face attribute "quadloom_patch".
        positions, offsets, corners, patch, feature_edges = decode_layout(result["trace"])
    quads = result.get("quads")
    if quads:
        # The quad mesh is the result; its faces carry their layout patch.
        positions, offsets, corners, patch = decode_quads(quads)
        feature_edges = quads.get("feature_edges", b"")
        if result.get("frame") and result.get("trace"):
            frame = frame_per_quad(result["frame"], field_faces, result["trace"], patch)
    elif result.get("frame"):
        # The frame's per-face scales reach the layout's triangles through source_face.
        source = None
        if result.get("trace"):
            source = result["trace"].get("source_face")
            if source is None:
                raise ValueError("frame result has no layout source faces")
        frame = decode_frame(result["frame"], field_faces, source_face=source, output_faces=len(offsets) - 1)
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
        if frame is not None:
            for attr_name, values in zip(("quadloom_size", "quadloom_stretch"), frame):
                attr = mesh.attributes.new(attr_name, "FLOAT", "FACE")
                attr.data.foreach_set("value", values)

        obj = bpy.data.objects.new(name, mesh)
        obj.matrix_world = snapshot.matrix_world
        _target_collection(snapshot, scene).objects.link(obj)
        obj["quadloom_report"] = json.dumps(
            {key: dict(result[key]) for key in ("validation", "repair", "features") if key in result}
            | ({"trace": dict(result["trace"]["report"])} if result.get("trace") else {})
            | ({"frame": _frame_report(result["frame"])} if result.get("frame") else {})
            | ({"quads": dict(quads["report"])} if quads else {})
        )
    except Exception:
        if obj is not None:
            bpy.data.objects.remove(obj)
        bpy.data.meshes.remove(mesh)
        raise
    return obj
