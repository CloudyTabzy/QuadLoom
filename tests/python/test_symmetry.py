# SPDX-License-Identifier: GPL-3.0-or-later
"""The symmetry settings section and its report."""

import math
from array import array

import pytest
import quadloom_native as ql


def torus(rings=32, segments=16, big=1.0, small=0.4):
    positions, indices = array("f"), array("I")
    for i in range(rings):
        u = 2 * math.pi * i / rings
        for j in range(segments):
            v = 2 * math.pi * j / segments
            r = big + small * math.cos(v)
            positions.extend([r * math.cos(u), r * math.sin(u), small * math.sin(v)])
    vid = lambda i, j: (i % rings) * segments + (j % segments)
    for i in range(rings):
        for j in range(segments):
            a, b, c, d = vid(i, j), vid(i + 1, j), vid(i + 1, j + 1), vid(i, j + 1)
            indices.extend([a, b, c, a, c, d])
    return positions, indices


def test_symmetry_x_is_reported_and_mirrored():
    positions, indices = torus()
    result = ql.run(positions, indices, ql.Control(), params={"symmetry": {"x": True}})
    sym = result["symmetry"]
    assert sym["axes"] == [True, False, False]
    assert sym["seam_edges"] > 0 and sym["skipped"] is None
    quads = array("f")
    quads.frombytes(result["quads"]["positions"])
    xs = sorted(round(quads[i], 5) for i in range(0, len(quads), 3))
    assert xs == sorted(-x for x in xs)


def test_auto_detect_finds_planes():
    positions, indices = torus()
    result = ql.run(positions, indices, ql.Control(), params={"symmetry": {"auto_detect": True}})
    assert result["symmetry"]["axes"] == [True, True, True]


def test_symmetry_off_reports_nothing():
    positions, indices = torus()
    assert ql.run(positions, indices, ql.Control())["symmetry"] is None


def test_unknown_symmetry_key_rejected():
    positions, indices = torus()
    with pytest.raises(ql.InvalidInputError, match="params.symmetry.w"):
        ql.run(positions, indices, ql.Control(), params={"symmetry": {"w": True}})
