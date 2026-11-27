"""Real isolated backend runs; no Blender import or system memory exhaustion."""

import array
import importlib
import json
import os
from pathlib import Path
import sys
import shutil
import subprocess
import threading
import time
import types

import pytest

ROOT = Path(__file__).resolve().parents[2]


@pytest.fixture(scope="module")
def worker():
    package = types.ModuleType("quadloom_worker_test")
    package.__path__ = [str(ROOT / "addon" / "quadloom")]
    sys.modules[package.__name__] = package
    return importlib.import_module(package.__name__ + ".worker")


def buffers():
    return array.array("f", [0, 0, 0, 1, 0, 0, 0, 1, 0]), array.array("I", [0, 1, 2])


def run(worker, control=None, **kwargs):
    return worker.run(*buffers(), control or worker.Control(),
                      params={"remesh": {"enabled": False}, "trace": {"enabled": False},
                              "repair": {"min_component_faces": 0}}, **kwargs)


def test_roundtrip_and_persistent_cache(worker):
    cache = worker.Cache()
    try:
        first = run(worker, cache=cache)
        assert first["result"]["positions"] == buffers()[0].tobytes()
        assert not first["result"]["reused"]
        assert cache.filled()
        child_pid = cache.pid
        second = run(worker, cache=cache)
        assert second["result"]["reused"]
        assert cache.pid == child_pid != os.getpid()
        assert cache.reusing()
    finally:
        cache.clear()
    assert not cache.filled() and cache.pid is None


def test_finished_run_keeps_final_progress(worker):
    control = worker.Control()
    run(worker, control=control)
    assert control.progress()[0] != "validate"


def test_close_waits_for_inflight_input_writer(worker, monkeypatch):
    protocol = importlib.import_module(worker.__package__ + ".worker_protocol")
    entered, resume = threading.Event(), threading.Event()
    original = protocol.write

    def paused(directory, *args, **kwargs):
        entered.set()
        assert resume.wait(3)
        return original(directory, *args, **kwargs)

    monkeypatch.setattr(protocol, "write", paused)
    cache, errors = worker.Cache(), []

    def work():
        try:
            run(worker, cache=cache)
        except Exception as error:
            errors.append(error)

    thread = threading.Thread(target=work)
    thread.start()
    assert entered.wait(3)
    session = cache._session
    closer = threading.Thread(target=cache.clear)
    closer.start()
    time.sleep(0.05)
    still_closing = closer.is_alive()
    resume.set()
    thread.join(3)
    closer.join(3)
    assert still_closing, "cleanup must wait until input writers release their files"
    assert not thread.is_alive() and not closer.is_alive()
    assert not session.path.exists()
    assert errors


@pytest.mark.parametrize("fault", ["worker:abort", "worker:oom"])
def test_child_failure_is_contained_and_next_run_recovers(worker, fault):
    cache = worker.Cache()
    try:
        with pytest.raises(worker.WorkerError):
            run(worker, cache=cache, fault=fault, memory_limit_mb=256)
        assert not cache.filled()
        assert run(worker, cache=cache)["result"]["repair"]
    finally:
        cache.clear()


def test_cancel_before_start_does_not_spawn(worker):
    cache, control = worker.Cache(), worker.Control()
    control.cancel()
    with pytest.raises(worker.WorkerCancelled):
        run(worker, cache=cache, control=control)
    assert cache.pid is None


def test_cancel_uncooperative_worker_reaps_child(worker):
    cache, control = worker.Cache(), worker.Control()
    failures = []

    def work():
        try:
            run(worker, cache=cache, control=control, fault="worker:stall")
        except Exception as error:
            failures.append(error)

    thread = threading.Thread(target=work)
    thread.start()
    deadline = time.monotonic() + 5
    while cache.pid is None and time.monotonic() < deadline:
        time.sleep(0.01)
    assert cache.pid is not None
    started = time.monotonic()
    control.cancel()
    thread.join(3)
    cache.clear()
    assert not thread.is_alive()
    assert time.monotonic() - started < 3
    assert len(failures) == 1 and isinstance(failures[0], worker.WorkerCancelled)


def test_native_error_keeps_stage_and_hint(worker):
    with pytest.raises(worker.WorkerError) as caught:
        run(worker, fault="panic:validate")
    assert caught.value.stage == "validate"
    assert "please report" in str(caught.value)
    assert caught.value.hint


