#!/usr/bin/env bash
# Installs the packages written by package-linux-release.sh into clean,
# disposable containers and runs scripts/linux-package-smoke.sh against them:
#
#   ubuntu:24.04     the .deb, for each requested architecture
#   archlinux:base   the .pkg.tar.zst, x86_64 only (the official Arch image is
#                    amd64 only; ARM Arch is checked in the Omarchy VM)
#
# Each package is installed with the distro package manager alone, so its
# declared dependencies must resolve from the distro's repositories. Results,
# logs and screenshots go to dist-linux/verify/<distro>-<arch>/.
#
# Usage: scripts/verify-linux-packages.sh [aarch64] [x86_64]   (default: both)
# EMAIL_LINUX_ARCH_PACKAGE=/abs/path.pkg.tar.zst verifies that Arch package
# (for example one built from the AUR PKGBUILD) instead of the compatibility
# one.
#
# A foreign architecture runs under Docker's CPU emulation. These are
# installability and smoke checks in containers under Xvfb, not tests on real
# hardware or in a desktop session.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
command -v docker >/dev/null || { echo "Docker is required." >&2; exit 1; }
sha256() { if command -v sha256sum >/dev/null; then sha256sum "$@"; else shasum -a 256 "$@"; fi; }
architectures=("$@")
[[ ${#architectures[@]} -gt 0 ]] || architectures=(aarch64 x86_64)
package_dir="$(cd "${EMAIL_LINUX_PACKAGE_DIR:-$repo_root/dist-linux}" && pwd)"
results_root="${EMAIL_LINUX_VERIFY_DIR:-$package_dir/verify}"
# A platform other than the host's runs under Docker's CPU emulation.
case "$(uname -m)" in
  arm64|aarch64) native_platform=linux/arm64 ;;
  x86_64) native_platform=linux/amd64 ;;
  *) native_platform= ;;
esac

# Checks a file against its SHA256SUMS line so the tested bytes are the
# recorded ones.
check_sum() {
  local directory="$1" name="$2" expected actual
  expected="$(awk -v name="$name" '{ n = $2; sub(/^\*/, "", n); if (n == name) print $1 }' "$directory/SHA256SUMS")"
  [[ -n "$expected" ]] || { echo "$name has no line in $directory/SHA256SUMS" >&2; return 1; }
  actual="$(sha256 "$directory/$name" | awk '{ print $1 }')"
  [[ "$actual" == "$expected" ]] || { echo "$name does not match SHA256SUMS" >&2; return 1; }
}

run_smoke() {
  local platform="$1" image="$2" results="$3" install="$4"
  shift 4
  rm -rf "$results"
  mkdir -p "$results"
  # --privileged lets WebKitGTK's bubblewrap sandbox create its namespaces in
  # the container; the sandbox itself stays enabled.
  local emulated=0
  [[ "$platform" == "$native_platform" ]] || emulated=1
  docker run --rm --platform "$platform" --privileged -e EMAIL_SMOKE_EMULATED="$emulated" \
    --mount "type=bind,source=$package_dir,target=/packages,readonly" \
    --mount "type=bind,source=$repo_root/scripts/linux-package-smoke.sh,target=/smoke.sh,readonly" \
    --mount "type=bind,source=$results,target=/results" \
    "$@" "$image" bash -c "set -euo pipefail
$install
bash /smoke.sh \"/packages/\$MCP\" /results \"\$ARCH\""
}

for arch in "${architectures[@]}"; do
  case "$arch" in
    aarch64) platform=linux/arm64 deb_arch=arm64 ;;
    x86_64) platform=linux/amd64 deb_arch=amd64 ;;
    *) echo "Unsupported architecture: $arch" >&2; exit 2 ;;
  esac
  info="$package_dir/BUILD-INFO-$arch.txt"
  [[ -f "$info" ]] || { echo "Missing $info; run scripts/package-linux-release.sh $arch first." >&2; exit 1; }
  # The files under test are described by their BUILD-INFO, not the checkout.
  version="$(sed -n 's/^version=//p' "$info")"
  channel="$(sed -n 's/^release_channel=//p' "$info")"
  [[ -n "$version" && -n "$channel" ]] || { echo "Incomplete $info; rebuild with package-linux-release.sh." >&2; exit 1; }
  deb="email-linux_${version}_${deb_arch}.deb"
  mcp="mail-mcp-$arch"
  for name in "$deb" "$mcp" "email-linux-$arch"; do check_sum "$package_dir" "$name"; done

  echo "==> $arch: Ubuntu 24.04 install and smoke"
  run_smoke "$platform" ubuntu:24.04 "$results_root/ubuntu-$arch" '
export DEBIAN_FRONTEND=noninteractive
cat /etc/os-release > /results/os-release.txt
apt-get update -qq
# The package and its declared dependencies only.
apt-get install -y --no-install-recommends "/packages/$DEB" > /results/install.log 2>&1
dpkg-query -W -f="\${Package} \${Version} \${Architecture}\n" email-linux > /results/installed.txt
dpkg-deb -f "/packages/$DEB" Architecture Depends > /results/control.txt
# Test tooling and a font; not package dependencies.
apt-get install -y --no-install-recommends file xvfb x11-utils xdotool imagemagick dbus fonts-dejavu-core sqlite3 procps > /results/tools.log 2>&1' \
    -e DEB="$deb" -e MCP="$mcp" -e ARCH="$arch"

  if [[ "$arch" == x86_64 ]]; then
    if [[ -z "${EMAIL_LINUX_ARCH_PACKAGE:-}" && "$channel" != local-alpha ]]; then
      echo "==> $arch: no Arch compatibility package for the $channel channel; set EMAIL_LINUX_ARCH_PACKAGE to an Arch package built from source."
      continue
    fi
    arch_package="${EMAIL_LINUX_ARCH_PACKAGE:-$package_dir/email-linux-$version-1-x86_64.pkg.tar.zst}"
    [[ -n "${EMAIL_LINUX_ARCH_PACKAGE:-}" ]] || check_sum "$package_dir" "$(basename "$arch_package")"
    [[ -f "$arch_package" ]] || { echo "Missing $arch_package" >&2; exit 1; }
    label="arch-x86_64"
    [[ -z "${EMAIL_LINUX_ARCH_PACKAGE:-}" ]] || label="arch-x86_64-$(basename "$(dirname "$arch_package")")"
    echo "==> $arch: Arch Linux install and smoke ($(basename "$arch_package"))"
    # QEMU/Rosetta user emulation cannot install pacman's download syscall
    # filter; see Dockerfile.arch-package. Signature checks stay enabled.
    run_smoke "$platform" archlinux:base "$results_root/$label" '
cat /etc/os-release > /results/os-release.txt
pacman --disable-sandbox-syscalls -Syu --noconfirm > /results/upgrade.log 2>&1
pacman --disable-sandbox-syscalls -U --noconfirm /arch-package/package.pkg.tar.zst > /results/install.log 2>&1
pacman -Qi "$(pacman -Qqo /usr/bin/email-linux)" > /results/installed.txt
# Test tooling and a font; not package dependencies. The standalone mail-mcp
# is built on Ubuntu and links OpenSSL 3, which Arch ships as openssl-3.
pacman --disable-sandbox-syscalls -S --noconfirm --needed file xorg-server-xvfb xorg-xdpyinfo xdotool imagemagick dbus ttf-dejavu sqlite procps-ng openssl-3 > /results/tools.log 2>&1' \
      --mount "type=bind,source=$arch_package,target=/arch-package/package.pkg.tar.zst,readonly" \
      -e MCP="$mcp" -e ARCH="$arch"
  fi
done
