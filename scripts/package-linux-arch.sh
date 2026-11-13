#!/usr/bin/env bash
set -euo pipefail
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$repo_root/scripts/package-notices.sh"
require_package_notices "$repo_root"
command -v makepkg >/dev/null || { echo "Run this script on Arch/Omarchy with base-devel installed." >&2; exit 1; }
[[ "$(id -u)" != 0 ]] || { echo "makepkg must run as a normal user, not root." >&2; exit 1; }
output_dir="${EMAIL_LINUX_PACKAGE_DIR:-$repo_root/dist-linux}"
mkdir -p "$output_dir"
export PKGDEST="$(cd "$output_dir" && pwd)"
if [[ -n "${CARGO_TARGET_DIR:-}" ]]; then
  mkdir -p "$CARGO_TARGET_DIR"
  export CARGO_TARGET_DIR="$(cd "$CARGO_TARGET_DIR" && pwd)"
fi
cd "$repo_root/apps/linux/packaging/arch"
exec makepkg --force --cleanbuild "$@"
