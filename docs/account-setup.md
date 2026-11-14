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
| Opens on | The email field | The email field | The email field |
| `gmail.com`, `googlemail.com` | Gmail sign-in (app password if no client is configured) | Gmail, on the app-password path | Gmail sign-in |
| Contact photos | Offered | Hidden; no Contacts scope compiled in | Hidden |

### Email first

The window has one title, the heading **Add an account** (macOS hides the
title-bar text; Linux shows an empty header bar). It opens with only the email
field, a small **Choose provider** menu under it, and Cancel / **Continue**.
Continue asks the engine which provider the address belongs to
(`setup_provider`, domain only, no network):

- `gmail.com`, `googlemail.com`: Gmail;
- `icloud.com`, `me.com`, `mac.com`: iCloud;
- `outlook.com`, `hotmail.com`, `live.com`, `msn.com`: Outlook where the build
  offers it, otherwise Other;
- everything else: Other.

Choose provider (and ⌘1…⌘n / Ctrl+1…Ctrl+n) picks a provider by hand, for
custom domains on Google Workspace or iCloud. Until then, editing the address
follows it to its provider.

### Continue with Google

Only where Google sign-in is available: the build's `GOOGLE_OAUTH_PUBLIC` is on
**and** a Google client is configured. Official builds with sign-in off show
the screen above unchanged.

The first screen then starts with **Continue with Google**, a thin **or**
divider, and the Email field for iCloud and everything else. Nothing else is
added. The button follows Google's sign-in branding guidelines: the standard
four-colour “G” at 18 pt, 40 pt tall, 4 pt corners, 12 pt before the logo and
the text centred; light theme white with a #747775 outline and #1F1F1F text,
dark theme #131314 with a #8E918F outline and #E3E3E3 text. The text is Roboto
Medium 14 when installed, otherwise the system font (macOS) or Inter (Linux).

Continue with Google goes straight to browser sign-in without typing; the
address comes from Google afterwards (`start_oauth` needs none). Where an
address is already known it is passed as `login_hint`, so Google skips its
account chooser: a gmail.com or googlemail.com address typed in the Email
field (for this button and for the Gmail “Sign in with browser” step). Other
typed addresses are not passed. Calendar and photo grants already hint the
account's address.

While “Waiting for browser sign-in…” shows (Google or Outlook), the window
holds only that line, the “Use Gmail (app password) instead” link and Cancel.
The Email field, Choose provider, the intro line and the disabled Sign in with
browser button come back when sign-in fails, including a Cancel on Google's
page. Cancel in the window closes it, as before. Verified 14 Nov 2026 on macOS
and Linux (dark): the waiting state, then Google's Cancel callback
(`error=access_denied` sent to the loopback redirect) brought the form back
with the existing denial message.

Verified (13 Nov 2026): macOS development build and the Linux shell in the
verify container, `EMAIL_GOOGLE_OAUTH_PUBLIC=true` with a placeholder client ID,
dark and light, and `EMAIL_GOOGLE_OAUTH_PUBLIC=false` (no button). The browser
command was replaced by a logger: Continue with Google opened the sign-in URL
with no `login_hint`; a typed `someone@gmail.com` plus the button, and a typed
`someone@googlemail.com` through Sign in with browser, passed the address; a
typed iCloud address plus the button passed none. `run-account-setup-checks.py`
still passes 126 checks (its development run shows the button). Google's live
consent screen was not opened.

### What each provider shows

Gmail, about 40 words before any disclosure:

