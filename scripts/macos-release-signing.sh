#!/usr/bin/env bash
# Developer ID signing for public macOS releases
# (scripts/package-macos-release.sh --distribute).
#
#   scripts/macos-release-signing.sh import CERT.cer KEY.pem
#       creates the release keychain from a Developer ID Application
#       certificate (as downloaded from Apple) and its private key, once per
#       certificate
#   scripts/macos-release-signing.sh status
#       prints the keychain, identity and certificate expiry
#
# The identity lives in its own keychain file with the keychain password in a
# sibling file, both mode 600, in EMAIL_RELEASE_SIGNING_DIR (default
# ~/.config/ycorp-release). Apple's Developer ID G2 intermediate is stored with
# it. The password reaches `security` on standard input, never as an argument.
#
# On macOS 27, codesign builds a Developer ID certificate chain only from
# keychains on the user's real search list. `--keychain` and the private search
# list that beta and fixture builds use (email_codesign_home) are not enough:
# signing fails with errSecInternalComponent ("unable to build chain to
# self-signed root"). email_release_signing_unlock therefore puts the keychain
# at the front of the user's search list, and email_release_signing_lock puts
# the list back exactly as it was. The packager calls the lock from its exit
# trap, so the list is restored when a build fails too. While a release build
# runs, scripts/test-macos-updates.sh refuses to start ("Fixture signing
# changed the keychain search list").
#
# Sourced by package-macos-release.sh for email_release_signing_check,
# email_release_signing_unlock and email_release_signing_lock. Never enable
# shell tracing around this file.

EMAIL_RELEASE_SIGNING_NAME=email-macos-release-signing
# Apple's "Developer ID Certification Authority" G2 intermediate, pinned by
# SHA-256 of https://www.apple.com/certificateauthority/DeveloperIDG2CA.cer.
EMAIL_DEVELOPER_ID_G2_URL=https://www.apple.com/certificateauthority/DeveloperIDG2CA.cer
EMAIL_DEVELOPER_ID_G2_SHA256=f16cd3c54c7f83cea4bf1a3e6a0819c8aaa8e4a1528fd144715f350643d2df3a

