# SPDX-License-Identifier: GPL-3.0-or-later
"""Attribute transfer (decision 0019): UV maps, colours and other attributes,
materials, vertex groups and the rig, from the source onto the result.

`collect` copies what's wanted out of the source at snapshot time (main
thread). The native `transfer_map` then maps the result onto the source on
the worker thread, and `apply` puts the values on the result (main thread).
Only names are kept, never Blender references, so data deleted mid-run is
skipped instead of crashing.
"""

from dataclasses import dataclass, field

# data_type: (foreach key, components, NumPy dtype, interpolated)
TYPES = {
    "FLOAT": ("value", 1, "float32", True),
    "FLOAT2": ("vector", 2, "float32", True),
    "FLOAT_VECTOR": ("vector", 3, "float32", True),
    "FLOAT4": ("vector", 4, "float32", True),
    "FLOAT_COLOR": ("color", 4, "float32", True),
    "BYTE_COLOR": ("color", 4, "float32", True),
    "QUATERNION": ("value", 4, "float32", False),
    "FLOAT4X4": ("value", 16, "float32", False),
    "INT": ("value", 1, "int32", False),
    "INT8": ("value", 1, "int32", False),
    "INT16_2D": ("value", 2, "int32", False),
    "INT32_2D": ("value", 2, "int32", False),
    "BOOLEAN": ("value", 1, "bool", False),
}

# Built-in attributes that are geometry or that the result sets itself.
SKIP = frozenset({"position", "material_index", "custom_normal", "sharp_edge"})
# Internal (dot-prefixed) attributes worth keeping.
KEEP_INTERNAL = frozenset({".sculpt_face_set"})
# Weights below this are dropped.
MIN_WEIGHT = 1e-6


@dataclass
class Options:
    uvs: bool = True
    materials: bool = True
    vertex_groups: bool = True
    attributes: bool = True
    follow_uv_islands: bool = True
    rig: bool = True

    @classmethod
    def from_settings(cls, settings):
        return cls(
            uvs=settings.keep_uvs,
            materials=settings.keep_materials,
            vertex_groups=settings.keep_vertex_groups,
            attributes=settings.keep_other_attributes,
            follow_uv_islands=settings.follow_uv_islands,
            rig=settings.bind_to_rig,
        )


@dataclass
class Attribute:
    name: str
    domain: str  # POINT, CORNER or FACE
    data_type: str
    values: object  # NumPy array, (elements, components)


@dataclass
class TransferData:
    """What the result gets from the source, copied out at snapshot time."""

    tri_loops: object  # (triangles, 3) int32: each triangle corner's source loop
    tri_polygons: object  # (triangles,) int32: each triangle's source polygon
    attributes: list = field(default_factory=list)
    uv_active: str | None = None
    uv_render: str | None = None
    color_active: str | None = None
    color_default: str | None = None
    materials: list | None = None  # material names in slot order (None for an empty slot)
    groups: list = field(default_factory=list)  # (name, vertex indices, weights)
    rig: dict | None = None
    corner_keys: object = None  # uint32, 3 per triangle, or None
    face_keys: object = None  # uint32, 1 per triangle, or None

    def is_empty(self):
        return not (self.attributes or self.materials or self.groups or self.rig)


def armature_modifiers(obj):
    """The source's Armature modifiers that deform it in the viewport."""
    return [m for m in obj.modifiers if m.type == "ARMATURE" and m.show_viewport]


def collect(obj, mesh, options, resting=()):
    """Copies the wanted data out of `mesh` (the evaluated mesh of `obj`, with
    loop triangles calculated). `resting` names the Armature modifiers switched
    off for the snapshot. Returns a TransferData, or None if there's nothing."""
    import numpy as np

    triangles = len(mesh.loop_triangles)
    tri_loops = np.zeros(triangles * 3, dtype=np.int32)
    mesh.loop_triangles.foreach_get("loops", tri_loops)
    tri_polygons = np.zeros(triangles, dtype=np.int32)
    mesh.loop_triangles.foreach_get("polygon_index", tri_polygons)
    data = TransferData(tri_loops=tri_loops.reshape(-1, 3), tri_polygons=tri_polygons)

    uv_names = {uv.name for uv in mesh.uv_layers}
    color_names = {c.name for c in mesh.color_attributes}
    for attr in mesh.attributes:
        name = attr.name
        if name in SKIP or name.startswith("quadloom_") or attr.domain not in ("POINT", "CORNER", "FACE"):
            continue
        if name.startswith(".") and name not in KEEP_INTERNAL:
            continue
        if attr.data_type not in TYPES:
            continue
        wanted = options.uvs if name in uv_names else options.attributes
        if not wanted:
            continue
        key, width, dtype, _ = TYPES[attr.data_type]
        values = np.zeros(len(attr.data) * width, dtype=dtype)
        attr.data.foreach_get(key, values)
        data.attributes.append(Attribute(name, attr.domain, attr.data_type, values.reshape(-1, width)))
    if options.uvs and mesh.uv_layers:
        active = mesh.uv_layers.active
        data.uv_active = active.name if active else None
        data.uv_render = next((uv.name for uv in mesh.uv_layers if uv.active_render), data.uv_active)
    if options.attributes and color_names:
        data.color_active = mesh.attributes.active_color_name or None
        data.color_default = mesh.attributes.default_color_name or None

    if options.materials and len(obj.material_slots) > 0:
        data.materials = [slot.material.name if slot.material else None for slot in obj.material_slots]

    if options.vertex_groups and len(obj.vertex_groups) > 0:
        data.groups = _vertex_groups(obj, mesh)

    if options.rig:
        data.rig = _rig(obj, resting)

    data.corner_keys = _corner_keys(data, mesh)
    data.face_keys = _face_keys(data, mesh)
    return None if data.is_empty() else data


