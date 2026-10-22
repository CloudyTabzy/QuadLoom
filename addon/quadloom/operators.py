# SPDX-License-Identifier: GPL-3.0-or-later
"""Remesh and cancel operators.

A run is a `Job`: the mesh is snapshotted on the main thread, the native call
runs on a worker thread (the backend releases the GIL), and the result is
turned into a new object back on the main thread. At most one job runs at a time.
"""

import array
import threading
import time

import bpy

from . import bridge, mesh_io, props

_active_job = None


class Job:
    def __init__(self, snapshot, kwargs, warnings=()):
        self.snapshot = snapshot
        self.kwargs = kwargs
        # Set when the job is dropped (undo, file load, unregister): its result
        # must never reach the scene.
        self.discarded = False
        # Warnings found before the run (selection, transform), as (message, hint).
        self.preflight = list(warnings)
        self.control = bridge.new_control()
        self.result = None
        self.error = None
        self.started = time.monotonic()
        self.thread = threading.Thread(target=self._work, name="QuadLoom job", daemon=True)

    def _work(self):
        try:
            self.result = bridge.run_job(self.snapshot.positions, self.snapshot.indices, self.control, **self.kwargs)
        except Exception as e:  # every failure is reported, never re-raised into Blender
            self.error = e

    @property
    def done(self):
        return not self.thread.is_alive()

    def elapsed_ms(self):
        return (time.monotonic() - self.started) * 1000.0


def job_running():
    """True while a job's worker thread is still running.

    A finished job whose modal operator never collected it (the handler was
    dropped by a file load, say) doesn't count, so it can never lock the add-on.
    """
    return _active_job is not None and not _active_job.done


def start_job(context):
    """Snapshots the active object and starts a job. Raises on bad input."""
    global _active_job
    if job_running():
        raise RuntimeError("a QuadLoom run is already in progress")
    settings = context.scene.quadloom
    obj = context.active_object
    if obj is not None and obj.mode == "EDIT":
        # Edit Mode changes reach the mesh data only when flushed.
        obj.update_from_editmode()
    preflight = preflight_warnings(context, settings)
    snapshot = mesh_io.extract(obj, context.evaluated_depsgraph_get(), settings.max_faces)
    kwargs = {"max_faces": settings.max_faces, "params": props.run_params(settings), **snapshot.feature_kwargs()}
    if settings.debug_fault and bridge.build_info().get("fault_injection"):
        kwargs["fault"] = settings.debug_fault
    job = Job(snapshot, kwargs, preflight)
    last = context.scene.quadloom_last_run
    last.status, last.message, last.stage, last.hint = "RUNNING", "Starting…", "", ""
    last.faces_in, last.faces_out, last.elapsed_ms, last.report_json = snapshot.face_count, 0, 0.0, ""
    last.quads_out = 0
    last.repair_summary, last.feature_summary, last.remesh_summary, last.field_summary = "", "", "", ""
    last.trace_summary, last.frame_summary, last.quad_summary = "", "", ""
    last.symmetry_summary = ""
    last.warnings = ""
    _active_job = job
    job.thread.start()
    return job


