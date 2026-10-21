# SPDX-License-Identifier: GPL-3.0-or-later
"""Remesh and cancel operators.

A run is a `Job`: the mesh is snapshotted on the main thread, the native call
runs on a worker thread (the backend releases the GIL), and the result is
turned into a new object back on the main thread. At most one job runs at a time.
"""

import threading
import time

import bpy

from . import bridge, mesh_io, props

_active_job = None


class Job:
    def __init__(self, snapshot, kwargs):
        self.snapshot = snapshot
        self.kwargs = kwargs
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
    snapshot = mesh_io.extract(context.active_object, context.evaluated_depsgraph_get(), settings.max_faces)
    kwargs = {"max_faces": settings.max_faces, "params": props.run_params(settings), **snapshot.feature_kwargs()}
    if settings.debug_fault and bridge.build_info().get("fault_injection"):
        kwargs["fault"] = settings.debug_fault
    job = Job(snapshot, kwargs)
    last = context.scene.quadloom_last_run
    last.status, last.message, last.stage, last.hint = "RUNNING", "Starting…", "", ""
    last.faces_in, last.faces_out, last.elapsed_ms, last.report_json = snapshot.face_count, 0, 0.0, ""
    last.repair_summary, last.feature_summary, last.remesh_summary = "", "", ""
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
    error = job.error
    if error is None:
        try:
            obj = mesh_io.build_result(job.snapshot, job.result, context.scene)
        except Exception as e:  # building the object failed; report, don't crash
            error = e
        else:
            last.status, last.message = "OK", f"Created {obj.name}"
            last.faces_out = len(obj.data.polygons)
            last.report_json = obj["quadloom_report"]
            last.repair_summary = repair_summary(job.result["repair"])
            last.feature_summary = feature_summary(job.result["features"])
            last.remesh_summary = remesh_summary(job.result.get("remesh"), job.kwargs["params"]["remesh"]["enabled"])
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
    # Ctrl+Z removes exactly the result object.
    bl_options = {"REGISTER", "UNDO"}

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


def unregister_handlers():
    if _on_load_pre in bpy.app.handlers.load_pre:
        bpy.app.handlers.load_pre.remove(_on_load_pre)
