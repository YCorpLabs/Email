#!/usr/bin/env python3
"""Fingerprint public app sources, independent of host, Git dirtiness and secrets."""
import hashlib
import re
import subprocess
import sys
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


def release_version():
    """The release version (docs/release-versions.md), from apps/linux/Cargo.toml."""
    text = (ROOT / "apps/linux/Cargo.toml").read_text()
    match = re.search(r'(?m)^version = "([0-9A-Za-z.+-]+)"$', text)
    if not match:
        raise SystemExit("apps/linux/Cargo.toml has no release version")
    return match.group(1)


def swift_source():
    """apps/macos/Sources/EmailUI/BuildIdentity.generated.swift (ignored by Git)."""
    return ("import Foundation\n"
            "enum EmailBuildIdentity {\n"
            f'    static let value = "{build_id()}"\n'
            f'    static let releaseVersion = "{release_version()}"\n'
            "}\n")


if __name__ == "__main__":
    if sys.argv[1:] == ["--swift"]:
        sys.stdout.write(swift_source())
    elif sys.argv[1:]:
        raise SystemExit("Usage: build-identity.py [--swift]")
    else:
        print(build_id())
