#!/usr/bin/env bash
# End-to-end test of Linux updates through APT (docs/linux-updates.md), with a
# throwaway signing key. It never touches the production key, the update
# bucket or any host besides the containers it starts.
#
#   1. Generates a throwaway ed25519 GnuPG key and a fixture distribution
#      profile: the official profile with that key and the repository
#      https://apt.email-test.invalid/email/apt.
#   2. Builds three real .debs with scripts/package-linux-deb.sh in the Linux
#      build container: 9.0.0-beta.1, 9.0.0-beta.2 and the final 9.0.0.
#   3. Publishes them with scripts/publish-linux-apt.sh, and checks that the
#      publisher refuses an older version, packages from another channel or
#      repository, and a signing key other than the profile's.
#   4. Serves the staged repository over HTTPS (a throwaway CA) from a
#      container on an internal Docker network, under the repository's host
#      name, and in an Ubuntu 24.04 container:
#      - installs 9.0.0-beta.1 from its .deb file and checks that the sources
#        file and key land as conffiles and that apt-get update verifies;
#      - checks that a tampered InRelease, a tampered Release, tampered
#        Packages indexes and a repository signed by another key are all
#        rejected and install nothing;
#      - upgrades to 9.0.0-beta.2 with apt-get upgrade, then to 9.0.0, which
#        moves the machine to the stable suite without a conffile prompt;
#      - purges the package and checks that the sources file and key go too;
#      - repeats the manual repository setup from docs/linux-updates.md and
#        installs the package from the repository without a conffile question.
#   5. Checks the upload plan for the staged repository (dry run, no network).
#
# Usage: scripts/test-linux-apt.sh [--keep]
#   --keep  keep the work directory, containers and network for inspection
#
# Needs Docker (or EMAIL_CONTAINER_ENGINE=podman), GnuPG, OpenSSL and Python 3.
# The first run compiles the app in the build container and downloads the
# app's runtime dependencies into a test image; later runs reuse both.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
engine="${EMAIL_CONTAINER_ENGINE:-docker}"
keep=0
case "${1:-}" in
  "") ;;
  --keep) keep=1 ;;
  *) echo "Usage: scripts/test-linux-apt.sh [--keep]" >&2; exit 2 ;;
esac
for tool in "$engine" gpg gpgconf openssl python3 git; do
  command -v "$tool" >/dev/null || { echo "$tool is required." >&2; exit 1; }
done

