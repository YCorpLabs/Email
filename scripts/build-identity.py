#!/usr/bin/env python3
"""Fingerprint public app sources, independent of host, Git dirtiness and secrets."""
import hashlib
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SOURCE_DIRS = ("crates", "apps", "assets", "scripts")
SUFFIXES = {".rs", ".swift", ".toml", ".svg", ".png", ".json", ".sh", ".py"}
SKIP_DIRS = {"target", ".build", ".swiftpm-cache", ".clang-cache", "__pycache__", "node_modules", ".secrets", ".wrangler"}


def build_id():
    digest = hashlib.sha256()
    sources = []
    try:
        names = subprocess.check_output(
            ["git", "-C", str(ROOT), "ls-files", "--cached", "--others", "--exclude-standard", "-z", "--", *SOURCE_DIRS],
            stderr=subprocess.DEVNULL,
        ).decode().split("\0")
        candidates = (ROOT / name for name in names if name)
    except (OSError, subprocess.CalledProcessError):
        # Source archives may not include Git metadata.
        candidates = (path for directory in SOURCE_DIRS for path in (ROOT / directory).rglob("*"))
    for path in candidates:
        relative = path.relative_to(ROOT)
        if (not path.is_file() or path.suffix not in SUFFIXES
                or any(part in SKIP_DIRS for part in relative.parts)
                or path.name.endswith(".generated.swift")):
            continue
        sources.append((relative.as_posix(), path))
    for name, path in sorted(sources):
        data = path.read_bytes()
        digest.update(name.encode() + b"\0" + str(len(data)).encode() + b"\0" + data)
    return "dev-" + digest.hexdigest()[:12]


if __name__ == "__main__":
    print(build_id())
