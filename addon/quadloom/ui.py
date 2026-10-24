# SPDX-License-Identifier: GPL-3.0-or-later
"""The QuadLoom N-panel (3D Viewport > Sidebar > QuadLoom).

The main panel holds what an artist needs: a preset, a few plain settings,
the Remesh button and the result. Per-stage details and every technical
setting live in collapsed sub-panels ("Keep Attributes", "Details", "Advanced").
"""

import bpy

from . import bridge, operators, wording

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
            for line in _wrap(wording.sentence(bridge.unavailable_reason or "unknown reason")):
                box.label(text=line)
            return
        settings = context.scene.quadloom
        layout.use_property_split = True
        layout.use_property_decorate = False

        layout.prop(settings, "preset")
        col = layout.column(align=True)
        _sizing(col, settings)
        col.prop(settings, "quad_adaptivity")
        row = layout.row(heading="Sharp Edges")
        row.prop(settings, "use_angle", text="")
        sub = row.row()
        sub.active = settings.use_angle
        sub.prop(settings, "angle_deg", text="Angle")
        col = layout.column(heading="Symmetry")
        axes = col.row(align=True)
        axes.active = not settings.symmetry_auto
        axes.prop(settings, "symmetry_x", toggle=True)
        axes.prop(settings, "symmetry_y", toggle=True)
        axes.prop(settings, "symmetry_z", toggle=True)
        col.prop(settings, "symmetry_auto", text="Auto Detect")

        layout.separator()
        row = layout.row(align=True)
        row.scale_y = 1.6
        if operators.job_running():
            row.operator("quadloom.cancel", icon="CANCEL")
        else:
            row.operator("quadloom.remesh", icon="MOD_REMESH")
        _draw_result(layout, context.scene.quadloom_last_run)


def _sizing(layout, settings):
    layout.prop(settings, "sizing_mode")
    layout.prop(settings, "target_quads" if settings.sizing_mode == "TARGET" else "density")


def _draw_result(layout, last):
    """The outcome of the last run: status, the key numbers, warnings."""
    if last.status == "NONE":
        return
    box = layout.box()
    status = last.bl_rna.properties["status"].enum_items[last.status].name
    if last.status == "RUNNING":
        box.label(text=last.message or "Running…", icon=STATUS_ICONS["RUNNING"])
        box.label(text="Esc or Cancel to Stop", icon="BLANK1")
        return
    if last.status == "OK":
        seconds = last.elapsed_ms / 1000.0
        if last.quads_out == last.faces_out:
            headline = f"{last.quads_out:,} Quads in {seconds:.1f} s"
        else:
            headline = f"{last.faces_out:,} Faces ({last.quads_out:,} Quads) in {seconds:.1f} s"
        box.label(text=headline, icon=STATUS_ICONS["OK"])
        if last.target_quads:
            delta = 100.0 * (last.quads_out - last.target_quads) / last.target_quads
            box.label(text=f"Target {last.target_quads:,} Quads ({delta:+.1f}%)", icon="BLANK1")
        col = box.column(align=True)
        for line in _wrap(last.message):
            col.label(text=line, icon="BLANK1")
        if last.reused:
            col.label(text="Quick Re-run: Reused the Patch Layout", icon="FILE_REFRESH")
        if last.symmetry_summary:
            col.label(text=last.symmetry_summary, icon="MOD_MIRROR")
    else:
        box.alert = last.status == "ERROR"
        box.label(text=status, icon=STATUS_ICONS.get(last.status, "INFO"))
        col = box.column(align=True)
        for line in _wrap(wording.sentence(last.message)):
            col.label(text=line)
        if last.hint:
            for n, line in enumerate(_wrap(wording.sentence(last.hint))):
                col.label(text=line, icon="LIGHT" if n == 0 else "BLANK1")
    if last.warnings:
        warn = layout.box()
        warn.label(text="Warnings", icon="ERROR")
        col = warn.column(align=True)
        for entry in last.warnings.split("\n"):
            message, _, hint = entry.partition("\t")
            for n, line in enumerate(_wrap(wording.sentence(message))):
                col.label(text=line, icon="DOT" if n == 0 else "BLANK1")
            for line in _wrap(wording.sentence(hint)):
                col.label(text=line, icon="BLANK1")


