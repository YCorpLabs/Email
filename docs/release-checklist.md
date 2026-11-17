# Release checklist

One version, both platforms, from version bump to GitHub release. Details live
in [release versions](release-versions.md), [macOS distribution
builds](macos-release.md), [macOS updates](macos-updates.md), [Linux
updates](linux-updates.md), [Arch Linux updates](arch-updates.md) and [AUR
packages](linux-aur.md).

The commands use `0.2.0-beta.1` and build `3`; substitute the release's
values. Keep the three update staging directories between releases, outside
this checkout, for example `../email-updates/macos`, `../email-updates/apt`
and `../email-updates/arch`.

## 1. Prepare the version

- [ ] Set the release version in `apps/linux/Cargo.toml` and
      `apps/linux/Cargo.lock`, and `_tag`, `pkgver`, `pkgrel` and `_debver` in
      both PKGBUILDs.
- [ ] Run `python3 scripts/generate-third-party-notices.py` (the lockfile
      change alters the notices).
- [ ] Pick `EMAIL_BUILD_NUMBER`: one more than the last release on any channel.
- [ ] Merge, then build every artifact from that commit.

```sh
export EMAIL_DISTRIBUTION_PROFILE=official
export EMAIL_RELEASE_GOOGLE_CLIENT_ID='<production desktop client ID>'
# EMAIL_RELEASE_GOOGLE_CLIENT_SECRET from the release environment.
```

## 2. Build Linux

```sh
EMAIL_LINUX_RELEASE_CHANNEL=distribution ./scripts/package-linux-release.sh aarch64 x86_64
./scripts/verify-linux-packages.sh aarch64 x86_64
```

- [ ] `dpkg-deb -c dist-linux/email-linux_0.2.0-beta.1_amd64.deb` lists
      `./etc/apt/sources.list.d/email-linux.sources` and
      `./usr/share/keyrings/email-linux-archive-keyring.asc`, and the sources
      file names `Suites: beta` (`stable` for a final).
- [ ] Every `BUILD-INFO-*.txt` names the release commit.

Then the Arch packages, repackaged from those `.deb` files
([AUR packages](linux-aur.md#publish-a-release), step 3):

```sh
./scripts/prepare-aur-package.sh --ref <release commit> --build --update-repo
```

- [ ] `dist-linux/aur-bin/` has `email-ycorp-bin-0.2.0beta1-1-x86_64.pkg.tar.zst`
      and `email-ycorp-bin-0.2.0beta1-1-aarch64.pkg.tar.zst`.

## 3. Build, sign and notarize macOS

With a Developer ID (betas on the testing channel, finals on stable):

```sh
# Signs with the release keychain (macos-release-signing.sh status shows it).
export EMAIL_NOTARY_PROFILE=email-release
export EMAIL_BUILD_NUMBER=3
export EMAIL_UPDATE_CHANNEL=testing    # stable for a final
./scripts/package-macos-release.sh --distribute
EMAIL_VERIFY_REQUIRE_UPDATES=1 ./scripts/verify-macos-release.sh dist-release/distribution/Email.app
```

`--distribute` notarizes and staples the app and the DMG, checks them with
Gatekeeper and writes `Email-0.2.0-beta.1-3-<arch>.zip`, the update archive.
Build on Apple Silicon and on Intel for both architectures. While it signs, the
packager puts the release keychain on the user keychain search list and
restores the list afterwards ([release keychain](macos-release.md#release-keychain)).

Without a Developer ID, build `--beta` instead (`EMAIL_BUILD_NUMBER=3
./scripts/package-macos-release.sh --beta`): no notarization, no feed, and
steps 4 and 6 do not apply to macOS.

- [ ] About shows **Email 0.2.0-beta.1 (3)**.
- [ ] Opening the DMG shows the installer window: Email, an arrow, Applications
      ([DMG window](macos-release.md#dmg-window)).

## 4. Publish the macOS feed

```sh
EMAIL_UPDATE_ED_PRIVATE_KEY="$(<from the release secret store>)" \
./scripts/publish-macos-update.sh \
  --archive dist-release/distribution/Email-0.2.0-beta.1-3-arm64.zip \
  --channel testing \
  --feed-dir ../email-updates/macos \
  --download-prefix https://updates.ycorplabs.com/email/macos/ \
  --ed-key-file -
```

Repeat for the other architecture's archive. Use `--channel stable` for a
final. The script stages and verifies; it uploads nothing.

## 5. Stage the Linux repositories

Both are signed with the same key, from the same GnuPG home:

```sh
export EMAIL_APT_GNUPGHOME=<release GnuPG home>
./scripts/publish-linux-apt.sh --staging ../email-updates/apt \
  dist-linux/email-linux_0.2.0-beta.1_amd64.deb dist-linux/email-linux_0.2.0-beta.1_arm64.deb
./scripts/publish-arch-repo.sh --staging ../email-updates/arch \
  dist-linux/aur-bin/email-ycorp-bin-0.2.0beta1-1-x86_64.pkg.tar.zst \
  dist-linux/aur-bin/email-ycorp-bin-0.2.0beta1-1-aarch64.pkg.tar.zst
```

A beta goes to the APT `beta` suite; a final goes to `stable` and `beta`.
`[ycorp]` has no suites: every release, beta or final, replaces the previous
package for its architecture.

## 6. Upload

```sh
./scripts/upload-updates.sh --dry-run --source ../email-updates/macos --prefix email/macos
./scripts/upload-updates.sh --dry-run --source ../email-updates/apt --prefix email/apt
./scripts/upload-updates.sh --dry-run --source ../email-updates/arch --prefix email/arch
# Review the three plans, then:
./scripts/upload-updates.sh --source ../email-updates/macos --prefix email/macos
./scripts/upload-updates.sh --source ../email-updates/apt --prefix email/apt
./scripts/upload-updates.sh --source ../email-updates/arch --prefix email/arch
```

- [ ] `curl -fsS https://updates.ycorplabs.com/email/macos/appcast.xml` lists
      the new build.
- [ ] `curl -fsS https://updates.ycorplabs.com/email/apt/dists/beta/InRelease`
      shows the new `Date`.
- [ ] `curl -fsS https://updates.ycorplabs.com/email/arch/x86_64/ycorp.db | tar -tzf - | head -1`
      shows `email-ycorp-bin-0.2.0beta1-1/`, and the same for `aarch64`.

## 7. GitHub release

- [ ] Tag the release commit `v0.2.0-beta.1` and push the tag.
- [ ] Create the GitHub release from the tag and mark it as the latest
      release, also for a beta (the version name already says beta, and a
      pre-release never shows as Latest on GitHub). Attach the `.deb` files,
      the macOS downloads (DMG or zip), the Linux binaries and `SHA256SUMS`.
      Check the uploaded assets against their checksums.
- [ ] Record the AUR checksums on `main` ([AUR packages](linux-aur.md)).

## 8. Check the update path

- [ ] On a Mac with the previous release: **Check for Updates…** offers and
      installs the new build (testing channel for a beta).
- [ ] On Ubuntu with the previous release: `sudo apt update && apt list
      --upgradable` shows `email-linux`, and `sudo apt upgrade` installs it.
- [ ] On Arch with the previous release from `[ycorp]`: `sudo pacman -Syu`
      installs the new `email-ycorp-bin`, and `pacman -Qi email-ycorp-bin`
      says `Validated By : Signature`.
