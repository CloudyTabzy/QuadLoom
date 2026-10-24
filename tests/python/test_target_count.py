"""0.4 count control: actual topology, accuracy, symmetry and resource bounds."""

from array import array
from collections import Counter
import math
import threading
import time

import pytest
import quadloom_native as native

from test_cache import torus


def cube(scale=1.0, center=False):
    points = [(0,0,0), (1,0,0), (1,1,0), (0,1,0), (0,0,1), (1,0,1), (1,1,1), (0,1,1)]
    faces = [(0,3,2,1), (4,5,6,7), (0,1,5,4), (1,2,6,5), (2,3,7,6), (3,0,4,7)]
    positions = array("f", ((c - (0.5 if center else 0))*scale for p in points for c in p))
    indices = array("I", (v for a,b,c,d in faces for v in (a,b,c,a,c,d)))
    return positions, indices


def geometry(result):
    output = result["quads"]
    assert output is not None, result.get("quad_error")
    return tuple(output[key] for key in ("positions", "face_offsets", "face_indices", "feature_edges"))


def check_topology(result, closed=True):
    output = result["quads"]
    assert output is not None, result.get("quad_error")
    positions = array("f", output["positions"])
    offsets, indices = array("I", output["face_offsets"]), array("I", output["face_indices"])
    assert all(math.isfinite(v) for v in positions)
    assert all(b-a == 4 for a,b in zip(offsets, offsets[1:]))
    assert max(indices) < len(positions)//3
    edges, directed = Counter(), Counter()
    for a,b in zip(offsets, offsets[1:]):
        face = indices[a:b]
        assert len(set(face)) == 4
        for i, v in enumerate(face):
            w = face[(i+1)%4]
            edges[min(v,w), max(v,w)] += 1
            directed[v,w] += 1
    assert max(edges.values()) <= 2
    assert max(directed.values()) == 1
    if closed:
        assert min(edges.values()) == 2
    else:
        assert 1 in edges.values()
    assert output["report"]["quads"] == len(offsets)-1
    return len(offsets)-1


@pytest.mark.parametrize("shape", [cube, torus])
def test_feasible_count_sweep_preserves_closed_oriented_quads(shape):
    positions, indices = shape()
    cache = native.Cache()
    achieved = []
    for target in (96, 600, 2000):
        result = native.run(positions, indices, native.Control(), cache=cache,
                            params={"quad": {"target_quads": target}})
        count = check_topology(result)
        assert abs(count-target) <= max(4, target*0.05), (target, count, result["warnings"])
        report = result["quads"]["report"]
        assert report["target_quads"] == target
        assert 1 <= report["target_attempts"] <= 8
        achieved.append(count)
    assert achieved[0] < achieved[1] < achieved[2]


def test_open_surface_target_preserves_boundary():
    positions = array("f", [0,0,0, 2,0,0, 2,1,0, 0,1,0])
    result = native.run(positions, array("I", [0,1,2,0,2,3]), native.Control(),
                        params={"quad": {"target_quads": 600}})
    count = check_topology(result, closed=False)
    assert abs(count-600) <= 30


def test_target_changes_reuse_cache_and_match_full_run():
    positions, indices = torus()
    cache = native.Cache()
    density = native.run(positions, indices, native.Control(), cache=cache)
    for target in (600, 2000):
        params = {"quad": {"target_quads": target}}
        quick = native.run(positions, indices, native.Control(), params=params, cache=cache)
        full = native.run(positions, indices, native.Control(), params=params)
        assert quick["reused"] and not full["reused"]
        assert geometry(quick) == geometry(full)
        assert quick["warnings"] == full["warnings"]
    back = native.run(positions, indices, native.Control(), cache=cache)
    assert back["reused"]
    assert geometry(back) == geometry(density)


@pytest.mark.parametrize("axes", [(True,False,False), (True,True,False), (True,True,True)])
def test_target_is_final_total_after_actual_symmetry(axes):
    positions, indices = cube(center=True)
    result = native.run(positions, indices, native.Control(), params={
        "quad": {"target_quads": 600}, "symmetry": dict(zip("xyz", axes))})
    count = check_topology(result)
    assert result["symmetry"]["axes"] == list(axes)
    assert abs(count-600) <= 30, (axes, count)
    assert count % (2**sum(axes)) == 0


def test_low_impossible_target_is_bounded_and_warned():
    result = native.run(*cube(), native.Control(), params={"quad": {"target_quads": 1}})
    count = check_topology(result)
    assert count >= 6
    assert result["quads"]["report"]["target_attempts"] <= 8
    assert any("target" in w["message"].lower() for w in result["warnings"])


