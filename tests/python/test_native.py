"""Tests for the quadloom_native extension module (built with fault injection)."""

import math
import struct
import threading
import time
from array import array

import pytest

import quadloom_native as ql

TET_POSITIONS = [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]
TET_INDICES = [0, 2, 1, 0, 1, 3, 1, 2, 3, 2, 0, 3]
NO_REMESH = {"remesh": {"enabled": False}}


def tet():
    return array("f", TET_POSITIONS), array("I", TET_INDICES)


def test_build_has_fault_injection():
    info = ql.build_info()
    assert info["fault_injection"] is True
    assert info["version"] == ql.version()


def test_tetrahedron_roundtrip():
    p, i = tet()
    result = ql.run(p, i, ql.Control(), params=NO_REMESH)
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


def test_limits_checked_before_conversion():
    # Over the face limit *and* the wrong positions dtype: the size check must
    # win, proving it runs before any buffer is converted or copied.
    _, i = tet()
    with pytest.raises(ql.InvalidInputError) as e:
        ql.run(array("d", TET_POSITIONS), i, ql.Control(), max_faces=1)
    assert "limit" in str(e.value)


def test_too_many_vertices_rejected():
    with pytest.raises(ql.InvalidInputError) as e:
        ql.run(array("f", [0.0] * 12), array("I", [0, 1, 2]), ql.Control(), max_faces=1)
    assert "vertices" in str(e.value)


CUBE_P = [0, 0, 0, 1, 0, 0, 1, 1, 0, 0, 1, 0, 0, 0, 1, 1, 0, 1, 1, 1, 1, 0, 1, 1]
CUBE_T = [0, 3, 2, 0, 2, 1, 4, 5, 6, 4, 6, 7, 0, 1, 5, 0, 5, 4, 3, 7, 6, 3, 6, 2, 0, 4, 7, 0, 7, 3, 1, 2, 6, 1, 6, 5]


def cube():
    return array("f", CUBE_P), array("I", CUBE_T)


def test_repair_and_feature_reports_present():
    p, i = cube()
    result = ql.run(p, i, ql.Control(), params=NO_REMESH)
    assert result["components"] == 1
    assert result["repair"]["welded_vertices"] == 0
    assert result["features"]["edges"] == 12
    assert result["features"]["corners"] == 8
    pairs = struct.unpack(f"<{2 * 12}I", result["feature_edges"])
    assert len(pairs) == 24


def test_params_select_feature_sources():
    p, i = cube()
    result = ql.run(p, i, ql.Control(), params={"features": {"use_angle": False}})
    assert result["features"]["edges"] == 0


def test_params_unknown_key_rejected():
    p, i = cube()
    with pytest.raises(ql.InvalidInputError) as e:
        ql.run(p, i, ql.Control(), params={"features": {"angel_deg": 30.0}})
    assert "features.angel_deg" in str(e.value)
    with pytest.raises(ql.InvalidInputError) as e:
        ql.run(p, i, ql.Control(), params={"smoothing": {}})
    assert "smoothing" in str(e.value)


def test_params_wrong_type_rejected():
    p, i = cube()
    with pytest.raises(ql.InvalidInputError) as e:
        ql.run(p, i, ql.Control(), params={"repair": {"orient": "yes"}})
    assert "repair.orient" in str(e.value)


def test_remesh_section():
    p, i = cube()
    result = ql.run(p, i, ql.Control(), params={"remesh": {"target_edge_rel": 0.1, "adaptive": False}})
    faces = len(result["face_offsets"]) // 4 - 1
    assert result["remesh"]["input_faces"] == 12
    assert result["remesh"]["output_faces"] == faces >= 300
    assert result["remesh"]["mean_radius_ratio"] > 0.8
    off = ql.run(p, i, ql.Control(), params=NO_REMESH)
    assert len(off["face_offsets"]) // 4 - 1 == 12


def test_remesh_range_checked():
    p, i = cube()
    with pytest.raises(ql.InvalidInputError) as e:
        ql.run(p, i, ql.Control(), params={"remesh": {"max_mult": 50.0}})
    assert "remesh.max_mult" in str(e.value)


@pytest.mark.parametrize("value", [float("nan"), float("inf"), float("-inf")])
def test_non_finite_param_rejected(value):
    p, i = cube()
    with pytest.raises(ql.InvalidInputError) as e:
        ql.run(p, i, ql.Control(), params={"repair": {"weld_epsilon_rel": value}})
    assert "repair.weld_epsilon_rel" in str(e.value)


