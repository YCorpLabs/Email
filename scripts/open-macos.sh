#!/usr/bin/env bash
set -euo pipefail

# Launch the primary checkout's development app, never a name-matched alpha.
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
common_dir="$(git -C "$repo_root" rev-parse --path-format=absolute --git-common-dir)"
primary_root="$(dirname "$common_dir")"
app="$primary_root/dist/Email.app"
if [[ "${1:-}" == "--print-path" ]]; then
  printf '%s\n' "$app"
  exit 0
fi
if [[ $# -gt 1 || ( $# -eq 1 && "$1" != "--verify" ) ]]; then
  echo "Usage: scripts/open-macos.sh [--print-path|--verify]" >&2
  exit 2
fi
if [[ ! -d "$app" ]]; then
  echo "Build the primary checkout with scripts/run-macos.sh --package first." >&2
  exit 1
fi
identifier="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$app/Contents/Info.plist")"
expected="$(python3 "$repo_root/scripts/distribution-profile.py" --root "$primary_root" get MACOS_BUNDLE_ID)"
distribution="$(python3 "$repo_root/scripts/distribution-profile.py" --root "$primary_root" get DISTRIBUTION)"
if [[ "$identifier" != "$expected" || "$distribution" == true ]]; then
  echo "Refusing to launch a bundle that does not match the primary checkout's distribution profile." >&2
  exit 1
fi
codesign --verify --deep --strict "$app"
requirement="$(codesign -dr - "$app" 2>&1 | sed -n 's/^designated => //p')"
for library in libEmailUI.dylib libmail_service.dylib; do
  codesign --verify --strict -R "=$requirement" "$primary_root/dist/Runtime/$library"
done
python3 - "$primary_root" "$repo_root" <<'PY'
import hashlib, json, subprocess, sys
from pathlib import Path
root = Path(sys.argv[1])
record_path = root / 'dist/email-build.json'
if not record_path.exists():
    raise SystemExit('Build record missing. Run scripts/run-macos.sh --package in the primary checkout.')
record = json.loads(record_path.read_text())
identity = subprocess.check_output([sys.executable, str(root / 'scripts/build-identity.py')], text=True).strip()
caller_identity = subprocess.check_output([sys.executable, str(Path(sys.argv[2]) / 'scripts/build-identity.py')], text=True).strip()
if caller_identity != identity:
    raise SystemExit('This worktree differs from the primary checkout. Integrate its native changes before launching.')
if record['buildIdentity'] != identity:
    raise SystemExit('Packaged Email is older than the sources. Run scripts/run-macos.sh --package in the primary checkout.')
for name, expected in record['artifacts'].items():
    if hashlib.sha256((root / 'dist' / name).read_bytes()).hexdigest() != expected:
        raise SystemExit('Packaged Email differs from its build record. Rebuild before launching.')
PY
if [[ "${1:-}" == "--verify" ]]; then
  echo "Verified $app"
  exit 0
fi
open "$app"
