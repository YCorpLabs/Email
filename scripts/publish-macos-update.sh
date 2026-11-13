#!/usr/bin/env bash
# Adds a signed Email build to a local appcast staging directory and verifies
# the result. It never uploads: copy the verified directory to the update host
# as a separate, reviewed step.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$repo_root/scripts/macos-updates-env.sh"
source "$repo_root/scripts/distribution-profile.sh"

usage() {
  cat >&2 <<'EOF'
Usage: publish-macos-update.sh --archive Email-X-N-arch.zip --channel stable|testing \
         --feed-dir DIR --download-prefix URL --ed-key-file PATH|- [--notes FILE.md] [--fixture]

--ed-key-file -   reads the private EdDSA key from EMAIL_UPDATE_ED_PRIVATE_KEY.
                  The login Keychain is never used.
--fixture         loopback acceptance tests only (fixture bundle ID, http://127.0.0.1).
EOF
  exit 2
}

archive="" channel="" feed_dir="" prefix="" key_file="" notes="" fixture=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --archive) archive="$2"; shift 2 ;;
    --channel) channel="$2"; shift 2 ;;
    --feed-dir) feed_dir="$2"; shift 2 ;;
    --download-prefix) prefix="$2"; shift 2 ;;
    --ed-key-file) key_file="$2"; shift 2 ;;
    --notes) notes="$2"; shift 2 ;;
    --fixture) fixture=1; shift ;;
    *) usage ;;
  esac
done
[[ -f "$archive" && -n "$feed_dir" && -n "$prefix" && -n "$key_file" ]] || usage
case "$channel" in stable|testing) ;; *) usage ;; esac
[[ "$prefix" == */ ]] || prefix="$prefix/"
if [[ "$key_file" == - ]]; then
  : "${EMAIL_UPDATE_ED_PRIVATE_KEY:?Set EMAIL_UPDATE_ED_PRIVATE_KEY when using --ed-key-file -}"
elif [[ ! -f "$key_file" ]]; then
  echo "Private key file not found." >&2; exit 2
fi

prefix_host="$(python3 -c 'import sys,urllib.parse as u; p=u.urlparse(sys.argv[1]); print(p.scheme, p.hostname, p.port or "")' "$prefix")"
read -r prefix_scheme prefix_hostname prefix_port <<<"$prefix_host"
if [[ "$fixture" == 1 ]]; then
  [[ "$prefix_hostname" == 127.0.0.1 ]] || { echo "Fixture downloads must use 127.0.0.1." >&2; exit 2; }
else
  # Production identity and update hosts come only from a distribution profile.
  [[ "$(email_profile_value "$repo_root" DISTRIBUTION)" == true ]] || { echo "Production publishing requires a distribution profile, such as official." >&2; exit 2; }
  production_bundle_id="$(email_profile_value "$repo_root" MACOS_BUNDLE_ID)"
  production_hosts="$(email_profile_value "$repo_root" UPDATE_ALLOWED_HOSTS)"
  [[ -n "$production_hosts" ]] || { echo "The distribution profile has no UPDATE_ALLOWED_HOSTS." >&2; exit 2; }
  [[ "$prefix_scheme" == https && -z "${prefix_port}" ]] || { echo "Production downloads must use HTTPS on the default port." >&2; exit 2; }
  [[ " $production_hosts " == *" $prefix_hostname "* ]] || { echo "Download host $prefix_hostname is not approved." >&2; exit 2; }
fi

scratch="$(mktemp -d "${TMPDIR:-/tmp}/email-publish.XXXXXX")"
tools=""
staged_feed="" feed_backup="" feed_lock="" feed_destination=""
cleanup() {
  if [[ -n "$feed_backup" && -d "$feed_backup" && ! -e "$feed_destination" ]]; then
    mv "$feed_backup" "$feed_destination"
  fi
  rm -rf "$scratch"
  if [[ -n "$tools" ]]; then rm -rf "$tools"; fi
  if [[ -n "$staged_feed" ]]; then rm -rf "$staged_feed"; fi
  if [[ -n "$feed_lock" ]]; then rmdir "$feed_lock"; fi
}
trap cleanup EXIT
ditto -x -k "$archive" "$scratch/extract"
app="$scratch/extract/Email.app"
[[ -d "$app" ]] || { echo "Archive must contain Email.app at its root." >&2; exit 2; }
plist="$app/Contents/Info.plist"
bundle_id="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$plist")"
build="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleVersion' "$plist")"
[[ "$build" =~ ^[1-9][0-9]{0,8}$ ]] || { echo "CFBundleVersion $build is not a positive integer." >&2; exit 2; }
packaged_channel="$(/usr/libexec/PlistBuddy -c 'Print :EmailUpdateDefaultChannel' "$plist" 2>/dev/null || echo stable)"
if [[ "$channel" == stable && "$packaged_channel" != stable ]]; then
  echo "A testing-channel build cannot be published to stable." >&2; exit 2
