#!/usr/bin/env bash
# End-to-end test of the [ycorp] pacman repository (docs/arch-updates.md),
# with a throwaway signing key. It never touches the production key or the
# update bucket; besides the containers it starts, it only downloads the
# release .debs from GitHub.
#
#   1. Generates a throwaway ed25519 GnuPG key and a fixture distribution
#      profile: the official profile with that key.
#   2. Downloads the amd64 and arm64 .debs of v0.1.0-beta.2 and v0.1.0-beta.3
#      (official-profile release builds) and checks them against each
#      release's SHA256SUMS.
#   3. Builds email-ycorp-bin 0.1.0beta2-1 and 0.1.0beta3-1 for x86_64 and
#      aarch64 from apps/linux/packaging/aur-bin/PKGBUILD with makepkg, as
#      scripts/prepare-aur-package.sh does, plus two altered beta.3 builds (a
#      local-alpha channel and another profile's desktop entry).
#   4. Publishes them with scripts/publish-arch-repo.sh, and checks that the
#      publisher refuses the altered builds, a package signed by another key,
#      a signing key other than the profile's, and an older or repeated
#      version, and leaves the staging directory unchanged.
#   5. Serves the staged repository over HTTP from a container on an internal
#      Docker network, and in an Arch Linux x86_64 container:
#      - runs the setup from docs/arch-updates.md (pacman-key --add and
#        --lsign-key, the [ycorp] section, pacman -Syu email-ycorp-bin) with
#        beta.2 published, and checks the package was validated by signature;
#      - with beta.3 on offer, checks that a tampered database, an unsigned
#        database, a tampered package, an unsigned package and a package
#        signed by another key are each refused and install nothing;
#      - publishes beta.3 and checks that pacman -Syu upgrades to it, that
#        the installed binaries find all their libraries and that mail-mcp
#        runs.
#      With --alarm-rootfs, it repeats the setup and the upgrade in an Arch
#      Linux ARM (aarch64) container made from that rootfs.
#   6. Checks the upload plan for the staged repository (dry run, no network).
#
# Usage: scripts/test-arch-repo.sh [--keep] [--debs DIR] [--alarm-rootfs FILE]
#   --keep          keep the work directory, containers and network
#   --debs DIR      use email-linux_<version>_<arch>.deb files from DIR
#                   instead of downloading them
#   --alarm-rootfs FILE
#                   also test on Arch Linux ARM, from the official
#                   ArchLinuxARM-aarch64-latest.tar.gz (os.archlinuxarm.org;
#                   check its .sig first). Native on an arm64 host.
#
# Needs Docker (or EMAIL_CONTAINER_ENGINE=podman), GnuPG, curl and Python 3.
# The Arch containers are x86_64 and run emulated on Apple Silicon. The first
# run builds a test image with the package's runtime libraries, makepkg and
# Python (a few minutes), and with --alarm-rootfs an Arch Linux ARM one;
# later runs reuse them.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
engine="${EMAIL_CONTAINER_ENGINE:-docker}"
platform=linux/amd64
keep=0 debs_from="" alarm_rootfs=""
usage() { echo "Usage: scripts/test-arch-repo.sh [--keep] [--debs DIR] [--alarm-rootfs FILE]" >&2; exit 2; }
while [[ $# -gt 0 ]]; do
  case "$1" in
    --keep) keep=1; shift ;;
    --debs) [[ $# -ge 2 ]] || usage; debs_from="$2"; shift 2 ;;
    --alarm-rootfs)
      [[ $# -ge 2 && -f "$2" ]] || usage
      alarm_rootfs="$(cd "$(dirname "$2")" && pwd)/$(basename "$2")"; shift 2 ;;
    *) usage ;;
  esac
done
for tool in "$engine" gpg gpgconf python3 curl shasum; do
  command -v "$tool" >/dev/null || { echo "$tool is required." >&2; exit 1; }
done

host=arch.email-test.invalid
releases=(0.1.0-beta.2 0.1.0-beta.3)
work="$(mktemp -d "${TMPDIR:-/tmp}/email-arch-test.XXXXXX")"
# GnuPG puts its agent socket in the home directory; keep that path short.
gnupg="$(mktemp -d /tmp/email-arch-gpg.XXXXXX)"
other_gnupg="$(mktemp -d /tmp/email-arch-gpg.XXXXXX)"
run_id="email-arch-test-$$"
network="$run_id" server="$run_id-server" client="$run_id-client" alarm_client="$run_id-alarm"
cleanup() {
  local result=$?
  gpgconf --homedir "$gnupg" --kill gpg-agent 2>/dev/null || true
  gpgconf --homedir "$other_gnupg" --kill gpg-agent 2>/dev/null || true
  rm -rf "$gnupg" "$other_gnupg"
  if [[ "$keep" == 1 ]]; then
    echo "Kept $work, containers $server, $run_id-client and $alarm_client, and network $network." >&2
  else
    "$engine" rm -f "$server" "$run_id-client" "$alarm_client" >/dev/null 2>&1 || true
    "$engine" network rm "$network" >/dev/null 2>&1 || true
    rm -rf "$work"
  fi
  exit "$result"
}
trap cleanup EXIT

passed=()
pass() { passed+=("$1"); printf 'ok   %s\n' "$1"; }
fail() { printf 'FAIL %s\n' "$1" >&2; exit 1; }
tree_hash() { (cd "$1" && find . -type f ! -name '.*' -print0 | LC_ALL=C sort -z | xargs -0 shasum -a 256) | shasum -a 256; }
pkgver() { printf '%s\n' "$1" | sed -E 's/-([A-Za-z]+)\.?/\1/'; } # scripts/release-version.sh
debarch() { case "$1" in x86_64) echo amd64 ;; aarch64) echo arm64 ;; esac; }
package() { printf '%s/packages/%s/email-ycorp-bin-%s-%s.pkg.tar.zst\n' "$work" "$1" "$2" "$3"; } # label version arch

