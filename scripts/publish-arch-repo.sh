#!/usr/bin/env bash
# Adds email-ycorp-bin packages to a local, signed pacman repository staging
# directory, [ycorp] (docs/arch-updates.md). It never uploads:
# scripts/upload-updates.sh copies the verified directory to the update host
# as a separate, reviewed step.
#
# Usage: scripts/publish-arch-repo.sh --staging DIR PACKAGE.pkg.tar.zst...
#
#   --staging DIR   the repository as published, kept between releases: every
#                   earlier package stays, and the databases are updated from
#                   what is already there. Keep it (or recreate it from the
#                   published repository) before publishing again.
#
# The packages are the email-ycorp-bin builds that
# scripts/prepare-aur-package.sh --build writes to dist-linux/aur-bin, at most
# one per architecture (x86_64, aarch64). A .sig beside a package is used
# when it is the profile key's signature and refused otherwise; without one,
# the package is signed here.
#
# Environment:
#   EMAIL_APT_GNUPGHOME        GnuPG home holding the repository signing key,
#                              the same key as the APT repository (required).
#                              It must be outside this checkout.
#   EMAIL_APT_PASSPHRASE_FILE  optional file with that key's passphrase, for
#                              signing without a pinentry prompt
#   EMAIL_DISTRIBUTION_PROFILE the distribution profile (official), as for builds
#   EMAIL_CONTAINER_ENGINE     docker (default) or podman
#   EMAIL_ARCH_TOOLS_IMAGE     image with bsdtar and repo-add (default archlinux:base)
#
# Each package must be email-ycorp-bin repackaged from this profile's
# distribution .debs (its desktop entry and RELEASE-CHANNEL.txt), and newer
# than what the database already has for its architecture. Signing happens on
# this machine with gpg; the container only reads packages and runs repo-add,
# without network access, and never sees key material. Every signature is
# checked against the committed public key before the staging directory is
# replaced.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$repo_root/scripts/distribution-profile.sh"
engine="${EMAIL_CONTAINER_ENGINE:-docker}"
tools_image="${EMAIL_ARCH_TOOLS_IMAGE:-archlinux:base}"
# The official Arch image is x86_64 only; Apple Silicon runs it emulated.
platform=linux/amd64

usage() {
  echo "Usage: scripts/publish-arch-repo.sh --staging DIR PACKAGE.pkg.tar.zst... (see the comment at the top)" >&2
  exit 2
}

staging="" packages=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    --staging) [[ $# -ge 2 ]] || usage; staging="$2"; shift 2 ;;
    -h|--help) usage ;;
    -*) usage ;;
    *) packages+=("$1"); shift ;;
  esac
