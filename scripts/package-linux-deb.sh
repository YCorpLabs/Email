#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"
source "$repo_root/scripts/package-notices.sh"
source "$repo_root/scripts/linux-desktop-entry.sh"
source "$repo_root/scripts/release-version.sh"
require_package_notices "$repo_root"
source "$repo_root/scripts/linux-build-config.sh"
require_linux_oauth_config

command -v dpkg-deb >/dev/null || { echo "dpkg-deb is required" >&2; exit 1; }
command -v dpkg-shlibdeps >/dev/null || { echo "dpkg-shlibdeps is required" >&2; exit 1; }

cargo build --manifest-path apps/linux/Cargo.toml --release --locked
# The MCP helper that Agent access connects Claude Code and Codex to. It is in
# the root workspace, which must not share a target directory with apps/linux.
mcp_target_dir="${EMAIL_MCP_TARGET_DIR:-$repo_root/target}"
CARGO_TARGET_DIR="$mcp_target_dir" cargo build --manifest-path Cargo.toml --release --locked -p mail-mcp

target_dir="${CARGO_TARGET_DIR:-$repo_root/apps/linux/target}"
binary="$target_dir/release/email-linux"
version="$(email_release_version "$repo_root")"
# The package's Debian version (docs/release-versions.md). The file name keeps
# the release version, which matches the tag and has no ':' or '~'.
deb_version="$(email_deb_version "$version")"
architecture="$(dpkg --print-architecture)"
package_dir="$(mktemp -d)"
trap 'rm -rf "$package_dir"' EXIT

install -Dm755 "$binary" "$package_dir/usr/bin/email-linux"
install -Dm755 "$mcp_target_dir/release/mail-mcp" "$package_dir/usr/lib/email-linux/mail-mcp"
install_package_notices "$repo_root" "$package_dir/usr/share/doc/email-linux"
install -m644 LICENSE "$package_dir/usr/share/doc/email-linux/copyright"
printf '%s\n' "${EMAIL_LINUX_RELEASE_CHANNEL:-local-alpha}" > "$package_dir/usr/share/doc/email-linux/RELEASE-CHANNEL.txt"
install_linux_desktop_entry "$repo_root" "$EMAIL_PROFILE_LINUX_APP_ID" "$package_dir"
mkdir -p "$package_dir/DEBIAN"
install -Dm644 apps/linux/packaging/debian/email-linux.apparmor "$package_dir/etc/apparmor.d/email-linux"
install -m755 apps/linux/packaging/debian/postinst "$package_dir/DEBIAN/postinst"
install -m755 apps/linux/packaging/debian/prerm "$package_dir/DEBIAN/prerm"
printf '/etc/apparmor.d/email-linux\n' > "$package_dir/DEBIAN/conffiles"
# Updates (docs/linux-updates.md): a distribution profile's package joins its
# APT repository on install, as Chrome's and VS Code's packages do. The public
# key and a deb822 sources file are conffiles, so a user's deletion sticks.
# A pre-release names the beta suite, a final release the stable suite.
# Local alphas and non-distribution profiles never add a repository.
if [[ "$EMAIL_PROFILE_DISTRIBUTION" == true && -n "$EMAIL_PROFILE_LINUX_APT_REPOSITORY" ]]; then
  apt_keyring="$(python3 scripts/apt-repository.py keyring-path "$EMAIL_DISTRIBUTION_PROFILE")"
  python3 scripts/apt-repository.py check-keyring "$apt_keyring" "$EMAIL_PROFILE_LINUX_APT_KEY_FINGERPRINT" >/dev/null
  install -Dm644 "$apt_keyring" "$package_dir/usr/share/keyrings/email-linux-archive-keyring.asc"
  install -d "$package_dir/etc/apt/sources.list.d"
  python3 scripts/apt-repository.py sources "$EMAIL_PROFILE_LINUX_APT_REPOSITORY" "$version" "$architecture" \
    > "$package_dir/etc/apt/sources.list.d/email-linux.sources"
  chmod 644 "$package_dir/etc/apt/sources.list.d/email-linux.sources"
  printf '%s\n' /etc/apt/sources.list.d/email-linux.sources /usr/share/keyrings/email-linux-archive-keyring.asc \
    >> "$package_dir/DEBIAN/conffiles"
fi

cat > "$package_dir/DEBIAN/control" <<EOF
Package: email-linux
Version: $deb_version
Architecture: $architecture
Maintainer: Email Desktop
Description: Keyboard-first desktop email client
 A local-first GTK and WebKitGTK email client with a Rust mail engine.
EOF

dependency_dir="$(mktemp -d)"
trap 'rm -rf "$package_dir" "$dependency_dir"' EXIT
mkdir -p "$dependency_dir/debian"
cat > "$dependency_dir/debian/control" <<EOF
Source: email-linux
Maintainer: Email Desktop

Package: email-linux
Architecture: any
Description: Keyboard-first desktop email client
EOF
depends="$(cd "$dependency_dir" && dpkg-shlibdeps -O -e"$package_dir/usr/bin/email-linux" -e"$package_dir/usr/lib/email-linux/mail-mcp" | sed -n 's/^shlibs:Depends=//p')"
[[ -n "$depends" ]] || { echo "Could not determine shared-library dependencies" >&2; exit 1; }
sed -i "/^Architecture:/a Depends: $depends" "$package_dir/DEBIAN/control"

output_dir="${EMAIL_LINUX_PACKAGE_DIR:-$repo_root/dist-linux}"
mkdir -p "$output_dir"
output="$output_dir/email-linux_${version}_${architecture}.deb"
dpkg-deb --build --root-owner-group "$package_dir" "$output"
echo "$output"
