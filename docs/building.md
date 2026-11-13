# Building and running Email

Email has one Rust engine and two native desktop apps: AppKit on macOS and
GTK 4 on Linux. This guide gets a fresh clone running without a mail account,
signing certificate or OAuth app registration.

## Quick start

```sh
git clone https://github.com/YCorpLabs/Email.git
cd Email
./scripts/dev.sh
```

`scripts/dev.sh` works on macOS and on a Linux desktop. It builds debug
binaries and opens the app with three labelled demo messages in an isolated
store at `dist/demo/mail.sqlite`. It does not open your real mail store, sign
anything or need OAuth configuration. Run `rm -rf dist/demo` to start over.

`./scripts/dev.sh --demo` opens the [launch demo mailbox](demo-mailbox.md)
instead: about 270 synthetic conversations with replies, attachments,
invitations, newsletters and splits, in a fresh temporary store.
`--demo search` adds 100,000 older conversations for search shots.

The first build downloads the pinned Rust toolchain and the Rust and Swift
dependencies, so it needs a network connection and takes a few minutes.

A fresh clone builds the neutral `unofficial`
[distribution profile](distribution-profile.md): its own app identifiers,
Keychain names and data directory, no OAuth client, support address,
Scheduling service or update feed. Maintainer checkouts select `development`
in the ignored `config/distribution-profile.local`.
`scripts/dev.sh` builds whichever non-distribution profile is selected and
refuses the official one.

## Requirements

