#!/usr/bin/env bash
# Sourced by the Linux packagers. The release version and the forms that
# Debian and Arch need; docs/release-versions.md explains the scheme.

# The release version, from apps/linux/Cargo.toml: 0.1.0-beta.2, 0.1.0.
email_release_version() {
  sed -n 's/^version = "\([^"]*\)"/\1/p' "$1/apps/linux/Cargo.toml" | head -1
}

# Debian version: 1:0.1.0~beta.2, 1:0.1.0. The epoch puts every version above
# the plain 0.1.0 that v0.1.0-beta.1 shipped; `~` sorts a pre-release before
# its release. Keep the epoch on every later version.
email_deb_version() {
  printf '1:%s\n' "$(printf '%s' "$1" | sed 's/-/~/')"
}

# Arch pkgver: 0.1.0beta2, 0.1.0. pacman's vercmp already sorts 0.1.0beta2
# before 0.1.0, so no epoch is needed.
email_arch_pkgver() {
  printf '%s\n' "$1" | sed -E 's/-([A-Za-z]+)\.?/\1/'
}
