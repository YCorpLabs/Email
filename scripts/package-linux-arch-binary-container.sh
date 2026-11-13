#!/usr/bin/env bash
# Compatibility package from an existing Linux ELF binary. No native Arch
# compilation is claimed. Runtime validation on the target distro is required.
set -euo pipefail
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$repo_root/scripts/distribution-profile.sh"
source "$repo_root/scripts/package-notices.sh"
require_package_notices "$repo_root"
[[ $# == 2 ]] || { echo "Usage: $0 /absolute/path/to/linux-elf aarch64|x86_64" >&2; exit 2; }
binary_dir="$(cd "$(dirname "$1")" && pwd)"
binary="$binary_dir/$(basename "$1")"
[[ -f "$binary" ]] || { echo "Linux ELF binary not found" >&2; exit 1; }
case "$2" in aarch64|x86_64) package_arch="$2" ;; *) echo "Unsupported package architecture" >&2; exit 2 ;; esac
output_dir="${EMAIL_LINUX_PACKAGE_DIR:-$repo_root/dist-linux}"
mkdir -p "$output_dir"
output_dir="$(cd "$output_dir" && pwd)"
staging="$(mktemp -d)"
trap 'rm -rf "$staging"' EXIT
# Allowlist the package inputs. Never mount project configs, mail, or VM files.
mkdir -p "$staging/assets" "$staging/apps/linux/packaging/arch-binary"
mkdir -p "$staging/scripts"
cp "$repo_root/scripts/package-notices.sh" "$repo_root/scripts/linux-desktop-entry.sh" "$staging/scripts/"
# The desktop entry must use the app ID compiled into the supplied binary, which
# is the currently selected distribution profile's LINUX_APP_ID.
# Existing-binary packages are local alphas: never under a distribution profile.
if [[ "$(email_profile_value "$repo_root" DISTRIBUTION)" == true ]]; then
  echo "Existing-binary packages are local-alpha only; select a non-distribution profile." >&2
  exit 1
fi
app_id="$(email_profile_value "$repo_root" LINUX_APP_ID)"
if ! LC_ALL=C grep -aqF "$app_id" "$binary"; then
  echo "The supplied binary was not built with the selected distribution profile ($app_id). Select the profile it was built with." >&2
  exit 1
fi
mkdir -p "$staging/apps/linux/packaging"
cp "$repo_root/apps/linux/packaging/metainfo.xml.in" "$staging/apps/linux/packaging/"
cp "$repo_root/LICENSE" "$repo_root/THIRD_PARTY_NOTICES.md" "$staging/"
cp "$repo_root/assets/email-icon.png" "$staging/assets/"
cp "$repo_root/apps/linux/email-linux.desktop" "$staging/apps/linux/"
cp "$repo_root/apps/linux/packaging/arch-binary/PKGBUILD" "$staging/apps/linux/packaging/arch-binary/"
docker build --platform linux/amd64 -q -f "$repo_root/apps/linux/Dockerfile.arch-package" -t email-arch-package:local "$repo_root/apps/linux"
docker run --rm --platform linux/amd64 --user root \
  -v "$staging:/src:ro" -v "$binary:/build/email-bin:ro" -v "$output_dir:/out" \
  -e EMAIL_PACKAGE_ARCH="$package_arch" -e EMAIL_LINUX_APP_ID="$app_id" email-arch-package:local bash -lc '
set -euo pipefail
mkdir -p /build/package-output /build/package-build /build/package-source
chown builder:builder /build/package-output /build/package-build /build/package-source
cat > /build/email-makepkg.conf <<CONFIG
source /etc/makepkg.conf
CARCH=$EMAIL_PACKAGE_ARCH
CHOST=$EMAIL_PACKAGE_ARCH-pc-linux-gnu
PKGDEST=/build/package-output
BUILDDIR=/build/package-build
SRCDEST=/build/package-source
CONFIG
cd /src/apps/linux/packaging/arch-binary
runuser -u builder -- env EMAIL_LINUX_BINARY=/build/email-bin EMAIL_LINUX_APP_ID="$EMAIL_LINUX_APP_ID" makepkg --config /build/email-makepkg.conf --nodeps --force
cp /build/package-output/*.pkg.tar.zst /out/
pacman -Qip /build/package-output/*.pkg.tar.zst
'
