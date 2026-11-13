# Distribution profiles

A distribution profile is the one place that says which build is the official
one. It names the app identity and the services a build may contact. Every
build reads exactly one profile, at build time. A build that selects nothing
gets the neutral `unofficial` profile, which cannot collide with or impersonate
an official app.

Profiles live in `config/profiles/<name>.env`. Each is a strict `KEY=value`
file: no quotes, no shell expansion, no `export`, every key present (empty when
unused), unknown or repeated keys refused. The same rules are implemented twice
and tested together: `scripts/distribution-profile.py` for build scripts and
`crates/distribution-profile` for Cargo build scripts.

## Selecting a profile

1. `EMAIL_DISTRIBUTION_PROFILE`, a profile name or a `.env` path; otherwise
2. the ignored file `config/distribution-profile.local`, containing one name or
   path; otherwise
3. `unofficial`.

Build scripts export the resolved absolute path, so the Rust engine, the Linux
shell and the scripts always read the same file. Check a selection with
`python3 scripts/distribution-profile.py get MACOS_BUNDLE_ID`.

## Profiles in this repository

| | `development` | `official` | `unofficial` (default) |
| --- | --- | --- | --- |
| Use | Maintainers' dev build and local alpha | Signed production releases | Self-builds, forks, validation |
| `DISTRIBUTION` | `false` | `true` | `false` |
| macOS bundle ID | `dev.email.client` | `com.ycorplabs.mail` | `invalid.email.unofficial` |
| macOS data directory | `Email` | `com.ycorplabs.mail` | `invalid.email.unofficial` |
| Keychain namespace | `email.native.mail` | `com.ycorplabs.mail` | `invalid.email.unofficial` |
| Linux app ID | `dev.email.client` | `com.ycorplabs.Mail` | `invalid.email.Unofficial` |
| Linux data directory | `email-client` | `com.ycorplabs.Mail` | `invalid.email.Unofficial` |
| Secret Service collection | `Email` | `com.ycorplabs.Mail` | `invalid.email.Unofficial` |
| Linux dispatcher unit | `email-dispatch` | `com.ycorplabs.Mail.dispatch` | `invalid.email.Unofficial.dispatch` |
| MCP registration name | `email` | `email` | `email-unofficial` |
| Support and feedback | official addresses | official addresses | none |
| Scheduling origin | official service | official service | none |
| Update hosts | none | `ycorplabs.com`, `updates.ycorplabs.com` | none |
| `OPEN_TRACKING` (Track opens) | `true` | `false` | `false` |
| Google client ID required | yes | yes | no |
| `GOOGLE_OAUTH_PUBLIC` (one-click Google sign-in, see below) | `true` | `false` | `true` |

The local alpha (`scripts/package-macos-release.sh --local-alpha`) appends
`.release-alpha` to the selected profile's bundle ID. With `development` it
becomes `dev.email.client.release-alpha`, using the development Keychain namespace
and data directory as before. Update-fixture bundles always use the neutral
profile and the test-only bundle ID `invalid.email.update-fixture`.

`.invalid` is reserved (RFC 2606) and can never belong to a publisher, so the
neutral identifiers cannot collide with any released app.

## Official production identity

The production macOS bundle ID is `com.ycorplabs.mail`. It is deliberately
neutral, so it survives a product rename.
The Linux app ID uses the same reverse-DNS base with Flathub's casing rules:
the domain portion is lowercase and the application component is `Mail`, as in
`com.ycorplabs.Mail`. The desktop entry, AppStream metadata, icon name and GTK
application ID all use it, as a future Flathub ID must.

Derived production identifiers:

- launchd dispatcher label: `com.ycorplabs.mail.dispatch`;
- Keychain services: `com.ycorplabs.mail.v4` on macOS and
  `com.ycorplabs.mail.v2` on Linux, which is new because no production build
  has shipped;
- Sparkle defaults domain and update-policy bundle check: `com.ycorplabs.mail`;
- `--distribute` packaging and production publishing require this bundle ID.

The bundle ID cannot change after release without breaking Keychain access,
stored data and Sparkle updates. No OAuth redirect depends on it: Google uses a
`127.0.0.1` loopback redirect and Microsoft `http://localhost:<port>/callback`.

## Development identity

The `development` profile keeps the Keychain namespace and data paths that
existing development builds use. A checkout that needs a different bundle ID,
for example to keep an older signed development build, keeps a copy of the
profile outside the catalog and selects it by path:

