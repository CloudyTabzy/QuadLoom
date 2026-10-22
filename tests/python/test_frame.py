"""Tests for the rectangular frame settings and results (stage 3b)."""

import math
import struct
from array import array

import pytest

import quadloom_native as ql


def torus(rings=24, segments=16, major=1.0, minor=0.35):
    """A closed genus-1 test surface as flat float32 positions and uint32 indices."""
    positions = []
    for i in range(rings):
        a = 2.0 * math.pi * i / rings
        for j in range(segments):
            b = 2.0 * math.pi * j / segments
            r = major + minor * math.cos(b)
            positions += [r * math.cos(a), r * math.sin(a), minor * math.sin(b)]
    indices = []
    for i in range(rings):
        for j in range(segments):
            v00 = i * segments + j
            v01 = i * segments + (j + 1) % segments
            v10 = ((i + 1) % rings) * segments + j
            v11 = ((i + 1) % rings) * segments + (j + 1) % segments
            indices += [v00, v10, v11, v00, v11, v01]
    return array("f", positions), array("I", indices)


def face_count(result):
    return len(result["face_offsets"]) // 4 - 1


def test_frame_is_off_by_default():
    p, i = torus()
    assert ql.run(p, i, ql.Control())["frame"] is None


def test_frame_section_accepted():
    p, i = torus()
    frame = {
        "enabled": True,
        "balance": 0.4,
        "curvature_stretch": True,
        "max_stretch": 2.5,
        "stretch_weight": 2.0,
        "align_weight": 0.5,
        "smoothness": 0.05,
        "max_iterations": 80,
        "max_part_faces": 500_000,
    }
    result = ql.run(p, i, ql.Control(), params={"frame": frame})
    report = result["frame"]
    assert report is not None
    assert report["solved_parts"] >= 1
    assert report["curvature_stretch"] is True
    for key in ("parts", "fallback_parts", "skipped_parts", "iterations", "residual",
                "max_rotation_deg", "size_p5", "size_p95", "stretch_p50", "stretch_p95"):
        assert key in report, key


def test_unknown_frame_key_rejected():
    p, i = torus()
    with pytest.raises(ql.InvalidInputError) as e:
        ql.run(p, i, ql.Control(), params={"frame": {"enabled": True, "bogus": 1.0}})
    assert "params.frame.bogus" in str(e.value)


def test_frame_range_checked():
    p, i = torus()
    with pytest.raises(ql.InvalidInputError) as e:
        ql.run(p, i, ql.Control(), params={"frame": {"enabled": True, "max_stretch": 0.5}})
    assert "frame.max_stretch" in str(e.value)


def test_frame_result_arrays_match_faces():
    p, i = torus()
    result = ql.run(p, i, ql.Control(), params={"frame": {"enabled": True}})
    frame = result["frame"]
    faces = face_count(result)
    assert len(frame["size"]) == 4 * faces
    assert len(frame["stretch"]) == 4 * faces
    size = struct.unpack(f"<{faces}f", frame["size"])
    stretch = struct.unpack(f"<{faces}f", frame["stretch"])
    assert all(math.isfinite(x) and x > 0 for x in size)
    assert all(math.isfinite(x) and x >= 1.0 for x in stretch)


def test_trace_source_face_present():
    p, i = torus()
    result = ql.run(p, i, ql.Control(), params={"frame": {"enabled": True}})
    trace = result["trace"]
    faces = len(trace["triangles"]) // 12
    source = struct.unpack(f"<{faces}I", trace["source_face"])
    assert all(s < face_count(result) for s in source)
    assert set(source) <= set(range(face_count(result)))
