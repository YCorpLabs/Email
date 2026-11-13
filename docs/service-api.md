# Native service JSON ABI (v1)

The desktop app keeps one `mail-service` library handle open. `mail_service_open(path)` opens the SQLite cache and account configuration. `mail_service_request_json(handle, json)` returns `{"ok":true,"data":...}` or `{"ok":false,"error":"..."}`. Calls can perform network I/O; run them off the UI thread.

## Read operations

- `{"op":"list_accounts"}` → array of `{id, provider, email, display_name}`.
- `{"op":"list_threads","account_id":null,"split_id":null,"unread_only":false,"limit":100,"offset":0}` → cached summaries with SQL paging and filters.
- `{"op":"list_thread_messages","thread_id":"..."}` → complete cached conversation, oldest first.
- `{"op":"search","query":"...","limit":100}` → cached summaries.
- `{"op":"get_latest_message_for_thread","thread_id":"..."}` → cached message or null, with raw `body_plain` and a derived `notification_snippet` (≤180 characters, see [desktop notifications](desktop-notifications.md)); do not insert raw `body_html` into a webview.
- `{"op":"render_message","message_id":"..."}` → `{body_plain, body_html_safe}` where safe HTML may be null.
- `{"op":"render_message","message_id":"...","include_inline_images":true}` → the same result with bounded same-message `cid:` images resolved to safe `data:` URLs. This may fetch attachments; request it asynchronously after the cached render.
- `{"op":"pending_actions"}` → queued actions.
- `{"op":"list_drafts","account_id":null}` → cached draft messages.
- `{"op":"list_labels"}` → labels with cached counts.
- `{"op":"list_splits"}` → saved inbox searches.
- `{"op":"list_snoozed_threads"}` → threads awaiting a local wake time.
- `{"op":"list_outbox"}` → scheduled and undoable sends with status and due time.
- `{"op":"list_attachments","message_id":"..."}` and `{"op":"fetch_attachment","message_id":"...","part_id":"..."}` → attachment metadata and fetched bytes for explicit preview/save.
- `{"op":"list_draft_attachments","message_id":"..."}` → local draft attachment metadata.

## Account setup

- `{"op":"add_imap_account","provider":"icloud"|"imap","email":"...","display_name":"...","username":"...","imap_host":"...","imap_port":993,"smtp_host":"...","smtp_port":587,"password":"..."}` → account object. The password goes to the OS credential store; never SQLite/logs. The service tests credentials before saving the account.
- `{"op":"start_oauth","provider":"gmail"|"microsoft","client_id":"..."}` → account object after system-browser OAuth with PKCE and a local loopback callback. This call blocks while the user authorizes; run it on a background thread.
- `{"op":"remove_account","account_id":"..."}` → null; removes local configuration, cache, and stored credential after explicit UI confirmation. For a Google sign-in account it first asks Google to revoke the grant. That step is best effort with a 4-second timeout; removal finishes even if it fails. This call can block on the network; run it on a background thread.

## Mail operations

- `{"op":"sync_account","account_id":"..."}` → `{messages_added, actions_applied, pending_actions, actions_retry_after_ms, warnings}`; see synchronization results below.
- `{"op":"sync_all"}` → array of `{account_id, ok, data}` or `{account_id, ok:false, error}`. Accounts run in up to four concurrent lanes; result order follows the account list.
- `{"op":"compose_draft","account_id":"...","to":"...","subject":"...","body_plain":"..."}` → local draft.
- `{"op":"update_draft","message_id":"...","to":"...","subject":"...","body_plain":"..."}` → edited local draft.
- `{"op":"send_draft","message_id":"..."}` → provider send result. Must only report success after provider accepts the message; errors leave the draft intact.
- `{"op":"add_draft_attachment","message_id":"...","filename":"...","mime_type":"...","content_base64":"..."}` and `{"op":"remove_draft_attachment","message_id":"...","attachment_id":"..."}` → add/remove bounded local attachment bytes.
- `{"op":"discard_draft","message_id":"..."}` → `{message_id, account_id, subject, discarded_at_ms, restorable_until_ms}`. Removes an unsent local draft, its attachments and search entry; a reply draft leaves its conversation intact. Scheduled/queued drafts must be cancelled first, and provider-stored drafts are refused. A private snapshot is kept for 7 days.
- `{"op":"restore_discarded_draft","message_id":"..."}` → restored draft, with attachments and open-tracking choice. Single use; a reply whose conversation disappeared returns as a standalone draft.
- `{"op":"schedule_draft","message_id":"...","send_at_ms":...}` → place a saved draft in the durable outbox.
- `{"op":"queue_send_draft","message_id":"...","undo_seconds":10}` → place a saved draft in the outbox with an undo delay.
- `{"op":"cancel_scheduled_draft","message_id":"..."}` → return an unclaimed outbox item to drafts.
- `{"op":"dispatch_due"}` → atomically claim and dispatch due items; a timer or the desktop UI calls this.
- `{"op":"snooze_thread","thread_id":"...","wake_at_ms":...}` and `{"op":"unsnooze_thread","thread_id":"..."}` → locally hide/restore a thread.
- `{"op":"upsert_split","id":null,"name":"...","query":"..."}` and `{"op":"delete_split","id":"..."}` → manage saved inbox searches.
- `{"op":"apply_action","operation_id":"...","action":{...}}` → local queued action (existing `mail-core` action schema).

