"""Bump the QuadLoom version everywhere it lives (owner's rule: every build
handed out gets a new version).

    python tools/bump_version.py            0.1.1 -> 0.1.2 (patch)
    python tools/bump_version.py minor      0.1.1 -> 0.2.0
    python tools/bump_version.py 0.3.0      set explicitly

A new version also renames the wheel, which is what makes Blender install the
new backend: it keeps an already-installed wheel of the same name and version.
"""

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CARGO = ROOT / "Cargo.toml"
MANIFEST = ROOT / "addon" / "quadloom" / "blender_manifest.toml"


def current() -> str:
    match = re.search(r'^version = "(\d+\.\d+\.\d+)"', CARGO.read_text(encoding="utf-8"), re.M)
    if not match:
        raise SystemExit("no version in Cargo.toml")
    return match.group(1)


def bumped(version: str, how: str) -> str:
    if re.fullmatch(r"\d+\.\d+\.\d+", how):
        return how
    major, minor, patch = map(int, version.split("."))
    if how == "major":
        return f"{major + 1}.0.0"
    if how == "minor":
        return f"{major}.{minor + 1}.0"
    if how == "patch":
        return f"{major}.{minor}.{patch + 1}"
    raise SystemExit(f"unknown bump {how!r} (patch, minor, major, or an explicit x.y.z)")


def replace(path: Path, pattern: str, replacement: str) -> None:
    text = path.read_text(encoding="utf-8")
    new, count = re.subn(pattern, replacement, text, count=1, flags=re.M)
    if count != 1:
        raise SystemExit(f"{path.name}: pattern not found: {pattern}")
    path.write_text(new, encoding="utf-8", newline="\n")


def main() -> None:
    old = current()
    new = bumped(old, sys.argv[1] if len(sys.argv) > 1 else "patch")
    replace(CARGO, r'^version = "\d+\.\d+\.\d+"', f'version = "{new}"')
    replace(MANIFEST, r'^version = "\d+\.\d+\.\d+"', f'version = "{new}"')
    replace(MANIFEST, r"quadloom_native-\d+\.\d+\.\d+-", f"quadloom_native-{new}-")
    print(f"{old} -> {new}")


if __name__ == "__main__":
    main()