# 1. Throwaway signing keys and the fixture profile.
chmod 700 "$gnupg" "$other_gnupg"
for home in "$gnupg" "$other_gnupg"; do
  gpg --homedir "$home" --batch --quiet --passphrase '' \
    --quick-generate-key "Email pacman test, throwaway <arch-test@email-test.invalid>" ed25519 sign 1d 2>/dev/null
done
key_of() { gpg --homedir "$1" --batch --with-colons --list-secret-keys 2>/dev/null | awk -F: '/^fpr:/ { print $10; exit }'; }
fingerprint="$(key_of "$gnupg")"
[[ "$fingerprint" =~ ^[0-9A-F]{40}$ ]] || fail "throwaway key generation"
[[ "$fingerprint" != 20B087BF1D1B2A04102882F213081979B6CEABDA ]] || fail "the test must never use the production key"
mkdir -p "$work/profile" "$work/debs" "$work/repo-parent"
profile="$work/profile/archfixture.env"
sed -e 's/^PROFILE=.*/PROFILE=archfixture/' \
    -e "s/^LINUX_APT_KEY_FINGERPRINT=.*/LINUX_APT_KEY_FINGERPRINT=$fingerprint/" \
    "$repo_root/config/profiles/official.env" > "$profile"
gpg --homedir "$gnupg" --batch --armor --export "$fingerprint" > "$work/profile/archfixture.apt.asc"
app_id="$(EMAIL_DISTRIBUTION_PROFILE="$profile" python3 "$repo_root/scripts/distribution-profile.py" get LINUX_APP_ID)" \
  || fail "fixture profile"
pass "throwaway signing key $fingerprint and fixture profile ($app_id)"

# 2. The official release .debs.
for release in "${releases[@]}"; do
  for arch in x86_64 aarch64; do
    name="email-linux_${release}_$(debarch "$arch").deb"
    if [[ -n "$debs_from" ]]; then
      cp "$debs_from/$name" "$work/debs/$name" || fail "$name is not in $debs_from"
      continue
    fi
    base="https://github.com/YCorpLabs/Email/releases/download/v$release"
    [[ -f "$work/debs/SHA256SUMS-$release" ]] || curl -fsSL -o "$work/debs/SHA256SUMS-$release" "$base/SHA256SUMS" \
      || fail "downloading SHA256SUMS of v$release"
    curl -fsSL -o "$work/debs/$name" "$base/$name" || fail "downloading $name"
    expected="$(awk -v name="$name" '$2 == name || $2 == "*" name { print $1 }' "$work/debs/SHA256SUMS-$release")"
    [[ -n "$expected" && "$(shasum -a 256 "$work/debs/$name" | awk '{ print $1 }')" == "$expected" ]] \
      || fail "$name does not match the release's SHA256SUMS"
  done
