#!/usr/bin/env bash
# Adds Email .deb packages to a local, signed APT repository staging directory
# (docs/linux-updates.md). It never uploads: scripts/upload-updates.sh copies
# the verified directory to the update host as a separate, reviewed step.
#
# Usage: scripts/publish-linux-apt.sh --staging DIR PACKAGE.deb...
#
#   --staging DIR   the repository as published, kept between releases: pool/
#                   keeps every earlier package, and the indexes are rebuilt
#                   from what is already there. Keep it (or recreate it from
#                   the published repository) before publishing again.
#
# Environment:
#   EMAIL_APT_GNUPGHOME        GnuPG home holding the repository signing key
#                              (required). It must be outside this checkout.
#   EMAIL_APT_PASSPHRASE_FILE  optional file with that key's passphrase, for
#                              signing without a pinentry prompt
#   EMAIL_DISTRIBUTION_PROFILE the distribution profile (official), as for builds
#   EMAIL_CONTAINER_ENGINE     docker (default) or podman
#   EMAIL_APT_TOOLS_IMAGE      image with dpkg-deb (default ubuntu:24.04)
#
# Each package must be this profile's distribution build of email-linux: its
# sources file names the profile's repository, its keyring is the profile's
# public key, and its version must be newer than what the suite already has.
# A pre-release goes to the beta suite; a final release goes to stable and
# beta. The signing key must have the profile's LINUX_APT_KEY_FINGERPRINT, and
# both signatures are checked with the committed public key before the staging
# directory is replaced. Key material is never printed or copied.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$repo_root/scripts/distribution-profile.sh"
engine="${EMAIL_CONTAINER_ENGINE:-docker}"
tools_image="${EMAIL_APT_TOOLS_IMAGE:-ubuntu:24.04}"

usage() {
  echo "Usage: scripts/publish-linux-apt.sh --staging DIR PACKAGE.deb... (see the comment at the top)" >&2
  exit 2
}

staging="" debs=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    --staging) [[ $# -ge 2 ]] || usage; staging="$2"; shift 2 ;;
    -h|--help) usage ;;
    -*) usage ;;
    *) debs+=("$1"); shift ;;
  esac
