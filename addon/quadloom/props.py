# SPDX-License-Identifier: GPL-3.0-or-later
"""Scene-level settings and the record of the last run."""

import bpy
from bpy.props import BoolProperty, EnumProperty, FloatProperty, IntProperty, PointerProperty, StringProperty

DEFAULT_MAX_FACES = 2_000_000


class QUADLOOM_PG_settings(bpy.types.PropertyGroup):
    # --- Cleanup (stage 0) ---------------------------------------------------
    weld_distance_rel: FloatProperty(
        name="Weld Distance",
        description="Merge vertices closer than this fraction of the object's size. Joins vertices that "
        "exporters split at UV seams or hard normals, so seams don't become holes",
        default=1e-6,
        min=0.0,
        max=1e-2,
        soft_max=1e-3,
        precision=7,
    )
    remove_degenerate: BoolProperty(
        name="Remove Degenerate Faces",
        description="Drop faces with zero area, repeated corners, or exact duplicates",
        default=True,
    )
    split_non_manifold: BoolProperty(
        name="Fix Non-Manifold",
        description="Split edges shared by more than two faces and vertices joining separate fans, "
        "so every part is a clean surface",
        default=True,
    )
    orient: BoolProperty(
        name="Fix Normals",
        description="Make face winding consistent and point closed parts outward",
        default=True,
    )
    min_component_faces: IntProperty(
        name="Minimum Part Size",
        description="Tiny loose parts (under 1% of the object's size) with fewer faces than this are skipped as debris; the largest part is always kept",
        default=8,
        min=1,
    )

    # --- Prep remesh (stage 2) ---------------------------------------------------
    remesh_enabled: BoolProperty(
        name="Remesh First",
        description="Rebuild the surface as clean, even triangles (keeping every feature line) before "
        "quad remeshing. Recommended; turn off only for meshes that are already clean and even",
        default=True,
    )
    remesh_target_rel: FloatProperty(
        name="Triangle Size",
        description="Target edge length as a fraction of the object's size. 0 picks a size automatically "
        "(about 10,000 triangles, more for complex shapes)",
        default=0.0,
        min=0.0,
        max=0.5,
        precision=4,
    )
    remesh_iterations: IntProperty(
        name="Iterations",
        description="Refinement passes. More passes give more even triangles but take longer",
        default=15,
        min=1,
        max=100,
    )
    remesh_adaptive: BoolProperty(
        name="Adaptive Density",
        description="Use smaller triangles where the surface curves, as much as Max Deviation needs (and larger ones on flat areas, if Coarsest is above 1)",
        default=True,
    )
    remesh_min_mult: FloatProperty(
        name="Finest",
        description="Smallest triangle size allowed by adaptive density, relative to Triangle Size",
        default=0.3,
        min=0.05,
        max=1.0,
    )
    remesh_max_mult: FloatProperty(
        name="Coarsest",
        description="Largest triangle size allowed by adaptive density, relative to Triangle Size. Above 1, flat areas get larger triangles and follow the original surface less closely",
        default=1.0,
        min=1.0,
        max=10.0,
    )
    remesh_max_deviation_rel: FloatProperty(
        name="Max Deviation",
        description="Furthest the rebuilt surface may move from the original, as a fraction of the object's size",
        default=1.0 / 2500.0,
        min=1e-6,
        max=0.1,
        precision=5,
    )

    # --- Sharp features (stage 1) ------------------------------------------------
    use_angle: BoolProperty(
        name="By Angle",
        description="Edges where the surface bends more than the angle threshold become features",
        default=True,
    )
    angle_deg: FloatProperty(
        name="Angle Threshold",
        description="Bend angle, in degrees, above which an edge becomes a feature. "
        "Lower finds subtler creases",
        default=35.0,
        min=0.0,
        max=180.0,
        precision=1,
    )
    use_marked_sharp: BoolProperty(
        name="Marked Sharp",
        description="Edges you marked Sharp become features",
        default=True,
    )
    use_seams: BoolProperty(
        name="UV Seams",
        description="UV seam edges become features",
        default=True,
    )
    use_material_boundaries: BoolProperty(
        name="Material Boundaries",
        description="Edges between faces with different materials become features",
        default=True,
    )
    use_face_set_boundaries: BoolProperty(
        name="Face Set Boundaries",
        description="Edges between sculpt face sets become features",
        default=True,
    )
    use_open_boundaries: BoolProperty(
        name="Open Boundaries",
        description="Holes and open edges (necklines, cuffs, hems…) become features",
        default=True,
    )
    prune_steps: IntProperty(
        name="Prune Short Spurs",
        description="Remove angle-detected feature fragments up to this many edges long that dangle "
        "from the feature network. Your marks are never pruned. 0 disables",
        default=4,
        min=0,
        soft_max=20,
    )

    # --- Advanced ------------------------------------------------------------
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
    repair_summary: StringProperty(name="Cleanup")
    remesh_summary: StringProperty(name="Remesh")
    feature_summary: StringProperty(name="Features")


classes = (QUADLOOM_PG_settings, QUADLOOM_PG_last_run)


def register_props():
    bpy.types.Scene.quadloom = PointerProperty(type=QUADLOOM_PG_settings)
    bpy.types.Scene.quadloom_last_run = PointerProperty(type=QUADLOOM_PG_last_run)


def unregister_props():
    for name in ("quadloom_last_run", "quadloom"):
        if hasattr(bpy.types.Scene, name):
            delattr(bpy.types.Scene, name)


def run_params(settings):
    """The native `params` dict for the current settings."""
    return {
        "repair": {
            "weld_epsilon_rel": settings.weld_distance_rel,
            "remove_degenerate": settings.remove_degenerate,
            "split_non_manifold": settings.split_non_manifold,
            "orient": settings.orient,
            "min_component_faces": settings.min_component_faces,
        },
        "features": {
            "angle_deg": settings.angle_deg,
            "use_angle": settings.use_angle,
            "use_marked_sharp": settings.use_marked_sharp,
            "use_seams": settings.use_seams,
            "use_material_boundaries": settings.use_material_boundaries,
            "use_face_set_boundaries": settings.use_face_set_boundaries,
            "use_open_boundaries": settings.use_open_boundaries,
            "prune_steps": settings.prune_steps,
        },
        "remesh": {
            "enabled": settings.remesh_enabled,
            "target_edge_rel": settings.remesh_target_rel,
            "iterations": settings.remesh_iterations,
            "adaptive": settings.remesh_adaptive,
            "min_mult": settings.remesh_min_mult,
            "max_mult": settings.remesh_max_mult,
            "max_surface_distance_rel": settings.remesh_max_deviation_rel,
        },
    }
