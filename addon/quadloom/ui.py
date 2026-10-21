# SPDX-License-Identifier: GPL-3.0-or-later
"""The QuadLoom N-panel (3D Viewport > Sidebar > QuadLoom)."""

import bpy

from . import bridge, operators

STATUS_ICONS = {
    "NONE": "INFO",
    "RUNNING": "SORTTIME",
    "OK": "CHECKMARK",
    "ERROR": "ERROR",
    "CANCELLED": "CANCEL",
}


class QUADLOOM_PT_main(bpy.types.Panel):
    bl_space_type = "VIEW_3D"
    bl_region_type = "UI"
    bl_category = "QuadLoom"
    bl_label = "QuadLoom"

    def draw(self, context):
        layout = self.layout
        if not bridge.available():
            box = layout.box()
            box.alert = True
            box.label(text="Backend unavailable", icon="ERROR")
            for line in _wrap(bridge.unavailable_reason or "unknown reason"):
                box.label(text=line)
            return

        row = layout.row(align=True)
        row.scale_y = 1.4
        if operators.job_running():
            row.operator("quadloom.cancel", icon="CANCEL")
        else:
            row.operator("quadloom.remesh", icon="MOD_REMESH")

        last = context.scene.quadloom_last_run
        box = layout.box()
        box.label(text=f"Last run: {last.bl_rna.properties['status'].enum_items[last.status].name}",
                  icon=STATUS_ICONS.get(last.status, "INFO"))
        if last.status != "NONE":
            col = box.column(align=True)
            for line in _wrap(last.message):
                col.label(text=line)
            if last.stage and last.status in {"ERROR", "CANCELLED"}:
                col.label(text=f"Stage: {last.stage}")
            if last.hint:
                for line in _wrap(f"Hint: {last.hint}"):
                    col.label(text=line, icon="LIGHT" if line.startswith("Hint") else "BLANK1")
            if last.status == "OK":
                col.label(text=f"Faces: {last.faces_in} → {last.faces_out}")
                for summary, icon in (
                    (last.repair_summary, "TOOL_SETTINGS"),
                    (last.feature_summary, "EDGESEL"),
                    (last.remesh_summary, "MOD_REMESH"),
                ):
                    for n, line in enumerate(_wrap(summary)):
                        col.label(text=line, icon=icon if n == 0 else "BLANK1")
            if last.status != "RUNNING":
                col.label(text=f"Time: {last.elapsed_ms:.0f} ms")
        layout.label(text=f"Backend {bridge.version()}", icon="SETTINGS")


class _SubPanel:
    bl_space_type = "VIEW_3D"
    bl_region_type = "UI"
    bl_category = "QuadLoom"
    bl_parent_id = "QUADLOOM_PT_main"
    bl_options = {"DEFAULT_CLOSED"}

    @classmethod
    def poll(cls, context):
        return bridge.available()


class QUADLOOM_PT_cleanup(_SubPanel, bpy.types.Panel):
    bl_label = "Cleanup"

    def draw(self, context):
        settings = context.scene.quadloom
        layout = self.layout
        layout.use_property_split = True
        layout.prop(settings, "weld_distance_rel")
        layout.prop(settings, "remove_degenerate")
        layout.prop(settings, "split_non_manifold")
        layout.prop(settings, "orient")
        layout.prop(settings, "min_component_faces")


class QUADLOOM_PT_remesh(_SubPanel, bpy.types.Panel):
    bl_label = "Prep Remesh"

    def draw_header(self, context):
        self.layout.prop(context.scene.quadloom, "remesh_enabled", text="")

    def draw(self, context):
        settings = context.scene.quadloom
        layout = self.layout
        layout.use_property_split = True
        layout.active = settings.remesh_enabled
        layout.prop(settings, "remesh_target_rel")
        layout.prop(settings, "remesh_iterations")
        layout.prop(settings, "remesh_adaptive")
        col = layout.column(align=True)
        col.active = settings.remesh_adaptive
        col.prop(settings, "remesh_min_mult")
        col.prop(settings, "remesh_max_mult")
        layout.prop(settings, "remesh_max_deviation_rel")


class QUADLOOM_PT_features(_SubPanel, bpy.types.Panel):
    bl_label = "Sharp Features"

    def draw(self, context):
        settings = context.scene.quadloom
        layout = self.layout
        layout.use_property_split = True
        row = layout.row(heading="By Angle")
        row.prop(settings, "use_angle", text="")
        sub = row.row()
        sub.active = settings.use_angle
        sub.prop(settings, "angle_deg", text="")
        col = layout.column(heading="Sources", align=True)
        col.prop(settings, "use_marked_sharp")
        col.prop(settings, "use_seams")
        col.prop(settings, "use_material_boundaries")
        col.prop(settings, "use_face_set_boundaries")
        col.prop(settings, "use_open_boundaries")
        layout.prop(settings, "prune_steps")


class QUADLOOM_PT_advanced(bpy.types.Panel):
    bl_space_type = "VIEW_3D"
    bl_region_type = "UI"
    bl_category = "QuadLoom"
    bl_label = "Advanced"
    bl_parent_id = "QUADLOOM_PT_main"
    bl_options = {"DEFAULT_CLOSED"}

    @classmethod
    def poll(cls, context):
        return bridge.available()

    def draw(self, context):
        settings = context.scene.quadloom
        layout = self.layout
        layout.use_property_split = True
        layout.prop(settings, "max_faces")
        if bridge.build_info().get("fault_injection"):
            box = layout.box()
            box.label(text="Developer build", icon="EXPERIMENTAL")
            box.prop(settings, "debug_fault")


def _wrap(text, width=38):
    words, lines, line = text.split(), [], ""
    for word in words:
        if line and len(line) + 1 + len(word) > width:
            lines.append(line)
            line = word
        else:
            line = f"{line} {word}".strip()
    if line:
        lines.append(line)
    return lines or [""]


classes = (QUADLOOM_PT_main, QUADLOOM_PT_cleanup, QUADLOOM_PT_features, QUADLOOM_PT_remesh, QUADLOOM_PT_advanced)
