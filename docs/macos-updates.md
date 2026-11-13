# macOS automatic updates

Email for macOS updates itself with [Sparkle](https://sparkle-project.org)
2.10.1, pinned exactly in `apps/macos/Package.swift` and `Package.resolved`.
Release tooling downloads the matching Sparkle command line tools and checks
their SHA-256 (`scripts/macos-updates-env.sh`).

Publishing a production update needs the items listed under
[Production requirements](#production-requirements).

## How it fits the app

| Piece | Role |
| --- | --- |
| `Sources/EmailUpdatePolicy` | Pure rules: feed URL, key, build number, Sparkle settings, channel, item gate, draft guard. |
| `Sources/EmailUpdater` → `libEmailUpdater.dylib` | Links Sparkle. C ABI: status, install into menu, fixture install. |
| `Sources/EmailUI/EmailUpdaterLoader.swift` | Loads the module only from a signed release bundle, after signature checks. |
| `Contents/Frameworks/Sparkle.framework` | Embedded by `scripts/package-macos-release.sh`, without sandbox XPC services. |

EmailUI never links Sparkle. The development app (`dist/Email.app`) and the
Linux shell are unaffected and never self-update.

### Fail-closed loading

`EmailUpdaterLoader` loads nothing unless `EmailReleaseBuild` is set and both
`libEmailUpdater.dylib` and `Sparkle.framework` exist. Before `dlopen` it
requires both to validate strictly (Sparkle including nested code) against
the exact leaf certificate that signed the app. A modified Sparkle binary or
module disables updates (`module-signature-invalid`).

The module then reads the Info.plist bound into the app's code signature, not
the file on disk. A modified Info.plist disables updates (`signature-invalid`).
Ad hoc signed bundles are refused.

### Configuration rules

Updates run only when the signed Info.plist has all of:

- `SUFeedURL` using HTTPS on a host listed in `EmailUpdateAllowedHosts`, default port, no credentials.
  The packager writes that array from the distribution profile's
  `UPDATE_ALLOWED_HOSTS` (official: `ycorplabs.com`, `updates.ycorplabs.com`).
  An absent or empty list disables updates, so a fork never follows another
  publisher's feed.
- `SUPublicEDKey`: base64 of 32 nonzero bytes.
- `CFBundleVersion`: a positive integer without leading zeros.
- `SUVerifyUpdateBeforeExtraction = YES`, `SURequireSignedFeed = YES`.
- `SUSignedFeedFailureExpirationInterval = 0`. Sparkle's default (20 days)
  would start offering updates again after a feed signature failure; zero
  keeps that failure permanent.
- No `SUEnableAutomaticChecks = YES`, `SUAutomaticallyUpdate = YES`,
  sandbox XPC services, release-notes JavaScript or system profiling.

No `SUFeedURL` means updates are off: no menu items, no Sparkle start, no
network request. That is the state of every build made without
`EMAIL_UPDATE_FEED_URL`, including local alphas.

The packager validates the generated Info.plist with the same Swift rules
(`email-update-policy-checks --validate-info`). `--distribute` refuses to
build without a valid production feed.

### Every offered item is gated again

`updater(_:shouldProceedWithUpdate:updateCheck:)` throws unless the feed
signature validated, the item's `sparkle:version` is an integer greater than
the running build, and the item's channel is selected. Sparkle independently
verifies the archive's EdDSA signature before extraction and checks that the
new app satisfies the running app's designated requirement. The feed URL is
always taken from the signed Info.plist; values in user defaults are cleared.

### Channels

- **Stable**: items without `<sparkle:channel>`.
- **Testing**: items with `<sparkle:channel>testing</sparkle:channel>`, plus stable.

Builds packaged with `EMAIL_UPDATE_CHANNEL=testing` start on testing. Users
switch with **Email › Updates › Testing Builds**. Sparkle never installs an
older build, so leaving testing waits for a newer stable build.

### User controls and consent

App menu:

- **Check for Updates…**
- **Updates ›** Check Automatically, Install Automatically, Testing Builds

Sparkle asks for permission to check automatically on the second launch.
Nothing checks in the background before that. Silent install is off until the
user enables Install Automatically.

The updater delegate returns an empty system-profile allowlist, including if
an old stored preference opts into profiling. Update hosting still receives the
ordinary network IP address and HTTP request/user-agent information.

### No forced restart with an open draft

When Sparkle is ready to restart, `shouldPostponeRelaunchForUpdate` asks
EmailUI whether a composer is open. If so, Email shows "Update ready. Email
restarts when you close your draft." and waits. Every two seconds it rechecks;
after the composer closes, the install continues. Updates downloaded in the
background install when the user quits, which is the user's own action.

## Building

```sh
# Unconfigured (updates off), as before:
./scripts/package-macos-release.sh --local-alpha

# Configured. The public key is not secret; the private key never enters Git.
export EMAIL_DISTRIBUTION_PROFILE=official
# Until the official profile records UPDATE_FEED_URL and UPDATE_PUBLIC_ED_KEY:
export EMAIL_UPDATE_FEED_URL=https://updates.ycorplabs.com/email/macos/appcast.xml
export EMAIL_UPDATE_PUBLIC_ED_KEY='<SUPublicEDKey>'
export EMAIL_UPDATE_CHANNEL=stable          # or testing
export EMAIL_VERSION=0.2.0 EMAIL_BUILD_NUMBER=102
./scripts/package-macos-release.sh --distribute
EMAIL_VERIFY_REQUIRE_UPDATES=1 ./scripts/verify-macos-release.sh dist-release/distribution/Email.app
```

Each build also writes `Email-<version>-<build>-<arch>.zip`, the Sparkle
update archive (for distribution, of the stapled app).

Signing order follows Sparkle's documentation, inside out and without
`--deep`: `Autoupdate`, `Updater.app`, `Sparkle.framework`, Email's libraries
and helper, then the app. Distribution adds hardened runtime and timestamps.

`--fixture` builds `invalid.email.update-fixture` with the neutral profile, for tests only. It must
sign from a disposable keychain file and may use a `http://127.0.0.1` feed.
The updater refuses fixture settings under any other bundle identifier.

When `EMAIL_RELEASE_CODESIGN_KEYCHAIN` names a keychain file (fixture and beta
builds), the packager passes it to codesign with `--keychain` and also runs
codesign with `HOME` set to a directory inside its staging folder. That
directory holds only a keychain search list: the signing keychain, then the
user's current search list. Before macOS 27, codesign finds an identity only in
a keychain on the search list; `--keychain` only narrows that search, and
macOS 26 reports "no identity found" without the private list. The private list
gives codesign the keychain without changing the user's search list or login
keychain. `email_codesign_home` in `scripts/macos-updates-env.sh` writes it.

## Publishing

```sh
EMAIL_UPDATE_ED_PRIVATE_KEY="$(<read from the release secret store>)" \
./scripts/publish-macos-update.sh \
  --archive dist-release/distribution/Email-0.2.0-102-arm64.zip \
  --channel stable \
  --feed-dir ../email-update-feed \
  --download-prefix https://updates.ycorplabs.com/email/macos/ \
  --ed-key-file -
```

The script refuses to stage a build unless it:

- comes from a distribution profile (`DISTRIBUTION=true`), has that profile's
  bundle ID (`com.ycorplabs.mail` for `official`) and passes the update policy,
- uses a download host from that profile's `UPDATE_ALLOWED_HOSTS`,
- is Developer ID signed, stapled and accepted by Gatekeeper,
- has a build number greater than every build already in the feed,
- and, for stable, was packaged as a stable build.

It runs Sparkle's `generate_appcast` with an explicit key (never the login
Keychain), which signs both the archive and the feed, then verifies the
result with `scripts/verify-macos-appcast.py` using only the public key. It
does not upload. Copy the verified directory to the host as a reviewed step.

`verify-macos-appcast.py` can be rerun in CI or before upload: feed signature,
integer and unique builds, channels, HTTPS approved hosts, archive lengths and
archive EdDSA signatures.

## Tests

```sh
./scripts/test-macos-updates.sh
swift build --package-path apps/macos --disable-sandbox --product email-update-policy-checks
"$(swift build --package-path apps/macos --disable-sandbox --show-bin-path)/email-update-policy-checks"
```

`test-macos-updates.sh` creates a disposable code signing identity in a
temporary keychain file (the keychain search list is checked unchanged),
checks that codesign can sign with it, and creates a disposable EdDSA key.
It runs on macOS 26 and later; CI runs it on `macos-26` and `macos-26-intel`.
On macOS 15, codesign refuses the self-signed fixture identity even through the
private search list ("The specified item could not be found in the keychain"),
so the test stops at that first check.

It packages four complete fixture bundles, publishes a signed appcast with the
real publishing script, serves it on loopback and drives the real Sparkle
pipeline inside the shipped bundle through a fixture-only non-interactive
driver. The fixture uses Sparkle's normal relaunch path; a bare
fixture relaunch reports updater status and exits before mailbox
initialization, and both successful update paths require a normal zero exit
status. Fixture runs write Sparkle defaults and caches under the fixture bundle
ID; the test removes them.

It checks:

- Packages A=100, B=101 stable, C=102 testing, and N without a feed.
- The release verifier on A and N (signatures, Sparkle signer, rpaths, keys,
  modified Sparkle, modified Info.plist).
- No feed: status `not-configured`, install refused.
- The default channel follows the packaged channel.
- No automatic-check consent is recorded without the user.
- The publisher stages a signed feed with stable and testing items.
- The publisher refuses an older build, a testing build on stable, and a
  fixture bundle for production, and keeps its last verified output when
  publication fails.
- Tampered archive (one byte, same length): "The update is improperly signed",
  app unchanged.
- Tampered feed (build changed, not re-signed): "The update feed is improperly
  signed", app unchanged.
- Feed with the signature block removed: refused, app unchanged.
- Stable A=100 to B=101 in place; signature valid, updater status and mail
  service run.
- B finds no newer stable item.
- Testing channel A=100 to C=102.
- Fixture settings re-signed under the official bundle ID
  (`com.ycorplabs.mail`): `fixture-not-allowed`.

`email-update-policy-checks` covers allowed hosts, schemes, ports, keys, build
numbers, Sparkle keys, fixture rules, channels, the item gate and the draft
guard. The interactive Sparkle windows and the open-draft postponement in a
running GUI session are not covered (only the policy decision is tested), and
neither is the distribution packaging path, which needs Apple credentials.

## Linux and platform gap

This is macOS-only. Linux packages (`.deb`, Arch) update through the system
package manager and have no in-app updater, update menu, channel switch or
draft-aware restart. There is no shared UI change and no parity claim. A
Linux updater would need its own design (repository signing, channels).

## Production requirements

The publisher signs and verifies in a sibling staging directory before replacing
its previous local output. A per-feed directory lock serializes publishers.
Failed signing or validation leaves the last verified output intact. After a
crash, review any empty `.publish-lock` directory before manually removing it.
This stages files only; uploading the verified result remains a separate step.

A production update needs:

1. Apple Developer ID Application certificate and notarytool profile for
   Y Corp AS. The publisher refuses non-Developer ID archives.
2. Production EdDSA key pair, generated and stored with maintainer approval in
   the release secret store. Only the public key goes into the build.
   If a protected local working copy is needed, use the ignored
   `config/macos-update-signing.local/` directory, mode 0700 with mode 0600
   files. Source preflight rejects even force-added files from this directory.
   A local copy is not an off-device recovery backup. The fixture tests never
   create a production key.
3. HTTPS hosting at `updates.ycorplabs.com` (or `ycorplabs.com`) for the
   appcast and archives.
4. A real Developer ID A-to-B update with Gatekeeper and quarantine, and the
   interactive Sparkle UI (permission prompt, update window, draft notice),
   checked by hand on a clean Mac. The tests here use the fixture driver.
5. Release CI that runs the commands above on a clean runner with secrets.
