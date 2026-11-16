# Linux updates

The Debian/Ubuntu package from a distribution profile (`official`) adds Email's
APT repository when it is installed, as the Chrome and VS Code packages do.
After that, `apt upgrade`, Software Updater and unattended-upgrades install
new releases like any other package. There is no in-app updater on Linux.

Only distribution builds do this (`DISTRIBUTION=true` and a
`LINUX_APT_REPOSITORY` in the profile, built with
`EMAIL_LINUX_RELEASE_CHANNEL=distribution`). Local alphas and packages built
with the `development` or neutral `unofficial` profile add no repository.
Arch Linux has its own pacman repository, `[ycorp]`, signed with the same key
([Arch Linux updates](arch-updates.md)).

## What the package installs

| File | Contents |
| --- | --- |
| `/usr/share/keyrings/email-linux-archive-keyring.asc` | The repository's public key, copied from `config/profiles/official.apt.asc`. |
| `/etc/apt/sources.list.d/email-linux.sources` | The repository, in deb822 form. |

The official 0.2.0-beta.1 package for amd64 would ship:

```
# Email updates (docs/linux-updates.md). Installed by the email-linux package;
# delete this file to stop updates from this repository.
Types: deb
URIs: https://updates.ycorplabs.com/email/apt
Suites: beta
Components: main
Architectures: amd64
Signed-By: /usr/share/keyrings/email-linux-archive-keyring.asc
```

`Signed-By` limits the key to this repository; it is never added to apt's
global trusted keys. `Architectures` is the package's own architecture, so a
machine with i386 multiarch does not ask for an i386 index.

Both files are dpkg conffiles. Deleting the sources file stops updates, and
later upgrades do not put it back. `apt purge email-linux` removes both. If a
user edits the sources file and a later package changes it, dpkg asks which
version to keep.

## Suites

| Suite | Carries | A package names it when |
| --- | --- | --- |
| `stable` | final releases only | its release version has no `-` (`0.2.0`) |
| `beta` | every release, finals included | it is a pre-release (`0.2.0-beta.1`, `0.2.0-rc.1`) |

A beta tester's machine follows `beta`, so it also receives the final release.
Installing a final package replaces the sources file (it was not edited, so
dpkg does this without asking), and from then on that machine follows `stable`.
To keep testing betas after a final, set `Suites: beta` in the sources file; at
the next release that changes the file, dpkg asks once whether to keep it.

Both suites list `amd64` and `arm64`. Every listed architecture has an index,
even an empty one.

## Repository layout

The repository is served from the `ycorp-updates` R2 bucket under `email/apt/`,
at `https://updates.ycorplabs.com/email/apt`:

```
email-linux-archive-keyring.asc                    public key, for manual setup
pool/main/e/email-linux/email-linux_<release>_<arch>.deb
dists/<suite>/InRelease                            signed Release (clearsigned)
dists/<suite>/Release, Release.gpg                 Release and detached signature
dists/<suite>/main/binary-<arch>/Packages(.gz)     package index
dists/<suite>/main/binary-<arch>/by-hash/SHA256/<sha256>
```

`Release` sets `Acquire-By-Hash: yes`: apt fetches each index by its SHA-256,
so a cached or half-uploaded index can never mismatch the `InRelease` that
names it. Old by-hash files and every published package stay in the
repository. A package keeps its release-version file name
(`email-linux_0.2.0-beta.1_amd64.deb`); its `Version` is the Debian form
(`1:0.2.0~beta.1`, [release versions](release-versions.md)).

## Signing key

| | |
| --- | --- |
| Algorithm | ed25519 |
| Fingerprint | `20B0 87BF 1D1B 2A04 1028  82F2 1308 1979 B6CE ABDA` |
| User ID | Y Corp AS Email packages <support@ycorplabs.com> |
| Expires | 2031-11-14 |

