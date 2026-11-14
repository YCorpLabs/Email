#!/usr/bin/env bash
# Builds the Linux release artifacts for each requested CPU architecture in a
# clean Ubuntu 24.04 container, from the committed source at HEAD:
#
#   email-linux-<arch>                  the GTK app (the engine is linked in)
#   email-linux_<version>_<debarch>.deb Debian/Ubuntu package
#   email-linux-<version>-1-<arch>.pkg.tar.zst
#                                       Arch compatibility package (local-alpha
#                                       only; distribution Arch packages are
#                                       built from source, see packaging/aur)
#   mail-mcp-<arch>                     stdio MCP server (also in the .deb, at
#                                       /usr/lib/email-linux/mail-mcp)
#   libmail_service-<arch>.so           engine C ABI library (not in the packages)
#   BUILD-INFO-<arch>.txt               commit, build ID, profile and toolchain
#
# and updates their lines in SHA256SUMS, leaving other architectures' lines.
#
# Usage: scripts/package-linux-release.sh [aarch64] [x86_64]   (default: both)
#
# A foreign architecture runs under Docker's CPU emulation (Rosetta or QEMU on
# Apple Silicon). That is a build path, not a test of the result on real
# hardware. The selected distribution profile, release channel and OAuth
# application values are read on the host exactly as package-linux-deb.sh
# reads them, and are passed into the container by name only.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"
source "$repo_root/scripts/package-notices.sh"
require_package_notices "$repo_root"
command -v docker >/dev/null || { echo "Docker is required." >&2; exit 1; }
sha256() { if command -v sha256sum >/dev/null; then sha256sum "$@"; else shasum -a 256 "$@"; fi; }

