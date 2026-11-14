# Email by YCorp

A fast, keyboard-first email client for macOS and Linux. Your mail lives on
your own computer and search is instant.

Both apps are fully native (AppKit on macOS, GTK 4 on Linux) and share one Rust
engine. It talks straight to your mail provider and keeps a local SQLite cache.
If you want your own AI agents to read your mail, there's an optional local MCP
server, and you choose what it can do.

> [!WARNING]
> **It's a beta.** Gmail on macOS is what's been tested most; other providers
> and plenty of edge cases still need real-world testing. Keep important mail
> on your provider, and expect things to break now and then.

## Features

- Fast keyboard navigation: `j`/`k`, `m`/`u` read state, `e` archive, `/`
  search, `⌘K`/`Ctrl+K` command palette, and a reader mode with Vim-style
  scrolling.
- Gmail, iCloud and any IMAP/SMTP account. Microsoft accounts are in the
  engine but untested, so they're off for now.
- Full-text search of downloaded mail, labels, saved split inboxes, snooze,
  follow-ups, snippets and signatures.
- Inline reply that keeps the conversation visible, attachments, and scheduled
  send with undo. Undo cannot recall mail after the provider accepts it.
- A confirmable unsubscribe action (`⌘U` on macOS, `Ctrl+Shift+U` on Linux).
- A calendar view for Google calendars. It needs Google sign-in, which the
  beta builds don't have yet.

## Build from source

Builds from source use the `unofficial`
[distribution profile](docs/distribution-profile.md): their own app ID,
Keychain items and data folder, so they never clash with an official install.
They come without an OAuth client, support address, Scheduling service or
update feed until you add your own.

### Quick start

```sh
./scripts/dev.sh
```

This builds the app for macOS or a Linux desktop and opens it with three demo
messages in an isolated store, `dist/demo/mail.sqlite`. It needs no signing
identity, OAuth client or mail account, and never opens a real mail store. From
any computer with Docker, `./scripts/build-linux-container.sh` builds a Linux
binary. [Building and running Email](docs/building.md) covers requirements,
tests, the source layout and common errors.

### macOS signed app