def test_mapping_runs_in_child(worker):
    result = run(worker, mapping={"corner_keys": None, "face_keys": None})
    assert len(result["mapping"]["vertex_triangles"]) == 12
    assert result["transfer_error"] is None


def test_protocol_rejects_oversized_and_truncated_buffers(worker, tmp_path):
    protocol = importlib.import_module(worker.__package__ + ".worker_protocol")
    protocol.write(tmp_path, {"mesh": array.array("I", [0, 1, 2])}, 1024)
    with pytest.raises(ValueError, match="budget"):
        protocol.read(tmp_path, 8)
    (tmp_path / "0.bin").write_bytes(b"\x00")
    with pytest.raises(ValueError, match="size|truncated"):
        protocol.read(tmp_path, 1024)


def test_protocol_rejects_path_and_huge_metadata(worker, tmp_path):
    protocol = importlib.import_module(worker.__package__ + ".worker_protocol")
    protocol.write(tmp_path, {"mesh": b"abc"}, 1024)
    manifest = tmp_path / "manifest.json"
    manifest.write_text(json.dumps({"$buffer": "../outside", "size": 3, "type": "bytes"}))
    with pytest.raises(ValueError):
        protocol.read(tmp_path, 1024)
    manifest.write_bytes(b" " * (protocol.MAX_MANIFEST + 1))
    with pytest.raises(ValueError):
        protocol.read(tmp_path, 1024)


def test_limit_really_refuses_large_allocation(worker):
    # The fault allocates beyond a 256 MiB OS limit inside the child only.
    # A MemoryError response proves allocation denial, rather than just a crash.
    with pytest.raises(worker.WorkerError, match="memory|Memory"):
        run(worker, fault="worker:oom", memory_limit_mb=256)


def test_native_allocator_abort_is_contained(worker):
    with pytest.raises(worker.WorkerError, match="stopped unexpectedly"):
        run(worker, fault="oom:validate", memory_limit_mb=256)


@pytest.mark.skipif(os.name != "nt", reason="Windows Job Object failure path")
def test_limit_setup_failure_refuses_work_and_reaps_child(worker, monkeypatch):
    children = []
    original = worker.subprocess.Popen

    def launch(*args, **kwargs):
        process = original(*args, **kwargs)
        children.append(process)
        return process

    def denied(*args, **kwargs):
        raise OSError("injected Job Object denial")

    monkeypatch.setattr(worker.subprocess, "Popen", launch)
    monkeypatch.setattr(worker.limits, "WindowsJob", denied)
    with pytest.raises(worker.WorkerError, match="Job Object denial") as caught:
        run(worker)
    assert caught.value.stage == "validate"
    assert len(children) == 1 and children[0].poll() is not None


@pytest.mark.skipif(os.name != "nt", reason="Windows orphan lifetime probe")
def test_abrupt_parent_exit_does_not_leave_a_worker(worker):
    import ctypes as c
    from ctypes import wintypes as w

    # A separate parent dies without atexit/finalizers, as Blender could.
    source = """
import importlib, json, os, sys, types
package = types.ModuleType('orphan_probe')
package.__path__ = [sys.argv[1]]
sys.modules[package.__name__] = package
worker = importlib.import_module('orphan_probe.worker')
session = worker._Session(256)
print(json.dumps({'pid': session.process.pid, 'path': str(session.path)}), flush=True)
os._exit(0)
"""
    parent = subprocess.run([sys.executable, "-c", source, str(ROOT / "addon" / "quadloom")],
                            capture_output=True, text=True, timeout=10)
    assert parent.returncode == 0, parent.stderr
    child = json.loads(parent.stdout)
    kernel = c.WinDLL("kernel32", use_last_error=True)
    kernel.OpenProcess.argtypes = [w.DWORD, w.BOOL, w.DWORD]
    kernel.OpenProcess.restype = w.HANDLE
    kernel.WaitForSingleObject.argtypes = [w.HANDLE, w.DWORD]
    kernel.WaitForSingleObject.restype = w.DWORD
    kernel.CloseHandle.argtypes = [w.HANDLE]
    handle = kernel.OpenProcess(0x100000, False, child["pid"])
    try:
        if handle:
            assert kernel.WaitForSingleObject(handle, 3000) == 0, "worker survived parent exit"
    finally:
        if handle:
            kernel.CloseHandle(handle)
        shutil.rmtree(child["path"], ignore_errors=True)


