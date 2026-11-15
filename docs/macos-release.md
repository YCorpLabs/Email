# macOS distribution builds

The development app in `dist/Email.app` deliberately keeps its stable launcher and sibling `Runtime` directory. Do not give that layout to end users. The separate release packager builds a complete app with its Rust engine and Swift interface in `Contents/Frameworks`, plus the read-only-by-default MCP executable in `Contents/Helpers/mail-mcp`.

The project is MIT licensed. Packaging requires nonempty root `LICENSE` and
`THIRD_PARTY_NOTICES.md` files and includes both in `Contents/Resources/Legal`
before signing. The release verifier rejects bundles missing either notice.
Third-party components retain their own licenses; these files do not replace
the dependency redistribution review required before release.

## Local alpha

```sh
./scripts/package-macos-release.sh --local-alpha
./scripts/verify-macos-release.sh
```

Output is `dist-release/local-alpha/Email.app` and a DMG named for the version and CPU architecture. The local alpha uses the existing local signing identity but a separate bundle identifier: the selected [distribution profile](distribution-profile.md)'s bundle ID plus `.release-alpha`. With the `development` profile that is `dev.email.client.release-alpha`. It does not replace the development app or reuse its account-token permissions. No certificate trust or Keychain access settings are changed by packaging. The alpha is **not Apple notarized and is not a public release**. Gatekeeper may refuse it on another Mac. Verification uses temporary empty databases rather than a real mailbox.

The verifier relocates the app outside the repository, verifies signatures, runs the service smoke check, supplies an invalid library environment override, rejects missing/tampered runtime copies, and starts the bundled MCP executable's help command. It does not grant notifications, contacts, or mail permissions and does not send any mail.

The distribution launcher accepts only its signed bundled libraries and checks they meet its own designated signing requirement. `CoreBridge` fails closed if a release library is missing; it does not search the environment or current directory. Public builds enable hardened runtime without disabling library validation. Locally signed alphas use the same explicit signature checks without claiming Developer ID-only hardened runtime compatibility.

## Production bundle ID

Official production builds use bundle ID `com.ycorplabs.mail` from
`config/profiles/official.env`. It is deliberately neutral, so it survives a
product rename. Related identifiers derive from it: the launchd dispatcher
label `com.ycorplabs.mail.dispatch`, the Keychain services
`com.ycorplabs.mail.v*` (new, as no production build has shipped), the data
directory `~/Library/Application Support/com.ycorplabs.mail`, and the update
policy's bundle check. Linux uses `com.ycorplabs.Mail`. The development build
(`dev.email.client`) and the local alpha (`dev.email.client.release-alpha`) use
their own identifiers. No OAuth redirect depends on the bundle ID: both providers use
loopback redirects. The bundle ID cannot change after release without breaking
Keychain access, stored data and Sparkle updates.

## Public distribution

