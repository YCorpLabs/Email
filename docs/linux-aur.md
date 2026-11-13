# AUR package

`apps/linux/packaging/aur/PKGBUILD` is the Arch User Repository source package
`email-linux`. It downloads the release tag's tarball (`_tag`, for example
`v0.1.0-beta.1`) from the public repository
[YCorpLabs/Email](https://github.com/YCorpLabs/Email), builds the
GTK shell with the Arch `cargo`/`rust` toolchain on the user's machine, and
installs the binary, desktop entry, AppStream metadata, icon and license
notices. It declares `x86_64` and `aarch64`.

Publishing to the AUR is a maintainer action. Nothing in this repository pushes to
the AUR.

## What an AUR build is

An AUR build runs on the user's machine, so it is a local build:
`scripts/linux-build-config.sh` refuses distribution profiles there and embeds
no OAuth application values unless the builder supplies them. With no
selection, the package builds the neutral `unofficial` distribution profile
(see [distribution profiles](distribution-profile.md)):

- app ID, data directory and Secret Service collection `invalid.email.Unofficial`;
- no support or feedback address and no Scheduling service;
- no Google OAuth client, so one-click Google sign-in is unavailable. Gmail via
  an app password over IMAP, iCloud and other IMAP/SMTP accounts work.

**Open decision before publishing:** whether the AUR package should be this
neutral source build, or an `email-linux-bin` package that repackages the
official x86_64 `.deb` from a GitHub release (built with the `official`
profile and production OAuth configuration). The second keeps the official
identity on Arch but needs a release `.deb` and a separate PKGBUILD; it is
not prepared here.

## Publish a release

1. Set `_tag` in the PKGBUILD to the release tag, `v` plus the `version` in
   `apps/linux/Cargo.toml` and any pre-release suffix (for example
   `v0.1.0-beta.1`). Set `pkgver` to the same version in Arch form, without
   `-` (`0.1.0beta1`), and `pkgrel=1`. Set the `# Maintainer:` line to the
   AUR account's name and address.
2. Merge the release source into `main` of YCorpLabs/Email, where the AUR
   downloads the tarball.
3. Tag and push from a clone of YCorpLabs/Email:
   `git tag v0.1.0-beta.1 && git push origin v0.1.0-beta.1`.
4. Run `scripts/prepare-aur-package.sh --build`. In a clean Arch x86_64
   container it downloads the tag tarball, writes its SHA-256 into the
   PKGBUILD (`updpkgsums`), writes `.SRCINFO`, runs `namcap`, and builds the
   package with dependencies from the Arch repositories. The files to publish
   are `dist-linux/aur/PKGBUILD` and `dist-linux/aur/.SRCINFO`. Commit the
   updated `sha256sums` line back to this repository as well.
5. Optional: install-test the package it built in a clean Arch container.
   `verify-linux-packages.sh` also re-checks the x86_64 `.deb`, so run
   `scripts/package-linux-release.sh x86_64` first, then:

   ```sh
   EMAIL_LINUX_ARCH_PACKAGE=$PWD/dist-linux/aur/email-linux-0.1.0beta1-1-x86_64.pkg.tar.zst \
   scripts/verify-linux-packages.sh x86_64
   ```
6. Create an account on <https://aur.archlinux.org>, add an SSH public key under
   *My Account*, then:

   ```sh
   git clone ssh://aur@aur.archlinux.org/email-linux.git aur-email-linux
   cp dist-linux/aur/PKGBUILD dist-linux/aur/.SRCINFO aur-email-linux/
   cd aur-email-linux
   git add PKGBUILD .SRCINFO
   git commit -m "Initial import: email-linux 0.1.0beta1"
   git push origin master
   ```

   The first push creates the package. The AUR repository must contain only
   PKGBUILD, `.SRCINFO` and files the PKGBUILD references.
7. On a real Arch or Omarchy x86_64 machine, install it the way users will
   (`yay -S email-linux` or `git clone` plus `makepkg -si`) and check the
   launcher and icon, Wayland rendering, keyboard
   navigation, Secret Service storage across a relaunch and account sign-in.

For later releases, bump `_tag` and `pkgver` (or `pkgrel` for packaging-only changes),
tag in YCorpLabs/Email, rerun step 4 and push the new PKGBUILD and `.SRCINFO`.

## Verification without a tag

`scripts/prepare-aur-package.sh --local` builds the same PKGBUILD from
`git archive HEAD` instead of the tag tarball, in a clean Arch x86_64
container, and writes to `dist-linux/aur-check/`. Its PKGBUILD carries the
local archive's checksum and must never be published.
