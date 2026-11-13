#!/usr/bin/env bash
set -euo pipefail

# A persistent local identity keeps macOS Keychain's app requirement stable
# between development builds. This certificate is for this Mac only; release
# builds must use a Developer ID identity.
identity_name="Email Local Development Signing"
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
identity_file="$repo_root/config/macos-signing-identity.local"
if [[ -f "$identity_file" ]]; then
  pinned_identity="$(cat "$identity_file")"
else
  pinned_identity=""
fi
identity_list="$(security find-identity -p codesigning)"
existing="$(printf '%s\n' "$identity_list" | awk -v name="$identity_name" -v pinned="$pinned_identity" 'index($0, "\"" name "\"") && (pinned == "" || $2 == pinned) { print $2; exit }')"
if [[ -n "$existing" ]]; then
  printf '%s\n' "$existing" > "$identity_file"
  echo "$existing"
  exit 0
fi

umask 077
temp_dir="$(mktemp -d)"
trap 'rm -rf "$temp_dir"' EXIT

openssl req -x509 -newkey rsa:3072 -nodes -days 3650 \
  -subj "/CN=$identity_name" \
  -addext 'basicConstraints=critical,CA:FALSE' \
  -addext 'keyUsage=critical,digitalSignature' \
  -addext 'extendedKeyUsage=codeSigning' \
  -keyout "$temp_dir/key.pem" -out "$temp_dir/cert.pem" 2>/dev/null
identity="$(openssl x509 -in "$temp_dir/cert.pem" -noout -fingerprint -sha1 | cut -d= -f2 | tr -d ':')"
bundle_password="$(openssl rand -hex 24)"
openssl pkcs12 -export -legacy \
  -inkey "$temp_dir/key.pem" -in "$temp_dir/cert.pem" \
  -out "$temp_dir/identity.p12" -passout "pass:$bundle_password"
security import "$temp_dir/identity.p12" \
  -k "$HOME/Library/Keychains/login.keychain-db" \
  -f pkcs12 -P "$bundle_password" -T /usr/bin/codesign

if ! security find-identity -p codesigning | grep -Fq "$identity"; then
  echo "The local code signing identity was not accepted by Keychain." >&2
  exit 1
fi
printf '%s\n' "$identity" > "$identity_file"
echo "In Keychain Access, rename this certificate's private key to 'Email Local Development Signing Key', then run scripts/authorize-macos-signing.sh in your own Terminal once." >&2
echo "$identity"