def finish_job(context, job):
    """Applies a finished job's outcome on the main thread. Returns the operator result set."""
    global _active_job
    if _active_job is job:
        _active_job = None
    last = context.scene.quadloom_last_run
    last.elapsed_ms = job.elapsed_ms()
    if job.discarded:
        # Dropped by undo, a file load or unregistering: its result never lands.
        last.status, last.message = "CANCELLED", "The run was dropped (undo or file change)"
        return {"CANCELLED"}, ("INFO", "QuadLoom: run dropped")
    error = job.error
    if error is None:
        try:
            obj = mesh_io.build_result(job.snapshot, job.result, context.scene)
        except Exception as e:  # building the object failed; report, don't crash
            error = e
        else:
            last.status, last.message = "OK", f"Created {obj.name}"
            last.faces_out = len(obj.data.polygons)
            sides = array.array("i", [0]) * len(obj.data.polygons)
            obj.data.polygons.foreach_get("loop_total", sides)
            last.quads_out = sides.count(4)
            last.report_json = obj["quadloom_report"]
            last.repair_summary = repair_summary(job.result["repair"])
            last.feature_summary = feature_summary(job.result["features"])
            last.remesh_summary = remesh_summary(job.result.get("remesh"), job.kwargs["params"]["remesh"]["enabled"])
            last.field_summary = field_summary(job.result.get("field"))
            last.frame_summary = frame_summary(job.result.get("frame"))
            last.trace_summary = trace_summary(job.result.get("trace"))
            last.quad_summary = quad_summary(job.result.get("quads"), job.result.get("quad_error"))
            last.symmetry_summary = symmetry_summary(job.result.get("symmetry"))
            warnings = job.preflight + [(w["message"], w.get("hint") or "") for w in job.result.get("warnings", [])]
            last.warnings = "\n".join(f"{m}\t{h}" for m, h in warnings)
            if warnings:
                more = f" (+{len(warnings) - 1} more, see the panel)" if len(warnings) > 1 else ""
                return {"FINISHED"}, ("WARNING", f"QuadLoom: {warnings[0][0]}{more}")
            return {"FINISHED"}, ("INFO", f"QuadLoom: {last.message} ({last.faces_out} faces)")

    cancelled = isinstance(error, bridge.CancelledError)
    last.status = "CANCELLED" if cancelled else "ERROR"
    last.message = str(error)
    last.stage = getattr(error, "stage", None) or ""
    last.hint = getattr(error, "hint", None) or ""
    if cancelled:
        return {"CANCELLED"}, ("INFO", "QuadLoom: cancelled")
    return {"CANCELLED"}, ("ERROR", f"QuadLoom failed: {error}")


def repair_summary(repair):
    """One line describing what cleanup changed, e.g. "welded 812 vertices, 2 parts"."""
    parts = []
    removed = repair["removed_repeated"] + repair["removed_zero_area"] + repair["removed_duplicates"]
    for count, text in (
        (repair["welded_vertices"], "welded {} vertices"),
        (removed, "removed {} degenerate faces"),
        (repair["split_non_manifold_edges"], "split {} non-manifold edges"),
        (repair["split_non_manifold_vertices"], "split {} non-manifold vertices"),
        (repair["flipped_faces"], "flipped {} faces"),
        (repair["cut_edges"], "cut {} edges to fix orientation"),
        (repair["skipped_small_components"], "skipped {} tiny parts"),
    ):
        if count:
            parts.append(text.format(f"{count:,}"))
    components = repair["components"]
    parts.append(f"{components} part" + ("" if components == 1 else "s"))
    return ", ".join(parts) if len(parts) > 1 else f"no cleanup needed, {parts[0]}"


def remesh_summary(remesh, enabled):
    if not enabled or not remesh:
        return "prep remesh off"
    return f"remeshed {remesh['input_faces']:,} → {remesh['output_faces']:,} triangles"


def field_summary(field):
    """One line about the cross field, e.g. "cross field: 32 singularities, follows curvature"."""
    if not field:
        return "cross field off"
    report = field["report"]
    text = f"cross field: {report['singularities']:,} singularities"
    if report["curvature_aligned"]:
        text += ", follows curvature"
    if report["fallback_parts"]:
        text += f", {report['fallback_parts']} parts used the fallback field"
    return text


def frame_summary(frame):
    """One line about the rectangular frame, e.g. "rotated ≤ 23.7°, size 0.70–1.91×, stretch 1.5 (p95 3.6)"."""
    if not frame:
        return "off"
    if frame["solved_parts"] == 0:
        if frame["fallback_parts"]:
            text = "kept the regular field (the field could not be made rectangular)"
        else:
            text = "no part was large enough to shape"
    else:
        text = (
            f"rotated ≤ {frame['max_rotation_deg']:.1f}°, size {frame['size_p5']:.2f}–{frame['size_p95']:.2f}×, "
            f"stretch {frame['stretch_p50']:.1f} (p95 {frame['stretch_p95']:.1f})"
        )
        if frame["fallback_parts"]:
            text += f"; kept the regular field on {_parts(frame['fallback_parts'])}"
    if frame["skipped_parts"]:
        text += f"; {_parts(frame['skipped_parts'])} too large"
    return text


def _parts(count):
    return f"{count} part" + ("" if count == 1 else "s")


