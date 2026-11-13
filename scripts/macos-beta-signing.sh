#!/usr/bin/env bash
# Self-signed code signing for unnotarized macOS betas
# (scripts/package-macos-release.sh --beta), used until the project has an
# Apple Developer ID.
#
#   scripts/macos-beta-signing.sh create   creates the identity, once
#   scripts/macos-beta-signing.sh status   prints its keychain and certificate SHA-1
#
# The identity lives in its own keychain file outside the repository, with the
# keychain password in a sibling file. Both are mode 600, in
# EMAIL_BETA_SIGNING_DIR (default ~/.config/ycorp-release). The keychain is
# never added to the keychain search list or made the default, and the login
# keychain is not touched: codesign reads the file through --keychain and a
# private search list (email_codesign_home in macos-updates-env.sh). The
# password reaches `security` on standard input, never as an argument.
#
# Reuse the same identity for every beta. Keychain items that a beta creates
# trust this certificate, so a new certificate makes macOS ask testers again
# for each account. Back up both files together, outside source control.
#
# Sourced by package-macos-release.sh for email_beta_signing_check,
# email_beta_signing_unlock and email_beta_signing_lock. Never enable shell
# tracing around this file.

EMAIL_BETA_SIGNING_NAME=email-macos-beta-signing
EMAIL_BETA_SIGNING_SUBJECT="/CN=Email by YCorp beta (self-signed)"

