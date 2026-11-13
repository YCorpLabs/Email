# Desktop notification behavior

Notifications are opt-in. On Mac use Email → Notifications; the command palette
also finds notification settings. Sender and subject previews are a separate
choice, enabled by default. Explicitly hidden previews stay hidden. Linux exposes
the same settings in Commands.

The app watches successful sync results for new unread messages and later replies
in existing conversations. Downloading older history, marking an old message
unread and repeating a sync do not create new-mail alerts. Each account has its
own initial cache baseline. Arrivals are consumed while notifications are off;
enabling alerts does not replay mail from that disabled period. Large bursts are
grouped.

Follow-up deadlines are evaluated locally without waiting for a network sync.
Each reminder is alerted once; changing the reminder creates a new deadline.
Notification clicks open the relevant conversation, using Follow-ups for a
reminder so archived and Sent conversations remain reachable. An active unsent
composer is preserved.

Delivery still depends on the operating system's notification permission, desktop
notification service and Focus/Do Not Disturb settings. These alerts require Email
to be running; they are not a push service for a fully quit app. A notification
request accepted by the OS is not evidence that a banner was displayed. Use Send
Test Notification to check the actual desktop presentation.

## Notification previews

- macOS and Linux now default missing preview preferences to on. Existing explicit
  hidden-preview preferences remain respected; notification permission itself is
  still opt-in.
- Individual arrivals show the sender and subject. Empty subjects show “(No subject)”.
  Bursts include up to three sender/subject pairs instead of an instruction to open
  the app. Hidden previews retain the generic text.

## Message snippets and sent-email activity

Both native shells now use the latest incoming message's sender and subject for
individual arrivals, followed by a compact plain-text snippet (up to 180 characters).
Whitespace is collapsed, empty subjects use `(No subject)`, and the complete body
is bounded to 320 characters. Existing explicit hidden-preview preferences remain
respected. Bursts retain their sender/subject summaries.

While Email runs, both shells check sent-email tracking activity at most once per
minute. Only messages already sent with Track opens enabled participate; this
does not enable tracking on any message. Notifications must also be enabled and
the original tracking service connection must remain usable. No tracking refresh
is requested while notifications are disabled. On first use, the first successful poll establishes
a quiet baseline. Activity from before the current session or notification-enable
time is consumed without alerting. Per-message count cursors persist in the local
store and use the opaque tracking ID, surviving provider message-ID adoption and
activity-panel refreshes. Multiple new requests for one message coalesce per poll;
large bursts show three individual alerts and a summary.

Alerts say `Email open activity` and explain that the image load may be an automated
preview. They cannot identify which recipient read a message or prove a human read.
Visible previews include the subject; hidden previews omit it. Clicking opens Sent
and selects the conversation when it is in the loaded list. An active unsent draft
is preserved. These remain local polling alerts, not notifications while Email is quit.
Count changes are consumed before OS delivery, so an OS-rejected alert is not replayed.

## Delivery, Dock badge and open alerts

On macOS, a missing notification grant (`notDetermined`, for example after the
bundle was re-signed) used to make every delivery return early without any
visible error, while `email.notifications.enabled` still said alerts were on.
The macOS shell now:

- asks once on first launch when the user has never chosen, and asks again on
  launch when the preference is on but the OS grant is missing;
- requests alert, badge and sound together (older grants asked for alerts only);
- plays the default sound and adds alerts to Notification Center when the app
  is frontmost;
- sets the Dock badge to the All Inboxes unread count on every inbox refresh;
- titles open alerts `Opened: <subject>` (hidden previews: `Your email was
  opened`), and updates the reader's open count when an alert arrives.

On Linux, the GTK shell has no Dock badge equivalent, and the sound and
first-launch prompt changes above do not apply.

## Useful content and actions

The preview preference (`email.notifications.previews`) is written only by the
menu item and the ⌘K toggle, and the ⌘K command used to give no feedback, so
previews could be turned off by accident. Both shells now:

- reset the preview preference once (macOS key
  `email.notifications.previewsDefaultV2`; Linux marker file
  `<store>.notification-previews-v2`), so it reads as on. Turning it off later
  is kept;
- show the sender's display name as the title. macOS shows the subject as the
  subtitle. Linux puts the subject on the first body line, because GNotification
  has no subtitle;
- show the message's new text: quoted `>` lines are skipped, and the snippet
  stops at `On … wrote:`, `-----Original Message-----`, a later `From:` line,
  `-- ` signatures, `Sent from my …` and Outlook rules. It is cut to 180
  characters with an ellipsis;
- add **Mark as Read** and **Archive** to single-conversation alerts. They run
  `batch_triage` without raising the window. The Undo bar can reverse them, and
  the account syncs afterwards;
- list up to three `Sender — Subject` lines in a batch, plus `and N more`;
- with previews hidden, show `New email` with the buttons, not an instruction
  to open the app.

On macOS, the alert's category supplies a `New email` placeholder for the OS
**Show previews: When Unlocked/Never** setting. The ⌘K preview toggle now
reports its new state in the status line.

## Newsletter snippets

Some newsletters have a stub plain part: dividers, a web-version link, “your
email client cannot display HTML”, then legal text. The real teaser is only in
the HTML preheader.

The snippet is now built once in `mail_core::notification_snippet` and returned
by `get_latest_message_for_thread` as `notification_snippet`. Both shells use
that field, and the separate Swift and Linux copies were removed. It:

- keeps the reply rules above (quoted lines, `On … wrote:`, signatures and so on);
- removes links, `[image: …]` placeholders, zero-width preheader padding,
  HTML entities left in plain text, and lines with no letters or digits
  (dividers);
- drops short paragraphs that are only chrome: “view web version”, “view in
  browser”, “trouble viewing”, “unsubscribe”, “manage preferences” and similar;
- when the plain part is empty, or says the client cannot display HTML, uses
  the hidden HTML preheader, or else the visible HTML text.

## Unbundled development runs

macOS only delivers notifications to app bundles. In the plain SwiftPM
executable, which `scripts/dev.sh` runs and `scripts/run-macos.sh` runs when
`EMAIL_STORE_PATH` is set, `UNUserNotificationCenter.current()` raises
`bundleProxyForCurrentProcess is nil`, which used to crash Email at launch.
`MailNotifications.available` is now true only when `Bundle.main` is an `.app`
bundle, and every UserNotifications call goes through one accessor that is nil
otherwise. Without a bundle, notifications are not started, enabling them
explains that they need the app, and the saved preference is left alone. An app bundle takes exactly
the same paths as before.

On Linux, a run with an isolated store (`EMAIL_STORE_PATH`) uses its own
application ID, `<profile Linux app ID>.Isolated.s<hash>`. No desktop file matches
that ID, and GNOME Shell may not show notifications from an application it
cannot find. Unpackaged builds with the normal ID have the same limitation until
the package's desktop file is installed. This is a known gap.
