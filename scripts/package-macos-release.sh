#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$repo_root/scripts/package-notices.sh"
source "$repo_root/scripts/macos-updates-env.sh"
source "$repo_root/scripts/distribution-profile.sh"
export EMAIL_UPDATE_FIXTURE_BUNDLE_ID
require_package_notices "$repo_root"
mode="${1:---local-alpha}"
case "$mode" in
  --local-alpha|--distribute|--beta|--fixture) ;;
  *) echo "Usage: $0 [--local-alpha|--distribute|--beta|--fixture]" >&2; exit 2 ;;
esac
stage=""
on_exit() {
  if [[ -n "$stage" ]]; then rm -rf "$stage"; fi
  if declare -F email_beta_signing_lock >/dev/null; then email_beta_signing_lock; fi
}
trap on_exit EXIT
# Fixture bundles are disposable acceptance tests: neutral identity and no
# OAuth application values. Other modes use the selected distribution profile.
if [[ "$mode" == --fixture ]]; then export EMAIL_DISTRIBUTION_PROFILE=unofficial; fi
email_load_distribution_profile "$repo_root"
if [[ "$mode" == --fixture ]]; then
  EMAIL_GOOGLE_CLIENT_ID="" EMAIL_GOOGLE_CLIENT_SECRET="" EMAIL_MICROSOFT_CLIENT_ID=""
elif [[ "$mode" == --local-alpha ]]; then
  email_refuse_distribution_profile
  email_require_google_client_id "$repo_root"
fi
export EMAIL_GOOGLE_CLIENT_ID EMAIL_GOOGLE_CLIENT_SECRET EMAIL_MICROSOFT_CLIENT_ID
export EMAIL_VERSION="${EMAIL_VERSION:-0.1.0}"
export EMAIL_BUILD_NUMBER="${EMAIL_BUILD_NUMBER:-1}"
# Sparkle orders builds by CFBundleVersion. Only positive integers are allowed
# so every published build is strictly greater than the last.
if [[ ! "$EMAIL_BUILD_NUMBER" =~ ^[1-9][0-9]{0,8}$ ]]; then
  echo "EMAIL_BUILD_NUMBER must be a positive integer without leading zeros." >&2
  exit 2
fi
# The profile's feed applies unless the release environment overrides it. Hosts
# come only from the profile: without them the updater is disabled.
export EMAIL_UPDATE_FEED_URL="${EMAIL_UPDATE_FEED_URL:-$EMAIL_PROFILE_UPDATE_FEED_URL}"
export EMAIL_UPDATE_PUBLIC_ED_KEY="${EMAIL_UPDATE_PUBLIC_ED_KEY:-$EMAIL_PROFILE_UPDATE_PUBLIC_ED_KEY}"
export EMAIL_UPDATE_ALLOWED_HOSTS="$EMAIL_PROFILE_UPDATE_ALLOWED_HOSTS"
# A feed or key recorded in the profile cannot be replaced from the environment.
for update_value in UPDATE_FEED_URL UPDATE_PUBLIC_ED_KEY; do
  profile_name="EMAIL_PROFILE_$update_value" environment_name="EMAIL_$update_value"
  if [[ -n "${!profile_name}" && "${!environment_name}" != "${!profile_name}" ]]; then
    echo "$environment_name differs from the $EMAIL_PROFILE_PROFILE profile's $update_value." >&2
    exit 2
  fi
done
export EMAIL_UPDATE_CHANNEL="${EMAIL_UPDATE_CHANNEL:-stable}"
case "$EMAIL_UPDATE_CHANNEL" in stable|testing) ;; *) echo "EMAIL_UPDATE_CHANNEL must be stable or testing." >&2; exit 2 ;; esac
if [[ -n "$EMAIL_UPDATE_FEED_URL" && -z "$EMAIL_UPDATE_PUBLIC_ED_KEY" ]]; then
  echo "EMAIL_UPDATE_FEED_URL needs EMAIL_UPDATE_PUBLIC_ED_KEY." >&2
  exit 2
fi
if [[ "$mode" != --fixture && -n "$EMAIL_UPDATE_FEED_URL" && -z "$EMAIL_UPDATE_ALLOWED_HOSTS" ]]; then
  echo "The $EMAIL_PROFILE_PROFILE distribution profile has no UPDATE_ALLOWED_HOSTS; its builds cannot use an update feed." >&2
  exit 2
