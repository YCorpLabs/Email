#!/usr/bin/env python3
"""Offline source preflight. Success does not authorize public distribution."""

import json
from pathlib import Path, PurePosixPath
import subprocess
import sys


ROOT = Path(__file__).resolve().parents[1]


def git(*args):
    return subprocess.check_output(["git", "-C", str(ROOT), *args])


def main():
    if len(sys.argv) != 1:
        print("Usage: python3 scripts/check-release-readiness.py", file=sys.stderr)
        return 2
    failures = []
    tracked = set(git("ls-files", "-z").decode().split("\0")) - {""}
    # Inspect the index, not only .gitignore: force-added private files still fail.
    for name in sorted(tracked):
        path = PurePosixPath(name)
        if (
            name.startswith("config/") and (path.name.endswith(".local") or ".local." in path.name)
            or name.startswith("config/macos-update-signing.local/")
            or path.name in {".dev.vars", ".env", "credentials.json", "token.json"}
            or ".secrets" in path.parts or ".wrangler" in path.parts
            or name.endswith((".p12", ".pfx", ".key", ".sqlite", ".accounts.json", ".accounts.lock"))
            or ".accounts.json.tmp" in path.name
            or ".sqlite-" in path.name
        ):
            failures.append(f"Private/local file is tracked: {name}")
    for name in ("Cargo.lock", "apps/linux/Cargo.lock"):
        if name not in tracked or not (ROOT / name).is_file():
            failures.append(f"Missing tracked dependency lockfile: {name}")
    for name in (
        "config/oauth-desktop.local.env", "config/distribution-profile.local",
        "config/macos-signing-identity.local",
        "config/macos-update-signing.local/private.txt",
        "dist-release/distribution/Email.app/Contents/Info.plist",
    ):
        result = subprocess.run(
            ["git", "-C", str(ROOT), "check-ignore", "--no-index", "-q", name],
            check=False,
        )
        if result.returncode != 0:
            failures.append(f"Private/generated path is not ignored: {name}")
    for name in sorted(tracked):
        if name.startswith("scripts/") and name.endswith(".sh"):
            result = subprocess.run(["bash", "-n", str(ROOT / name)], check=False)
            if result.returncode:
                failures.append(f"Shell syntax failed: {name}")
    for args in (("diff", "--check"), ("diff", "--cached", "--check")):
        result = subprocess.run(["git", "-C", str(ROOT), *args], check=False)
        if result.returncode:
            failures.append(f"Working changes failed git {' '.join(args)}")
    if failures:
        print("Source preflight FAILED:", file=sys.stderr)
        for failure in failures:
            print(f"- {failure}", file=sys.stderr)
        return 1
    print("Source preflight passed: lockfiles, local-file exclusions, shell syntax, diff whitespace.")
    print("Public release approval remains external: Google verification, signing/notarization,")
    print("license/notices, privacy/support, provider acceptance and restore acceptance.")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f"Source preflight could not complete: {error}", file=sys.stderr)
        sys.exit(1)