done
pass "release .debs for ${releases[*]} (amd64 and arm64)$([[ -n "$debs_from" ]] && echo " from $debs_from" || echo ", checked against SHA256SUMS")"

# 3. The test image: Arch with makepkg, Python (the HTTP server) and the
# package's runtime libraries, so the client needs no network but the test's.
pkgbuild="$repo_root/apps/linux/packaging/aur-bin/PKGBUILD"
depends="$(bash -c 'source "$1" >/dev/null 2>&1; printf "%s\n" "${depends[@]}" "${depends_aarch64[@]}"' _ "$pkgbuild" | LC_ALL=C sort -u | tr '\n' ' ')"
"$engine" image inspect archlinux:base >/dev/null 2>&1 || "$engine" pull -q --platform "$platform" archlinux:base >/dev/null
base_id="$("$engine" image inspect -f '{{.Id}}' archlinux:base)"
test_image="email-arch-test:$(printf '%s %s' "$base_id" "$depends" | shasum -a 256 | cut -c1-12)"
if ! "$engine" image inspect "$test_image" >/dev/null 2>&1; then
  mkdir -p "$work/image"
  # User-mode CPU emulation cannot install pacman's download syscall filter
  # (see prepare-aur-package.sh); signature checks stay enabled.
  cat > "$work/image/Dockerfile" <<EOF
FROM archlinux:base
RUN sed -i '/^\[options\]/a DisableSandboxSyscalls' /etc/pacman.conf \\
 && pacman -Syu --noconfirm --needed base-devel python $depends >/dev/null \\
 && rm -rf /var/cache/pacman/pkg/* \\
 && useradd --create-home builder
EOF
  "$engine" build --platform "$platform" -q -t "$test_image" "$work/image" >"$work/test-image.log" 2>&1 \
    || { tail -20 "$work/test-image.log" >&2; fail "test image"; }
fi
pass "test image $test_image"

# Packages, built from the PKGBUILD as prepare-aur-package.sh builds them:
# aarch64 on this x86_64 container with only CARCH changed.
build_dir() { # label release arch [package() addition]
  local label="$1" release="$2" arch="$3" deb sum dir="$work/build/$1"
  deb="$work/debs/email-linux_${release}_$(debarch "$arch").deb"
  sum="$(shasum -a 256 "$deb" | awk '{ print $1 }')"
  mkdir -p "$dir"
  sed -e "s/^pkgver=.*/pkgver=$(pkgver "$release")/" -e "s/^_tag=.*/_tag=v$release/" \
      -e "s/^_debver=.*/_debver=$release/" -e "s/^pkgrel=.*/pkgrel=1/" \
      -e "s/^sha256sums_$arch=.*/sha256sums_$arch=('$sum')/" "$pkgbuild" > "$dir/PKGBUILD"
  if [[ -n "${4:-}" ]]; then
    # shellcheck disable=SC2016 # Written into the PKGBUILD as is.
    printf '\neval "_release_$(declare -f package)"\npackage() {\n  _release_package\n  %s\n}\n' "$4" >> "$dir/PKGBUILD"
  fi
  cp "$deb" "$dir/email-ycorp-bin-$(pkgver "$release")-$(debarch "$arch").deb"
  echo "$arch" > "$dir/ARCH"
}
for release in "${releases[@]}"; do
  for arch in x86_64 aarch64; do build_dir "$(pkgver "$release")-$arch" "$release" "$arch"; done
done
# shellcheck disable=SC2016 # Lines for the PKGBUILD's package().
build_dir channel 0.1.0-beta.3 x86_64 'printf "local-alpha\n" > "$pkgdir/usr/share/licenses/$pkgname/RELEASE-CHANNEL.txt"'
# shellcheck disable=SC2016
build_dir profile 0.1.0-beta.3 x86_64 \
  'mv "$pkgdir/usr/share/applications/$_appid.desktop" "$pkgdir/usr/share/applications/invalid.email.Unofficial.desktop"'
mkdir -p "$work/packages"
# shellcheck disable=SC2016 # Expanded inside the container.
"$engine" run --rm --platform "$platform" --network none \
  --mount "type=bind,source=$work/build,target=/in,readonly" \
  --mount "type=bind,source=$work/packages,target=/out" "$test_image" bash -c '
