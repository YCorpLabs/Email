#!/usr/bin/env bash
set -euo pipefail

# Contributor first run: build this platform's desktop app and open it with
# labelled demo messages in an isolated store. It needs no signing identity,
# OAuth configuration or mail account, and never opens the default mail store.
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

usage() {
  cat <<'EOF'
Usage: scripts/dev.sh [--demo [launch|search|small]] [--store PATH] [--no-demo]
                      [--messages N] [--release] [--smoke]

Builds the desktop app for this OS and starts it with three demo messages in
dist/demo/mail.sqlite. Nothing is signed and no account is needed.

  --demo [SET]  Start on a fresh temporary store with a demo set (see
                docs/demo-mailbox.md):
                  launch  the launch demo mailbox, about 300 conversations
                          (the default for --demo)
                  search  the launch demo plus 100,000 older conversations,
                          with an inbox search timing table
                  small   the three demo messages
                Also builds mail-mcp and writes mail.claude-mcp.json beside the
                store for the agent demo.
  --store PATH  Use another absolute database path. Account settings are kept
                beside it. Its directory is created if missing, and must be
                private to you (chmod 700) and not a symlink. With --demo,
                the path must not exist yet.
  --no-demo     Do not add demo messages. Combine with --store for an empty
                store, for example to try a test account.
  --messages N  With --demo search: how many older conversations (100000).
  --release     Build optimized binaries (slower to compile, faster to run).
  --smoke       macOS only: run the headless engine check, not the window.

On macOS this runs the unsigned SwiftPM build directly. scripts/run-macos.sh
builds the signed Email.app instead. See docs/building.md.
EOF
}

default_store="$repo_root/dist/demo/mail.sqlite"
store="$default_store"
store_given=false
demo=1
demo_set=""
messages=100000
profile=debug
smoke=false
while (($#)); do
  case "$1" in
    --store)
      (($# >= 2)) || { usage >&2; exit 2; }
      store="$2"; store_given=true; shift 2 ;;
    --demo)
      demo_set=launch; shift
      if (($#)) && [[ "$1" =~ ^(launch|search|small)$ ]]; then demo_set="$1"; shift; fi ;;
    --messages)
      (($# >= 2)) && [[ "$2" =~ ^[0-9]+$ ]] || { usage >&2; exit 2; }
      messages="$2"; shift 2 ;;
    --no-demo) demo=0; shift ;;
    --release) profile=release; shift ;;
    --smoke) smoke=true; shift ;;
    -h|--help) usage; exit 0 ;;
    *) usage >&2; exit 2 ;;
  esac
done
if [[ -n "$demo_set" ]]; then
  [[ "$demo" == 1 ]] || { echo "--demo and --no-demo cannot be combined." >&2; exit 2; }
  if [[ "$store_given" == true ]]; then
    [[ ! -e "$store" ]] || { echo "--demo needs a new store; $store already exists." >&2; exit 2; }
  else
    # A fresh private directory per run, outside the repository, so every
    # recording starts from the same mailbox with current dates.
    temp_root="${TMPDIR:-/tmp}"
    store="$(umask 077 && mktemp -d "${temp_root%/}/email-demo-$demo_set.XXXXXX")/mail.sqlite"
  fi
