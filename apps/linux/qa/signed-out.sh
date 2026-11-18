#!/usr/bin/env bash
# Linux GTK signed-out account QA on a virtual X display in a disposable
# container: the launch demo store with the personal demo account given Gmail
# settings and a saved token that has expired with no refresh token, so the
# engine's own sync finds it signed out, as when Google answers invalid_grant.
# No request leaves the container. Captures All Inboxes, a refresh (Ctrl+R),
# the account's inbox, a narrow window, Manage accounts and Sign in again
# waiting for the browser. A fake xdg-open records the sign-in URL. Needs a
# `--features qa-fixture` build of the shell (credentials come from
# EMAIL_TEST_CREDENTIALS_DIR) in ./qa-out/email-linux. Usage, from the
# repository root:
#
#   docker run --rm --memory=3g --cpus=2 -v "$PWD/qa-out:/out:ro" -v "$PWD/qa-shots:/shots" \
#     -v "$PWD/apps/linux/qa/signed-out.sh:/qa.sh:ro" \
#     email-linux-dev:<rust version> bash /qa.sh [light|dark]
#
# Sidebar and notice positions assume the 1440x1000 window at its default
# column widths.
set -euo pipefail
theme="${1:-light}"
scratch="$(mktemp -d)"
chmod 755 "$scratch"
apt-get update -qq >/dev/null
apt-get install -y -qq --no-install-recommends xvfb openbox xdotool imagemagick dbus dbus-x11 \
  fonts-inter fonts-dejavu-core libgl1-mesa-dri tzdata >/dev/null 2>&1
Xvfb :99 -screen 0 1440x1000x24 -nolisten tcp >"$scratch/xvfb.log" 2>&1 &
sleep 1
DISPLAY=:99 openbox >"$scratch/openbox.log" 2>&1 &
sleep 1
export DISPLAY=:99
useradd -m demo 2>/dev/null || true
echo "built executable: $(sha256sum /out/email-linux | cut -d' ' -f1)"

newest_window() { # newest visible window with this exact title
  for _ in $(seq 100); do
    local id; id=$(xdotool search --onlyvisible --name "^$1\$" 2>/dev/null | tail -1 || true)
    [[ -n "$id" ]] && { echo "$id"; return; }
    sleep 0.2
  done
  echo "window $1 did not open" >&2; return 1
}
keys() { xdotool key --clearmodifiers "$@"; sleep 0.6; }
type_text() { xdotool type --delay 30 "$1"; sleep 0.6; }
click() { xdotool mousemove "$1" "$2" click 1; sleep 1; }
shot() { xdotool mousemove 1435 995; sleep "${2:-1.2}"; import -window root "/shots/linux-$theme-$1.png"; echo "shot $1"; }
shot_window() { xdotool mousemove 1435 995; sleep 1.2; import -frame -window "$2" "/shots/linux-$theme-$1.png"; echo "shot $1"; }
visible_windows() { xdotool search --onlyvisible --name '.' 2>/dev/null | wc -l; }

base="$scratch/run"
mkdir -p "$base/store" "$base/credentials" "$base/bin"
echo "$theme" > "$base/store/mail.theme"
# Gmail settings and a dead token for the personal demo account.
printf '[{"account_id":"demo:personal","generation":"check","provider":"gmail","client_id":"synthetic-client.apps.googleusercontent.com"}]' \
  > "$base/store/mail.accounts.json"
token="$base/credentials/$(printf '%s' 'demo:personal' | sha256sum | cut -d' ' -f1)"
printf '{"access_token":"synthetic-expired","refresh_token":null,"expires_at_ms":0}' > "$token"
printf '#!/bin/sh\nprintf "%%s\\n" "$*" >> %s/open.log\n' "$base" > "$base/bin/xdg-open"
chmod 755 "$base/bin/xdg-open"
chown -R demo "$base"
chmod 700 "$base/store" "$base/credentials"
chmod 600 "$base/store/mail.accounts.json" "$token"
bus="unix:path=$base/bus"
su demo -c "dbus-daemon --session --address=$bus --fork --nopidfile" >/dev/null
su demo -c "env DISPLAY=$DISPLAY DBUS_SESSION_BUS_ADDRESS=$bus GDK_BACKEND=x11 GSK_RENDERER=cairo LIBGL_ALWAYS_SOFTWARE=1 \
  NO_AT_BRIDGE=1 EMAIL_SOFTWARE_RENDERING=1 WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS=1 TZ=Europe/Oslo HOME=$base \
  XDG_DATA_HOME=$base/data EMAIL_STORE_PATH=$base/store/mail.sqlite EMAIL_DEMO=launch \
  EMAIL_TEST_CREDENTIALS_DIR=$base/credentials PATH=$base/bin:/usr/local/bin:/usr/bin:/bin \
  /out/email-linux" >"$base/app.log" 2>&1 &
app_window=$(newest_window Email)
xdotool windowsize "$app_window" 1440 1000 windowmove "$app_window" 0 0
sleep 6
for pid in $(pgrep -x email-linux); do echo "running process $pid: $(su demo -c "sha256sum /proc/$pid/exe" | cut -d' ' -f1)"; done
xdotool windowactivate --sync "$app_window" 2>/dev/null || true
sleep 0.5

# Refresh, then All Inboxes with the notice and the sidebar mark.
before=$(visible_windows)
keys ctrl+r
sleep 4
after=$(visible_windows)
echo "visible windows before refresh: $before, after: $after"
[[ "$after" -le "$before" ]] || { echo "FAIL: a refresh opened a window"; exit 1; }
shot 01-all-inboxes

# The account's own inbox: its sidebar row.
click 100 732
shot 02-account-inbox

# Narrow window.
xdotool windowsize "$app_window" 1000 760
sleep 1.5
shot 03-narrow
xdotool windowsize "$app_window" 1440 1000
sleep 1.5

# Manage accounts from the palette.
keys ctrl+k; type_text "manage"; keys Return
accounts=$(newest_window Accounts)
xdotool windowmove "$accounts" 420 220
shot_window 05-manage-accounts "$accounts"
xdotool windowactivate --sync "$accounts" 2>/dev/null || true
keys alt+F4
xdotool windowactivate --sync "$app_window" 2>/dev/null || true

# Sign in again: the fake xdg-open records Google's sign-in URL.
click 282 228
sleep 2
shot 04-waiting-for-browser
if grep -q "accounts.google.com" "$base/open.log" 2>/dev/null; then
  grep -q "login_hint=" "$base/open.log" && echo "PASS sign-in opened Google with a login hint"
else
  echo "FAIL: Sign in again did not open the browser"; cat "$base/app.log"; exit 1
fi
pkill -x email-linux || true
