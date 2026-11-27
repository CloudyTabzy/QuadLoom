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


def quad_count(result):
    return result["quads"]["report"]["quads"]


def test_density_is_the_same_with_and_without_symmetry():
    # One side is remeshed at the whole model's scale: its triangles times its copies
    # are the whole's triangles, at the same edge length. Taken for all there is, a
    # half got the whole's triangles on half the area, and a quarter four times too many.
    positions, indices = torus(64, 32)
    one = {"attempts": 1}
    whole = ql.run(positions, indices, ql.Control(), params={"quad": one})
    faces, edge = whole["remesh"]["output_faces"], whole["remesh"]["target_edge_length"]
    for symmetry, copies in (({"x": True}, 2), ({"x": True, "y": True}, 4), ({"auto_detect": True}, 8)):
        side = ql.run(positions, indices, ql.Control(), params={"symmetry": symmetry, "quad": one})
        assert side["remesh"]["target_edge_length"] == pytest.approx(edge, rel=1e-6), symmetry
        assert side["remesh"]["output_faces"] * copies == pytest.approx(faces, rel=0.05), symmetry
        # The quads follow, as nearly as whole counts along a few long patch sides allow.
        assert 0.6 * quad_count(whole) < quad_count(side) < 1.6 * quad_count(whole), symmetry


def test_advise_finds_mirror_planes_and_suggests_auto_detect():
    positions, indices = torus()
    advice = ql.advise(positions, indices, ql.Control())
    assert advice["mirror_axes"] == [True, True, True] and advice["symmetry"] is True
    assert advice["mirror_offsets"] == [0.0, 0.0, 0.0]
    assert all(0.0 <= a < 1e-5 for a in advice["mirror_asymmetry"] + advice["mirror_worst"])
    # Stretched on one side of each plane by another amount: no plane, nothing suggested,
    # and the numbers say how far from symmetric it is.
    lopsided = array("f", positions)
    for i in range(0, len(lopsided), 3):
        for k, factor in enumerate((1.5, 1.3, 1.8)):
            if lopsided[i + k] > 0:
                lopsided[i + k] *= factor
    advice = ql.advise(lopsided, indices, ql.Control())
    assert advice["mirror_axes"] == [False, False, False] and advice["symmetry"] is False
    assert all(a > 0.005 for a in advice["mirror_asymmetry"])
    assert all(w >= a for a, w in zip(advice["mirror_asymmetry"], advice["mirror_worst"]))


def test_auto_detect_leaves_a_one_sided_detail_alone():
    # A bump on the outer rim at +X, three vertices: the torus is still symmetric
    # across X on the whole, and mirroring it would double the bump or lose it.
    positions, indices = torus(48, 24)
    bumped, moved = array("f", positions), 0
    for i in range(0, len(bumped), 3):
        if math.dist(bumped[i:i + 3], (1.4, 0.0, 0.0)) < 0.12:
            bumped[i] += 0.15
            moved += 1
    assert moved == 3
    found = ql.run(bumped, indices, ql.Control(), params={"symmetry": {"auto_detect": True}})["symmetry"]
    assert found["axes"] == [False, True, True]
    assert found["asymmetry"][0] < 0.005 < 0.02 < found["worst"][0]
    assert ql.advise(bumped, indices, ql.Control())["mirror_axes"] == [False, True, True]
    # The planes are found at Symmetry's tolerance, whatever axes are chosen there:
    # at 5% of the size the bump (3.6%) is let through.
    loose = ql.advise(bumped, indices, ql.Control(), params={"symmetry": {"tolerance": 0.05, "z": True}})
    assert loose["mirror_axes"] == [True, True, True] and loose["symmetry"] is True
    # Chosen by hand it is mirrored, with a warning that says what that costs.
    forced = ql.run(bumped, indices, ql.Control(), params={"symmetry": {"x": True}})
    assert forced["symmetry"]["axes"] == [True, False, False]
    assert any("on one side of X only" in w["message"] for w in forced["warnings"])


def test_the_report_carries_the_largest_difference():
    positions, indices = torus()
    report = ql.run(positions, indices, ql.Control(), params={"symmetry": {"x": True}})["symmetry"]
    assert set(report) == {"axes", "offsets", "asymmetry", "worst", "seam_edges", "skipped"}
    assert 0.0 <= report["worst"][0] < 1e-5 and report["worst"][1] == -1.0


def test_symmetry_off_reports_nothing():
    positions, indices = torus()
    assert ql.run(positions, indices, ql.Control())["symmetry"] is None


def test_unknown_symmetry_key_rejected():
    positions, indices = torus()
    with pytest.raises(ql.InvalidInputError, match="params.symmetry.w"):
        ql.run(positions, indices, ql.Control(), params={"symmetry": {"w": True}})


def test_warnings_are_listed_with_stage_and_hint():
    positions, indices = torus()
    lopsided = array("f", positions)
    for i in range(0, len(lopsided), 3):
        if lopsided[i] > 0:
            lopsided[i] *= 1.5
    result = ql.run(lopsided, indices, ql.Control(), params={"symmetry": {"x": True}})
    warnings = result["warnings"]
    assert any("isn't symmetric across X" in w["message"] for w in warnings)
    assert all(set(w) == {"stage", "message", "hint"} for w in warnings)
    clean = ql.run(positions, indices, ql.Control())
    assert clean["warnings"] == []
