#!/usr/bin/env bash
# Linux GTK launch-demo QA on a virtual X display in a disposable container:
# opens the launch demo mailbox (EMAIL_DEMO=launch) in a fresh isolated store
# and captures the states used in the launch recordings. Synthetic data only;
# no account, keyring or provider. Usage (from the repository root, with a
# Linux build of the shell in ./qa-out/email-linux):
#
#   docker run --rm -v "$PWD/qa-out:/out:ro" -v "$PWD/qa-shots:/shots" \
#     -v "$PWD/apps/linux/qa/launch-demo.sh:/qa.sh:ro" \
#     email-linux-verify:local bash /qa.sh
#
# See docs/demo-mailbox.md.
set -euo pipefail
# Disposable scratch space; the demo user must be able to reach it.
scratch="$(mktemp -d)"
chmod 755 "$scratch"
apt-get update -qq >/dev/null
apt-get install -y -qq --no-install-recommends xvfb openbox xdotool imagemagick dbus dbus-x11 \
  fonts-inter fonts-dejavu-core fonts-noto-color-emoji libgl1-mesa-dri tzdata sqlite3 >/dev/null 2>&1
Xvfb :99 -screen 0 1440x900x24 -nolisten tcp >$scratch/xvfb.log 2>&1 &
sleep 1
DISPLAY=:99 openbox >$scratch/openbox.log 2>&1 &
sleep 1
export DISPLAY=:99 GDK_BACKEND=x11 GSK_RENDERER=cairo LIBGL_ALWAYS_SOFTWARE=1 NO_AT_BRIDGE=1 EMAIL_SOFTWARE_RENDERING=1
# Container QA only: Docker's seccomp profile blocks WebKit's namespace sandbox.
export WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS=1
export TZ=Europe/Oslo

useradd -m demo 2>/dev/null || true
base=$scratch/launch-demo
mkdir -p "$base"
chown demo "$base"
store="$base/store/mail.sqlite"
shot() { sleep "${2:-1.5}"; import -window root "/shots/linux-$1.png"; echo "shot $1"; }
# XTEST input to the focused window; GTK 4 ignores events sent to a window.
keys() { xdotool key --clearmodifiers "$@"; sleep 0.6; }
type_text() { xdotool type --delay 30 "$1"; sleep 0.4; }

su demo -c "env DISPLAY=$DISPLAY GDK_BACKEND=x11 GSK_RENDERER=cairo LIBGL_ALWAYS_SOFTWARE=1 NO_AT_BRIDGE=1 \
  EMAIL_SOFTWARE_RENDERING=1 WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS=1 TZ=$TZ HOME=$base \
  XDG_DATA_HOME=$base/data EMAIL_STORE_PATH=$store EMAIL_DEMO=launch \
  dbus-run-session -- /out/email-linux" >"$base/app.log" 2>&1 &
window=""
for _ in $(seq 100); do
  window=$(xdotool search --onlyvisible --name "^Email" 2>/dev/null | tail -1 || true)
  [[ -n "$window" ]] && break
  sleep 0.2
done
[[ -n "$window" ]] || { echo "Email window did not open"; cat "$base/app.log"; exit 1; }
xdotool windowsize "$window" 1440 900 windowmove "$window" 0 0
xdotool windowactivate --sync "$window" 2>/dev/null || true
sleep 4

echo "running executable: $(sha256sum /out/email-linux | cut -d' ' -f1)"
# The app runs as demo; root in a container may not read its /proc entry.
for pid in $(pgrep -x email-linux); do
  echo "process $pid exe: $(su demo -c "sha256sum /proc/$pid/exe" | cut -d' ' -f1)"
done
echo "store: $(sqlite3 "$store" "select count(*) from threads where id like 'demo:launch:%'") demo conversations," \
  "$(sqlite3 "$store" "select count(*) from threads where is_inbox=1") in the inbox"

shot 01-inbox 2
keys j j j
shot 02-thread
keys slash
type_text "Designing for the second visit"
keys Return
shot 03-newsletter-blocked-images 2.5
keys slash ctrl+a
type_text "Invitation go/no-go"
keys Return
shot 04-invite 2
keys slash ctrl+a
type_text "hytta"
keys Return
shot 05-norwegian-photos 2.5
keys slash ctrl+a
type_text "invoice"
keys Return
shot 06-search-invoice 2
keys slash ctrl+a
type_text "Skip for now"
keys Return
shot 07-inline-picture 2.5
keys slash Escape
keys g s
shot 08-snoozed
xdotool windowsize "$window" 980 700
keys g i
shot 09-narrow 2
echo "app log:"; tail -20 "$base/app.log"