def trace_summary(trace):
    """One line about the patch layout, e.g. "layout: 312 patches (4 need fixing)"."""
    if not trace:
        return "patch layout off"
    report = trace["report"]
    text = f"layout: {report['patches']:,} patches"
    if report["unsolved"]:
        text += f" ({report['unsolved']:,} need fixing)"
    if report["seams"]:
        text += f", {report['seams']:,} seams"
    return text


def quad_summary(quads, error=None):
    """One line about the quad mesh, e.g. "1,234 quads (310 grid, 40 single-center patches)"."""
    if error:
        return f"quads failed ({error}); showing the patch layout"
    if not quads:
        return "quads off"
    report = quads["report"]
    text = f"{report['quads']:,} quads"
    parts = [f"{report['grid_patches']:,} grid", f"{report['center_patches']:,} center"]
    if report["split_patches"]:
        parts.append(f"{report['split_patches']:,} split")
    if report["fan_patches"]:
        parts.append(f"{report['fan_patches']:,} fallback")
    text += " (" + ", ".join(parts) + " patches)"
    if report["kept_patches"]:
        text += f"; {report['kept_patches']:,} patches kept as triangles"
    return text


def preflight_warnings(context, settings):
    """Warnings Blender knows about before the run: (message, hint) pairs."""
    warnings = []
    obj = context.active_object
    selected = [o for o in context.selected_objects if o is not obj]
    if selected:
        warnings.append(("only the active object is remeshed; other selected objects are ignored", ""))
    symmetric = settings.symmetry_auto or settings.symmetry_x or settings.symmetry_y or settings.symmetry_z
    if obj is not None and symmetric:
        scale = obj.matrix_world.to_scale()
        if min(scale) < 0:
            warnings.append(("the object is mirrored by a negative scale: symmetry uses its own (local) axes",
                             "apply the scale (Ctrl+A) to work in the axes you see"))
        elif max(scale) > 1.0001 * min(scale):
            warnings.append(("the object has a non-uniform scale: symmetry uses its own (local) axes",
                             "apply the scale (Ctrl+A) to work in the axes you see"))
    return warnings


def symmetry_summary(symmetry):
    """One line about symmetry, e.g. "mirrored X, Z (auto, asymmetry 0.12%)"; empty when off."""
    if not symmetry:
        return ""
    if symmetry.get("skipped"):
        return f"symmetry skipped: {symmetry['skipped']}"
    axes = [name for name, on in zip("XYZ", symmetry["axes"]) if on]
    text = "mirrored " + ", ".join(axes)
    measured = [a for a, on in zip(symmetry["asymmetry"], symmetry["axes"]) if on and a >= 0]
    if measured:
        text += f" (auto, asymmetry {100 * max(measured):.2f}%)"
    return text


def feature_summary(features):
    return f"{features['edges']:,} feature edges, {features['corners']:,} corners"


def cancel_active_job(join_timeout=None):
    """Requests cancellation of the running job, optionally waiting for its thread."""
    job = _active_job
    if job is None:
        return
    job.control.cancel()
    if join_timeout is not None:
        job.thread.join(join_timeout)


def discard_active_job(join_timeout=5.0):
    """Cancels, waits for, and forgets the active job; its result is never used."""
    if _active_job is not None:
        _active_job.discarded = True
    _discard(join_timeout)


def _discard(join_timeout=5.0):
    """Cancels, waits for, and forgets the active job. Its result (if any) is dropped.

    Used when the context the job belongs to goes away: the add-on being
    disabled, or a different .blend being loaded.
    """
    global _active_job
    cancel_active_job(join_timeout)
    _active_job = None


@bpy.app.handlers.persistent
def _on_load_pre(*_args):
    discard_active_job()


@bpy.app.handlers.persistent
def _on_undo_pre(*_args):
    # Undo/redo restores an older scene: a running job's result would land in
    # it out of place, so the job is dropped (cancelled without waiting long).
    if job_running():
        discard_active_job(join_timeout=0.5)


def _redraw_panels(context):
    for window in context.window_manager.windows:
        for area in window.screen.areas:
            if area.type == "VIEW_3D":
                area.tag_redraw()


