# SPDX-License-Identifier: GPL-3.0-or-later
"""Scene-level settings and the record of the last run."""

import bpy
from bpy.props import EnumProperty, FloatProperty, IntProperty, PointerProperty, StringProperty

DEFAULT_MAX_FACES = 2_000_000


class QUADLOOM_PG_settings(bpy.types.PropertyGroup):
    max_faces: IntProperty(
        name="Face Limit",
        description="Inputs with more triangles than this are refused before any work starts. "
        "Raise it for very dense meshes if your machine has the memory",
        default=DEFAULT_MAX_FACES,
        min=1,
    )
    debug_fault: StringProperty(
        name="Inject Fault",
        description="Developer builds only: panic:<stage>, error:<stage> or slow:<ms>",
        default="",
    )


class QUADLOOM_PG_last_run(bpy.types.PropertyGroup):
    status: EnumProperty(
        name="Status",
        items=[
            ("NONE", "Not run", ""),
            ("RUNNING", "Running", ""),
            ("OK", "Finished", ""),
            ("ERROR", "Failed", ""),
            ("CANCELLED", "Cancelled", ""),
        ],
        default="NONE",
    )
    message: StringProperty(name="Message")
    stage: StringProperty(name="Stage")
    hint: StringProperty(name="Hint")
    faces_in: IntProperty(name="Faces In")
    faces_out: IntProperty(name="Faces Out")
    elapsed_ms: FloatProperty(name="Time (ms)")
    report_json: StringProperty(name="Report")


classes = (QUADLOOM_PG_settings, QUADLOOM_PG_last_run)


def register_props():
    bpy.types.Scene.quadloom = PointerProperty(type=QUADLOOM_PG_settings)
    bpy.types.Scene.quadloom_last_run = PointerProperty(type=QUADLOOM_PG_last_run)


def unregister_props():
    for name in ("quadloom_last_run", "quadloom"):
        if hasattr(bpy.types.Scene, name):
            delattr(bpy.types.Scene, name)