set -euo pipefail
sed -e "s/^CARCH=.*/CARCH=\"aarch64\"/" -e "s/^CHOST=.*/CHOST=\"aarch64-unknown-linux-gnu\"/" \
  /etc/makepkg.conf > /etc/makepkg-aarch64.conf
for input in /in/*; do
  label="$(basename "$input")" dir="/home/builder/$(basename "$input")"
  install -d -o builder "$dir"
  install -o builder -m644 "$input"/* "$dir/"
  config=/etc/makepkg.conf
  if [[ "$(cat "$dir/ARCH")" == aarch64 ]]; then config=/etc/makepkg-aarch64.conf; fi
  (cd "$dir" && runuser -u builder -- makepkg --nodeps --noconfirm --cleanbuild --config "$config") \
    > "/out/$label.log" 2>&1 || { tail -40 "/out/$label.log" >&2; exit 1; }
  mkdir -p "/out/$label"
  cp "$dir"/*.pkg.tar.zst "/out/$label/"
done' || fail "building packages with makepkg"
for release in "${releases[@]}"; do
  for arch in x86_64 aarch64; do
    [[ -f "$(package "$(pkgver "$release")-$arch" "$(pkgver "$release")-1" "$arch")" ]] || fail "no $release $arch package"
  done
done
pass "built email-ycorp-bin 0.1.0beta2-1 and 0.1.0beta3-1 for x86_64 and aarch64, and two altered builds"

# 4. Publishing, and what the publisher refuses.
repo="$work/repo-parent/arch"
publish() {
  EMAIL_DISTRIBUTION_PROFILE="$profile" EMAIL_APT_GNUPGHOME="${publish_gnupg:-$gnupg}" \
    "$repo_root/scripts/publish-arch-repo.sh" --staging "$repo" "$@"
}
snapshot() { rm -rf "$work/snap-$1"; cp -R "$repo" "$work/snap-$1"; }
refused() { # label pattern package...
  local label="$1" pattern="$2"; shift 2
  if publish "$@" >"$work/publish-$label.log" 2>&1; then fail "publisher accepted $label"; fi
  grep -Eq "$pattern" "$work/publish-$label.log" || { cat "$work/publish-$label.log" >&2; fail "$label: expected /$pattern/"; }
}
b2() { package "0.1.0beta2-$1" 0.1.0beta2-1 "$1"; }
b3() { package "0.1.0beta3-$1" 0.1.0beta3-1 "$1"; }
publish "$(b2 x86_64)" "$(b2 aarch64)" >"$work/publish-1.log" 2>&1 || { cat "$work/publish-1.log" >&2; fail "publishing beta.2"; }
snapshot 1
before="$(tree_hash "$repo")"
refused channel "not repackaged from a distribution build" "$(package channel 0.1.0beta3-1 x86_64)"
refused profile "no /usr/share/applications/$app_id.desktop" "$(package profile 0.1.0beta3-1 x86_64)"
mkdir -p "$work/other-signed"
cp "$(b3 x86_64)" "$work/other-signed/"
gpg --homedir "$other_gnupg" --batch --yes --no-armor --detach-sign \
  -o "$work/other-signed/$(basename "$(b3 x86_64)").sig" "$work/other-signed/$(basename "$(b3 x86_64)")" 2>/dev/null
refused other-signed "is not a valid signature of .* by $fingerprint" "$work/other-signed/$(basename "$(b3 x86_64)")"
publish_gnupg="$other_gnupg" refused other-key "no secret key with fingerprint $fingerprint" "$(b3 x86_64)"
refused two-versions "one package per architecture" "$(b3 x86_64)" "$(b2 x86_64)"
[[ "$(tree_hash "$repo")" == "$before" && ! -e "$repo.publish-lock" ]] || fail "a refused publication changed the staging directory"
pass "publisher refuses another channel, another profile, a package signed by another key and another signing key"
publish "$(b3 x86_64)" "$(b3 aarch64)" >"$work/publish-2.log" 2>&1 || { cat "$work/publish-2.log" >&2; fail "publishing beta.3"; }
snapshot 2
before="$(tree_hash "$repo")"
refused older "0.1.0beta2-1 is not newer than 0.1.0beta3-1" "$(b2 x86_64)"
refused repeated "0.1.0beta3-1 is not newer than 0.1.0beta3-1" "$(b3 aarch64)"
[[ "$(tree_hash "$repo")" == "$before" ]] || fail "a refused publication changed the staging directory"
python3 - "$repo_root/scripts/arch-repository.py" "$repo" <<'PY' || fail "staged layout"
import importlib.util, sys
from pathlib import Path
spec = importlib.util.spec_from_file_location("arch", sys.argv[1])
arch = importlib.util.module_from_spec(spec); spec.loader.exec_module(arch)
root = Path(sys.argv[2])
assert not [p for p in root.rglob("*") if p.is_symlink()], "symlinks"
for a in ("x86_64", "aarch64"):
    names = sorted(p.name for p in (root / a).iterdir())
    packages = [f"email-ycorp-bin-0.1.0beta{v}-1-{a}.pkg.tar.zst" for v in (2, 3)]
    assert names == sorted(packages + [p + ".sig" for p in packages] + arch.database_files()), names
    entry = arch.read_db(root, a)["email-ycorp-bin"]
    assert arch.field(entry, "VERSION") == "0.1.0beta3-1" and arch.field(entry, "ARCH") == a, entry
    for db in ("db", "files"):
        assert (root / a / f"ycorp.{db}").read_bytes() == (root / a / f"ycorp.{db}.tar.gz").read_bytes()
assert sorted(p.name for p in root.iterdir() if not p.name.startswith(".")) == ["aarch64", "x86_64", "ycorp.asc"]
PY
pass "publisher refuses an older or repeated version; the staged repository is real files, keeps beta.2 and lists beta.3"

# 5. Clients against the served repository.
mkdir -p "$work/served"
"$engine" network create --internal "$network" >/dev/null
"$engine" run -d --name "$server" --platform "$platform" --network "$network" --network-alias "$host" \
  "$test_image" python3 -m http.server 80 --directory /srv >/dev/null
# The served files are copied into the server container, never bind-mounted:
# a bind mount can show stale files right after they are replaced. Copies
# also get fresh modification times, so pacman never takes them for unchanged.
stage() { rm -rf "$work/served/arch"; cp -R "$1" "$work/served/arch"; }
publish_served() {
  "$engine" exec "$server" rm -rf /srv/email/arch
  "$engine" exec "$server" mkdir -p /srv/email
  "$engine" cp "$work/served/arch" "$server:/srv/email/arch" >/dev/null
}
serve() { stage "$1"; publish_served; } # served as http://$host/email/arch
client() { "$engine" exec -i "$client" bash -c "set -euo pipefail; $1" </dev/null; }
installed() { client "pacman -Q email-ycorp-bin 2>/dev/null || true"; }
start_client() { # name platform image; client() acts on it from now on
  "$engine" run -d --name "$1" --platform "$2" --network "$network" "$3" sleep infinity >/dev/null
  client="$1"
  # Only the test repository: the distribution's repositories are unreachable
  # on purpose, and the runtime libraries are already in the image.
  client "awk '/^\\[/ { keep = (\$0 == \"[options]\") } keep' /etc/pacman.conf > /tmp/pacman.conf && cat /tmp/pacman.conf > /etc/pacman.conf"
  client "getent hosts $host" >/dev/null || fail "the client cannot resolve $host on the test network"
  # An installed system has a local master key, which --lsign-key signs with;
  # the Arch Docker image leaves it out on purpose. --init makes one only
  # when there is none.
  client "pacman-key --init >/dev/null 2>&1" || fail "pacman-key --init"
}

# The setup from the docs, with the test's host and key.
python3 - "$repo_root/docs/arch-updates.md" "$host" "$fingerprint" > "$work/setup.sh" <<'PY' || fail "setup snippet in docs/arch-updates.md"
import re, sys
text = open(sys.argv[1]).read()
blocks = [b for b in re.findall(r"```sh\n(.*?)```", text, re.S) if "pacman-key --lsign-key" in b]
assert len(blocks) == 1, "expected one setup block"
block = blocks[0]
replacements = [("https://updates.ycorplabs.com", f"http://{sys.argv[2]}"),
                ("20B087BF1D1B2A04102882F213081979B6CEABDA", sys.argv[3]),
                ("pacman -Syu email-ycorp-bin", "pacman -Syu --noconfirm email-ycorp-bin")]
for old, new in replacements:
    assert old in block, f"the setup block has no {old!r}"
    block = block.replace(old, new)
sys.stdout.write('set -euo pipefail\nsudo() { "$@"; }\ncd "$(mktemp -d)"\n' + block)
PY
documented_setup() { # arch: with beta.2 published, run the setup and check the install
  serve "$work/snap-1"
  "$engine" exec -i "$client" bash -c 'cat > /tmp/setup.sh' < "$work/setup.sh"
  client "bash /tmp/setup.sh 2>&1" >"$work/setup-$1.log" || { cat "$work/setup-$1.log" >&2; fail "the documented setup ($1)"; }
  [[ "$(installed)" == "email-ycorp-bin 0.1.0beta2-1" ]] || { cat "$work/setup-$1.log" >&2; fail "setup installed $(installed) ($1)"; }
  client "pacman -Qi email-ycorp-bin" > "$work/info-$1-1.txt"
  grep -Eq '^Validated By +: Signature' "$work/info-$1-1.txt" \
    || { cat "$work/info-$1-1.txt" >&2; fail "beta.2 was not validated by signature ($1)"; }
  grep -Eq "^Architecture +: $1\$" "$work/info-$1-1.txt" || { cat "$work/info-$1-1.txt" >&2; fail "not the $1 package"; }
  client "tail -4 /etc/pacman.conf" > "$work/pacman-conf-$1.txt"
  # shellcheck disable=SC2016 # pacman expands $arch.
  if ! grep -qx '\[ycorp\]' "$work/pacman-conf-$1.txt" \
      || ! grep -qx 'Server = http://'"$host"'/email/arch/$arch' "$work/pacman-conf-$1.txt"; then
    cat "$work/pacman-conf-$1.txt" >&2; fail "the [ycorp] section ($1)"
  fi
  pass "$1: the setup from docs/arch-updates.md trusts the key, adds [ycorp] and installs 0.1.0beta2-1, validated by signature"
}
genuine_upgrade() { # arch: publish beta.3 and upgrade the way users do
  serve "$work/snap-2"
  client "rm -rf /var/cache/pacman/pkg/*"
  client "pacman -Syu --noconfirm 2>&1" >"$work/upgrade-$1.log" || { cat "$work/upgrade-$1.log" >&2; fail "pacman -Syu to beta.3 ($1)"; }
  [[ "$(installed)" == "email-ycorp-bin 0.1.0beta3-1" ]] || { cat "$work/upgrade-$1.log" >&2; fail "beta.3 not installed: $(installed) ($1)"; }
  client "pacman -Qi email-ycorp-bin" > "$work/info-$1-2.txt"
  grep -Eq '^Validated By +: Signature' "$work/info-$1-2.txt" \
    || { cat "$work/info-$1-2.txt" >&2; fail "beta.3 was not validated by signature ($1)"; }
  client "ldd /usr/bin/email-linux /usr/lib/email-linux/mail-mcp" > "$work/ldd-$1.txt" || fail "ldd ($1)"
  if grep 'not found' "$work/ldd-$1.txt"; then fail "an installed binary is missing a library ($1)"; fi
  client "/usr/lib/email-linux/mail-mcp --help 2>&1" > "$work/mail-mcp-$1.txt" || fail "mail-mcp --help ($1)"
  grep -q '^Usage: mail-mcp' "$work/mail-mcp-$1.txt" || { cat "$work/mail-mcp-$1.txt" >&2; fail "mail-mcp does not run ($1)"; }
  client "pacman -Syu --noconfirm 2>&1" >"$work/upgrade-again-$1.log" \
    || { cat "$work/upgrade-again-$1.log" >&2; fail "pacman -Syu again ($1)"; }
  grep -q "there is nothing to do" "$work/upgrade-again-$1.log" \
    || { cat "$work/upgrade-again-$1.log" >&2; fail "a second pacman -Syu changed something ($1)"; }
  "$engine" logs "$server" >"$work/server.log" 2>&1
  for path in "$1/ycorp.db" "$1/ycorp.db.sig" "$1/email-ycorp-bin-0.1.0beta3-1-$1.pkg.tar.zst"; do
    grep -q "GET /email/arch/$path HTTP" "$work/server.log" || fail "the client never fetched $path"
  done
  pass "$1: pacman -Syu upgraded 0.1.0beta2-1 to 0.1.0beta3-1, validated by signature; the binaries find their libraries and mail-mcp runs"
}

start_client "$client" "$platform" "$test_image"
documented_setup x86_64

# Each of these offers beta.3 and must install nothing.
rejected() { # label pattern
  client "rm -rf /var/cache/pacman/pkg/*"
  if client "pacman -Syyu --noconfirm 2>&1" >"$work/client-$1.log"; then
    cat "$work/client-$1.log" >&2; fail "$1 was accepted"
  fi
  grep -Eq "$2" "$work/client-$1.log" || { cat "$work/client-$1.log" >&2; fail "$1: expected /$2/"; }
  [[ "$(installed)" == "email-ycorp-bin 0.1.0beta2-1" ]] || fail "$1 installed $(installed)"
  pass "$1 refused; 0.1.0beta2-1 stays installed"
}
variant() { # Python run with root = the served copy of snapshot 2, then served
  stage "$work/snap-2"
  python3 -c "import sys
from pathlib import Path
root = Path(sys.argv[1]) / 'x86_64'
$1" "$work/served/arch"
}
# repo-add on the served copy, then its database signed with KEY_HOME.
redo_database() { # key_home [repo-add option]
  local dir="$work/served/arch/x86_64"
  rm -f "$dir"/ycorp.{db,files}{,.sig} "$dir"/ycorp.{db,files}.tar.gz.sig
  # shellcheck disable=SC2086
  "$engine" run --rm --platform "$platform" --network none --user "$(id -u):$(id -g)" \
    --mount "type=bind,source=$dir,target=/repo" "$test_image" \
    repo-add --quiet ${2:-} /repo/ycorp.db.tar.gz "/repo/$(basename "$(b3 x86_64)")" >"$work/repo-add.log" 2>&1 \
    || { cat "$work/repo-add.log" >&2; fail "repo-add for a test variant"; }
  python3 "$repo_root/scripts/arch-repository.py" finalize "$work/served/arch" x86_64
  for database in db files; do
    gpg --homedir "$1" --batch --yes --no-armor --detach-sign -o "$dir/ycorp.$database.tar.gz.sig" "$dir/ycorp.$database.tar.gz" 2>/dev/null
    cp "$dir/ycorp.$database.tar.gz.sig" "$dir/ycorp.$database.sig"
  done
}
variant '
import io, tarfile
data = (root / "ycorp.db").read_bytes()
out = io.BytesIO()
with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as old, tarfile.open(fileobj=out, mode="w:gz") as new:
    for member in old.getmembers():
        content = old.extractfile(member).read() if member.isfile() else None
        if member.name.endswith("/desc"):
            content = content.replace(b"%DESC%\n", b"%DESC%\nTampered. ", 1)
            member.size = len(content)
        new.addfile(member, io.BytesIO(content) if content is not None else None)
for name in ("ycorp.db", "ycorp.db.tar.gz"):
    (root / name).write_bytes(out.getvalue())'
publish_served
rejected "tampered database" 'invalid or corrupted database \(PGP signature\)'
variant 'for name in ("ycorp.db.sig", "ycorp.db.tar.gz.sig"): (root / name).unlink()'
publish_served
rejected "unsigned database" 'missing required signature|failed retrieving file .ycorp\.db\.sig'
# One byte changed, same size: only the signature and checksum can tell.
variant '
p = next(root.glob("*0.1.0beta3-1-x86_64.pkg.tar.zst"))
data = bytearray(p.read_bytes())
data[len(data) // 2] ^= 0xFF
p.write_bytes(bytes(data))'
publish_served
rejected "tampered package" 'invalid or corrupted package'
stage "$work/snap-2"
rm "$work/served/arch/x86_64/$(basename "$(b3 x86_64)").sig"
redo_database "$gnupg"
publish_served
rejected "unsigned package" 'missing required signature|failed retrieving file .*\.pkg\.tar\.zst\.sig'
stage "$work/snap-2"
gpg --homedir "$other_gnupg" --batch --yes --no-armor --detach-sign \
  -o "$work/served/arch/x86_64/$(basename "$(b3 x86_64)").sig" "$work/served/arch/x86_64/$(basename "$(b3 x86_64)")" 2>/dev/null
redo_database "$gnupg" --include-sigs
publish_served
rejected "package signed by another key" \
  'unknown public key|unknown trust|invalid or corrupted package|required key missing|could not be looked up'

# The genuine update, the way users get it.
genuine_upgrade x86_64

# Optional: the same setup and upgrade on Arch Linux ARM, from its official
# rootfs (native on an arm64 host).
if [[ -n "$alarm_rootfs" ]]; then
  alarm_platform=linux/arm64
  rootfs_id="$(shasum -a 256 "$alarm_rootfs" | cut -c1-12)"
  alarm_base="email-alarm-base:$rootfs_id"
  "$engine" image inspect "$alarm_base" >/dev/null 2>&1 \
    || "$engine" import --platform "$alarm_platform" "$alarm_rootfs" "$alarm_base" >/dev/null \
    || fail "importing $alarm_rootfs"
  alarm_image="email-alarm-test:$(printf '%s %s' "$rootfs_id" "$depends" | shasum -a 256 | cut -c1-12)"
  if ! "$engine" image inspect "$alarm_image" >/dev/null 2>&1; then
    mkdir -p "$work/alarm-image"
    # A container has no kernel, firmware or boot loader to update, and
    # cannot apply pacman's download sandbox (Landlock and the alpm user);
    # signature checks stay enabled.
    # shellcheck disable=SC1003,SC2016 # Dockerfile lines, written as they are.
    {
      echo "FROM $alarm_base"
      echo 'RUN sed -i "/^\[options\]/a DisableSandbox" /etc/pacman.conf \'
      echo ' && pacman-key --init >/dev/null 2>&1 && pacman-key --populate archlinuxarm >/dev/null 2>&1 \'
      echo ' && boot="$(pacman -Qq | grep -E "^(linux-aarch64.*|linux-firmware.*|mkinitcpio.*|uboot-.*)$" || true)" \'
      echo ' && { [ -z "$boot" ] || pacman -Rdd --noconfirm $boot >/dev/null; } \'
      echo " && pacman -Syu --noconfirm --needed $depends >/dev/null \\"
      echo ' && rm -rf /var/cache/pacman/pkg/*'
    } > "$work/alarm-image/Dockerfile"
    "$engine" build --platform "$alarm_platform" -q -t "$alarm_image" "$work/alarm-image" >"$work/alarm-image.log" 2>&1 \
      || { tail -20 "$work/alarm-image.log" >&2; fail "Arch Linux ARM test image"; }
  fi
  pass "Arch Linux ARM test image $alarm_image, from $(basename "$alarm_rootfs")"
  start_client "$alarm_client" "$alarm_platform" "$alarm_image"
  documented_setup aarch64
  genuine_upgrade aarch64
fi

# 6. Upload plan for the staged repository (no network, nothing uploaded).
EMAIL_DISTRIBUTION_PROFILE="$profile" "$repo_root/scripts/upload-updates.sh" --dry-run --skip-remote-check \
  --source "$repo" --prefix email/arch >"$work/upload-plan.log" 2>&1 || { cat "$work/upload-plan.log" >&2; fail "upload plan"; }
python3 - "$work/upload-plan.log" <<'PY' || { cat "$work/upload-plan.log" >&2; fail "upload order"; }
import re, sys
lines = [l.split()[:2] for l in open(sys.argv[1]) if re.match(r"\s+\[\d\]", l)]
phases = [int(p.strip("[]")) for p, _ in lines]
names = [n for _, n in lines]
assert phases == sorted(phases), "phases out of order"
packages = [i for i, n in enumerate(names) if ".pkg.tar.zst" in n]
databases = [i for i, n in enumerate(names) if "/ycorp." in n]
assert len(packages) == 8 and len(databases) == 16, (len(packages), len(databases))
assert max(packages) < min(databases)
assert sorted(names[-4:]) == ["aarch64/ycorp.db", "aarch64/ycorp.db.sig", "x86_64/ycorp.db", "x86_64/ycorp.db.sig"]
PY
grep -q "Dry run: nothing was uploaded." "$work/upload-plan.log" || fail "upload dry run"
pass "upload plan puts packages and signatures first and ycorp.db last"

printf '\nAll %d pacman repository checks passed.\n' "${#passed[@]}"
