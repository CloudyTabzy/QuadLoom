# SPDX-License-Identifier: GPL-3.0-or-later
"""Scene-level settings and the record of the last run."""

import bpy
from bpy.props import BoolProperty, EnumProperty, FloatProperty, IntProperty, PointerProperty, StringProperty

DEFAULT_MAX_FACES = 2_000_000

# Presets: what each sets, which is every setting on the main panel plus the
# quad-stage settings that change the result. Symmetry and the System
# toggles (Quick Re-run, Show Intermediate Steps) are never touched.
_COMMON = {"quad_regularity": 0.5, "quad_solver_effort": "BALANCED"}
PRESETS = {
    "BALANCED": {"density": 1.0, "quad_adaptivity": 0.5, "use_angle": True, "angle_deg": 35.0, "quad_smoothing": 20},
    "ORGANIC": {"density": 1.0, "quad_adaptivity": 0.6, "use_angle": False, "angle_deg": 35.0, "quad_smoothing": 25},
    "HARD_SURFACE": {"density": 1.0, "quad_adaptivity": 0.3, "use_angle": True, "angle_deg": 30.0, "quad_smoothing": 12},
    "REGULAR_QUADS": {"density": 1.0, "quad_adaptivity": 0.3, "use_angle": True, "angle_deg": 35.0, "quad_smoothing": 25,
                      "quad_regularity": 1.0},
    "LOW_POLY": {"density": 0.5, "quad_adaptivity": 0.3, "use_angle": True, "angle_deg": 40.0, "quad_smoothing": 20},
    "HIGH_DETAIL": {"density": 2.0, "quad_adaptivity": 0.7, "use_angle": True, "angle_deg": 35.0, "quad_smoothing": 20},
}
PRESETS = {name: {**_COMMON, **values} for name, values in PRESETS.items()}
# Quad edge length at Density 1, as a multiple of the prep remesh's edge.
BASE_EDGE_SCALE = 0.75

# Solver Effort: multiples of the quad-count solver's node limits.
SOLVER_EFFORT = {"FAST": 0.25, "BALANCED": 1.0, "THOROUGH": 4.0}

# Regularity 0–1 maps onto the quantizer's weight 25–100 (0.5 → its default 50).
# Below 25 patches start needing uneven patterns; above 100 nothing changes.
def regularity_weight(value):
    return 25.0 * 4.0 ** min(max(value, 0.0), 1.0)


_applying_preset = False


def _apply_preset(settings, _context):
    global _applying_preset
    values = PRESETS.get(settings.preset)
    if values is None:
        return
    _applying_preset = True
    try:
        for name, value in values.items():
            setattr(settings, name, value)
    finally:
        _applying_preset = False


def _mark_custom(settings, _context):
    """A preset-controlled setting changed by hand: the preset becomes Custom."""
    if _applying_preset or settings.preset == "CUSTOM":
        return
    values = PRESETS.get(settings.preset, {})
    if any(getattr(settings, name) != value for name, value in values.items()):
        settings.preset = "CUSTOM"


