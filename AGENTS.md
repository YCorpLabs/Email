# Guide for coding agents and contributors

This file is read by coding agents (Claude Code, Codex and others) and is also
a short guide for people. It applies to everyone working in this repository.
How to propose a change is in [CONTRIBUTING.md](CONTRIBUTING.md); report
vulnerabilities privately as described in [SECURITY.md](SECURITY.md).

## Start here

- Build and run with demo data, no account or signing setup:
  `./scripts/dev.sh` (macOS or a Linux desktop). Linux binaries from any
  computer with Docker: `./scripts/build-linux-container.sh`.
- Requirements, tests, the source map and common errors:
  [docs/building.md](docs/building.md). All documentation:
  [docs/README.md](docs/README.md).
- Rust is pinned in `rust-toolchain.toml`; rustup installs it on first use.

## Checks before you hand back work

- `cargo test --workspace --locked` for any engine or MCP change.
- Linux shell changes: `cargo test --manifest-path apps/linux/Cargo.toml --locked`,
  or `./scripts/build-linux-container.sh --test` from macOS.
- macOS shell changes: `./scripts/dev.sh --smoke` and
  `python3 scripts/check-release-swift.py`.
- Scripts and repository hygiene: `python3 scripts/check-release-readiness.py`.
- Report what you ran and what you did not. A successful compile is not a
  visual or behavioral check.

## Where things live

- `crates/mail-core`: mail model, SQLite cache, search, drafts, outbox.
- `crates/mail-imap`, `crates/mail-cloud`: provider adapters (IMAP/SMTP;
  Gmail and Microsoft Graph).
- `crates/mail-service`: accounts, credentials, sync and the JSON API that
  both apps use ([docs/service-api.md](docs/service-api.md)).
- `crates/mail-mcp`: local MCP server ([docs/mcp.md](docs/mcp.md)).
- `apps/macos`: AppKit app (SwiftPM). `apps/linux`: GTK 4 app (its own Cargo
  workspace and lockfile).

## Working safely

- Run development builds against demo data or a disposable
  `EMAIL_STORE_PATH`. Do not open someone's real mail store, connect a real
  account, send email or grant MCP write access unless they asked for it.
- Builds read a distribution profile (`docs/distribution-profile.md`). Without
  a selection they use the neutral `unofficial` profile; never commit
  `config/distribution-profile.local` or switch a checkout's profile casually.
- Never commit credentials, OAuth client secrets, signing material or mail
  databases. Keep local configuration in `config/` files named `*.local` or
  `*.local.*`, which Git ignores. `scripts/check-release-readiness.py` also
  rejects any of them that is force-added.
- Treat mail content as untrusted. HTML is sanitized by `mail-service` and
  shown with scripts and navigation disabled. Keep it that way.
- Build with `--locked` and keep `Cargo.lock`, `apps/linux/Cargo.lock` and
  `apps/macos/Package.resolved` consistent with manifest changes.
- `rustfmt` is not applied repository-wide. Do not reformat code you are not
  otherwise changing.
- Shared reader, composer, navigation, palette and settings changes follow the
  Desktop UI parity rule below. Update both apps or name the remaining gap.

## Desktop UI parity

macOS is the design and interaction reference for shared Email UI. The native
shells are separate: `apps/macos/Sources/EmailUI` and `apps/linux/src`. A
shared reader, composer, navigation, palette or settings change must be checked
in both implementations. Do not finish a change on one platform and describe it
as an app-wide fix.

For each shared UI change:
- Identify and update both implementations, or record the concrete remaining gap.
- Compare the same relevant state, including keyboard focus and dismissal; check
  narrow layouts when the affected component depends on available width.
- Build the affected shells. When you check an installed package, confirm that
  the running executable matches the one you built, then inspect the changed
  behavior.
- Record what was actually verified in the relevant feature document. A
  successful compile or package install alone is not visual or behavioral
  parity. Report unverified work plainly.

OS window chrome, modifier conventions and compositor-provided blur can differ;
these exceptions do not excuse inconsistent application layout or behavior.

## Running the macOS app

- Launch the development build with `scripts/open-macos.sh`, not `open -a Email`
  or a release bundle. The script checks which build it opens.
- Keep the local signing identity stable. Don't change Keychain access
  settings, remove credentials or weaken trust to get rid of prompts.
- After changing credential or MCP code, rebuild and restart any MCP server
  processes that are still running.

## Related services

Scheduling (the hosted booking service) and the ycorplabs.com website are
separate projects; do not add their implementations here. Email talks to
Scheduling only through the contract pinned in `contracts/scheduling.json`, and
a change to that contract needs a matching Scheduling release.
