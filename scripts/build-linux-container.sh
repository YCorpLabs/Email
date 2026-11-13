#!/usr/bin/env bash
set -euo pipefail

# Build the Linux GTK app in an Ubuntu 24.04 container, from macOS or Linux.
# Only files Git would commit (tracked, plus untracked files that are not
# ignored) are copied in, so local OAuth configuration, signing material, mail
# databases and the home directory never enter the container.
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
engine="${EMAIL_CONTAINER_ENGINE:-docker}"

usage() {
  cat <<'EOF'
Usage: scripts/build-linux-container.sh [--test | --image-only]

Builds apps/linux in a container for this computer's CPU architecture and
writes dist-linux/email-linux-<arch> (aarch64 or x86_64).

  --test        Also run the Rust workspace tests and the Linux shell tests.
  --image-only  Only build the email-linux-dev:<rust version> image, which
                scripts/validate-local.py --profile linux uses.

Set EMAIL_CONTAINER_ENGINE=podman to use Podman instead of Docker. The binary
needs GTK 4.10+ and WebKitGTK 6 at runtime; see docs/building.md.
EOF
}

mode=build
case "${1:-}" in
  "") ;;
  --test) mode="test" ;;
  --image-only) mode="image" ;;
  -h|--help) usage; exit 0 ;;
  *) usage >&2; exit 2 ;;
esac
(($# <= 1)) || { usage >&2; exit 2; }

command -v "$engine" >/dev/null 2>&1 || {
  echo "$engine was not found. Install Docker, or set EMAIL_CONTAINER_ENGINE=podman." >&2
  exit 127
}
rust_version="$(python3 "$repo_root/scripts/check-toolchain.py" --print-version)"
image="email-linux-dev:$rust_version"

# The Dockerfile copies nothing, so build from an empty context instead of
# sending apps/linux (and any local target directory) to the daemon.
context="$(mktemp -d)"
trap 'rm -rf "$context"' EXIT
"$engine" build --build-arg "RUST_VERSION=$rust_version" \
  -f "$repo_root/apps/linux/Dockerfile.verify" -t "$image" "$context"
if [[ "$mode" == image ]]; then
  echo "Built image $image"
  exit 0
fi

cd "$repo_root"
build_id="$(python3 scripts/build-identity.py)"
output="$repo_root/dist-linux"
mkdir -p "$output"
arch="$("$engine" run --rm "$image" uname -m)"
pending="$output/.email-linux-$arch.pending"
trap 'rm -rf "$context" "$pending"' EXIT

# Build logs go to stderr. The finished binary is the container's only stdout,
# so this shell writes it and owns it under Docker, rootless Docker and Podman.
# shellcheck disable=SC2016 # Expanded inside the container.
container_script='
set -euo pipefail
exec 3>&1 1>&2
mkdir -p /work
# Fresh timestamps: checkouts share the target volume, and Cargo must never
# treat another checkout'"'"'s build output as newer than these sources.
tar -xmf - -C /work
cd /work
# Checkouts share the target volume; one build and copy-out at a time.
exec 9>/target/.email-linux-dev.lock
flock 9
# The root and apps/linux workspaces resolve dependencies separately; mixing
# their artifacts in one directory links mismatched crate builds.
if [[ "$EMAIL_CONTAINER_MODE" == test ]]; then
  CARGO_TARGET_DIR=/target/workspace cargo test --workspace --locked
  CARGO_TARGET_DIR=/target/linux cargo test --manifest-path apps/linux/Cargo.toml --locked
fi
CARGO_TARGET_DIR=/target/linux cargo build --manifest-path apps/linux/Cargo.toml --release --locked
cat /target/linux/release/email-linux >&3
'

# The Cargo download cache is shared with scripts/validate-local.py.
git ls-files -z --cached --others --exclude-standard \
  | while IFS= read -r -d '' name; do if [[ -e "$name" ]]; then printf '%s\0' "$name"; fi; done \
  | COPYFILE_DISABLE=1 tar --null --no-xattrs -T - -cf - \
  | "$engine" run --rm -i \
      --mount "type=volume,source=email-linux-cargo,target=/cargo" \
      --mount "type=volume,source=email-linux-dev-target,target=/target" \
      -e CARGO_HOME=/cargo \
      -e EMAIL_BUILD_ID="$build_id" -e EMAIL_CONTAINER_MODE="$mode" \
      "$image" bash -c "$container_script" > "$pending"
chmod 755 "$pending"
mv -f "$pending" "$output/email-linux-$arch"
echo "Built dist-linux/email-linux-$arch with Rust $rust_version"