Agent access operations are for the desktop shells only. `mail-mcp` has no
policy rule for them, so it refuses them ([mcp-security.md](mcp-security.md)).

- `{"op":"agent_access_status"}` → accounts with their consent (`null` when off), `current`, `grant_labels`, and the pending summary.
- `{"op":"agent_access_disclosure","account_id":"..."}` → the disclosure both shells render: `version`, `title`, `change_title`, `intro`, `data_categories`, `shared_note`, `provider_terms`, `safeguards`, `grants_heading`, `read_label`, `grants[{id,label,detail}]`, `revoke`, `confirm`, `change_confirm`.
- `{"op":"grant_agent_access","account_id":"...","grants":["draft","send"],"disclosure_version":1}` → stored consent. Narrowing grants rejects pending requests they no longer cover.
- `{"op":"revoke_agent_access","account_id":"..."}` → `{account_id, revoked}`; rejects that account's pending requests.
- `{"op":"agent_pending_summary"}` → `{count, items, blocked, revision}`. `blocked` is the latest refusal that still needs action, with `affected_account_ids` and `needs_access`; `revision` changes whenever a request, decision, activity or consent changes.
- `{"op":"agent_access_overview","history_limit":60}` → the status plus `requests` (waiting, oldest first) and `history` (newest first).
- `{"op":"list_agent_actions","status":"pending"|"history"|...,"limit":200}` → agent actions.
- `{"op":"approve_agent_action","id":"...","undo_seconds":10}` → the approved action. The request is applied in the same transaction; sends are queued with the undo window. A request that can no longer apply closes as failed and returns an error.
- `{"op":"reject_agent_action","id":"..."}` → the rejected action.

The API is a target contract. Operations not implemented yet must return an explicit error, never silently simulate provider success.

## Unread counts and bulk sender cleanup

- `{"op":"unread_counts"}` → `{total, accounts: {account_id: count}, scope, followups: {active, due}}`. Inbox counts cover downloaded unread conversations; follow-up counts use the saved reminder state and evaluate due deadlines against the current clock.
  Counts downloaded inbox conversations, excluding snoozed conversations until due.
- `{"op":"unread_from_sender","account_id":"…","sender":"Name <address@example.org>"}`
  → all matching downloaded unread thread IDs, independent of pagination.
- `{"op":"bulk_mark_read","operation_id":"bulk:<uuid>","account_id":"…","thread_ids":["…"]}`
  → number of newly queued thread actions. Account ownership is validated before
  the atomic optimistic mutation; retry the same operation ID for idempotency.
  Pass a `bulk:` prefix to enable Gmail batch sync. Call normal `sync_account`
  in the background after saving; failure leaves the pending actions durable.

See `docs/unread-cleanup.md` for exact scope and verification limitations.

Search optionally accepts `is_inbox: true|false` to filter before the result
limit. Omit it for global search (backwards compatible).

### Atomic selected-conversation triage

`{"op":"batch_triage","operation_id":"unique-client-id","kind":"archive","thread_ids":["thread-id"]}`

Kinds: archive, restore, spam, read, unread. Maximum 20,000 IDs. Validates all
IDs and commits local state plus queued actions atomically. Identical retries
return changed=0; conflicting operation-ID reuse fails. Response includes
changed and undoable. This does not perform remote synchronization itself.

`{"op":"undo_triage","operation_id":"unique-client-id"}` restores original
folders and queues inverse moves for archive/restore/spam batches. It refuses
to overwrite newer local actions; successful repeats return changed=0.


