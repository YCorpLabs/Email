#!/usr/bin/env bash
# Prepares and checks the two AUR packages in the official Arch Linux container
# (amd64; emulated on Apple Silicon):
#
#   apps/linux/packaging/aur/PKGBUILD      email-ycorp      builds from source
#   apps/linux/packaging/aur-bin/PKGBUILD  email-ycorp-bin  installs the release .debs
#
#   scripts/prepare-aur-package.sh [--ref COMMIT] [--debs DIR] [--build] [--update-repo]
#       Release mode. Writes the release source tarball Email-<version>.tar.gz
#       (`git archive` of COMMIT with the Email-<version>/ prefix) to
#       dist-linux/aur/; attach it to the GitHub release. Records its SHA-256,
#       and those of the two .debs in DIR (default dist-linux, written by
#       scripts/package-linux-release.sh from the same commit), in the
#       PKGBUILDs, writes .SRCINFO, verifies every source against its checksum
#       and runs namcap on the PKGBUILDs. The files to publish go to
#       dist-linux/aur/ and dist-linux/aur-bin/. --update-repo also copies both
#       PKGBUILDs and .SRCINFO files into apps/linux/packaging/ to commit.
#       --build builds email-ycorp-bin for x86_64 and aarch64 and email-ycorp
#       for x86_64 (dependencies from the Arch repositories) and runs namcap on
#       the packages. COMMIT defaults to the PKGBUILD's _tag once it exists.
#
#   scripts/prepare-aur-package.sh --local [--debs DIR]
#       Verification mode: the same with COMMIT=HEAD and --build, writing to
#       dist-linux/aur-check/. Skips email-ycorp-bin when DIR has no .debs. Its
#       checksums are for local files: never publish or commit them.
#
# Install-test a package it built in a clean Arch container with
#   EMAIL_LINUX_ARCH_PACKAGE=<path> scripts/verify-linux-packages.sh x86_64
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"
command -v docker >/dev/null || { echo "Docker is required." >&2; exit 1; }
usage() { echo "Usage: $0 [--ref COMMIT] [--debs DIR] [--build] [--update-repo] | --local [--debs DIR]" >&2; }
mode=release build=false update_repo=false ref="" debs_dir="$repo_root/dist-linux"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --local) mode=local build=true ;;
    --build) build=true ;;
    --update-repo) update_repo=true ;;
    --ref) [[ $# -ge 2 ]] || { usage; exit 2; }; ref="$2"; shift ;;
    --debs) [[ $# -ge 2 ]] || { usage; exit 2; }; debs_dir="$2"; shift ;;
    *) usage; exit 2 ;;
  esac
  shift
done
if [[ "$mode" == local && ( "$update_repo" == true || -n "$ref" ) ]]; then
  echo "--local builds HEAD and never updates the repository's PKGBUILDs." >&2
  exit 2
fi

source_pkgbuild="apps/linux/packaging/aur/PKGBUILD"
bin_pkgbuild="apps/linux/packaging/aur-bin/PKGBUILD"
value() { sed -n "s/^$2=//p" "$1"; }
pkgver="$(value "$source_pkgbuild" pkgver)"
tag="$(value "$source_pkgbuild" _tag)"
version="${tag#v}"
cargo_version="$(sed -n 's/^version = "\([^"]*\)"/\1/p' apps/linux/Cargo.toml | head -1)"
[[ "$tag" == "v$cargo_version"* ]] || {
  echo "PKGBUILD _tag $tag does not match apps/linux/Cargo.toml version $cargo_version." >&2
  exit 1
}
for key in pkgver _tag pkgrel; do
  [[ "$(value "$bin_pkgbuild" "$key")" == "$(value "$source_pkgbuild" "$key")" ]] || {
    echo "$key differs between $source_pkgbuild and $bin_pkgbuild." >&2
    exit 1
  }
done
debver="$(value "$bin_pkgbuild" _debver)"
[[ "$debver" == "$cargo_version" ]] || {
  echo "$bin_pkgbuild _debver $debver does not match apps/linux/Cargo.toml version $cargo_version." >&2
  exit 1
}

if [[ "$mode" == local ]]; then
  if [[ -n "$(git status --porcelain)" ]]; then
    echo "Commit or stash working changes first; --local builds HEAD." >&2
    exit 1
  fi
  ref=HEAD
  output_root="${EMAIL_AUR_OUTPUT_DIR:-$repo_root/dist-linux/aur-check}"
else
  ref="${ref:-$tag}"
  output_root="${EMAIL_AUR_OUTPUT_DIR:-$repo_root/dist-linux}"
fi
commit="$(git rev-parse --verify --quiet "$ref^{commit}")" || {
  echo "$ref is not a commit in this clone. Pass --ref with the release commit until $tag exists." >&2
  exit 1
}

# The binary package repackages the .debs package-linux-release.sh built.
debs_dir="$(cd "$debs_dir" 2>/dev/null && pwd)" || debs_dir=""
with_bin=true
for arch in x86_64 aarch64; do
  case "$arch" in x86_64) deb_arch=amd64 ;; aarch64) deb_arch=arm64 ;; esac
  deb="$debs_dir/email-linux_${debver}_${deb_arch}.deb"
  info="$debs_dir/BUILD-INFO-$arch.txt"
  if [[ -z "$debs_dir" || ! -f "$deb" ]]; then
    if [[ "$mode" == local ]]; then
      echo "No email-linux_${debver}_${deb_arch}.deb; skipping email-ycorp-bin."
      with_bin=false
      break
    fi
    echo "Missing $deb. Run scripts/package-linux-release.sh first, or pass --debs." >&2
    exit 1
  fi
  if [[ "$mode" == release && "$(sed -n 's/^commit=//p' "$info" 2>/dev/null)" != "$commit" ]]; then
    echo "$(basename "$deb") was not built from $commit (see $info)." >&2
    exit 1
  fi
done

sha256() { if command -v sha256sum >/dev/null; then sha256sum "$1"; else shasum -a 256 "$1"; fi | awk '{print $1}'; }
# Replaces one checksum array, which must hold a single entry.
set_sum() {
  local file="$1" key="$2" sum="$3"
  sed -e "s/^$key=(.*)\$/$key=('$sum')/" "$file" > "$file.new"
  mv "$file.new" "$file"
  grep -qx "$key=('$sum')" "$file" || { echo "Could not set $key in $file" >&2; exit 1; }
}

staging="$(mktemp -d)"
trap 'rm -rf "$staging"' EXIT
mkdir -p "$staging/in/aur" "$staging/out"
tarball="Email-$version.tar.gz"
# Same layout as GitHub's tag archive: one Email-<version>/ directory.
git archive --format=tar.gz --prefix="Email-$version/" "$commit" > "$staging/$tarball"
cp "$source_pkgbuild" "$staging/in/aur/PKGBUILD"
cp "$staging/$tarball" "$staging/in/aur/email-ycorp-$pkgver.tar.gz"
set_sum "$staging/in/aur/PKGBUILD" sha256sums "$(sha256 "$staging/$tarball")"
if [[ "$with_bin" == true ]]; then
  mkdir -p "$staging/in/aur-bin"
  cp "$bin_pkgbuild" "$staging/in/aur-bin/PKGBUILD"
  for arch in x86_64 aarch64; do
    case "$arch" in x86_64) deb_arch=amd64 ;; aarch64) deb_arch=arm64 ;; esac
    deb="$debs_dir/email-linux_${debver}_${deb_arch}.deb"
    cp "$deb" "$staging/in/aur-bin/email-ycorp-bin-$pkgver-$deb_arch.deb"
    set_sum "$staging/in/aur-bin/PKGBUILD" "sha256sums_$arch" "$(sha256 "$deb")"
  done
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
# Packages the aarch64 binary package on this x86_64 container. Nothing is
# compiled for it, so only the architecture name changes.
sed -e "s/^CARCH=.*/CARCH=\"aarch64\"/" -e "s/^CHOST=.*/CHOST=\"aarch64-unknown-linux-gnu\"/" \
  /etc/makepkg.conf > /etc/makepkg-aarch64.conf
