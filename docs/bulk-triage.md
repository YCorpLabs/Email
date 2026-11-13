# Bulk triage

## Interaction

Mac and Linux have independent marked selections, keyed by conversation ID,
separate from the conversation currently displayed in the reader.

- X toggles the focused conversation; Shift+J/K includes the current and next/previous conversation.
- Command+A (Mac) / Ctrl+A (Linux), outside an editable field, selects all currently loaded results.
- M marks the selection read; U marks it unread; E archives it.
- In Archive, E remains a no-op; Shift+E restores the selection to Inbox.
- The spam action applies to the marked selection, or the focused conversation when no selection exists.
- Z undoes the latest still-valid archive/restore/spam batch, including after restarting the app.
- Escape clears the marked selection. Search/scope changes clear marks; refresh retains IDs still in the result set.

The selection strip replaces filter tabs without inserting another row. It offers
selection count, select-loaded, read, archive, undo, and clear. Undo metadata is
stored in SQLite. Both shells restore the newest still-valid undo handle on startup; undo then offers the previous valid batch. A newer conflicting local action invalidates the affected batch.

## Persistence and synchronization

`batch_triage` validates every member before changing any of them. The local
changes, provider action queue, immutable operation request, and inverse folder
snapshots commit in one SQLite transaction. The UI refreshes after that local
commit; provider work happens afterward. This is local atomicity, not a promise
that multiple remote providers apply changes atomically.

Operation IDs make identical retries idempotent; conflicting reuse fails. Undo
queues inverse moves after the original actions and refuses the whole undo if a
newer local action touched any member. Read/unread does not offer undo because
threads can contain mixed per-message read states. Gmail selected-read actions
can use the existing bulk read grouping; archive keeps thread-level provider
semantics, including messages not downloaded locally.

## Remaining work

Selecting every provider-side search match (beyond downloaded results), read-state
undo and a browsable undo-history screen are not implemented. Checkboxes use
neutral styling; not every third-party GTK theme has been tested. Labels, snooze, favorites, and reply still target the focused
conversation, rather than being bulk actions. Bulk changes have not been tested
end to end against live provider accounts.

## Batch actions

- Batch actions are explicit in the command palette. Right-click outside a marked
  set clears marks; right-click inside it shows count-labelled batch actions only.
- Escape clears marks while retaining available undo. Clearing an already-empty
  selection dismisses the undo affordance.

## Gmail move and undo placement

An explicit Move to Inbox previously added INBOX without removing SPAM/TRASH.
The local undo appeared successful, but subsequent Gmail snapshots still carried
Spam/Trash; provider reconciliation correctly prioritized those labels and the
conversation bounced out of Inbox again. Explicit moves now replace conflicting
placement labels: Inbox removes Spam/Trash; Archive removes Inbox/Spam/Trash;
Spam removes Inbox/Trash; Trash removes Inbox/Spam. Custom-folder moves clear
these placement labels while applying the destination user label. Ordinary
Archive retains its existing remove-INBOX-only semantics.

Read/unread, stars and user labels are preserved.

Google documents applying/removing system labels through
[threads.modify](https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.threads/modify)
and identifies Spam/Trash as applicable system labels in
[Manage labels](https://developers.google.com/workspace/gmail/api/guides/labels).

## Header layout

The Mac scope tabs and triage status/actions are now arranged vertically in one
stack, rather than independently anchored to the same search-field edge. A late
sidebar refresh no longer re-shows tabs on top of the undo controls. Linux keeps
its filters visible above its independent triage row.