| | Needed | Tested in this repository |
| --- | --- | --- |
| All platforms | Git, Bash, Python 3, [rustup](https://rustup.rs) | rustup installs Rust 1.99.0 from `rust-toolchain.toml` on first use |
| macOS | Swift 6 toolchain: Command Line Tools (`xcode-select --install`) or Xcode. The app runs on macOS 13 or later. | Swift 6.4 from Command Line Tools with the macOS 27 SDK on macOS 27 (Apple Silicon). The CI workflow targets GitHub's `macos-26` and `macos-26-intel` images. The package declares Swift tools version 6.0; older Swift 6 releases are untested. |
| Linux | GTK 4.10 or later, WebKitGTK 6.0 API (`webkitgtk-6.0` 2.40 or later), OpenSSL, D-Bus, pkg-config and a C toolchain | Ubuntu 24.04 (GTK 4.14, WebKitGTK 2.52) in the ARM64 build container; earlier hosted CI runs also built it on x86_64 |
| Linux container build | Docker or Podman on macOS or Linux | Docker Desktop on Apple Silicon |

Linux build packages:

```sh
# Ubuntu 24.04 or later, Debian 13
sudo apt install build-essential pkg-config libssl-dev libdbus-1-dev libgtk-4-dev libwebkitgtk-6.0-dev
# Fedora
sudo dnf install gcc pkgconf-pkg-config openssl-devel dbus-devel gtk4-devel webkitgtk6.0-devel
# Arch
sudo pacman -S --needed base-devel pkgconf openssl dbus gtk4 webkitgtk-6.0
```

Ubuntu 22.04 and older do not ship WebKitGTK 6; use the container build there.
The Debian, Fedora and Arch package names come from those distributions'
repositories and have not been tested by this project.

Connecting a real account on Linux also needs a Secret Service provider such as
GNOME Keyring or KWallet. Demo mode does not.

## macOS

### Unsigned development run

`./scripts/dev.sh` builds `mail-service` with Cargo and the app with SwiftPM, then
runs the SwiftPM executable directly. That executable is not wrapped in an
`.app` bundle, so:

- It appears as `email-desktop` without the app icon, and new-mail
  notifications are off (macOS only delivers them to app bundles).
- macOS identifies it by its code signature hash. Demo data needs no
  credentials, and an isolated store keeps its own account and Scheduling
  credentials (see [demo mode](#demo-mode-and-isolated-stores)), so a demo run
  does not read the Keychain items of an installed Email. If you add a test
  account (`--no-demo --store /absolute/path/test.sqlite`), macOS asks for
  Keychain access whenever a differently built executable reads that account's
  item, so expect prompts again after rebuilds. Allow them only for accounts you
  added in a development build, and deny any prompt you do not expect.
- iCloud and IMAP accounts work with an app-specific password. Gmail and
  Outlook sign-in need a desktop OAuth client registration; see
  [cloud provider setup](../crates/mail-cloud/README.md).

Options: `--release` builds optimized binaries, `--store PATH` selects another
absolute database path, `--no-demo` skips demo messages, `--demo
[launch|search|small]` starts a demo set on a fresh temporary store (see
[launch demo mailbox](demo-mailbox.md)), and `--smoke` runs a headless engine
check instead of opening the window.

### Signed Email.app (maintainer path)

`./scripts/run-macos.sh` builds `dist/Email.app` for the selected profile with
a persistent local code signing identity. Its launcher signature stays the same
across rebuilds, so Keychain grants survive them. Setup is a one-time, manual
Keychain step, and needs OpenSSL 3 on your `PATH` (for example from Homebrew);
the system LibreSSL cannot create the signing bundle:

1. `./scripts/setup-macos-signing.sh` creates a self-signed identity in your
   login Keychain.
2. In Keychain Access, rename its private key as the script instructs.
3. Run `./scripts/authorize-macos-signing.sh` in your own Terminal.

Ad-hoc signing (`EMAIL_CODESIGN_IDENTITY=-`) does not produce a working app:
the launcher checks that `dist/Runtime` libraries match its own designated
requirement, and an ad-hoc requirement is that one file's hash. Use
`scripts/dev.sh` instead. Distribution builds are covered in
[macOS distribution builds](macos-release.md).

### Building by hand

```sh
cargo build -p mail-service --locked
# The Swift build needs this generated, Git-ignored file:
printf 'import Foundation\nenum EmailBuildIdentity { static let value = "%s" }\n' \
  "$(python3 scripts/build-identity.py)" > apps/macos/Sources/EmailUI/BuildIdentity.generated.swift
swift build --package-path apps/macos --disable-sandbox --only-use-versions-from-resolved-file
EMAIL_DEMO=1 EMAIL_STORE_PATH="$PWD/dist/demo/mail.sqlite" \
  MAIL_SERVICE_LIBRARY="$PWD/target/debug/libmail_service.dylib" \
  "$(swift build --package-path apps/macos --disable-sandbox --show-bin-path)/email-desktop"
```

## Linux

### On a Linux desktop

Install the build packages above, then run `./scripts/dev.sh`.
`./scripts/run-linux.sh` builds a release binary and runs it against your real
mail store. [The Linux app README](../apps/linux/README.md) covers `.deb` and
Arch packages. Those packages need the project's desktop OAuth configuration.

Ubuntu 24.04 and later block user namespaces for unconfined programs, which
stops WebKitGTK's sandbox (`bwrap: Creating new namespace failed` or
`Failed to fully launch dbus-proxy`). The `.deb` installs a narrow AppArmor
profile for `/usr/bin/email-linux`. For a development binary, install the same
profile for its path. It allows only that binary to create user namespaces and
keeps WebKit's sandbox on. These commands have not been tested on a desktop:

```sh
binary="$PWD/apps/linux/target/debug/email-linux"   # absolute path, no spaces
sed -e "s#profile email-linux /usr/bin/email-linux#profile email-linux-dev $binary#" \
    -e 's#<local/email-linux>#<local/email-linux-dev>#' \
    apps/linux/packaging/debian/email-linux.apparmor | sudo tee /etc/apparmor.d/email-linux-dev >/dev/null
sudo apparmor_parser -r /etc/apparmor.d/email-linux-dev
```

Do not disable WebKit's sandbox or the system-wide namespace restriction
instead. In a virtual machine without GPU acceleration, add
`EMAIL_SOFTWARE_RENDERING=1`.

### From any computer with Docker

```sh
./scripts/build-linux-container.sh          # writes dist-linux/email-linux-<arch>
./scripts/build-linux-container.sh --test   # also runs the Rust and Linux shell tests
```

The script builds an Ubuntu 24.04 image with the pinned Rust version
(`email-linux-dev:<version>`), copies in only the files Git would commit, and
compiles a release binary of the neutral `unofficial` profile (an ignored
profile selection does not enter the container) for this computer's CPU (`aarch64` on Apple Silicon,
`x86_64` on Intel and most PCs). Ignored local files, such as OAuth
configuration or mail databases, never enter the container. Cargo downloads are
cached in the `email-linux-cargo` volume, which `scripts/validate-local.py`
shares, and build output in `email-linux-dev-target`. The binary is written by
your shell, so you own it under Docker or rootless Podman.

The container has no display. Copy the binary to a Linux desktop with the
runtime libraries (Ubuntu 24.04: `sudo apt install libgtk-4-1 libwebkitgtk-6.0-4`)
and start it with an isolated store:

```sh
EMAIL_DEMO=1 EMAIL_STORE_PATH="$HOME/email-demo/mail.sqlite" ./email-linux-aarch64
```

The Ubuntu AppArmor note above applies to this binary too.

## Demo mode and isolated stores

Both apps read two environment variables:

- `EMAIL_STORE_PATH=/path/to/mail.sqlite` uses that database instead of the
  real store, `mail.sqlite` in the profile's data directory:
  `~/Library/Application Support/<directory>` on macOS and
  `$XDG_DATA_HOME/<directory>` on Linux (`invalid.email.unofficial` and
  `invalid.email.Unofficial` for a plain build, `Email` and `email-client` for
  the development profile). Relative paths resolve
  against the working directory. The store keeps its own account settings and
  custom sender pictures beside it, and any store other than the default one
  has its own Scheduling credential item. Mail account credentials are always per account. On Linux,
  preference files are kept beside it too. On macOS, preferences stay in the
  executable's defaults domain (`email-desktop` for `scripts/dev.sh` runs,
  separate from Email.app's). Both apps create a missing store directory with
  mode `700`, refuse a symlinked database file and create the database and
  account files private (`600`); `scripts/dev.sh` also requires the directory
  to be private and not a symlink. On Linux the app runs as its own GTK
  application for that store: it never hands off to an installed copy, and
  relaunching the same store reuses its window. An empty `EMAIL_STORE_PATH`
  stops both apps at launch.
- `EMAIL_DEMO=1` adds a demo account (`alex@example.com`) and three labelled
  example messages to that store on every launch. `EMAIL_DEMO=launch` adds the
  [launch demo mailbox](demo-mailbox.md) once instead. Both only work together with
  `EMAIL_STORE_PATH`; on its own it is ignored with a warning, so demo messages
  never reach your real store. Only a separate directory isolates a store: a
  path in the real store's directory (through `..` or a directory symlink, for
  example) shares its settings, so it is treated as the real store and gets no
  demo data either. `scripts/dev.sh` refuses such a `--store`. `mail-service` itself refuses demo data
  in the default store for every front end, including `mail-mcp --seed-demo`.