### Discovering durable undo

`{"op":"latest_triage_undo"}` → `{operation_id, kind, count, account_ids}` or
null. This local read survives process restarts and examines the latest 100
non-undone placement batches. It skips batches whose conversations have newer
local actions, or no longer exist. Pass the returned operation ID to
`undo_triage`; that write revalidates the batch transactionally. Discovery does
not mutate mail or contact a provider. Read/unread batches do not offer undo.

## Local plain-text snippets

- `{"op":"list_snippets"}` → array of `{id, name, body}`, sorted by name.
- `{"op":"upsert_snippet","id":null,"name":"Thanks","body":"Thank you!"}` → saved snippet. Supply an existing ID to edit; an unknown ID is an error.
- `{"op":"delete_snippet","id":"..."}` → null; an unknown ID is an error.

These operations use local SQLite only; they invoke no AI and make no network
requests. Names are trimmed, must contain 1–80 printable characters, and must
be unique under SQLite's ASCII case-insensitive comparison. Body text must be
nonblank, contain no NUL, and fit within 65,536 UTF-8 bytes. At most 500 snippets
can be saved. Editing or deleting a snippet does not alter drafts that already
contain inserted text.

## Synchronization results

Incoming mail refresh runs before a bounded flush of queued changes. Pending
local intents remain overlaid on provider snapshots. A successful service call
does **not** mean every queued provider mutation has finished:

- `pending_actions` counts this account's durable actions still awaiting provider acceptance.
- `actions_retry_after_ms` is a suggested delay before another pass when work remains or an incoming refresh failed. It is at least 5,000 ms and can increase for provider throttling/errors; it is not an internal timer or completion deadline.
- `warnings` reports nonfatal conditions, including queued-action failures and refreshed-credential persistence failures.
- `sync_error`, when present, is the incoming-refresh error. Cached mail remains readable, and the action flush may still have made progress. Inspect this field even when the JSON ABI envelope has `ok:true`.
- `sync_in_progress:true` means another handle already holds this account's sync lease. This call did not perform another refresh/flush; it returns zero work counts, the current pending count and a 5,000 ms retry hint.

For `sync_all`, a row with `data.sync_error` has `ok:false` while retaining its
partial progress in `data`. Setup/database errors instead appear as `error`.
A row can have `ok:true` and still have pending actions or warnings. Clients
should inspect all these fields, preserve cached mail, and schedule later passes
rather than treating partial progress as either complete success or lost mail.

### Composer recipient lists

`compose_draft` and `update_draft` accept optional `cc` and `bcc` mailbox-list strings, with the same quoted-name/group syntax as `to`. Draft responses persist them in `recipient_headers.cc` and `.bcc` arrays. Omitting a field during update preserves it; an explicit empty string clears it. Drafts can hold incomplete input, but immediate send and scheduling validate every recipient list before claiming the draft. At least one address across To, Cc and Bcc is required; Bcc-only messages are supported.

SMTP builds its delivery envelope from all three lists and removes Bcc from transmitted message headers. Gmail's private raw-message API submission includes Bcc because Google derives delivery recipients from those headers; Microsoft receives Bcc separately in `bccRecipients`. Provider delivery has not been exercised by these local transport tests.

## Resumable provider-wide sender cleanup

Currently Gmail only. The service never accepts an arbitrary Gmail query for this workflow.

- `{"op":"create_sender_cleanup","account_id":"…","sender":"news@example.com"}` → durable discovery job; no network mutation.
- `{"op":"list_sender_cleanups","account_id":null}` → up to 100 jobs, active first; omit the account filter for all local accounts.
- `{"op":"get_sender_cleanup","job_id":"…"}` → current job and counters.
- `{"op":"step_sender_cleanup","job_id":"…"}` → one discovery page or one apply batch, at most 500 IDs. Can perform network I/O; call off the UI thread. Returns unchanged progress when paused, awaiting confirmation, leased or cooling down.
- `{"op":"confirm_sender_cleanup","job_id":"…"}` → confirms a finished discovery snapshot, moving ready to running (or completed if empty). Does not itself send a provider request.
- `{"op":"pause_sender_cleanup","job_id":"…"}`, `resume_sender_cleanup`, `cancel_sender_cleanup` → change future execution; in-flight work can still commit.

