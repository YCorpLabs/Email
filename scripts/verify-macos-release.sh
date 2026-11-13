#!/usr/bin/env bash
set -euo pipefail
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source_app="${1:-$repo_root/dist-release/local-alpha/Email.app}"
if [[ ! -d "$source_app/Contents/Frameworks" ]]; then echo "No self-contained app at $source_app" >&2; exit 2; fi
source "$repo_root/scripts/package-notices.sh"
require_package_notices "$source_app/Contents/Resources/Legal"
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
# Relocation detects assumptions about the repository or sibling Runtime folder.
ditto "$source_app" "$scratch/Email.app"
app="$scratch/Email.app"
codesign --verify --deep --strict "$app"
EMAIL_STORE_PATH="$scratch/smoke.sqlite" MAIL_SERVICE_LIBRARY=/definitely-not-a-library.dylib \
  "$app/Contents/MacOS/Email" --smoke-service
# Neither an incomplete nor a modified bundle may fall back to an external library.
ditto "$source_app" "$scratch/Incomplete.app"
rm "$scratch/Incomplete.app/Contents/Frameworks/libmail_service.dylib"
if EMAIL_STORE_PATH="$scratch/incomplete.sqlite" MAIL_SERVICE_LIBRARY="$source_app/Contents/Frameworks/libmail_service.dylib" \
  "$scratch/Incomplete.app/Contents/MacOS/Email" --smoke-service >"$scratch/incomplete.log" 2>&1; then
  echo "FAIL: an incomplete bundle loaded an external runtime" >&2; exit 1
fi
ditto "$source_app" "$scratch/Modified.app"
printf '\001' >> "$scratch/Modified.app/Contents/Frameworks/libmail_service.dylib"
if EMAIL_STORE_PATH="$scratch/modified.sqlite" "$scratch/Modified.app/Contents/MacOS/Email" --smoke-service >"$scratch/modified.log" 2>&1; then
  echo "FAIL: a modified runtime was accepted" >&2; exit 1
fi
"$app/Contents/Helpers/mail-mcp" --help >/dev/null

# Automatic updates: Sparkle embedding, signatures and fail-closed loading.
source "$repo_root/scripts/macos-updates-env.sh"
fail() { echo "FAIL: $*" >&2; exit 1; }
grep -Fq "### sparkle $EMAIL_SPARKLE_VERSION" "$app/Contents/Resources/Legal/THIRD_PARTY_NOTICES.md" || fail "Sparkle's bundled license notice is missing"
plist_value() { /usr/libexec/PlistBuddy -c "Print :$2" "$1" 2>/dev/null || true; }
sparkle="$app/Contents/Frameworks/Sparkle.framework"
updater_lib="$app/Contents/Frameworks/libEmailUpdater.dylib"
[[ -d "$sparkle" && -f "$updater_lib" ]] || fail "Sparkle.framework or libEmailUpdater.dylib is missing"
[[ "$(plist_value "$sparkle/Versions/B/Resources/Info.plist" CFBundleShortVersionString)" == "$EMAIL_SPARKLE_VERSION" ]] \
  || fail "embedded Sparkle is not the pinned $EMAIL_SPARKLE_VERSION"
[[ -L "$sparkle/Versions/Current" && -L "$sparkle/Sparkle" ]] || fail "Sparkle framework symlinks were not preserved"
[[ ! -e "$sparkle/Versions/B/XPCServices" && ! -e "$sparkle/XPCServices" ]] || fail "sandbox XPC services should not ship"
codesign -d --extract-certificates="$scratch/app-cert" "$app" 2>/dev/null
[[ -f "$scratch/app-cert0" ]] || fail "the app is not signed with a certificate"
leaf="$(shasum -a 1 < "$scratch/app-cert0" | awk '{print $1}')"
for code in "$sparkle/Versions/B/Autoupdate" "$sparkle/Versions/B/Updater.app" "$sparkle" "$updater_lib"; do
  codesign --verify --strict -R "=certificate leaf = H\"$leaf\"" "$code" || fail "$code is not signed by the app's signer"
done
for binary in "$updater_lib" "$app/Contents/Frameworks/libEmailUI.dylib"; do
  if otool -l "$binary" | awk '/cmd LC_RPATH/{getline; getline; print $2}' | grep -Ev '^(@loader_path|@executable_path|/usr/lib/swift$)' >/dev/null; then
    fail "$(basename "$binary") keeps a build-machine runtime search path"
  fi
done
info="$app/Contents/Info.plist"
[[ "$(plist_value "$info" SUVerifyUpdateBeforeExtraction)" == true ]] || fail "SUVerifyUpdateBeforeExtraction must be true"
[[ "$(plist_value "$info" SURequireSignedFeed)" == true ]] || fail "SURequireSignedFeed must be true"
[[ "$(plist_value "$info" SUSignedFeedFailureExpirationInterval)" == 0 ]] || fail "feed signature failures must not expire"
[[ "$(plist_value "$info" SUEnableAutomaticChecks)" != true ]] || fail "automatic checks must stay opt-in"
[[ "$(plist_value "$info" SUAutomaticallyUpdate)" != true ]] || fail "silent installs must stay opt-in"
bundle_id="$(plist_value "$info" CFBundleIdentifier)"
if [[ -n "$(plist_value "$info" EmailUpdateFixture)" && "$bundle_id" != "$EMAIL_UPDATE_FIXTURE_BUNDLE_ID" ]]; then
  fail "fixture update settings in a non-fixture bundle"
fi
status="$(EMAIL_STORE_PATH="$scratch/updater.sqlite" "$app/Contents/MacOS/Email" --updater-status)"
if [[ -n "$(plist_value "$info" SUFeedURL)" ]]; then
  [[ "$status" == *'"enabled":true'* ]] || fail "configured updater did not start: $status"
else
  [[ "$status" == *'"reason":"not-configured"'* ]] || fail "unconfigured updater reported: $status"
fi
if [[ "${EMAIL_VERIFY_REQUIRE_UPDATES:-0}" == 1 && "$status" != *'"enabled":true'* ]]; then
  fail "this release must have automatic updates enabled: $status"
fi
ditto "$source_app" "$scratch/SparkleModified.app"
printf '\001' >> "$scratch/SparkleModified.app/Contents/Frameworks/Sparkle.framework/Versions/B/Autoupdate"
status="$(EMAIL_STORE_PATH="$scratch/sparkle-modified.sqlite" "$scratch/SparkleModified.app/Contents/MacOS/Email" --updater-status)"
[[ "$status" == *'"reason":"module-signature-invalid"'* ]] || fail "modified Sparkle was not refused: $status"
ditto "$source_app" "$scratch/ConfigModified.app"
/usr/libexec/PlistBuddy -c 'Delete :SUFeedURL' "$scratch/ConfigModified.app/Contents/Info.plist" 2>/dev/null || true
/usr/libexec/PlistBuddy -c 'Add :SUFeedURL string https://updates.example.invalid/appcast.xml' "$scratch/ConfigModified.app/Contents/Info.plist"
status="$(EMAIL_STORE_PATH="$scratch/config-modified.sqlite" "$scratch/ConfigModified.app/Contents/MacOS/Email" --updater-status)"
[[ "$status" == *'"reason":"signature-invalid"'* ]] || fail "modified update configuration was not refused: $status"
printf 'Release package checks passed: signatures, relocation, isolated store, environment override, missing library, modified library, bundled MCP, Sparkle %s embedding and signer, updater status, modified Sparkle, modified update configuration.\n' "$EMAIL_SPARKLE_VERSION"
