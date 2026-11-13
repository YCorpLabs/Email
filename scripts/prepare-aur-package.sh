#!/usr/bin/env bash
# Prepares and checks the AUR source package in apps/linux/packaging/aur, using
# the official Arch Linux container (amd64; emulated on Apple Silicon).
#
#   scripts/prepare-aur-package.sh [--build]
#       Release mode. Downloads the v<pkgver> tag tarball from GitHub (the
#       repository must be public and the tag pushed), records its SHA-256 in
#       the PKGBUILD, writes .SRCINFO and runs namcap. Writes the two files to
#       publish to dist-linux/aur/. --build also builds the package from it.
#
#   scripts/prepare-aur-package.sh --local
#       Verification mode. Uses `git archive HEAD` in place of the tag tarball,
#       builds the package with `makepkg --syncdeps` in a clean Arch x86_64
#       container (build and runtime dependencies come from the Arch
#       repositories) and runs namcap on the PKGBUILD and the package. Writes
#       to dist-linux/aur-check/. That PKGBUILD's checksum is for the local
#       archive: never publish it. Install-test the package with
#       EMAIL_LINUX_ARCH_PACKAGE=<path> scripts/verify-linux-packages.sh x86_64
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"
command -v docker >/dev/null || { echo "Docker is required." >&2; exit 1; }
mode=release build=false
for argument in "$@"; do
  case "$argument" in
    --local) mode=local build=true ;;
    --build) build=true ;;
    *) echo "Usage: $0 [--local | --build]" >&2; exit 2 ;;
  esac
done

pkgbuild="apps/linux/packaging/aur/PKGBUILD"
pkgver="$(sed -n 's/^pkgver=//p' "$pkgbuild")"
tag="$(sed -n 's/^_tag=//p' "$pkgbuild")"
cargo_version="$(sed -n 's/^version = "\([^"]*\)"/\1/p' apps/linux/Cargo.toml | head -1)"
[[ "$tag" == "v$cargo_version"* ]] || {
  echo "PKGBUILD _tag $tag does not match apps/linux/Cargo.toml version $cargo_version." >&2
  exit 1
}

staging="$(mktemp -d)"
trap 'rm -rf "$staging"' EXIT
mkdir -p "$staging/in" "$staging/out"
cp "$pkgbuild" "$staging/in/PKGBUILD"
if [[ "$mode" == local ]]; then
  if [[ -n "$(git status --porcelain)" ]]; then
    echo "Commit or stash working changes first; --local builds HEAD." >&2
    exit 1
  fi
  # Same layout as GitHub's tag tarball: one <repository>-<version>/ directory
  # (Email-<version>/ for YCorpLabs/Email).
  git archive --format=tar.gz --prefix="Email-${tag#v}/" HEAD > "$staging/in/email-linux-$pkgver.tar.gz"
  output_dir="${EMAIL_AUR_OUTPUT_DIR:-$repo_root/dist-linux/aur-check}"
else
  output_dir="${EMAIL_AUR_OUTPUT_DIR:-$repo_root/dist-linux/aur}"
fi

docker run --rm --platform linux/amd64 \
  --mount "type=bind,source=$staging/in,target=/in,readonly" \
  --mount "type=bind,source=$staging/out,target=/out" \
  -e BUILD="$build" -e CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}" \
  archlinux:base bash -c '
set -euo pipefail
# User-mode CPU emulation cannot install pacman'"'"'s download syscall filter
# (see apps/linux/Dockerfile.arch-package); signature checks stay enabled.
sed -i "/^\[options\]/a DisableSandboxSyscalls" /etc/pacman.conf
pacman -Syu --noconfirm --needed base-devel pacman-contrib namcap > /out/container-setup.log 2>&1
useradd --create-home builder
echo "builder ALL=(root) NOPASSWD: /usr/bin/pacman" > /etc/sudoers.d/builder
install -d -o builder /home/builder/package
install -o builder -m644 /in/* /home/builder/package/
cd /home/builder/package
runuser -u builder -- updpkgsums 2>&1 | tee /out/updpkgsums.log
runuser -u builder -- makepkg --printsrcinfo > .SRCINFO
namcap PKGBUILD | tee /out/namcap-PKGBUILD.txt
if [[ "$BUILD" == true ]]; then
  runuser -u builder -- makepkg --syncdeps --noconfirm --cleanbuild > /out/makepkg.log 2>&1 || {
    tail -60 /out/makepkg.log >&2
    exit 1
  }
  for package in *.pkg.tar.zst; do
    namcap "$package" | tee -a /out/namcap-package.txt
    cp "$package" /out/
  done
  pacman -Qip email-linux-[0-9]*.pkg.tar.zst > /out/package-info.txt
  pacman -Qlp email-linux-[0-9]*.pkg.tar.zst > /out/package-files.txt
  pacman -Q rust > /out/toolchain.txt
fi
cp PKGBUILD .SRCINFO /out/
'
mkdir -p "$output_dir"
cp -f "$staging/out/"* "$staging/out/.SRCINFO" "$output_dir/"
echo "Wrote $(cd "$staging/out" && ls -A | tr '\n' ' ')to $output_dir"