@pytest.mark.parametrize("failure", ["budget", "truncated"])
def test_protocol_checks_later_descriptors_before_allocating(worker, monkeypatch, tmp_path, failure):
    protocol = importlib.import_module(worker.__package__ + ".worker_protocol")
    protocol.write(tmp_path, [array.array("I", [1, 2]), array.array("I", [3, 4])], 1024)
    if failure == "truncated":
        (tmp_path / "1.bin").write_bytes(b"\x00")

    def forbidden(*args, **kwargs):
        pytest.fail("all descriptors must be validated before allocating any array")

    monkeypatch.setattr(protocol, "array", forbidden)
    with pytest.raises(ValueError):
        protocol.read(tmp_path, 12 if failure == "budget" else 1024)


def test_repeated_buffer_descriptors_count_toward_host_admission(worker, tmp_path):
    protocol = importlib.import_module(worker.__package__ + ".worker_protocol")
    protocol.write(tmp_path, [array.array("I", [1, 2])], 1024)
    path = tmp_path / "manifest.json"
    descriptor = json.loads(path.read_text())[0]
    path.write_text(json.dumps([descriptor, descriptor]))
    admitted = []
    assert len(protocol.read(tmp_path, 1024, admit=admitted.append)) == 2
    assert admitted == [16], "admit decoded sizes, even when descriptors reuse a file"


def test_prepared_runtime_uses_no_bpy_on_controller_thread(worker, monkeypatch):
    calls = []

    def resolve_executable():
        calls.append(threading.current_thread())
        assert threading.current_thread() is threading.main_thread(), "bpy called in background thread"
        return sys.executable

    monkeypatch.setattr(worker, "python_executable", resolve_executable)
    # The operator prepares this on the main thread and passes plain paths.
    executable = worker.python_executable()
    failures, results = [], []

    def work():
        try:
            results.append(run(worker, runtime=(executable, worker.native_path())))
        except Exception as error:
            failures.append(error)

    thread = threading.Thread(target=work)
    thread.start()
    thread.join(10)
    assert not thread.is_alive()
    assert not failures, failures
    assert results[0]["result"]["validation"]["faces"] == 1
    assert calls == [threading.main_thread()]


# --- Progress is advisory: a locked or half-replaced file never fails a run ---
#
# On Windows the child replaces progress.json while the parent polls it. Opening
# a file that is being replaced, or replacing one the other side has open, is
# refused with PermissionError. Under load that failed about one run in five.


def test_locked_progress_file_does_not_fail_the_run(worker, monkeypatch):
    protocol = importlib.import_module(worker.__package__ + ".worker_protocol")
    original, refused = protocol.read_json, []

    def locked(path):
        if Path(path).name == "progress.json":
            refused.append(path)
            raise PermissionError(13, "Permission denied", str(path))
        return original(path)

    monkeypatch.setattr(protocol, "read_json", locked)
    result = run(worker, fault="slow:300")
    assert refused, "the run must have polled its progress at least once"
    assert result["result"]["validation"]["faces"] == 1


def test_progress_reader_ignores_missing_locked_and_malformed_files(worker, monkeypatch, tmp_path):
    protocol = importlib.import_module(worker.__package__ + ".worker_protocol")
    path = tmp_path / "progress.json"
    assert protocol.read_progress(path) is None, "not published yet"
    good = {"stage": "trace", "fraction": 0.5, "filled": False, "reusing": True}
    protocol.publish(path, good)
    assert protocol.read_progress(path) == good
    for broken in (b"", b'{"stage": "tra', b"[]", b'{"stage": 3, "fraction": 0.5, "filled": false, "reusing": false}',
                   b'{"stage": "trace", "fraction": "half", "filled": false, "reusing": false}',
                   b'{"stage": "trace", "fraction": 0.5}'):
        path.write_bytes(broken)
        assert protocol.read_progress(path) is None, broken

    def locked(_path):
        raise PermissionError(13, "Permission denied")

    monkeypatch.setattr(protocol, "read_json", locked)
    assert protocol.read_progress(path) is None