def test_global_target_across_unequal_components():
    positions, indices = cube()
    second, other_indices = cube(scale=2)
    second = array("f", (v + (4 if i%3 == 0 else 0) for i,v in enumerate(second)))
    positions.extend(second)
    indices.extend(v+8 for v in other_indices)
    result = native.run(positions, indices, native.Control(), params={"quad": {"target_quads": 600}})
    count = check_topology(result)
    assert result["components"] == 2
    assert abs(count-600) <= 30
    flat = array("f", result["quads"]["positions"])
    assert any(flat[i] < 2 for i in range(0,len(flat),3))
    assert any(flat[i] > 3 for i in range(0,len(flat),3))


def test_density_disabled_target_is_identical():
    positions, indices = cube()
    default = native.run(positions, indices, native.Control())
    explicit = native.run(positions, indices, native.Control(), params={"quad": {"target_quads": 0}})
    assert geometry(default) == geometry(explicit)
    assert default["warnings"] == explicit["warnings"]
    assert explicit["quads"]["report"]["target_attempts"] == 0


@pytest.mark.parametrize("value", [-1, True, 1.5, "600", 2_000_001, 2**64])
def test_invalid_target_rejected_before_slow_work(value):
    with pytest.raises(native.InvalidInputError, match="target_quads"):
        native.run(*cube(), native.Control(), params={"quad": {"target_quads": value}}, fault="slow:5000")


@pytest.mark.parametrize("section", ["quad", "trace", "field"])
def test_disabled_stage_does_not_run_target_search(section):
    result = native.run(*cube(), native.Control(), params={
        "quad": {"target_quads": 600, **({"enabled": False} if section == "quad" else {})},
        **({section: {"enabled": False}} if section != "quad" else {})})
    assert result["quads"] is None


def test_dense_target_above_input_resolution_remains_valid():
    result = native.run(*cube(), native.Control(), params={"quad": {"target_quads": 100_000}})
    count = check_topology(result)
    assert abs(count-100_000) <= 5000
    assert result["quads"]["report"]["target_attempts"] <= 8


def test_adaptivity_and_frame_keep_global_budget():
    positions, indices = torus()
    cache = native.Cache()
    outputs = []
    for adaptivity in (0.0, 1.0):
        result = native.run(positions, indices, native.Control(), cache=cache,
                            params={"quad": {"target_quads": 2000, "adaptivity": adaptivity}})
        assert abs(check_topology(result)-2000) <= 100
        outputs.append(geometry(result))
    assert outputs[0] != outputs[1], "adaptivity should redistribute the fixed global budget"
    framed = native.run(positions, indices, native.Control(), params={
        "frame": {"enabled": True}, "quad": {"target_quads": 2000}})
    assert abs(check_topology(framed)-2000) <= 100


def test_target_is_invariant_under_uniform_geometric_scale():
    outputs = []
    for scale in (1.0, 10.0):
        result = native.run(*cube(scale=scale), native.Control(), params={"quad": {"target_quads": 600}})
        count = check_topology(result)
        assert abs(count-600) <= 30
        outputs.append(count)
    assert outputs[0] == outputs[1]


def test_cancellation_during_target_planning_is_not_layout_fallback():
    control, failures = native.Control(), []

    def work():
        try:
            native.run(*cube(), control, params={"quad": {"target_quads": 100_000}})
        except Exception as error:
            failures.append(error)

    thread = threading.Thread(target=work)
    thread.start()
    deadline = time.monotonic()+5
    while thread.is_alive() and time.monotonic() < deadline:
        if control.progress()[0] in ("quantize", "patterns", "map"):
            control.cancel()
            break
        time.sleep(0.001)
    thread.join(5)
    assert not thread.is_alive()
    assert len(failures) == 1 and isinstance(failures[0], native.CancelledError), failures


def test_auto_and_skipped_symmetry_use_actual_output_budget():
    detected = native.run(*cube(center=True), native.Control(), params={
        "symmetry": {"auto_detect": True}, "quad": {"target_quads": 601}})
    assert any(detected["symmetry"]["axes"])
    assert abs(check_topology(detected)-601) <= 30
    assert detected["quads"]["report"]["target_quads"] == 601
    positions, indices = cube()
    for i in range(0,len(positions),3):
        positions[i] += 5
    skipped = native.run(positions, indices, native.Control(), params={
        "symmetry": {"x": True, "keep_negative": True}, "quad": {"target_quads": 600}})
    assert skipped["symmetry"]["skipped"]
    assert abs(check_topology(skipped)-600) <= 30
