"""Download the QuadLoom benchmark meshes listed in sources.json.

Usage:
    python benchmark/fetch_meshes.py            # fetch everything missing
    python benchmark/fetch_meshes.py --force    # re-download everything

Files land in benchmark/meshes/<source>/<model>/ (gitignored). A
MANIFEST.json next to them records each file's origin URL, license,
tags, size and SHA-256 so runs are reproducible. Standard library only.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
import time
import urllib.request
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent
SOURCES = ROOT / "sources.json"
MESHES = ROOT / "meshes"
USER_AGENT = "QuadLoom-benchmark-fetch/1.0"


def http_get(url: str, retries: int = 3) -> bytes:
    last_error: Exception | None = None
    for attempt in range(retries):
        try:
            req = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
            with urllib.request.urlopen(req, timeout=120) as resp:
                return resp.read()
        except Exception as e:  # network errors are retried, then reported
            last_error = e
            time.sleep(2 * (attempt + 1))
    raise RuntimeError(f"GET {url} failed: {last_error}")


def save(path: Path, data: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)


def safe_extract(archive: Path, dest: Path) -> list[Path]:
    """Extract a zip, refusing entries that would escape `dest`."""
    out = []
    dest = dest.resolve()
    with zipfile.ZipFile(archive) as zf:
        for info in zf.infolist():
            target = (dest / info.filename).resolve()
            if dest not in target.parents and target != dest:
                raise RuntimeError(f"unsafe path in {archive.name}: {info.filename}")
            if info.is_dir() or "__MACOSX" in info.filename:
                continue
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(zf.read(info))
            out.append(target)
    return out


def record(entries: list, source: str, model: str, path: Path, url: str, license_: str, tags: list) -> None:
    data = path.read_bytes()
    entries.append({
        "source": source,
        "model": model,
        "file": path.relative_to(MESHES).as_posix(),
        "url": url,
        "license": license_,
        "tags": tags,
        "bytes": len(data),
        "sha256": hashlib.sha256(data).hexdigest(),
    })


def khronos_license(meta: dict) -> str:
    legal = meta.get("legal") or []
    parts = sorted({l.get("spdx") or l.get("license") or "unknown" for l in legal})
    return " / ".join(parts) if parts else "unknown"


def fetch_khronos(cfg: dict, force: bool, entries: list, failures: list) -> None:
    base, api = cfg["base_url"], cfg["api_url"]
    jobs = [(m, "binary") for m in cfg["binary"]] + [(m, "folder") for m in cfg["gltf_folder"]]
    for m, kind in jobs:
        name = m["name"]
        folder = MESHES / "khronos" / name
        try:
            meta_path = folder / "metadata.json"
            if force or not meta_path.exists():
                save(meta_path, http_get(f"{base}/{name}/metadata.json"))
            license_ = khronos_license(json.loads(meta_path.read_text(encoding="utf-8")))

            if kind == "binary":
                url = f"{base}/{name}/glTF-Binary/{name}.glb"
                target = folder / f"{name}.glb"
                if force or not target.exists():
                    data = http_get(url)
                    if data[:4] != b"glTF":
                        raise RuntimeError("downloaded file is not a GLB (git-lfs pointer?)")
                    save(target, data)
                record(entries, "khronos", name, target, url, license_, m["tags"])
            else:
                listing = json.loads(http_get(f"{api}/{name}/glTF"))
                for item in listing:
                    if item["type"] != "file":
                        continue
                    target = folder / "glTF" / item["name"]
                    if force or not target.exists():
                        save(target, http_get(item["download_url"]))
                    if target.suffix in (".gltf", ".bin"):
                        record(entries, "khronos", name, target, item["download_url"], license_, m["tags"])
            print(f"  ok   khronos/{name}")
        except Exception as e:
            failures.append(f"khronos/{name}: {e}")
            print(f"  FAIL khronos/{name}: {e}")


def fetch_crane(cfg: dict, force: bool, entries: list, failures: list) -> None:
    for m in cfg["models"]:
        name = m["name"]
        folder = MESHES / "crane" / name
        url = f"{cfg['base_url']}/{name}.zip"
        try:
            archive = folder / f"{name}.zip"
            if force or not archive.exists():
                save(archive, http_get(url))
            for path in safe_extract(archive, folder):
                if path.suffix.lower() in (".obj", ".ply", ".off", ".stl"):
                    record(entries, "crane", name, path, url, cfg["license"], m["tags"])
            print(f"  ok   crane/{name}")
        except Exception as e:
            failures.append(f"crane/{name}: {e}")
            print(f"  FAIL crane/{name}: {e}")


def fetch_common3d(cfg: dict, force: bool, entries: list, failures: list) -> None:
    for m in cfg["models"]:
        name = m["name"]
        url = f"{cfg['base_url']}/{name}.obj"
        target = MESHES / "common3d" / name / f"{name}.obj"
        try:
            if force or not target.exists():
                save(target, http_get(url))
            record(entries, "common3d", name, target, url, cfg["license"], m["tags"])
            print(f"  ok   common3d/{name}")
        except Exception as e:
            failures.append(f"common3d/{name}: {e}")
            print(f"  FAIL common3d/{name}: {e}")


def fetch_thingi(cfg: dict, force: bool, entries: list, failures: list) -> None:
    for m in cfg["models"]:
        fid = m["file_id"]
        url = f"{cfg['base_url']}/{fid}.stl"
        target = MESHES / "thingi10k" / fid / f"{fid}.stl"
        try:
            if force or not target.exists():
                save(target, http_get(url))
            tags = ["thingi10k", m["category"]]
            license_ = f"{m['license']} (thing {m['thing_id']} by {m['author']})"
            record(entries, "thingi10k", f"{fid} {m['name']}", target, url, license_, tags)
            print(f"  ok   thingi10k/{fid} ({m['name']})")
        except Exception as e:
            failures.append(f"thingi10k/{fid}: {e}")
            print(f"  FAIL thingi10k/{fid}: {e}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--force", action="store_true", help="re-download existing files")
    args = parser.parse_args()

    cfg = json.loads(SOURCES.read_text(encoding="utf-8"))
    entries: list = []
    failures: list = []
    for label, fn in (("khronos", fetch_khronos), ("crane", fetch_crane),
                      ("common3d", fetch_common3d), ("thingi10k", fetch_thingi)):
        print(f"[{label}]")
        fn(cfg[label], args.force, entries, failures)

    manifest = {"generated": time.strftime("%Y-%m-%dT%H:%M:%S"), "files": entries, "failures": failures}
    save(MESHES / "MANIFEST.json", json.dumps(manifest, indent=2).encode("utf-8"))
    total = sum(e["bytes"] for e in entries)
    print(f"\n{len(entries)} files, {total / 1e6:.1f} MB, {len(failures)} failures")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