fi

# The archive must carry an update configuration the shipped app accepts.
swift build --package-path "$repo_root/apps/macos" --disable-sandbox -c release --only-use-versions-from-resolved-file \
  --product email-update-policy-checks >/dev/null
policy_bin="$(swift build --package-path "$repo_root/apps/macos" --disable-sandbox -c release --only-use-versions-from-resolved-file --show-bin-path)"
"$policy_bin/email-update-policy-checks" --validate-info "$plist"

codesign --verify --deep --strict "$app"
if [[ "$fixture" == 1 ]]; then
  [[ "$bundle_id" == "$EMAIL_UPDATE_FIXTURE_BUNDLE_ID" ]] || { echo "Fixture publishing requires the fixture bundle ID." >&2; exit 2; }
else
  [[ "$bundle_id" == "$production_bundle_id" ]] || { echo "Production publishing requires bundle ID $production_bundle_id, not $bundle_id." >&2; exit 2; }
  # Never publish a build that is not Developer ID signed and notarized.
  codesign --verify --strict -R '=anchor apple generic and certificate leaf[field.1.2.840.113635.100.6.1.13] exists' "$app"
  xcrun stapler validate "$app"
  spctl --assess --type execute --verbose=2 "$app"
fi

[[ ! -L "$feed_dir" ]] || { echo "Feed staging directory must not be a symlink." >&2; exit 2; }
mkdir -p "$feed_dir"
feed_destination="$(cd "$feed_dir" && pwd)"
if ! mkdir "$feed_destination.publish-lock"; then
  echo "Another publisher owns this feed. If a prior run crashed, review and remove its empty .publish-lock directory before retrying." >&2
  exit 2
fi
feed_lock="$feed_destination.publish-lock"
# Keep the last verified output untouched until the replacement is signed and
# fully checked. A sibling directory keeps the final rename on one filesystem.
staged_feed="$(mktemp -d "$(dirname "$feed_destination")/.email-feed.XXXXXX")"
ditto "$feed_destination" "$staged_feed"
feed_dir="$staged_feed"
appcast="$feed_dir/appcast.xml"
newest_existing=0
if [[ -f "$appcast" ]]; then
  newest_existing="$(python3 -I - "$appcast" <<'PY'
import re, sys
text = open(sys.argv[1], encoding='utf-8').read()
builds = [int(v) for v in re.findall(r'<sparkle:version>([0-9]+)</sparkle:version>', text)]
print(max(builds, default=0))
PY
)"
fi
if (( build <= newest_existing )); then
  echo "Build $build is not newer than published build $newest_existing." >&2; exit 2
fi

archive_name="$(basename "$archive")"
cp "$archive" "$feed_dir/$archive_name"
if [[ -n "$notes" ]]; then cp "$notes" "$feed_dir/${archive_name%.zip}.md"; fi
tools="$(email_sparkle_tools)"
generate=("$tools/bin/generate_appcast" --download-url-prefix "$prefix" --maximum-versions 0)
if [[ "$channel" == testing ]]; then generate+=(--channel testing); fi
if [[ "$key_file" == - ]]; then
  printf '%s' "$EMAIL_UPDATE_ED_PRIVATE_KEY" | "${generate[@]}" --ed-key-file - "$feed_dir"
else
  "${generate[@]}" --ed-key-file "$key_file" "$feed_dir"
fi

public_key="$(/usr/libexec/PlistBuddy -c 'Print :SUPublicEDKey' "$plist")"
if [[ "$fixture" != 1 ]]; then
  profile_key="$(email_profile_value "$repo_root" UPDATE_PUBLIC_ED_KEY)"
  if [[ -n "$profile_key" && "$public_key" != "$profile_key" ]]; then
    echo "The archive's update key differs from the distribution profile's UPDATE_PUBLIC_ED_KEY." >&2
    exit 2
  fi
fi
verify_args=("$appcast" --public-key "$public_key" --archives "$feed_dir" --newest-channel "$channel")
if [[ "$fixture" != 1 ]]; then verify_args+=(--production "$production_hosts"); fi
python3 -I "$repo_root/scripts/verify-macos-appcast.py" "${verify_args[@]}"
feed_backup="$(mktemp -d "$(dirname "$feed_destination")/.email-feed-previous.XXXXXX")"
rmdir "$feed_backup"
mv "$feed_destination" "$feed_backup"
mv "$staged_feed" "$feed_destination"
staged_feed=""
rm -rf "$feed_backup"
feed_backup=""
printf 'Staged build %s on %s in %s. Not uploaded.\n' "$build" "$channel" "$feed_destination"