class QUADLOOM_OT_remesh(bpy.types.Operator):
    bl_idname = "quadloom.remesh"
    bl_label = "Remesh"
    bl_description = "Remesh the active object into a new quad mesh. The original is left untouched"
    # UNDO: the step is pushed when the modal operator returns FINISHED, so
    # Ctrl+Z removes exactly the result object. No REGISTER: "Adjust Last
    # Operation" would re-run the background job synchronously, which a modal
    # operator can't do safely.
    bl_options = {"UNDO"}

    _timer = None
    _job = None

    @classmethod
    def poll(cls, context):
        if not bridge.available():
            cls.poll_message_set(f"QuadLoom backend unavailable: {bridge.unavailable_reason}")
            return False
        if job_running():
            cls.poll_message_set("A QuadLoom run is already in progress")
            return False
        obj = context.active_object
        if obj is None or obj.type != "MESH":
            cls.poll_message_set("Select a mesh object")
            return False
        # Faces may come from modifiers alone (Screw, Geometry Nodes, …); the
        # evaluated mesh is checked when the run starts.
        if len(obj.data.polygons) == 0 and not obj.modifiers:
            cls.poll_message_set("The active mesh has no faces")
            return False
        return True

    def execute(self, context):
        try:
            self._job = start_job(context)
        except Exception as e:
            last = context.scene.quadloom_last_run
            last.status, last.message, last.stage, last.hint = "ERROR", str(e), "validate", ""
            self.report({"ERROR"}, f"QuadLoom could not start: {e}")
            return {"CANCELLED"}
        wm = context.window_manager
        self._timer = wm.event_timer_add(0.1, window=context.window)
        wm.modal_handler_add(self)
        return {"RUNNING_MODAL"}

    def modal(self, context, event):
        job = self._job
        if job.discarded:
            # Dropped by undo, a file load or unregistering: never build its result.
            self._remove_timer(context)
            return {"CANCELLED"}
        if _active_job is not job and not job.done:
            # The job was discarded underneath us (add-on disabled, file loaded).
            self._remove_timer(context)
            return {"CANCELLED"}
        if event.type == "ESC" and event.value == "PRESS":
            job.control.cancel()
        if event.type != "TIMER":
            return {"PASS_THROUGH"}
        if not job.done:
            stage, fraction = job.control.progress()
            context.scene.quadloom_last_run.message = f"{stage.replace('_', ' ')} {fraction:.0%}"
            _redraw_panels(context)
            return {"RUNNING_MODAL"}
        self._remove_timer(context)
        result, (level, message) = finish_job(context, job)
        self.report({level}, message)
        _redraw_panels(context)
        return result

    def cancel(self, context):
        # Blender calls this when it drops a running modal operator (file load,
        # window closed). Stop the job and release the add-on for the next run.
        self._remove_timer(context)
        if _active_job is self._job:
            discard_active_job()

    def _remove_timer(self, context):
        if self._timer is not None:
            context.window_manager.event_timer_remove(self._timer)
            self._timer = None


class QUADLOOM_OT_cancel(bpy.types.Operator):
    bl_idname = "quadloom.cancel"
    bl_label = "Cancel"
    bl_description = "Stop the running QuadLoom job"

    @classmethod
    def poll(cls, context):
        return job_running()

    def execute(self, context):
        cancel_active_job()
        return {"FINISHED"}


def _run_job_blocking(context, timeout_s=30.0, on_started=None):
    """Test hook for headless Blender (no event loop): start, wait, finish.

    `on_started(job)` runs on the main thread right after the worker starts, so a
    test can cancel, delete objects, or check `poll()` while the job is in flight.
    """
    job = start_job(context)
    if on_started is not None:
        on_started(job)
    job.thread.join(timeout_s)
    if not job.done:
        job.control.cancel()
        job.thread.join(5.0)
    return finish_job(context, job)


classes = (QUADLOOM_OT_remesh, QUADLOOM_OT_cancel)


def register_handlers():
    if _on_load_pre not in bpy.app.handlers.load_pre:
        bpy.app.handlers.load_pre.append(_on_load_pre)
    for handlers in (bpy.app.handlers.undo_pre, bpy.app.handlers.redo_pre):
        if _on_undo_pre not in handlers:
            handlers.append(_on_undo_pre)


def unregister_handlers():
    if _on_load_pre in bpy.app.handlers.load_pre:
        bpy.app.handlers.load_pre.remove(_on_load_pre)
    for handlers in (bpy.app.handlers.undo_pre, bpy.app.handlers.redo_pre):
        if _on_undo_pre in handlers:
            handlers.remove(_on_undo_pre)
