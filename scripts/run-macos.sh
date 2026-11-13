#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$repo_root/scripts/distribution-profile.sh"
# Keep cache metadata outside the signed app. This helper never builds, signs,
# launches, or writes the app, including during explicit legacy migration.
launcher_metadata() {
  python3 - "$repo_root" "$@" <<'PYLAUNCHER'
import hashlib, json, subprocess, sys, tempfile
from pathlib import Path

root = Path(sys.argv[1])
action = sys.argv[2]
cache = root / 'dist/.launcher-cache'
app = root / 'dist/Email.app'
executable = app / 'Contents/MacOS/Email'
metadata = cache / 'contract-v2.json'
main = 'apps/macos/Sources/EmailDesktop/main.swift'
manifest = 'apps/macos/Package.swift'
def digest(data):
    return hashlib.sha256(data).hexdigest()
def run(*args):
    return subprocess.check_output(args, text=True, stderr=subprocess.STDOUT).strip()
def contract(package_dir, source):
    package = json.loads(run('swift', 'package', '--package-path', str(package_dir), 'dump-package'))
    target, = [t for t in package['targets'] if t['name'] == 'EmailDesktop']
    product, = [p for p in package['products'] if p['name'] == 'email-desktop']
    # This contract supports only the standalone launcher. Dependencies, plugins,
    # resources, additional sources or custom paths need a deliberate new model.
    if (target.get('dependencies') or target.get('resources') or target.get('pluginUsages')
            or target.get('path') or target.get('sources') or target.get('exclude')
            or target['type'] != 'executable'
            or product['targets'] != ['EmailDesktop'] or product['type'] != {'executable': None}):
        raise ValueError('Launcher layout/link dependencies changed; review the launcher contract.')
    # Retain all other package-wide fields, including future fields, fail closed.
    relevant = {k: v for k, v in package.items()
                if k not in ('targets', 'products', 'dependencies', 'packageKind')}
    relevant.update(target=target, product=product, mainSHA256=digest(source))
    return digest(json.dumps(relevant, sort_keys=True, separators=(',', ':')).encode())
def current_contract():
    sources = root / 'apps/macos/Sources/EmailDesktop'
    if sorted(p.relative_to(sources).as_posix() for p in sources.rglob('*') if p.is_file()) != ['main.swift']:
        raise ValueError('Launcher source layout changed; review the launcher contract.')
    return contract(root / 'apps/macos', (root / main).read_bytes())
def write_record(value):
    cache.mkdir(parents=True, exist_ok=True)
    cache.chmod(0o700)
    temporary = cache / 'contract-v2.json.pending'
    temporary.write_text(json.dumps(value, indent=2) + '\n')
    temporary.chmod(0o600)
    temporary.replace(metadata)
try:
    current = current_contract()
    if action == 'write':
        if (cache / 'Email').read_bytes() != executable.read_bytes():
            raise ValueError('Launcher cache differs from the packaged executable.')
        write_record(dict(version=2, contractSHA256=current, executableSHA256=digest(executable.read_bytes())))
    elif action == 'check':
        record = json.loads(metadata.read_text())
        if record['version'] != 2 or record['contractSHA256'] != current:
            raise ValueError('Launcher inputs changed.')
        if digest((cache / 'Email').read_bytes()) != record['executableSHA256']:
            raise ValueError('Launcher cache bytes differ from metadata.')
        if executable.exists() and digest(executable.read_bytes()) != record['executableSHA256']:
            raise ValueError('Installed launcher bytes differ from metadata.')
    elif action == 'prepare':
        expected = sys.argv[3]
        if len(expected) != 40 or any(c not in '0123456789abcdef' for c in expected):
            raise ValueError('Supply the independently recorded 40-character CDHash.')
        if metadata.exists():
            raise ValueError('Version 2 metadata already exists; inspect it instead of overwriting it.')
        run('codesign', '--verify', '--deep', '--strict', str(app))
        identity = (root / 'config/macos-signing-identity.local').read_text().strip().lower()
        if len(identity) != 40 or any(c not in '0123456789abcdef' for c in identity):
            raise ValueError('Expected a persistent local signing certificate fingerprint.')
        requirement = 'identifier "' + sys.argv[4] + '" and certificate leaf = H"' + identity + '"'
        run('codesign', '--verify', '--strict', '-R', '=' + requirement, str(app))
        details = run('codesign', '-dv', '--verbose=4', str(app))
        hashes = [line.removeprefix('CDHash=') for line in details.splitlines() if line.startswith('CDHash=')]
        if hashes != [expected]:
            raise ValueError('Installed CDHash differs from the independently recorded value.')
        binary = executable.read_bytes()
        if (cache / 'Email').read_bytes() != binary:
            raise ValueError('Legacy cache and installed launcher differ.')
        provenance = json.loads((root / 'dist/email-build.json').read_text())
        if provenance['artifacts']['Email.app/Contents/MacOS/Email'] != digest(binary):
            raise ValueError('Installed launcher differs from build provenance.')
        commit = provenance['gitCommit']
        # Validate the legacy cache against its recorded commit, not current HEAD.
        # Its old hash included absolute filenames as emitted by shasum.
        old = {}
        for name in (main, manifest):
            old[name] = subprocess.check_output(['git', '-C', str(root), 'show', f'{commit}:{name}'])
        legacy = ''.join(f'{digest(old[name])}  {root / name}\n' for name in (main, manifest))
        if (cache / 'source-hash').read_text().strip() != digest(legacy.encode()):
            raise ValueError('Legacy source hash does not match the provenance commit at this path.')
        paths = run('git', '-C', str(root), 'ls-tree', '-r', '--name-only', commit, 'apps/macos/Sources/EmailDesktop').splitlines()
        if paths != [main]:
            raise ValueError('Historical launcher source layout is unsupported.')
        with tempfile.TemporaryDirectory(prefix='email-launcher-manifest-') as temporary:
            historical = Path(temporary)
            (historical / 'Package.swift').write_bytes(old[manifest])
            previous = contract(historical, old[main])
        if previous != current:
            raise ValueError('Current launcher inputs differ from the validated historical inputs.')
        write_record(dict(version=2, contractSHA256=current, executableSHA256=digest(binary),
                          migratedFromCommit=commit, preservedCDHash=expected))
        print(f'Prepared launcher metadata; preserved executable and CDHash {expected}')
    else:
        raise ValueError('Unknown launcher metadata action.')
except (ValueError, KeyError, OSError, subprocess.CalledProcessError) as error:
    raise SystemExit(f'Launcher cache: {error}')
PYLAUNCHER
}
if [[ "${1:-}" == "--prepare-launcher-cache" && $# == 2 ]]; then
  # Only the bundle ID is read from the distribution profile; no OAuth values.
  prepare_bundle_id="$(python3 "$repo_root/scripts/distribution-profile.py" --root "$repo_root" get MACOS_BUNDLE_ID)"
  launcher_metadata prepare "$2" "$prepare_bundle_id"
  exit
fi
if [[ "${1:-}" == "--check-launcher-cache" && $# == 1 ]]; then
  launcher_metadata check
  exit
fi
# The distribution profile names the app identity and services; the default is
# the neutral `unofficial` profile. Development builds select `development`
# (or a local copy of it) in config/distribution-profile.local. OAuth application values
# come from the environment or the ignored config/oauth-desktop.local.env.
# A Google Desktop client may require its generated client secret at the token
# endpoint. It cannot be confidential in a shipped desktop app; PKCE protects
# the authorization code. Keep local build values out of source control.
email_load_distribution_profile "$repo_root"
mode="${1:-run}"
if [[ "$mode" != "run" && "$mode" != "--smoke" && "$mode" != "--package" ]]; then
  echo "Usage: scripts/run-macos.sh [--smoke|--package|--check-launcher-cache|--prepare-launcher-cache CDHASH]" >&2
  exit 2
fi
if [[ "$mode" != "--smoke" ]]; then
  email_refuse_distribution_profile
  email_require_google_client_id "$repo_root"
fi

# Never repackage an existing app under another profile's identity: its
# Keychain items, data and launcher signature belong to that identity.
if [[ "$mode" != "--smoke" && -f "$repo_root/dist/Email.app/Contents/Info.plist" ]]; then
  existing_bundle_id="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$repo_root/dist/Email.app/Contents/Info.plist" 2>/dev/null || true)"
  if [[ "$existing_bundle_id" != "$EMAIL_PROFILE_MACOS_BUNDLE_ID" ]]; then
    echo "dist/Email.app is ${existing_bundle_id:-unidentified}, but the $EMAIL_PROFILE_PROFILE distribution profile builds $EMAIL_PROFILE_MACOS_BUNDLE_ID." >&2
    echo "Select the matching profile in config/distribution-profile.local, or move dist/Email.app aside first." >&2
    exit 1
  fi
fi

# A legacy hash alone cannot establish the new contract. Require explicit,
# evidence-backed preparation before an ordinary installed-app rebuild.
if [[ "$mode" != "--smoke" && -d "$repo_root/dist/Email.app" \
  && ! -f "$repo_root/dist/.launcher-cache/contract-v2.json" ]]; then
  echo "Prepare legacy launcher metadata with --prepare-launcher-cache CDHASH before packaging." >&2
  exit 1
fi
# Resolve the persistent local identity before building, so a missing identity
# cannot leave a partial unsigned dist/Email.app behind.
if [[ -f "$repo_root/config/macos-signing-identity.local" ]]; then
  local_signing_identity="$(cat "$repo_root/config/macos-signing-identity.local")"
else
  local_signing_identity=""
fi
signing_identity="${EMAIL_CODESIGN_IDENTITY:-$local_signing_identity}"
if [[ "$mode" != "--smoke" && -z "$signing_identity" ]]; then
  echo "Run scripts/setup-macos-signing.sh once to create a local signing identity." >&2
  echo "Or run scripts/dev.sh for an unsigned development build with demo data (docs/building.md)." >&2
  exit 1
fi
cd "$repo_root"
if command -v cargo >/dev/null 2>&1; then
  cargo_bin="$(command -v cargo)"
elif [[ -x "$HOME/.cargo/bin/cargo" ]]; then
  cargo_bin="$HOME/.cargo/bin/cargo"
else
  echo "cargo was not found. Install Rust or add cargo to PATH." >&2
  exit 127
fi
rust_library="$repo_root/target/debug/libmail_service.dylib"
profile_args=()
if [[ "$mode" != "--smoke" ]]; then
  profile_args+=(--release)
  rust_library="$repo_root/target/release/libmail_service.dylib"
fi
"$cargo_bin" build -p mail-service ${profile_args[@]+"${profile_args[@]}"}

build_identity="$(python3 "$repo_root/scripts/build-identity.py")"
printf 'import Foundation\nenum EmailBuildIdentity { static let value = "%s" }\n' "$build_identity" > "$repo_root/apps/macos/Sources/EmailUI/BuildIdentity.generated.swift"
cd "$repo_root/apps/macos"
mkdir -p .swiftpm-cache .clang-cache
export CLANG_MODULE_CACHE_PATH="$PWD/.clang-cache"

swift_args=(--disable-sandbox --cache-path "$PWD/.swiftpm-cache" --manifest-cache local)
if [[ "$mode" != "--smoke" ]]; then
  swift_args+=(-c release)
fi
host_sdk=/Library/Developer/CommandLineTools/SDKs/MacOSX26.5.sdk
if [[ -d "$host_sdk" ]]; then
  swift_args+=(--sdk "$host_sdk")
fi
swift build "${swift_args[@]}"
bin_dir="$(swift build --show-bin-path "${swift_args[@]}")"

export MAIL_SERVICE_LIBRARY="${MAIL_SERVICE_LIBRARY:-$rust_library}"
if [[ "$mode" == "--smoke" ]]; then
  if [[ -z "${EMAIL_STORE_PATH:-}" ]]; then
    smoke_dir="$(mktemp -d)"
    trap 'rm -rf "$smoke_dir"' EXIT
    export EMAIL_STORE_PATH="$smoke_dir/mail.sqlite"
  fi
  "$bin_dir/email-desktop" --smoke-service
  exit
fi

app_dir="$repo_root/dist/Email.app"
# Preserve a recoverable signed bundle before touching any sealed files.
app_backup="$(mktemp -d)"
original_app_hash=""
if [[ -d "$app_dir" ]] && codesign --verify --deep --strict "$app_dir" >/dev/null 2>&1; then
  ditto "$app_dir" "$app_backup/Email.app"
  original_app_hash="$(codesign -dv --verbose=4 "$app_dir" 2>&1 | sed -n 's/^CDHash=//p' | head -1)"
fi
restore_failed_package() {
  local result=$?
  if [[ "$result" != 0 && -n "$original_app_hash" ]]; then
    rm -rf "$app_dir"
    ditto "$app_backup/Email.app" "$app_dir"
    echo "Restored the prior signed Email.app after packaging failed." >&2
  fi
  rm -rf "$app_backup"
}
trap restore_failed_package EXIT
runtime_dir="$repo_root/dist/Runtime"
mkdir -p "$app_dir/Contents/MacOS" "$app_dir/Contents/Resources" "$runtime_dir"
icon_set="$repo_root/dist/.Email.iconset"
mkdir -p "$icon_set"
icon_swift_args=()
if [[ -d "$host_sdk" ]]; then icon_swift_args+=(-sdk "$host_sdk"); fi
swift ${icon_swift_args[@]+"${icon_swift_args[@]}"} "$repo_root/scripts/generate-macos-icon.swift" "$icon_set/icon_512x512@2x.png"
for icon_size in 16 32 128 256 512; do
  sips -s format png -z "$icon_size" "$icon_size" "$icon_set/icon_512x512@2x.png" \
    --out "$icon_set/icon_${icon_size}x${icon_size}.png" >/dev/null
  if [[ "$icon_size" -lt 512 ]]; then
    doubled_size=$((icon_size * 2))
    sips -s format png -z "$doubled_size" "$doubled_size" "$icon_set/icon_512x512@2x.png" \
      --out "$icon_set/icon_${icon_size}x${icon_size}@2x.png" >/dev/null
  fi
done
iconutil -c icns "$icon_set" -o "$app_dir/Contents/Resources/Email.icns"
# Earlier builds bundled a third-party logo pack; remove it from reused bundles.
rm -rf "$app_dir/Contents/Resources/BrandLogos"
rm -f "$app_dir/Contents/Frameworks/libEmailUI.dylib" "$app_dir/Contents/Frameworks/libmail_service.dylib"
# Never overwrite a mapped Mach-O inode: macOS can retain its previous code
# validation state and kill a later launch even when on-disk verification passes.
# Keep an identical launcher untouched; publish changed binaries on fresh inodes.
copy_macho_atomically() {
  local source="$1" destination="$2" temporary
  if [[ -f "$destination" ]] && cmp -s "$source" "$destination"; then return; fi
  temporary="$(mktemp "${destination}.pending.XXXXXX")"
  cp -p "$source" "$temporary"
  mv -f "$temporary" "$destination"
}
launcher_cache_dir="$repo_root/dist/.launcher-cache"
# Fingerprint only inputs which affect the executable's source or link graph.
# Package products/other targets do not participate in EmailDesktop's link.
if launcher_metadata check; then
  # The cache can be an earlier signing of the same code. Prefer the actual
  # verified installed launcher so cached signatures cannot reset its grants.
  if [[ -z "$original_app_hash" ]]; then
    copy_macho_atomically "$launcher_cache_dir/Email" "$app_dir/Contents/MacOS/Email"
  fi
  reuse_launcher=true
else
  copy_macho_atomically "$bin_dir/email-desktop" "$app_dir/Contents/MacOS/Email"
  reuse_launcher=false
fi
copy_macho_atomically "$bin_dir/libEmailUI.dylib" "$runtime_dir/libEmailUI.dylib"
copy_macho_atomically "$rust_library" "$runtime_dir/libmail_service.dylib"
cat > "$app_dir/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>$EMAIL_PROFILE_MACOS_BUNDLE_ID</string>
<key>CFBundleName</key><string>Email</string>
<key>CFBundleDisplayName</key><string>Email</string>
<key>CFBundleExecutable</key><string>Email</string>
<key>CFBundleIconFile</key><string>Email</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleVersion</key><string>1</string>
<key>CFBundleShortVersionString</key><string>0.1</string>
<key>NSHighResolutionCapable</key><true/>
<key>LSFileQuarantineEnabled</key><true/>
<key>NSContactsUsageDescription</key><string>Email shows matching photos from your Mac Contacts beside messages. Contacts data stays on your Mac.</string>
</dict></plist>
EOF
if [[ -n "${EMAIL_GOOGLE_CLIENT_ID:-}" ]]; then
  /usr/libexec/PlistBuddy -c "Add :EmailGoogleOAuthClientID string $EMAIL_GOOGLE_CLIENT_ID" "$app_dir/Contents/Info.plist"
fi
if [[ -n "${EMAIL_GOOGLE_CLIENT_SECRET:-}" ]]; then
  /usr/libexec/PlistBuddy -c "Add :EmailGoogleOAuthClientSecret string $EMAIL_GOOGLE_CLIENT_SECRET" "$app_dir/Contents/Info.plist"
fi
if [[ -n "${EMAIL_MICROSOFT_CLIENT_ID:-}" ]]; then
  /usr/libexec/PlistBuddy -c "Add :EmailMicrosoftOAuthClientID string $EMAIL_MICROSOFT_CLIENT_ID" "$app_dir/Contents/Info.plist"
fi
# The Swift build signs its standalone executable. Sign the assembled bundle
# with the persistent local identity resolved above.
codesign --force --sign "$signing_identity" --identifier "$EMAIL_PROFILE_MACOS_BUNDLE_ID" "$runtime_dir/libEmailUI.dylib"
codesign --force --sign "$signing_identity" --identifier "$EMAIL_PROFILE_MACOS_BUNDLE_ID" "$runtime_dir/libmail_service.dylib"
codesign --verify --strict "$runtime_dir/libEmailUI.dylib"
codesign --verify --strict "$runtime_dir/libmail_service.dylib"
if [[ "$reuse_launcher" == true && -z "$original_app_hash" ]] && ! codesign --verify --strict "$app_dir/Contents/MacOS/Email" >/dev/null 2>&1; then
  copy_macho_atomically "$bin_dir/email-desktop" "$app_dir/Contents/MacOS/Email"
  reuse_launcher=false
fi
if [[ "$reuse_launcher" == false ]]; then
  codesign --force --sign "$signing_identity" "$app_dir/Contents/MacOS/Email"
fi
# A login Keychain item may authorize this app by its code hash. Re-signing the
# bundle after an ordinary UI or mail-engine rebuild needlessly changes that
# hash and makes macOS ask for the account token again. The runtime libraries
# live outside the sealed bundle; if its existing seal and identity still
# verify, leave the app signature untouched.
reuse_bundle_signature=false
if [[ "$signing_identity" == "$local_signing_identity" ]] \
  && codesign --verify --deep --strict "$app_dir" >/dev/null 2>&1 \
  && codesign -dr - "$app_dir" 2>&1 | grep -Fq "certificate leaf = H\"$(printf '%s' "$local_signing_identity" | tr '[:upper:]' '[:lower:]')\""; then
  reuse_bundle_signature=true
fi
if [[ "$reuse_bundle_signature" == false ]]; then
  codesign --force --sign "$signing_identity" "$app_dir"
fi
codesign --verify --deep --strict "$app_dir"
final_app_hash="$(codesign -dv --verbose=4 "$app_dir" 2>&1 | sed -n 's/^CDHash=//p' | head -1)"
if [[ -n "$original_app_hash" && "$final_app_hash" != "$original_app_hash" && "${EMAIL_ALLOW_LAUNCHER_CHANGE:-0}" != 1 ]]; then
  echo "Packaging changed Email's Keychain identity. Refusing launch and restoring the prior app." >&2
  echo "Use EMAIL_ALLOW_LAUNCHER_CHANGE=1 only for an intentional launcher/resource update." >&2
  exit 1
fi
if [[ -n "$original_app_hash" && "$final_app_hash" == "$original_app_hash" ]]; then
  echo "Preserved exact Email.app code hash: $final_app_hash"
fi
# Rebuilding in place otherwise leaves Finder displaying an older icon.
launch_services=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister
# A first build was never registered; unregistering it reports an error.
"$launch_services" -u "$app_dir" 2>/dev/null || true
"$launch_services" -f "$app_dir"
touch "$app_dir"
if [[ "$reuse_launcher" == false ]]; then
  mkdir -p "$launcher_cache_dir"
  chmod 700 "$launcher_cache_dir"
  cp "$app_dir/Contents/MacOS/Email" "$launcher_cache_dir/Email"
  chmod 700 "$launcher_cache_dir/Email"
  launcher_metadata write
fi
# Provenance stays outside the sealed app so routine rebuilds preserve its trust.
python3 - "$repo_root" "$build_identity" <<'PYBUILD'
import hashlib, json, subprocess, sys
from pathlib import Path
root = Path(sys.argv[1])
files = ['Email.app/Contents/MacOS/Email', 'Runtime/libEmailUI.dylib', 'Runtime/libmail_service.dylib']
record = {
    'buildIdentity': sys.argv[2],
    'gitCommit': subprocess.check_output(['git', '-C', str(root), 'rev-parse', 'HEAD'], text=True).strip(),
    'artifacts': {name: hashlib.sha256((root / 'dist' / name).read_bytes()).hexdigest() for name in files},
}
(root / 'dist/email-build.json').write_text(json.dumps(record, indent=2) + '\n')
PYBUILD
echo "Built $app_dir"
if [[ "$mode" == "--package" ]]; then exit 0; fi
if [[ -z "${EMAIL_STORE_PATH:-}" ]] && open -n "$app_dir"; then exit 0; fi
exec "$bin_dir/email-desktop"
