# Account setup: Gmail app passwords, Google sign-in gate, release features

User guide: [Connect Gmail with an app password](gmail-app-password.md).

## Why

Google's restricted-scope review for `gmail.modify` takes weeks. Until it
passes, Email's production Google Cloud project stays in Testing and
blocks every Google account that is not one of at most 100 test users at the
consent screen. Publishing it unverified would show the "unverified app" warning
and use up a lifetime cap of 100 users. So for launch, most Gmail users connect
with a Google **app password** over IMAP and SMTP, which involves no OAuth
client and no Google API scope.

## Build-time switches

All are resolved once, at compile time, in `crates/mail-cloud/build.rs`
(`crates/mail-cloud/src/release.rs`). The service reports them to both shells
with `{"op":"build_features"}`; macOS reads it through `CoreBridge.buildFeatures()`
and Linux through `account_setup::build_features`.

| Input | Values | Default | Effect |
| --- | --- | --- | --- |
| `EMAIL_RELEASE_CHANNEL` | development, local-alpha, fixture, distribution | development; the packagers set local-alpha, fixture or distribution | `distribution` sets `cfg(email_distribution)`: no Contacts scope request is compiled in and contact photos are hidden, so the first release does not need Google to approve the Contacts scopes; Outlook/Hotmail is hidden and refused until it has been tested. |
| Profile `GOOGLE_OAUTH_PUBLIC` (`config/profiles`) | true, false | false in `official`; true in `development` and `unofficial` | "Google OAuth publicly available". When false, account setup offers no Google sign-in and `start_oauth` refuses Gmail. Accounts already connected through Google sign-in keep syncing, sending and connecting Calendar with their stored client, but cannot be re-authorized in that build; re-adding Gmail there uses an app password. A `distribution`-channel build with a non-distribution profile keeps it false. |
| `EMAIL_GOOGLE_OAUTH_PUBLIC` | true, false, unset | unset | Overrides the profile value for one build. |

`scripts/package-macos-release.sh` and `scripts/linux-build-config.sh` export
the channel and the resolved sign-in value (`email_resolve_google_oauth_public`
in `scripts/distribution-profile.sh`, the same rule as `crates/mail-cloud/build.rs`).
Distribution packaging requires a distribution profile (`official`), so
official releases keep Google sign-in off. Once Google's review passes,
maintainers set `GOOGLE_OAUTH_PUBLIC=true` in `config/profiles/official.env`.

## Behavior (both shells)

| | Development build | Official release, sign-in off (launch) | Official release, sign-in on |
| --- | --- | --- | --- |
| Choices | Gmail, Gmail (app password), iCloud, Outlook, Other | **Gmail**, iCloud, Other | Gmail, Gmail (app password), iCloud, Other |
| Selected on open | Gmail sign-in (app password if no client is configured) | Gmail, on the app-password path | Gmail sign-in |
| Contact photos | Offered | Hidden; no Contacts scope compiled in | Hidden |

The app-password panel shows, in order:
- "One-click Google sign-in is coming once Google finishes reviewing the app." (only when sign-in is off);
- three numbered steps, each with its direct link where there is one:
  1. Turn on 2-Step Verification: <https://myaccount.google.com/signinoptions/twosv>.
  2. Create an app password named "Email": <https://myaccount.google.com/apppasswords>.
  3. Paste the 16 letters; spaces don't matter.
- the address and app-password fields. The request names no servers: the service always uses `imap.gmail.com:993` and `smtp.gmail.com:465`, both TLS;
- what an app password is and how to revoke it;
- which accounts can't create one;
- a link to Google's help article (185833).

Keyboard: ⌘1…⌘n on macOS and Ctrl+1…Ctrl+n on Linux pick a choice (the
modifier alone; Shift is allowed for keyboard layouts that need it). Focus
moves to the address field, Return submits and Escape closes; with a security
list open, Escape first closes only the list. Errors appear inline; the window
stays open and nothing is saved. A choice with a request still running cannot
be submitted again (returning to it shows "Still connecting…"). Other choices
stay usable meanwhile. A request that ends while another choice is on screen
keeps its outcome for its own choice: an error is shown when that choice is
selected again, and a success refreshes the app, keeps the window and what is
typed for the other choice, and shows "Connected <address>." on return.

When Google sign-in is offered but may not work for this person, a "Use Gmail
(app password) instead" link appears: while the browser sign-in waits, after it
fails, or when no Google client is configured. Google's denial and timeout
errors name the app-password path.

## Shared service behavior (`mail-imap`, `mail-service`)

- Request: `add_imap_account` with `"provider":"imap","preset":"gmail"`, the
  address and the app password. This is the generic IMAP path.
  `ImapConfig::gmail` strips spacing from the app password and uses the full
  address as the IMAP and SMTP username. The preset ignores any server, port,
  security, username or archive values in the request, so an app password made
  for Google is only ever sent to Google's servers.
- Before any connection, a value that is not 16 letters is refused with "That
  isn't a Google app password…". A regular Google password is therefore never
  sent to Gmail.