fi
export EMAIL_UPDATE_FIXTURE=0
if [[ "$mode" == --distribute || "$mode" == --beta ]]; then
  if [[ "$EMAIL_PROFILE_DISTRIBUTION" != true ]]; then
    echo "The $EMAIL_PROFILE_PROFILE distribution profile is not for distribution. Select official, or your fork's own profile." >&2
    exit 2
  fi
  : "${EMAIL_RELEASE_GOOGLE_CLIENT_ID:?Set the production Google OAuth desktop client ID}"
  : "${EMAIL_RELEASE_GOOGLE_CLIENT_SECRET:?Set the production Google desktop token-exchange value}"
  if ! EMAIL_RELEASE_GOOGLE_CLIENT_ID="$EMAIL_RELEASE_GOOGLE_CLIENT_ID" \
    python3 "$repo_root/scripts/distribution-profile.py" --root "$repo_root" check-release-google-client; then
    echo "Distribution cannot use the development Google OAuth project." >&2
    exit 2
  fi
  export EMAIL_GOOGLE_CLIENT_ID="$EMAIL_RELEASE_GOOGLE_CLIENT_ID"
  export EMAIL_GOOGLE_CLIENT_SECRET="$EMAIL_RELEASE_GOOGLE_CLIENT_SECRET"
  export EMAIL_RELEASE_BUNDLE_ID="$EMAIL_PROFILE_MACOS_BUNDLE_ID"
fi
if [[ "$mode" == --distribute ]]; then
  : "${EMAIL_RELEASE_CODESIGN_IDENTITY:?Set a Developer ID Application identity for distribution}"
  : "${EMAIL_NOTARY_PROFILE:?Set the existing notarytool Keychain profile name for distribution}"
  # A profile with update hosts must ship its feed; one without has no updater.
  if [[ -n "$EMAIL_UPDATE_ALLOWED_HOSTS" ]]; then
    : "${EMAIL_UPDATE_FEED_URL:?Set the production HTTPS appcast URL (UPDATE_FEED_URL in the profile or EMAIL_UPDATE_FEED_URL)}"
  fi
  signing_identity="$EMAIL_RELEASE_CODESIGN_IDENTITY"
  channel=distribution
elif [[ "$mode" == --beta ]]; then
  # Unnotarized beta, until the project has an Apple Developer ID: the
  # distribution profile, bundle ID and production Google client of
  # --distribute, signed with the project's persistent self-signed beta
  # certificate (scripts/macos-beta-signing.sh). No hardened runtime,
  # notarization or update feed. The launcher's same-signer library checks
  # apply unchanged; an ad-hoc signature cannot pass them.
  if [[ -n "${EMAIL_RELEASE_CODESIGN_IDENTITY:-}" || -n "${EMAIL_RELEASE_CODESIGN_KEYCHAIN:-}" ]]; then
    echo "Beta builds sign only with the identity from scripts/macos-beta-signing.sh; unset EMAIL_RELEASE_CODESIGN_IDENTITY and EMAIL_RELEASE_CODESIGN_KEYCHAIN." >&2
    exit 2
  fi
  if [[ -n "$EMAIL_UPDATE_FEED_URL" ]]; then
    echo "Beta builds are not notarized and have no updater; unset EMAIL_UPDATE_FEED_URL." >&2
    exit 2
  fi
  source "$repo_root/scripts/macos-beta-signing.sh"
  email_beta_signing_check || exit 2
  channel=beta
elif [[ "$mode" == --fixture ]]; then
  # Acceptance-test bundles: separate identifier, disposable signing keychain,
  # loopback feed only. The updater refuses fixture settings on any other ID.
  : "${EMAIL_RELEASE_CODESIGN_IDENTITY:?Fixture builds need a disposable identity (see email_create_fixture_signing)}"
  : "${EMAIL_RELEASE_CODESIGN_KEYCHAIN:?Fixture builds must sign from a disposable keychain file}"
  signing_identity="$EMAIL_RELEASE_CODESIGN_IDENTITY"
  export EMAIL_RELEASE_BUNDLE_ID="$EMAIL_UPDATE_FIXTURE_BUNDLE_ID"
  export EMAIL_UPDATE_FIXTURE=1
  channel=fixture
else
  signing_identity="${EMAIL_RELEASE_CODESIGN_IDENTITY:-$(cat "$repo_root/config/macos-signing-identity.local")}"
  # A separate identity: it never reuses the profile app's Keychain grants.
  export EMAIL_RELEASE_BUNDLE_ID="$EMAIL_PROFILE_MACOS_BUNDLE_ID.release-alpha"
  channel=local-alpha
