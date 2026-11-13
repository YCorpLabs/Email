# Local follow-up reminders

Follow-ups keep conversations in a dedicated local view without moving the mail
or changing its provider labels. A reminder may be unconditional, or end when a
new incoming message appears in the same cached thread. This is distinct from
snooze: archiving a conversation does not remove its active follow-up.

## Lifecycle

- `waiting`: the reminder is active and its deadline is in the future.
- `due`: its deadline has passed. This is not a notification-delivery receipt.
- `replied`: a conditional reminder observed an eligible incoming message.
- `cancelled`: the user or an authorized client explicitly cancelled it.

Setting a reminder again resets its baseline and returns it to `waiting`.
Cancelled and replied records stay terminal unless explicitly set again. Deleting
a downloaded reply does not resurrect a completed reminder.

The local service exposes `set_followup`, `list_followups`,
`list_followup_threads` and `cancel_followup`. `list_followup_threads` returns only
waiting/due conversations, in due order, including archived mail. The same
reader and navigation can open them without restoring them to the Inbox.

Deadlines are persisted as epoch milliseconds and must be in the future, within
five years. Evaluating the list updates due/replied status transactionally. A
reminder that becomes due while the app is closed appears due when evaluated
after reopening; this does not guarantee an alert while the computer is off.

## Reply detection and limits

The baseline snapshots IDs already cached when the reminder is set. A reply must
be newly cached, non-draft, non-sent, in the same account/thread, and have a
received timestamp after reminder creation. This prevents newly imported old
history and existing future-dated messages from accidentally ending a reminder.

The Email activity candidate resolves explicitly linked local conversations in
baseline capture and reply evaluation, retaining the same account boundary. It
does not match conversations by subject. An optional `set_followup.sent_message_id`
anchors a conditional reminder to an already-tracked sent message. Ownership and
cached reply eligibility are checked in the same SQLite write transaction as
creation; an intervening reply refuses the action without replacing an existing
reminder. A bounded/incomplete reply scan refuses guarded creation. The ordinary
unanchored reminder contract remains available.

Connected account addresses and recent provider-marked sent addresses are treated
as own identities. Unsent draft From text is not proof of ownership. Core callers
may additionally provide provider-verified identities with
`set_followup_with_own_addresses`; saving a signature does not verify an identity.

Important limits:

- Detection uses the downloaded cache; a provider reply not synced yet cannot be
  recognized until sync succeeds.
- It recognizes incoming messages in a thread, not verified `In-Reply-To` or
  `Auto-Submitted` semantics. An automatic reply in that thread may count.
- Up to 512 candidate senders are evaluated per reminder per call. Skipped IDs
  are recorded so later calls advance without blocking forever behind a large
  batch of own or malformed messages. Late imports do not fall behind a timestamp
  cursor.
- At most 1,000 reminders can be active. Lists return active records first and
  recent terminal history, with a maximum of 2,000 records.
- Reminder evaluation does not itself restore Inbox labels, mark read, send mail
  or deliver a notification.

## Security and durability

Follow-up changes are local transactions using immediate write reservation, so
separate desktop and MCP database connections do not race baseline snapshots.
Account ownership comes from the actual thread. Account-filtered lists constrain
the SQL query before returning data. Cascading foreign keys remove reminder and
baseline records when the source thread/account is removed.

MCP `set_followup` and `cancel_followup` require `--allow-triage` (or legacy
`--allow-write`). Listing requires no mutation grant, but an account-restricted
server requires an explicit permitted account. Thread IDs are checked against
the account allowlist before dispatch. No follow-up operation grants send, sync
or signature-setting permission.

No network request or provider mutation is needed to create, list, cancel or
evaluate follow-ups. Malformed sender lists and headers containing controls are
not accepted as replies. Indexed account/due and terminal-history queries avoid
sorting all historical reminders during refresh.

## Inbox counters and desktop alerts

`unread_counts` includes `followups.active` and `followups.due`. Counts are read-only; reminder listing
and desktop evaluation reconcile cached replies, while due counts compare the
deadline with the current clock. The inbox total counts downloaded unread
inbox conversations, excludes snoozed conversations until wake time, and does not
pretend to count provider mail that has not been downloaded. Desktop sidebars
show the inbox unread total and due follow-up count; the follow-up tooltip also
reports all active reminders. The Unread filter stays visible alongside the
separate selection/undo action row.