def _vertex_groups(obj, mesh):
    """Each group's (name, vertex indices, weights), read once over the vertices."""
    import numpy as np

    count = len(obj.vertex_groups)
    verts = [[] for _ in range(count)]
    weights = [[] for _ in range(count)]
    for v in mesh.vertices:
        for g in v.groups:
            if g.group < count and g.weight > 0.0:
                verts[g.group].append(v.index)
                weights[g.group].append(g.weight)
    return [
        (group.name, np.asarray(verts[i], dtype=np.int32), np.asarray(weights[i], dtype=np.float32))
        for i, group in enumerate(obj.vertex_groups)
    ]


def _rig(obj, resting):
    """The parent and the deforming Armature modifiers (those in `resting`, which
    are switched off for the snapshot), by name. None if there's neither."""
    modifiers = []
    for m in (m for m in obj.modifiers if m.type == "ARMATURE" and m.name in resting):
        props = {}
        for prop in m.bl_rna.properties:
            if prop.is_readonly or prop.identifier in ("name", "is_active", "show_expanded"):
                continue
            value = getattr(m, prop.identifier)
            props[prop.identifier] = value.name if prop.identifier == "object" and value is not None else value
        props["show_viewport"] = True
        modifiers.append({"name": m.name, "props": props})
    if obj.parent is None and not modifiers:
        return None
    rig = {"modifiers": modifiers}
    if obj.parent is not None:
        rig.update(
            parent=obj.parent.name,
            parent_type=obj.parent_type,
            parent_bone=obj.parent_bone,
            matrix_parent_inverse=obj.matrix_parent_inverse.copy(),
            matrix_basis=obj.matrix_basis.copy(),
        )
    return rig


def _unique_rows(rows):
    """Ids such that equal rows (bitwise, after folding -0.0 into 0.0) share one."""
    import numpy as np

    rows = np.ascontiguousarray(rows + 0.0)
    view = rows.view(np.dtype((np.void, rows.dtype.itemsize * rows.shape[1]))).ravel()
    return np.unique(view, return_inverse=True)[1].astype(np.uint32).ravel()


def _corner_keys(data, mesh):
    """Per triangle corner: one id per (vertex, every corner value)."""
    import numpy as np

    corner = [a.values.astype(np.float64) for a in data.attributes if a.domain == "CORNER"]
    if not corner:
        return None
    vertex = np.zeros(len(mesh.loops), dtype=np.int32)
    mesh.loops.foreach_get("vertex_index", vertex)
    keys = _unique_rows(np.column_stack([vertex.astype(np.float64)] + corner))
    return keys[data.tri_loops].ravel()


def _face_keys(data, mesh):
    """Per triangle: one id per combination of its polygon's face values."""
    import numpy as np

    columns = [a.values.astype(np.float64) for a in data.attributes if a.domain == "FACE"]
    if data.materials and len(data.materials) > 1:
        material = np.zeros(len(mesh.polygons), dtype=np.int32)
        mesh.polygons.foreach_get("material_index", material)
        columns.append(material.astype(np.float64)[:, None])
    if not columns:
        return None
    keys = _unique_rows(np.column_stack(columns))
    return keys[data.tri_polygons]


def uv_island_borders(mesh, uv_name):
    """Vertex pairs of the edges where UV map `uv_name` jumps (its island
    borders), as an int32 buffer."""
    import numpy as np
    from array import array

    uv_layer = mesh.uv_layers.get(uv_name)
    if uv_layer is None or len(mesh.polygons) == 0:
        return array("i")
    uv = np.zeros(len(mesh.loops) * 2, dtype=np.float32)
    uv_layer.uv.foreach_get("vector", uv)
    uv = uv.reshape(-1, 2)
    vertex = np.zeros(len(mesh.loops), dtype=np.int64)
    mesh.loops.foreach_get("vertex_index", vertex)
    start = np.zeros(len(mesh.polygons), dtype=np.int64)
    total = np.zeros(len(mesh.polygons), dtype=np.int64)
    mesh.polygons.foreach_get("loop_start", start)
    mesh.polygons.foreach_get("loop_total", total)
    borders = _jumps(vertex, uv, start, total)
    return array("i", borders.astype(np.int32).ravel().tobytes())