fi
# Compile-time feature switches (crates/mail-cloud/build.rs). Distribution
# builds omit development-only Google scopes and Outlook. Google sign-in follows
# the profile's GOOGLE_OAUTH_PUBLIC: off in the official profile until maintainers
# turn it on there after Google approves the restricted-scope review.
# A beta is a public build and compiles with the distribution feature set.
if [[ "$channel" == beta ]]; then export EMAIL_RELEASE_CHANNEL=distribution; else export EMAIL_RELEASE_CHANNEL="$channel"; fi
email_resolve_google_oauth_public || exit 2
output="${EMAIL_RELEASE_OUTPUT_DIR:-$repo_root/dist-release/$channel}"
cargo_bin="$(command -v cargo || true)"
if [[ -z "$cargo_bin" ]]; then cargo_bin="$HOME/.cargo/bin/cargo"; fi
cd "$repo_root"
# Package from a separate target directory: rebuilding target/release under
# another profile would replace the MCP helper registered from this checkout.
export CARGO_TARGET_DIR="$repo_root/target/package-$channel"
"$cargo_bin" build --release --locked -p mail-service -p mail-mcp
printf 'import Foundation\nenum EmailBuildIdentity { static let value = "%s" }\n' "$(python3 "$repo_root/scripts/build-identity.py")" \
  > "$repo_root/apps/macos/Sources/EmailUI/BuildIdentity.generated.swift"
swift_args=(--package-path apps/macos --disable-sandbox -c release --only-use-versions-from-resolved-file)
swift build "${swift_args[@]}"
bin_dir="$(swift build "${swift_args[@]}" --show-bin-path)"
policy_check="$bin_dir/email-update-policy-checks"
sparkle_source="$bin_dir/Sparkle.framework"
sparkle_built="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$sparkle_source/Versions/B/Resources/Info.plist")"
if [[ "$sparkle_built" != "$EMAIL_SPARKLE_VERSION" ]]; then
  echo "Sparkle $sparkle_built does not match pinned $EMAIL_SPARKLE_VERSION." >&2
  exit 2
fi
mkdir -p "$output"
stage="$(mktemp -d "$output/.stage.XXXXXX")"
app="$stage/Email.app"
mkdir -p "$app/Contents/"{MacOS,Frameworks,Helpers,Resources}
install_package_notices "$repo_root" "$app/Contents/Resources/Legal"
swiftc -O -parse-as-library -target "$(uname -m)-apple-macosx13.0" "$repo_root/scripts/release/EmailReleaseLauncher.swift" -o "$app/Contents/MacOS/Email"
cp "$bin_dir/libEmailUI.dylib" "$bin_dir/libEmailUpdater.dylib" "$app/Contents/Frameworks/"
cp "$CARGO_TARGET_DIR/release/libmail_service.dylib" "$app/Contents/Frameworks/"
cp "$CARGO_TARGET_DIR/release/mail-mcp" "$app/Contents/Helpers/"
# ditto keeps the framework's symlinks and permissions, which Sparkle needs.
ditto "$sparkle_source" "$app/Contents/Frameworks/Sparkle.framework"
# XPC services exist for sandboxed hosts. Email is not sandboxed.
rm -rf "$app/Contents/Frameworks/Sparkle.framework/Versions/B/XPCServices" "$app/Contents/Frameworks/Sparkle.framework/XPCServices"
# Drop build-machine search paths; bundled code resolves via @loader_path.
for binary in "$app/Contents/Frameworks/libEmailUI.dylib" "$app/Contents/Frameworks/libEmailUpdater.dylib"; do
  otool -l "$binary" | awk '/cmd LC_RPATH/{getline; getline; print $2}' | while read -r rpath; do
    case "$rpath" in
      @loader_path*|@executable_path*|/usr/lib/swift) ;;
      *) install_name_tool -delete_rpath "$rpath" "$binary" ;;
    esac
  done
done
icon_set="$stage/Email.iconset"
mkdir -p "$icon_set"
swift "$repo_root/scripts/generate-macos-icon.swift" "$icon_set/icon_512x512@2x.png"
for size in 16 32 128 256 512; do
  sips -z "$size" "$size" "$icon_set/icon_512x512@2x.png" --out "$icon_set/icon_${size}x${size}.png" >/dev/null
  if [[ "$size" -lt 512 ]]; then
    double=$((size * 2))
    sips -z "$double" "$double" "$icon_set/icon_512x512@2x.png" --out "$icon_set/icon_${size}x${size}@2x.png" >/dev/null
  fi
