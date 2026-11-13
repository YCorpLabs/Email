# Gmail history without body re-downloads

## Problem observed in the source

History handling previously combined `messagesAdded`, `labelsAdded`, and
`labelsRemoved` into one message-ID set and fetched `messages.get?format=full`
for every member. Reading or archiving already-downloaded messages therefore
re-downloaded unchanged bodies. Bulk cleanup could generate significant quota
and parsing work even though the reader already had those bodies.

## Implemented

`MailAdapter::sync_with_cached_messages` receives an account-scoped index of
cached message ID → thread ID. This index contains no bodies and is collected
before network work, with the database lock released before the adapter runs.
The default trait implementation preserves existing IMAP/Graph behavior.

Gmail history divides hydration into two paths:

- Unknown message IDs fetch full bodies as before.
- Changes to known messages fetch one `threads.get?format=minimal` snapshot per
  distinct affected thread. This returns flags and labels without MIME bodies
  or recipient headers. The whole-thread label union is authoritative even
  when a history page touched only one member of a multi-message conversation.

Normalized `MessageState` updates cached read/draft/sent flags only. Body text,
HTML, recipients, unsubscribe headers and attachment metadata are preserved.
Unknown members in a minimal snapshot remain unhydrated until their own normal
full-message path runs. Pending local actions replay after provider updates, so
an older remote read/inbox snapshot cannot undo a queued local change.

The quota estimator now charges 40 units for a minimal thread request. Current
[Google usage limits](https://developers.google.com/workspace/gmail/api/reference/quota)
list 60 units for a full message and 40 for a non-full thread. Thus singleton
known-message history uses 40 rather than 60 units; multiple changed messages
in the same thread share one 40-unit snapshot. This is a request-plan/quota
improvement, **not a measured end-to-end speedup**. Gmail can still throttle
mailboxes, and initial/backfill pages still fetch full messages.

Google documents that
[thread minimal format](https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.threads/get)
returns IDs and labels without headers, body, or payload. Its
[history API](https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.history/list)
returns change records and a resumable cursor.

## Follow-up

Ordinary historical backfill pages still hydrate full messages. The periodic
newest-inbox refresh now uses the same cache-aware split described below. Missing
cached MIME still needs a dedicated hydration state rather than treating every
known ID as necessarily complete. Cross-process quota coordination and structured
per-account progress remain separate work. Service action-flush fairness is
handled independently from this provider metadata path.


## Newest-inbox refresh during import

The periodic newest-inbox check previously hydrated all 50 returned IDs again,
even if their immutable MIME was already cached. It now hydrates only unknown
message IDs, and fetches one minimal snapshot per distinct cached conversation.
A new reply in an existing thread still receives a full body fetch; the minimal
snapshot supplies the authoritative whole-thread label union and current flags.
The full-message partial label union is discarded for threads covered by a
minimal snapshot. Neither local cache reading nor pending-action overlay changes.

Request-plan regression covers two cached members plus a newly arrived reply in
one conversation, a separate new thread, duplicate list IDs, malformed list
entries and a foreign cached thread reference. Known members schedule no MIME
fetch, new messages still hydrate, and duplicate IDs/threads are fetched once.
A malformed cached reference falls back to owned-message hydration instead of
silently skipping the message. A page of 50 cached messages in one conversation
therefore schedules one minimal thread request rather than 50 full-message
requests, excluding the common list/catalog requests. Fifty different cached
conversations schedule fifty minimal requests: this is not a universal 50-fold
improvement. No claim of live quota-stress performance is made.
