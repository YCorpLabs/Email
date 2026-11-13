# shellcheck shell=bash
# Shared helpers for macOS update packaging, publishing and acceptance tests.
# Source this file; it defines functions and constants only.

# Keep in step with apps/macos/Package.swift and Package.resolved.
EMAIL_SPARKLE_VERSION=2.10.1
EMAIL_SPARKLE_TOOLS_URL="https://github.com/sparkle-project/Sparkle/releases/download/${EMAIL_SPARKLE_VERSION}/Sparkle-${EMAIL_SPARKLE_VERSION}.tar.xz"
EMAIL_SPARKLE_TOOLS_SHA256=1a856d2a9edb8e628de88121d7be04cf0e2167f8641650b78fca00b14244cf27
# Production update hosts come from the distribution profile (UPDATE_ALLOWED_HOSTS).
# Disposable acceptance-test bundles only; keep in step with UpdatePolicy.swift.
EMAIL_UPDATE_FIXTURE_BUNDLE_ID=invalid.email.update-fixture

# Prints a directory holding Sparkle's bin/ tools, verified by SHA-256.
# The download is cached outside the repository; extraction is per call.
email_sparkle_tools() {
  local cache="${EMAIL_SPARKLE_CACHE:-$HOME/Library/Caches/email-build-tools/sparkle}"
  local archive="$cache/Sparkle-${EMAIL_SPARKLE_VERSION}.tar.xz"
  mkdir -p "$cache"
  if [[ ! -f "$archive" ]]; then
    curl -fsSL --proto '=https' -o "$archive.partial" "$EMAIL_SPARKLE_TOOLS_URL"
    mv "$archive.partial" "$archive"
  fi
  local actual
  actual="$(shasum -a 256 "$archive" | awk '{print $1}')"
  if [[ "$actual" != "$EMAIL_SPARKLE_TOOLS_SHA256" ]]; then
    rm -f "$archive"
    echo "Sparkle tools checksum mismatch: $actual" >&2
    return 1
  fi
  local out
  out="$(mktemp -d "${TMPDIR:-/tmp}/email-sparkle-tools.XXXXXX")"
  tar -xJf "$archive" -C "$out" bin
  printf '%s\n' "$out"
}

# Writes a private keychain search list for codesign to directory $2: keychain
# $1 first, then the user's current search list. Use it as
#   HOME="$2" codesign ... --keychain "$1" ...
# codesign before macOS 27 resolves a signing identity only from keychains on
# the keychain search list; --keychain only narrows that search, so macOS 26
# reports "no identity found" for a keychain file that is not on the list.
# codesign reads the user-domain search list from
# $HOME/Library/Preferences/com.apple.security.plist, so a HOME of its own
# gives it the keychain without changing the user's search list. Nothing is
# written outside $2; the user's preferences, search list and login keychain
# stay untouched. --keychain still limits which identity is used.
email_codesign_home() {
  local keychain="$1" dir="$2"
  mkdir -p "$dir/Library/Preferences" || return 1
  security list-keychains -d user | python3 -I -c '
import os, plistlib, sys
paths = [os.path.abspath(sys.argv[2])]
for line in sys.stdin:
    path = line.strip().strip("\"")
    if path and path not in paths:
        paths.append(path)
# The Apple CSP/DL module and service type that `security list-keychains -s` records.
entries = [{"DbName": path, "GUID": "{87191ca3-0fc9-11d4-849a-000502b52122}", "SubserviceType": 6}
           for path in paths]
with open(sys.argv[1], "wb") as out:
    plistlib.dump({"DLDBSearchList": entries}, out)
' "$dir/Library/Preferences/com.apple.security.plist" "$keychain"
}