as_builder() { runuser -u builder -- "$@"; }
packages=(aur)
[[ ! -d /in/aur-bin ]] || packages+=(aur-bin)
for package in "${packages[@]}"; do
  install -d -o builder "/home/builder/$package"
  install -o builder -m644 "/in/$package"/* "/home/builder/$package/"
  mkdir -p "/out/$package"
  cd "/home/builder/$package"
  as_builder makepkg --printsrcinfo > .SRCINFO
  as_builder makepkg --verifysource 2>&1 | tee "/out/$package/verifysource.log"
  if [[ "$package" == aur-bin ]]; then
    as_builder makepkg --verifysource --config /etc/makepkg-aarch64.conf 2>&1 | tee -a "/out/$package/verifysource.log"
  fi
  namcap PKGBUILD | tee "/out/$package/namcap-PKGBUILD.txt"
done
build_logged() {
  local log="$1"; shift
  as_builder "$@" > "$log" 2>&1 || { tail -60 "$log" >&2; exit 1; }
}
if [[ "$BUILD" == true ]]; then
  if [[ -d /home/builder/aur-bin ]]; then
    cd /home/builder/aur-bin
    # --syncdeps installs the runtime libraries, which namcap needs to map them.
    build_logged /out/aur-bin/makepkg-x86_64.log makepkg --syncdeps --noconfirm --cleanbuild
    build_logged /out/aur-bin/makepkg-aarch64.log makepkg --nodeps --noconfirm --cleanbuild --config /etc/makepkg-aarch64.conf
  fi
  cd /home/builder/aur
  build_logged /out/aur/makepkg.log makepkg --syncdeps --noconfirm --cleanbuild
  pacman -Q rust > /out/aur/toolchain.txt
  for package in "${packages[@]}"; do
    for file in "/home/builder/$package"/*.pkg.tar.zst; do
      name="$(basename "$file")"
      namcap "$file" | tee -a "/out/$package/namcap-package.txt"
      pacman -Qip "$file" > "/out/$package/$name.info.txt"
      pacman -Qlp "$file" > "/out/$package/$name.files.txt"
      cp "$file" "/out/$package/"
    done
  done
fi
for package in "${packages[@]}"; do
  cp "/home/builder/$package/PKGBUILD" "/home/builder/$package/.SRCINFO" "/out/$package/"
done
'
for package in aur aur-bin; do
  [[ -d "$staging/out/$package" ]] || continue
  rm -rf "${output_root:?}/$package"
  mkdir -p "$output_root/$package"
  cp -f "$staging/out/$package/"* "$staging/out/$package/.SRCINFO" "$output_root/$package/"
done
cp -f "$staging/out/container-setup.log" "$output_root/aur/"
cp -f "$staging/$tarball" "$output_root/aur/"
if [[ "$update_repo" == true ]]; then
  cp -f "$output_root/aur/PKGBUILD" "$output_root/aur/.SRCINFO" apps/linux/packaging/aur/
  if [[ "$with_bin" == true ]]; then
    cp -f "$output_root/aur-bin/PKGBUILD" "$output_root/aur-bin/.SRCINFO" apps/linux/packaging/aur-bin/
  fi
fi
echo "Source tarball for the release: $output_root/aur/$tarball ($(sha256 "$output_root/aur/$tarball"))"
echo "AUR files: $output_root/aur/{PKGBUILD,.SRCINFO}$([[ "$with_bin" == true ]] && echo " and $output_root/aur-bin/{PKGBUILD,.SRCINFO}")"
