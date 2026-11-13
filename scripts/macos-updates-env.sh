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

# Creates a throwaway keychain file and self-signed code signing identity in
# $1. Never touches the login keychain or the keychain search list.
# Sets EMAIL_RELEASE_CODESIGN_IDENTITY and EMAIL_RELEASE_CODESIGN_KEYCHAIN.
email_create_fixture_signing() {
  local dir="$1"
  mkdir -p "$dir"
  local password keychain
  password="$(openssl rand -hex 24)"
  keychain="$dir/fixture-signing.keychain-db"
  openssl req -x509 -newkey rsa:2048 -nodes -days 2 \
    -keyout "$dir/fixture-key.pem" -out "$dir/fixture-cert.pem" \
    -subj "/CN=Email Update Fixture $$" \
    -addext "keyUsage=critical,digitalSignature" \
    -addext "extendedKeyUsage=critical,codeSigning" \
    -addext "basicConstraints=critical,CA:false" 2>/dev/null
  openssl pkcs12 -export -legacy -inkey "$dir/fixture-key.pem" -in "$dir/fixture-cert.pem" \
    -out "$dir/fixture.p12" -passout "pass:$password"
  local before
  before="$(security list-keychains -d user)"
  security create-keychain -p "$password" "$keychain"
  security set-keychain-settings "$keychain"
  security unlock-keychain -p "$password" "$keychain"
  security import "$dir/fixture.p12" -k "$keychain" -P "$password" -T /usr/bin/codesign >/dev/null
  security set-key-partition-list -S apple-tool:,apple: -s -k "$password" "$keychain" >/dev/null
  rm -f "$dir/fixture-key.pem" "$dir/fixture.p12"
  if [[ "$(security list-keychains -d user)" != "$before" ]]; then
    echo "Fixture signing changed the keychain search list; refusing to continue." >&2
    security delete-keychain "$keychain" || true
    return 1
  fi
  EMAIL_RELEASE_CODESIGN_IDENTITY="$(security find-certificate -a -Z "$keychain" | awk '/SHA-1/{print $NF; exit}')"
  EMAIL_RELEASE_CODESIGN_KEYCHAIN="$keychain"
  export EMAIL_RELEASE_CODESIGN_IDENTITY EMAIL_RELEASE_CODESIGN_KEYCHAIN
}

email_remove_fixture_signing() {
  if [[ -n "${EMAIL_RELEASE_CODESIGN_KEYCHAIN:-}" && -f "$EMAIL_RELEASE_CODESIGN_KEYCHAIN" ]]; then
    security delete-keychain "$EMAIL_RELEASE_CODESIGN_KEYCHAIN" 2>/dev/null || rm -f "$EMAIL_RELEASE_CODESIGN_KEYCHAIN"
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
