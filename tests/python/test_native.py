"""Tests for the quadloom_native extension module (built with fault injection)."""

import struct
import threading
import time
from array import array

import pytest

import quadloom_native as ql

TET_POSITIONS = [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]
TET_INDICES = [0, 2, 1, 0, 1, 3, 1, 2, 3, 2, 0, 3]


def tet():
    return array("f", TET_POSITIONS), array("I", TET_INDICES)


def test_build_has_fault_injection():
    info = ql.build_info()
    assert info["fault_injection"] is True
    assert info["version"] == ql.version()


def test_tetrahedron_roundtrip():
    p, i = tet()
    result = ql.run(p, i, ql.Control())
    assert len(result["face_offsets"]) == 5 * 4
    assert result["validation"]["faces"] == 4
    assert result["validation"]["components"] == 1
    positions = struct.unpack("<12f", result["positions"])
    assert positions == pytest.approx(TET_POSITIONS)
    assert struct.unpack("<12I", result["face_indices"]) == tuple(TET_INDICES)
    stages = [stage for stage, _ms in result["timings_ms"]]
    assert stages[0] == "validate"


def test_exception_hierarchy():
    for cls in (ql.InvalidInputError, ql.CancelledError, ql.InternalError):
        assert issubclass(cls, ql.QuadLoomError)
    assert issubclass(ql.QuadLoomError, Exception)


def test_float64_positions_rejected():
    _, i = tet()
    with pytest.raises(ql.InvalidInputError) as e:
        ql.run(array("d", TET_POSITIONS), i, ql.Control())
    assert "positions" in str(e.value)
    assert e.value.stage == "validate"


def test_negative_index_rejected():
    p, _ = tet()
    with pytest.raises(ql.InvalidInputError) as e:
        ql.run(p, array("i", [0, 1, -1]), ql.Control())
    assert "indices" in str(e.value)


def test_int64_indices_rejected():
    p, _ = tet()
    with pytest.raises(ql.InvalidInputError) as e:
        ql.run(p, array("q", TET_INDICES), ql.Control())
    assert "indices" in str(e.value)


def test_non_buffer_rejected():
    _, i = tet()
    with pytest.raises(ql.InvalidInputError):
        ql.run(TET_POSITIONS, i, ql.Control())  # a list, not a buffer


def test_odd_length_rejected():
    _, i = tet()
    with pytest.raises(ql.InvalidInputError):
        ql.run(array("f", [0.0] * 4), i, ql.Control())


def test_oversized_rejected():
    p, i = tet()
    with pytest.raises(ql.InvalidInputError) as e:
        ql.run(p, i, ql.Control(), max_faces=1)
    assert e.value.hint


def test_panic_becomes_internal_error():
    p, i = tet()
    with pytest.raises(ql.InternalError) as e:
        ql.run(p, i, ql.Control(), fault="panic:validate")
    assert isinstance(e.value, Exception)
    assert e.value.stage == "validate"
    assert "please report" in str(e.value)


def test_injected_error_is_internal_error():
    p, i = tet()
    with pytest.raises(ql.InternalError) as e:
        ql.run(p, i, ql.Control(), fault="error:output")
    assert e.value.stage == "output"


def test_unknown_fault_rejected():
    p, i = tet()
    with pytest.raises(ql.InvalidInputError):
        ql.run(p, i, ql.Control(), fault="explode:now")


def test_cancel_from_other_thread():
    p, i = tet()
    control = ql.Control()
    outcome = {}

    def worker():
        try:
            ql.run(p, i, control, fault="slow:5000")
        except ql.QuadLoomError as e:
            outcome["error"] = e

    thread = threading.Thread(target=worker)
    started = time.monotonic()
    thread.start()
    time.sleep(0.05)
    assert control.progress()[0] == "validate"
    control.cancel()
    thread.join(timeout=2)
    assert not thread.is_alive()
    assert time.monotonic() - started < 1.0
    assert isinstance(outcome["error"], ql.CancelledError)
    assert control.is_cancelled()


def test_gil_released():
    p, i = tet()
    thread = threading.Thread(target=ql.run, args=(p, i, ql.Control()), kwargs={"fault": "slow:300"})
    counter = 0
    thread.start()
    while thread.is_alive():
        counter += 1
    thread.join()
    assert counter > 1000
