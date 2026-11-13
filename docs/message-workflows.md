# Message workflow correctness

## Reply and sending identity

Mac now honors the selected message's Reply-To header, falling back to Sender.
Linux already used Reply-To. Both consider To and Cc when selecting a receiving
alias. A reply keeps the selected message's account, and an explicit From choice
is not replaced when alias lookup finishes.

Linux alias selection now prefers the receiving address and then the provider's
`is_default` address. It no longer picks the first alphabetical alias. Cached
identities can display before fresh provider identities; updates may refine the
automatic choice until the user picks one. Reopened drafts preserve their saved
From address before a network lookup and keep their existing account on both
platforms. Mac hydrates its last verified Gmail aliases from local preferences before the composer first renders, then refreshes them asynchronously. This avoids briefly choosing a primary address while waiting for a receiving-alias lookup. Preferences contain address/default metadata only, never tokens.

Mac recipient UI validation accepts comma/semicolon-separated lists,
quoted display names and whitespace between addresses, while rejecting control
characters and malformed address structures. Provider delivery validation remains
authoritative.

Both native composers now support Cc/Bcc editing and Reply All (Shift+R), with explicit palette, toolbar and context-menu actions. Reply-All prefers Reply-To, deduplicates recipients, excludes known owned accounts and cached verified aliases, and never copies original Bcc recipients. Details reveals the recipient fields; Tab and Shift-Tab follow the visible field order. Drafts preserve all three recipient lists, including Cc/Bcc-only messages.

## Keyboard safety

Linux now returns unhandled Ctrl/Alt/Super combinations to the desktop/editor
instead of letting them trigger plain-letter mail actions such as archive.
Ctrl+U remains Vim-style half-page up; unsubscribe review is Ctrl+Shift+U, shown
in the command palette, message menu, tooltip and shortcut help. Linux's drawn
selection footer now actually displays selection shortcuts rather than the
standard inbox footer. Neither change grants subscriptions or automatically
opens a sender link.

Mac also installs the standard responder-chain Edit menu (Undo/Redo, Cut/Copy/Paste, Select All), with text undo enabled in the composer. Palette Return/arrow/Escape handling is wired directly through the native search field editor, as well as the event monitor, so command execution does not depend on nonactivating-panel key-window reporting.

Right-clicking a conversation outside the marked set clears the marks and opens single-message actions. Right-clicking inside the marked set preserves it and offers only batch archive/restore, spam, read and unread actions, each explicitly naming the selected count. Reply, labels, favorite, snooze and unsubscribe are hidden in batch context.

## Arrival and queued actions

Linux schedules a non-demo inbox sync at startup and every idle minute. A shared
atomic worker guard coalesces simultaneous whole-inbox requests and is released
even on worker failure. Both shells schedule bounded queue continuation using
`pending_actions` and `actions_retry_after_ms`; pending work normally continues
in about five seconds while provider backoff is honored. Incoming sync failures
reported as `data.sync_error` are displayed as failures, including when outgoing
queue work succeeded in the same pass.

## Local plain-text snippets

While composing or replying, open Commands and choose **Save message as snippet…**,
name the current message body, then later choose **Insert snippet…** and search
its name or preview. Enter inserts at the caret, replacing only selected text;
existing surrounding draft text stays. There is no AI generation and no automatic
expansion. Snippets stay in the local mail database. The first UI version exposes
save and insert; editing/deletion are available through the service/MCP API.

## Full Gmail sender cleanup

**Sender cleanup…** offers all matching unread Gmail messages or the existing
Downloaded conversations only flow. A full scan includes mail not downloaded,
uses an exact sender, and excludes Junk and Trash. Scanning makes no read-state
changes. The **Cleanup jobs…** command reopens saved progress. Each job shows its
account, sender, match count, marked count, skipped count, samples and errors.
Only the explicit count/account/sender confirmation starts marking messages read;
messages arriving after that preview are excluded. Pause, Resume and Cancel job
are available; an already executing batch may finish. Completed changes cannot
restore the original unread state. Job state is durable; opening the jobs panel
continues an active scan or run after restart. Closing the panel stops its next
UI-driven step, preserving the current job. Provider retry deadlines are honored,
and a worker never overlaps another step in the same panel.

Cleanup controls stay available during a network batch. Pause/Resume/Cancel
requests queue ahead of automatic next steps, with immediate “current batch may
finish” feedback. Repeated controls for the same job keep the latest requested
intent. Confirmation and job labels show the configured account's email address;
subject previews are explicitly labelled as cached samples, not all results.

## Structured local search

