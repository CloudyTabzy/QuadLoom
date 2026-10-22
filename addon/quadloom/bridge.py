# SPDX-License-Identifier: GPL-3.0-or-later
"""The only module that imports the native backend.

If `quadloom_native` can't be imported (missing wheel, wrong platform, ABI
mismatch), the add-on still registers: `native` is None, `unavailable_reason`
says why, and the exception names below fall back to local stand-ins so
`except bridge.QuadLoomError` keeps working everywhere.
"""

import importlib


class QuadLoomError(Exception):
    """Stand-in base error used when the native module is unavailable."""

    stage = None
    hint = None


class InvalidInputError(QuadLoomError):
    pass


class CancelledError(QuadLoomError):
    pass


class InternalError(QuadLoomError):
    pass


native = None
unavailable_reason = None

# Must equal the backend's build_info()["api"]. Blender can't unload a native
# module, so after an update the old backend stays loaded until a restart.
API_VERSION = 5


def _load(import_module=importlib.import_module):
    """(Re)loads the backend. `import_module` is injectable for tests."""
    global native, unavailable_reason
    global QuadLoomError, InvalidInputError, CancelledError, InternalError
    try:
        module = import_module("quadloom_native")
    except Exception as e:  # ImportError, OSError (DLL load), anything else
        native = None
        unavailable_reason = f"{type(e).__name__}: {e}"
        return
    try:
        api = module.build_info().get("api")
    except Exception:
        api = None
    if api != API_VERSION:
        native = None
        unavailable_reason = (
            "QuadLoom was updated while Blender was running: restart Blender to load the new backend "
            f"(add-on API {API_VERSION}, loaded backend API {api})"
        )
        return
    native = module
    unavailable_reason = None
    QuadLoomError = module.QuadLoomError
    InvalidInputError = module.InvalidInputError
    CancelledError = module.CancelledError
    InternalError = module.InternalError


_load()


def available():
    return native is not None


def version():
    return native.version() if native is not None else None


def build_info():
    return dict(native.build_info()) if native is not None else {}


def new_control():
    if native is None:
        raise QuadLoomError(f"backend unavailable: {unavailable_reason}")
    return native.Control()


def run_job(positions, indices, control, **kwargs):
    """Runs the pipeline; blocks. Call from a worker thread, never from the UI thread."""
    if native is None:
        raise QuadLoomError(f"backend unavailable: {unavailable_reason}")
    return native.run(positions, indices, control, **kwargs)