Job responses contain `id`, `account_id`, `sender`, `state`, `phase`, `page_token`, `created_at_ms`, `last_error`, `retry_at_ms`, `matched`, `completed`, `skipped`, and cached `sample_subjects`. States are discovering, ready, running, paused, completed or cancelled; phases are scan or apply. `completed` counts successfully applied snapshot IDs, `skipped` counts protected later local changes. Failures normally return the job with a safe error and cooldown; callers must inspect those fields rather than treating an ABI success as provider completion.

Discovery excludes Spam/Trash and deduplicates at most 100,000 IDs. Apply removes only UNREAD, without fetching every body or changing placement/other labels. New mail is not added after discovery. Progress and two-minute leases survive restart; a caller continues stepping to resume. There are at most 50 unfinished jobs. Pause/cancel do not reverse successful batches. See [unread-cleanup.md](unread-cleanup.md) for concurrent-mailbox and eventual reconciliation limitations, plus synthetic test coverage.

## Scoped cached search and listings

`search` additionally accepts optional `offset`, `account_id`, `unread_only`,
`starred_only`, `label`, `folder`, `split_id`, and `snoozed_only` fields. Existing
`query`, `limit` and `is_inbox` fields remain supported. All supplied restrictions
intersect and are applied in SQLite before pagination; no provider request is
made. Missing boolean filters default to false. Omitting account/folder filters
preserves global scope. Use `folder:"archive"` for the semantic archive view;
`is_inbox:false` retains its broader meaning of any non-inbox conversation.

An empty `query` returns a scoped cached listing, allowing mailbox views to use
these same filters without filtering an already limited result array. A missing
or unmatched account yields no rows; it never falls back to global results.
`offset` defaults to zero and `limit` defaults to 100 (bounded by the core).


## Recent recipient suggestions

`{"op":"suggest_recipients","account_id":"gmail:…","query":"ja","limit":6}`
returns an array such as
`[{"email":"jane@example.org","display_name":"Jane Doe","formatted":"\"Jane Doe\" <jane@example.org>"}]`.
Insert `formatted` as one mailbox; display `display_name` and `email` separately.
The selected sending `account_id` is required; unknown accounts fail closed.
No provider, Google People, operating-system contacts, or message-body lookup occurs.

Coverage is **recent correspondents**, not a complete address book: up to 5,000
latest cached messages/drafts from that account, with at most 512 matching records
parsed. Sender, To and Cc contribute; Bcc contributes only from explicitly saved
local drafts, never from received/sent messages. Invalid/incomplete mailboxes,
control characters and oversized metadata are ignored. De-duplication uses a
case-insensitive mailbox; a known name is preferred over a bare address.

Exact email matches rank first, followed by email prefixes, name/word prefixes,
and substrings; recency then occurrence count and email break ties. Query text is
literal (no SQL, wildcard or FTS syntax), trimmed and Unicode lowercased, with a
128-byte limit and no control characters. Empty queries return `[]`; `limit`
defaults to 6, zero returns `[]`, and positive values are capped at 20.
The native composers and read-only `suggest_recipients` MCP tool use this local
API; the MCP wrapper additionally enforces its account allowlist. No write, send
or network permission is implied.


## Invitations, follow-ups and identity signatures

- `list_calendar_invitations {account_id?,limit?}` and `calendar_invitation {message_id}` read only the local invitation-document cache.
- `refresh_calendar_invitation {message_id}` explicitly fetches bounded calendar attachment data using existing mail-provider credentials. It never RSVPs, creates provider events or fetches arbitrary URLs. A failed/malformed refresh preserves saved invitation data.
- `set_followup {thread_id,due_at_ms,only_if_no_reply,sent_message_id?}`, `list_followups {account_id?}`, `list_followup_threads {account_id?}`, `cancel_followup {thread_id}` persist device-local reminders. The optional tracked sent-message anchor checks same-account conversation ownership and reply eligibility atomically with reminder creation; a reply downloaded since the activity panel loaded refuses the new reminder. Omit the anchor for the existing general-purpose reminder behavior. Lifecycle and limitations: [followups](followups.md).
- Structured `search` accepts `followups_only:true`; it evaluates local reminder state, includes active waiting/due threads from any mail folder, respects normal account/query filters and sorts by reminder due time.
- `get_signature {account_id,email}`, `upsert_signature {account_id,email,body,for_new,for_reply}`, `delete_signature {account_id,email}` manage local plaintext defaults for each identity. These preferences never authorize sending as an unverified alias.

Live provider calendars, recurrence expansion, conflict detection, availability sharing, booking and RSVP delivery are not supplied by the invitation cache.


## Connected calendar editing and unified views

