#!/usr/bin/env bash
# Uploads a verified update staging directory to the update bucket with
# Wrangler, in dependency order. Never run it on a directory that the
# publishers did not stage. Details: scripts/upload-updates.py.
#
#   scripts/upload-updates.sh --dry-run --source DIR --prefix email/apt
#   scripts/upload-updates.sh --dry-run --source DIR --prefix email/arch
#   scripts/upload-updates.sh --dry-run --source DIR --prefix email/macos
#   scripts/upload-updates.sh --source DIR --prefix email/apt
set -euo pipefail
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
if [[ " $* " != *" --dry-run "* ]]; then
  command -v npx >/dev/null || { echo "npx (Node.js) is required to run Wrangler." >&2; exit 1; }
fi
exec python3 -I "$repo_root/scripts/upload-updates.py" "$@"
