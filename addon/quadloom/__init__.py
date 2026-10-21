# SPDX-License-Identifier: GPL-3.0-or-later
"""QuadLoom: feature-aligned quad remeshing for Blender.

Registration never depends on the native backend: if it fails to load, the
panel says why instead of the add-on failing to enable.
"""

import bpy

from . import bridge, operators, props, ui

_classes = (*props.classes, *operators.classes, *ui.classes)


def register():
    for cls in _classes:
        bpy.utils.register_class(cls)
    props.register_props()


def unregister():
    # Stop any running job before its operator and properties disappear.
    operators.cancel_active_job(join_timeout=5.0)
    props.unregister_props()
    for cls in reversed(_classes):
        bpy.utils.unregister_class(cls)