`scripts/dev.sh` sets both, and refuses a `--store` in any profile's real data
directory or linked to its database. Demo accounts have no credentials or
provider settings, so sync has nothing to fetch and reports success; archive,
snooze and other changes stay local, and sending fails.

To explore the MCP server against demo data, first run `./scripts/dev.sh`,
open **Agent access** in the sidebar and turn it on for the demo account:
agents see only accounts you turn on there. Then build the server into its own
directory and register it with the demo store. It is read-only unless you pass
write flags; changes also need the matching in-app grants, and sends wait for
your approval in Agent access:

```sh
cargo build -p mail-mcp --release --locked --target-dir target/contributor
claude mcp add --scope user --transport stdio email-demo -- \
  "$PWD/target/contributor/release/mail-mcp" --db "$PWD/dist/demo/mail.sqlite"
```

Codex uses `codex mcp add email-demo -- <same command>`. The separate target
directory leaves `target/release/mail-mcp` alone: on a maintainer's Mac that
is the signed binary that `scripts/connect-mcp.sh` registers and
`scripts/build-mcp-macos.sh` signs. Like the apps, a server on any store outside
the default directory keeps its own Scheduling connection. On Linux the server
must see the same `XDG_DATA_HOME` as the app to recognize the real store. Changing Scheduling
settings always needs `--allow-booking-settings` and approval in the browser.
See [MCP](mcp.md).

## Tests

| What | Command |
| --- | --- |
| Rust engine and MCP server (any OS) | `cargo test --workspace --locked` |
| Linux shell (needs GTK dev packages) | `cargo test --manifest-path apps/linux/Cargo.toml --locked` |
| Linux shell from macOS | `./scripts/build-linux-container.sh --test`, or `DOCS_RS=1 cargo check --manifest-path apps/linux/Cargo.toml` for a type check without GTK |
| macOS headless engine check | `./scripts/dev.sh --smoke` |
| macOS Swift rule harnesses | `python3 scripts/check-release-swift.py` |
| macOS launcher tooling | `python3 scripts/run-macos-launcher-cache.test.py` |
| Source checks | `python3 scripts/check-release-readiness.py` and `python3 scripts/check-toolchain.py` |