class _SubPanel:
    bl_space_type = "VIEW_3D"
    bl_region_type = "UI"
    bl_category = "QuadLoom"
    bl_options = {"DEFAULT_CLOSED"}

    @classmethod
    def poll(cls, context):
        return bridge.available()


class QUADLOOM_PT_attributes(_SubPanel, bpy.types.Panel):
    bl_label = "Keep Attributes"
    bl_parent_id = "QUADLOOM_PT_main"

    def draw_header(self, context):
        self.layout.prop(context.scene.quadloom, "keep_attributes", text="")

    def draw(self, context):
        settings = context.scene.quadloom
        layout = self.layout
        layout.active = settings.keep_attributes
        col = layout.column(align=True)
        col.prop(settings, "keep_uvs")
        sub = col.row()
        sub.active = settings.keep_uvs
        sub.separator(factor=2.0)
        sub.prop(settings, "follow_uv_islands")
        col.prop(settings, "keep_materials")
        col.prop(settings, "keep_vertex_groups")
        col.prop(settings, "keep_other_attributes")
        col.prop(settings, "bind_to_rig")


class QUADLOOM_PT_details(_SubPanel, bpy.types.Panel):
    bl_label = "Details"
    bl_parent_id = "QUADLOOM_PT_main"

    @classmethod
    def poll(cls, context):
        return bridge.available() and context.scene.quadloom_last_run.status == "OK"

    def draw(self, context):
        last = context.scene.quadloom_last_run
        col = self.layout.column(align=True)
        col.label(text=f"Faces: {last.faces_in:,} → {last.faces_out:,}")
        for summary, icon in (
            (last.repair_summary, "TOOL_SETTINGS"),
            (last.feature_summary, "EDGESEL"),
            (last.remesh_summary, "MOD_REMESH"),
            (last.field_summary, "ORIENTATION_GIMBAL"),
            (last.frame_summary, "MOD_SIMPLEDEFORM"),
            (last.trace_summary, "MOD_LATTICE"),
            (last.quad_summary, "MESH_GRID"),
            (last.symmetry_summary, "MOD_MIRROR"),
        ):
            if not summary:
                continue
            for n, line in enumerate(_wrap(summary)):
                col.label(text=line, icon=icon if n == 0 else "BLANK1")


class QUADLOOM_PT_advanced(_SubPanel, bpy.types.Panel):
    bl_label = "Advanced"
    bl_parent_id = "QUADLOOM_PT_main"

    def draw(self, context):
        self.layout.label(text="Technical settings for each stage", icon="INFO")


class _AdvancedChild(_SubPanel):
    bl_parent_id = "QUADLOOM_PT_advanced"


def _split(layout):
    """Labels on the left, values on the right (Blender's property-split style)."""
    layout.use_property_split = True
    layout.use_property_decorate = False
    return layout


class QUADLOOM_PT_quads(_AdvancedChild, bpy.types.Panel):
    bl_label = "Quads"

    def draw_header(self, context):
        self.layout.prop(context.scene.quadloom, "quad_enabled", text="")

    def draw(self, context):
        settings = context.scene.quadloom
        layout = _split(self.layout)
        layout.active = settings.quad_enabled and settings.trace_enabled and settings.field_enabled
        if not (settings.trace_enabled and settings.field_enabled):
            layout.label(text="Needs the Cross Field and Patch Layout", icon="INFO")
        _sizing(layout, settings)
        layout.prop(settings, "quad_adaptivity")
        layout.prop(settings, "quad_regularity")
        layout.prop(settings, "quad_smoothing")
        layout.prop(settings, "quad_solver_effort")


class QUADLOOM_PT_symmetry(_AdvancedChild, bpy.types.Panel):
    bl_label = "Symmetry"

    def draw(self, context):
        settings = context.scene.quadloom
        layout = _split(self.layout)
        sub = layout.column()
        sub.active = settings.symmetry_auto
        sub.prop(settings, "symmetry_tolerance")
        layout.prop(settings, "symmetry_flip")


class QUADLOOM_PT_features(_AdvancedChild, bpy.types.Panel):
    bl_label = "Sharp Features"

    def draw(self, context):
        settings = context.scene.quadloom
        layout = _split(self.layout)
        col = layout.column(heading="Sources", align=True)
        col.prop(settings, "use_marked_sharp")
        col.prop(settings, "use_seams")
        col.prop(settings, "use_material_boundaries")
        col.prop(settings, "use_face_set_boundaries")
        col.prop(settings, "use_open_boundaries")
        layout.prop(settings, "prune_steps")