- `verify_account` reports the step that failed (connect, login, mailbox, SMTP).
  `SetupError::message` maps Gmail's responses to the next step:
  - `[AUTHENTICATIONFAILED]` and SMTP 535: wrong app password, including the security-key, Advanced Protection and work-account case;
  - "Application-specific password required" and SMTP 534 5.7.9: the regular Google password was used;
  - "IMAP access is disabled for your domain": a Workspace administrator turned IMAP off, with the Admin console path;
  - `[WEBALERT]` / "Web login required" and SMTP 5.7.14: confirm the sign-in in a browser;
  - too many connections, unreachable hosts and TLS failures.

  The advice adapts to whether this build offers Google sign-in. Messages never
  contain the password. The mapping uses Google's documented and commonly
  reported texts; Google's live wording may differ.
- Setup uses one IMAP session: it logs in, selects INBOX and, on servers that
  advertise `X-GM-EXT-1`, tries to list mailboxes (a failure there does not fail setup) to find the one marked `\All` (All
  Mail). The name is localized for some accounts, for example
  `[Google Mail]/Alle Nachrichten`. Archive moves to the saved name and looks
  `\All` up again only if Gmail rejects it (for example after a display-language
  change). If All Mail is hidden from IMAP, nothing is guessed: the archive stays
  queued with "turn on Show in IMAP for All Mail".
- **Sync bandwidth.** Each IMAP sync used to re-download the 25 newest message
  bodies to refresh read state. At a sync every minute that approaches Gmail's
  2,500 MB/day IMAP download limit. Messages already cached now refresh with
  `UID FETCH … UID FLAGS` only, because IMAP bodies never change. Read state
  changed in other clients still arrives. This applies to every IMAP account,
  including iCloud.
- **Download budget.** A message that does not fit what is left of one sync's
  download budget is no longer stored as an empty "Message exceeds local sync
  limit" placeholder. It is skipped, and the sync cursor stays where it was, so
  the next sync downloads it (messages already cached cost only their flags).
  The first download of every sync is always allowed, so each sync makes
  progress. Permanent placeholders (messages over the 40 MB limit, undecodable
  MIME, no size or body from the server) are stored once and not downloaded
  again. Previously, every sync downloaded the 25 newest messages again,
  placeholders included; a budget placeholder that then fit became a real
  message and failed the sync with "message ID belongs to a different account
  or thread". Flags-only refresh and deferral remove that path. Placeholders stored before this change
  stay as they are; removing and re-adding the account downloads them again.
  Deferral needs to know what is cached, so it applies to the store's sync
  (`run_sync`). A plain `MailAdapter::sync` call has no cache index and stores
  over-budget mail as placeholders, as before, so it can never stall.
- **Known limitation: missing size or body.** If Gmail answers a fetch without
  a size or body, the message is stored as a placeholder and not downloaded
  again. (Previously it was downloaded again while among the 25 newest, and a
  successful download then failed the sync.)
  Retrying safely needs a re-threading step in mail-core; that is left for a
  follow-up. Removing and re-adding the account downloads such messages again.
- **Duplicates.** A mailbox that is already connected cannot be added again,
  whichever way, for example a Google sign-in finished in the browser after an
  app-password setup. For gmail.com and googlemail.com, case, dots and `+tags`
  are ignored, as Gmail does (`mail_imap::mailbox_identity`, also used by the
  fixture). Google sign-in learns the address only after consent, so a
  duplicate is refused after the browser step. Its new token is discarded, not
  revoked: revoking ends the whole grant, which would also sign out an existing
  Google sign-in account of the same address. Release builds with Google
  sign-in off don't offer this path.
- **All Mail name found while archiving is not saved.** If Gmail rejects the
  saved All Mail name (after a display-language change) or setup could not see
  All Mail, each archive looks it up again: one extra LIST, and one refused
  MOVE in the renamed case. Saving it back needs the account-configuration
  update path (generation checks against concurrent removal); not worth that
  risk for a rare, performance-only case. Re-adding the account saves the new
  name.
- **Build features.** Both shells ask `build_features` once per process and
  keep the first answer, so the offered choices never change while the app
  runs. If the library cannot answer, optional features (Google sign-in,
  Outlook, contact photos) stay off for that session.
- **Advice.** The shells send `google_sign_in` (whether they offer Google sign-in
  right now). The "choose Gmail to sign in with Google" advice appears only when
  it does.
- **macOS handles.** Account setup (`add_imap_account`, `start_oauth`) runs on its
  own service handle, so a slow server never blocks the window or cached reads.
  This matches Linux's per-request service.
- **Known limitation (all IMAP accounts, unchanged):** IMAP sync does not
  notice mail that left INBOX elsewhere (archived on the web or phone, moved by a
  filter), so it stays in Email's inbox until archived in Email. Fixing it needs
  a "left the inbox" signal in mail-core sync rather than deletion, because
  `removed_message_ids` deletes rows and would drop archived mail locally.
- QA only: the `test-fixture` feature (`mail-imap`, `mail-service`;
  `qa-fixture` in `apps/linux`) adds a synthetic Gmail IMAP/SMTP server
  (`crates/mail-imap/src/fixture.rs`), trust for its in-memory CA, an endpoint
  redirect (`EMAIL_TEST_FIXTURE_ENDPOINTS`) and file credentials
  (`EMAIL_TEST_CREDENTIALS_DIR`). These keep QA runs out of the Keychain and the
  Secret Service. Release builds never enable the feature.