def _jumps(vertex, values, start, total, tolerance=1e-6):
    """Seam edges, as sorted vertex pairs ((n, 2)): edges shared by faces
    whose corner values differ there. `vertex`/`values` per corner; faces as
    `start`/`total` into them.

    An edge whose values differ at both ends is a seam. One that differs at
    only one end is a seam's last edge (where a cut stops inside an island)
    only if that vertex is on at most two such edges; a vertex on more, like
    a UV sphere's pole with its fan of triangles, is a single point mapped
    many ways, not a seam.
    """
    import numpy as np

    count = len(vertex)
    nxt = np.arange(count) + 1
    nxt[start + total - 1] = start
    a, b = vertex, vertex[nxt]
    va, vb = values, values[nxt]
    swap = a > b
    lo, hi = np.where(swap, b, a), np.where(swap, a, b)
    vlo = np.where(swap[:, None], vb, va)
    vhi = np.where(swap[:, None], va, vb)
    order = np.lexsort((hi, lo))
    lo, hi, vlo, vhi = lo[order], hi[order], vlo[order], vhi[order]
    same = (lo[1:] == lo[:-1]) & (hi[1:] == hi[:-1])
    differs_lo = same & (np.abs(vlo[1:] - vlo[:-1]).max(axis=1) > tolerance)
    differs_hi = same & (np.abs(vhi[1:] - vhi[:-1]).max(axis=1) > tolerance)
    if not (differs_lo.any() or differs_hi.any()):
        return np.zeros((0, 2), np.int64)
    # One row per edge (a non-manifold edge compares several faces).
    n = int(vertex.max(initial=0)) + 1
    keys, inverse = np.unique(lo[1:][same] * n + hi[1:][same], return_inverse=True)
    dlo = np.bincount(inverse, weights=differs_lo[same].astype(np.float64), minlength=len(keys)) > 0
    dhi = np.bincount(inverse, weights=differs_hi[same].astype(np.float64), minlength=len(keys)) > 0
    elo, ehi = keys // n, keys % n
    jumping = dlo | dhi
    on = np.bincount(elo[jumping], minlength=n) + np.bincount(ehi[jumping], minlength=n)
    keep = (dlo & dhi) | (dlo & ~dhi & (on[elo] <= 2)) | (dhi & ~dlo & (on[ehi] <= 2))
    return np.column_stack([elo[keep], ehi[keep]])


def apply(obj, data, source_triangles, source_material, mapping):
    """Puts the transferred data on the result object `obj`. `source_triangles`
    is the snapshot's vertex indices (3 per triangle), `source_material` its
    material index per triangle, `mapping` the native transfer map. Returns
    warnings (message, hint) for anything skipped."""
    import bpy
    import numpy as np

    mesh = obj.data
    warnings = []
    vt = np.frombuffer(mapping["vertex_triangles"], dtype="<u4")
    vw = np.frombuffer(mapping["vertex_weights"], dtype="<f4").reshape(-1, 3)
    ct = np.frombuffer(mapping["corner_triangles"], dtype="<u4")
    cw = np.frombuffer(mapping["corner_weights"], dtype="<f4").reshape(-1, 3)
    ft = np.frombuffer(mapping["face_triangles"], dtype="<u4")
    if len(vt) != len(mesh.vertices) or len(ct) != len(mesh.loops) or len(ft) != len(mesh.polygons):
        raise ValueError("the attribute mapping doesn't match the result mesh")
    tris = np.asarray(source_triangles, dtype=np.int64).reshape(-1, 3)
    sources = {
        "POINT": (tris[vt], vw),
        "CORNER": (data.tri_loops[ct], cw),
    }
    face_polygon = data.tri_polygons[ft]

    for attr in data.attributes:
        key, width, dtype, interpolated = TYPES[attr.data_type]
        if attr.domain == "FACE":
            out = attr.values[face_polygon]
        else:
            index, weights = sources[attr.domain]
            if interpolated:
                out = np.einsum("nk,nkc->nc", weights, attr.values[index].astype(np.float32))
            else:
                out = attr.values[index[np.arange(len(index)), weights.argmax(axis=1)]]
        try:
            target = mesh.attributes.new(attr.name, attr.data_type, attr.domain)
        except RuntimeError:
            warnings.append((f'The attribute "{attr.name}" could not be created on the result', ""))
            continue
        target.data.foreach_set(key, np.ascontiguousarray(out, dtype=dtype).ravel())

    if data.uv_active and data.uv_active in mesh.uv_layers:
        mesh.uv_layers.active = mesh.uv_layers[data.uv_active]
    if data.uv_render and data.uv_render in mesh.uv_layers:
        mesh.uv_layers[data.uv_render].active_render = True
        _mark_seams(mesh, data.uv_render)
    if data.color_active and data.color_active in mesh.color_attributes:
        mesh.attributes.active_color_name = data.color_active
    if data.color_default and data.color_default in mesh.color_attributes:
        mesh.attributes.default_color_name = data.color_default

    if data.materials is not None:
        missing = [name for name in data.materials if name and bpy.data.materials.get(name) is None]
        for name in data.materials:
            mesh.materials.append(bpy.data.materials.get(name) if name else None)
        if missing:
            warnings.append((f"Materials deleted during the run were left empty: {', '.join(missing)}", ""))
        material = np.asarray(source_material, dtype=np.int32)[ft]
        mesh.polygons.foreach_set("material_index", np.clip(material, 0, max(len(data.materials) - 1, 0)))

    if data.groups:
        _apply_groups(obj, data.groups, tris[vt], vw)

    if data.rig:
        warnings += _apply_rig(obj, data.rig)
    return warnings