Requires macOS 13 or later, the Xcode command line tools, Rust (via
[rustup](https://rustup.rs)) and OpenSSL 3 on your `PATH`, for example from
Homebrew. The system LibreSSL cannot create the signing bundle.

```sh
./scripts/setup-macos-signing.sh        # once: a local code-signing identity
# In Keychain Access, rename the imported private key to
# "Email Local Development Signing Key", as the script instructs.
./scripts/authorize-macos-signing.sh    # once, in your own Terminal
./scripts/run-macos.sh                  # build dist/Email.app and open it
```

`./scripts/run-macos.sh --package` builds without launching. Keep
`dist/Email.app` and `dist/Runtime` together. To try the app against a
disposable cache, set `EMAIL_STORE_PATH` to a database in its own directory,
such as `/absolute/path/test/mail.sqlite`. Add `EMAIL_DEMO=1` for demo messages,
or `EMAIL_DEMO=launch` for the [launch demo mailbox](docs/demo-mailbox.md);
both are ignored without `EMAIL_STORE_PATH`.

### Linux

Install the GTK 4, WebKitGTK 6 and pkg-config development packages and Rust.
Then run `./scripts/run-linux.sh`, or build a package with
`./scripts/package-linux-deb.sh`. See [apps/linux/README.md](apps/linux/README.md)
for dependencies, the verification container and Arch packaging.

### Bring your own OAuth client

iCloud and IMAP accounts work without any registration: use an app-specific or
provider-issued app password. Browser sign-in for Gmail needs your own Google
desktop OAuth client. Never commit its values; put them in the ignored
`config/oauth-desktop.local.env`:

```sh
EMAIL_GOOGLE_CLIENT_ID=<your client ID>.apps.googleusercontent.com
EMAIL_GOOGLE_CLIENT_SECRET=<its desktop client secret>
```

Exported environment variables override this file. [FORKING.md](FORKING.md)
explains how to register the client, which APIs and scopes to enable, and the
limits of Google's Testing mode and personal-use exemption.

### Checks

```sh
cargo test --workspace --locked
./scripts/dev.sh --smoke                    # macOS headless engine check
./scripts/build-linux-container.sh --test   # Linux tests from any OS
python3 scripts/check-release-swift.py
python3 scripts/validate-local.py --profile source --gitleaks /path/to/gitleaks
```

## Privacy and security model

- **Local-first:** mail, drafts and the search index live in a local SQLite
  database in a directory only your user can read. Email talks directly to your
  provider.
- **Credentials:** passwords and OAuth tokens are stored in the operating
  system's vault, the macOS Keychain or the Linux Secret Service. Account
  configuration files hold server names, usernames and the OAuth client used,
  including a Google desktop client's token-exchange value, which desktop apps
  cannot keep confidential.
- **Safe rendering:** message HTML is sanitized and shown with JavaScript
  disabled, navigation blocked and a restrictive content security policy.
  Remote images are blocked by default: the reader says so above the message
  and offers **Load images** for that email, while `i` or the image control
  changes the remembered default. Loading them can tell a sender when you
  opened a message. Bounded `cid:` images from the same message still render.
- **Bounded input:** IMAP messages, attachments, Gmail text decoding and sync
  cursors use size limits and staged storage, so malformed or oversized mail
  cannot stall sync or exhaust memory.
- **Other network requests:**
  - Scheduling and booking links contact the Scheduling service named by the
    build's profile; builds without one hide them. Per-draft open tracking
    exists only in development-profile builds; official and unofficial builds
    compile it out ([reader privacy](docs/reader-privacy.md));
  - Google contact photos are opt-in;
  - the macOS app looks up published BIMI logos for organization senders by
    DNS and HTTPS, whatever the remote-image setting;
  - signed official macOS releases check for updates with your permission.
- **No telemetry:** no analytics and no automatic crash reports.

See [SECURITY.md](SECURITY.md) to report a vulnerability.

## MCP permission model

Email includes a local MCP server so AI tools such as Claude Code and Codex
can work with your mail. In the app, **Agent access** connects them
(**Connect**) and decides what they may do.

- It is a **local stdio process** started by your client, not a network
  service.
- **Every account is off** until you turn it on in Agent access: Read only
  by default, or Read and organize, or Read, organize and write.
- **Sends and bulk changes wait for you**: they appear under Agent requests
  in the main window, showing the email as it will go out, and happen only
  when you approve them, with the usual undo window.
- Mail content reaches agents labelled as untrusted.

From a source checkout, `./scripts/connect-mcp.sh` registers the server
instead. Its registration is read-only by default; `--allow-draft`,
`--allow-triage`, `--allow-sync`, `--allow-send` (or `--allow-write`) widen it,
and `--account` limits it to specific accounts. The registration's flags and
the in-app permissions both apply. Scheduling settings need the separate
`--allow-booking-settings` and browser approval.

See [docs/mcp.md](docs/mcp.md) for the tool list and limits.

## Architecture

- `crates/mail-core`: accounts, messages, threads, SQLite cache, full-text
  search, drafts and queued actions.
- `crates/mail-imap`: IMAP sync, MIME parsing and SMTP send.
- `crates/mail-cloud`: Gmail API, Microsoft Graph, OAuth with PKCE.
- `crates/mail-service`: account configuration, credential storage, provider
  coordination, safe HTML rendering and the C ABI used by the macOS shell.
- `crates/mail-mcp`: the local MCP server and scheduled-send dispatcher.
- `crates/distribution-profile`: the build-time distribution profile parser.
- `apps/macos` and `apps/linux`: the native desktop shells.

## Contributing, support and forks

- [CONTRIBUTING.md](CONTRIBUTING.md): how to contribute.
- [docs/building.md](docs/building.md) and the [documentation index](docs/README.md):
  building, testing, demo mode and how the engine fits together.
- [SUPPORT.md](SUPPORT.md): where to ask questions and report bugs.
- [FORKING.md](FORKING.md): ship your own build with your own name, OAuth
  clients, support channels and update feed.

## License and trademarks

MIT, see [LICENSE](LICENSE). The YCorp name and logo aren't part of that; see
[TRADEMARKS.md](TRADEMARKS.md). Dependencies keep their own licenses
([THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)).
