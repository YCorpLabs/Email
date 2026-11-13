#!/usr/bin/env bash
set -euo pipefail

# Read access-control metadata only. Never ask Keychain to reveal a password.
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$repo_root/scripts/distribution-profile.sh"
app="$repo_root/dist/Email.app"
accounts_file="$HOME/Library/Application Support/$(email_profile_value "$repo_root" MACOS_DATA_DIRECTORY)/mail.accounts.json"
# Historical v2 mail items, named from the profile's Keychain namespace.
keychain_service="$(email_profile_value "$repo_root" KEYCHAIN_NAMESPACE).v2"
keychain="$HOME/Library/Keychains/login.keychain-db"

if [[ ! -d "$app" || ! -f "$accounts_file" ]]; then
  echo "Email.app or its account configuration is missing." >&2
  exit 1
fi

code_hash="$(codesign -dv --verbose=4 "$app" 2>&1 | sed -n 's/^CDHash=//p' | head -1)"
if [[ ! "$code_hash" =~ ^[0-9a-fA-F]{40}$ ]]; then
  echo "Could not read Email's signed code hash." >&2
  exit 1
fi

# dump-keychain -a includes ACLs but omits secret values. Keep the full output
# in a private temporary file and report only aggregate results.
umask 077
acl_dump="$(mktemp)"
trap 'rm -f "$acl_dump"' EXIT
security dump-keychain -a "$keychain" > "$acl_dump"
python3 - "$accounts_file" "$code_hash" "$acl_dump" "$keychain_service" <<'PY'
import json
import re
import sys

accounts_path, current_hash, dump_path, service = sys.argv[1:]
with open(accounts_path, encoding="utf-8") as file:
    accounts = {
        item.get("account_id") for item in json.load(file)
        if isinstance(item.get("account_id"), str)
    }

with open(dump_path, encoding="utf-8") as file:
    dump = file.read()
found = current = stale = 0
for item in re.split(r"(?=^keychain: )", dump, flags=re.MULTILINE):
    if 'class: "genp"' not in item or f'"svce"<blob>="{service}"' not in item:
        continue
    account = re.search(r'^\s*"acct"<blob>="([^"]+)"', item, re.MULTILINE)
    if not account or account.group(1) not in accounts:
        continue
    found += 1
    partitions = re.findall(
        r'authorizations \(1\): partition_id\s+don.t-require-password\s+description: ([^\n]+)',
        item,
    )
    if f"cdhash:{current_hash.lower()}" in ",".join(partitions).lower():
        current += 1
    elif any("cdhash:" in partition for partition in partitions):
        stale += 1

print(f"Email signing hash: {current_hash.lower()}")
print(f"Account Keychain items found: {found}")
print(f"Items permitting this exact app hash: {current}")
print(f"Items pinned to another app hash: {stale}")
if stale:
    print("A macOS Keychain grant may be needed after this signed build changed.")
PY
