# AUR packages

Email has two Arch packages. Their PKGBUILDs live in this repository and both
declare `x86_64` and `aarch64`:

| Package | PKGBUILD | Installs |
| --- | --- | --- |
| `email-ycorp` | `apps/linux/packaging/aur/PKGBUILD` | The GTK shell, built on the user's machine from the release's source tarball, with the neutral profile |
| `email-ycorp-bin` | `apps/linux/packaging/aur-bin/PKGBUILD` | The release's prebuilt `.deb` for the user's CPU, with the official profile. Provides and conflicts with `email-ycorp` |

Publishing to the AUR is a maintainer action. Nothing in this repository pushes
to the AUR.

## Installing before the AUR packages exist

New AUR account registration was closed when the first beta shipped, so the
packages are not on the AUR yet. Until they are, build them from a clone of
this repository:

```sh
git clone https://github.com/YCorpLabs/Email
cd Email/apps/linux/packaging/aur-bin   # or aur, to build from source
makepkg -si
```

This needs the PKGBUILDs on `main` to carry the release's checksums (step 5
below) and the release to be published, because both PKGBUILDs download their
sources from the GitHub release.

## email-ycorp: built from source

The source is `Email-<version>.tar.gz`, attached to the GitHub release: a
`git archive` of the release commit with the prefix `Email-<version>/`, the
same layout as GitHub's tag archive. It is a release asset rather than
GitHub's generated `archive/refs/tags/...` download because its checksum is
known before the tag exists and never depends on how GitHub generates archives.
`scripts/prepare-aur-package.sh` writes it.

An AUR build runs on the user's machine, so it is a local build:
`scripts/linux-build-config.sh` refuses distribution profiles there and embeds
no OAuth application values unless the builder supplies them. With no
selection, the package builds the neutral `unofficial` distribution profile
(see [distribution profiles](distribution-profile.md)):

- app ID, data directory and Secret Service collection `invalid.email.Unofficial`;
- no support or feedback address and no Scheduling service;
- no Google OAuth client, so one-click Google sign-in is unavailable. Gmail via
  an app password over IMAP, iCloud and other IMAP/SMTP accounts work.

## email-ycorp-bin: prebuilt

It repackages `email-linux_<version>_amd64.deb` or `email-linux_<version>_arm64.deb`
from the release: the binary, desktop entry, AppStream metadata, icon and
license notices. It leaves out the `.deb`'s AppArmor profile, which exists for
Ubuntu's user-namespace restriction. `_debver` is the version in the `.deb`
file names, the `version` in `apps/linux/Cargo.toml`.

The `.deb` files come from `scripts/package-linux-release.sh` with the
`official` profile and the `distribution` channel, built in Ubuntu 24.04
containers. That is the official identity: app ID and data directory
`com.ycorplabs.Mail`, the production Google desktop client (Google sign-in stays
off until Google's review passes), the support address and Scheduling. The
binary links OpenSSL 3, which Arch ships as `openssl-3`.

The two packages use different app IDs, data directories and Secret Service
collections. Switching between them means adding accounts again.

## Publish a release

1. Set the release version in `apps/linux/Cargo.toml`, then `_tag`, `pkgver`
   and `pkgrel=1` in both PKGBUILDs and `_debver` in `aur-bin`, in the forms
   [release versions](release-versions.md) lists (for v0.1.0-beta.2:
   `v0.1.0-beta.2`, `0.1.0beta2`, `0.1.0-beta.2`). Set the `# Maintainer:`
   lines to the AUR account's name and address. The script refuses mismatched
   values.
2. Build the Linux release from the release commit, with the production Google
   desktop client values in the environment
   ([Linux release packages](../apps/linux/README.md#release-packages-for-x86_64-and-arm64-from-one-machine)):

   ```sh
   EMAIL_DISTRIBUTION_PROFILE=official EMAIL_LINUX_RELEASE_CHANNEL=distribution \
     scripts/package-linux-release.sh
   ```
3. Run `scripts/prepare-aur-package.sh --ref <release commit> --build --update-repo`.
   In a clean Arch x86_64 container it writes the source tarball
   `dist-linux/aur/Email-<version>.tar.gz`, records its SHA-256 and those of
   the two `.deb` files in the PKGBUILDs, writes `.SRCINFO`, verifies every
   source against its checksum, runs `namcap`, builds `email-ycorp-bin` for
   both architectures and `email-ycorp` for x86_64 with dependencies from the
   Arch repositories, and runs `namcap` on the packages. The files to publish
   are in `dist-linux/aur/` and `dist-linux/aur-bin/`; `--update-repo` also
   copies them into `apps/linux/packaging/`. After the tag exists, `--ref` can
   be left out.
4. Attach the source tarball and both `.deb` files to the GitHub release, with
   `SHA256SUMS`, and publish it. The PKGBUILDs download from
   `releases/download/<tag>/`.
5. Commit the updated PKGBUILDs and `.SRCINFO` files to `main`.
6. Optional: install-test the built packages in a clean Arch container.
   `verify-linux-packages.sh` also re-checks the x86_64 `.deb`:

   ```sh
   EMAIL_LINUX_ARCH_PACKAGE=$PWD/dist-linux/aur-bin/email-ycorp-bin-0.1.0beta1-1-x86_64.pkg.tar.zst \
     scripts/verify-linux-packages.sh x86_64
   EMAIL_LINUX_ARCH_PACKAGE=$PWD/dist-linux/aur/email-ycorp-0.1.0beta1-1-x86_64.pkg.tar.zst \
     scripts/verify-linux-packages.sh x86_64
   ```
7. Create an account on <https://aur.archlinux.org> and add an SSH public key
   under *My Account*. Then, for each package:

   ```sh
   git clone ssh://aur@aur.archlinux.org/email-ycorp.git aur-email-ycorp
   cp dist-linux/aur/PKGBUILD dist-linux/aur/.SRCINFO aur-email-ycorp/
   cd aur-email-ycorp
   git add PKGBUILD .SRCINFO
   git commit -m "Initial import: email-ycorp 0.1.0beta1"
   git push origin master
   ```

   and the same for `email-ycorp-bin` with `dist-linux/aur-bin/`. The first
   push creates the package. An AUR repository must contain only `PKGBUILD`,
   `.SRCINFO` and files the PKGBUILD references.
8. On a real Arch or Omarchy machine, install it the way users will
   (`yay -S email-ycorp-bin` or `git clone` plus `makepkg -si`) and check the
   launcher and icon, Wayland rendering, keyboard navigation, Secret Service
   storage across a relaunch and account sign-in.

For later releases, bump `_tag`, `pkgver` and `_debver` (or `pkgrel` for
packaging-only changes), rerun steps 2 to 5 and push the new PKGBUILD and
`.SRCINFO` to both AUR repositories.

## Verification without a release

`scripts/prepare-aur-package.sh --local` checks and builds the same PKGBUILDs
from `git archive HEAD` instead of a release commit, in a clean Arch x86_64
container, and writes to `dist-linux/aur-check/`. It includes
`email-ycorp-bin` when `dist-linux` holds the two `.deb` files. Its
checksums are for local files: never publish or commit them.