done
[[ -n "$staging" && ${#packages[@]} -gt 0 ]] || usage
for package in "${packages[@]}"; do
  [[ -f "$package" && "$package" == *.pkg.tar.zst ]] || { echo "Not a .pkg.tar.zst file: $package" >&2; exit 2; }
done
command -v gpg >/dev/null && command -v gpgv >/dev/null || { echo "GnuPG (gpg and gpgv) is required." >&2; exit 1; }
command -v "$engine" >/dev/null || { echo "$engine is required to read packages and run repo-add (set EMAIL_CONTAINER_ENGINE)." >&2; exit 1; }

# Only a distribution profile with a repository signing key can publish.
profile="$(python3 "$repo_root/scripts/distribution-profile.py" --root "$repo_root" path)"
[[ "$(email_profile_value "$repo_root" DISTRIBUTION)" == true ]] || { echo "Publishing requires a distribution profile, such as official." >&2; exit 2; }
fingerprint="$(email_profile_value "$repo_root" LINUX_APT_KEY_FINGERPRINT)"
[[ -n "$fingerprint" ]] || { echo "The selected profile has no LINUX_APT_KEY_FINGERPRINT." >&2; exit 2; }
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
# pacman reads binary detached signatures only.
sign() { gpg "${sign_args[@]}" --detach-sign --no-armor --output "$1.sig" "$1"; }

[[ ! -L "$staging" ]] || { echo "The staging directory must not be a symlink." >&2; exit 2; }
mkdir -p "$(dirname "$staging")"
staging="$(cd "$(dirname "$staging")" && pwd -P)/$(basename "$staging")"
scratch="$(mktemp -d "${TMPDIR:-/tmp}/email-arch-publish.XXXXXX")"
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

"$engine" image inspect "$tools_image" >/dev/null 2>&1 || "$engine" pull -q --platform "$platform" "$tools_image" >/dev/null
container() { "$engine" run --rm --platform "$platform" --network none --user "$(id -u):$(id -g)" "$@"; }

# Read each package with bsdtar in a container without network access.
mounts=() pairs=() index=0
for package in "${packages[@]}"; do
  index=$((index + 1))
  absolute="$(cd "$(dirname "$package")" && pwd -P)/$(basename "$package")"
  mounts+=(--mount "type=bind,source=$absolute,target=/in/$index/$(basename "$package"),readonly")
  pairs+=("$absolute=$scratch/inspect/$index")
done
mkdir -p "$scratch/inspect"
# shellcheck disable=SC2016 # Expanded inside the container.
container "${mounts[@]}" --mount "type=bind,source=$scratch/inspect,target=/out" "$tools_image" bash -c '
set -euo pipefail
for directory in /in/*; do
  package="$(ls "$directory"/*.pkg.tar.zst)" out="/out/$(basename "$directory")"
  mkdir -p "$out"
  bsdtar -xOqf "$package" .PKGINFO > "$out/pkginfo"
  bsdtar -tf "$package" > "$out/contents"
  bsdtar -xOqf "$package" usr/share/licenses/email-ycorp-bin/RELEASE-CHANNEL.txt > "$out/channel" 2>/dev/null \
    || rm -f "$out/channel"
done'

# Build the new repository beside the staging directory, from a copy of it.
# The copy is checked against the committed key before anything is added.
work="$(mktemp -d "$(dirname "$staging")/.email-arch.XXXXXX")"
python3 "$repo_root/scripts/arch-repository.py" stage --profile "$profile" --staging "$staging" \
  --work "$work" --added "$scratch/added" "${pairs[@]}"

architectures=()
while IFS=$'\t' read -r arch name signed; do
  [[ -n "$arch" ]] || continue
  if [[ "$signed" == 0 ]]; then sign "$work/$arch/$name"; fi
  architectures+=("$arch")
done < "$scratch/added"

# repo-add embeds each package signature in the database (--include-sigs),
# so pacman checks it without fetching the .sig. Signing the database is
# done here, after the container is gone.
# shellcheck disable=SC2016
container --mount "type=bind,source=$work,target=/repo" \
  --mount "type=bind,source=$scratch/added,target=/added,readonly" "$tools_image" bash -c '
set -euo pipefail
while read -r arch name _; do
  repo-add --quiet --include-sigs "/repo/$arch/ycorp.db.tar.gz" "/repo/$arch/$name"
done < /added' >"$scratch/repo-add.log" 2>&1 || { cat "$scratch/repo-add.log" >&2; echo "repo-add failed." >&2; exit 1; }
python3 "$repo_root/scripts/arch-repository.py" finalize "$work" "${architectures[@]}"
for arch in "${architectures[@]}"; do
  for database in db files; do
    sign "$work/$arch/ycorp.$database.tar.gz"
    cp "$work/$arch/ycorp.$database.tar.gz.sig" "$work/$arch/ycorp.$database.sig"
  done
done

# Check the result as pacman would: every database entry against its package,
# and every package and database signature against the committed public key.
python3 "$repo_root/scripts/arch-repository.py" verify "$work" --keyring "$keyring" --fingerprint "$fingerprint"

if [[ -e "$staging" ]]; then
  backup="$(mktemp -d "$(dirname "$staging")/.email-arch-previous.XXXXXX")"
  rmdir "$backup"
  mv "$staging" "$backup"
fi
mv "$work" "$staging"
work=""
if [[ -n "$backup" ]]; then rm -rf "$backup"; backup=""; fi
printf 'Staged and verified %s (architectures changed: %s). Not uploaded.\n' "$staging" "${architectures[*]}"
printf 'Next: scripts/upload-updates.sh --dry-run --source %q --prefix email/arch\n' "$staging"
