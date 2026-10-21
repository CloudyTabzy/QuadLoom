"""Build the QuadLoom native wheel and the Blender extension zip.

Usage:
    python tools/build.py           release wheel + extension zip in dist/
    python tools/build.py --dev     fault-injection wheel + zip in dist/dev/ (for tests only),
                                    also installed into .venv for pytest
    python tools/build.py --no-zip  stop after copying the wheel into the add-on

Standard library only. Exits non-zero, with the failing step's output, on error.
"""

from __future__ import annotations

import argparse
import os
import re
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
VENV = ROOT / ".venv"
VENV_PY = VENV / "Scripts" / "python.exe" if os.name == "nt" else VENV / "bin" / "python"
CRATE = ROOT / "crates" / "quadloom-py" / "Cargo.toml"
WHEEL_OUT = ROOT / "target" / "wheels"
ADDON = ROOT / "addon" / "quadloom"
ADDON_WHEELS = ADDON / "wheels"
MANIFEST = ADDON / "blender_manifest.toml"
DEFAULT_BLENDER = Path(r"C:\Program Files\Blender Foundation\Blender 5.2\blender.exe")


class BuildError(RuntimeError):
    pass


def child_env() -> dict[str, str]:
    env = dict(os.environ)
    # A stray global CMAKE variable on the dev machine points at another
    # project's script; nothing here needs CMake, so keep it out of children.
    env.pop("CMAKE", None)
    env["VIRTUAL_ENV"] = str(VENV)
    return env


def step(name: str, cmd: list[str | Path]) -> str:
    print(f"==> {name}")
    proc = subprocess.run([str(c) for c in cmd], cwd=ROOT, env=child_env(),
                          capture_output=True, text=True, encoding="utf-8", errors="replace")
    if proc.returncode != 0:
        raise BuildError(f"{name} failed (exit {proc.returncode})\n{proc.stdout}\n{proc.stderr}")
    return proc.stdout + proc.stderr


def find_blender() -> Path:
    candidate = Path(os.environ.get("QUADLOOM_BLENDER", DEFAULT_BLENDER))
    if not candidate.is_file():
        raise BuildError(f"Blender not found at {candidate}; set QUADLOOM_BLENDER")
    return candidate


def ensure_venv() -> None:
    if not VENV_PY.is_file():
        step("create .venv", [sys.executable, "-m", "venv", VENV])
    step("install build tools", [VENV_PY, "-m", "pip", "install", "-q", "maturin==1.14.*", "pytest"])


def build_wheel(dev: bool) -> Path:
    WHEEL_OUT.mkdir(parents=True, exist_ok=True)
    for old in WHEEL_OUT.glob("quadloom_native-*.whl"):
        old.unlink()
    cmd: list[str | Path] = [VENV_PY, "-m", "maturin", "build", "--release", "-m", CRATE, "-o", WHEEL_OUT]
    if dev:
        cmd += ["--features", "fault-injection"]
    step("build wheel" + (" (fault-injection)" if dev else ""), cmd)
    wheels = sorted(WHEEL_OUT.glob("quadloom_native-*-cp311-abi3-*.whl"), key=lambda p: p.stat().st_mtime)
    if not wheels:
        raise BuildError(f"no abi3 wheel produced in {WHEEL_OUT}")
    return wheels[-1]


def install_into_addon(wheel: Path) -> None:
    ADDON_WHEELS.mkdir(parents=True, exist_ok=True)
    for old in ADDON_WHEELS.glob("*.whl"):
        old.unlink()
    shutil.copy2(wheel, ADDON_WHEELS / wheel.name)
    print(f"==> copied {wheel.name} into {ADDON_WHEELS.relative_to(ROOT)}")
    if MANIFEST.is_file():
        text = MANIFEST.read_text(encoding="utf-8")
        line = f'wheels = ["./wheels/{wheel.name}"]'
        new, count = re.subn(r"(?m)^wheels\s*=.*$", line, text)
        if count != 1:
            raise BuildError(f"{MANIFEST.name}: expected exactly one 'wheels = ...' line, found {count}")
        MANIFEST.write_text(new, encoding="utf-8")
        print(f"==> manifest wheels -> {wheel.name}")


def build_zip(blender: Path, dev: bool) -> Path:
    out_dir = ROOT / "dist" / ("dev" if dev else "")
    out_dir.mkdir(parents=True, exist_ok=True)
    for old in out_dir.glob("quadloom-*.zip"):
        old.unlink()
    step("build extension zip", [blender, "--factory-startup", "-c", "extension", "build",
                                 "--source-dir", ADDON, "--output-dir", out_dir])
    zips = list(out_dir.glob("quadloom-*.zip"))
    if len(zips) != 1:
        raise BuildError(f"expected one extension zip in {out_dir}, found {len(zips)}")
    output = step("validate extension zip", [blender, "--factory-startup", "-c", "extension", "validate", zips[0]])
    if "error" in output.lower():
        raise BuildError(f"extension validate reported problems:\n{output}")
    return zips[0]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--dev", action="store_true", help="fault-injection build for tests")
    parser.add_argument("--no-zip", action="store_true", help="skip the Blender extension zip")
    args = parser.parse_args()
    try:
        ensure_venv()
        wheel = build_wheel(args.dev)
        if args.dev:
            step("install wheel into .venv", [VENV_PY, "-m", "pip", "install", "-q", "--force-reinstall", wheel])
        install_into_addon(wheel)
        if not args.no_zip:
            zip_path = build_zip(find_blender(), args.dev)
            print(f"\nextension: {zip_path}")
    except BuildError as e:
        print(f"\nBUILD FAILED: {e}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