done
iconutil -c icns "$icon_set" -o "$app/Contents/Resources/Email.icns"
python3 - "$app/Contents/Info.plist" <<'PY'
import os, plistlib, sys
bundle_id = os.environ['EMAIL_RELEASE_BUNDLE_ID']
display = {os.environ.get('EMAIL_UPDATE_FIXTURE_BUNDLE_ID', ''): 'Email Update Fixture'}
if bundle_id.endswith('.release-alpha'):
    display[bundle_id] = 'Email Local Alpha'
info = {
    'CFBundleIdentifier': bundle_id,
    'CFBundleName': 'Email', 'CFBundleDisplayName': display.get(bundle_id, 'Email'),
    'CFBundleExecutable': 'Email', 'CFBundleIconFile': 'Email',
    'CFBundlePackageType': 'APPL', 'CFBundleShortVersionString': os.environ['EMAIL_VERSION'],
    'CFBundleVersion': os.environ['EMAIL_BUILD_NUMBER'], 'LSMinimumSystemVersion': '13.0',
    'NSHighResolutionCapable': True, 'LSFileQuarantineEnabled': True,
    'EmailReleaseBuild': True,
    'NSContactsUsageDescription': 'Email shows matching photos from your Mac Contacts beside messages. Contacts data stays on your Mac.',
    # Sparkle: verify the EdDSA signature before extraction, require a signed
    # feed and never let feed signature failures expire. Background checks
    # stay off until the user agrees; silent install is opt-in.
    'SUVerifyUpdateBeforeExtraction': True,
    'SURequireSignedFeed': True,
    'SUSignedFeedFailureExpirationInterval': 0,
    'SUAllowsAutomaticUpdates': True,
    'EmailUpdateDefaultChannel': os.environ['EMAIL_UPDATE_CHANNEL'],
}
if os.environ.get('EMAIL_UPDATE_FEED_URL'):
    info['SUFeedURL'] = os.environ['EMAIL_UPDATE_FEED_URL']
    info['SUPublicEDKey'] = os.environ['EMAIL_UPDATE_PUBLIC_ED_KEY']
    # The updater refuses any feed host outside this signed list.
    info['EmailUpdateAllowedHosts'] = os.environ['EMAIL_UPDATE_ALLOWED_HOSTS'].split()
if os.environ.get('EMAIL_UPDATE_FIXTURE') == '1':
    info['EmailUpdateFixture'] = True
for environment, key in [('EMAIL_GOOGLE_CLIENT_ID', 'EmailGoogleOAuthClientID'),
                         ('EMAIL_GOOGLE_CLIENT_SECRET', 'EmailGoogleOAuthClientSecret'),
                         ('EMAIL_MICROSOFT_CLIENT_ID', 'EmailMicrosoftOAuthClientID')]:
    if os.environ.get(environment): info[key] = os.environ[environment]
with open(sys.argv[1], 'wb') as out: plistlib.dump(info, out)
PY
# Validate with the same Swift rules the shipped updater applies.
if [[ "$mode" == --distribute && -n "$EMAIL_UPDATE_ALLOWED_HOSTS" ]]; then
  "$policy_check" --validate-info "$app/Contents/Info.plist"
else
  "$policy_check" --validate-info "$app/Contents/Info.plist" --allow-unconfigured
fi
if [[ "$mode" == --beta ]]; then
  # Unlocked only for signing; the keychain locks again on exit.
  email_beta_signing_unlock || exit 2
  signing_identity="$EMAIL_RELEASE_CODESIGN_IDENTITY"
fi
sign_args=(--force --sign "$signing_identity")
codesign_home=""
if [[ -n "${EMAIL_RELEASE_CODESIGN_KEYCHAIN:-}" ]]; then
  sign_args+=(--keychain "$EMAIL_RELEASE_CODESIGN_KEYCHAIN")
  # Before macOS 27, codesign also needs the keychain file on a search list.
  # It gets a private one; the user's search list is not changed.
  codesign_home="$stage/codesign-home"
  email_codesign_home "$EMAIL_RELEASE_CODESIGN_KEYCHAIN" "$codesign_home"
fi
if [[ "$mode" == --distribute ]]; then sign_args+=(--options runtime --timestamp); else sign_args+=(--timestamp=none); fi
sign() {
  if [[ -n "$codesign_home" ]]; then
    HOME="$codesign_home" codesign "${sign_args[@]}" "$@"
  else
    codesign "${sign_args[@]}" "$@"
  fi
}
# Inside-out, without --deep, as Sparkle's documentation requires.
sparkle="$app/Contents/Frameworks/Sparkle.framework"
sign "$sparkle/Versions/B/Autoupdate"
sign "$sparkle/Versions/B/Updater.app"
sign "$sparkle"
for binary in "$app/Contents/Frameworks/libEmailUI.dylib" "$app/Contents/Frameworks/libEmailUpdater.dylib" \
              "$app/Contents/Frameworks/libmail_service.dylib" "$app/Contents/Helpers/mail-mcp"; do
  sign --identifier "$EMAIL_RELEASE_BUNDLE_ID" "$binary"
