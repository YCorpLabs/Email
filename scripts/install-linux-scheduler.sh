#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$repo_root/scripts/distribution-profile.sh"
mode="${1:-install}"
if [[ "$mode" != "install" && "$mode" != "--dry-run" ]]; then
  echo "Usage: scripts/install-linux-scheduler.sh [--dry-run]" >&2
  exit 2
fi
dispatch_bin="$repo_root/target/release/mail-mcp"
if [[ "$mode" == "install" && ! -x "$dispatch_bin" ]]; then
  echo "Build the dispatcher first with cargo build -p mail-mcp --release." >&2
  exit 1
fi

data_root="${XDG_DATA_HOME:-$HOME/.local/share}"
database_path="$data_root/$(email_profile_value "$repo_root" LINUX_DATA_DIRECTORY)/mail.sqlite"
unit="$(email_profile_value "$repo_root" LINUX_DISPATCH_UNIT)"
if [[ "$mode" == "--dry-run" ]]; then
  unit_dir="$(mktemp -d)"
  trap 'rm -rf "$unit_dir"' EXIT
else
  unit_dir="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
fi
mkdir -p "$unit_dir"
python3 - "$dispatch_bin" "$database_path" "$unit_dir" "$unit" <<'PY'
import pathlib
import sys

program, database, unit_directory, unit = sys.argv[1:]
folder = pathlib.Path(unit_directory)
def quote(value: str) -> str:
    return '"' + value.replace('\\', '\\\\').replace('"', '\\"').replace('%', '%%') + '"'

(folder / f'{unit}.service').write_text(
    '[Unit]\nDescription=Send due Email drafts\n'
    f'ConditionPathExists={quote(database)}\n\n'
    '[Service]\nType=oneshot\n'
    f'ExecStart={quote(program)} --db {quote(database)} --allow-send --dispatch-due\n',
    encoding='utf-8',
)
(folder / f'{unit}.timer').write_text(
    '[Unit]\nDescription=Check Email scheduled drafts\n\n'
    '[Timer]\nOnStartupSec=1min\nOnUnitActiveSec=30s\nAccuracySec=5s\n'
    f'Unit={unit}.service\n\n'
    '[Install]\nWantedBy=timers.target\n',
    encoding='utf-8',
)
PY
if [[ "$mode" == "--dry-run" ]]; then
  echo "Scheduled mail dispatcher units generated."
  exit 0
fi
systemctl --user daemon-reload
systemctl --user enable --now "$unit.timer"
echo "Scheduled mail dispatcher installed for this user session."