Required inputs are a distribution profile (`official`, or a fork's own profile with `DISTRIBUTION=true`), an Apple Developer ID Application signing identity, an existing notarytool credential profile, and a **separate production Google desktop OAuth registration**. The packager refuses non-distribution profiles. It rejects the development Google client by its SHA-256 digest, so the development client ID no longer appears in source. Finish Google's branding and required scope verification before admitting public users. The desktop token-exchange value is packaged like the macOS development build; desktop applications cannot treat that value as confidential, and OAuth uses PKCE.

```sh
export EMAIL_DISTRIBUTION_PROFILE=official
export EMAIL_RELEASE_CODESIGN_IDENTITY='Developer ID Application: Your Organization (TEAMID)'
export EMAIL_NOTARY_PROFILE='email-release'
export EMAIL_RELEASE_GOOGLE_CLIENT_ID='your-production-desktop-client-id'
# Supply EMAIL_RELEASE_GOOGLE_CLIENT_SECRET through your release environment.
export EMAIL_VERSION='0.1.0'
export EMAIL_BUILD_NUMBER='1'
# Google sign-in stays off until Google's review passes (GOOGLE_OAUTH_PUBLIC=false
# in config/profiles/official.env; docs/account-setup.md). Keep
# EMAIL_GOOGLE_OAUTH_PUBLIC unset here.
./scripts/package-macos-release.sh --distribute
./scripts/verify-macos-release.sh dist-release/distribution/Email.app
```

The script signs nested code before the app, checks for an Apple Developer ID certificate requirement, submits the app and DMG to Apple's notary service, staples both tickets, checks the app with Gatekeeper, and emits a SHA-256 checksum. A failure stops the script. A local alpha never gets a successful distribution status marker.

The script packages **the architecture it is built on**. Intel and Apple Silicon need their own builds and hardware tests, or a separately engineered universal-binary build. Use a clean release environment and preserve signing credentials outside source control.

These artifacts are installer preparation, not a finished release. [Signed automatic updates](macos-updates.md) also need production signing and update hosting. Do not publish the local-alpha artifacts as a release.

## Unnotarized beta

Until the project has an Apple Developer ID, betas are built with `--beta`. It
builds the official app, with the `official` profile, bundle ID
`com.ycorplabs.mail` and the production Google desktop client, and signs it with
the project's self-signed beta certificate instead of a Developer ID. It is not
notarized, has no hardened runtime and no update feed. The launcher's checks are
unchanged: every bundled library must carry the same signature as the launcher.
Ad-hoc signing cannot pass them, because an ad-hoc designated requirement is
the launcher's own code hash, and no library has that hash.

Create the signing identity once:

```sh
./scripts/macos-beta-signing.sh create   # needs OpenSSL 3 on PATH
./scripts/macos-beta-signing.sh status   # keychain path and certificate SHA-1
```

It writes `email-macos-beta-signing.keychain-db` and
`email-macos-beta-signing.password`, both mode 600, to `~/.config/ycorp-release/`
(`EMAIL_BETA_SIGNING_DIR` selects another directory). The keychain is a
standalone file. It is never added to the keychain search list or made the
default, and the login keychain is not touched. codesign reads the file with
`--keychain` and, because codesign before macOS 27 needs it on a search list, a
private search list (see `--fixture` in [macOS updates](macos-updates.md)).
The password reaches `security` on standard input, never as an argument. Back
up both files together, outside source control.

Build and check each beta:

```sh
export EMAIL_DISTRIBUTION_PROFILE=official
export EMAIL_RELEASE_GOOGLE_CLIENT_ID='your-production-desktop-client-id'
# Supply EMAIL_RELEASE_GOOGLE_CLIENT_SECRET through your release environment.
export EMAIL_VERSION='0.1.0-beta.2'
export EMAIL_BUILD_NUMBER='2'   # one more for every release
./scripts/package-macos-release.sh --beta
./scripts/verify-macos-release.sh dist-release/beta/Email.app
```

Output goes to `dist-release/beta/`: `Email.app`, the download
`Email-<version>-<build>-<arch>.zip` with its `.sha256`, and `STATUS.txt`,
which says the build is not notarized. There is no DMG. The zip can be renamed
for the release, for example `Email-0.1.0-beta.2-macos-arm64.zip`.
[Release versions](release-versions.md) lists both values for each release. Like
`--distribute`, it packages the architecture of the Mac it runs on.

Later betas reuse the same identity: run the build again. The packager unlocks
the keychain from its password file only to sign, and locks it again
afterwards. Don't run `create` again. Keychain items that a beta creates trust
this certificate, so a new certificate would make macOS ask every tester again
for each account. When a Developer ID build replaces the beta, macOS asks once
per account, and testers choose **Always Allow**.

Testers open an unnotarized beta the first time with System Settings → Privacy
& Security → **Open Anyway**, or by running
`xattr -dr com.apple.quarantine /Applications/Email.app` before opening it.
Betas have no automatic updates; testers download new betas by hand.

## Keychain items and repeated password prompts

macOS checks each item's access requirements against the requesting program.
Changing a program's designated requirement or a specifically authorized code
hash can require approval again. A changed code hash alone does not imply lost
access: compatible certificate-bound designated requirements survive rebuilds.
Repeated reads, independent items and helpers amplified the old approval problem.

On macOS the service now stores credentials under `email.native.mail.v3`. When
a v3 item is missing, an authorized historical read attempts to copy it to v3.
Mail, calendar and Scheduling now share serialized access, memory caching and
cross-process revision fencing. A successful read remains cached if migration
cannot be persisted. After an access error, subsequent uncached reads are
noninteractive, so unlocks can recover without another dialog. Historical items
and separate app/helper identities can still need their own approval. Linux
keeps its existing `email.native.mail.v2` and SecretService behavior. These
service names belong to the `development` profile; other profiles derive theirs
from their own `KEYCHAIN_NAMESPACE` and never read these items.

The macOS MCP entry point disables interactive Keychain access before
opening MailService, for both stdio and dispatch-only runs. An item requiring
approval returns an error instead of raising a dialog from a background agent
process. The desktop app retains its normal OS approval flow. This does not
authorize MCP to read items approved only for the desktop, and existing
desktop approvals do not all fit into one dialog.

## Aligning development launches across worktrees

Use `scripts/open-macos.sh` from any aligned worktree. It resolves the primary
checkout through Git, verifies `dev.email.client`, its runtime signatures and
`dist/email-build.json`, then opens the exact development bundle. The build record
contains source identity, commit and artifact hashes only, outside the sealed
bundle. A source or artifact mismatch refuses launch instead of silently starting
an older app. `--print-path` reports the target without launching or reading tokens.

The local-alpha bundle has a different identifier and must remain separate from
the development app. Name-based `open -a Email` can select that alpha and
cause fresh Keychain requests even while the fixed development app exists.
Unregister obsolete alpha/test copies from LaunchServices rather than deleting
credentials, changing Keychain ACLs or re-signing the trusted development launcher.
Existing grants that were previously denied can still require approval; the
credential guard prevents repeated attempts within a process, not every system
prompt across independent programs.

Build identities use tracked and non-ignored source files. Ignored credentials
and Wrangler runtime caches are excluded, so identical worktrees produce the
same identity without depending on local secrets or cache state. Archive builds
retain explicit exclusions for secret and runtime directories.

Packaging now backs up the signed bundle, prefers its verified installed launcher
rather than a differently signed cached executable, and compares the exact code
hash before claiming preservation. An unexpected identity change fails and
restores the prior bundle before launch. Intentional launcher/resource changes
require `EMAIL_ALLOW_LAUNCHER_CHANGE=1`; that does not change Keychain grants.
`scripts/open-macos.sh --verify` validates the target without starting the app.