class QUADLOOM_PT_shape(_AdvancedChild, bpy.types.Panel):
    bl_label = "Quad Shape"

    def draw_header(self, context):
        self.layout.prop(context.scene.quadloom, "frame_enabled", text="")

    def draw(self, context):
        settings = context.scene.quadloom
        layout = _split(self.layout)
        layout.active = settings.frame_enabled and settings.field_enabled
        if not settings.field_enabled:
            layout.label(text="Needs the Cross Field", icon="INFO")
        layout.prop(settings, "frame_balance")
        layout.prop(settings, "frame_curvature_stretch")
        col = layout.column()
        col.active = settings.frame_curvature_stretch
        col.prop(settings, "frame_max_stretch")


class QUADLOOM_PT_cleanup(_AdvancedChild, bpy.types.Panel):
    bl_label = "Cleanup"

    def draw(self, context):
        settings = context.scene.quadloom
        layout = _split(self.layout)
        layout.prop(settings, "weld_distance_rel", text="Weld")
        layout.prop(settings, "min_component_faces", text="Min Part")
        col = layout.column(align=True)
        col.use_property_split = False
        col.prop(settings, "remove_degenerate")
        col.prop(settings, "split_non_manifold")
        col.prop(settings, "orient")


class QUADLOOM_PT_remesh(_AdvancedChild, bpy.types.Panel):
    bl_label = "Prep Remesh"

    def draw_header(self, context):
        self.layout.prop(context.scene.quadloom, "remesh_enabled", text="")

    def draw(self, context):
        settings = context.scene.quadloom
        layout = _split(self.layout)
        layout.active = settings.remesh_enabled
        layout.prop(settings, "remesh_target_rel")
        layout.prop(settings, "remesh_iterations")
        layout.prop(settings, "remesh_adaptive")
        col = layout.column(align=True)
        col.active = settings.remesh_adaptive
        col.prop(settings, "remesh_min_mult")
        col.prop(settings, "remesh_max_mult")
        layout.prop(settings, "remesh_max_deviation_rel")


class QUADLOOM_PT_field(_AdvancedChild, bpy.types.Panel):
    bl_label = "Cross Field"

    def draw_header(self, context):
        self.layout.prop(context.scene.quadloom, "field_enabled", text="")

    def draw(self, context):
        settings = context.scene.quadloom
        layout = _split(self.layout)
        layout.active = settings.field_enabled
        layout.prop(settings, "field_curvature")
        col = layout.column(align=True)
        col.active = settings.field_curvature != "OFF"
        col.prop(settings, "field_alignment_weight")
        sub = col.column(align=True)
        sub.active = settings.field_curvature == "AUTO"
        sub.prop(settings, "field_sharp_factor")


class QUADLOOM_PT_layout(_AdvancedChild, bpy.types.Panel):
    bl_label = "Patch Layout"

    def draw_header(self, context):
        self.layout.prop(context.scene.quadloom, "trace_enabled", text="")

    def draw(self, context):
        settings = context.scene.quadloom
        layout = _split(self.layout)
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


class QUADLOOM_PT_system(_AdvancedChild, bpy.types.Panel):
    bl_label = "System"

    def draw(self, context):
        settings = context.scene.quadloom
        layout = _split(self.layout)
        layout.prop(settings, "max_faces")
        layout.prop(settings, "worker_memory_mb")
        col = layout.column()
        col.use_property_split = False
        col.prop(settings, "quick_rerun")
        col.prop(settings, "show_steps")
        layout.label(text=f"Backend {bridge.version()}", icon="SETTINGS")
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
    QUADLOOM_PT_attributes,
    QUADLOOM_PT_details,
    QUADLOOM_PT_advanced,
    QUADLOOM_PT_quads,
    QUADLOOM_PT_symmetry,
    QUADLOOM_PT_features,
    QUADLOOM_PT_shape,
    QUADLOOM_PT_cleanup,
    QUADLOOM_PT_remesh,
    QUADLOOM_PT_field,
    QUADLOOM_PT_layout,
    QUADLOOM_PT_system,
)
