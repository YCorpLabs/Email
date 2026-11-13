#!/usr/bin/env bash
# Linux GTK Agent access QA on a virtual X display in a disposable container:
# a fresh demo store (EMAIL_DEMO=1), the real mail-mcp acting as an agent
# through crates/mail-mcp/tests/agent_flow.py, and a fake `claude` CLI so
# Connect is exercised without any real Claude Code or Codex. Captures the
# window's states in dark and light. Synthetic data only; no account, keyring
# or provider. Usage (from the repository root, with Linux builds of the shell
# and the helper in ./qa-out):
#
#   docker run --rm -v "$PWD/qa-out:/out:ro" -v "$PWD/qa-shots:/shots" \
#     -v "$PWD/apps/linux/qa/agent-access.sh:/qa.sh:ro" \
#     -v "$PWD/crates/mail-mcp/tests/agent_flow.py:/agent_flow.py:ro" \
#     email-linux-dev:<rust version> bash /qa.sh
#
# /out must contain email-linux and mail-mcp. See docs/agent-access.md.
set -euo pipefail
scratch="$(mktemp -d)"
chmod 755 "$scratch"
apt-get update -qq >/dev/null
apt-get install -y -qq --no-install-recommends xvfb openbox xdotool imagemagick dbus dbus-x11 \
  fonts-inter fonts-dejavu-core libgl1-mesa-dri libglib2.0-bin sqlite3 python3 >/dev/null 2>&1
Xvfb :99 -screen 0 1280x1000x24 -nolisten tcp >"$scratch/xvfb.log" 2>&1 &
sleep 1
DISPLAY=:99 openbox >"$scratch/openbox.log" 2>&1 &
sleep 1
export DISPLAY=:99
useradd -m demo 2>/dev/null || true

# A stand-in for the claude CLI. It only writes next to itself.
fake="$scratch/fake-bin"
mkdir -p "$fake"
cat > "$fake/claude" <<'SH'
#!/bin/sh
dir="$(cd "$(dirname "$0")" && pwd)"
name="$(basename "$0")"
printf '%s\n' "$*" >> "$dir/$name.log"
case "$1 $2" in
  "mcp get")
    if [ -f "$dir/$name.registered" ] && grep -qx "$3" "$dir/$name.registered"; then echo "$3:"; cat "$dir/$name.command"; exit 0; fi
    echo "No MCP server found with name: $3" >&2; exit 1 ;;
  "mcp add")
    server="$3"; [ "$3" = "--scope" ] && server="$7"
    echo "$server" >> "$dir/$name.registered"; printf 'Command: %s\n' "$*" > "$dir/$name.command"; exit 0 ;;
esac
exit 2
SH
chmod 755 "$fake/claude"
chown -R demo "$fake"

app_window=""; panel=""; store=""; bus=""; appearance=""
shot() { xdotool mousemove 2 990; sleep "${2:-1.2}"; import -frame -window "${3:-$panel}" "/shots/linux-$appearance-$1.png"; echo "shot $1"; }
keys() { xdotool key --clearmodifiers "$@"; sleep 0.5; }
as_demo() { su demo -c "$1"; }
agent() { as_demo "python3 /agent_flow.py /out/mail-mcp '$store' $1" ; echo; }
newest_window() { # newest visible window with this exact title, other than $2
  for _ in $(seq 50); do
    local id; id=$(xdotool search --onlyvisible --name "^$1\$" 2>/dev/null | grep -vx "${2:-none}" | tail -1 || true)
    [[ -n "$id" ]] && { echo "$id"; return; }
    sleep 0.2
  done
  echo "window $1 did not open" >&2; return 1
}

start() { # appearance [helper|no-helper] [run name]
  appearance=$1
  local base="$scratch/${3:-$appearance}"
  mkdir -p "$base/store"
  chown -R demo "$base"
  chmod 700 "$base/store"
  store="$base/store/mail.sqlite"
  # The app keeps its light/dark choice beside the store.
  [[ "$appearance" == light ]] && { echo light > "$base/store/mail.theme"; chown demo "$base/store/mail.theme"; }
  bus="unix:path=$base/bus"
  as_demo "dbus-daemon --session --address=$bus --fork --nopidfile" >/dev/null
  local helper="EMAIL_MCP_HELPER=/out/mail-mcp" binary=/out/email-linux clis="$fake"
  if [[ "${2:-helper}" == no-helper ]]; then
    # No helper anywhere: not configured, and not beside the executable.
    helper=""
    binary="$base/app/email-linux"
    install -D -m755 /out/email-linux "$binary"
    clis="$base/fake-bin"
    install -D -m755 "$fake/claude" "$clis/claude"
    chown -R demo "$base"
  fi
  su demo -c "env DISPLAY=$DISPLAY DBUS_SESSION_BUS_ADDRESS=$bus GDK_BACKEND=x11 GSK_RENDERER=cairo LIBGL_ALWAYS_SOFTWARE=1 \
    NO_AT_BRIDGE=1 EMAIL_SOFTWARE_RENDERING=1 WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS=1 TZ=Europe/Oslo HOME=$base \
    XDG_DATA_HOME=$base/data EMAIL_STORE_PATH=$store EMAIL_DEMO=1 EMAIL_AGENT_CLI_PATH=$clis $helper \
    $binary" >"$base/app.log" 2>&1 &
  app_window=$(newest_window Email)
  xdotool windowsize "$app_window" 1200 860 windowmove "$app_window" 0 0
  sleep 3
  echo "built executable: $(sha256sum /out/email-linux | cut -d' ' -f1)"
  # The app runs as demo; root in a container may not read its /proc entry.
  for pid in $(pgrep -x email-linux); do echo "running process $pid: $(su demo -c "sha256sum /proc/$pid/exe" | cut -d' ' -f1)"; done
}

