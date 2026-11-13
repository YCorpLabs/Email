# Linux desktop app

Native GTK 4 inbox with an isolated WebKitGTK 6 reading pane. It uses the live `mail-service` API and keeps the local cache at `$XDG_DATA_HOME/<LINUX_DATA_DIRECTORY>/mail.sqlite` (default `~/.local/share/...`). The directory, GTK application ID and Secret Service collection come from the [distribution profile](../../docs/distribution-profile.md): `email-client` and `dev.email.client` for the `development` profile, `com.ycorplabs.Mail` for official releases and `invalid.email.Unofficial` for a build that selects no profile. The app data directory is restricted to the current user (`0700`). `EMAIL_STORE_PATH=/path/to/mail.sqlite` selects a disposable store instead, with settings files beside it; a missing directory is created private (`0700`), a symlinked database is refused, and the app then runs under its own per-store application ID, separate from any installed copy. `EMAIL_DEMO=1` adds three labelled demo messages to that store and `EMAIL_DEMO=launch` the [launch demo mailbox](../../docs/demo-mailbox.md), as on macOS; both are ignored without `EMAIL_STORE_PATH`. Otherwise no demo account is inserted. Account credentials go to the system Secret Service keyring; account configuration and mail cache remain in the data directory.

Install GTK 4.10 or newer, WebKitGTK 6, OpenSSL, D-Bus and pkg-config development packages. On Ubuntu 24.04: `sudo apt install build-essential pkg-config libssl-dev libdbus-1-dev libgtk-4-dev libwebkitgtk-6.0-dev`. For a first run with demo data in an isolated store, run `./scripts/dev.sh` from the repository root; [Building and running Email](../../docs/building.md) covers Ubuntu's WebKit sandbox restriction for development binaries. Run `./scripts/run-linux.sh` to use your real mail store. The launcher builds an optimized release binary and passes a locally configured Google desktop OAuth client value when present.

Click **Add account** to connect Gmail, Outlook/Hotmail, iCloud, or another IMAP/SMTP server. Gmail signs in with the configured Email Desktop OAuth client or connects with a Google app password ([guide](../../docs/gmail-app-password.md)); Outlook needs a developer-registered Microsoft client ID until its app registration is packaged. Distribution builds hide Outlook and, until the official profile sets `GOOGLE_OAUTH_PUBLIC=true`, Google sign-in ([account setup](../../docs/account-setup.md)). The system browser opens for authorization. See [cloud provider setup](../../crates/mail-cloud/README.md) for registration details. iCloud Mail needs an app-specific password. Sync runs on a worker thread after connecting; **Sync now** refreshes every account.

The sidebar offers all inboxes, each account, saved split inboxes, labels, snoozed mail, drafts, and outbox. New split inboxes use a query filter. `j`/`k` or arrows move selection; `/` filters the loaded summaries; `e` archives; `m` marks read; `u` marks unread; `l` labels; `z` opens Snooze (or unsnoozes in Snoozed); `i` toggles the remembered remote-image default; `c` composes; `r` starts a reply; `s` syncs; `a` adds an account; `Ctrl+Shift+U` finds an unsubscribe link with a confirmation step; `Ctrl+K` opens commands. The Snooze picker accepts `5` or `5m` for five minutes, `3h`, `1d`, and `1w`, shows the duration before confirmation, and offers presets. Escape leaves search or closes the inline composer. Composer typing does not trigger mail shortcuts. The thread reader lists messages in the conversation, each selectable without leaving the reader.

Compose allows file attachments (up to 20 MiB each, with the service enforcing total/count limits), editable saved drafts, a configurable send-later delay, and a ten-second undo window for immediate sends. The outbox shows scheduled and undoable sends; select one and use **Cancel scheduled send** to restore its draft. The app dispatches due sends while open. For sending while it is closed, install the user timer described in the root README.

