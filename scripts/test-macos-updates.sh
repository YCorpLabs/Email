#!/usr/bin/env bash
# Isolated acceptance test for macOS automatic updates.
#
# Builds complete fixture bundles (A: build 100, B: build 101 stable,
# C: build 102 testing, N: no feed), signed with a disposable identity and a
# disposable EdDSA key, serves a signed appcast on 127.0.0.1, and drives the
# real Sparkle pipeline inside the shipped app bundle. Nothing touches the
# login Keychain, the development app or any network host besides loopback.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$repo_root/scripts/macos-updates-env.sh"
work="$(mktemp -d "${TMPDIR:-/tmp}/email-update-test.XXXXXX")"
server_pid=""
fixture_domain="$EMAIL_UPDATE_FIXTURE_BUNDLE_ID"
cleanup() {
  local result=$?
  if [[ -n "$server_pid" ]]; then kill "$server_pid" 2>/dev/null || true; fi
  email_remove_fixture_signing
  defaults delete "$fixture_domain" >/dev/null 2>&1 || true
  rm -rf "$HOME/Library/Caches/$fixture_domain" "$HOME/Library/HTTPStorages/$fixture_domain"
  if [[ "$result" != 0 && "${EMAIL_KEEP_UPDATE_TEST:-0}" == 1 ]]; then
    echo "Kept $work for inspection." >&2
  else
    rm -rf "$work"
  fi
}
trap cleanup EXIT

passed=()
pass() { passed+=("$1"); printf 'ok   %s\n' "$1"; }
fail() { printf 'FAIL %s\n' "$1" >&2; exit 1; }
build_of() { /usr/libexec/PlistBuddy -c 'Print :CFBundleVersion' "$1/Contents/Info.plist"; }
status_of() { EMAIL_STORE_PATH="$work/status.sqlite" "$1/Contents/MacOS/Email" --updater-status; }

email_create_fixture_signing "$work/signing"
email_create_fixture_ed_key "$work/ed"
public_key="$(tr -d '\n' < "$work/ed/public.key")"
port="$(python3 -I -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
feed="http://127.0.0.1:$port/appcast.xml"

package() { # name version build channel feed
  EMAIL_RELEASE_OUTPUT_DIR="$work/pkg/$1" EMAIL_VERSION="$2" EMAIL_BUILD_NUMBER="$3" EMAIL_UPDATE_CHANNEL="$4" \
    EMAIL_UPDATE_FEED_URL="$5" EMAIL_UPDATE_PUBLIC_ED_KEY="$( [[ -n "$5" ]] && printf '%s' "$public_key" )" \
    "$repo_root/scripts/package-macos-release.sh" --fixture >"$work/pkg-$1.log" 2>&1 \
    || { tail -30 "$work/pkg-$1.log" >&2; fail "packaging $1"; }
}
package A 0.1.0 100 stable "$feed"
package B 0.1.1 101 stable "$feed"
package C 0.2.0 102 testing "$feed"
package N 0.1.0 100 stable ""
pass "packaged fixture builds A=100, B=101 stable, C=102 testing, N without feed"

"$repo_root/scripts/verify-macos-release.sh" "$work/pkg/A/Email.app" >"$work/verify-A.log" 2>&1 || { cat "$work/verify-A.log" >&2; fail "release verifier on A"; }
pass "release verifier on configured bundle (signatures, Sparkle signer, tamper refusal)"
"$repo_root/scripts/verify-macos-release.sh" "$work/pkg/N/Email.app" >"$work/verify-N.log" 2>&1 || { cat "$work/verify-N.log" >&2; fail "release verifier on N"; }
pass "release verifier on unconfigured bundle"

# Safe first launch: no feed means no updater, no menu and no network.
[[ "$(status_of "$work/pkg/N/Email.app")" == *'"reason":"not-configured"'* ]] || fail "unconfigured status"
if EMAIL_STORE_PATH="$work/n.sqlite" "$work/pkg/N/Email.app/Contents/MacOS/Email" --updater-fixture-install 2>"$work/n.log"; then fail "unconfigured bundle ran an update"; fi
pass "no configured feed: updater disabled and install refused"
[[ "$(status_of "$work/pkg/A/Email.app")" == *'"channel":"stable"'* ]] || fail "A defaults to stable"
[[ "$(status_of "$work/pkg/C/Email.app")" == *'"channel":"testing"'* ]] || fail "C defaults to testing"
pass "testing builds default to the testing channel, stable builds to stable"
# Opt-in: Sparkle must not have recorded a check or consent during these runs.
if defaults read "$fixture_domain" SUEnableAutomaticChecks >/dev/null 2>&1; then fail "automatic checks were enabled without consent"; fi
pass "no automatic-check consent recorded without the user"