open_panel() {
  # The notification action, over the app's own session bus.
  local digest id
  digest=$(printf '%s' "$store" | sha256sum | cut -c1-12)
  id="invalid.email.Unofficial.Isolated.s$digest"
  as_demo "DBUS_SESSION_BUS_ADDRESS=$bus gdbus call --session --dest $id --object-path /${id//.//} \
    --method org.gtk.Actions.Activate open-agent-access [] {}" >/dev/null
  panel=$(newest_window "Agent access")
  xdotool windowsize "$panel" 600 680 windowmove "$panel" 300 40
  xdotool windowactivate --sync "$panel" 2>/dev/null || true
  sleep 4   # looking for agent CLIs
}

stop() {
  pkill -x email-linux || true
  sleep 1
}

click() { # client coordinates in window $3 (default: the panel)
  xdotool mousemove --window "${3:-$panel}" "$1" "$2" click 1; sleep 0.8
}
root_shot() { sleep "${2:-1}"; import -window root "/shots/debug-$appearance-$1.png"; }

# ---- dark: the whole flow -------------------------------------------------
start dark
echo "probe: $(agent probe)"
open_panel
shot 01-blocked-nothing-pending
click 536 520   # the account switch opens the turn-on window
sheet=$(newest_window "Agent access" "$panel")
shot 02-sheet-collapsed 1.5 "$sheet"
click 114 346 "$sheet"   # What exactly can agents see?
shot 03-sheet-expanded 1.5 "$sheet"
keys Escape
shot 03b-cancelled-switch-off
click 147 165   # Turn on for alex@example.com… in the banner
sheet=$(newest_window "Agent access" "$panel")
for y in 131 152 173; do click 32 $y "$sheet"; done   # Drafts, Archive and snooze, Ask to send
shot 03c-three-grants-checked 0.5 "$sheet"
click 400 393 "$sheet"   # Turn on
shot 04-account-on 2
click 315 408   # Edit: the sheet opens with the current grants checked
sheet=$(newest_window "Agent access" "$panel")
shot 04b-edit-sheet 1.5 "$sheet"
keys Escape
echo "consent: $(sqlite3 "$store" 'select grants_json, disclosure_version from agent_consents')"
echo "read: $(agent read | cut -c1-80)"
echo "request: $(agent request | cut -c1-200)"
shot 05-pending 7
click 84 264    # Show message
shot 06-pending-expanded
xdotool windowsize "$panel" 440 640
shot 07-pending-narrow 1.5
xdotool windowsize "$panel" 600 680
sleep 1
click 88 410    # Hide message
click 495 264   # Approve and send
shot 08-approved-undo 1
echo "outbox after approval: $(sqlite3 "$store" 'select status from outbox')"
click 468 650   # Undo send
shot 09-undo-send 1
echo "outbox after undo: $(sqlite3 "$store" 'select count(*) from outbox')"
sleep 1
click 446 301   # Reject the batch
shot 09b-rejected 1
click 78 459    # Recent activity
shot 10-recent-activity
click 78 459
click 525 154   # Connect Claude Code
shot 11-connected 3
echo "fake claude calls:"; sed 's/^/  /' "$fake/claude.log"
click 536 399   # switch off
dialog=$(xdotool getactivewindow)
shot 12-turn-off-confirm 1 "$dialog"
keys Escape
shot 12b-still-on 1
echo "consent after cancel: $(sqlite3 "$store" 'select count(*) from agent_consents')"
echo "attach: $(agent attach | cut -c1-120)"
sleep 7
click 84 264    # Show message · 1 attachment
shot 13-attachment
stop

# ---- light: the main states -----------------------------------------------
start light
agent probe >/dev/null
open_panel
shot 01-blocked-nothing-pending
click 147 165   # Turn on for alex@example.com… in the banner
sheet=$(newest_window "Agent access" "$panel")
shot 02-sheet-collapsed 1.5 "$sheet"
for y in 131 152 173; do click 32 $y "$sheet"; done
click 400 393 "$sheet"
shot 04-account-on 2
agent request >/dev/null
shot 05-pending 7
click 84 264
shot 06-pending-expanded
stop

# ---- no helper: Connect explains instead of offering a broken button ------
start dark no-helper no-helper
open_panel
shot 14-no-helper
stop