# Creates a throwaway keychain file and self-signed code signing identity in
# $1, and checks that codesign can sign with it. Never touches the login
# keychain or the keychain search list.
# Sets EMAIL_RELEASE_CODESIGN_IDENTITY and EMAIL_RELEASE_CODESIGN_KEYCHAIN.
email_create_fixture_signing() {
  local dir="$1"
  mkdir -p "$dir"
  local password keychain identity
  password="$(openssl rand -hex 24)"
  keychain="$dir/fixture-signing.keychain-db"
  ( umask 077
    openssl req -x509 -newkey rsa:2048 -nodes -days 2 \
      -keyout "$dir/fixture-key.pem" -out "$dir/fixture-cert.pem" \
      -subj "/CN=Email Update Fixture $$" \
      -addext "keyUsage=critical,digitalSignature" \
      -addext "extendedKeyUsage=critical,codeSigning" \
      -addext "basicConstraints=critical,CA:false" 2>/dev/null )
  local before
  before="$(security list-keychains -d user)"
  security create-keychain -p "$password" "$keychain"
  security set-keychain-settings "$keychain"
  security unlock-keychain -p "$password" "$keychain"
  # Separate PEM files import the same way whether openssl is OpenSSL 1.1,
  # OpenSSL 3 or LibreSSL. A PKCS#12 bundle would need -legacy on OpenSSL 3,
  # which the others reject; Keychain cannot read OpenSSL 3's default PKCS#12.
  security import "$dir/fixture-key.pem" -k "$keychain" -t priv -T /usr/bin/codesign >/dev/null
  security import "$dir/fixture-cert.pem" -k "$keychain" -t cert >/dev/null
  security set-key-partition-list -S apple-tool:,apple: -s -k "$password" "$keychain" >/dev/null
  rm -f "$dir/fixture-key.pem"
  identity="$(security find-certificate -a -Z "$keychain" | awk '/SHA-1/{print $NF; exit}')"
  # Fail here, not after a long build, if codesign cannot use the identity.
  email_codesign_home "$keychain" "$dir/codesign-home"
  cp /usr/bin/true "$dir/codesign-probe"
  if ! HOME="$dir/codesign-home" codesign --force --sign "$identity" --keychain "$keychain" \
      --timestamp=none "$dir/codesign-probe" 2>"$dir/codesign-probe.log"; then
    echo "codesign cannot sign with the fixture identity:" >&2
    cat "$dir/codesign-probe.log" >&2
    security delete-keychain "$keychain" 2>/dev/null || rm -f "$keychain"
    return 1
  fi
  rm -f "$dir/codesign-probe" "$dir/codesign-probe.log"
  if [[ "$(security list-keychains -d user)" != "$before" ]]; then
    echo "Fixture signing changed the keychain search list; refusing to continue." >&2
    security delete-keychain "$keychain" || true
    return 1
  fi
  EMAIL_RELEASE_CODESIGN_IDENTITY="$identity"
  EMAIL_RELEASE_CODESIGN_KEYCHAIN="$keychain"
  export EMAIL_RELEASE_CODESIGN_IDENTITY EMAIL_RELEASE_CODESIGN_KEYCHAIN
}

email_remove_fixture_signing() {
  if [[ -n "${EMAIL_RELEASE_CODESIGN_KEYCHAIN:-}" && -f "$EMAIL_RELEASE_CODESIGN_KEYCHAIN" ]]; then
    security delete-keychain "$EMAIL_RELEASE_CODESIGN_KEYCHAIN" 2>/dev/null || rm -f "$EMAIL_RELEASE_CODESIGN_KEYCHAIN"
    local dir
    dir="$(dirname "$EMAIL_RELEASE_CODESIGN_KEYCHAIN")"
    rm -rf "$dir/codesign-home" "$dir/fixture-cert.pem"
  fi
}

# Writes a throwaway EdDSA key pair in Sparkle's file format to $1.
# private.key: base64 seed for --ed-key-file. public.key: SUPublicEDKey.
email_create_fixture_ed_key() {
  local dir="$1"
  mkdir -p "$dir"
  ( umask 077
    openssl genpkey -algorithm ed25519 -out "$dir/ed25519.pem"
    openssl pkey -in "$dir/ed25519.pem" -outform DER | tail -c 32 | base64 > "$dir/private.key"
    openssl pkey -in "$dir/ed25519.pem" -pubout -outform DER | tail -c 32 | base64 > "$dir/public.key"
    rm -f "$dir/ed25519.pem" )
}
