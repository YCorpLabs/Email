#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$repo_root/scripts/distribution-profile.sh"
bundle_id="$(email_profile_value "$repo_root" MACOS_BUNDLE_ID)"
# The helper shares the app's Keychain namespace and data directory, both
# compiled from the distribution profile. Never build it for another identity.
app_plist="$repo_root/dist/Email.app/Contents/Info.plist"
if [[ -f "$app_plist" ]] && [[ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$app_plist" 2>/dev/null)" != "$bundle_id" ]]; then
  echo "dist/Email.app has a different bundle ID than the selected distribution profile ($bundle_id)." >&2
  echo "Select the matching profile in config/distribution-profile.local first." >&2
  exit 1
fi
identity_file="$repo_root/config/macos-signing-identity.local"
if [[ ! -f "$identity_file" ]]; then
  echo "Run scripts/setup-macos-signing.sh first." >&2
  exit 1
fi
signing_identity="$(cat "$identity_file")"
cd "$repo_root"
if command -v cargo >/dev/null 2>&1; then
  cargo_bin="$(command -v cargo)"
else
  cargo_bin="$HOME/.cargo/bin/cargo"
fi
"$cargo_bin" build -p mail-mcp --release

# Keep the same designated requirement, but do not mistake it for a shared
# development Keychain partition: self-signed binaries have different CDHashes.
# MCP stays noninteractive and cannot create migration destinations before Email.
# Production app/helper access still needs Developer ID partition acceptance.
codesign --force --sign "$signing_identity" --identifier "$bundle_id" \
  "$repo_root/target/release/mail-mcp"
codesign --verify --strict "$repo_root/target/release/mail-mcp"
echo "Built signed $repo_root/target/release/mail-mcp"
