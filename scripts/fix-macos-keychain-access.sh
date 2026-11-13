#!/usr/bin/env bash
set -euo pipefail

# Run once in Terminal for existing development Keychain items. The login
# Keychain asks for its password locally; this script never reads or stores it.
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$repo_root/scripts/distribution-profile.sh"
app="$repo_root/dist/Email.app"
identity_file="$repo_root/config/macos-signing-identity.local"
accounts_file="$HOME/Library/Application Support/$(email_profile_value "$repo_root" MACOS_DATA_DIRECTORY)/mail.accounts.json"
# Historical v2 mail items, named from the profile's Keychain namespace.
keychain_service="$(email_profile_value "$repo_root" KEYCHAIN_NAMESPACE).v2"
keychain="$HOME/Library/Keychains/login.keychain-db"

if [[ ! -f "$identity_file" || ! -f "$accounts_file" ]]; then
  echo "Email's signing identity or account configuration is missing." >&2
  exit 1
fi
identity="$(tr '[:upper:]' '[:lower:]' < "$identity_file")"
if [[ ! "$identity" =~ ^[0-9a-f]{40}$ ]]; then
  echo "Email's signing identity is invalid." >&2
  exit 1
fi
codesign --verify --deep --strict "$app"
if ! codesign -dr - "$app" 2>&1 | grep -Fq "certificate leaf = H\"$identity\""; then
  echo "The packaged app does not match Email's signing identity." >&2
  exit 1
fi

umask 077
accounts="$(mktemp)"
trap 'rm -f "$accounts"' EXIT
python3 - "$accounts_file" > "$accounts" <<'PY'
import json, sys
with open(sys.argv[1], encoding="utf-8") as file:
    configs = json.load(file)
for config in configs:
    account_id = config.get("account_id")
    if isinstance(account_id, str) and account_id and "\n" not in account_id:
        print(account_id)
PY
if [[ ! -s "$accounts" ]]; then
  echo "No Email account needs a Keychain access update."
  exit 0
fi

requirement="req:identifier \"$(email_profile_value "$repo_root" MACOS_BUNDLE_ID)\" and certificate leaf = H\"$identity\""
echo "Updating Email's existing Keychain access. Enter your Mac login Keychain password in Terminal when asked."
count=0
while IFS= read -r account_id; do
  /usr/bin/security set-generic-password-partition-list \
    -a "$account_id" -s "$keychain_service" -S "$requirement" "$keychain" \
    < /dev/tty > /dev/null
  count=$((count + 1))
done < "$accounts"
echo "Attempted to update $count Email account(s). macOS may still pin a self-signed development build to its exact code hash."
echo "Checking the resulting access-control metadata without reading credentials:"
"$repo_root/scripts/diagnose-macos-keychain.sh"