_email_release_paths() {
  local dir="${EMAIL_RELEASE_SIGNING_DIR:-$HOME/.config/ycorp-release}"
  email_release_keychain="$dir/$EMAIL_RELEASE_SIGNING_NAME.keychain-db"
  email_release_password_file="$dir/$EMAIL_RELEASE_SIGNING_NAME.password"
  # `security -i` splits its input on whitespace.
  if [[ "$email_release_keychain" =~ [[:space:]\"\'\\] ]]; then
    echo "The release signing directory must not contain spaces, quotes or backslashes: $dir" >&2
    return 1
  fi
}

_email_release_mode() { stat -f '%Lp' "$1"; }

# Prints "SHA-1<TAB>name" of the one Developer ID Application identity in the
# release keychain. Validity is not checked here: the keychain is not on the
# search list yet, so its chain cannot be evaluated.
_email_release_identity() {
  local lines
  lines="$(security find-identity -p codesigning "$email_release_keychain" 2>/dev/null \
    | sed -nE 's/^ *[0-9]+\) ([0-9A-F]{40}) "(Developer ID Application: [^"]+)"$/\1	\2/p' | sort -u)"
  if [[ "$(printf '%s\n' "$lines" | grep -c .)" != 1 ]]; then
    echo "The release keychain must hold exactly one Developer ID Application identity: $email_release_keychain" >&2
    return 1
  fi
  printf '%s\n' "$lines"
}

# Checks that the identity exists and is private, without unlocking it.
email_release_signing_check() {
  _email_release_paths || return 1
  if [[ ! -f "$email_release_keychain" || ! -f "$email_release_password_file" ]]; then
    echo "No Developer ID release keychain in $(dirname "$email_release_keychain")." >&2
    echo "Create it once with scripts/macos-release-signing.sh import, set EMAIL_RELEASE_SIGNING_DIR, or set EMAIL_RELEASE_CODESIGN_IDENTITY to an identity in your login keychain." >&2
    return 1
  fi
  local file
  for file in "$email_release_keychain" "$email_release_password_file"; do
    if [[ -L "$file" || "$(_email_release_mode "$file")" != 600 ]]; then
      echo "$file must be a regular file with mode 600." >&2
      return 1
    fi
  done
  _email_release_identity >/dev/null || return 1
}

_email_release_unlock_keychain() {
  local password
  password="$(cat "$email_release_password_file")" || return 1
  if ! printf 'unlock-keychain -p %s %s\n' "$password" "$email_release_keychain" | security -i >/dev/null; then
    echo "Could not unlock $email_release_keychain with its password file." >&2
    return 1
  fi
}

# Unlocks the keychain, puts it first on the user's search list and exports
# EMAIL_RELEASE_CODESIGN_IDENTITY (the certificate's SHA-1),
# EMAIL_RELEASE_CODESIGN_KEYCHAIN and EMAIL_RELEASE_CODESIGN_NAME. Safe to call
# again later in a build (notarization can outlast the keychain's idle lock):
# it unlocks again and leaves the saved search list alone.
email_release_signing_unlock() {
  email_release_signing_check || return 1
  _email_release_unlock_keychain || return 1
  if [[ -z "${_email_release_saved_list_set:-}" ]]; then
    _email_release_saved_list=()
    local line
    while IFS= read -r line; do
      line="${line#"${line%%[![:space:]]*}"}"; line="${line%\"}"; line="${line#\"}"
      [[ -n "$line" ]] && _email_release_saved_list+=("$line")
    done < <(security list-keychains -d user)
    _email_release_saved_list_set=1
    local entry new_list=("$email_release_keychain")
    for entry in "${_email_release_saved_list[@]}"; do
      [[ "$entry" != "$email_release_keychain" ]] && new_list+=("$entry")
    done
    security list-keychains -d user -s "${new_list[@]}" || return 1
  fi
  local identity
  identity="$(_email_release_identity)" || return 1
  EMAIL_RELEASE_CODESIGN_IDENTITY="${identity%%	*}"
  EMAIL_RELEASE_CODESIGN_NAME="${identity#*	}"
  EMAIL_RELEASE_CODESIGN_KEYCHAIN="$email_release_keychain"
  export EMAIL_RELEASE_CODESIGN_IDENTITY EMAIL_RELEASE_CODESIGN_NAME EMAIL_RELEASE_CODESIGN_KEYCHAIN
}

# Restores the search list saved by email_release_signing_unlock and locks the
# keychain. Does nothing if unlock never ran.
email_release_signing_lock() {
  if [[ -n "${_email_release_saved_list_set:-}" ]]; then
    if (( ${#_email_release_saved_list[@]} )); then
      security list-keychains -d user -s "${_email_release_saved_list[@]}" \
        || echo "Could not restore the keychain search list; check it with: security list-keychains -d user" >&2
    else
      security list-keychains -d user -s \
        || echo "Could not restore the keychain search list; check it with: security list-keychains -d user" >&2
    fi
    _email_release_saved_list_set=""
  fi
  _email_release_paths 2>/dev/null || return 0
  [[ -f "$email_release_keychain" ]] || return 0
  security lock-keychain "$email_release_keychain" 2>/dev/null || true
}

email_release_signing_import() {
  local certificate="${1:-}" key="${2:-}"
  if [[ ! -f "$certificate" || ! -f "$key" ]]; then
    echo "Usage: $0 import CERT.cer KEY.pem" >&2
    return 2
  fi
  _email_release_paths || return 1
  local dir
  dir="$(dirname "$email_release_keychain")"
  if [[ -e "$email_release_keychain" || -e "$email_release_password_file" ]]; then
    echo "A release keychain already exists in $dir. To replace the certificate, move both files aside first." >&2
    return 1
  fi
  if ! openssl version 2>/dev/null | grep -Eq '^OpenSSL ([3-9]|[1-9][0-9])\.'; then
    echo "Importing needs OpenSSL 3 or later on PATH (for example from Homebrew); the system LibreSSL cannot write the bundle." >&2
    return 1
  fi
  ( umask 077; mkdir -p "$dir" ) || return 1
  local work
  work="$(mktemp -d)" || return 1
  (
    set -euo pipefail
    trap 'rm -rf "$work"' EXIT
    umask 077
    openssl x509 -inform DER -in "$certificate" -out "$work/certificate.pem" 2>/dev/null \
      || openssl x509 -in "$certificate" -out "$work/certificate.pem"
    subject="$(openssl x509 -in "$work/certificate.pem" -noout -subject -nameopt RFC2253)"
    if [[ "$subject" != *"CN=Developer ID Application: "* ]]; then
      echo "Not a Developer ID Application certificate: $subject" >&2
      exit 1
    fi
    if [[ "$(openssl x509 -in "$work/certificate.pem" -noout -pubkey | openssl sha256)" \
          != "$(openssl pkey -in "$key" -pubout | openssl sha256)" ]]; then
      echo "The private key does not belong to this certificate." >&2
      exit 1
    fi
    curl -sSfL --proto '=https' -o "$work/intermediate.cer" "$EMAIL_DEVELOPER_ID_G2_URL"
    if [[ "$(shasum -a 256 < "$work/intermediate.cer" | awk '{print $1}')" != "$EMAIL_DEVELOPER_ID_G2_SHA256" ]]; then
      echo "Apple's Developer ID G2 intermediate does not match its pinned SHA-256." >&2
      exit 1
    fi
    openssl x509 -inform DER -in "$work/intermediate.cer" -out "$work/intermediate.pem"
    openssl rand -hex 32 > "$work/password"
    name="${subject#*CN=}"; name="${name%%,*}"
    openssl pkcs12 -export -legacy -inkey "$key" -in "$work/certificate.pem" \
      -certfile "$work/intermediate.pem" -name "$name" \
      -out "$work/identity.p12" -passout "file:$work/password"
    before="$(security list-keychains -d user)"
    password="$(cat "$work/password")"
    {
      printf 'create-keychain -p %s %s\n' "$password" "$email_release_keychain"
      printf 'unlock-keychain -p %s %s\n' "$password" "$email_release_keychain"
      printf 'import %s -k %s -P %s -T /usr/bin/codesign\n' "$work/identity.p12" "$email_release_keychain" "$password"
      printf 'set-key-partition-list -S apple-tool:,apple: -s -k %s %s\n' "$password" "$email_release_keychain"
    } | security -i >/dev/null
    if [[ "$(security list-keychains -d user)" != "$before" ]]; then
      echo "Creating the keychain changed the keychain search list; removing it." >&2
      security delete-keychain "$email_release_keychain" 2>/dev/null || rm -f "$email_release_keychain"
      exit 1
    fi
    install -m 600 "$work/password" "$email_release_password_file"
    chmod 600 "$email_release_keychain"
    security lock-keychain "$email_release_keychain"
  ) || { echo "Could not import the release signing identity." >&2; return 1; }
  email_release_signing_status
}

email_release_signing_status() {
  email_release_signing_check || return 1
  local identity expiry
  identity="$(_email_release_identity)" || return 1
  expiry="$(security find-certificate -c "${identity#*	}" -p "$email_release_keychain" | openssl x509 -noout -enddate)"
  printf 'Keychain: %s\nIdentity: %s\nCertificate SHA-1: %s\nExpires: %s\n' \
    "$email_release_keychain" "${identity#*	}" "${identity%%	*}" "${expiry#notAfter=}"
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  set -euo pipefail
  case "${1:-}" in
    import) shift; email_release_signing_import "$@" ;;
    status) email_release_signing_status ;;
    *) echo "Usage: $0 import CERT.cer KEY.pem | status" >&2; exit 2 ;;
  esac
fi
