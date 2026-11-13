# macOS Email app

Native AppKit desktop shell backed by the Rust `mail-service` dynamic library. It presents one unified inbox, a command palette, account setup, sync status, drafts, compose/reply, and a native plain-text reader. Sanitized HTML from the service is shown in a lazily created `WKWebView` with JavaScript and external navigation blocked. Remote HTTPS images follow the remembered image preference.

For a first run without a signing identity or account, run `scripts/dev.sh` from the repository root. It builds the unsigned SwiftPM executable and opens it with demo messages in `dist/demo/mail.sqlite`; see [Building and running Email](../../docs/building.md).

From the repository root, run `scripts/run-macos.sh` to build the signed app launcher and runtime libraries, create `dist/Email.app` and `dist/Runtime`, and launch it. It needs the persistent local signing identity described in the building guide. Keep both in the same `dist` folder. Run `scripts/run-macos.sh --package` to build without launching, or `scripts/run-macos.sh --smoke` for a headless ABI and database check. Set `EMAIL_STORE_PATH=/path/to/test.sqlite` before the launcher to use a separate database.

## Build

From the repository root:

```sh
cargo build -p mail-service
```

Generate the Git-ignored build identity file once per source change (`scripts/dev.sh` and `scripts/run-macos.sh` do this for you):

```sh
printf 'import Foundation\nenum EmailBuildIdentity { static let value = "%s" }\n' \
  "$(python3 scripts/build-identity.py)" > apps/macos/Sources/EmailUI/BuildIdentity.generated.swift
```

Then, from `apps/macos`:

```sh
swift build
```

If your Swift toolchain and Command Line Tools SDK versions differ, pass the SDK that matches your toolchain explicitly (replace `<version>`):

```sh
mkdir -p .swiftpm-cache .clang-cache
CLANG_MODULE_CACHE_PATH="$PWD/.clang-cache" swift build --disable-sandbox \
  --cache-path "$PWD/.swiftpm-cache" --manifest-cache local \
  --sdk /Library/Developer/CommandLineTools/SDKs/MacOSX<version>.sdk
```

Launch from `apps/macos` with an isolated store (without `EMAIL_STORE_PATH` it opens your real mail cache):

```sh
EMAIL_DEMO=1 EMAIL_STORE_PATH="$PWD/../../dist/demo/mail.sqlite" \
MAIL_SERVICE_LIBRARY=../../target/debug/libmail_service.dylib \
  "$(swift build --show-bin-path)/email-desktop"
```

This unbundled executable runs without new-mail notifications, which macOS only delivers to app bundles.

The app stores its cache at `~/Library/Application Support/<data directory>/mail.sqlite`, where the [distribution profile](../../docs/distribution-profile.md) names the directory: `invalid.email.unofficial` for a plain build, `Email` for the development profile. Set `EMAIL_STORE_PATH` to a test database path in its own directory to keep testing separate. `EMAIL_DEMO=1` adds three clearly labelled demo messages to that database, and `EMAIL_DEMO=launch` the [launch demo mailbox](../../docs/demo-mailbox.md); both are ignored unless `EMAIL_STORE_PATH` is set, so demo data never reaches the real cache. Real accounts are added through **+ Add account** in the sidebar. Never enter a primary account password into Email. iCloud uses an Apple app-specific password; other IMAP providers should supply an app password. Gmail and Microsoft use browser OAuth sign-in.

A build packages its own Google desktop OAuth client ID, so users can choose **Gmail → Sign in with browser** without entering a client ID or password. The client ID and its token-exchange value are not in source control: packaging reads `EMAIL_GOOGLE_CLIENT_ID` and `EMAIL_GOOGLE_CLIENT_SECRET` from the environment or the ignored, locally permissioned `config/oauth-desktop.local.env`, with the environment taking precedence. The `development` and `official` [distribution profiles](../../docs/distribution-profile.md) require the client ID and fail with a message naming that variable and file; a build with no profile has no client and no browser sign-in. To bring your own client, see [FORKING.md](../../FORKING.md). The development client is in Google's Testing mode, so only its listed test users can connect. A desktop app cannot keep this value confidential, so OAuth uses PKCE as well. Outlook sign-in needs a registered Microsoft client ID supplied as `EMAIL_MICROSOFT_CLIENT_ID`. OAuth access and refresh tokens are kept in the OS credential store.

## Keyboard

