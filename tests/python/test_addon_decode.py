# SPDX-License-Identifier: GPL-3.0-or-later
"""Add-on result decoding that needs no Blender: mesh_io is loaded with stub bpy modules."""

import importlib.util
import struct
import sys
import types
from pathlib import Path

import pytest

MESH_IO = Path(__file__).resolve().parents[2] / "addon" / "quadloom" / "mesh_io.py"


@pytest.fixture
def mesh_io(monkeypatch):
    bpy = types.ModuleType("bpy")
    mathutils = types.ModuleType("mathutils")
    mathutils.Matrix = object
    monkeypatch.setitem(sys.modules, "bpy", bpy)
    monkeypatch.setitem(sys.modules, "mathutils", mathutils)
    # A stand-in package, so mesh_io's relative imports find their siblings.
    package = types.ModuleType("quadloom_under_test")
    package.__path__ = [str(MESH_IO.parent)]
    monkeypatch.setitem(sys.modules, "quadloom_under_test", package)
    spec = importlib.util.spec_from_file_location("quadloom_under_test.mesh_io", MESH_IO)
    module = importlib.util.module_from_spec(spec)
    monkeypatch.setitem(sys.modules, "quadloom_under_test.mesh_io", module)
    spec.loader.exec_module(module)
    return module


def floats(*values):
    return struct.pack(f"<{len(values)}f", *values)


def uints(*values):
    return struct.pack(f"<{len(values)}I", *values)


def triangle(value=0.0):
    return {"positions": floats(value, 0, 0, 1, 0, 0, 0, 1, 0),
            "face_offsets": uints(0, 3), "face_indices": uints(0, 1, 2)}


@pytest.mark.parametrize("value", [float("nan"), float("inf"), -float("inf")])
def test_nonfinite_result_and_layout_never_reach_blender(mesh_io, value):
    result = triangle(value)
    with pytest.raises(ValueError, match="finite"):
        mesh_io.decode_result(result)
    trace = {"positions": result["positions"], "triangles": result["face_indices"], "patch_of_face": uints(0)}
    with pytest.raises(ValueError, match="finite"):
        mesh_io.decode_layout(trace)


def test_blender_signed_index_limit_checked_without_allocating(mesh_io):
    with pytest.raises(ValueError, match="32-bit"):
        mesh_io.check_blender_counts(3, 2**31, 1)


@pytest.mark.parametrize("offsets", [(0, 4, 3), (0, 0, 3), (1, 3), (0, 2)])
def test_bad_face_offsets_rejected(mesh_io, offsets):
    result = triangle()
    result["face_offsets"] = uints(*offsets)
    with pytest.raises(ValueError):
        mesh_io.decode_result(result)


def test_frame_patch_id_cannot_force_a_huge_bincount(mesh_io, monkeypatch):
    import numpy as np

    def forbidden(*args, **kwargs):
        pytest.fail("patch ids must be rejected before allocating bincount")

    monkeypatch.setattr(np, "bincount", forbidden)
    with pytest.raises(ValueError, match="patch"):
        mesh_io.frame_per_quad({"size": floats(1), "stretch": floats(1)}, 1,
                               {"patch_of_face": uints(2**31)}, [0])


def test_frame_maps_onto_plain_faces(mesh_io):
    frame = {"size": floats(1.0, 2.0), "stretch": floats(1.5, 3.0)}
    size, stretch = mesh_io.decode_frame(frame, 2)
    assert list(size) == [1.0, 2.0]
    assert list(stretch) == [1.5, 3.0]


def test_frame_maps_through_source_faces(mesh_io):
    frame = {"size": floats(1.0, 2.0), "stretch": floats(1.5, 3.0)}
    size, stretch = mesh_io.decode_frame(frame, 2, source_face=uints(1, 1, 0))
    assert list(size) == [2.0, 2.0, 1.0]
    assert list(stretch) == [3.0, 3.0, 1.5]


def test_short_frame_array_rejected(mesh_io):
    frame = {"size": floats(1.0), "stretch": floats(1.5, 3.0)}
    with pytest.raises(ValueError):
        mesh_io.decode_frame(frame, 2)


def test_misaligned_frame_bytes_rejected(mesh_io):
    frame = {"size": floats(1.0, 2.0) + b"\x00", "stretch": floats(1.5, 3.0)}
    with pytest.raises(ValueError):
        mesh_io.decode_frame(frame, 2)


def test_out_of_range_source_face_rejected(mesh_io):
    frame = {"size": floats(1.0, 2.0), "stretch": floats(1.5, 3.0)}
    with pytest.raises(ValueError):
        mesh_io.decode_frame(frame, 2, source_face=uints(0, 2))


def test_source_face_count_must_match_output(mesh_io):
    frame = {"size": floats(1.0, 2.0), "stretch": floats(1.5, 3.0)}
    with pytest.raises(ValueError):
        mesh_io.decode_frame(frame, 2, source_face=uints(0, 1), output_faces=3)
