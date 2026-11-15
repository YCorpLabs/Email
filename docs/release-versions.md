# Release versions

Each release has one version, written in the forms each platform needs:

| Where | v0.1.0-beta.2 | Final 0.1.0 | Set by |
| --- | --- | --- | --- |
| Git tag and GitHub release | `v0.1.0-beta.2` | `v0.1.0` | publishing |
| Release version (SemVer) | `0.1.0-beta.2` | `0.1.0` | `version` in `apps/linux/Cargo.toml` (and `apps/linux/Cargo.lock`); the Linux app shows it |
| `.deb` `Version` | `1:0.1.0~beta.2` | `1:0.1.0` | `scripts/release-version.sh`, from the release version |
| `.deb` file name | `email-linux_0.1.0-beta.2_amd64.deb` | `email-linux_0.1.0_amd64.deb` | the release version, which has no `:` or `~` |
| Arch `pkgver`, `pkgrel` | `0.1.0beta2`, `1` | `0.1.0`, `1` | both PKGBUILDs; `aur-bin` also sets `_debver` to the release version |
| macOS `CFBundleShortVersionString` | `0.1.0-beta.2` | `0.1.0` | `EMAIL_VERSION` |
| macOS `CFBundleVersion` | `2` | the next integer | `EMAIL_BUILD_NUMBER`: one more for every release |

v0.1.0-beta.1 shipped its `.deb` as plain `0.1.0`, and dpkg sorts
`0.1.0~beta.2` below that, so apt would not upgrade to it. The epoch `1:` puts
every later version above `0.1.0`, and `~` keeps each beta below its final
release; keep the epoch on every version from now on. pacman already sorts
`0.1.0beta2` after `0.1.0beta1` and before `0.1.0`, so the Arch packages need
no epoch.

`dpkg --compare-versions` and `vercmp` confirm the order:

- Debian: `0.1.0` < `1:0.1.0~beta.2` < `1:0.1.0~beta.10` < `1:0.1.0~rc.1` <
  `1:0.1.0` < `1:0.1.1~beta.1`.
- Arch: `0.1.0beta1` < `0.1.0beta2` < `0.1.0beta10` < `0.1.0rc1` < `0.1.0` <
  `0.1.1beta1`.

`pkgrel` starts at 1 for each new `pkgver` and rises only for a packaging-only
change. Sparkle orders macOS builds by `CFBundleVersion`: v0.1.0-beta.1 was
build 1 and v0.1.0-beta.2 is build 2.

To start a release, set the release version in `apps/linux/Cargo.toml` and
`apps/linux/Cargo.lock`, and `_tag`, `pkgver`, `pkgrel` and `_debver` in the
PKGBUILDs ([AUR packages](linux-aur.md)). `scripts/prepare-aur-package.sh`
refuses values that don't match. Pass `EMAIL_VERSION` and `EMAIL_BUILD_NUMBER`
to the [macOS build](macos-release.md#unnotarized-beta).