The list pages through cached thread summaries in 500-row chunks, automatically requests more near the bottom, and fetches a body only when selected. Attachments load asynchronously; image and text previews stay in memory, while other types (including PDF) require an explicit save. PDF preview is not implemented on Linux because no safe in-app PDF renderer is included yet. HTML passes through the service sanitizer before entering the WebKit view. JavaScript, local storage, WebGL, and external navigation are disabled. Same-message `cid:` images load within strict size limits. Remote images are blocked by default. A notice above the message offers **Load images** for that email; the image control offers the same per-email and default choices as macOS, and `i` toggles the remembered default. Remote images may notify the sender that you opened their email. Per-draft **Track opens** exists only when the build's distribution profile enables `OPEN_TRACKING` (the `development` profile); official and unofficial builds leave it out. Plain text uses a native GTK text view.

The app can be syntax and type checked on a macOS machine without GTK libraries using `DOCS_RS=1 cargo check --manifest-path apps/linux/Cargo.toml`. For a real Linux compile and link on a Mac with Docker, `./scripts/build-linux-container.sh` builds `dist-linux/email-linux-<arch>` in one step (`--test` also runs the tests). For a quick debug compile of your working tree with the same image and download cache (this mounts the whole checkout read-only, including ignored local files):

```sh
./scripts/build-linux-container.sh --image-only
docker run --rm -v "$PWD:/src:ro" -v email-linux-cargo:/cargo -e CARGO_HOME=/cargo \
  -v email-linux-dev-target:/target -e CARGO_TARGET_DIR=/target/linux -w /src \
  "email-linux-dev:$(python3 scripts/check-toolchain.py --print-version)" \
  cargo build --manifest-path apps/linux/Cargo.toml --locked
```

This verifies native linking but does not open a window. Runtime testing requires a Linux display session and an available Secret Service keyring.

On Ubuntu 24.04 or another Debian-based Linux with the build dependencies installed, `./scripts/package-linux-deb.sh` creates `dist-linux/email-linux_<version>_<architecture>.deb`. The package includes the desktop launcher and app icon and detects shared-library dependencies from the linked binary. Install it with `sudo apt install ./dist-linux/email-linux_*.deb`. The package is built for the architecture on which the script runs; the arm64 package built on an Apple Silicon Mac is for arm64 Linux, not x86_64 Linux.

### Release packages for x86_64 and ARM64 from one machine

`./scripts/package-linux-release.sh [aarch64] [x86_64]` (default: both) builds the release artifacts from the committed source at `HEAD` in clean Ubuntu 24.04 containers, one per CPU architecture, using the Rust version pinned in `rust-toolchain.toml`. A foreign architecture runs under Docker's CPU emulation (Rosetta or QEMU on Apple Silicon), so an Apple Silicon Mac can produce the x86_64 packages. For each architecture it writes, next to the other architecture's files in `dist-linux/`: `email-linux-<arch>`, `email-linux_<version>_<arm64|amd64>.deb`, the Arch compatibility package `email-linux-<version>-1-<arch>.pkg.tar.zst` (local alpha only), `mail-mcp-<arch>`, `libmail_service-<arch>.so` and `BUILD-INFO-<arch>.txt`, and it replaces that architecture's lines in `dist-linux/SHA256SUMS`. The packages contain only the app; the MCP server and engine library are separate downloads. They are built on Ubuntu 24.04 and link OpenSSL 3, so on Arch the standalone `mail-mcp` needs `openssl-3`. The selected distribution profile, release channel and OAuth application values are resolved on the host as `package-linux-deb.sh` resolves them and passed into the container by name.

`./scripts/verify-linux-packages.sh [aarch64] [x86_64]` then installs each `.deb` into a clean `ubuntu:24.04` container, and the x86_64 Arch package into a clean `archlinux:base` container, with the distro package manager alone, so the declared dependencies must resolve. It checks the ELF machine and shared libraries, seeds synthetic demo mail through `mail-mcp`, queries it over MCP's stdio protocol, and starts the installed app under Xvfb to capture screenshots in `dist-linux/verify/`. These are emulated container checks, not tests on x86_64 hardware or in a desktop session.

## Arch and Omarchy packages

