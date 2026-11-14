#!/usr/bin/env bash
# Linux GTK Agent requests and Agent access QA on a virtual X display in a
# disposable container: a fresh launch demo store (two accounts), the real
# mail-mcp acting as Claude Code and Codex through
# crates/mail-mcp/tests/agent_flow.py, and a fake `claude` CLI so Connect is
# exercised without any real Claude Code or Codex. Captures the states in
# dark, and the main ones in light. Synthetic data only; no account, keyring
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
  fonts-inter fonts-dejavu-core libgl1-mesa-dri libglib2.0-bin sqlite3 python3 dunst >/dev/null 2>&1
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

app_window=""; settings=""; store=""; bus=""; appearance=""; app_id=""
shot() { xdotool mousemove 2 990; sleep "${2:-1.2}"; import -frame -window "${3:-$app_window}" "/shots/linux-$appearance-$1.png"; echo "shot $1"; }
keys() { xdotool key --clearmodifiers "$@"; sleep 0.6; }
focus_app() { xdotool windowactivate --sync "$app_window" 2>/dev/null || true; sleep 0.4; }
as_demo() { su demo -c "$1"; }
agent() { # step account client [threads]
  as_demo "AGENT_FLOW_CLIENT=$3 AGENT_FLOW_THREADS='${4:-}' python3 /agent_flow.py /out/mail-mcp '$store' $1 $2"; echo
}
newest_window() { # newest visible window with this exact title, other than $2
  for _ in $(seq 50); do
    local id; id=$(xdotool search --onlyvisible --name "^$1\$" 2>/dev/null | grep -vx "${2:-none}" | tail -1 || true)
    [[ -n "$id" ]] && { echo "$id"; return; }
    sleep 0.2
  done
  echo "window $1 did not open" >&2; return 1
}
click() { # client coordinates in window $3 (default: the main window)
  xdotool mousemove --window "${3:-$app_window}" "$1" "$2" click 1; sleep 0.8
}
action() { # app action [string target]
  local target='[]'
  [[ $# -gt 1 ]] && target="[<'$2'>]"
  as_demo "DBUS_SESSION_BUS_ADDRESS=$bus gdbus call --session --dest $app_id --object-path /${app_id//.//} \
    --method org.gtk.Actions.Activate $1 \"$target\" {}" >/dev/null
  sleep 1.5
}
sql() { sqlite3 "$store" "$1"; }

start() { # appearance [helper|no-helper] [run name]
  appearance=$1
  local base="$scratch/${3:-$appearance}"
  mkdir -p "$base/store"
  chown -R demo "$base"
  chmod 700 "$base/store"
  store="$base/store/mail.sqlite"
  # The app keeps its light/dark choice beside the store.
  [[ "$appearance" == light ]] && { echo light > "$base/store/mail.theme"; chown demo "$base/store/mail.theme"; }
  # Desktop notifications on, as a user would turn them on.
  printf on > "$base/store/mail.notifications"; chown demo "$base/store/mail.notifications"
  bus="unix:path=$base/bus"
  as_demo "dbus-daemon --session --address=$bus --fork --nopidfile" >/dev/null
  # A notification server on that bus, so notifications show on the display.
  su demo -c "env DISPLAY=$DISPLAY DBUS_SESSION_BUS_ADDRESS=$bus dunst" >"$base/dunst.log" 2>&1 &
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
    XDG_DATA_HOME=$base/data EMAIL_STORE_PATH=$store EMAIL_DEMO=launch EMAIL_AGENT_CLI_PATH=$clis $helper \
    $binary" >"$base/app.log" 2>&1 &
  app_window=$(newest_window Email)
  xdotool windowsize "$app_window" 1240 780 windowmove "$app_window" 0 0
  sleep 4
  app_id="invalid.email.Unofficial.Isolated.s$(printf '%s' "$store" | sha256sum | cut -c1-12)"
  echo "built executable: $(sha256sum /out/email-linux | cut -d' ' -f1)"
  # The app runs as demo; root in a container may not read its /proc entry.
  for pid in $(pgrep -x email-linux); do echo "running process $pid: $(su demo -c "sha256sum /proc/$pid/exe" | cut -d' ' -f1)"; done
}

open_settings() {
  action open-agent-access
  settings=$(newest_window "Agent access")
  xdotool windowsize "$settings" 620 560 windowmove "$settings" 300 60
  sleep 3   # looking for agent CLIs
}

stop() {
  pkill -x email-linux || true
  sleep 1
}

# ---- dark: the whole flow -------------------------------------------------
start dark
agent use demo:personal claude-code >/dev/null
agent use demo:work codex-mcp-client >/dev/null
echo "access requests: $(sql "select count(*) from agent_actions where status='blocked'")"
sleep 6
import -window root "/shots/linux-$appearance-00-notification.png"; echo "shot 00-notification"
action open-agent-request ''
shot 01-access-requests 2
open_settings
shot 02-settings-waiting 1 "$settings"
keys Escape
focus_app
sleep 1
keys ctrl+Return   # Allow… on the selected refused agent
sheet=$(newest_window "Agent access" "$settings")
shot 03-sheet-default 1.5 "$sheet"
click 54 314 "$sheet"   # Custom…
shot 04-sheet-custom 1.5 "$sheet"
keys Escape
focus_app
keys ctrl+Return
sheet=$(newest_window "Agent access" "$settings")
click 170 314 "$sheet"  # What can agents see?
shot 05-sheet-details 1.5 "$sheet"
keys Escape
focus_app
keys ctrl+Return
sheet=$(newest_window "Agent access" "$settings")
click 43 203 "$sheet"   # Read, organize and write
click 415 314 "$sheet"  # Allow
sleep 1
echo "consent: $(sql "select account_id, grants_json, disclosure_version from agent_consents")"
threads=$(sql "select id from threads where account_id='demo:personal' and is_inbox=1 order by last_message_at desc limit 2" | paste -sd, -)
agent showcase demo:personal claude-code "$threads" | cut -c1-160
sleep 6
focus_app
keys k k
shot 06-requests-send 1.5
keys j
shot 07-requests-bulk 1.5
keys j
shot 08-requests-access 1.5
open_settings
shot 09-settings-waiting 1 "$settings"
keys Escape
focus_app
# Ctrl+Enter sends with the undo window.
keys k k
sleep 1
keys ctrl+Return
shot 10-sent-undo 1
draft=$(sql "select json_extract(details_json,'$.request.message_id') from agent_actions where kind='send_draft' and status='approved' limit 1")
echo "outbox after approval: $(sql "select status from outbox where message_id='$draft'")"
# E opens the agent's draft in the composer without sending.
agent send demo:personal claude-code >/dev/null
sleep 6
action open-agent-request ''
focus_app
keys j
sleep 1
shot 10b-second-send 0.5
keys e
sleep 2
shot 11-edit-in-composer 1
echo "edited: $(sql "select count(*) from agent_actions where status='edited'") outbox for it: $(sql "select count(*) from outbox o join agent_actions a on o.message_id=json_extract(a.details_json,'$.request.message_id') where a.status='edited'")"
keys Escape
action open-agent-request ''
focus_app
# Backspace declines the bulk change, then ignores the refused agent.
keys k k
sleep 1
keys BackSpace
sleep 2
echo "bulk: $(sql "select status from agent_actions where kind='batch_triage'")"
sleep 1
keys BackSpace
sleep 2
echo "waiting after ignore: $(sql "select count(*) from agent_actions where status='pending'") pending"
shot 12-requests-empty 1
open_settings
shot 13-settings-nothing-waiting 1 "$settings"
click 540 302 "$settings"   # Connect Claude Code (the fake CLI)
sleep 3
shot 14-settings-connected 1 "$settings"
echo "fake claude calls:"; sed 's/^/  /' "$fake/claude.log"
stop

# ---- light: the main states -----------------------------------------------
start light
agent use demo:personal claude-code >/dev/null
agent use demo:work codex-mcp-client >/dev/null
sleep 6
action open-agent-request ''
focus_app
sleep 1
keys ctrl+Return
sheet=$(newest_window "Agent access" "none")
shot 03-sheet-default 1.5 "$sheet"
click 43 203 "$sheet"
click 415 314 "$sheet"
threads=$(sql "select id from threads where account_id='demo:personal' and is_inbox=1 order by last_message_at desc limit 2" | paste -sd, -)
agent showcase demo:personal claude-code "$threads" >/dev/null
sleep 6
focus_app
keys k k
shot 06-requests-send 1.5
keys j
shot 07-requests-bulk 1.5
open_settings
shot 09-settings-waiting 1 "$settings"
stop