def test_weld_distance_capped():
    p, i = cube()
    with pytest.raises(ql.InvalidInputError) as e:
        ql.run(p, i, ql.Control(), params={"repair": {"weld_epsilon_rel": 1.0}})
    assert "weld_epsilon_rel" in str(e.value)


def test_marked_sharp_edges_used():
    p, i = cube()
    params = {"features": {"use_angle": False, "use_open_boundaries": False}, **NO_REMESH}
    result = ql.run(p, i, ql.Control(), sharp_edges=array("I", [0, 1, 1, 2]), params=params)
    assert result["features"]["edges"] == 2
    assert result["features"]["by_source"][1] == 2


def test_empty_buffers_accepted():
    # An empty array's data pointer isn't aligned; empty inputs must still work.
    p, i = cube()
    result = ql.run(p, i, ql.Control(), sharp_edges=array("i"), seam_edges=array("I"),
                    material=array("i", [0] * 12), face_set=array("i", [0] * 12))
    assert result["features"]["edges"] == 12


def test_material_length_must_match():
    p, i = cube()
    with pytest.raises(ql.InvalidInputError) as e:
        ql.run(p, i, ql.Control(), material=array("i", [0, 1]))
    assert "material" in str(e.value)


def test_odd_edge_buffer_rejected():
    p, i = cube()
    with pytest.raises(ql.InvalidInputError) as e:
        ql.run(p, i, ql.Control(), seam_edges=array("I", [0, 1, 2]))
    assert "seam_edges" in str(e.value)


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


def test_field_bytes_are_one_finite_direction_per_face():
    p, i = tet()
    result = ql.run(p, i, ql.Control())
    field = result["field"]
    faces = len(result["face_offsets"]) // 4 - 1
    directions = struct.unpack(f"<{faces * 3}f", field["directions"])
    assert all(abs(x) <= 1.0 + 1e-6 for x in directions)
    assert len(field["singular_vertices"]) == len(field["singular_quarters"])
    report = field["report"]
    quarters = struct.unpack(f"<{len(field['singular_quarters']) // 4}i", field["singular_quarters"])
    # A closed genus-0 surface: indices sum to χ = 2, i.e. 8 quarters.
    assert sum(quarters) == 8
    assert report["positive_quarters"] + report["negative_quarters"] == 8
    assert report["fallback_parts"] == 0


def test_field_section_parsed_and_checked():
    p, i = tet()
    off = ql.run(p, i, ql.Control(), params={"field": {"enabled": False}})
    assert off["field"] is None
    on = ql.run(p, i, ql.Control(), params={"field": {"curvature_alignment": "on", "alignment_weight": 1.0}})
    assert on["field"]["report"]["curvature_aligned"] is True
    with pytest.raises(ql.InvalidInputError) as e:
        ql.run(p, i, ql.Control(), params={"field": {"curvature_alignment": "sometimes"}})
    assert "field.curvature_alignment" in str(e.value)
    with pytest.raises(ql.InvalidInputError):
        ql.run(p, i, ql.Control(), params={"field": {"alignment_weight": -1.0}})


def test_trace_section_and_bytes():
    p, i = cube()
    result = ql.run(p, i, ql.Control())
    trace = result["trace"]
    faces = len(trace["triangles"]) // 12
    assert faces > 0 and len(trace["positions"]) % 12 == 0
    patch = struct.unpack(f"<{faces}I", trace["patch_of_face"])
    report = trace["report"]
    assert report["patches"] == 6 and report["unsolved"] == 0
    assert set(patch) == set(range(report["patches"]))
    offsets = struct.unpack(f"<{report['patches'] + 1}I", trace["corner_offsets"])
    assert offsets[0] == 0 and offsets[-1] * 4 == len(trace["corners"])
    lengths = struct.unpack(f"<{offsets[-1]}d", trace["side_lengths"])
    assert all(math.isfinite(x) and x > 0 for x in lengths)
    assert len(trace["kinds"]) == report["patches"]
    assert set(trace["kind_names"]) >= {"ok", "non_disk"}
    off = ql.run(p, i, ql.Control(), params={"trace": {"enabled": False}})
    assert off["trace"] is None
    with pytest.raises(ql.InvalidInputError) as e:
        ql.run(p, i, ql.Control(), params={"trace": {"max_sides": 9}})
    assert "trace.max_sides" in str(e.value)
    with pytest.raises(ql.InvalidInputError) as e:
        ql.run(p, i, ql.Control(), params={"trace": {"cost": "cheap"}})
    assert "params.trace.cost" in str(e.value)
    hexes = ql.run(p, i, ql.Control(), params={"trace": {"max_sides": 6, "cost": "angle_penalty", "drift": 100.0}})
    assert hexes["trace"]["report"]["unsolved"] == 0

