# SPDX-License-Identifier: GPL-3.0-or-later
"""Layout Attempts: the setting, what the report says, and that a result is the same on every run."""

import pytest
import quadloom_native as ql

from test_cache import quads, torus
from test_target_count import check_topology


def two_tori():
    """A torus and a smaller one beside it: two parts, each with layouts to choose from."""
    positions, indices = torus()
    second, other = torus(rings=24, segments=12, big=0.6, small=0.25)
    base = len(positions) // 3
    positions.extend(v + (3.0 if i % 3 == 0 else 0.0) for i, v in enumerate(second))
    indices.extend(v + base for v in other)
    return positions, indices


def stages(result):
    return [name for name, _ in result["timings_ms"]]


def test_one_attempt_makes_one_layout():
    result = ql.run(*two_tori(), ql.Control(), params={"quad": {"attempts": 1}})
    report = result["quads"]["report"]
    assert (report["attempts"], report["parts_retried"], report["parts_improved"]) == (1, 0, 0)
    assert "attempts" not in stages(result)


def test_more_attempts_report_what_they_did_and_give_a_whole_mesh():
    positions, indices = two_tori()
    result = ql.run(positions, indices, ql.Control(), params={"quad": {"attempts": 4}})
    report = result["quads"]["report"]
    assert report["attempts"] == 4
    assert 0 <= report["parts_improved"] <= report["parts_retried"] <= 2
    assert "attempts" in stages(result)
    assert check_topology(result) == report["quads"]
    # The attempts run side by side; the result does not depend on which finished first.
    again = ql.run(positions, indices, ql.Control(), params={"quad": {"attempts": 4}})
    assert quads(again) == quads(result)
    assert again["quads"]["patch_of_face"] == result["quads"]["patch_of_face"]


def test_the_default_is_the_add_ons():
    """The panel's Layout Attempts starts at the backend's own default."""
    default = ql.run(*two_tori(), ql.Control())["quads"]["report"]["attempts"]
    assert default == 4


@pytest.mark.parametrize("value", [0, 9, -1, 1.5, True, "3"])
def test_attempts_out_of_range_are_rejected(value):
    with pytest.raises(ql.InvalidInputError, match="attempts"):
        ql.run(*torus(), ql.Control(), params={"quad": {"attempts": value}})


def test_changing_the_attempts_is_a_quick_rerun():
    positions, indices = two_tori()
    cache = ql.Cache()
    first = ql.run(positions, indices, ql.Control(), params={"quad": {"attempts": 1}}, cache=cache)
    assert first["reused"] is False
    quick = ql.run(positions, indices, ql.Control(), params={"quad": {"attempts": 3}}, cache=cache)
    full = ql.run(positions, indices, ql.Control(), params={"quad": {"attempts": 3}})
    assert quick["reused"] is True and full["reused"] is False
    assert quads(quick) == quads(full)
    assert quick["quads"]["report"] == full["quads"]["report"]


def test_a_count_asked_for_holds_with_more_attempts():
    result = ql.run(*two_tori(), ql.Control(), params={"quad": {"target_quads": 1500, "attempts": 3}})
    report = result["quads"]["report"]
    assert report["attempts"] == 3
    assert abs(check_topology(result) - 1500) <= 75
