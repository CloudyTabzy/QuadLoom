# SPDX-License-Identifier: GPL-3.0-or-later
"""Parent-side backend isolation. This module never calls the native pipeline."""

import atexit
import importlib.util
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import threading

from . import worker_limits as limits, worker_protocol as protocol


class WorkerError(Exception):
    def __init__(self, message, stage="validate", hint=None, kind="InternalError"):
        super().__init__(message)
        self.stage, self.hint, self.kind = stage, hint, kind


class WorkerCancelled(WorkerError):
    pass


class Control:
    def __init__(self):
        self.cancelled = threading.Event()
        self._progress = ("validate", 0.0)

    def cancel(self):
        self.cancelled.set()

    def is_cancelled(self):
        return self.cancelled.is_set()

    def progress(self):
        return self._progress

    def checkpoint(self):
        if self.cancelled.is_set():
            raise WorkerCancelled("cancelled by user", self._progress[0])


class Cache:
    """Owns a persistent child and its native cache; no native objects in parent."""

    def __init__(self):
        self._session = None
        self._lock = threading.Lock()
        self._filled = self._reusing = False

    @property
    def pid(self):
        session = self._session
        return session.process.pid if session is not None and session.process.poll() is None else None

    def filled(self):
        return self.pid is not None and self._filled

    def reusing(self):
        return self._reusing

    def clear(self):
        with self._lock:
            session, self._session = self._session, None
            self._filled = self._reusing = False
        if session is not None:
            session.close()

    def session(self, limit_mb, runtime):
        runtime = runtime if runtime is not None else (sys.executable, native_path())
        with self._lock:
            current = self._session
            if current is not None and (current.process.poll() is not None or current.limit_mb != limit_mb
                                        or current.runtime != runtime):
                current.close()
                self._session = None
            if self._session is None:
                self._session = _Session(limit_mb, runtime)
            return self._session


def python_executable():
    """Blender's sys.executable is blender.exe; use its bundled standalone Python."""
    if "bpy" not in sys.modules:
        return sys.executable
    import bpy
    directory = Path(bpy.utils.system_resource("PYTHON")) / "bin"
    names = ("python.exe",) if os.name == "nt" else ("python3", f"python{sys.version_info.major}.{sys.version_info.minor}")
    for name in names:
        candidate = directory / name
        if candidate.is_file():
            return str(candidate)
    raise WorkerError("Blender's bundled Python could not be found", hint="Reinstall Blender")


def native_path():
    spec = importlib.util.find_spec("quadloom_native")
    if spec is None or spec.origin is None:
        raise WorkerError("QuadLoom backend is unavailable")
    origin = Path(spec.origin).resolve()
    return origin.parent.parent if spec.submodule_search_locations is not None else origin.parent


class _Session:
    def __init__(self, limit_mb, runtime=None):
        self.limit_mb = limit_mb
        # Blender callers prepare paths on their main thread. Standalone
        # Python callers require no bpy discovery or API access at all.
        self.runtime = runtime if runtime is not None else (sys.executable, native_path())
        self.budget = limits.memory_budget(limit_mb)
        self.directory = tempfile.TemporaryDirectory(prefix="quadloom-worker-")
        self.path = Path(self.directory.name)
        self.process = self.job = None
        self._closed = False
        self._close_lock = threading.Lock()
        self._busy = threading.Lock()
        self._number = 0
        try:
            env = dict(os.environ)
            for name in ("CMAKE", "PYTHONPATH", "PYTHONHOME"):
                env.pop(name, None)
            self.process = subprocess.Popen(
                [str(self.runtime[0]), "-I", "-u", str(Path(__file__).with_name("worker_entry.py")),
                 str(self.path), str(self.runtime[1]), str(self.budget)],
                stdin=subprocess.PIPE, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                env=env, creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0,
            )
            # Child waits for stdin before importing native or reading geometry.
            if os.name == "nt":
                self.job = limits.WindowsJob(self.process, self.budget)
        except BaseException:
            self.close()
            raise
        atexit.register(self.close)

    def close(self):
        with self._close_lock:
            if self._closed:
                return
            self._closed = True
            process = self.process
            if self.job is not None:
                self.job.close()
                self.job = None
            if process is not None:
                if process.poll() is None:
                    process.kill()
                process.wait(timeout=5)
                if process.stdin is not None:
                    process.stdin.close()
            # A concurrent discard can interrupt an input writer. Reap the
            # process first, then let that request unwind before deleting files.
            with self._busy:
                self.directory.cleanup()
            atexit.unregister(self.close)

    def request(self, payload, control, cache):
        if not self._busy.acquire(blocking=False):
            raise WorkerError("the worker is already running a job")
        try:
            if self._closed:
                raise WorkerError("the worker was closed")
            control.checkpoint()
            directory = self.path / str(self._number)
            self._number += 1
            protocol.write(directory / "input", payload, self.budget // 2)
            control.checkpoint()
            self.process.stdin.write((directory.name + "\n").encode("ascii"))
            self.process.stdin.flush()
            while True:
                control.checkpoint()
                progress = directory / "progress.json"
                if progress.is_file():
                    data = protocol.read_json(progress)
                    control._progress = (data["stage"], data["fraction"])
                    cache._filled, cache._reusing = data["filled"], data["reusing"]
                if (directory / "output" / "manifest.json").is_file():
                    # Reading can duplicate buffers on the Blender side.
                    response = protocol.read(directory / "output", self.budget // 2,
                                             admit=lambda size: limits.admit_host_memory(size * 4))
                    control.checkpoint()
                    if "error" in response:
                        error = response["error"]
                        cls = WorkerCancelled if error["kind"] == "CancelledError" else WorkerError
                        raise cls(error["message"], error["stage"], error["hint"], error["kind"])
                    cache._filled, cache._reusing = response["filled"], response["reusing"]
                    control._progress = tuple(response["progress"])
                    shutil.rmtree(directory)
                    return response
                code = self.process.poll()
                if code is not None:
                    raise WorkerError(
                        f"QuadLoom worker stopped unexpectedly (exit {code}); it may have exceeded its memory limit",
                        control.progress()[0], "Lower Density or increase Worker Memory in System; retry the run",
                    )
                control.cancelled.wait(0.02)
        finally:
            self._busy.release()


def run(positions, indices, control, *, cache=None, memory_limit_mb=2048, mapping=None, runtime=None, **kwargs):
    own_cache = cache is None
    cache = cache if cache is not None else Cache()
    try:
        control.checkpoint()
        session = cache.session(memory_limit_mb, runtime)
        return session.request({"positions": positions, "indices": indices, "kwargs": kwargs,
                                "mapping": mapping, "keep_cache": not own_cache}, control, cache)
    except Exception as error:
        cache.clear()
        control.checkpoint()
        if isinstance(error, WorkerError):
            raise
        raise WorkerError(f"QuadLoom worker failed: {error}", control.progress()[0],
                          "Check free memory and temporary disk space, then retry") from error
    except BaseException:
        cache.clear()
        raise
    finally:
        if own_cache:
            cache.clear()