architectures=("$@")
[[ ${#architectures[@]} -gt 0 ]] || architectures=(aarch64 x86_64)
for arch in "${architectures[@]}"; do
  case "$arch" in aarch64|x86_64) ;; *) echo "Unsupported architecture: $arch (use aarch64 or x86_64)" >&2; exit 2 ;; esac
done

# Artifacts must correspond to a commit. Ignored files (config/*.local*,
# dist-linux) are allowed; tracked edits and untracked sources are not.
if [[ -n "$(git status --porcelain)" ]]; then
  echo "Commit or stash working changes first; release packages are built from HEAD only." >&2
  exit 1
fi
commit="$(git rev-parse HEAD)"
version="$(sed -n 's/^version = "\([^"]*\)"/\1/p' apps/linux/Cargo.toml | head -1)"

# Resolve the profile and OAuth application values once on the host and fail
# before any container work if they are not acceptable for this channel.
source "$repo_root/scripts/linux-build-config.sh"
require_linux_oauth_config
host_build_id="$EMAIL_BUILD_ID"
profile="$EMAIL_PROFILE_PROFILE"
channel="${EMAIL_LINUX_RELEASE_CHANNEL:-local-alpha}"
# The container resolves the profile by name from the committed source.
[[ "$EMAIL_DISTRIBUTION_PROFILE" -ef "config/profiles/$profile.env" ]] || {
  echo "Select a profile committed under config/profiles/ so the container resolves the same file." >&2
  exit 1
}
export EMAIL_DISTRIBUTION_PROFILE="$profile" EMAIL_LINUX_RELEASE_CHANNEL="$channel"

# The Rust version pinned in rust-toolchain.toml, as CI and the dev container use.
toolchain="${EMAIL_LINUX_RUST_TOOLCHAIN:-$(python3 scripts/check-toolchain.py --print-version)}"
[[ "$toolchain" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "Could not determine the Rust toolchain version." >&2; exit 1; }
jobs="${CARGO_BUILD_JOBS:-4}"

output_dir="${EMAIL_LINUX_PACKAGE_DIR:-$repo_root/dist-linux}"
mkdir -p "$output_dir"
output_dir="$(cd "$output_dir" && pwd)"
staging="$(mktemp -d)"
trap 'rm -rf "$staging"' EXIT
# Committed source only: no .git, local config, caches or mail data. Fresh
# modification times (-m): the target volume outlives this run, and Cargo must
# never treat output built from another commit as newer than these sources.
mkdir -p "$staging/src"
git archive --format=tar HEAD | tar -x -m -C "$staging/src"

for arch in "${architectures[@]}"; do
  case "$arch" in
    aarch64) platform=linux/arm64 deb_arch=arm64 ;;
    x86_64) platform=linux/amd64 deb_arch=amd64 ;;
  esac
  image="email-linux-package:$arch"
  work="$staging/out-$arch"
  mkdir -p "$work"
  echo "==> $arch: build image $image (Rust $toolchain)"
  docker build --platform "$platform" -q --build-arg RUST_VERSION="$toolchain" \
    -f apps/linux/Dockerfile.verify -t "$image" apps/linux >/dev/null
  echo "==> $arch: compile and package (profile $profile, channel $channel)"
  docker run --rm --platform "$platform" \
    --mount "type=bind,source=$staging/src,target=/src,readonly" \
    --mount "type=bind,source=$work,target=/out" \
    --mount "type=volume,source=email-linux-package-cargo-$arch,target=/opt/cargo/registry" \
    --mount "type=volume,source=email-linux-package-target-$arch,target=/target" \
    -e EMAIL_DISTRIBUTION_PROFILE -e EMAIL_LINUX_RELEASE_CHANNEL \
    -e EMAIL_GOOGLE_CLIENT_ID -e EMAIL_GOOGLE_CLIENT_SECRET -e EMAIL_MICROSOFT_CLIENT_ID \
    -e EMAIL_RELEASE_GOOGLE_CLIENT_ID -e EMAIL_RELEASE_GOOGLE_CLIENT_SECRET \
    -e EMAIL_PACKAGE_ARCH="$arch" -e EMAIL_COMMIT="$commit" -e EMAIL_VERSION="$version" -e EMAIL_HOST_BUILD_ID="$host_build_id" \
    -e CARGO_BUILD_JOBS="$jobs" -e EMAIL_LINUX_PACKAGE_DIR=/out \
    -w /src "$image" bash -c '
set -euo pipefail
machine="$(uname -m)"
[[ "$machine" == "$EMAIL_PACKAGE_ARCH" ]] || { echo "Container machine $machine is not $EMAIL_PACKAGE_ARCH" >&2; exit 1; }
# Export the profile, OAuth application values and build ID for every Cargo
# build below, as package-linux-deb.sh does for the app.
source scripts/linux-build-config.sh
require_linux_oauth_config
# The embedded build ID must be the one the host computed for this commit.
[[ "$EMAIL_BUILD_ID" == "$EMAIL_HOST_BUILD_ID" ]] || { echo "Build ID mismatch: container $EMAIL_BUILD_ID, host $EMAIL_HOST_BUILD_ID" >&2; exit 1; }
CARGO_TARGET_DIR=/target/linux EMAIL_MCP_TARGET_DIR=/target/workspace scripts/package-linux-deb.sh
CARGO_TARGET_DIR=/target/workspace cargo build --release --locked -p mail-mcp -p mail-service
install -m755 /target/linux/release/email-linux "/out/email-linux-$EMAIL_PACKAGE_ARCH"
install -m755 /target/workspace/release/mail-mcp "/out/mail-mcp-$EMAIL_PACKAGE_ARCH"
install -m644 /target/workspace/release/libmail_service.so "/out/libmail_service-$EMAIL_PACKAGE_ARCH.so"
. /etc/os-release
{
  echo "commit=$EMAIL_COMMIT"
  echo "version=$EMAIL_VERSION"
  echo "build_id=$EMAIL_BUILD_ID"
  echo "profile=$EMAIL_PROFILE_PROFILE"
  echo "linux_app_id=$EMAIL_PROFILE_LINUX_APP_ID"
  echo "linux_data_directory=$EMAIL_PROFILE_LINUX_DATA_DIRECTORY"
  echo "release_channel=${EMAIL_LINUX_RELEASE_CHANNEL:-local-alpha}"
  echo "architecture=$EMAIL_PACKAGE_ARCH"
  echo "build_os=$PRETTY_NAME"
  echo "glibc=$(dpkg-query -W -f="\${Version}" libc6)"
  echo "gtk4=$(dpkg-query -W -f="\${Version}" libgtk-4-dev)"
  echo "webkitgtk6=$(dpkg-query -W -f="\${Version}" libwebkitgtk-6.0-dev)"
  echo "rustc=$(rustc --version)"
} > "/out/BUILD-INFO-$EMAIL_PACKAGE_ARCH.txt"
'
  if [[ "$channel" == local-alpha ]]; then
    echo "==> $arch: Arch compatibility package"
    EMAIL_LINUX_PACKAGE_DIR="$work" "$repo_root/scripts/package-linux-arch-binary-container.sh" \
      "$work/email-linux-$arch" "$arch" >/dev/null
  else
    echo "==> $arch: no Arch compatibility package for $channel; build Arch packages from source (apps/linux/packaging/aur)."
  fi
  # Replace this architecture's artifacts and checksum lines. Earlier files for
  # this architecture (another version, or an Arch package from another
  # channel) are removed so they cannot be verified or shipped by mistake.
  sums="$output_dir/SHA256SUMS"
  touch "$sums"
  : > "$staging/replaced-$arch"
  for stale in "$output_dir/email-linux-$arch" "$output_dir"/email-linux_*_"$deb_arch".deb \
      "$output_dir"/email-linux-*-"$arch".pkg.tar.zst "$output_dir/mail-mcp-$arch" \
      "$output_dir/libmail_service-$arch.so" "$output_dir/BUILD-INFO-$arch.txt"; do
    [[ -e "$stale" ]] || continue
    basename "$stale" >> "$staging/replaced-$arch"
    rm -f "$stale"
  done
  produced=()
  for file in "$work"/*; do
    name="$(basename "$file")"
    cp -f "$file" "$output_dir/$name"
    produced+=("$name")
  done
  printf '%s\n' "${produced[@]}" >> "$staging/replaced-$arch"
  {
    awk 'NR == FNR { skip[$0] = 1; next } { name = $2; sub(/^\*/, "", name); if (!(name in skip)) print }' \
      "$staging/replaced-$arch" "$sums"
    (cd "$output_dir" && sha256 "${produced[@]}")
  } > "$sums.new"
  mv "$sums.new" "$sums"
  echo "==> $arch: wrote ${produced[*]} to $output_dir"
done

# Per-architecture runs may leave another architecture's files from an older
# commit; publish one set only when every BUILD-INFO names the same commit.
for info in "$output_dir"/BUILD-INFO-*.txt; do
  [[ -e "$info" ]] || continue
  if [[ "$(sed -n 's/^commit=//p' "$info")" != "$commit" ]]; then
    echo "WARNING: $(basename "$info") is from another commit; rebuild that architecture before publishing $output_dir." >&2
  fi
done
