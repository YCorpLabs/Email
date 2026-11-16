# Arch Linux updates

Arch Linux users install and update Email from `[ycorp]`, a signed pacman
repository served with Email's other update files. It carries
`email-ycorp-bin`: the release's prebuilt app, repackaged from the official
`.deb` by the [AUR binary package's](linux-aur.md#email-ycorp-bin-prebuilt)
PKGBUILD. After a one-time setup, `pacman -Syu` installs new releases with
the rest of the system.

The AUR is closed to new accounts, with no date for reopening, so this is
how Arch users get Email for now. The AUR packages are still the plan once
an account can be made; the repository can stay as it is next to them.

## Setup

Once per machine (also on [ycorplabs.com/email/download](https://ycorplabs.com/email/download/#arch)):

1. Trust the key Email's packages are signed with:

   ```sh
   curl -fsSLO https://updates.ycorplabs.com/email/arch/ycorp.asc
   sudo pacman-key --add ycorp.asc
   sudo pacman-key --lsign-key 20B087BF1D1B2A04102882F213081979B6CEABDA
   ```

2. Add these lines to the end of `/etc/pacman.conf`:

   ```ini
   [ycorp]
   SigLevel = Required
   Server = https://updates.ycorplabs.com/email/arch/$arch
   ```

3. Install Email:

   ```sh
   sudo pacman -Syu email-ycorp-bin
   ```

`pacman-key --lsign-key` trusts only the key with that full fingerprint. If
`ycorp.asc` held any other key, signing fails and pacman refuses everything
from `[ycorp]`. To compare the downloaded key with this page first,
`gpg --show-keys ycorp.asc` prints its fingerprint:
`20B0 87BF 1D1B 2A04 1028  82F2 1308 1979 B6CE ABDA`.

`SigLevel = Required` makes pacman refuse a database or a package without a
valid signature from that key. `$arch` stays as written: pacman fills in
`x86_64` or `aarch64`.

- **Architectures:** x86_64, and aarch64 for Arch Linux ARM.
- **Coming from a makepkg build:** a machine that built `email-ycorp-bin`
  from this repository already has the package; `pacman -Syu` replaces it
  with the next release from `[ycorp]`. `email-ycorp`, built from source, is
  a different app ID with its own data and credentials: installing
  `email-ycorp-bin` replaces it, and accounts have to be added again.
- **Stopping updates:** delete the `[ycorp]` section from `/etc/pacman.conf`,
  then `sudo pacman-key --delete 20B087BF1D1B2A04102882F213081979B6CEABDA`.
  `sudo pacman -Rns email-ycorp-bin` removes the app.

## Signing key

The APT repository's key ([Linux updates](linux-updates.md#signing-key))
signs every package and database:

| | |
| --- | --- |
| Algorithm | ed25519 |
| Fingerprint | `20B0 87BF 1D1B 2A04 1028  82F2 1308 1979 B6CE ABDA` |
| User ID | Y Corp AS Email packages <support@ycorplabs.com> |
| Expires | 2031-11-14 |

The profile records the fingerprint (`LINUX_APT_KEY_FINGERPRINT`) and the
public key sits beside it (`config/profiles/official.apt.asc`); `ycorp.asc`
is a byte-for-byte copy. The private key lives in the release secret store
and is read only from the GnuPG home named by `EMAIL_APT_GNUPGHOME`.

Unlike the `.deb`, no package carries the key, so machines don't pick up a
new expiry date by themselves. When the key is extended
([expiry and rotation](linux-updates.md#expiry-and-rotation)), tell Arch
users to run the first two setup lines again, well before 2031-11-14: the
updated `ycorp.asc` carries the new date, and the local signature stays. A
rotation to a new key needs the whole setup again with the new fingerprint.

## Repository layout

The repository is served from the `ycorp-updates` R2 bucket under
`email/arch/`, at `https://updates.ycorplabs.com/email/arch`:

```
ycorp.asc                                                   public key
<arch>/email-ycorp-bin-<pkgver>-<pkgrel>-<arch>.pkg.tar.zst   package
<arch>/email-ycorp-bin-<pkgver>-<pkgrel>-<arch>.pkg.tar.zst.sig
<arch>/ycorp.db, ycorp.db.sig                               the database pacman -Sy reads
<arch>/ycorp.files, ycorp.files.sig                         file lists, for pacman -F
<arch>/ycorp.db.tar.gz, ycorp.files.tar.gz (and .sig)       the same bytes, under repo-add's names
```

`<arch>` is `x86_64` or `aarch64`, and `<pkgver>` is the Arch form of the
release version (`0.1.0beta3`, [release versions](release-versions.md)).
repo-add writes `ycorp.db` and `ycorp.files` as symlinks; the bucket has no
symlinks, so the staging directory holds real copies, and their signatures
are copies too. Every package and its signature are binary detached OpenPGP
signatures; the database also carries each package's signature
(`repo-add --include-sigs`). Each database lists only the newest package for
its architecture, and every earlier package file stays in the repository.

## Publishing

```sh
export EMAIL_DISTRIBUTION_PROFILE=official
export EMAIL_APT_GNUPGHOME=/path/to/release/gnupg-home   # outside this checkout
# Optional, for signing without a pinentry prompt:
# export EMAIL_APT_PASSPHRASE_FILE=/path/to/passphrase
./scripts/publish-arch-repo.sh --staging ../email-updates/arch \
  dist-linux/aur-bin/email-ycorp-bin-0.2.0beta1-1-x86_64.pkg.tar.zst \
  dist-linux/aur-bin/email-ycorp-bin-0.2.0beta1-1-aarch64.pkg.tar.zst
./scripts/upload-updates.sh --dry-run --source ../email-updates/arch --prefix email/arch
./scripts/upload-updates.sh --source ../email-updates/arch --prefix email/arch
```

The packages are the ones `scripts/prepare-aur-package.sh --build` writes to
`dist-linux/aur-bin/` from the release `.deb` files
([AUR packages](linux-aur.md#publish-a-release), step 3).

`publish-arch-repo.sh` stages; it never uploads. It reads each package with
`bsdtar` in an `archlinux:base` container without network access, and
refuses a package unless:

- it is `email-ycorp-bin` for x86_64 or aarch64, with a release version and
  no epoch, and its file name matches;
- it was repackaged from this profile's distribution `.deb`: the profile's
  desktop entry and AppStream file, the app and `mail-mcp`, and
  `RELEASE-CHANNEL.txt` saying `distribution`;
- it is the only package for its architecture in this run, and newer, in
  pacman's order, than what the database already lists;
- a `.sig` beside it, if there is one, is a valid signature by the profile's
  key. Without one, the package is signed.

It also refuses a GnuPG home without the profile's secret key. It copies the
staging directory (package files are hard-linked and never change) and
checks the copy's signatures against the committed public key before adding
anything. Signing happens on the host with `gpg`; the container only reads
packages and runs `repo-add` and never sees key material. Then it replaces
repo-add's symlinks with real files, signs the databases, and checks the result
as pacman would: every database entry against its package's size, SHA-256 and
signature, and every package and database signature with `gpgv` against the
committed public key only. Only then does it replace the staging directory. A
lock beside the staging directory serializes publishers.

Keep the staging directory between releases: each database is updated from
it. The upload script refuses a staging directory that lacks the published
package, so a stale or empty one cannot roll the repository back. To recreate
it, download `ycorp.asc` and, for each architecture, `ycorp.db`,
`ycorp.files`, both `.tar.gz` names and every `.sig` of them, plus the
package and `.sig` that `ycorp.db` names, into the same paths.

### Uploading

`scripts/upload-updates.sh` ([Linux updates](linux-updates.md#uploading))
verifies the directory again with the profile's public key, compares it with
what is published at `https://updates.ycorplabs.com/email/arch`, and uploads in
dependency order:

1. packages and their signatures, cached for a year (`immutable`);
2. `ycorp.asc`;
3. `ycorp.files`, and the `.tar.gz` names with their signatures;
4. `ycorp.db` and `ycorp.db.sig`, the only files `pacman -Sy` reads.

Everything except packages is cached for five minutes. The comparison
refuses a staging directory whose database lists an older package than the
published one, or whose files lack the published package. Packages already
uploaded are recorded in `.upload-manifest.json` and skipped; the script
refuses one whose bytes changed.

A database and its signature are separate files, so for up to five minutes
after an upload a client can get the new `ycorp.db` with the old
`ycorp.db.sig` from a cache, or the reverse. pacman then reports an invalid
database signature and installs nothing; running `pacman -Syu` again a few
minutes later works.

## Tests

```sh
python3 scripts/arch-repository.test.py   # vercmp, package checks, databases, staging, gpgv
python3 scripts/upload-updates.test.py    # upload order, caching, manifest, published-history check
./scripts/test-arch-repo.sh               # end to end in containers, throwaway key
```

`test-arch-repo.sh` generates a throwaway ed25519 key (never the production
key) and a fixture profile with it. It downloads the amd64 and arm64 `.deb`
files of v0.1.0-beta.2 and v0.1.0-beta.3 from GitHub, checks them against
each release's `SHA256SUMS`, and builds `email-ycorp-bin` 0.1.0beta2-1 and
0.1.0beta3-1 for both architectures from the aur-bin PKGBUILD with `makepkg`,
as `prepare-aur-package.sh` does. It publishes them with
`publish-arch-repo.sh`, serves the staged repository over HTTP from a
container on an internal Docker network, and in an Arch Linux x86_64
container checks that:

- the publisher refuses a build with another release channel or another
  profile's desktop entry, a package signed by another key, a signing key
  other than the profile's and an older or repeated version, and leaves the
  staging directory unchanged;
- the setup above, read from this document, trusts the key, adds `[ycorp]`
  and installs 0.1.0beta2-1, validated by signature;
- with 0.1.0beta3-1 on offer, a tampered database, an unsigned database, a
  tampered package, an unsigned package and a package signed by another key
  are each refused and install nothing;
- `pacman -Syu` then upgrades to 0.1.0beta3-1, the installed `email-linux`
  and `mail-mcp` find all their libraries, and `mail-mcp` runs;
- the upload plan puts packages first and `ycorp.db` last.

With `--alarm-rootfs ArchLinuxARM-aarch64-latest.tar.gz` (the official rootfs
from <http://os.archlinuxarm.org/os/>; check its `.sig` against the Arch
Linux ARM build key first), it also runs the setup and the upgrade in an Arch
Linux ARM container, natively on an arm64 host.

The client containers start from test images with the package's runtime
libraries installed and their distribution's repositories removed, so
`[ycorp]` is the only repository they can reach. Not covered: the production
key and bucket (nothing is uploaded), HTTPS and the CDN cache, the `curl`
download over the internet, and running the app on a desktop.
