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

    # --- Cross field (stage 3) ---------------------------------------------------
    field_enabled: BoolProperty(
        name="Cross Field",
        description="Compute the cross field that will orient the quads' edge flow",
        default=True,
    )
    field_curvature: EnumProperty(
        name="Follow Curvature",
        description="When the edge flow follows the surface's principal curvature",
        items=[
            ("AUTO", "Auto", "Where sharp features are too sparse to guide the flow (QuadWild's rule)"),
            ("ON", "Always", "Follow principal curvature everywhere it's clear"),
            ("OFF", "Never", "Follow only sharp features and borders"),
        ],
        default="AUTO",
    )
    field_alignment_weight: FloatProperty(
        name="Curvature Strength",
        description="How strongly the flow follows curvature instead of staying smooth. Small values "
        "already align wherever curvature is consistent; large ones also follow noisy curvature",
        default=0.02,
        min=0.0,
        max=100.0,
        soft_max=0.5,
        precision=3,
    )
    field_sharp_factor: FloatProperty(
        name="Feature Sufficiency",
        description="Auto follows curvature unless the sharp features are at least this many times "
        "longer than the square root of the surface area",
        default=6.0,
        min=0.0,
        max=1000.0,
        soft_max=20.0,
    )

    # --- Patch layout (stage 4) --------------------------------------------------
    trace_enabled: BoolProperty(
        name="Patch Layout",
        description="Trace field-aligned paths that cut the surface into patches for the quads "
        "(needs the cross field)",
        default=True,
    )
    trace_max_sides: IntProperty(
        name="Max Sides",
        description="Most sides a patch may have. 5 follows QuadWild; 6 allows hexagonal patches, "
        "giving fewer, larger patches",
        default=5,
        min=4,
        max=6,
    )
    trace_cost: EnumProperty(
        name="Path Cost",
        description="How a traced path is charged for drifting off the field",
        items=[
            ("ANISOTROPIC", "Anisotropic", "Length measured with sideways motion stretched by Drift"),
            ("ANGLE_PENALTY", "Angle Penalty", "QuadWild's: length times 1 + Drift × (angle / 45°)²"),
        ],
        default="ANISOTROPIC",
    )
    trace_drift: FloatProperty(
        name="Drift",
        description="How strongly paths are kept on the field. Higher: straighter, more field-true paths; "
        "lower: shorter paths that may cut across",
        default=10.0,
        min=0.0,
        max=1000.0,
        soft_max=200.0,
    )
    trace_cc_tolerance: FloatProperty(
        name="Side Tolerance",
        description="How far (in edge lengths) a patch's sides may be from a clean single-singularity "
        "quadrangulation before the patch is split further",
        default=1.0,
        min=0.0,
        max=10.0,
    )
    trace_match_valence: BoolProperty(
        name="Match Singularities",
        description="Keep at most one singularity per patch, matching its number of sides",
        default=True,
    )
    trace_loop_density: FloatProperty(
        name="Loop Density",
        description="Loop seeds per vertex (QuadWild's sample ratio)",
        default=0.01,
        min=0.001,
        max=0.1,
        precision=3,
    )
    trace_sub_passes: IntProperty(
        name="Patch Passes",
        description="Rounds of tracing inside patches that still fail a condition",
        default=3,
        min=0,
        max=10,
    )

    # --- Quad Shape (stage 3b) ---------------------------------------------------
    frame_enabled: BoolProperty(
        name="Rectangular Quads",
        description="Shape the quads as rectangles that follow the cross field, instead of "
        "leaving their shape to the field alone (needs the Cross Field)",
        default=False,
    )
    frame_balance: FloatProperty(
        name="Keep Square",
        description="0 keeps the quads the same size and lets them stretch; 1 keeps them square "
        "and lets their size vary",
        default=0.5,
        min=0.0,
        max=1.0,
        precision=2,
    )
    frame_curvature_stretch: BoolProperty(
        name="Stretch Along Curvature",
        description="Let quads stretch along the surface's curvature, longer where it bends less",
        default=False,
    )
    frame_max_stretch: FloatProperty(
        name="Max Stretch",
        description="Largest allowed ratio between a quad's long and short sides",
        default=3.0,
        min=1.0,
        max=20.0,
        soft_max=6.0,
        precision=1,
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
    field_summary: StringProperty(name="Field")
    frame_summary: StringProperty(name="Shape")
    trace_summary: StringProperty(name="Layout")


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
        "field": {
            "enabled": settings.field_enabled,
            "curvature_alignment": settings.field_curvature.lower(),
            "alignment_weight": settings.field_alignment_weight,
            "sharp_factor": settings.field_sharp_factor,
        },
        "frame": {
            "enabled": settings.frame_enabled,
            "balance": settings.frame_balance,
            "curvature_stretch": settings.frame_curvature_stretch,
            "max_stretch": settings.frame_max_stretch,
        },
        "trace": {
            "enabled": settings.trace_enabled,
            "max_sides": settings.trace_max_sides,
            "cost": settings.trace_cost.lower(),
            "drift": settings.trace_drift,
            "cc_tolerance": settings.trace_cc_tolerance,
            "match_valence": settings.trace_match_valence,
            "loop_density": settings.trace_loop_density,
            "sub_passes": settings.trace_sub_passes,
        },
    }