fi
[[ "$store" == /* ]] || { echo "--store needs an absolute path: $store" >&2; exit 2; }
[[ "$smoke" == false || "$(uname -s)" == Darwin ]] || {
  echo "--smoke is only available on macOS." >&2; exit 2; }

if command -v cargo >/dev/null 2>&1; then
  cargo_bin="$(command -v cargo)"
elif [[ -x "$HOME/.cargo/bin/cargo" ]]; then
  cargo_bin="$HOME/.cargo/bin/cargo"
else
  echo "cargo was not found. Install Rust with rustup (https://rustup.rs)." >&2
  exit 127
fi
cargo_args=(--locked)
if [[ "$profile" == release ]]; then cargo_args+=(--release); fi

# Honors CARGO_TARGET_DIR and build.target-dir in Cargo configuration.
target_directory() {
  "$cargo_bin" metadata --format-version 1 --no-deps --locked "$@" \
    | python3 -c 'import json, sys; print(json.load(sys.stdin)["target_directory"])'
}

cd "$repo_root"
# The distribution profile names the app identity and services (see
# docs/distribution-profile.md). A fresh clone selects nothing and builds the
# neutral `unofficial` profile; Cargo resolves the same file.
source scripts/distribution-profile.sh
email_load_distribution_profile "$repo_root"
email_refuse_distribution_profile
build_identity="$(python3 scripts/build-identity.py)"
store_dir="$(dirname "$store")"
(umask 077 && mkdir -p "$store_dir")
# The same rule as the Linux app, on both platforms: a private, real directory.
if [[ "$store" == "$default_store" ]]; then chmod 700 "$store_dir"; fi
if [[ -L "$store_dir" ]] || ! python3 -c 'import os, sys; sys.exit(os.stat(sys.argv[1]).st_mode & 0o077 != 0)' "$store_dir"; then
  echo "The store directory $store_dir must be private to you (chmod 700) and not a symlink." >&2
  exit 2
fi
# Never point an unsigned development build at any profile's real store: not
# its directory, and not a symlink or hard link to its database.
if [[ -L "$store" ]]; then
  echo "--store must be a separate database file, not a symlink." >&2
  exit 2
fi
if [[ "$(uname -s)" == Darwin ]]; then data_key=MACOS_DATA_DIRECTORY; else data_key=LINUX_DATA_DIRECTORY; fi
# The selected profile (which may be a fork's file elsewhere) and every profile here.
for profile_file in "$EMAIL_DISTRIBUTION_PROFILE" config/profiles/*.env; do
  name="$(EMAIL_DISTRIBUTION_PROFILE="$profile_file" email_profile_value "$repo_root" "$data_key")"
  if [[ "$(uname -s)" == Darwin ]]; then
    real_store_dir="$HOME/Library/Application Support/$name"
  else
    real_store_dir="${XDG_DATA_HOME:-$HOME/.local/share}/$name"
  fi
  [[ -d "$real_store_dir" ]] || continue
  if python3 -c 'import os, sys; sys.exit(not os.path.samefile(sys.argv[1], sys.argv[2]))' "$store_dir" "$real_store_dir" \
    || { [[ -e "$store" && -e "$real_store_dir/mail.sqlite" ]] \
      && python3 -c 'import os, sys; sys.exit(not os.path.samefile(sys.argv[1], sys.argv[2]))' "$store" "$real_store_dir/mail.sqlite"; }; then
    echo "--store must not use the real mail store of the $(basename "$profile_file" .env) profile ($real_store_dir). Choose a separate directory." >&2
    exit 2
  fi
done
export EMAIL_STORE_PATH="$store"
case "$demo_set" in
  launch|search) export EMAIL_DEMO=launch ;;
  small) export EMAIL_DEMO=1 ;;
  *) if [[ "$demo" == 1 ]]; then export EMAIL_DEMO=1; else unset EMAIL_DEMO; fi ;;
esac

# --demo: build the agent server too, create the older history for `search`,
# and leave an MCP configuration for the agent demo beside the store.
prepare_demo_store() {
  [[ -n "$demo_set" ]] || return 0
  "$cargo_bin" build -p mail-mcp "${cargo_args[@]}"
  local target_dir mcp offset
  target_dir="$(target_directory)"
  mcp="$target_dir/$profile/mail-mcp"
  if [[ "$demo_set" == search ]]; then
    # Minutes east of UTC, so "today" in the demo is today where you are.
    offset="$(python3 -c 'import datetime; print(int(datetime.datetime.now().astimezone().utcoffset().total_seconds() // 60))')"
    echo "Building the search demo store (launch demo + $messages older conversations)…"
    "$cargo_bin" run -p mail-service --example demo_store --release --locked -- \
      --store "$store" --messages "$messages" --utc-offset-minutes "$offset"
  fi
  # Beside the store and named after it; an existing file is never replaced.
  local config="${store%.*}.claude-mcp.json"
  printf '\nDemo store: %s\n' "$store"
  if python3 - "$mcp" "$store" "$config" <<'PY'
import json, os, sys
server, store, path = sys.argv[1:]
config = {"mcpServers": {"email-demo": {"command": server, "args": ["--db", store, "--allow-draft", "--allow-triage"]}}}
try:
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
except FileExistsError:
    sys.exit(f"{path} already exists and was left unchanged; no agent configuration was written.")
with os.fdopen(fd, "w") as handle:
    json.dump(config, handle, indent=2)
PY
  then
    printf 'Agent demo: in Email, open Agent access and turn it on for both demo accounts\n'
    printf '(Drafts and Triage), then in another terminal:\n'
    printf '  claude --mcp-config "%s"\n' "$config"
  fi
  if [[ "$store_given" == true ]]; then
    printf 'Delete the demo afterwards: the store and the files beside it named %s.*\n\n' "$(basename "${store%.*}")"
  else
    printf 'Delete the store afterwards with: rm -r "%s"\n\n' "$store_dir"
  fi
}

case "$(uname -s)" in
  Darwin)
    "$cargo_bin" build -p mail-service "${cargo_args[@]}"
    # Generated, ignored by Git, and required by the Swift build. Rewrite it only
    # when it changes, so SwiftPM does not recompile EmailUI on every run.
    identity_file=apps/macos/Sources/EmailUI/BuildIdentity.generated.swift
    identity_source="$(printf 'import Foundation\nenum EmailBuildIdentity { static let value = "%s" }' "$build_identity")"
    if [[ ! -f "$identity_file" || "$(cat "$identity_file")" != "$identity_source" ]]; then
      printf '%s\n' "$identity_source" > "$identity_file"
    fi
    swift_args=(--package-path apps/macos --disable-sandbox --only-use-versions-from-resolved-file)
    if [[ "$profile" == release ]]; then swift_args+=(-c release); fi
    swift build "${swift_args[@]}"
    bin_dir="$(swift build "${swift_args[@]}" --show-bin-path)"
    target_dir="$(target_directory)"
    export MAIL_SERVICE_LIBRARY="$target_dir/$profile/libmail_service.dylib"
    prepare_demo_store
    if [[ "$smoke" == true ]]; then exec "$bin_dir/email-desktop" --smoke-service; fi
    echo "Starting Email with store $store (close the window or press Ctrl+C here to quit)."
    exec "$bin_dir/email-desktop"
    ;;
  Linux)
    export EMAIL_BUILD_ID="$build_identity"
    # The Linux shell compiles its OAuth application values in; profiles such as
    # development require the Google client from config/oauth-desktop.local.env.
    export EMAIL_GOOGLE_CLIENT_ID EMAIL_GOOGLE_CLIENT_SECRET EMAIL_MICROSOFT_CLIENT_ID
    "$cargo_bin" build --manifest-path apps/linux/Cargo.toml "${cargo_args[@]}"
    prepare_demo_store
    echo "Starting Email with store $store (close the window or press Ctrl+C here to quit)."
    target_dir="$(target_directory --manifest-path apps/linux/Cargo.toml)"
    exec "$target_dir/$profile/email-linux"
    ;;
  *)
    echo "scripts/dev.sh supports macOS and Linux." >&2
    exit 1 ;;
esac