```sh
sed 's/^MACOS_BUNDLE_ID=.*/MACOS_BUNDLE_ID=your.bundle.id/' config/profiles/development.env > config/development.local.env
echo config/development.local.env > config/distribution-profile.local
```

Both files are ignored by Git. Copy the profile again when
`config/profiles/development.env` changes. Guards:

- `scripts/run-macos.sh` and `scripts/build-mcp-macos.sh` refuse to package or
  sign when `dist/Email.app` has a different bundle ID than the selected profile;
- tests assert the development values in both parsers;
- the launcher-cache tests run with the `development` profile and require the
  exact `dev.email.client` designated requirement.

`GOOGLE_OAUTH_PUBLIC` is the one-click Google sign-in gate
([account setup](account-setup.md)). `crates/mail-cloud/build.rs` reads it from
the selected profile. When it is false, account setup offers Gmail only through
an app password and the engine refuses to start Google sign-in; accounts already
connected through Google sign-in keep working. The official profile keeps it
false until Google's restricted-scope review passes. `EMAIL_GOOGLE_OAUTH_PUBLIC=true`
or `false` overrides it for one build. A `distribution`-channel build whose profile is not for distribution
keeps it off.

## Credentials are not profile values

OAuth client IDs and the Google desktop token-exchange value come from the
environment or the ignored `config/oauth-desktop.local.env`, with the
environment winning. Google's OAuth policies say not to commit client
credentials to public repositories. Profiles with
`GOOGLE_CLIENT_ID_REQUIRED=true` fail to build without
`EMAIL_GOOGLE_CLIENT_ID` and name that file and variable. Distribution builds
use `EMAIL_RELEASE_GOOGLE_CLIENT_ID` and `EMAIL_RELEASE_GOOGLE_CLIENT_SECRET`,
and refuse the development client by its SHA-256 digest. The development
profile itself accepts only that client, because its stored tokens and signed
Info.plist depend on it.

## What reads the profile

- **Rust engine** (`crates/mail-service/src/profile.rs`): Keychain and Secret
  Service names, data directory names, the Scheduling origin and the
  tracking-pixel origin derived from it, whether open tracking is compiled in
  (`OPEN_TRACKING`), support and feedback addresses. The
  MCP server uses the same engine.
- **macOS shell**: loads the engine's profile through
  `mail_service_distribution_profile_json` (`DistributionProfile.swift`). It
  keeps no defaults; without a loadable engine there is no support channel,
  Scheduling or data directory.
- **Linux shell**: `mail_service::profile`.
- **Scripts**: bundle ID, update hosts and feed, dispatcher labels,
  MCP registration names, desktop entry names.

Unset values remove features rather than falling back:

- no support addresses: the chooser explains that the build has none;
- no Scheduling origin: Bookings, booking links, Track opens and Recent opens
  are hidden in both shells, and the engine refuses Scheduling requests;
- no update hosts: the macOS updater stays disabled and packaging refuses a
  feed URL;
- `OPEN_TRACKING=false`: the engine compiles the tracking pixel out and both
  shells hide Track opens; saved activity for already-tracked sent mail stays
  readable. The parsers refuse `true` in a distribution profile or without a
  Scheduling origin. See [reader privacy](reader-privacy.md).

## Build safeguards

- Local builds (`run-macos.sh`, `run-linux.sh`, local-alpha packages) refuse
  distribution profiles. Those would share an installed release's data and
  Keychain items.
- `package-macos-release.sh` builds the engine in `target/package-<channel>`,
  so packaging never replaces `target/release/mail-mcp`, the helper that
  `connect-mcp.sh` registers.
- `connect-mcp.sh` rebuilds an existing helper that was compiled for another
  profile.
- A feed or update key recorded in the profile cannot be overridden from the
  environment. Publishing refuses an archive whose key differs.

## Known gaps

- The `official` and `development` profiles both register MCP as `email`. On a
  Mac with both, `connect-mcp.sh` keeps the existing registration; use `--name`
  for the second one.

- Account setup without a Google client differs between the shells. macOS says
  sign-in isn't configured in this build. Linux shows a field for an OAuth
  client ID, as it already did for unconfigured Microsoft sign-in. The separate
  account-setup change owns that dialog.
- A checkout that has `config/oauth-desktop.local.env` but no selection fails
  to build, in Cargo and in every script, until it selects a profile. It never
  silently builds the neutral identity there.

Forks: see [FORKING.md](../FORKING.md).