_email_beta_paths() {
  local dir="${EMAIL_BETA_SIGNING_DIR:-$HOME/.config/ycorp-release}"
  email_beta_keychain="$dir/$EMAIL_BETA_SIGNING_NAME.keychain-db"
  email_beta_password_file="$dir/$EMAIL_BETA_SIGNING_NAME.password"
  # `security -i` splits its input on whitespace.
  if [[ "$email_beta_keychain" =~ [[:space:]\"\'\\] ]]; then
    echo "The beta signing directory must not contain spaces, quotes or backslashes: $dir" >&2
    return 1
  fi
}

# Runs `security -i` commands read from standard input, so passwords stay out
# of process arguments.
_email_beta_security() {
  security -i >/dev/null
}

_email_beta_mode() { stat -f '%Lp' "$1"; }

# Prints the SHA-1 of the one certificate in the beta keychain.
_email_beta_certificate_sha1() {
  local hashes
  hashes="$(security find-certificate -a -Z "$email_beta_keychain" | awk '/^SHA-1 hash:/{print $NF}')"
  if [[ "$(printf '%s\n' "$hashes" | grep -c .)" != 1 ]]; then
    echo "The beta keychain must hold exactly one certificate: $email_beta_keychain" >&2
    return 1
  fi
  printf '%s\n' "$hashes"
}

# Checks that the identity exists and is private, without unlocking it.
email_beta_signing_check() {
  _email_beta_paths || return 1
  if [[ ! -f "$email_beta_keychain" || ! -f "$email_beta_password_file" ]]; then
    echo "No macOS beta signing identity in $(dirname "$email_beta_keychain")." >&2
    echo "Create it once with scripts/macos-beta-signing.sh create, or set EMAIL_BETA_SIGNING_DIR." >&2
    return 1
  fi
  local file
  for file in "$email_beta_keychain" "$email_beta_password_file"; do
    if [[ -L "$file" || "$(_email_beta_mode "$file")" != 600 ]]; then
      echo "$file must be a regular file with mode 600." >&2
      return 1
    fi
  done
  _email_beta_certificate_sha1 >/dev/null || return 1
}

# Unlocks the beta keychain (it locks itself again after five idle minutes)
# and exports EMAIL_RELEASE_CODESIGN_IDENTITY, EMAIL_RELEASE_CODESIGN_KEYCHAIN
# and EMAIL_BETA_CERTIFICATE_SHA1.
email_beta_signing_unlock() {
  email_beta_signing_check || return 1
  local password
  password="$(cat "$email_beta_password_file")" || return 1
  if ! printf 'unlock-keychain -p %s %s\n' "$password" "$email_beta_keychain" | _email_beta_security; then
    echo "Could not unlock $email_beta_keychain with its password file." >&2
    return 1
  fi
  EMAIL_BETA_CERTIFICATE_SHA1="$(_email_beta_certificate_sha1)" || return 1
  EMAIL_RELEASE_CODESIGN_IDENTITY="$EMAIL_BETA_CERTIFICATE_SHA1"
  EMAIL_RELEASE_CODESIGN_KEYCHAIN="$email_beta_keychain"
  export EMAIL_BETA_CERTIFICATE_SHA1 EMAIL_RELEASE_CODESIGN_IDENTITY EMAIL_RELEASE_CODESIGN_KEYCHAIN
}

email_beta_signing_lock() {
  _email_beta_paths 2>/dev/null || return 0
  [[ -f "$email_beta_keychain" ]] || return 0
  security lock-keychain "$email_beta_keychain" 2>/dev/null || true
}

email_beta_signing_create() {
  _email_beta_paths || return 1
  local dir
  dir="$(dirname "$email_beta_keychain")"
  if [[ -e "$email_beta_keychain" || -e "$email_beta_password_file" ]]; then
    echo "A beta signing identity already exists in $dir. Keep using it; a new certificate makes macOS ask testers again for each account." >&2
    return 1
  fi
  if ! openssl version 2>/dev/null | grep -Eq '^OpenSSL ([3-9]|[1-9][0-9])\.'; then
    echo "Creating the identity needs OpenSSL 3 or later on PATH (for example from Homebrew); the system LibreSSL cannot write it." >&2
    return 1
  fi
  ( umask 077; mkdir -p "$dir" ) || return 1
  local work before
  work="$(mktemp -d)" || return 1
  # Private key material exists only in this directory until it is imported.
  (
    set -euo pipefail
    trap 'rm -rf "$work"' EXIT
    umask 077
    openssl rand -hex 32 > "$work/password"
    openssl req -x509 -newkey rsa:3072 -nodes -days 3650 -sha256 \
      -keyout "$work/key.pem" -out "$work/certificate.pem" \
      -subj "$EMAIL_BETA_SIGNING_SUBJECT" \
      -addext "keyUsage=critical,digitalSignature" \
      -addext "extendedKeyUsage=critical,codeSigning" \
      -addext "basicConstraints=critical,CA:false" 2>/dev/null
    openssl pkcs12 -export -legacy -inkey "$work/key.pem" -in "$work/certificate.pem" \
      -out "$work/identity.p12" -passout "file:$work/password"
    rm -f "$work/key.pem"
    before="$(security list-keychains -d user)"
    password="$(cat "$work/password")"
    {
      printf 'create-keychain -p %s %s\n' "$password" "$email_beta_keychain"
      printf 'unlock-keychain -p %s %s\n' "$password" "$email_beta_keychain"
      printf 'import %s -k %s -P %s -T /usr/bin/codesign\n' "$work/identity.p12" "$email_beta_keychain" "$password"
      printf 'set-key-partition-list -S apple-tool:,apple: -s -k %s %s\n' "$password" "$email_beta_keychain"
    } | _email_beta_security
    if [[ "$(security list-keychains -d user)" != "$before" ]]; then
      echo "Creating the keychain changed the keychain search list; removing it." >&2
      security delete-keychain "$email_beta_keychain" 2>/dev/null || rm -f "$email_beta_keychain"
      exit 1
    fi
    install -m 600 "$work/password" "$email_beta_password_file"
    chmod 600 "$email_beta_keychain"
    security lock-keychain "$email_beta_keychain"
  ) || { echo "Could not create the beta signing identity." >&2; return 1; }
  email_beta_signing_status
}

email_beta_signing_status() {
  email_beta_signing_check || return 1
  printf 'Keychain: %s\nCertificate SHA-1: %s\n' "$email_beta_keychain" "$(_email_beta_certificate_sha1)"
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  set -euo pipefail
  case "${1:-}" in
    create) email_beta_signing_create ;;
    status) email_beta_signing_status ;;
    *) echo "Usage: $0 create|status" >&2; exit 2 ;;
  esac
fi
