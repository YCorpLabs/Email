#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
server="$repo_root/target/release/mail-mcp"
client=both
allow_write=false
rebuild=false
dry_run=false
custom_name=""
grants=()
accounts=()

# Registration names and the default cache follow the distribution profile.
source "$repo_root/scripts/distribution-profile.sh"
base_name="$(email_profile_value "$repo_root" MCP_SERVER_NAME)"
if [[ "$(uname -s)" == Darwin ]]; then
  db="${EMAIL_STORE_PATH:-$HOME/Library/Application Support/$(email_profile_value "$repo_root" MACOS_DATA_DIRECTORY)/mail.sqlite}"
else
  db="${EMAIL_STORE_PATH:-${XDG_DATA_HOME:-$HOME/.local/share}/$(email_profile_value "$repo_root" LINUX_DATA_DIRECTORY)/mail.sqlite}"
fi

usage() {
  cat <<'EOF'
Usage: scripts/connect-mcp.sh [--codex | --claude | --both] [--db PATH] [--build]
       [--allow-draft] [--allow-triage] [--allow-sync] [--allow-send | --allow-write]
       [--allow-booking-settings] [--account ID ...] [--name NAME] [--dry-run]

Registers Email's local MCP server in installed clients. The default server is
read-only and is named by the distribution profile's MCP_SERVER_NAME ("email"
for official builds, "email-unofficial" otherwise). --allow-write registers a
separate "<name>-write" server that can change and send real mail; use it only
when you want to grant that access. Existing registrations are left alone. Use
each client's "mcp get" command to inspect one before changing it.
Independent grants register as "<name>-custom"; account-only restrictions use
"<name>-scoped". --name selects a distinct registration. --dry-run prints the
planned commands without building or changing client settings.
Scheduling settings require --allow-booking-settings separately; --allow-write
does not grant it. Browser approval and a reviewed configuration plan are also
required before an agent can apply scheduling settings. No mail is sent by it.
EOF
}

while (($#)); do
  case "$1" in
    --codex) client=codex; shift ;;
    --claude) client=claude; shift ;;
    --both) client=both; shift ;;
    --db)
      (($# >= 2)) || { usage >&2; exit 2; }
      db="$2"; shift 2 ;;
    --build) rebuild=true; shift ;;
    --allow-write) allow_write=true; shift ;;
    --allow-draft|--allow-triage|--allow-sync|--allow-send|--allow-booking-settings) grants+=("$1"); shift ;;
    --account)
      (($# >= 2)) && [[ -n "$2" && "$2" != -* ]] || { usage >&2; exit 2; }
      accounts+=(--account "$2"); shift 2 ;;
    --name)
      (($# >= 2)) && [[ "$2" =~ ^[a-zA-Z0-9][a-zA-Z0-9_-]*$ ]] || { usage >&2; exit 2; }
      custom_name="$2"; shift 2 ;;
    --dry-run) dry_run=true; shift ;;
    -h|--help) usage; exit 0 ;;
    *) usage >&2; exit 2 ;;
  esac
done

[[ "$db" == /* ]] || { echo "Database path must be absolute: $db" >&2; exit 2; }
[[ -f "$db" ]] || { echo "Mail cache not found: $db. Open Email and connect an account first." >&2; exit 1; }

# The helper's Keychain namespace is compiled in; rebuild one made for another profile.
namespace="$(email_profile_value "$repo_root" KEYCHAIN_NAMESPACE)"
if [[ -x "$server" ]] && ! LC_ALL=C grep -aqF "$namespace.v" "$server"; then
  echo "The existing MCP server was built for another distribution profile; rebuilding."
  rebuild=true
fi
if [[ "$dry_run" != true ]] && [[ "$rebuild" == true || ! -x "$server" ]]; then
  if [[ "$(uname -s)" == Darwin ]]; then
    "$repo_root/scripts/build-mcp-macos.sh"
  else
    cargo_bin="$(command -v cargo || true)"
    [[ -n "$cargo_bin" ]] || cargo_bin="$HOME/.cargo/bin/cargo"
    "$cargo_bin" build --manifest-path "$repo_root/Cargo.toml" -p mail-mcp --release
  fi
fi
[[ "$dry_run" == true || -x "$server" ]] || { echo "MCP server not found: $server" >&2; exit 1; }

name="$base_name"
args=(--db "$db")
if [[ "$allow_write" == true ]]; then
  name="$base_name-write"
  args+=(--allow-write)
fi
if ((${#grants[@]})); then
  [[ "$allow_write" == true ]] || name="$base_name-custom"
  args+=("${grants[@]}")
fi
if ((${#accounts[@]})); then
  [[ "$name" != "$base_name" ]] || name="$base_name-scoped"
  args+=("${accounts[@]}")
fi
[[ -z "$custom_name" ]] || name="$custom_name"

if [[ "$dry_run" == true ]]; then
  if [[ "$client" == both || "$client" == codex ]]; then
    printf '%q ' codex mcp add "$name" -- "$server" "${args[@]}"; printf '\n'
  fi
  if [[ "$client" == both || "$client" == claude ]]; then
    printf '%q ' claude mcp add --scope user --transport stdio "$name" -- "$server" "${args[@]}"; printf '\n'
  fi
  exit 0
fi

if [[ "$client" == both || "$client" == codex ]]; then
  if command -v codex >/dev/null 2>&1; then
    if codex mcp get "$name" >/dev/null 2>&1; then
      echo "Codex: $name is already registered; inspect it with: codex mcp get $name"
    else
      codex mcp add "$name" -- "$server" "${args[@]}"
      echo "Codex: registered $name"
    fi
  else
    echo "Codex CLI is not installed; skipped."
  fi
fi

if [[ "$client" == both || "$client" == claude ]]; then
  if command -v claude >/dev/null 2>&1; then
    if claude mcp get "$name" >/dev/null 2>&1; then
      echo "Claude Code: $name is already registered; inspect it with: claude mcp get $name"
    else
      claude mcp add --scope user --transport stdio "$name" -- "$server" "${args[@]}"
      echo "Claude Code: registered $name"
    fi
  else
    echo "Claude Code CLI is not installed; skipped."
  fi
fi

echo "Server: $server"
echo "Cache: $db"
if [[ "$allow_write" == true ]]; then
  echo "Access: read and write, including sending mail"
else
  if ((${#grants[@]})); then
    printf 'Access: cached reads plus'; printf ' %s' "${grants[@]}"; printf '\n'
  else
    echo "Access: read-only"
  fi
fi
if ((${#accounts[@]})); then
  printf 'Account restrictions:'; printf ' %s' "${accounts[@]}"; printf '\n'
fi
