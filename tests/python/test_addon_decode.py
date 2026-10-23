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