# Publish B (stable) and C (testing) with the real publishing script.
feed_dir="$work/feed"
publish() { # archive channel
  "$repo_root/scripts/publish-macos-update.sh" --fixture --archive "$1" --channel "$2" --feed-dir "$feed_dir" \
    --download-prefix "http://127.0.0.1:$port/" --ed-key-file "$work/ed/private.key" >"$work/publish-$2.log" 2>&1 \
    || { tail -30 "$work/publish-$2.log" >&2; fail "publishing $1"; }
}
publish "$work/pkg/B/Email-0.1.1-101-$(uname -m).zip" stable
original_feed_hash="$(shasum -a 256 "$feed_dir/appcast.xml" | awk '{print $1}')"
email_create_fixture_ed_key "$work/wrong-ed"
if "$repo_root/scripts/publish-macos-update.sh" --fixture --archive "$work/pkg/C/Email-0.2.0-102-$(uname -m).zip" --channel testing \
    --feed-dir "$feed_dir" --download-prefix "http://127.0.0.1:$port/" --ed-key-file "$work/wrong-ed/private.key" >"$work/publish-invalid-key.log" 2>&1; then
  fail "publishing with the wrong signing key was accepted"
fi
[[ "$(shasum -a 256 "$feed_dir/appcast.xml" | awk '{print $1}')" == "$original_feed_hash" ]] || fail "failed publication changed the verified feed"
[[ ! -e "$feed_dir/Email-0.2.0-102-$(uname -m).zip" && ! -d "$feed_dir.publish-lock" ]] || fail "failed publication left unverified output or a lock"
pass "failed publication preserves the last verified feed and archives"
publish "$work/pkg/C/Email-0.2.0-102-$(uname -m).zip" testing
pass "published signed appcast with stable and testing items"
if "$repo_root/scripts/publish-macos-update.sh" --fixture --archive "$work/pkg/A/Email-0.1.0-100-$(uname -m).zip" --channel stable \
    --feed-dir "$feed_dir" --download-prefix "http://127.0.0.1:$port/" --ed-key-file "$work/ed/private.key" >"$work/publish-old.log" 2>&1; then
  fail "publishing an older build was accepted"
fi
if "$repo_root/scripts/publish-macos-update.sh" --fixture --archive "$work/pkg/C/Email-0.2.0-102-$(uname -m).zip" --channel stable \
    --feed-dir "$work/feed-wrong" --download-prefix "http://127.0.0.1:$port/" --ed-key-file "$work/ed/private.key" >"$work/publish-wrong.log" 2>&1; then
  fail "publishing a testing build to stable was accepted"
fi
official_host="$(python3 "$repo_root/scripts/distribution-profile.py" get UPDATE_ALLOWED_HOSTS --profile official | awk '{print $NF}')"
if EMAIL_DISTRIBUTION_PROFILE=official "$repo_root/scripts/publish-macos-update.sh" --archive "$work/pkg/B/Email-0.1.1-101-$(uname -m).zip" --channel stable \
    --feed-dir "$work/feed-prod" --download-prefix "https://$official_host/email/macos/" --ed-key-file "$work/ed/private.key" >"$work/publish-prod.log" 2>&1; then
  fail "production publishing accepted a fixture-signed build"
fi
pass "publisher refuses older builds, testing builds on stable and fixture bundles for production"

