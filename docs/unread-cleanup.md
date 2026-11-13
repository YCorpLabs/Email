# Unread counts and sender cleanup

Unread badges count downloaded inbox conversations, excluding snoozed conversations until due. They are independent of the visible page, but are not provider-wide unread message counts. Unsynced mail can change the total when downloaded.

## Provider-wide Gmail cleanup

The cleanup workflow enumerates unread messages from one sender in one Gmail account, including messages not downloaded locally. It excludes Spam and Trash. A validated literal mailbox is passed to Gmail's sender-search query; no arbitrary query operators are accepted. This is an explicit sender operation, not AI newsletter classification.

1. Create a cleanup and step discovery. Each step requests at most 500 IDs.
2. Review the completed snapshot and confirm it. No remote mutation occurs before confirmation.
3. Step the confirmed cleanup. Each apply request removes only Gmail's `UNREAD` label from at most 500 snapshot IDs. It does not archive, delete, unsubscribe or change other labels.

The snapshot is deduplicated, account scoped and capped at 100,000 distinct messages. New arrivals after discovery are not added. Gmail search pagination is not a point-in-time mailbox transaction: concurrent external mailbox changes can affect discovery results. Preview subjects come only from locally cached snapshot members; discovery does not download every message body.

Progress, cooldowns and leases persist in SQLite. Paused work resumes from its saved page or remaining IDs. Cancellation stops future steps; it cannot undo completed batches or recall an in-flight request. An already-running step can still commit after pause or cancellation. Expired two-minute leases permit recovery after a crash. At most 50 unfinished jobs are allowed; lists show active jobs before recent completed history.

A step shares the service's in-process account sync guard and quota pacing. Before applying cleanup it tries a bounded flush of earlier pending account actions; remaining work postpones cleanup. Transient failures preserve the snapshot and set a visible retry time. Cached mail remains readable during cooldowns. The caller must continue stepping; creation and confirmation do not start an independent server daemon.

Later local user actions protect cached messages from being overwritten by cleanup. If explicit unread intent was already delivered by a concurrent worker while cleanup was in flight, successful completion queues a fresh unread compensation. This is eventual reconciliation, not cross-process serialization of provider requests. A crash or lost response after provider success can require recovery/retry before reconciliation completes. Changes made in another mail client after discovery cannot be detected from the local action log.

## Downloaded-only operations

`unread_from_sender` and `bulk_mark_read` remain available for downloaded conversations. The former resolves a sender mailbox ignoring case and display-name changes, includes cached archive as well as inbox, excludes junk/trash/drafts, and never combines accounts. Bulk requests validate ownership and atomically update cached state plus durable actions; the same operation ID is idempotent. The limit is 20,000 threads. Normal synchronization delivers these actions.

Gmail's downloaded-mail queue can coalesce contiguous bulk read actions into requests of at most 1,000 cached message IDs, preserving later unread ordering. This is separate from the provider cleanup workflow's 500-message step bound.

## Verification

Eight core cleanup tests cover confirmation, restart recovery, bounded snapshots, deduplication at the 100,000-message cap, pause/cancel and cooldowns, cross-account rejection, stale leases, active-job visibility and concurrent confirmed unread compensation. Service tests exercise discovery without mutation and failed-provider cooldowns. Cloud tests validate literal sender queries and malformed/account-scoped provider pages. These checks use synthetic data and mock operations.

## Cleanup panel

Both native shells show cleanup in a compact, 520 × 300 point/pixel
command-style panel. It has a single
header, sender/account identity row, right-aligned state, compact count/outcome,
expandable downloaded samples, an explicit Done action and Escape dismissal.
Cancelled jobs with zero completions say “No messages changed.” Terminal states
have no empty action strip. Sample disclosure state survives job refreshes.

The Mac scroll document now uses flipped coordinates, keeping short content at
the top instead of the bottom of a tall viewport. Samples and multiple jobs scroll
inside the panel, rather than increasing the outer app window.

## Bulk command routing and discoverability

The primary command is now **Bulk mark sender as read…**. Searching `bulk`,
`mass read` or `mark all read` prioritizes the selected-sender action. Cleanup
history no longer carries the ambiguous bulk alias. Gmail opens the read-only
sender discovery preview directly, removing the extra all-vs-downloaded dialog.
The explicit **Mark downloaded mail from sender as read…** command remains
available. With a selection, bulk searches show the selected-message read action.
An empty cleanup history offers **Clean up selected sender…**, including Return
activation, instead of sending the user back to hunt through another menu.

The underlying provider workflow still requires count review and explicit
confirmation. A zero-match job offers no mark action.
