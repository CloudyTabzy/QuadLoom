# SPDX-License-Identifier: GPL-3.0-or-later
"""Quick Re-run: the Cache, and the new quad settings (regularity, solver effort)."""

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


def quads(result):
    q = result["quads"]
    return q["positions"], q["face_offsets"], q["face_indices"]


def test_cache_reuses_the_layout_and_matches_a_full_run():
    positions, indices = torus()
    cache = ql.Cache()
    first = ql.run(positions, indices, ql.Control(), cache=cache)
    assert first["reused"] is False and cache.filled()
    for quad in ({"edge_scale": 0.5}, {"regularity": 100.0, "smoothing": 3}, {"solver_effort": 0.25}):
        quick = ql.run(positions, indices, ql.Control(), params={"quad": quad}, cache=cache)
        full = ql.run(positions, indices, ql.Control(), params={"quad": quad})
        assert quick["reused"] is True, quad
        assert full["reused"] is False
        assert quads(quick) == quads(full), quad
        assert quick["warnings"] == full["warnings"]


def test_cache_runs_everything_when_more_than_quads_change():
    positions, indices = torus()
    cache = ql.Cache()
    ql.run(positions, indices, ql.Control(), cache=cache)
    angle = ql.run(positions, indices, ql.Control(), params={"features": {"angle_deg": 20.0}}, cache=cache)
    assert angle["reused"] is False, "a feature setting changed"
    other_positions, other_indices = torus(rings=28)
    other = ql.run(other_positions, other_indices, ql.Control(), params={"features": {"angle_deg": 20.0}}, cache=cache)
    assert other["reused"] is False, "a different mesh"
    again = ql.run(other_positions, other_indices, ql.Control(), params={"features": {"angle_deg": 20.0}}, cache=cache)
    assert again["reused"] is True
    cache.clear()
    assert not cache.filled()
    cleared = ql.run(other_positions, other_indices, ql.Control(), params={"features": {"angle_deg": 20.0}}, cache=cache)
    assert cleared["reused"] is False


def test_quad_settings_are_validated():
    positions, indices = torus()
    with pytest.raises(ql.InvalidInputError, match="solver_effort"):
        ql.run(positions, indices, ql.Control(), params={"quad": {"solver_effort": 0.0}})
    with pytest.raises(ql.InvalidInputError, match="regularity"):
        ql.run(positions, indices, ql.Control(), params={"quad": {"regularity": -1.0}})