> Gmail needs an app password.
> 1. Turn on 2-Step Verification ↗ (<https://myaccount.google.com/signinoptions/twosv>)
> 2. Create an app password ↗ (<https://myaccount.google.com/apppasswords>)
> 3. Paste it below
>
> App password: "16 letters, spaces are fine" · ▸ What's an app password?

The disclosure says it's a 16-letter code just for Email, stored in the
Keychain (Secret Service on Linux); that it can be revoked in the Google
Account and that changing the Google password revokes it; that Google hides
the option for security-key-only, Advanced Protection and some work or school
accounts; and that Email connects to `imap.gmail.com` and `smtp.gmail.com`.
The request names no servers: the service always uses `imap.gmail.com:993`
and `smtp.gmail.com:465`, both TLS.

iCloud follows the same pattern: "iCloud needs an app-specific password.",
1. Sign in to your Apple Account ↗ (<https://account.apple.com/>),
2. Create an app-specific password, 3. Paste it below; placeholder
`abcd-efgh-ijkl-mnop`; ▸ What's an app-specific password?

Other: "Use your email password or an app password." and a Password field. The
request names no servers; the engine guesses them (see below). The server
fields appear only when the guess fails, filled with the guess, or under
**Advanced** after any failed attempt (servers, ports, security, usernames,
display name).

Gmail, iCloud and Other end with **Need help? Step-by-step guide ↗**
(`https://ycorplabs.com/email/guide/#gmail`, `#icloud`, `#other`) only in
builds whose support address is Y Corp's own (`profile::setup_guide_url`,
reported as `setup_guide_url` in the profile JSON). Forks and unofficial
builds never link to that site.

Not shown any more: the Google-review sentence (no review has been
submitted), "Never enter your main account password here", the
`imap.gmail.com`/`smtp.gmail.com` footnote (now in the disclosure) and the
intro paragraph.

### Errors

A failure shows one line, a fix link where there is one, and **Details** with
what the server said (`setup_error` from the engine, below). They sit directly
under the password field (under the email field before a provider is shown),
and an error closes the "What's an app password?" disclosure:

| Situation | Line | Link |
| --- | --- | --- |
| Gmail: not 16 letters (checked before connecting), or "Application-specific password required" | That looks like your Google password. | Create an app password instead ↗ |
| Gmail: `[AUTHENTICATIONFAILED]` with an app password | Wrong email or app password. | Create a new app password ↗ |
| iCloud refused, password not shaped like `abcd-efgh-ijkl-mnop` | That looks like your Apple Account password. | Create an app-specific password instead ↗ |
| iCloud refused an app-specific password | Wrong email or app-specific password. | Create a new app-specific password ↗ |
| Any other server refused | Wrong email or app password. | none |
| Other: guessed servers don't answer | Couldn't find the mail servers for example.com. Check the server settings. | none; server fields open |
| Workspace admin turned IMAP off | Your Google Workspace admin has turned off IMAP for this account. | none; Admin console path in Details |
| Google blocked the sign-in | Google blocked this sign-in. Confirm it was you in Gmail, then try again. | Open Gmail ↗ |

"Turn on 2-Step Verification first" has no error of its own: an account
without 2-Step Verification has no App passwords page, so it never reaches the
server with an app password. Step 1 and the disclosure cover it, and the "looks
like your Google password" line catches the regular password before it is
sent. A Gmail `[AUTHENTICATIONFAILED]` after that check means the app password
or address is wrong, so it is not reported as the Google password.

Keyboard: ⌘1…⌘n on macOS and Ctrl+1…Ctrl+n on Linux pick a choice (the
modifier alone; Shift is allowed for keyboard layouts that need it). Focus
starts in the email field and moves to the password once a provider is shown.
Return continues, then submits; Escape closes; with a security list or the
provider menu open, Escape first closes only the list. Errors appear inline;
the window stays open and nothing is saved. A choice with a request still
running cannot be submitted again (returning to it shows "Still connecting…").
Other choices stay usable meanwhile. A request that ends while another choice
is on screen keeps its outcome for its own choice: an error is shown when that
choice is selected again, and a success refreshes the app, keeps the window
and what is typed for the other choice, and shows "Connected <address>." on
return.

### Verified (13 Nov 2026)

- macOS, official-profile distribution engine: the email field, Gmail (with and
  without the disclosure), iCloud, Other, the Gmail "looks like your Google
  password" error with Details, and Other's servers-not-found state, in dark
  and light mode. `run-account-setup-checks.py` passed 126 checks, including
  the error's position under the field against the
  fixture in both modes: routing by domain, ⌘1–⌘n, focus, payload keys, every
  error line and fix link above (iCloud and a guessed Other server through
  fixture redirects), in-flight and background outcomes, Return, Escape.
- Linux, the same engine in the verify container under Xvfb: the same states
  in dark and light; the running executable matched the built artifact. The
  Linux shell tests (62) pass. `apps/linux/qa/account-setup-fixture.sh` ran
  through: the welcome, routing, every error above (iCloud and Other through
  the fixture redirects), in-flight and background outcomes, connecting,
  search, Escape in the provider menu, and the development build's Gmail
  sign-in and Outlook choices. Its screenshots were checked by eye.
- Linux dialog frame: the header bar makes the dialog client-side decorated,
  and GTK framed it in the default light theme's colors (a white 4 px band
  without a compositor, a light outline with one). The frame now uses the
  app's line color; checked with and without a compositor (`xcompmgr`).
- Not verified: Google's live responses (the fixture uses their documented
  texts), real iCloud and third-party servers, and a Wayland session.

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
  looks like your Google password." A regular Google password is therefore
  never sent to Gmail.
- Failures cross the JSON API as `setup_error` in the response envelope
  (`MailService::request_envelope`, also the C ABI): `code`, a one-line
  `message`, an optional `action` (`label`, `https` `url`), `detail` (what the
  server said, password redacted) and `servers`. `request_json` keeps a plain
  `error` string for every other caller.
- **Server guess (Other).** `add_imap_account` for `imap` without a preset and
  without servers uses `mail_imap::guess_servers`: a few providers by name
  (Yahoo, AOL, Fastmail, plus Gmail and iCloud domains), otherwise
  `imap.<domain>:993` TLS and `smtp.<domain>:587` STARTTLS. There is no MX or
  autoconfig lookup; connecting on submit is the only network use. If nothing
  answers (or TLS or sending fails), the failure is `servers_not_found` with the
  guess, which the shells show for editing.
- `verify_account` reports the step that failed (connect, login, mailbox, SMTP).
  `SetupError::explain` maps Gmail's responses to the next step:
  - `[AUTHENTICATIONFAILED]` and SMTP 535: wrong app password, including the security-key, Advanced Protection and work-account case;
  - "Application-specific password required" and SMTP 534 5.7.9: the regular Google password was used;
  - "IMAP access is disabled for your domain": a Workspace administrator turned IMAP off, with the Admin console path;
  - `[WEBALERT]` / "Web login required" and SMTP 5.7.14: confirm the sign-in in a browser;
  - too many connections, unreachable hosts and TLS failures.

  Messages never contain the password. The mapping uses Google's documented and commonly
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
- **Advice.** Older shells send `google_sign_in`; the engine accepts and
  ignores it. The one-line messages don't depend on it, and the window's
  disclosure covers accounts without app passwords.
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