Both native shells use the same scoped search service for mailbox lists and
queries. Account, split, unread, favorite, label, archive and snoozed restrictions
are applied before pagination, rather than filtering a limited global result.
Typing waits 120 ms before querying; results are guarded against a newer query
or navigation. Linux query work runs off the GTK thread. Search never makes
provider requests on each keystroke and covers downloaded messages only.

The field placeholder and **Search syntax and coverage** command explain this
coverage and operators: `from:`, `to:`, `cc:`, `subject:`, quoted `label:`,
`is:unread/read/starred`, `has:attachment`, `in:inbox/archive/spam/trash`,
`before:YYYY-MM-DD` and `after:YYYY-MM-DD`. Query restrictions intersect the
selected mailbox; conflicting restrictions return no matches. Open Archive to
search archived messages. Contact Find mail uses `from:`; Linux label buttons select a dedicated exact label
scope, including names containing spaces. Typing keeps that label selected.
Favorites in All Inboxes include downloaded archived favorites on both shells. Parse errors show their actual cause and keep the
previous results visible. Escape clears the query and restores inbox keyboard
navigation. [Search benchmark](benchmarks/search-100k.md) records results on a
100k-message cache and its limitations.

Compact reply Details now scrolls internally (160 pt maximum) on both shells.
Recipient rows no longer impose their full minimum height on the outer window.
The area can compress in a short window; editor and context height preferences
remain flexible. Mac reply height uses a bounded target instead of a fraction of
an unusually tall window. Draft list previews show To, then Cc, then Bcc with an
explicit recipient label, including copy-only drafts. Reopened copy fields show
“Hide Cc/Bcc” consistently.

## Send destination and Sent mailbox

Mac Send/Schedule no longer unconditionally navigates to Outbox. Closing the
composer returns to the reader, retains the browsing context, and offers the
queued send's Undo/Cancel action. Sending an existing draft currently returns
to the main inbox. Linux already retained the current mailbox when queuing.
Outbox remains a pending queue: provider-accepted messages are removed from it.
Both shells now expose Sent in the sidebar and command palette, with G then T
navigation. Sent searches local messages with is_sent=1 and is_draft=0, including
mixed incoming/outgoing threads. This is not a full provider Sent-history sync.

## Sent conversation continuity

Both composers pass an explicit `reply_thread_id` when creating a reply draft.
The engine validates account ownership and attaches that draft to the existing
conversation. Unsent drafts are excluded from reader history. Marking a reply
sent retains Inbox/Archive placement when received messages exist in the thread.
Gmail replies submit threadId plus References/In-Reply-To derived from the
provider's metadata; accepted provider IDs replace local sent IDs for sync
reconciliation. Other providers retain local conversation continuity; their
remote reply headers remain a gap.

Follow-ups is a waiting-for-reply list: select a conversation, use Commands >
“Follow up unless they reply…” to set a due time, and G then W to open the list.
This differs from snoozing mail the user intends to handle later.

## Named snooze dates

Both command palettes and nested snooze pickers accept `remind tonight`,
`tonight`, `this evening`, `tomorrow morning`, `tomorrow evening`, and explicit
times such as `remind me tonight at 8pm` or `tomorrow 09:30`. Recognized remind/
snooze prefixes are removed inside the picker as well. Evening defaults to
20:00 and morning to 09:00 in the system timezone. The suggestion displays the
resolved date/time before Return confirms. After 20:00, an unspecified tonight
uses the next whole hour while still on the same day. Invalid or expired explicit
times are rejected. Existing durations and explicit date formats remain available.

## macOS window chrome

Main window hides its displayed title, uses transparent titlebar/full-size content,
and removes the titlebar separator. Its accessible/window-menu name remains Email.
Sidebar top padding reserves space for native traffic-light controls. Native titled,
resizable, closable and miniaturizable window styles remain enabled.

Linux chrome is unchanged: Hyprland already controls decorations and GNOME uses
platform chrome.

## Preserve the window size when composing

Opening Reply or Compose on macOS keeps the window's size. The reply's
preferred width and height use priority 499, below AppKit's `windowSizeStayPut`
priority (500), so together with the required 780-point editor width cap they
remain stronger than the optional message-context spacing but yield to the
user's existing window size. The reply card can stay centered with extra space
around it in a large reader.

`--smoke-composer-window`, which requires `EMAIL_DEMO=1` and an explicit
disposable `EMAIL_STORE_PATH` (plus `MAIL_SERVICE_LIBRARY` for a development
build), checks the exact outer frame through compose, close, reply, expanding
actual Cc/Bcc details, and close again, with the contact pane on and off, and
verifies editor/inbox focus. Existing content minima keep the window at least
1100 points wide without contacts and 1370 with contacts.

On Linux, `reader_viewport` isolates the shared reading/composer area behind
non-propagating External-policy GTK viewports, and the composer does not set the
outer window size.
