# SPDX-License-Identifier: GPL-3.0-or-later
"""Standalone worker. Never import bpy or the add-on's __init__."""

from array import array
import os
from pathlib import Path
import queue
import sys
import threading
import time

# -I excludes the script directory as well as user/site paths. Only this
# installed add-on directory and the parent's native-module path are added.
sys.path.insert(0, str(Path(__file__).resolve().parent))
import worker_limits as limits
import worker_protocol as protocol


def read_commands(commands):
    for line in sys.stdin.buffer:
        name = line.decode("ascii").strip()
        if not name.isdecimal() or len(name) > 16:
            os._exit(2)
        commands.put(name)
    # Parent exited/closed the pipe. Also interrupts blocked native work.
    os._exit(0)


def final_geometry(result):
    geometry = result.get("quads") or result.get("trace") or result
    positions = array("f", geometry["positions"])
    if "triangles" in geometry:
        corners = array("I", geometry["triangles"])
        offsets = array("I", range(0, len(corners) + 1, 3))
    else:
        corners = array("I", geometry["face_indices"])
        offsets = array("I", geometry["face_offsets"])
    return positions, offsets, corners


def report_progress(directory, control, cache, stop):
    while not stop.is_set():
        stage, fraction = control.progress()
        protocol.publish(directory / "progress.json", {
            "stage": stage, "fraction": fraction, "filled": cache.filled(), "reusing": cache.reusing()})
        stop.wait(0.05)


def execute(native, cache, request, control, budget):
    kwargs = request["kwargs"]
    fault = kwargs.get("fault", "") or ""
    if fault.startswith("worker:"):
        if not native.build_info().get("fault_injection"):
            raise ValueError("worker fault injection requires a development build")
        kwargs.pop("fault")
        if fault == "worker:abort":
            # Abrupt native-process equivalent; no unwinding or Python cleanup.
            os._exit(86)
        if fault == "worker:oom":
            bytearray(budget * 2)  # real allocation denied by the OS cap
            raise RuntimeError("worker memory limit did not deny an oversized allocation")
        if fault == "worker:stall":
            time.sleep(3600)  # deliberately ignores cooperative cancellation
        else:
            raise ValueError("unknown worker fault")
    result = native.run(request["positions"], request["indices"], control,
                        cache=cache if request["keep_cache"] else None, **kwargs)
    mapping, transfer_error = None, None
    if request["mapping"] is not None:
        try:
            positions, offsets, corners = final_geometry(result)
            mapping = native.transfer_map(request["positions"], request["indices"],
                                          positions, offsets, corners, control, **request["mapping"])
        except native.CancelledError:
            raise
        except Exception as error:
            transfer_error = str(error)
    return {"result": result, "mapping": mapping, "transfer_error": transfer_error,
            "filled": cache.filled(), "reusing": cache.reusing(), "progress": control.progress()}


def main():
    directory, native_path, budget = Path(sys.argv[1]), sys.argv[2], int(sys.argv[3])
    commands = queue.Queue()
    threading.Thread(target=read_commands, args=(commands,), daemon=True).start()
    native = cache = None
    while True:
        job = directory / commands.get()
        control, stop, reporter = None, threading.Event(), None
        try:
            limits.limit_child(budget)
            if native is None:
                sys.path.insert(0, native_path)
                import quadloom_native as native
                cache = native.Cache()
            request = protocol.read(job / "input", budget // 2)
            control = native.Control()
            reporter = threading.Thread(target=report_progress, args=(job, control, cache, stop), daemon=True)
            reporter.start()
            response = execute(native, cache, request, control, budget)
            stop.set()
            reporter.join()
            # Drop input buffers before writing the output.
            del request
            protocol.write(job / "output", response, budget // 2)
            del response
        except BaseException as error:
            stop.set()
            if reporter is not None:
                reporter.join()
            stage = getattr(error, "stage", None) or (control.progress()[0] if control is not None else "validate")
            protocol.write(job / "output", {"error": {
                "kind": type(error).__name__, "message": str(error) or type(error).__name__,
                "stage": stage, "hint": getattr(error, "hint", None)}}, budget // 2)
        finally:
            stop.set()
            if reporter is not None:
                reporter.join()


if __name__ == "__main__":
    main()