def _apply_groups(obj, groups, index, weights):
    import numpy as np

    source_count = int(index.max(initial=-1)) + 1
    for name, verts, values in groups:
        group = obj.vertex_groups.new(name=name)
        dense = np.zeros(max(source_count, int(verts.max(initial=-1)) + 1), dtype=np.float32)
        dense[verts] = values
        out = np.einsum("nk,nk->n", weights, dense[index])
        hit = np.flatnonzero(out > MIN_WEIGHT)
        if len(hit) == 0:
            continue
        # One call per distinct weight (large regions often share 1.0).
        unique, inverse = np.unique(out[hit], return_inverse=True)
        order = np.argsort(inverse, kind="stable")
        bounds = np.searchsorted(inverse[order], np.arange(len(unique) + 1))
        for u, weight in enumerate(unique):
            members = hit[order[bounds[u] : bounds[u + 1]]]
            group.add(members.tolist(), float(weight), "REPLACE")


def _apply_rig(obj, rig):
    import bpy

    warnings = []
    parent_name = rig.get("parent")
    if parent_name:
        parent = bpy.data.objects.get(parent_name)
        if parent is None:
            warnings.append((f'The parent "{parent_name}" was deleted during the run; the result is not parented', ""))
        else:
            obj.parent = parent
            obj.parent_type = rig["parent_type"]
            if rig["parent_type"] == "BONE":
                obj.parent_bone = rig["parent_bone"]
            obj.matrix_parent_inverse = rig["matrix_parent_inverse"]
            obj.matrix_basis = rig["matrix_basis"]
    for entry in rig["modifiers"]:
        modifier = obj.modifiers.new(entry["name"], "ARMATURE")
        for identifier, value in entry["props"].items():
            if identifier == "object":
                value = bpy.data.objects.get(value) if value else None
                if value is None and entry["props"]["object"]:
                    warnings.append((f'The armature "{entry["props"]["object"]}" was deleted during the run', ""))
                    continue
            try:
                setattr(modifier, identifier, value)
            except (AttributeError, TypeError, ValueError):
                pass
    return warnings


def _mark_seams(mesh, uv_name):
    """Marks the result's edges where UV map `uv_name` jumps as UV seams."""
    import numpy as np

    uv_layer = mesh.uv_layers.get(uv_name)
    if uv_layer is None:
        return
    uv = np.zeros(len(mesh.loops) * 2, dtype=np.float32)
    uv_layer.uv.foreach_get("vector", uv)
    vertex = np.zeros(len(mesh.loops), dtype=np.int64)
    mesh.loops.foreach_get("vertex_index", vertex)
    start = np.zeros(len(mesh.polygons), dtype=np.int64)
    total = np.zeros(len(mesh.polygons), dtype=np.int64)
    mesh.polygons.foreach_get("loop_start", start)
    mesh.polygons.foreach_get("loop_total", total)
    jumps = _jumps(vertex, uv.reshape(-1, 2), start, total)
    if len(jumps) == 0:
        return
    edges = np.zeros(len(mesh.edges) * 2, dtype=np.int64)
    mesh.edges.foreach_get("vertices", edges)
    edges = np.sort(edges.reshape(-1, 2), axis=1)
    n = len(mesh.vertices)
    edge_keys = edges[:, 0] * n + edges[:, 1]
    flags = np.isin(edge_keys, jumps[:, 0] * n + jumps[:, 1])
    mesh.edges.foreach_set("use_seam", flags)
