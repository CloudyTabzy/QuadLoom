# SPDX-License-Identifier: GPL-3.0-or-later
"""Attribute-transfer helpers that need no Blender (transfer.py's NumPy parts)."""

import importlib.util
from pathlib import Path

import pytest

np = pytest.importorskip("numpy")  # Blender ships NumPy; the dev venv may not

TRANSFER = Path(__file__).resolve().parents[2] / "addon" / "quadloom" / "transfer.py"


@pytest.fixture(scope="module")
def transfer():
    spec = importlib.util.spec_from_file_location("quadloom_transfer_under_test", TRANSFER)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def faces(*polygons):
    """(vertex per corner, start, total) for polygons given as vertex lists."""
    vertex = np.array([v for poly in polygons for v in poly], dtype=np.int64)
    total = np.array([len(p) for p in polygons], dtype=np.int64)
    start = np.concatenate([[0], np.cumsum(total)[:-1]]).astype(np.int64)
    return vertex, start, total


def test_a_seam_between_two_faces(transfer):
    # Two quads side by side; the right one's values jump on the shared edge.
    vertex, start, total = faces([0, 1, 4, 3], [1, 2, 5, 4])
    values = np.array([[0, 0], [1, 0], [1, 1], [0, 1], [5, 0], [6, 0], [6, 1], [5, 1]], dtype=np.float32)
    assert transfer._jumps(vertex, values, start, total).tolist() == [[1, 4]]


def test_continuous_values_have_no_seam(transfer):
    vertex, start, total = faces([0, 1, 4, 3], [1, 2, 5, 4])
    values = np.array([[0, 0], [1, 0], [1, 1], [0, 1], [1, 0], [2, 0], [2, 1], [1, 1]], dtype=np.float32)
    assert len(transfer._jumps(vertex, values, start, total)) == 0


def test_a_cut_that_stops_inside_an_island_keeps_its_last_edge(transfer):
    # 2 x 2 quads (vertices 0-8); cut between the top two along edge 4-7. The
    # cut's tip (4) has one value; vertex 7 has two.
    vertex, start, total = faces([0, 1, 4, 3], [1, 2, 5, 4], [3, 4, 7, 6], [4, 5, 8, 7])
    grid = {v: (v % 3, v // 3) for v in range(9)}
    values = np.array([grid[v] for v in vertex], dtype=np.float32)
    values[15] += np.array([10.0, 0.0], dtype=np.float32)  # face 3 at vertex 7
    seams = transfer._jumps(vertex, values, start, total).tolist()
    assert [4, 7] in seams
    assert len(seams) == 1


def test_a_pole_fan_is_not_a_seam(transfer):
    # Six triangles around a pole (0); each maps the pole to its own value.
    polygons = [[0, i, i % 6 + 1] for i in range(1, 7)]
    vertex, start, total = faces(*polygons)
    values = []
    for n, poly in enumerate(polygons):
        for v in poly:
            values.append((n / 6.0, 1.0) if v == 0 else (v / 6.0, 0.0))
    values = np.array(values, dtype=np.float32)
    assert len(transfer._jumps(vertex, values, start, total)) == 0


def test_unique_rows_fold_negative_zero(transfer):
    rows = np.array([[0.0, 1.0], [-0.0, 1.0], [0.5, 1.0]])
    keys = transfer._unique_rows(rows)
    assert keys[0] == keys[1] != keys[2]
    assert keys.dtype == np.uint32