- `list_calendars {account_id?}` lists cached sources; omission combines accounts.
- `list_agenda {account_id?,start_ms,end_ms,limit?,deduplicate_shared?,hidden_sources?}` reads cached events. `hidden_sources` is an array of `{account_id,calendar_id}`. Filtering precedes deduplication. With `deduplicate_shared:true`, identical non-primary calendar IDs, event IDs and occurrence times across accounts appear once, preferring an editable copy. Names alone never establish shared identity. `primary` is account-relative and never deduplicated. Existing 10,000-event cache retrieval bounds apply.
- `create_calendar_event {account_id,calendar_id,title,start_ms,end_ms,attendee_emails,description?,location?}` creates a timed event. Success may contain a persistence/cache warning; do not blindly retry an ambiguous response.
- `update_calendar_event {account_id,calendar_id,event_id,expected_etag,edit_scope?,title?,start_ms?,end_ms?,description?,location?}` patches an editable timed event. Start/end must be supplied together. Empty description/location clears that field; omission preserves it. Guests are preserved. `edit_scope` defaults to `single`; recurring occurrences require explicit `instance`. Series and all-day changes are currently unsupported. The provider version must equal `expected_etag`; the PATCH also uses `If-Match`. A conflict requires refresh and user review. Guests receive Google's update notification.
- Event data includes `can_edit` and `recurring_event_id`; old saved data defaults to non-editable until refreshed. Calendar write permission and provider access are verified again at mutation time.

MCP exposes create/update under the independent `--allow-calendar-write` capability. Mail-send and mail-write grants do not imply calendar write access; account restrictions remain enforced.


`compose_draft` accepts optional
`reply_thread_id`. It must identify a conversation in the chosen account.
Explicit recovered conversation links resolve to the original thread. This
preserves conversation membership; subject text alone never selects a thread.

### Hosted scheduling and email activity

- `booking_status` returns connection status and cached host identity without a provider refresh.
- `booking_connect_start {name?}` returns a short-lived browser verification URL and a fallback user code. The URL must match the configured HTTPS origin, `/device` path and exact `#code=` fragment. The fragment is cleared by the browser page immediately.
- `booking_connect_poll` completes a PKCE-bound, explicitly approved request; `booking_disconnect` revokes this device.
- `booking_list {refresh:false}` returns host, published meeting types, bookings, analytics and cache metadata. `booking_analytics {refresh:false}` reads aggregate booking analytics.
- `booking_link {meeting_type_id}` returns an email-attributed published booking URL. `booking_availability {meeting_type_id}` checks fresh availability and returns suggested slots with booking URLs.
- `tracking_get {message_id}` reads local draft tracking state. `tracking_set {message_id,enabled}` registers an opaque tracking identifier only when enabled; it cannot alter queued/sent messages. Neither recipient nor message content is sent to the tracking service.
- `tracking_activity {message_id?,refresh:false}` returns local or refreshed tracking-image request counts, first/last timestamps and local conversation context. Candidate activity items also include `account_id`, `thread_id`, `subject`, `recipient`, `sent_at_ms`, `last_activity_at_ms`, `tracking_status`, cached reply evidence, existing `followup` state and a bounded `timeline` of `{kind,at_ms,count?}` events. These context fields are assembled locally, never uploaded to the tracker. `reply_coverage: "cached"` does not establish complete provider coverage. `timeline_complete` distinguishes retained real request history from legacy totals or truncated history; old totals are not expanded into invented events. Root `refreshed_at_ms` is nullable. An image request is not a verified read. The existing request and MCP authorization requirements remain unchanged.

Network operations run outside the cached-mail UI request lane. Desktop credentials remain in the OS credential store and are not returned by these APIs. Open-tracking retention and history limits are pinned in `contracts/scheduling.json` (`tracking`): message scope, no recipient attribution, minute grouping and 90-day retention.

Activity candidate detail: `sent_time_source` is `provider_acceptance` for newly
accepted tracked sends and `cached_message` for legacy/provider history.
`reply_scan_limited` identifies a bounded candidate scan; replies are evaluated
only from downloaded same-account conversation messages. `refreshed_at_ms` at the
response root is the oldest known item check only when all returned items have a
known check; otherwise it is null. It never treats one fresh detail as a fresh
whole feed. No request-history buckets are manufactured from first/last totals.
`reply_baseline_available` distinguishes new tracked sends with a persisted
already-cached-message baseline from legacy timestamp-only reply evidence.