def test_progress_writer_survives_a_reader_holding_the_file(worker, monkeypatch, tmp_path):
    protocol = importlib.import_module(worker.__package__ + ".worker_protocol")
    path = tmp_path / "progress.json"
    value = {"stage": "trace", "fraction": 0.25, "filled": False, "reusing": False}

    def refused(_source, _target):
        raise PermissionError(13, "Permission denied")

    with monkeypatch.context() as patch:
        patch.setattr(protocol.os, "replace", refused)
        assert protocol.publish_progress(path, value) is False
    assert protocol.publish_progress(path, value) is True, "the next tick publishes again"
    assert protocol.read_progress(path) == value


@pytest.mark.skipif(os.name != "nt", reason="Windows refuses to replace a file another handle holds open")
def test_progress_writer_survives_a_real_open_handle(worker, tmp_path):
    protocol = importlib.import_module(worker.__package__ + ".worker_protocol")
    path = tmp_path / "progress.json"
    first = {"stage": "validate", "fraction": 0.0, "filled": False, "reusing": False}
    second = {**first, "stage": "trace"}
    assert protocol.publish_progress(path, first) is True
    with path.open("rb"):
        assert protocol.publish_progress(path, second) is False
    assert protocol.publish_progress(path, second) is True
    assert protocol.read_progress(path) == second


def test_progress_polling_races_replacement_without_errors(worker, tmp_path):
    """The real race: one thread republishes while another polls, as child and parent do."""
    protocol = importlib.import_module(worker.__package__ + ".worker_protocol")
    path = tmp_path / "progress.json"
    stop, failures, seen = threading.Event(), [], []

    def publish():
        n = 0
        while not stop.is_set():
            try:
                protocol.publish_progress(path, {"stage": "trace", "fraction": (n % 100) / 100,
                                                 "filled": False, "reusing": False})
            except Exception as error:
                failures.append(error)
                return
            n += 1

    def poll():
        while not stop.is_set():
            try:
                data = protocol.read_progress(path)
            except Exception as error:
                failures.append(error)
                return
            if data is not None:
                seen.append(data["fraction"])

    threads = [threading.Thread(target=publish), threading.Thread(target=poll), threading.Thread(target=poll)]
    for thread in threads:
        thread.start()
    time.sleep(0.5)
    stop.set()
    for thread in threads:
        thread.join(5)
    assert not failures, failures[:3]
    assert seen, "the reader must still see progress between replacements"


def test_manifest_publish_retries_a_briefly_held_file(worker, monkeypatch, tmp_path):
    protocol = importlib.import_module(worker.__package__ + ".worker_protocol")
    original, attempts = protocol.os.replace, []

    def briefly_held(source, target):
        attempts.append(target)
        if len(attempts) < 3:
            raise PermissionError(13, "Permission denied")
        return original(source, target)

    monkeypatch.setattr(protocol.os, "replace", briefly_held)
    protocol.write(tmp_path, {"mesh": array.array("I", [0, 1, 2])}, 1024)
    assert len(attempts) == 3
    assert protocol.read(tmp_path, 1024)["mesh"] == array.array("I", [0, 1, 2])


def test_manifest_publish_gives_up_on_a_file_that_stays_held(worker, monkeypatch, tmp_path):
    protocol = importlib.import_module(worker.__package__ + ".worker_protocol")

    def held(_source, _target):
        raise PermissionError(13, "Permission denied")

    monkeypatch.setattr(protocol.os, "replace", held)
    monkeypatch.setattr(protocol.time, "sleep", lambda _seconds: None)
    with pytest.raises(PermissionError):
        protocol.write(tmp_path, {"mesh": b"abc"}, 1024)


def test_cleanup_failure_keeps_the_finished_result(worker, monkeypatch):
    original, refused = worker.shutil.rmtree, []

    def locked(path, ignore_errors=False, **kwargs):
        # Only a job's own folder (<session>/<number>) is held; the session's
        # folder is removed by tempfile, later, through the same function.
        if Path(path).name.isdecimal():
            attempted.append(path)
            if not ignore_errors:
                refused.append(path)
                raise PermissionError(13, "Permission denied", str(path))
            return None
        return original(path, ignore_errors=ignore_errors, **kwargs)

    attempted = []
    monkeypatch.setattr(worker.shutil, "rmtree", locked)
    result = run(worker)
    assert attempted, "the finished job's folder is still cleaned up when possible"
    assert not refused, "removing a finished job's files must not be able to fail the run"
    assert result["result"]["validation"]["faces"] == 1
