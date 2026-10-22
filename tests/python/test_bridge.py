# SPDX-License-Identifier: GPL-3.0-or-later
"""The add-on refuses a backend from another build (Blender keeps an old one
loaded until restart), and the real backend matches."""

import importlib.util
import types
from pathlib import Path

import quadloom_native

BRIDGE = Path(__file__).resolve().parents[2] / "addon" / "quadloom" / "bridge.py"


def load_bridge():
    spec = importlib.util.spec_from_file_location("quadloom_bridge_under_test", BRIDGE)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def fake_backend(info):
    module = types.ModuleType("quadloom_native")
    module.build_info = lambda: info
    for name in ("QuadLoomError", "InvalidInputError", "CancelledError", "InternalError"):
        setattr(module, name, type(name, (Exception,), {}))
    return module


def test_real_backend_matches_the_addon():
    bridge = load_bridge()
    assert bridge.available(), bridge.unavailable_reason
    assert quadloom_native.build_info()["api"] == bridge.API_VERSION


def test_stale_backend_asks_for_a_restart():
    bridge = load_bridge()
    bridge._load(lambda name: fake_backend({"version": "0.1.0"}))  # no "api": an older build
    assert not bridge.available()
    assert "restart Blender" in bridge.unavailable_reason


def test_other_api_version_is_refused():
    bridge = load_bridge()
    bridge._load(lambda name: fake_backend({"version": "0.1.0", "api": bridge.API_VERSION + 1}))
    assert not bridge.available()
    assert "restart Blender" in bridge.unavailable_reason
