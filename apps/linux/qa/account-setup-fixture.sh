#!/usr/bin/env bash
# Linux GTK account-setup QA against the synthetic Gmail fixture, on a virtual
# X display in a disposable container. Synthetic data and file credentials only;
# never a real mailbox or keyring. Usage (from the repository root, after
# building the QA binaries with `--features qa-fixture` into ./qa-out):
#
#   docker run --rm -v "$PWD/qa-out:/out:ro" -v "$PWD/qa-shots:/shots" \
#     -v "$PWD/apps/linux/qa/account-setup-fixture.sh:/qa.sh:ro" \
#     email-linux-verify:local bash /qa.sh
#
# /out must contain email-linux-distribution-qa (EMAIL_RELEASE_CHANNEL=distribution),
# email-linux-local-alpha-qa and gmail_fixture (mail-imap example). See
# docs/account-setup.md.
set -euo pipefail
# Disposable scratch space, readable by the app user.
scratch="$(mktemp -d)"
chmod 755 "$scratch"
apt-get update -qq >/dev/null
apt-get install -y -qq --no-install-recommends xvfb openbox xdotool imagemagick dbus dbus-x11 fonts-inter fonts-dejavu-core libgl1-mesa-dri >/dev/null 2>&1
Xvfb :99 -screen 0 1400x1000x24 -nolisten tcp >$scratch/xvfb.log 2>&1 &
sleep 1
DISPLAY=:99 openbox >$scratch/openbox.log 2>&1 &
sleep 1
export DISPLAY=:99 GDK_BACKEND=x11 GSK_RENDERER=cairo LIBGL_ALWAYS_SOFTWARE=1 NO_AT_BRIDGE=1 EMAIL_SOFTWARE_RENDERING=1
# Container QA only: Docker's seccomp profile blocks WebKit's namespace sandbox.
export WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS=1
/out/gmail_fixture $scratch/fixture-ca.pem >$scratch/fixture.log 2>&1 &
for _ in $(seq 50); do grep -q ENDPOINTS $scratch/fixture.log 2>/dev/null && break; sleep 0.1; done
export EMAIL_TEST_FIXTURE_CA=$scratch/fixture-ca.pem
export EMAIL_TEST_FIXTURE_ENDPOINTS="$(sed -n 's/^EMAIL_TEST_FIXTURE_ENDPOINTS=//p' $scratch/fixture.log)"
# Without the redirects the app would sign in to the real Gmail servers.
[[ -n "$EMAIL_TEST_FIXTURE_ENDPOINTS" ]] || { echo "The Gmail fixture did not start." >&2; exit 1; }
shot() { sleep "${2:-1.2}"; import -window root "/shots/$1.png"; echo "shot $1"; }
dialog() {
  for _ in $(seq 50); do
    local id; id=$(xdotool search --name "Connect an account" 2>/dev/null | head -1)
    [[ -n "$id" ]] && { echo "$id"; return; }
    sleep 0.2
  done
  echo "account window did not open" >&2; return 1
}

run() { # mode binary
  local mode=$1 binary=$2 base=$scratch/qa-$1
  mkdir -p "$base/home" "$base/data"
  HOME="$base/home" XDG_DATA_HOME="$base/data" EMAIL_TEST_CREDENTIALS_DIR="$base/credentials" \
    EMAIL_GOOGLE_CLIENT_ID=fixture-not-a-real-client.apps.googleusercontent.com \
    dbus-run-session -- "$binary" >"$scratch/app-$mode.log" 2>&1 &
  for _ in $(seq 100); do pgrep -f "$binary" -n >/dev/null && xdotool search --name "^Email" >/dev/null 2>&1 && break; sleep 0.2; done
  sleep 2
  local pid; pid=$(pgrep -f "^$binary" -n)
  echo "$mode running exe sha256 $(sha256sum /proc/$pid/exe | cut -d' ' -f1) artifact $(sha256sum "$binary" | cut -d' ' -f1)"
  xdotool search --name "^Email" windowactivate --sync 2>/dev/null || true
  xdotool key a
  local d; d=$(dialog); xdotool windowactivate --sync "$d"
}

