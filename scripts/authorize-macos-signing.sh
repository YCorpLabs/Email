#!/usr/bin/env bash
set -euo pipefail

# Run interactively in Terminal once after setup-macos-signing.sh. macOS asks
# for the login Keychain password locally; do not put it in chat or a file.
if ! security dump-keychain "$HOME/Library/Keychains/login.keychain-db" 2>/dev/null \
  | grep -Fq '"Email Local Development Signing Key"'; then
  echo "Rename Email's imported private key to 'Email Local Development Signing Key' in Keychain Access first." >&2
  exit 1
fi
security set-key-partition-list \
  -S apple: -s -t private -l 'Email Local Development Signing Key' \
  "$HOME/Library/Keychains/login.keychain-db"
