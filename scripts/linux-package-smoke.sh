#!/usr/bin/env bash
# Runs inside a disposable container after email-linux has been installed by the
# distro package manager. Synthetic demo data only: no account, credential,
# provider request or network service is used.
#
# Usage: linux-package-smoke.sh MCP_BINARY RESULTS_DIR ARCH
# ARCH is aarch64 or x86_64. Writes logs and screenshot.png to RESULTS_DIR.
# Demo data goes only into a disposable EMAIL_STORE_PATH store, which the app
# and mail-mcp both honor; neither seeds the default mail store.
# EMAIL_SMOKE_EMULATED=1 marks a run under user-mode CPU emulation.
set -euo pipefail
mcp="$1" results="$2" arch="$3"
case "$arch" in
  aarch64) machine='ARM aarch64' ;;
  x86_64) machine='x86-64' ;;
  *) echo "Unsupported architecture: $arch" >&2; exit 2 ;;
esac
mkdir -p "$results"
fail() { echo "SMOKE FAILED: $*" | tee -a "$results/summary.txt" >&2; exit 1; }
pass() { echo "PASS: $*" | tee -a "$results/summary.txt"; }

# 1. Installed binaries are ELF for the expected machine, and every shared
# library they need resolves from the distro's packages.
for binary in /usr/bin/email-linux "$mcp"; do
  description="$(file -L "$binary")"
  echo "$description" >> "$results/file.txt"
  [[ "$description" == *"ELF 64-bit"*"$machine"* ]] || fail "$binary is not a $machine ELF: $description"
  ldd "$binary" > "$results/ldd-$(basename "$binary").txt" 2>&1 || true
  ! grep -q 'not found' "$results/ldd-$(basename "$binary").txt" || fail "$binary has unresolved libraries"
done
pass "email-linux and $(basename "$mcp") are $machine ELF; all shared libraries resolve"

scratch="$(mktemp -d)"
export HOME="$scratch/user-home" XDG_DATA_HOME="$scratch/user-home/.local/share" XDG_RUNTIME_DIR="$scratch/runtime"
mkdir -p "$XDG_DATA_HOME" "$XDG_RUNTIME_DIR" "$scratch/store"
chmod 700 "$HOME" "$XDG_RUNTIME_DIR" "$scratch/store"
export EMAIL_STORE_PATH="$scratch/store/mail.sqlite"
db="$EMAIL_STORE_PATH"

# 2. The MCP server seeds synthetic demo mail and answers JSON-RPC over stdio.
# The seeded database is then also the app's.
"$mcp" --help > "$results/mcp-help.txt" 2>&1 || fail "mail-mcp --help exited non-zero"
# Talk to the server over named pipes (no coproc, so bash 3.2 can parse this).
pipes="$(mktemp -d)"
mkfifo "$pipes/in" "$pipes/out"
"$mcp" --db "$db" --seed-demo < "$pipes/in" > "$pipes/out" 2> "$results/mcp-stderr.txt" &
mcp_pid=$!
exec 3> "$pipes/in" 4< "$pipes/out"
send() { printf '%s\n' "$1" >&3; }
receive() {
  local line
  IFS= read -r -t 180 line <&4 || fail "mail-mcp did not answer"
  printf '%s\n' "$line"
}
send '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"package-smoke","version":"1"}}}'
receive > "$results/mcp-initialize.json"
send '{"jsonrpc":"2.0","method":"notifications/initialized"}'
send '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}'
receive > "$results/mcp-tools-list.json"
send '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"list_threads","arguments":{}}}'
receive > "$results/mcp-list-threads.json"
exec 3>&-
wait "$mcp_pid" || true
exec 4<&-
grep -q '"serverInfo"' "$results/mcp-initialize.json" || fail "initialize returned no serverInfo"
grep -q '"list_threads"' "$results/mcp-tools-list.json" || fail "tools/list did not list list_threads"
# Mail reads need agent access turned on in the app for the account. This
# synthetic profile has none, so the documented refusal is the expected answer;
# builds without that gate return the demo threads.
if grep -q 'Design review on Friday' "$results/mcp-list-threads.json"; then
  answer="returned the seeded demo threads"
elif grep -q 'agent access' "$results/mcp-list-threads.json"; then
  answer="refused with the in-app agent access message (no consent in this synthetic profile)"
else
  fail "list_threads returned neither the demo threads nor the agent access refusal"
