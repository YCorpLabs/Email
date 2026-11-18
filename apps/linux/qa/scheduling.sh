#!/usr/bin/env bash
# Linux GTK Scheduling opt-in QA on a virtual X display in a disposable
# container: the launch demo store with the Scheduling connection not
# connected, waiting for the browser, and connected. Captures Manage accounts,
# the composer, the command palette ("sched" and "book") and Calendar in each.
# The connection state comes from EMAIL_QA_SCHEDULING, which only a
# `--features qa-fixture` build honors, so no Scheduling service, browser or
# keyring is used. Usage (from the repository root, with a development-profile
# QA build of the shell in ./qa-out/email-linux):
#
#   docker run --rm --memory=3g --cpus=2 -v "$PWD/qa-out:/out:ro" -v "$PWD/qa-shots:/shots" \
#     -v "$PWD/apps/linux/qa/scheduling.sh:/qa.sh:ro" \
#     email-linux-dev:<rust version> bash /qa.sh
#
# See docs/scheduling.md.
set -euo pipefail
scratch="$(mktemp -d)"
chmod 755 "$scratch"
apt-get update -qq >/dev/null
apt-get install -y -qq --no-install-recommends xvfb openbox xdotool imagemagick dbus dbus-x11 \
  fonts-inter fonts-dejavu-core libgl1-mesa-dri tzdata sqlite3 >/dev/null 2>&1
Xvfb :99 -screen 0 1440x1000x24 -nolisten tcp >"$scratch/xvfb.log" 2>&1 &
sleep 1
DISPLAY=:99 openbox >"$scratch/openbox.log" 2>&1 &
sleep 1
export DISPLAY=:99
useradd -m demo 2>/dev/null || true
echo "built executable: $(sha256sum /out/email-linux | cut -d' ' -f1)"

app_window=""
newest_window() { # newest visible window with this exact title, other than $2
  for _ in $(seq 100); do
    local id; id=$(xdotool search --onlyvisible --name "^$1\$" 2>/dev/null | grep -vx "${2:-none}" | tail -1 || true)
    [[ -n "$id" ]] && { echo "$id"; return; }
    sleep 0.2
  done
  echo "window $1 did not open" >&2; return 1
}
# XTEST input to the focused window inside this virtual display only.
keys() { xdotool key --clearmodifiers "$@"; sleep 0.6; }
type_text() { xdotool type --delay 30 "$1"; sleep 0.6; }
shot() { xdotool mousemove 1435 995; sleep "${2:-1.2}"; import -window root "/shots/linux-$1.png"; echo "shot $1"; }
shot_window() { xdotool mousemove 1435 995; sleep 1.2; import -frame -window "$2" "/shots/linux-$1.png"; echo "shot $1"; }
focus_app() { xdotool windowactivate --sync "$app_window" 2>/dev/null || true; sleep 0.4; }

start() { # state
  local base="$scratch/$1"
  mkdir -p "$base/store"; chown -R demo "$base"; chmod 700 "$base/store"
  echo light > "$base/store/mail.theme"; chown demo "$base/store/mail.theme"
  local bus="unix:path=$base/bus"
  su demo -c "dbus-daemon --session --address=$bus --fork --nopidfile" >/dev/null
  su demo -c "env DISPLAY=$DISPLAY DBUS_SESSION_BUS_ADDRESS=$bus GDK_BACKEND=x11 GSK_RENDERER=cairo LIBGL_ALWAYS_SOFTWARE=1 \
    NO_AT_BRIDGE=1 EMAIL_SOFTWARE_RENDERING=1 WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS=1 TZ=Europe/Oslo HOME=$base \
    XDG_DATA_HOME=$base/data EMAIL_STORE_PATH=$base/store/mail.sqlite EMAIL_DEMO=launch EMAIL_QA_SCHEDULING=$1 \
    /out/email-linux" >"$base/app.log" 2>&1 &
  app_window=$(newest_window Email)
  xdotool windowsize "$app_window" 1440 1000 windowmove "$app_window" 0 0
  sleep 5
  for pid in $(pgrep -x email-linux); do echo "running process $pid: $(su demo -c "sha256sum /proc/$pid/exe" | cut -d' ' -f1)"; done
  focus_app
}
stop() { pkill -x email-linux || true; sleep 1; }
palette() { # query
  focus_app; keys ctrl+k; type_text "$1"
}
accounts() { # name: open Manage accounts from the palette's Scheduling command
  palette "sched"; keys Return
  local window; window=$(newest_window Accounts)
  xdotool windowmove "$window" 420 220
  shot_window "$1" "$window"
  # The Accounts window has no Esc binding; close it with the window manager.
  xdotool windowactivate --sync "$window" 2>/dev/null || true
  keys alt+F4
}
composer() { # name: reply to the selected conversation, then "book" in its palette
  focus_app; keys r; sleep 1.5
  shot "$1-composer"
  palette "book"; shot "$1-composer-palette-book"
  keys Escape; keys Escape
}

start disconnected
shot disconnected-main-window
palette "sched"; shot disconnected-palette-sched; keys Escape
accounts disconnected-accounts
composer disconnected
focus_app; keys g c; sleep 2; shot disconnected-calendar; keys g i
stop

start waiting
accounts waiting-accounts
stop

start connected
accounts connected-accounts
palette "book"; shot connected-palette-book; keys Escape
composer connected
focus_app; keys g c; sleep 2; shot connected-calendar
stop