host=apt.email-test.invalid
versions=(9.0.0-beta.1 9.0.0-beta.2 9.0.0)
work="$(mktemp -d "${TMPDIR:-/tmp}/email-apt-test.XXXXXX")"
# GnuPG puts its agent socket in the home directory; keep that path short.
gnupg="$(mktemp -d /tmp/email-apt-gpg.XXXXXX)"
other_gnupg="$(mktemp -d /tmp/email-apt-gpg.XXXXXX)"
run_id="email-apt-test-$$"
network="$run_id" server="$run_id-server" client="$run_id-client"
cleanup() {
  local result=$?
  gpgconf --homedir "$gnupg" --kill gpg-agent 2>/dev/null || true
  gpgconf --homedir "$other_gnupg" --kill gpg-agent 2>/dev/null || true
  rm -rf "$gnupg" "$other_gnupg"
  if [[ "$keep" == 1 ]]; then
    echo "Kept $work, containers $server and $client, and network $network." >&2
  else
    "$engine" rm -f "$server" "$client" >/dev/null 2>&1 || true
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

# 1. Throwaway signing keys and the fixture profile.
chmod 700 "$gnupg" "$other_gnupg"
for home in "$gnupg" "$other_gnupg"; do
  gpg --homedir "$home" --batch --quiet --passphrase '' \
    --quick-generate-key "Email APT test, throwaway <apt-test@email-test.invalid>" ed25519 sign 1d 2>/dev/null
done
fingerprint="$(gpg --homedir "$gnupg" --batch --with-colons --list-secret-keys 2>/dev/null | awk -F: '/^fpr:/ { print $10; exit }')"
[[ "$fingerprint" =~ ^[0-9A-F]{40}$ ]] || fail "throwaway key generation"
[[ "$fingerprint" != 20B087BF1D1B2A04102882F213081979B6CEABDA ]] || fail "the test must never use the production key"
mkdir -p "$work/profile" "$work/debs" "$work/repo-parent"
profile="$work/profile/aptfixture.env"
sed -e 's/^PROFILE=.*/PROFILE=aptfixture/' \
    -e 's/^UPDATE_FEED_URL=.*/UPDATE_FEED_URL=/' -e 's/^UPDATE_PUBLIC_ED_KEY=.*/UPDATE_PUBLIC_ED_KEY=/' \
    -e "s/^UPDATE_ALLOWED_HOSTS=.*/UPDATE_ALLOWED_HOSTS=$host/" \
    -e "s#^LINUX_APT_REPOSITORY=.*#LINUX_APT_REPOSITORY=https://$host/email/apt#" \
    -e "s/^LINUX_APT_KEY_FINGERPRINT=.*/LINUX_APT_KEY_FINGERPRINT=$fingerprint/" \
    "$repo_root/config/profiles/official.env" > "$profile"
gpg --homedir "$gnupg" --batch --armor --export "$fingerprint" > "$work/profile/aptfixture.apt.asc"
EMAIL_DISTRIBUTION_PROFILE="$profile" python3 "$repo_root/scripts/distribution-profile.py" get LINUX_APT_REPOSITORY >/dev/null \
  || fail "fixture profile"
pass "throwaway signing key $fingerprint and fixture profile"

# 2. Real packages from the packager, built in the Linux build container.
rust_version="$(python3 "$repo_root/scripts/check-toolchain.py" --print-version)"
build_image="email-linux-dev:$rust_version"
"$repo_root/scripts/build-linux-container.sh" --image-only >"$work/image.log" 2>&1 || { tail -20 "$work/image.log" >&2; fail "build image"; }
# shellcheck disable=SC2016 # Expanded inside the container.
build_script='
set -euo pipefail
mkdir -p /work
tar -xmf - -C /work
cd /work
exec 9>/target/.email-apt-test.lock
flock 9
for version in $EMAIL_TEST_VERSIONS; do
  # The same sources at another release version; Cargo.lock must agree for --locked.
  python3 - "$version" <<"PY"
import re, sys
from pathlib import Path
version = sys.argv[1]
toml = Path("apps/linux/Cargo.toml")
toml.write_text(re.sub(r"(?m)^version = \"[^\"]*\"", f"version = \"{version}\"", toml.read_text(), count=1))
lock = Path("apps/linux/Cargo.lock")
lock.write_text(re.sub(r"(name = \"email-linux\"\nversion = \")[^\"]*", lambda m: m.group(1) + version, lock.read_text(), count=1))
PY
  CARGO_TARGET_DIR=/target/linux EMAIL_MCP_TARGET_DIR=/target/workspace EMAIL_LINUX_PACKAGE_DIR=/out \
    scripts/package-linux-deb.sh
done
'
git -C "$repo_root" ls-files -z --cached --others --exclude-standard \
  | (cd "$repo_root" && while IFS= read -r -d '' name; do if [[ -e "$name" ]]; then printf '%s\0' "$name"; fi; done) \
  | (cd "$repo_root" && COPYFILE_DISABLE=1 tar --null --no-xattrs -T - -cf -) \
  | "$engine" run --rm -i \
      --mount "type=volume,source=email-linux-cargo,target=/cargo" \
      --mount "type=volume,source=email-linux-apt-test-target,target=/target" \
      --mount "type=bind,source=$work/profile,target=$work/profile,readonly" \
      --mount "type=bind,source=$work/debs,target=/out" \
      -e CARGO_HOME=/cargo -e EMAIL_DISTRIBUTION_PROFILE="$profile" -e EMAIL_LINUX_RELEASE_CHANNEL=distribution \
      -e EMAIL_RELEASE_GOOGLE_CLIENT_ID=1-apt-test-fixture.apps.googleusercontent.com \
      -e EMAIL_RELEASE_GOOGLE_CLIENT_SECRET=apt-test-fixture \
      -e EMAIL_TEST_VERSIONS="${versions[*]}" \
      "$build_image" bash -c "$build_script" >"$work/build.log" 2>&1 \
  || { tail -40 "$work/build.log" >&2; fail "building packages"; }
arch="$("$engine" run --rm "$build_image" dpkg --print-architecture)"
deb() { printf '%s/debs/email-linux_%s_%s.deb\n' "$work" "$1" "$arch"; }
for version in "${versions[@]}"; do [[ -f "$(deb "$version")" ]] || fail "package $version was not built"; done
pass "built email-linux ${versions[*]} for $arch with the real packager"

# The tools image reads packages; the test image serves and installs them.
tools_image=ubuntu:24.04
"$engine" image inspect "$tools_image" >/dev/null 2>&1 || "$engine" pull -q "$tools_image" >/dev/null
# shellcheck disable=SC2016
"$engine" run --rm --network none --mount "type=bind,source=$work/debs,target=/debs" "$tools_image" bash -c '
set -euo pipefail
for deb in /debs/*.deb; do
  name="$(basename "$deb" .deb)"
  dpkg-deb -c "$deb" > "/debs/$name.contents"
  dpkg-deb -f "$deb" Depends > "/debs/$name.depends"
  dpkg-deb --ctrl-tarfile "$deb" | tar -xO ./conffiles > "/debs/$name.conffiles"
  dpkg-deb --fsys-tarfile "$deb" | tar -xO ./etc/apt/sources.list.d/email-linux.sources > "/debs/$name.sources"
done'
for version in "${versions[@]}"; do
  base="$work/debs/email-linux_${version}_$arch"
  grep -q ' ./etc/apt/sources.list.d/email-linux.sources$' "$base.contents" || fail "$version has no sources file"
  grep -q ' ./usr/share/keyrings/email-linux-archive-keyring.asc$' "$base.contents" || fail "$version has no keyring"
  grep -qx /etc/apt/sources.list.d/email-linux.sources "$base.conffiles" || fail "$version sources file is not a conffile"
  grep -qx /usr/share/keyrings/email-linux-archive-keyring.asc "$base.conffiles" || fail "$version keyring is not a conffile"
  suite=stable; [[ "$version" == *-* ]] && suite=beta
  grep -qx "Suites: $suite" "$base.sources" || fail "$version names the wrong suite"
done
pass "packages ship the sources file and key as conffiles; betas name beta, the final names stable"

# 3. Publishing, and what the publisher refuses.
repo="$work/repo-parent/apt"
publish() {
  EMAIL_DISTRIBUTION_PROFILE="$profile" EMAIL_APT_GNUPGHOME="${publish_gnupg:-$gnupg}" \
    "$repo_root/scripts/publish-linux-apt.sh" --staging "$repo" "$@"
}
snapshot() { rm -rf "$work/snap-$1"; cp -R "$repo" "$work/snap-$1"; }
publish "$(deb 9.0.0-beta.1)" >"$work/publish-1.log" 2>&1 || { cat "$work/publish-1.log" >&2; fail "publishing beta.1"; }
snapshot 1
before="$(tree_hash "$repo")"
# Foreign packages: the same version repacked from another channel or with
# another repository in its sources file.
# shellcheck disable=SC2016
"$engine" run --rm --network none --mount "type=bind,source=$work,target=/work" -e ARCH="$arch" "$tools_image" bash -c '
set -euo pipefail
name="email-linux_9.0.0-beta.1_$ARCH.deb"
for kind in channel repository; do
  root="/tmp/$kind"
  dpkg-deb -R "/work/debs/$name" "$root"
  if [[ $kind == channel ]]; then
    echo local-alpha > "$root/usr/share/doc/email-linux/RELEASE-CHANNEL.txt"
  else
    sed -i "s#^URIs: .*#URIs: https://other.email-test.invalid/apt#" "$root/etc/apt/sources.list.d/email-linux.sources"
  fi
  mkdir -p "/work/foreign/$kind"
  dpkg-deb --root-owner-group -b "$root" "/work/foreign/$kind/$name" >/dev/null
done'
for kind in channel repository; do
  if publish "$work/foreign/$kind/email-linux_9.0.0-beta.1_$arch.deb" >"$work/publish-foreign-$kind.log" 2>&1; then
    fail "publisher accepted a package with another $kind"
  fi
done
if publish_gnupg="$other_gnupg" publish "$(deb 9.0.0-beta.2)" >"$work/publish-other-key.log" 2>&1; then
  fail "publisher signed with a key other than the profile's"
fi
grep -q "no secret key with fingerprint $fingerprint" "$work/publish-other-key.log" || { cat "$work/publish-other-key.log" >&2; fail "other key message"; }
[[ "$(tree_hash "$repo")" == "$before" && ! -e "$repo.publish-lock" ]] || fail "a refused publication changed the staging directory"
pass "publisher refuses another channel, another repository and another signing key, and changes nothing"
publish "$(deb 9.0.0-beta.2)" >"$work/publish-2.log" 2>&1 || { cat "$work/publish-2.log" >&2; fail "publishing beta.2"; }
snapshot 2
if publish "$(deb 9.0.0-beta.1)" >"$work/publish-old.log" 2>&1; then fail "publisher accepted an older version"; fi
grep -q "is not newer than" "$work/publish-old.log" || { cat "$work/publish-old.log" >&2; fail "older version message"; }
publish "$(deb 9.0.0)" >"$work/publish-3.log" 2>&1 || { cat "$work/publish-3.log" >&2; fail "publishing the final"; }
snapshot 3
if publish "$(deb 9.0.0)" >"$work/publish-again.log" 2>&1; then fail "publisher accepted the same final twice"; fi
python3 -c '
import importlib.util, sys
spec = importlib.util.spec_from_file_location("apt", sys.argv[1])
apt = importlib.util.module_from_spec(spec); spec.loader.exec_module(apt)
beta = [e["Version"] for e in apt.read_index(sys.argv[2], "beta", sys.argv[3])]
stable = [e["Version"] for e in apt.read_index(sys.argv[2], "stable", sys.argv[3])]
assert beta == ["1:9.0.0~beta.1", "1:9.0.0~beta.2", "1:9.0.0"], beta
assert stable == ["1:9.0.0"], stable
' "$repo_root/scripts/apt-repository.py" "$repo" "$arch" || fail "suite contents"
[[ "$(ls "$repo/pool/main/e/email-linux" | wc -l | tr -d ' ')" == 3 ]] || fail "pool keeps every package"
pass "publisher refuses older or repeated versions; beta has all three, stable only the final; pool keeps all"

# 4. A client against the served repository.
mkdir -p "$work/tls" "$work/served"
openssl req -x509 -newkey rsa:2048 -nodes -days 1 -subj "/CN=Email APT test CA (throwaway)" \
  -keyout "$work/tls/ca.key" -out "$work/tls/ca.crt" \
  -addext "basicConstraints=critical,CA:true" -addext "keyUsage=critical,keyCertSign" 2>/dev/null
openssl req -newkey rsa:2048 -nodes -subj "/CN=$host" -keyout "$work/tls/server.key" -out "$work/tls/server.csr" 2>/dev/null
printf 'subjectAltName=DNS:%s\nextendedKeyUsage=serverAuth\n' "$host" > "$work/tls/server.ext"
openssl x509 -req -in "$work/tls/server.csr" -CA "$work/tls/ca.crt" -CAkey "$work/tls/ca.key" -CAcreateserial \
  -days 1 -extfile "$work/tls/server.ext" -out "$work/tls/server.crt" 2>/dev/null
chmod 644 "$work/tls/server.key"
# Runtime dependencies are installed once into a cached image, so the client
# needs no network beyond the internal one.
depends="$(python3 -c '
import re, sys
names = []
for group in open(sys.argv[1]).read().split(","):
    name = re.sub(r"\s*\(.*\)", "", group.split("|")[0]).strip()
    if name and name not in names:
        names.append(name)
print(" ".join(sorted(names)))
' "$work/debs/email-linux_${versions[0]}_$arch.depends")"
test_image="email-apt-test:$(printf '%s %s' "$tools_image" "$depends" | shasum -a 256 | cut -c1-12)"
if ! "$engine" image inspect "$test_image" >/dev/null 2>&1; then
  mkdir -p "$work/image"
  printf 'FROM %s\nRUN apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends python3 ca-certificates %s >/dev/null && rm -rf /var/lib/apt/lists/*\n' \
    "$tools_image" "$depends" > "$work/image/Dockerfile"
  "$engine" build -q -t "$test_image" "$work/image" >"$work/test-image.log" 2>&1 || { tail -20 "$work/test-image.log" >&2; fail "test image"; }
fi
"$engine" network create --internal "$network" >/dev/null
# shellcheck disable=SC2016
"$engine" run -d --name "$server" --network "$network" --network-alias "$host" \
  --mount "type=bind,source=$work/tls,target=/tls,readonly" "$test_image" python3 -c '
import functools, http.server, ssl
context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
context.load_cert_chain("/tls/server.crt", "/tls/server.key")
handler = functools.partial(http.server.SimpleHTTPRequestHandler, directory="/srv")
server = http.server.ThreadingHTTPServer(("", 443), handler)
server.socket = context.wrap_socket(server.socket, server_side=True)
server.serve_forever()' >/dev/null
# The served files are copied into the server container, never bind-mounted:
# a bind mount can show stale files right after they are replaced.
stage() { rm -rf "$work/served/apt"; cp -R "$1" "$work/served/apt"; }
publish_served() {
  "$engine" exec "$server" rm -rf /srv/email/apt
  "$engine" exec "$server" mkdir -p /srv/email
  "$engine" cp "$work/served/apt" "$server:/srv/email/apt" >/dev/null
}
serve() { stage "$1"; publish_served; } # served as https://$host/email/apt
serve "$work/snap-1"
"$engine" run -d --name "$client" --network "$network" \
  --mount "type=bind,source=$work/debs,target=/debs,readonly" \
  --mount "type=bind,source=$work/tls/ca.crt,target=/usr/local/share/ca-certificates/email-apt-test.crt,readonly" \
  -e DEBIAN_FRONTEND=noninteractive "$test_image" sleep infinity >/dev/null
client() { "$engine" exec -i "$client" bash -c "set -euo pipefail; $1" </dev/null; }
# Only the test repository: Ubuntu's own sources are unreachable on purpose.
client "rm -f /etc/apt/sources.list.d/ubuntu.sources /etc/apt/sources.list; update-ca-certificates >/dev/null 2>&1"
client "getent hosts $host" >/dev/null || fail "the client cannot resolve $host on the test network"
installed() { client "dpkg-query -W -f='\${Version}' email-linux"; }
update_ok() { # label
  client "apt-get update 2>&1" >"$work/update-$1.log" || { cat "$work/update-$1.log" >&2; fail "apt-get update ($1)"; }
  if grep -E '^(W|E|Err):' "$work/update-$1.log" >/dev/null; then cat "$work/update-$1.log" >&2; fail "apt-get update warned ($1)"; fi
}
update_rejected() { # label pattern
  if client "apt-get update 2>&1" >"$work/update-$1.log"; then
    grep -E '^(E|Err):' "$work/update-$1.log" >/dev/null || { cat "$work/update-$1.log" >&2; fail "$1 was accepted"; }
  fi
  grep -Eq "$2" "$work/update-$1.log" || { cat "$work/update-$1.log" >&2; fail "$1: expected /$2/"; }
  client "apt-get -y upgrade 2>&1" >"$work/upgrade-$1.log" || true
  [[ "$(installed)" == "1:9.0.0~beta.1" ]] || fail "$1 installed $(installed)"
}

client "apt-get install -y /debs/email-linux_9.0.0-beta.1_$arch.deb 2>&1" >"$work/install.log" \
  || { cat "$work/install.log" >&2; fail "installing beta.1"; }
[[ "$(installed)" == "1:9.0.0~beta.1" ]] || fail "beta.1 installed"
client "cat /etc/apt/sources.list.d/email-linux.sources" > "$work/client-sources"
grep -qx "URIs: https://$host/email/apt" "$work/client-sources" && grep -qx "Suites: beta" "$work/client-sources" \
  && grep -qx "Signed-By: /usr/share/keyrings/email-linux-archive-keyring.asc" "$work/client-sources" \
  && grep -qx "Architectures: $arch" "$work/client-sources" || { cat "$work/client-sources" >&2; fail "installed sources file"; }
"$engine" exec -i "$client" cmp /usr/share/keyrings/email-linux-archive-keyring.asc /dev/stdin < "$work/profile/aptfixture.apt.asc" \
  || fail "installed keyring differs from the profile's public key"
client "dpkg-query -W -f='\${Conffiles}' email-linux" > "$work/conffiles"
grep -q "/etc/apt/sources.list.d/email-linux.sources" "$work/conffiles" && grep -q "/usr/share/keyrings/email-linux-archive-keyring.asc" "$work/conffiles" \
  || fail "dpkg does not record the sources file and key as conffiles"
update_ok beta1
client "apt-cache policy email-linux" > "$work/policy-1.log"
grep -q "https://$host/email/apt beta/main $arch Packages" "$work/policy-1.log" || { cat "$work/policy-1.log" >&2; fail "repository not in policy"; }
grep -q "Candidate: 1:9.0.0~beta.1" "$work/policy-1.log" || { cat "$work/policy-1.log" >&2; fail "candidate with beta.1 published"; }
pass "installed beta.1: sources file and key are conffiles; apt-get update verifies the signed repository"

# Tampering, with a newer version on offer each time: nothing may install.
tamper() { # label, then a Python snippet run with root = served repository
  stage "$work/snap-2"
  python3 -c "import re, sys; from pathlib import Path; root = Path(sys.argv[1]); $2" "$work/served/apt"
  publish_served
}
tamper inrelease 'p = root / "dists/beta/InRelease"; t = p.read_text(); p.write_text(t.replace("Suite: beta", "Suite: betb", 1))'
update_rejected inrelease 'BADSIG|signatures were invalid|not signed'
pass "tampered InRelease rejected; beta.1 stays installed"
tamper release '(root / "dists/beta/InRelease").unlink(); p = root / "dists/beta/Release"; p.write_text(p.read_text().replace("Suite: beta", "Suite: betb", 1))'
update_rejected release 'BADSIG|signatures were invalid|not signed'
pass "tampered Release (InRelease removed, Release.gpg kept) rejected; beta.1 stays installed"
tamper packages '
for name in ("Packages", "Packages.gz"):
    for path in [root / "dists/beta/main" / f"binary-{a}" / name for a in ("amd64", "arm64")]:
        data = path.read_bytes()
        for copy in [path, *path.parent.glob("by-hash/SHA256/*")]:
            if copy.read_bytes() == data:
                copy.write_bytes(data + b"\n")'
update_rejected packages 'Hash Sum mismatch|unexpected size|Mirror sync in progress'
pass "tampered Packages indexes rejected; beta.1 stays installed"
stage "$work/snap-2"
for suite_dir in "$work/served/apt/dists"/*; do
  gpg --homedir "$other_gnupg" --batch --yes --digest-algo SHA512 --clearsign -o "$suite_dir/InRelease" "$suite_dir/Release" 2>/dev/null
  gpg --homedir "$other_gnupg" --batch --yes --digest-algo SHA512 --armor --detach-sign -o "$suite_dir/Release.gpg" "$suite_dir/Release" 2>/dev/null
done
publish_served
update_rejected other-key 'NO_PUBKEY|not signed|signatures couldn.t be verified'
pass "repository signed by another key rejected; beta.1 stays installed"

# Genuine updates.
serve "$work/snap-2"
update_ok beta2
client "apt-get -y upgrade 2>&1" >"$work/upgrade-beta2.log" || { cat "$work/upgrade-beta2.log" >&2; fail "upgrade to beta.2"; }
[[ "$(installed)" == "1:9.0.0~beta.2" ]] || { cat "$work/upgrade-beta2.log" >&2; fail "beta.2 not installed: $(installed)"; }
client "grep -qx 'Suites: beta' /etc/apt/sources.list.d/email-linux.sources" || fail "beta.2 sources file"
"$engine" logs "$server" >"$work/server.log" 2>&1
grep -q "GET /email/apt/dists/beta/main/binary-$arch/by-hash/SHA256/" "$work/server.log" \
  || fail "apt did not fetch indexes by hash"
pass "apt-get upgrade moved beta.1 to beta.2 (indexes fetched by hash)"
serve "$work/snap-3"
update_ok final
client "apt-get -y upgrade 2>&1" >"$work/upgrade-final.log" || { cat "$work/upgrade-final.log" >&2; fail "upgrade to the final"; }
[[ "$(installed)" == "1:9.0.0" ]] || { cat "$work/upgrade-final.log" >&2; fail "final not installed: $(installed)"; }
client "grep -qx 'Suites: stable' /etc/apt/sources.list.d/email-linux.sources" || fail "the final did not move the sources file to stable"
if grep -qi "conffile\|configuration file" "$work/upgrade-final.log"; then cat "$work/upgrade-final.log" >&2; fail "conffile prompt"; fi
update_ok stable
client "apt-cache policy email-linux" > "$work/policy-final.log"
grep -q "https://$host/email/apt stable/main $arch Packages" "$work/policy-final.log" || { cat "$work/policy-final.log" >&2; fail "stable suite in policy"; }
pass "apt-get upgrade moved beta.2 to the final through the beta suite; the machine now follows stable"
client "apt-get -y purge email-linux 2>&1" >"$work/purge.log" || { cat "$work/purge.log" >&2; fail "purge"; }
client "test ! -e /etc/apt/sources.list.d/email-linux.sources && test ! -e /usr/share/keyrings/email-linux-archive-keyring.asc" \
  || fail "purge left the sources file or key"
pass "purging the package removes its sources file and key"

# Manual setup for installs that predate the repository (docs/linux-updates.md):
# the documented files, then the package from the repository, with no
# conffile question because the files are identical.
python3 - "$repo_root/docs/linux-updates.md" "$host" "$arch" > "$work/manual.sources" <<'PY' || fail "manual setup snippet in docs/linux-updates.md"
import re, sys, textwrap
text = open(sys.argv[1]).read()
block = re.search(r"sudo tee /etc/apt/sources.list.d/email-linux.sources >/dev/null <<EOF\n(.*?)\n *EOF\n", text, re.S).group(1)
block = textwrap.dedent(block).replace("$arch", sys.argv[3]).replace("updates.ycorplabs.com", sys.argv[2])
sys.stdout.write(block.replace("Suites: beta", "Suites: stable") + "\n")
PY
python3 "$repo_root/scripts/apt-repository.py" sources "https://$host/email/apt" 9.0.0 "$arch" | cmp -s - "$work/manual.sources" \
  || fail "the documented sources file differs from the one the package ships"
"$engine" exec -i "$client" python3 - "$host" <<'PY' || fail "downloading the published key"
import sys, urllib.request
url = f"https://{sys.argv[1]}/email/apt/email-linux-archive-keyring.asc"
open("/usr/share/keyrings/email-linux-archive-keyring.asc", "wb").write(urllib.request.urlopen(url).read())
PY
"$engine" exec -i "$client" tee /etc/apt/sources.list.d/email-linux.sources < "$work/manual.sources" >/dev/null
update_ok manual
client "apt-get install -y email-linux 2>&1" >"$work/install-manual.log" || { cat "$work/install-manual.log" >&2; fail "installing after manual setup"; }
[[ "$(installed)" == "1:9.0.0" ]] || fail "manual setup installed $(installed)"
client "dpkg-query -S /etc/apt/sources.list.d/email-linux.sources /usr/share/keyrings/email-linux-archive-keyring.asc" >/dev/null \
  || fail "the package did not take over the manually written files"
pass "manual setup from the docs, then the package takes over the files without a conffile question"

# 5. Upload plan for the staged repository (no network, nothing uploaded).
EMAIL_DISTRIBUTION_PROFILE="$profile" "$repo_root/scripts/upload-updates.sh" --dry-run --skip-remote-check \
  --source "$repo" --prefix email/apt >"$work/upload-plan.log" 2>&1 || { cat "$work/upload-plan.log" >&2; fail "upload plan"; }
python3 - "$work/upload-plan.log" <<'PY' || { cat "$work/upload-plan.log" >&2; fail "upload order"; }
import re, sys
lines = [l.split()[:2] for l in open(sys.argv[1]) if re.match(r"\s+\[\d\]", l)]
phases = [int(p.strip("[]")) for p, _ in lines]
names = [n for _, n in lines]
assert phases == sorted(phases), "phases out of order"
last_pool = max(i for i, n in enumerate(names) if n.startswith("pool/") or "/by-hash/" in n)
first_index = min(i for i, n in enumerate(names) if n.endswith(("/Packages", "/Packages.gz")))
first_release = min(i for i, n in enumerate(names) if n.endswith(("/Release", "/Release.gpg")))
first_inrelease = min(i for i, n in enumerate(names) if n.endswith("/InRelease"))
assert last_pool < first_index < first_release < first_inrelease
assert names[-1].endswith("/InRelease")
assert sum(n.startswith("pool/") for n in names) == 3
PY
grep -q "Dry run: nothing was uploaded." "$work/upload-plan.log" || fail "upload dry run"
pass "upload plan puts packages and by-hash indexes first and InRelease last"

printf '\nAll %d APT update checks passed.\n' "${#passed[@]}"