On an Arch/Omarchy system, install build dependencies with `sudo pacman -S --needed base-devel rust pkgconf gtk4 webkitgtk-6.0 openssl dbus`, then run `./scripts/package-linux-arch.sh` as your normal user. It writes a native `.pkg.tar.zst` into `dist-linux`; install it with `sudo pacman -U ./dist-linux/email-linux-*.pkg.tar.zst`. A working Secret Service provider (such as GNOME Keyring) must be available in the desktop session for credentials. The local PKGBUILD targets x86_64 and aarch64, but each architecture must actually be built and tested on its corresponding Linux environment. This is a local alpha package built from this checkout. The AUR source package is `apps/linux/packaging/aur/PKGBUILD`; see [AUR packaging](../../docs/linux-aur.md).

Both package scripts read the ignored `config/oauth-desktop.local.env` without executing it and embed only the named Google/Microsoft **desktop OAuth application** values at compile time. Exported environment variables take precedence, so you can bring your own client. Profiles that require a Google client (`development`, `official`) fail with a message naming `EMAIL_GOOGLE_CLIENT_ID` and that file when it is missing; the neutral profile builds without one. Packages install the desktop entry, AppStream metadata and icon under the profile's `LINUX_APP_ID`. The Google desktop client value used for token exchange cannot be kept confidential in an installed desktop binary; PKCE is also used. The file itself is not included, and user account credentials/tokens are never read during packaging. A configured package can sign into Gmail without users entering a client ID or copying a configuration file. Environment variables can still override the app configuration for development. Google OAuth test-audience and verification restrictions still apply independently of packaging.

Packages default to `EMAIL_LINUX_RELEASE_CHANNEL=local-alpha`. Distribution
source builds require `EMAIL_LINUX_RELEASE_CHANNEL=distribution` and explicit
`EMAIL_RELEASE_GOOGLE_CLIENT_ID` / `EMAIL_RELEASE_GOOGLE_CLIENT_SECRET` desktop
application configuration. The known development client is rejected, even when
local development configuration exists. Packages include `RELEASE-CHANNEL.txt`
with their notices. This configuration guard does not establish Google approval,
package signing or acceptance. Existing-binary Arch compatibility packaging is
local-alpha only; use source builds for distribution.

Dependency references: [Arch WebKitGTK 6 package](https://archlinux.org/packages/extra/x86_64/webkitgtk-6.0/) and [Arch PKGBUILD specification](https://man.archlinux.org/man/PKGBUILD.5).

For a VM compatibility check before a native Arch build, `scripts/package-linux-arch-binary-container.sh /path/to/already-built-linux-elf aarch64` uses the official Arch container's `makepkg` tooling to wrap an existing ARM64 Linux binary. This path checks the ELF machine against the package architecture, includes only the binary, icon and desktop launcher, and writes a `.pkg.tar.zst` to `dist-linux`. It mounts no account cache, OAuth source config, `.git`, or VM directories. It is explicitly a cross-packaged `local-alpha` artifact: Ubuntu-to-Arch ABI compatibility, Secret Service, Wayland rendering, and shortcuts still require testing on a real Omarchy system. For a release build, prefer the native source PKGBUILD on each target architecture. The container's pacman downloader uses the documented syscall-filter compatibility option because QEMU user emulation cannot install that filter; container isolation, filesystem download isolation, and package-signature verification remain enabled.

For a VM with no usable GPU, `EMAIL_SOFTWARE_RENDERING=1 email-linux` explicitly disables WebKit hardware acceleration while preserving its content sandbox. Normal launches retain acceleration. This is a compatibility/testing option, not a bare-metal performance setting.

### Ubuntu's user-namespace policy

The Debian package installs `/etc/apparmor.d/email-linux`, attached only to
`/usr/bin/email-linux`. Ubuntu 24.04's default restriction otherwise prevents
WebKitGTK's bubblewrap renderer from creating its user namespace. Following
[Ubuntu's release guidance](https://discourse.ubuntu.com/t/ubuntu-24-04-lts-noble-numbat-release-notes/39890),
the compatibility profile gives this executable a named `unconfined` label and
`userns` permission. It does not add filesystem confinement to the application;
it permits WebKit's existing namespace and seccomp sandbox to start. It does not
disable AppArmor, change a global sysctl, or disable WebKit's sandbox.

Package configuration reloads this profile when AppArmor is active and fails if
the parser rejects it. Systems without AppArmor retain the profile for future
service startup. The profile targets AppArmor 4 (Ubuntu 24.04+); test older
AppArmor distributions separately before claiming support.