`j`/`k` move through mail, `e` archives, `m`/`u` mark read/unread, `r` replies, `c` composes, `/` focuses search, `i` toggles remote images, `⌘K` opens commands, `⌘R` syncs, and `⌘N` composes. Single-letter shortcuts do not fire while editing text. Remote images are blocked by default. A notice above the message offers **Load images** for that email; `i`, the View menu and the compact image control change the remembered default. Per-draft **Track opens** exists only when the build's distribution profile enables `OPEN_TRACKING` (the `development` profile); official and unofficial builds leave it out.

Press `Return` on a selected email to enter reader mode. `j`/`k` scroll the message, `Ctrl-D`/`Ctrl-U` or `Space`/`Shift-Space` move a page, `h`/`l` open the previous or next message in a conversation, `gg` jumps to the top, and `Shift-G` jumps to the bottom. `Esc` returns to inbox navigation. A native footer shows contextual shortcut keycaps beneath the reader and reply composer. The hints change for reading, composing, drafts, and outbox, and stay outside the email content. Account setup opens on Gmail; ⌘1–⌘4 choose a provider, Return connects, and Escape closes setup.

View → Appearance offers Follow System, Light, and Dark. `⌘⇧A` cycles through them. The choice persists across launches. Email content can retain the sender's own light backgrounds in Dark mode so its formatting and images remain readable as authored.

The contact pane opens beside a selected message on wide windows; press `0` to hide or show it. It uses the local mail cache for recent conversations. Use **Picture** to choose a local PNG, JPEG, or HEIC image for one sender or an entire email domain, or to remove an override. Email crops it to a small square and stores it only on this Mac. A custom picture takes priority over an available personal contact photo, a published BIMI logo, and initials. BIMI logos are fetched in the background and cached by domain; they are decorative and do not assert that a message or certificate is verified. If Email already has Mac Contacts access, matching local contact photos appear automatically; browsing mail never requests Contacts access. Email does not upload Contacts data.

For Gmail accounts, **Picture → Use Google contact photos…** starts a separate browser consent flow for read-only Google Contacts and Other contacts access. Once granted, Email looks up the sender's exact email address and shows an available personal photo in the contact pane. The permission is optional and does not interrupt normal mail sign-in or sync. Photos are cached in memory, and missing or unavailable photos fall back to the normal picture sources. Google's People API does not expose every picture Gmail may display, so matching Gmail's avatar for every sender is not guaranteed. The Google Cloud project used by the desktop OAuth client must have the People API enabled.

## Verification

A headless integration smoke check opens an isolated database, reads accounts and threads through the C ABI, and renders the first message if one exists:

```sh
MAIL_SERVICE_LIBRARY=/absolute/path/to/libmail_service.dylib \
EMAIL_STORE_PATH="$TMPDIR/email-macos-smoke.sqlite" \
.build/out/Products/Debug/email-desktop --smoke-service
```

The app uses a long-lived in-process service handle. Network operations run off the main UI thread. Provider sends only show success after the service reports provider acceptance.

## New mail notifications

Choose **Email → Notifications → Enable New Mail Notifications**, or search for notifications in `⌘K`. macOS requests permission when you opt in. A new-mail alert shows the sender, the subject and the first new lines of the message. Quoted history, reply headers and signatures are left out. Its **Mark as Read** and **Archive** buttons act without opening the window. **Show Sender and Subject** is on by default. Turning it off leaves only “New email” and the buttons. Earlier builds saved that setting as off, so it is switched back on once. macOS’s own **Show previews** setting still hides content when the Mac is locked. **Send Test Notification** checks delivery, including while Email is in front.

Notifications follow the existing 60-second mail refresh. Each account's first successful sync establishes a quiet baseline; older-mail downloads, read messages, and repeated syncs do not create alerts. New replies can notify again. Bursts of more than three arrivals are grouped. Normal incoming banners stay quiet while Email is active. Clicking an alert opens its account and selects the thread if it is still in the loaded inbox; an active composer is preserved. Closing the last window keeps Email running only when notifications are enabled. `⌘Q` stops sync and notifications; there is no server push while the app is quit or the Mac is asleep.

Arrival policy checks can run without XCTest:

```sh
swiftc -parse-as-library apps/macos/Sources/EmailUI/MailArrivalTracker.swift \
  apps/macos/Tests/NotificationTrackerChecks.swift -o "$TMPDIR/email-notification-checks"
"$TMPDIR/email-notification-checks"
```
