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
                    (last.field_summary, "ORIENTATION_GIMBAL"),
                    (last.frame_summary, "MOD_SIMPLEDEFORM"),
                    (last.trace_summary, "MOD_LATTICE"),
                    (last.quad_summary, "MESH_GRID"),
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


class QUADLOOM_PT_field(_SubPanel, bpy.types.Panel):
    bl_label = "Cross Field"

    def draw_header(self, context):
        self.layout.prop(context.scene.quadloom, "field_enabled", text="")

    def draw(self, context):
        settings = context.scene.quadloom
        layout = self.layout
        layout.use_property_split = True
        layout.active = settings.field_enabled
        layout.prop(settings, "field_curvature")
        col = layout.column(align=True)
        col.active = settings.field_curvature != "OFF"
        col.prop(settings, "field_alignment_weight")
        sub = col.column(align=True)
        sub.active = settings.field_curvature == "AUTO"
        sub.prop(settings, "field_sharp_factor")


class QUADLOOM_PT_shape(_SubPanel, bpy.types.Panel):
    bl_label = "Quad Shape"

    def draw_header(self, context):
        self.layout.prop(context.scene.quadloom, "frame_enabled", text="")

    def draw(self, context):
        settings = context.scene.quadloom
        layout = self.layout
        layout.use_property_split = True
        layout.active = settings.frame_enabled and settings.field_enabled
        if not settings.field_enabled:
            layout.label(text="Needs the Cross Field", icon="INFO")
        layout.prop(settings, "frame_balance")
        layout.prop(settings, "frame_curvature_stretch")
        col = layout.column()
        col.active = settings.frame_curvature_stretch
        col.prop(settings, "frame_max_stretch")


class QUADLOOM_PT_layout(_SubPanel, bpy.types.Panel):
    bl_label = "Patch Layout"

    def draw_header(self, context):
        self.layout.prop(context.scene.quadloom, "trace_enabled", text="")

    def draw(self, context):
        settings = context.scene.quadloom
        layout = self.layout
        layout.use_property_split = True
        layout.active = settings.trace_enabled and settings.field_enabled
        layout.prop(settings, "trace_max_sides")
        layout.prop(settings, "trace_match_valence")
        layout.prop(settings, "trace_cc_tolerance")
        col = layout.column(align=True)
        col.prop(settings, "trace_cost")
        col.prop(settings, "trace_drift")
        col = layout.column(align=True)
        col.prop(settings, "trace_loop_density")
        col.prop(settings, "trace_sub_passes")


class QUADLOOM_PT_quads(_SubPanel, bpy.types.Panel):
    bl_label = "Quads"

    def draw_header(self, context):
        self.layout.prop(context.scene.quadloom, "quad_enabled", text="")

    def draw(self, context):
        settings = context.scene.quadloom
        layout = self.layout
        layout.use_property_split = True
        layout.active = settings.quad_enabled and settings.trace_enabled and settings.field_enabled
        if not (settings.trace_enabled and settings.field_enabled):
            layout.label(text="Needs the Cross Field and Patch Layout", icon="INFO")
        layout.prop(settings, "quad_size")


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


classes = (
    QUADLOOM_PT_main,
    QUADLOOM_PT_cleanup,
    QUADLOOM_PT_features,
    QUADLOOM_PT_remesh,
    QUADLOOM_PT_field,
    QUADLOOM_PT_shape,
    QUADLOOM_PT_layout,
    QUADLOOM_PT_quads,
    QUADLOOM_PT_advanced,
)