The same key signs the Arch repository ([Arch Linux updates](arch-updates.md)).
The profile records the fingerprint (`LINUX_APT_KEY_FINGERPRINT`) and the
public key sits beside it (`config/profiles/official.apt.asc`). The packager
refuses a key file without that fingerprint or with any secret-key packet, and
`scripts/check-release-readiness.py` checks the same for every tracked profile.
The private key lives in the release secret store, outside this repository.
Signing reads it from a GnuPG home named by `EMAIL_APT_GNUPGHOME`.

### Expiry and rotation

apt refuses signatures from an expired key, so act a year before 2031-11-14:

- **Extend:** `gpg --quick-set-expire <fingerprint> 5y`, export the public key
  again into `config/profiles/official.apt.asc` and ship it in a release. The
  fingerprint stays the same. Machines get the new key file with that package
  (an unedited conffile is replaced without a prompt), so ship it well before
  the old expiry.
- **Rotate:** add the new public key to `official.apt.asc` next to the old one
  and ship a release. Once that release is widely installed, set
  `LINUX_APT_KEY_FINGERPRINT` to the new key and sign with it. Drop the old key
  in a later release.

A machine that skipped every release in between has to fetch the new key with
the manual setup below. Arch machines get the key only from the setup in
[Arch Linux updates](arch-updates.md#signing-key), so tell Arch users to fetch
it again after an extension or a rotation.

## Publishing

```sh
export EMAIL_DISTRIBUTION_PROFILE=official
export EMAIL_APT_GNUPGHOME=/path/to/release/gnupg-home   # outside this checkout
# Optional, for signing without a pinentry prompt:
# export EMAIL_APT_PASSPHRASE_FILE=/path/to/passphrase
./scripts/publish-linux-apt.sh --staging ../email-updates/apt \
  dist-linux/email-linux_0.2.0-beta.1_amd64.deb dist-linux/email-linux_0.2.0-beta.1_arm64.deb
./scripts/upload-updates.sh --dry-run --source ../email-updates/apt --prefix email/apt
./scripts/upload-updates.sh --source ../email-updates/apt --prefix email/apt
```

`publish-linux-apt.sh` stages; it never uploads. It reads each package with
`dpkg-deb` in an `ubuntu:24.04` container without network access, and refuses
a package unless:

- it is `email-linux` for amd64 or arm64, with an Email version
  (`1:<version>`) that matches its file name;
- it is a distribution build (`RELEASE-CHANNEL.txt`), with the profile's
  desktop entry;
- its sources file names the profile's repository, the suite for its version
  and its architecture, and its keyring is byte for byte the profile's key,
  both as conffiles;
- its version is newer than everything that suite already has for that
  architecture. A final goes to `stable` and to `beta`; when `beta` already has
  something newer, the final goes to `stable` only.

It also refuses a GnuPG home without the profile's secret key. It copies the
staging directory, adds the packages (pool files are hard-linked and never
change), rewrites the indexes, signs `InRelease` and `Release.gpg`, and checks
the result as a client would: every index and package against `Release`, and
both signatures of every suite with `gpgv` against the committed public key
only. Only then does it replace the staging directory. A lock beside the
staging directory serializes publishers.

Keep the staging directory between releases: the indexes are rebuilt from it.
The upload script refuses a staging directory whose indexes lack a published
package, so a stale or empty one cannot drop releases. To recreate it, download
`email-linux-archive-keyring.asc`, each suite's `InRelease`, `Release`,
`Release.gpg` and `main/binary-{amd64,arm64}/Packages{,.gz}`, and every
`Filename` those indexes list, into the same paths.

### Uploading

`scripts/upload-updates.sh` copies a staged directory (this repository, the
[Arch repository](arch-updates.md#uploading) or the macOS feed) to the bucket
with Wrangler, pinned exactly (`npx wrangler@4.155.0 r2 object put --remote`).
Wrangler takes credentials from its own login or `CLOUDFLARE_API_TOKEN`.
Before uploading it verifies the directory again,
with the profile's public keys, and compares it with what is published at
`https://updates.ycorplabs.com/<prefix>`. Then it uploads in dependency order:

1. packages and by-hash indexes, cached for a year (`immutable`);
2. `Packages`, `Packages.gz` and the public key;
3. `Release` and `Release.gpg`;
4. `InRelease`.

Everything except versioned files is cached for five minutes. `--dry-run`
prints the plan and uploads nothing. Versioned files already uploaded are
recorded in the staging directory's `.upload-manifest.json` and skipped; the
script refuses one whose bytes changed.

## Existing beta.1 and beta.2 installs

v0.1.0-beta.1 and beta.2 were installed from `.deb` files that add no
repository. Each machine has to do one of these once:

- Install the next release's `.deb` by hand. It adds the repository, and later
  releases arrive through apt.
- Or add the repository now:

  ```sh
  sudo curl -fsSLo /usr/share/keyrings/email-linux-archive-keyring.asc \
    https://updates.ycorplabs.com/email/apt/email-linux-archive-keyring.asc
  gpg --show-keys /usr/share/keyrings/email-linux-archive-keyring.asc
  # Check the fingerprint: 20B0 87BF 1D1B 2A04 1028  82F2 1308 1979 B6CE ABDA
  arch="$(dpkg --print-architecture)"
  sudo tee /etc/apt/sources.list.d/email-linux.sources >/dev/null <<EOF
  # Email updates (docs/linux-updates.md). Installed by the email-linux package;
  # delete this file to stop updates from this repository.
  Types: deb
  URIs: https://updates.ycorplabs.com/email/apt
  Suites: beta
  Components: main
  Architectures: $arch
  Signed-By: /usr/share/keyrings/email-linux-archive-keyring.asc
  EOF
  sudo apt update && sudo apt upgrade
  ```

  The files are exactly the ones the package ships, so the next package takes
  them over without a conffile question while the suites match. Use
  `Suites: stable` to follow final releases only.

## Tests

```sh
python3 scripts/apt-repository.test.py   # versions, keyrings, package checks, indexes, gpgv
python3 scripts/upload-updates.test.py   # upload order, caching, manifest, published-history check
./scripts/test-linux-apt.sh              # end to end in containers, throwaway key
```

`test-linux-apt.sh` generates a throwaway ed25519 key (never the production
key) and a fixture profile with the repository
`https://apt.email-test.invalid/email/apt`. It builds 9.0.0-beta.1,
9.0.0-beta.2 and 9.0.0 with the real packager in the Linux build container,
publishes them with `publish-linux-apt.sh`, and serves the staged repository
over HTTPS (a throwaway CA) from a container on an internal Docker network. In
an Ubuntu 24.04 container it checks that:

- the publisher refuses an older or repeated version, packages from another
  channel or repository, and a signing key other than the profile's, and
  leaves the staging directory unchanged;
- installing beta.1 from its file adds the sources file and key as conffiles,
  and `apt-get update` verifies the repository without warnings;
- a tampered `InRelease`, a tampered `Release` (with `InRelease` removed), tampered
  `Packages` indexes and a repository signed by another key are each rejected
  and install nothing;
- `apt-get upgrade` moves beta.1 to beta.2, fetching indexes by hash, and then
  to the final through the beta suite, after which the machine follows
  `stable`, with no conffile prompt;
- purging removes the sources file and key;
- the sources file in the manual setup above (read from this document) is
  byte for byte the one the package ships, and after writing it and the key by
  hand, installing the package from the repository asks no conffile question;
- the upload plan puts packages and by-hash indexes first and `InRelease` last.

Not covered: the production key and bucket (nothing is uploaded), the `curl`
download in the manual setup (the test fetches the same file with Python),
Software Updater and unattended-upgrades, and packages for the architecture
the test machine is not.
