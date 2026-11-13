#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"
source "$repo_root/scripts/package-notices.sh"
source "$repo_root/scripts/linux-desktop-entry.sh"
require_package_notices "$repo_root"
source "$repo_root/scripts/linux-build-config.sh"
require_linux_oauth_config

command -v dpkg-deb >/dev/null || { echo "dpkg-deb is required" >&2; exit 1; }
command -v dpkg-shlibdeps >/dev/null || { echo "dpkg-shlibdeps is required" >&2; exit 1; }

cargo build --manifest-path apps/linux/Cargo.toml --release --locked

target_dir="${CARGO_TARGET_DIR:-$repo_root/apps/linux/target}"
binary="$target_dir/release/email-linux"
version="$(sed -n 's/^version = "\([^"]*\)"/\1/p' apps/linux/Cargo.toml | head -1)"
architecture="$(dpkg --print-architecture)"
package_dir="$(mktemp -d)"
trap 'rm -rf "$package_dir"' EXIT

install -Dm755 "$binary" "$package_dir/usr/bin/email-linux"
install_package_notices "$repo_root" "$package_dir/usr/share/doc/email-linux"
install -m644 LICENSE "$package_dir/usr/share/doc/email-linux/copyright"
printf '%s\n' "${EMAIL_LINUX_RELEASE_CHANNEL:-local-alpha}" > "$package_dir/usr/share/doc/email-linux/RELEASE-CHANNEL.txt"
install_linux_desktop_entry "$repo_root" "$EMAIL_PROFILE_LINUX_APP_ID" "$package_dir"
mkdir -p "$package_dir/DEBIAN"
install -Dm644 apps/linux/packaging/debian/email-linux.apparmor "$package_dir/etc/apparmor.d/email-linux"
install -m755 apps/linux/packaging/debian/postinst "$package_dir/DEBIAN/postinst"
install -m755 apps/linux/packaging/debian/prerm "$package_dir/DEBIAN/prerm"
printf '/etc/apparmor.d/email-linux\n' > "$package_dir/DEBIAN/conffiles"

cat > "$package_dir/DEBIAN/control" <<EOF
Package: email-linux
Version: $version
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
depends="$(cd "$dependency_dir" && dpkg-shlibdeps -O -e"$package_dir/usr/bin/email-linux" | sed -n 's/^shlibs:Depends=//p')"
[[ -n "$depends" ]] || { echo "Could not determine shared-library dependencies" >&2; exit 1; }
sed -i "/^Architecture:/a Depends: $depends" "$package_dir/DEBIAN/control"

output_dir="${EMAIL_LINUX_PACKAGE_DIR:-$repo_root/dist-linux}"
mkdir -p "$output_dir"
output="$output_dir/email-linux_${version}_${architecture}.deb"
dpkg-deb --build --root-owner-group "$package_dir" "$output"
echo "$output"