done
sign "$app"
codesign --verify --deep --strict "$app"
if [[ "$mode" == --beta ]]; then
  # Fail closed: the app must carry the beta certificate, never an ad-hoc signature.
  codesign --verify --strict -R "=certificate leaf = H\"$EMAIL_BETA_CERTIFICATE_SHA1\"" "$app"
  email_beta_signing_lock
fi
if [[ "$mode" == --distribute ]]; then
  # Fail closed: local/ad-hoc signing must never masquerade as a public release.
  codesign --verify --strict -R '=anchor apple generic and certificate leaf[field.1.2.840.113635.100.6.1.13] exists' "$app"
  ditto -c -k --keepParent "$app" "$stage/Email-notarization.zip"
  xcrun notarytool submit "$stage/Email-notarization.zip" --keychain-profile "$EMAIL_NOTARY_PROFILE" --wait
  xcrun stapler staple "$app"
  xcrun stapler validate "$app"
  spctl --assess --type execute --verbose=2 "$app"
fi
# Preserve the development bundle. Only replace this channel's previous output.
rm -rf "$output/Email.app"
mv "$app" "$output/Email.app"
app="$output/Email.app"
# Sparkle update archive: the signed (and, for distribution, stapled) app.
update_archive="$output/Email-$EMAIL_VERSION-$EMAIL_BUILD_NUMBER-$(uname -m).zip"
rm -f "$output"/Email-*.zip "$output"/Email-*.zip.sha256
ditto -c -k --sequesterRsrc --keepParent "$app" "$update_archive"
if [[ "$mode" == --fixture ]]; then
  printf 'UPDATE FIXTURE ONLY: disposable signature, loopback feed, not for distribution.\n' > "$output/STATUS.txt"
  printf 'Built %s\n' "$update_archive"
  exit 0
fi
if [[ "$mode" == --beta ]]; then
  # The zip is the beta download. Testers install new betas by hand.
  (cd "$output" && shasum -a 256 "$(basename "$update_archive")") > "$update_archive.sha256"
  printf 'BETA, NOT NOTARIZED: signed with the self-signed beta certificate (SHA-1 %s), not with an Apple Developer ID. Gatekeeper blocks the first launch until the user chooses Open Anyway in System Settings > Privacy & Security. No automatic updates.\n' \
    "$EMAIL_BETA_CERTIFICATE_SHA1" > "$output/STATUS.txt"
  printf 'Built %s\n' "$update_archive"
  exit 0
fi
volume_name="Email"
if [[ "$mode" == --local-alpha ]]; then volume_name="Email Local Alpha"; fi
mkdir -p "$stage/dmg"
cp -R "$app" "$stage/dmg/Email.app"
ln -s /Applications "$stage/dmg/Applications"
if [[ "$mode" == --local-alpha ]]; then
  printf 'This is a locally signed development alpha. It is not notarized by Apple and is not a public release. Its separate bundle ID does not reuse the development app account-token grants.\n' > "$stage/dmg/LOCAL-ALPHA.txt"
fi
dmg="$output/Email-$EMAIL_VERSION-$(uname -m).dmg"
hdiutil create -quiet -volname "$volume_name" -srcfolder "$stage/dmg" -ov -format UDZO "$dmg"
if [[ "$mode" == --distribute ]]; then
  codesign --force --sign "$signing_identity" --timestamp "$dmg"
  xcrun notarytool submit "$dmg" --keychain-profile "$EMAIL_NOTARY_PROFILE" --wait
  xcrun stapler staple "$dmg"
  xcrun stapler validate "$dmg"
fi
(cd "$output" && shasum -a 256 "$(basename "$dmg")" "$(basename "$update_archive")") > "$dmg.sha256"
if [[ "$mode" == --local-alpha ]]; then
  printf 'LOCAL ALPHA ONLY: locally signed, not Apple-notarized, not for public distribution.\n' > "$output/STATUS.txt"
else
  printf 'Developer ID signed and notarized by Apple. Functional release approval is still required.\n' > "$output/STATUS.txt"
fi
printf 'Built %s\n' "$dmg"