# Distribution build: app password first, Outlook hidden.
run distribution /out/email-linux-distribution-qa
shot distribution-1-app-password
xdotool key ctrl+2; shot distribution-2-icloud
xdotool key ctrl+1; sleep 0.5
xdotool type --delay 15 "fixture.user@gmail.com"; xdotool key Tab
xdotool type --delay 15 "my regular google password"; xdotool key Return
shot distribution-3-regular-password 1.5
xdotool key ctrl+a; xdotool type --delay 15 "zzzz yyyy xxxx wwww"; xdotool key Return
shot distribution-4-wrong-app-password 3
xdotool key shift+Tab ctrl+a; xdotool type --delay 15 "fixture.admin@workspace.example"; xdotool key Tab ctrl+a
xdotool type --delay 15 "abcd efgh ijkl mnop"; xdotool key Return
shot distribution-5-workspace-imap-off 3
# A slow request (5 s): the same choice can't be resubmitted; others stay usable.
xdotool key shift+Tab ctrl+a; xdotool type --delay 15 "fixture.slow@workspace.example"; xdotool key Tab ctrl+a
xdotool type --delay 15 "abcd efgh ijkl mnop"; xdotool key Return
shot distribution-5a-slow-connecting 0.6
xdotool key ctrl+2; shot distribution-5b-slow-icloud-usable 0.4
xdotool key ctrl+1; shot distribution-5c-slow-still-connecting 0.4
xdotool key ctrl+2; sleep 5; shot distribution-5d-slow-finished-on-icloud 0.3
# Back on Gmail: usable again, and its failure is shown now.
xdotool key ctrl+1; shot distribution-5e-gmail-failure-shown-on-return 0.4
# A slow success while iCloud is on screen keeps the window and what is typed there.
xdotool key ctrl+a; xdotool type --delay 15 "fixture.later@workspace.example"; xdotool key Tab ctrl+a
xdotool type --delay 15 "abcd efgh ijkl mnop"; xdotool key Return; sleep 0.3
xdotool key ctrl+2; sleep 0.3; xdotool key ctrl+a; xdotool type --delay 15 "someone@icloud.example"; xdotool key Tab
xdotool type --delay 15 "half typed"
sleep 6
echo "dialog after background success: $(xdotool search --name 'Connect an account' 2>/dev/null | wc -l)"
shot distribution-5f-background-success-icloud-kept 0.3
xdotool key ctrl+1; shot distribution-5g-gmail-connected-on-return 0.4
# Focus is in the address field after switching back.
xdotool key ctrl+a; xdotool type --delay 15 "fixture.user@gmail.com"; xdotool key Tab
xdotool type --delay 15 "abcd efgh ijkl mnop"; xdotool key Return
sleep 6
echo "dialog after connect: $(xdotool search --name 'Connect an account' 2>/dev/null | wc -l)"
shot distribution-6-inbox 1
# Search the synced mail.
xdotool key slash; sleep 0.3; xdotool type --delay 20 "roadmap"; shot distribution-7-search 1.5
xdotool key Escape; sleep 0.5
# Escape in an open security list closes only the list; a second Escape closes the window.
xdotool key a; d=$(dialog); xdotool windowactivate --sync "$d"; sleep 0.5
xdotool key ctrl+3; sleep 0.5
for _ in 1 2 3 4 5 6; do xdotool key Tab; sleep 0.1; done
xdotool key space; shot distribution-8-security-list-open 0.8
xdotool key Escape; sleep 0.8
echo "dialog after Escape in list: $(xdotool search --name 'Connect an account' 2>/dev/null | wc -l)"
shot distribution-8a-security-list-closed 0.3
xdotool key Escape; sleep 0.8
echo "dialog after Escape: $(xdotool search --name 'Connect an account' 2>/dev/null | wc -l)"
pkill -f email-linux-distribution-qa || true; sleep 1

# Development (local-alpha) build: Gmail sign-in first, Outlook kept.
run development /out/email-linux-local-alpha-qa
shot development-1-gmail
xdotool key ctrl+2; shot development-2-app-password
xdotool key ctrl+4; shot development-3-outlook
pkill -f email-linux-local-alpha-qa || true
grep -E "^command|^delivered" $scratch/fixture.log | sed 's/^/fixture /' | head -80
echo "--- app logs (distribution)"; tail -20 $scratch/app-distribution.log