serve="$work/serve"
mkdir -p "$serve"
python3 -I -m http.server --bind 127.0.0.1 --directory "$serve" "$port" >"$work/server.log" 2>&1 &
server_pid=$!
for _ in $(seq 50); do curl -fs "http://127.0.0.1:$port/" >/dev/null 2>&1 && break; sleep 0.1; done
stage_feed() { # mutate-command
  rm -rf "$serve"/*
  cp "$feed_dir/appcast.xml" "$feed_dir"/*.zip "$serve/"
}
fresh_app() { rm -rf "$work/run/$1"; mkdir -p "$work/run/$1"; ditto "$work/pkg/A/Email.app" "$work/run/$1/Email.app"; printf '%s\n' "$work/run/$1/Email.app"; }
run_update() { # app log
  EMAIL_STORE_PATH="$work/run.sqlite" "$1/Contents/MacOS/Email" --updater-fixture-install >"$2" 2>&1
}
wait_for_build() { # app build
  for _ in $(seq 120); do
    [[ "$(build_of "$1" 2>/dev/null)" == "$2" ]] && codesign --verify --deep --strict "$1" 2>/dev/null && return 0
    sleep 0.5
  done
  return 1
}

# Tampered payload: same length, one byte changed. Must not install.
stage_feed
python3 -I - "$serve/Email-0.1.1-101-$(uname -m).zip" <<'PY'
import sys
path = sys.argv[1]
data = bytearray(open(path, 'rb').read())
data[len(data) // 2] ^= 0xFF
open(path, 'wb').write(data)
PY
app="$(fresh_app payload)"
if run_update "$app" "$work/payload.log"; then cat "$work/payload.log" >&2; fail "tampered payload run reported success"; fi
sleep 3
[[ "$(build_of "$app")" == 100 ]] && codesign --verify --deep --strict "$app" || fail "tampered payload changed the app"
grep -q "updater-fixture: error" "$work/payload.log" || { cat "$work/payload.log" >&2; fail "tampered payload error not reported"; }
pass "tampered update archive rejected; app unchanged ($(grep -m1 'updater-fixture: error' "$work/payload.log" | cut -c1-90))"

# Tampered feed: change the offered build without re-signing.
stage_feed
sed -i '' 's#<sparkle:version>101</sparkle:version>#<sparkle:version>109</sparkle:version>#' "$serve/appcast.xml"
app="$(fresh_app feed)"
if run_update "$app" "$work/feed.log"; then cat "$work/feed.log" >&2; fail "tampered feed run reported success"; fi
sleep 3
[[ "$(build_of "$app")" == 100 ]] || fail "tampered feed changed the app"
pass "tampered appcast rejected; app unchanged ($(grep -m1 'updater-fixture:' "$work/feed.log" | cut -c1-90))"

# Unsigned feed: signature block removed.
stage_feed
python3 -I - "$serve/appcast.xml" <<'PY'
import re, sys
path = sys.argv[1]
text = open(path, encoding='utf-8').read()
open(path, 'w', encoding='utf-8').write(re.sub(r'<!-- sparkle-signatures:.*?-->\s*$', '', text, flags=re.S))
PY
app="$(fresh_app unsigned)"
if run_update "$app" "$work/unsigned.log"; then cat "$work/unsigned.log" >&2; fail "unsigned feed run reported success"; fi
sleep 3
[[ "$(build_of "$app")" == 100 ]] || fail "unsigned feed changed the app"
pass "unsigned appcast rejected; app unchanged"

# Genuine stable update: A (100) to B (101), never to testing C (102).
stage_feed
app="$(fresh_app stable)"
run_update "$app" "$work/stable.log" || { cat "$work/stable.log" >&2; fail "stable update host did not exit normally"; }
wait_for_build "$app" 101 || { cat "$work/stable.log" >&2; fail "A did not update to B"; }
[[ "$(status_of "$app")" == *'"build":101'* ]] || fail "updated app status"
EMAIL_STORE_PATH="$work/stable-smoke.sqlite" "$app/Contents/MacOS/Email" --smoke-service >/dev/null || fail "updated app smoke"
pass "stable A=100 updated to B=101 in place; signature valid, updater and service run"

# Up to date: B sees nothing newer on stable.
if run_update "$app" "$work/current.log"; then fail "up-to-date run reported an install"; fi
grep -q "no update" "$work/current.log" || { cat "$work/current.log" >&2; fail "up-to-date run"; }
[[ "$(build_of "$app")" == 101 ]] || fail "up-to-date app changed"
pass "updated build finds no newer stable item"

# Testing channel opt-in: A moves straight to C (102).
defaults write "$fixture_domain" EmailUpdateChannel testing
app="$(fresh_app testing)"
run_update "$app" "$work/testing.log" || { cat "$work/testing.log" >&2; fail "testing update host did not exit normally"; }
wait_for_build "$app" 102 || { cat "$work/testing.log" >&2; fail "testing channel did not update to C"; }
defaults delete "$fixture_domain" EmailUpdateChannel
pass "testing channel A=100 updated to C=102"

# Fixture-only install path is refused by a non-fixture bundle ID.
app="$(fresh_app idcheck)"
/usr/libexec/PlistBuddy -c "Set :CFBundleIdentifier $(python3 "$repo_root/scripts/distribution-profile.py" get MACOS_BUNDLE_ID --profile official)" "$app/Contents/Info.plist"
email_codesign_home "$EMAIL_RELEASE_CODESIGN_KEYCHAIN" "$work/codesign-home"
HOME="$work/codesign-home" codesign --force --sign "$EMAIL_RELEASE_CODESIGN_IDENTITY" --keychain "$EMAIL_RELEASE_CODESIGN_KEYCHAIN" \
  --timestamp=none --identifier "$fixture_domain" "$app" 2>/dev/null
[[ "$(status_of "$app")" == *'"reason":"fixture-not-allowed"'* ]] || fail "fixture settings on production ID: $(status_of "$app")"
if run_update "$app" "$work/idcheck.log"; then fail "production ID ran the fixture installer"; fi
pass "fixture settings re-signed under the production bundle ID are refused"

printf '\nAll %d update acceptance checks passed.\n' "${#passed[@]}"
