# Reader responsiveness during provider work

## Background

macOS routed `render_message` with `include_inline_images=true` through the
primary CoreBridge. CID image resolution performs provider attachment requests;
its pacing/network wait held that bridge's request lock and delayed subsequent
local reads (and synchronous cache queries). Sending, due-send dispatch and
send-as discovery also shared this path. Separate sync bridges alone did not
protect cached reading.

Repeated actions also queued overlapping sync calls. Linux creates a service per
background worker, so overlapping workers could flush the same provider queue.

## Changes

- Mac uses independent connections for local reads, CID enrichment, attachment
  operations, headers, sync, alias discovery and outgoing delivery. Remote paths
  fail rather than falling back onto the cache connection if a bridge cannot open.
- Mac overlapping sync calls share an in-flight task per account (and share an
  active all-account sync). They do not build an unbounded mutex-waiter queue.
- Shared service uses a nonblocking per-store/account lease to coalesce overlapping
  syncs across handles in the same process. Other accounts/stores are independent.
  The lease releases on success/error. This does not coordinate other processes.
- Optional headers, CID images and attachment metadata wait 250 ms on navigation;
  obsolete selections are skipped in both shells. Mac also checks between remote
  requests and reuses resolved CID bodies within its bounded rendered-body cache.
- Bulk cleanup sync failures after a successful local commit use persistent status
  text rather than another blocking alert. Pending actions stay durable.

Provider waits can still delay remote images, metadata and changes appearing at
Gmail. These changes isolate cached reading; they do not remove provider limits.

Tests pause a provider sync and require cached thread/count reads to finish
within 500 ms before releasing the provider, and check same-account lease
coalescing, independent accounts/stores and release/reacquisition.

## Archive while searching

Inbox search previously used global FTS results. An optimistic archive removed
the row, but the next query returned the archived thread again. Search now
accepts optional `is_inbox`, applied in SQL before LIMIT; omitted preserves
global MCP/client search. Mac Inbox searches request true; Linux requests true
outside Archive and false inside Archive. Existing dedicated Mac archive search
continues to use the archived mailbox.

## Stable archive shortcut

E previously toggled into restore in Archive view. Both shells now leave
archived mail archived on plain E and show an explanatory status. Shift+E
explicitly restores in Archive; the restore button and context action remain
available. Read state intentionally remains independent of archive.

## Fair sync passes and metadata-only queue routing

Incoming refresh now runs before draining pending triage. The core's pending
intent overlay remains responsible for preventing fetched provider state from
reverting local actions. A refresh failure is reported as `sync_error` while
queued actions still receive a bounded attempt. Provider action failures leave
work queued and stop that account's flush at the failed action; later moves,
read changes and undo inverses cannot overtake it.

Each outgoing pass starts at most 40 provider operations and stops starting new
ones after two seconds. This is checked **between** operations: an individual
network/IMAP operation can still run until its own timeout, and an operation may
internally need more than one HTTP request (for example label resolution or token
refresh). Gmail bulk read groups stay at most 1,000 cached message IDs; an
exceptionally large single thread uses the thread endpoint. Archive keeps
thread-level semantics rather than incorrectly archiving only downloaded messages.

`sync_all` uses up to four concurrent account lanes, preserving response order
and reporting account failures independently. Queues are loaded as an ordered,
account-specific prefix of at most 2,048 actions, rather than deserializing every
account's entire queue. Account routing and message-ID enumeration select only
account/ID columns; they do not hydrate HTML or message recipients/bodies.

Responses include `pending_actions`, `actions_applied`, `actions_retry_after_ms`
and `warnings`. The normal pending-work retry suggestion is 5,000 ms; a provider
retry-after interval is respected, with a conservative 60,000 ms default after
errors. Ordinary idle refresh remains a UI responsibility. These bounds improve
fairness; they do not establish a specific end-to-end quota-limited cleanup time.

## Plain-text reader

Plain-text bodies now have a left-aligned reading column, preserved whitespace,
comfortable line height and top padding in both native shells. Mac plain mail
bypasses newsletter scaling and resolves the native reader color into sRGB so
its WebKit background blends with the header instead of forming a dark block.
Markup remains escaped. Mixed HTML/plain threads keep the HTML document layout.