done
[[ -n "$staging" && ${#debs[@]} -gt 0 ]] || usage
for deb in "${debs[@]}"; do
  [[ -f "$deb" && "$deb" == *.deb ]] || { echo "Not a .deb file: $deb" >&2; exit 2; }
done
command -v gpg >/dev/null && command -v gpgv >/dev/null || { echo "GnuPG (gpg and gpgv) is required." >&2; exit 1; }
command -v "$engine" >/dev/null || { echo "$engine is required to read .deb packages (set EMAIL_CONTAINER_ENGINE)." >&2; exit 1; }

# The profile names the repository and its signing key; only a distribution
# profile with a repository can publish.
profile="$(python3 "$repo_root/scripts/distribution-profile.py" --root "$repo_root" path)"
[[ "$(email_profile_value "$repo_root" DISTRIBUTION)" == true ]] || { echo "Publishing requires a distribution profile, such as official." >&2; exit 2; }
repository="$(email_profile_value "$repo_root" LINUX_APT_REPOSITORY)"
fingerprint="$(email_profile_value "$repo_root" LINUX_APT_KEY_FINGERPRINT)"
[[ -n "$repository" ]] || { echo "The selected profile has no LINUX_APT_REPOSITORY." >&2; exit 2; }
keyring="$(python3 "$repo_root/scripts/apt-repository.py" keyring-path "$profile")"
python3 "$repo_root/scripts/apt-repository.py" check-keyring "$keyring" "$fingerprint" >/dev/null

: "${EMAIL_APT_GNUPGHOME:?Set EMAIL_APT_GNUPGHOME to the GnuPG home that holds the repository signing key}"
[[ -d "$EMAIL_APT_GNUPGHOME" ]] || { echo "EMAIL_APT_GNUPGHOME is not a directory." >&2; exit 2; }
gnupg_home="$(cd "$EMAIL_APT_GNUPGHOME" && pwd -P)"
case "$gnupg_home/" in
  "$(cd "$repo_root" && pwd -P)/"*) echo "EMAIL_APT_GNUPGHOME must be outside this checkout." >&2; exit 2 ;;
esac
# The secret key must be the profile's key, by full fingerprint.
secret_keys="$(gpg --homedir "$gnupg_home" --batch --with-colons --list-secret-keys -- "$fingerprint" 2>/dev/null || true)"
if ! grep -q "^fpr:::::::::$fingerprint:" <<<"$secret_keys"; then
  echo "EMAIL_APT_GNUPGHOME has no secret key with fingerprint $fingerprint (LINUX_APT_KEY_FINGERPRINT)." >&2
  exit 2
fi
sign_args=(--homedir "$gnupg_home" --batch --no-tty --yes --local-user "$fingerprint!" --digest-algo SHA512)
if [[ -n "${EMAIL_APT_PASSPHRASE_FILE:-}" ]]; then
  [[ -f "$EMAIL_APT_PASSPHRASE_FILE" ]] || { echo "EMAIL_APT_PASSPHRASE_FILE is not a file." >&2; exit 2; }
  sign_args+=(--pinentry-mode loopback --passphrase-file "$EMAIL_APT_PASSPHRASE_FILE")
fi

[[ ! -L "$staging" ]] || { echo "The staging directory must not be a symlink." >&2; exit 2; }
mkdir -p "$(dirname "$staging")"
staging="$(cd "$(dirname "$staging")" && pwd -P)/$(basename "$staging")"
scratch="$(mktemp -d "${TMPDIR:-/tmp}/email-apt-publish.XXXXXX")"
lock="" work="" backup=""
cleanup() {
  if [[ -n "$backup" && -d "$backup" && ! -e "$staging" ]]; then mv "$backup" "$staging"; fi
  rm -rf "$scratch"
  if [[ -n "$work" ]]; then rm -rf "$work"; fi
  if [[ -n "$lock" ]]; then rmdir "$lock"; fi
}
trap cleanup EXIT
if ! mkdir "$staging.publish-lock" 2>/dev/null; then
  echo "Another publisher owns $staging. If a prior run crashed, review and remove its empty .publish-lock directory." >&2
  exit 2
fi
lock="$staging.publish-lock"

# Read each package with dpkg-deb in a container without network access.
mounts=() pairs=() index=0
for deb in "${debs[@]}"; do
  index=$((index + 1))
  absolute="$(cd "$(dirname "$deb")" && pwd -P)/$(basename "$deb")"
  mounts+=(--mount "type=bind,source=$absolute,target=/in/$index/$(basename "$deb"),readonly")
  pairs+=("$absolute=$scratch/inspect/$index")
done
mkdir -p "$scratch/inspect"
"$engine" image inspect "$tools_image" >/dev/null 2>&1 || "$engine" pull -q "$tools_image" >/dev/null
# shellcheck disable=SC2016 # Expanded inside the container.
"$engine" run --rm --network none --user "$(id -u):$(id -g)" "${mounts[@]}" \
  --mount "type=bind,source=$scratch/inspect,target=/out" "$tools_image" bash -c '
set -euo pipefail
for directory in /in/*; do
  deb="$(ls "$directory"/*.deb)" out="/out/$(basename "$directory")"
  mkdir -p "$out"
  dpkg-deb -f "$deb" > "$out/control"
  dpkg-deb --ctrl-tarfile "$deb" | tar -xO ./conffiles > "$out/conffiles" 2>/dev/null || rm -f "$out/conffiles"
  dpkg-deb -c "$deb" | awk "{ print \$6 }" > "$out/contents"
  extract() { dpkg-deb --fsys-tarfile "$deb" | tar -xO "$1" > "$out/$2" 2>/dev/null || rm -f "$out/$2"; }
  extract ./etc/apt/sources.list.d/email-linux.sources sources
  extract ./usr/share/keyrings/email-linux-archive-keyring.asc keyring
  extract ./usr/share/doc/email-linux/RELEASE-CHANNEL.txt channel
done'

# Build the new repository beside the staging directory, from a copy of it.
work="$(mktemp -d "$(dirname "$staging")/.email-apt.XXXXXX")"
python3 "$repo_root/scripts/apt-repository.py" add --profile "$profile" --staging "$staging" \
  --work "$work" --changed "$scratch/changed" "${pairs[@]}"

while read -r suite; do
  [[ -n "$suite" ]] || continue
  release="$work/dists/$suite/Release"
  rm -f "$work/dists/$suite/InRelease" "$work/dists/$suite/Release.gpg"
  gpg "${sign_args[@]}" --clearsign --output "$work/dists/$suite/InRelease" "$release"
  gpg "${sign_args[@]}" --armor --detach-sign --output "$work/dists/$suite/Release.gpg" "$release"
done < "$scratch/changed"

# Check the result as a client would: indexes and pool against Release, and
# both signatures of every suite against the committed public key only.
python3 "$repo_root/scripts/apt-repository.py" verify "$work" --keyring "$keyring" --fingerprint "$fingerprint"

if [[ -e "$staging" ]]; then
  backup="$(mktemp -d "$(dirname "$staging")/.email-apt-previous.XXXXXX")"
  rmdir "$backup"
  mv "$staging" "$backup"
fi
mv "$work" "$staging"
work=""
if [[ -n "$backup" ]]; then rm -rf "$backup"; backup=""; fi
printf 'Staged and verified %s (suites changed: %s). Not uploaded.\n' "$staging" "$(tr '\n' ' ' < "$scratch/changed")"
printf 'Next: scripts/upload-updates.sh --dry-run --source %q --prefix email/apt\n' "$staging"
