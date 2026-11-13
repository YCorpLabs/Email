#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$repo_root/scripts/distribution-profile.sh"
# The dispatcher label derives from the distribution profile's bundle ID.
label="$(email_profile_value "$repo_root" MACOS_BUNDLE_ID).dispatch"
mode="${1:-install}"
if [[ "$mode" != "install" && "$mode" != "--dry-run" ]]; then
  echo "Usage: scripts/install-macos-scheduler.sh [--dry-run]" >&2
  exit 2
fi
email_bin="$repo_root/dist/Email.app/Contents/MacOS/Email"
if [[ ! -x "$email_bin" ]]; then
  echo "Build dist/Email.app first with scripts/run-macos.sh --package." >&2
  exit 1
fi

if [[ "$mode" == "--dry-run" ]]; then
  agent_dir="$(mktemp -d)"
  trap 'rm -rf "$agent_dir"' EXIT
else
  agent_dir="$HOME/Library/LaunchAgents"
fi
agent_file="$agent_dir/$label.plist"
mkdir -p "$agent_dir"
python3 - "$email_bin" "$agent_file" "$label" <<'PY'
import os
import plistlib
import sys

program, destination, label = sys.argv[1:]
settings = {
    "Label": label,
    "ProgramArguments": [program, "--dispatch-due"],
    "RunAtLoad": True,
    "StartInterval": 30,
    "ProcessType": "Background",
}
with open(destination, "wb") as file:
    plistlib.dump(settings, file)
os.chmod(destination, 0o600)
PY
plutil -lint "$agent_file" >/dev/null
if [[ "$mode" == "--dry-run" ]]; then
  echo "Scheduled mail dispatcher plist is valid."
  exit 0
fi
launchctl bootout "gui/$(id -u)" "$agent_file" 2>/dev/null || true
launchctl bootstrap "gui/$(id -u)" "$agent_file"
echo "Scheduled mail dispatcher installed for this login session."
