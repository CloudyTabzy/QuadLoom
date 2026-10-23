# SPDX-License-Identifier: GPL-3.0-or-later
"""Bounded JSON trees plus raw buffers. No executable deserialization."""

from array import array
import json
import os
from pathlib import Path

MAX_MANIFEST = 1024 * 1024
MAX_DEPTH = 32


def publish(path, value):
    raw = json.dumps(value, allow_nan=False, separators=(",", ":")).encode("utf-8")
    if len(raw) > MAX_MANIFEST:
        raise ValueError("worker manifest exceeds its size limit")
    path = Path(path)
    temporary = path.with_suffix(".tmp")
    temporary.write_bytes(raw)
    os.replace(temporary, path)


def read_json(path):
    with Path(path).open("rb") as stream:
        raw = stream.read(MAX_MANIFEST + 1)
    if len(raw) > MAX_MANIFEST:
        raise ValueError("worker manifest exceeds its size limit")
    return json.loads(raw)


def write(directory, value, budget):
    directory = Path(directory)
    directory.mkdir(parents=True, exist_ok=True)
    used, number = 0, 0

    def encode(value, depth=0):
        nonlocal used, number
        if depth > MAX_DEPTH:
            raise ValueError("worker manifest is nested too deeply")
        if isinstance(value, (bytes, array)):
            view = memoryview(value).cast("B")
            used += len(view)
            if used > budget:
                raise ValueError("worker buffers exceed the memory budget")
            index = number
            number += 1
            with (directory / f"{index}.bin").open("wb") as stream:
                stream.write(view)
            return {"$buffer": index, "size": len(view),
                    "type": value.typecode if isinstance(value, array) else "bytes"}
        if isinstance(value, dict):
            if "$buffer" in value or any(not isinstance(k, str) for k in value):
                raise ValueError("invalid worker metadata key")
            return {k: encode(v, depth + 1) for k, v in value.items()}
        if isinstance(value, (list, tuple)):
            return [encode(v, depth + 1) for v in value]
        if value is None or isinstance(value, (str, bool, int, float)):
            return value
        raise ValueError(f"unsupported worker value: {type(value).__name__}")

    publish(directory / "manifest.json", encode(value))


def read(directory, budget, admit=None):
    directory = Path(directory)
    tree = read_json(directory / "manifest.json")
    used = 0

    def validate(value, depth=0):
        nonlocal used
        if depth > MAX_DEPTH:
            raise ValueError("worker manifest is nested too deeply")
        if isinstance(value, dict):
            if "$buffer" in value:
                number, size, kind = value.get("$buffer"), value.get("size"), value.get("type")
                if (type(number) is not int or number < 0 or type(size) is not int or size < 0
                        or kind not in ("bytes", "f", "i", "I")):
                    raise ValueError("invalid worker buffer descriptor")
                used += size
                if used > budget:
                    raise ValueError("worker buffers exceed the memory budget")
                if (directory / f"{number}.bin").stat().st_size != size:
                    raise ValueError("worker buffer size mismatch (truncated output)")
                if kind != "bytes" and size % 4:
                    raise ValueError("worker array size is misaligned")
            else:
                for item in value.values():
                    validate(item, depth + 1)
        elif isinstance(value, list):
            for item in value:
                validate(item, depth + 1)

    # Validate the entire tree, including cumulative decoded sizes, before
    # allocating even the first binary buffer. Reused IDs count every time.
    validate(tree)
    if admit is not None:
        admit(used)

    def decode(value):
        if isinstance(value, dict):
            if "$buffer" in value:
                size, kind = value["size"], value["type"]
                with (directory / f"{value['$buffer']}.bin").open("rb") as stream:
                    # Recheck after opening in case a file changed since validation.
                    if os.fstat(stream.fileno()).st_size != size:
                        raise ValueError("worker buffer size mismatch (truncated output)")
                    if kind == "bytes":
                        data = stream.read(size)
                        if len(data) != size:
                            raise ValueError("truncated worker buffer")
                        return data
                    data = array(kind)
                    data.fromfile(stream, size // data.itemsize)
                    return data
            return {k: decode(v) for k, v in value.items()}
        if isinstance(value, list):
            return [decode(v) for v in value]
        return value

    return decode(tree)