`scripts/validate-local.py` runs the same groups as CI against a clean,
committed checkout: `--profile source` (needs [Gitleaks](https://github.com/gitleaks/gitleaks)),
`rust`, `macos`, `linux` (needs the image from
`./scripts/build-linux-container.sh --image-only`; add
`--linux-target-volume <new name>` to start from an empty Linux target) and
`audit` (needs `cargo-audit`).

Once the workflow is enabled in the public repository's settings,
`.github/workflows/validate.yml` runs on pull requests and pushes to `main`.

The Rust code is not uniformly `rustfmt`-formatted. Do not reformat files you
are not otherwise changing.

## Where things live

| Path | What it is |
| --- | --- |
| `crates/mail-core` | Normalized mail model, SQLite cache, full-text search, drafts, outbox and action queue. No network access, no credentials. |
| `crates/mail-imap` | IMAP/SMTP adapter for iCloud and standard servers. |
| `crates/mail-cloud` | Gmail API and Microsoft Graph adapter, desktop OAuth with PKCE. |
| `crates/mail-service` | Account service: configuration, OS credential storage, sync, safe HTML rendering. JSON API in Rust and over a C ABI ([service API](service-api.md)). |
| `crates/mail-mcp` | Local stdio MCP server over the same cache and service ([MCP](mcp.md)). |
| `crates/distribution-profile`, `config/profiles` | Build-time distribution profiles: app identity, data directories and services ([distribution profiles](distribution-profile.md)). |
| `apps/macos` | SwiftPM package: `EmailDesktop` launcher, `EmailUI` AppKit app, `EmailUpdater` (Sparkle, release builds only). |
| `apps/linux` | GTK 4 and WebKitGTK app (separate Cargo workspace and lockfile), packaging and the build container. |
| `scripts/` | Build, run, packaging, validation and release tooling, with tests in the `*.test.py` files. |
| `contracts/scheduling.json` | Pinned contract for the separately hosted Scheduling service. |
| `assets/` | App icon. |
| `docs/` | Guides and internal records; start at the [docs index](README.md). |

## Common errors

| Message | Cause and fix |
| --- | --- |
| `cannot find 'EmailBuildIdentity' in scope` | The generated `BuildIdentity.generated.swift` is missing. Run `./scripts/dev.sh`, or generate it as in [Building by hand](#building-by-hand). |
| `Run scripts/setup-macos-signing.sh once to create a local signing identity.` | `run-macos.sh` builds the signed app. Use `./scripts/dev.sh`, or set up the identity as described above. |
| `Prepare legacy launcher metadata with --prepare-launcher-cache CDHASH before packaging.` | `dist/Email.app` exists without launcher cache metadata, for example a partial app left by an older `run-macos.sh` run. If it is your own throwaway build, delete `dist/Email.app` and rebuild. If Keychain grants are tied to that app, keep it and run `./scripts/run-macos.sh --prepare-launcher-cache CDHASH` with the app's independently recorded 40-character CDHash instead. |
| `failed to scan …/dist/Email.app: -10814 from spotlight` from `run-macos.sh` | LaunchServices could not register the app, as happens for a checkout inside a hidden directory (such as `~/.something`) or `/tmp`, which Spotlight does not index. Clone into an ordinary folder, or use `./scripts/dev.sh`. |
| `Email runtime signature is invalid. Rebuild the app.` | The app and its `dist/Runtime` libraries were not signed by the same persistent identity, for example with ad-hoc signing. Use `./scripts/dev.sh` or the maintainer setup. |
| `bundleProxyForCurrentProcess is nil` at launch | An older checkout ran the unbundled executable with notifications enabled. Update; unbundled runs now skip notifications. |
| rustup downloads a toolchain, or `toolchain '1.99.0-…' is not installed` | Expected on first use. Offline machines need `rustup toolchain install 1.99.0` beforehand. |
| `The system library 'webkitgtk-6.0' … was not found` | Install the Linux build packages above. Distributions without WebKitGTK 6 should use the container build. |
| `bwrap: Creating new namespace failed` or `Failed to fully launch dbus-proxy` | WebKit's sandbox cannot create a user namespace. See the Ubuntu AppArmor note above. Containers cannot run the app with the sandbox on. |
| `The store directory … must be private to you` from `scripts/dev.sh` | Use a new directory (it is created with mode `700`) or `chmod 700` your own. Shared directories such as `/tmp` itself are refused. |
| `EMAIL_DEMO=1 is ignored without EMAIL_STORE_PATH` | Set both, or use `./scripts/dev.sh`. |
| `No such image: email-linux-dev:…` from `validate-local.py --profile linux` | Run `./scripts/build-linux-container.sh --image-only`, or pass `--linux-image`. |