class QUADLOOM_PG_settings(bpy.types.PropertyGroup):
    # --- The main panel ----------------------------------------------------------
    preset: EnumProperty(
        name="Preset",
        description="Starting points for common kinds of models",
        items=[
            ("BALANCED", "Balanced", "Good all-round settings"),
            ("ORGANIC", "Organic / Sculpt", "Characters, sculpts and scans: no creases from surface noise, more detail where needed"),
            ("HARD_SURFACE", "Hard Surface", "Mechanical and modelled objects: keeps sharp edges crisp"),
            ("REGULAR_QUADS", "Regular Quads", "Evenly divided patches and even quad sizes, at the cost of matching the size exactly"),
            ("LOW_POLY", "Low Poly", "Fewer, larger quads"),
            ("HIGH_DETAIL", "High Detail", "More quads, concentrated on detail"),
            ("CUSTOM", "Custom", "Your own settings"),
        ],
        default="BALANCED",
        update=_apply_preset,
    )
    sizing_mode: EnumProperty(
        name="Sizing",
        description="Choose relative Density or an approximate total number of output quads",
        items=[("DENSITY", "Density", "Relative quad density"),
               ("TARGET", "Target Quads", "Aim for a total quad count, including all parts and symmetry")],
        default="DENSITY",
    )
    target_quads: IntProperty(
        name="Target Quads",
        description="Approximate total output quads, including all parts and symmetry. "
        "Feature boundaries and integer subdivisions can limit accuracy; the achieved count is reported",
        default=10_000,
        min=1,
        max=2_000_000,
        soft_max=100_000,
    )
    density: FloatProperty(
        name="Density",
        description="How many quads: 2 gives about four times as many as 1, 0.5 about a quarter",
        default=1.0,
        min=0.2,
        max=5.0,
        soft_min=0.25,
        soft_max=3.0,
        precision=2,
        update=_mark_custom,
    )
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

    # --- Quads (stages 5–6) -----------------------------------------------------
    quad_enabled: BoolProperty(
        name="Quads",
        description="Turn the patch layout into an all-quad mesh",
        default=True,
    )
    quad_adaptivity: FloatProperty(
        name="Adaptive Detail",
        description="Smaller quads where the shape is detailed, larger where it's plain: 0 keeps every quad the same size",
        default=0.5,
        min=0.0,
        max=1.0,
        precision=2,
        update=_mark_custom,
    )
    quad_regularity: FloatProperty(
        name="Regularity",
        description=(
            "How evenly patches are divided: higher gives regular grids everywhere (and a few more quads), "
            "lower matches the quad size more closely"
        ),
        default=0.5,
        min=0.0,
        max=1.0,
        precision=2,
        update=_mark_custom,
    )
    quad_solver_effort: EnumProperty(
        name="Solver Effort",
        description="How hard the quad-count solver searches. The result is the same on every run either way",
        items=[
            ("FAST", "Fast", "About 3x quicker quad sizing on large layouts, usually the same result: good for previews"),
            ("BALANCED", "Balanced", "The default search"),
            ("THOROUGH", "Thorough", "A 4x longer search for difficult layouts; rarely changes the result, much slower on large ones"),
        ],
        default="BALANCED",
        update=_mark_custom,
    )
    quad_smoothing: IntProperty(
        name="Smoothing",
        description="Rounds of relaxing the quads on the surface (more is smoother, 0 is none)",
        default=20,
        min=0,
        max=100,
        update=_mark_custom,
    )

    # --- Symmetry -----------------------------------------------------------------
    symmetry_x: BoolProperty(name="X", description="Remesh one side of the X mirror plane and mirror it", default=False)
    symmetry_y: BoolProperty(name="Y", description="Remesh one side of the Y mirror plane and mirror it", default=False)
    symmetry_z: BoolProperty(name="Z", description="Remesh one side of the Z mirror plane and mirror it", default=False)
    symmetry_auto: BoolProperty(
        name="Auto Detect",
        description="Find the mirror planes the mesh is symmetric about (through the origin or the bounding-box center)",
        default=False,
    )
    symmetry_flip: BoolProperty(
        name="Keep Negative Side",
        description="Remesh the negative side of each plane and mirror it, instead of the positive side",
        default=False,
    )
    symmetry_tolerance: FloatProperty(
        name="Tolerance",
        description="Largest asymmetry Auto Detect accepts, as a fraction of the object's size",
        default=0.005,
        min=0.0,
        max=0.1,
        precision=3,
    )

    # --- Attributes (decision 0019) ---------------------------------------------
    keep_attributes: BoolProperty(
        name="Keep Attributes",
        description="Carry the source's UV maps, materials, vertex groups, colors and rig over to the result",
        default=True,
    )
    keep_uvs: BoolProperty(
        name="UV Maps",
        description="Every UV map, by name, with seams marked where the active render map is cut",
        default=True,
    )
    keep_materials: BoolProperty(
        name="Materials",
        description="The material slots and each face's material",
        default=True,
    )
    keep_vertex_groups: BoolProperty(
        name="Vertex Groups",
        description="Every vertex group, by name, with its weights",
        default=True,
    )
    keep_other_attributes: BoolProperty(
        name="Colors and Other Attributes",
        description="Color attributes, face sets, flat or smooth shading and any other point, corner or face attribute",
        default=True,
    )
    follow_uv_islands: BoolProperty(
        name="Follow UV Islands",
        description=(
            "Quads follow the borders of the render UV map's islands, so no quad spans two islands and "
            "the UVs carry over exactly. Turn off for maps with very many small islands"
        ),
        default=True,
    )
    bind_to_rig: BoolProperty(
        name="Bind to Rig",
        description=(
            "Give the result the source's parent and Armature modifiers. A rigged mesh is remeshed in its "
            "rest shape, so the result deforms with the rig like the source"
        ),
        default=True,
    )

    # --- Sharp features (stage 1) ------------------------------------------------
    use_angle: BoolProperty(
        name="By Angle",
        description="Edges where the surface bends more than the angle threshold become features",
        default=True,
        update=_mark_custom,
    )
    angle_deg: FloatProperty(
        name="Angle Threshold",
        description="Bend angle, in degrees, above which an edge becomes a feature. "
        "Lower finds subtler creases",
        default=35.0,
        min=0.0,
        max=180.0,
        precision=1,
        update=_mark_custom,
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
    quick_rerun: BoolProperty(
        name="Quick Re-run",
        description=(
            "Keep the last run's cleanup, cross field and patch layout. When only quad settings change "
            "(Density, Adaptive Detail, Regularity, Smoothing, Solver Effort) the next run starts at the quads, "
            "which is much faster. Anything else changing runs everything again"
        ),
        default=True,
    )
    show_steps: BoolProperty(
        name="Show Intermediate Steps",
        description=(
            "Also create the prep remesh and the patch layout (each patch in its own color) as hidden objects "
            "in a \"QuadLoom Steps\" collection, to see where a result went wrong"
        ),
        default=False,
    )
    max_faces: IntProperty(
        name="Face Limit",
        description="Inputs with more triangles than this are refused before any work starts. "
        "Raise it for very dense meshes if your machine has the memory",
        default=DEFAULT_MAX_FACES,
        min=1,
    )
    worker_memory_mb: IntProperty(
        name="Worker Memory (MiB)",
        description="Memory limit for the isolated remesher, including Quick Re-run. "
        "A run that reaches it stops safely; lower Density or raise this limit to retry. "
        "At most half the available memory is used when the worker starts",
        default=2048,
        min=256,
        max=65536,
    )
    debug_fault: StringProperty(
        name="Inject Fault",
        description="Developer builds only: panic:<stage>, error:<stage>, oom:<stage>, slow:<ms>, "
        "worker:abort, worker:oom or worker:stall",
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
    quads_out: IntProperty(name="Quads Out")
    target_quads: IntProperty(name="Requested Quads")
    reused: BoolProperty(name="Reused the Layout")
    elapsed_ms: FloatProperty(name="Time (ms)")
    report_json: StringProperty(name="Report")
    repair_summary: StringProperty(name="Cleanup")
    remesh_summary: StringProperty(name="Remesh")
    feature_summary: StringProperty(name="Features")
    field_summary: StringProperty(name="Field")
    frame_summary: StringProperty(name="Shape")
    trace_summary: StringProperty(name="Layout")
    quad_summary: StringProperty(name="Quads")
    symmetry_summary: StringProperty(name="Symmetry")
    # One warning per line: "message\thint" (hint may be empty).
    warnings: StringProperty(name="Warnings")


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
        "quad": {
            "enabled": settings.quad_enabled,
            "target_quads": settings.target_quads if settings.sizing_mode == "TARGET" else 0,
            "edge_scale": BASE_EDGE_SCALE if settings.sizing_mode == "TARGET" else BASE_EDGE_SCALE / max(settings.density, 1e-3),
            "adaptivity": settings.quad_adaptivity,
            "regularity": regularity_weight(settings.quad_regularity),
            "solver_effort": SOLVER_EFFORT[settings.quad_solver_effort],
            "smoothing": settings.quad_smoothing,
        },
        "symmetry": {
            "x": settings.symmetry_x,
            "y": settings.symmetry_y,
            "z": settings.symmetry_z,
            "auto_detect": settings.symmetry_auto,
            "keep_negative": settings.symmetry_flip,
            "tolerance": settings.symmetry_tolerance,
        },
    }