fi
pass "mail-mcp answered initialize and tools/list over stdio; list_threads $answer"

# Give the newest demo message the synthetic HTML fixture used in the VM checks
# (heading, bold text, table) so its reader goes through WebKitGTK.
sqlite3 "$db" "UPDATE messages SET body_html='<h1>Synthetic HTML fixture</h1><p><b>Bold text</b> rendered by WebKitGTK.</p><table border=\"1\"><tr><th>Column</th><th>Value</th></tr><tr><td>Architecture</td><td>$arch</td></tr></table>' WHERE id='demo:1'"

# 3. The installed app starts on an X display, stays up and draws its window.
# No GPU exists here, so WebKit uses the app's documented software option.
Xvfb :99 -screen 0 1440x900x24 -nolisten tcp > "$results/xvfb.log" 2>&1 &
xvfb_pid=$!
export DISPLAY=:99 GDK_BACKEND=x11 EMAIL_SOFTWARE_RENDERING=1 NO_AT_BRIDGE=1
for _ in $(seq 1 50); do xdpyinfo > /dev/null 2>&1 && break; sleep 0.2; done
xdpyinfo > /dev/null 2>&1 || fail "Xvfb did not start"
sandbox=enabled
dbus-run-session -- /usr/bin/email-linux > "$results/app.log" 2>&1 &
app_pid=$!
sleep "${EMAIL_SMOKE_STARTUP_SECONDS:-45}"
if ! kill -0 "$app_pid" 2>/dev/null && [[ "${EMAIL_SMOKE_EMULATED:-0}" == 1 ]] &&
    grep -q '^bwrap: ' "$results/app.log"; then
  # User-mode CPU emulation (Rosetta/QEMU) runs this foreign-architecture
  # container on another architecture's kernel. Bubblewrap then fails: that
  # kernel rejects an x86_64 seccomp filter, and newer mount syscalls can be
  # unimplemented by the emulator. Record the error, then retry once with
  # WebKit's sandbox off in this disposable container only. Real hardware of
  # this architecture is needed to check the sandbox.
  mv "$results/app.log" "$results/app-sandboxed.log"
  echo "NOTE: emulated $arch: WebKit's bubblewrap sandbox did not start ($(grep -m1 '^bwrap: ' "$results/app-sandboxed.log")); retrying with WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS=1" | tee -a "$results/summary.txt"
  sandbox="disabled (emulation)"
  WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS=1 dbus-run-session -- /usr/bin/email-linux > "$results/app.log" 2>&1 &
  app_pid=$!
  sleep "${EMAIL_SMOKE_STARTUP_SECONDS:-45}"
fi
kill -0 "$app_pid" 2>/dev/null || fail "email-linux exited during startup (see app.log)"
# The app must have opened the seeded store, not the default one.
other="$(find "$XDG_DATA_HOME" -name mail.sqlite | head -1)"
[[ -z "$other" ]] || fail "email-linux opened $other, not the seeded $db"
window="$(xdotool search --onlyvisible --class email 2>/dev/null | head -1 || true)"
[[ -n "$window" ]] || window="$(xdotool search --onlyvisible --name . 2>/dev/null | head -1 || true)"
[[ -n "$window" ]] || fail "no visible window"
xdotool getwindowname "$window" > "$results/window-name.txt" 2>&1 || true
xdotool getwindowgeometry "$window" > "$results/window-geometry.txt" 2>&1 || true
import -window root "$results/screenshot.png"
# Open the newest demo message in the reader, then capture again.
xdotool windowfocus --sync "$window" 2>/dev/null || true
xdotool key --window "$window" Return 2>/dev/null || true
sleep 10
import -window root "$results/screenshot-reader.png"
kill -0 "$app_pid" 2>/dev/null || fail "email-linux exited after opening a message (see app.log)"
ps -eo pid,args | grep -E '[W]ebKit(Web|Network)Process|[b]wrap' > "$results/webkit-processes.txt" || true
grep -q '[W]ebKitWebProcess' "$results/webkit-processes.txt" || fail "no WebKitWebProcess is running for the HTML message"
! grep -E -q 'panicked at|Segmentation fault|core dumped' "$results/app.log" || fail "app.log contains a crash"
pass "email-linux opened the seeded data, stayed up, drew its window and has a WebKit web process; WebKit sandbox $sandbox (screenshots captured)"
kill "$app_pid" "$xvfb_pid" 2>/dev/null || true
wait 2>/dev/null || true
