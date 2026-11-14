# Local MCP server

`mail-mcp` is an stdio MCP server backed by the same SQLite cache and `mail-service` provider configuration as the desktop apps. It exposes accounts, threads and complete conversations, search, sanitized message rendering, attachments, labels, saved inbox splits, snoozed mail, drafts, and the scheduled outbox. Mutating tools cover provider sync, triage, labels, draft editing and attachments, scheduling, cancellation, and explicit sending. AI stays outside the desktop app; an MCP host can use these tools when you connect it.

## Agent access and approvals

Every account is off for agents until you turn it on in the desktop app.
Open **Agent access** in the sidebar (also in the command palette and in
Manage accounts) and pick what agents can do with an account, or choose
**Allow…** on an agent's request. The turn-on sheet offers Read only (the
default), Read and organize, and Read, organize and write, with Custom… for
the individual permissions; it says what an agent can read (with the full
list one click away), that the agent's AI provider's terms then apply, and how
to turn access off. Turning access off applies to the agent's next request.
[agent-access.md](agent-access.md) describes the windows.

- **Reading** an account needs agent access for it. List and search tools
  return only accounts with agent access.
- **Changing** an account also needs the matching in-app permission: Drafts,
  Archive and snooze, Ask to send, Fetch mail or Calendar changes.
  Command-line flags still apply; neither can widen the other. A registration
  made with **Connect** in Agent access passes every mail flag, so the in-app
  permissions decide.
- **Sending and bulk changes wait for you.** `send_draft`, `queue_send_draft`,
  `schedule_draft`, `batch_triage`, `undo_triage` and `confirm_sender_cleanup` return
  `{approval_id, status: "pending_approval"}`. Nothing happens until you
  approve it under **Agent requests** in the main window. Approved sends keep
  the 10-second undo window. **Edit** closes a send request without sending
  (status `edited`) and opens the draft in your composer. Agents check the
  outcome with `get_agent_action` or `list_agent_actions`.
- **Email content is labelled untrusted.** Tools that return messages,
  subjects, attachments, calendar text or other third-party content wrap it in
  an envelope (`content_notice`, `content_trust`, `data`,
  `end_of_untrusted_data`).
- **Refused calls become requests.** A call refused because an account is off
  or lacks a permission shows under Agent requests as "‹Agent› wants to use
  ‹account›", with **Allow…** (the sheet, preset to what the call needed) or
  **Ignore**. The agent's error is unchanged.
- **Activity.** Agent access → Activity lists your decisions, changes agents
  made without approval, consent changes and refused attempts.

**After upgrading:** restart agent sessions so they start the new
`mail-mcp`; a process started earlier keeps the old behavior until it exits.
Existing registrations, including `email-write`, are then refused until you
turn on agent access for each account they should use. The
first refused call shows up under Agent requests as "wants to use"; Allow…
opens the sheet at the level the call needed. For `--allow-write` choose Read,
organize and write.
Review the Outbox once: mail scheduled through MCP before the approval gate
existed is not retroactively gated.

Threat model, mitigations and what is not approval-gated:
[mcp-security.md](mcp-security.md).

Build:

```sh
cargo build -p mail-mcp --release
```

The helper uses the selected distribution profile, like the apps. With the
`development` profile (`OPEN_TRACKING=true`) it and the scheduled-send
dispatcher send drafts the development app marked with Track opens; with
`official` or `unofficial` they send such drafts without a pixel and turn their
tracking off. See [reader privacy](reader-privacy.md).

On macOS, use `./scripts/build-mcp-macos.sh` to build and sign with Email's stable local development identity. The server disables Keychain interaction before opening the service, including dispatch-only runs. Existing permissions still work; locked or unapproved credentials return an error rather than opening a password dialog. This does not grant the helper access to credentials approved only for the desktop app.

## Connect Codex and Claude Code

In the app: open **Agent access** and choose **Connect** next to Claude Code
or Codex. The row says whether the CLI is installed and whether Email is
already registered in it ("✓ Connected"). Connect runs the same command as
the script below, with the helper that ships with the app
(`Email.app/Contents/Helpers/mail-mcp` on macOS) and the app's own cache:

```sh
claude mcp add --scope user --transport stdio <name> -- <helper> --db <cache> \
  --allow-draft --allow-triage --allow-send --allow-sync --allow-calendar-write
codex mcp add <name> -- <helper> --db <cache> <same flags>
```

`<name>` is the profile's `MCP_SERVER_NAME` (`email` for official builds).
An existing registration under that name is never replaced; remove it with
`claude mcp remove <name>` or `codex mcp remove <name>` first if you want the
app's. **Copy** on "Other MCP client" copies the same command and an
`mcpServers` JSON snippet for any other client. Start a new agent session
afterwards so it picks up the server.

Why every mail flag: the registration is an upper bound and the in-app
permissions narrow it per account, so passing every mail flag makes Agent
access the one place you decide. Nothing works until you turn an account
on, and then only Read only until you choose more. Sends and bulk changes still
wait for your approval. `--allow-booking-settings` is left out: Scheduling
settings aren't tied to an account, have no permission in the app, and have
their own browser approval. To give an agent less than the app allows, use the
script below with fewer flags.

On Linux the packages install the helper at `/usr/lib/email-linux/mail-mcp`
(the .deb and the AUR packages from the release after v0.1.0-beta.1); a
separately downloaded `mail-mcp` in `~/.local/bin` or on PATH works too.
Without a helper, Agent access says so and Connect stays off.

### From a source checkout

Once Email has created its local cache, run this from the repository root:

```sh
./scripts/connect-mcp.sh
```

This registers the existing release binary with both installed CLIs, building it if needed. It leaves existing registrations untouched and gives agents **read-only** access by default. To register just one client, use `--codex` or `--claude`. To rebuild after a code update, use `--build`. The default cache is `mail.sqlite` in the selected profile's data directory: `~/Library/Application Support/<MACOS_DATA_DIRECTORY>` on macOS and `$XDG_DATA_HOME/<LINUX_DATA_DIRECTORY>` on Linux (`Email` and `email-client` for the development profile). Use `--db /absolute/path/to/mail.sqlite` for a custom cache.

Verify the connection with `codex mcp get <name>` or `claude mcp get <name>`, where the name is the profile's `MCP_SERVER_NAME` (`email-unofficial` for a plain build); in Claude Code, `/mcp` also shows server status. Restart an already-open agent session if it does not discover the newly registered tools.

Registration names come from the [distribution profile](distribution-profile.md)'s `MCP_SERVER_NAME`: `email` for the official and development profiles, `email-unofficial` for the neutral default. The default cache path follows the profile's data directory. The script's default `email` registration is read-only, while one made with Connect in the app passes every mail flag; `claude mcp get <name>` shows which you have.

If you intentionally want agent-controlled mail changes, run `./scripts/connect-mcp.sh --allow-write`. This creates a **separate** `email-write` registration with `--allow-write`; it does not change the read-only `email` registration. Sends and bulk changes still wait for your approval in Agent access, and each account still needs agent access with the matching grants. The registration is local to this computer; it does not expose an HTTP server.

The script and the app use the official [Codex MCP CLI](https://learn.chatgpt.com/docs/extend/mcp?surface=cli) and [Claude Code MCP CLI](https://code.claude.com/docs/en/mcp) commands rather than editing either client's config file.

The helper also accepts individual grants and repeated account restrictions. For
example, prepare drafts for one account without permission to send:

```sh
./scripts/connect-mcp.sh --allow-draft --account ACCOUNT_ID --name email-drafts --dry-run
# Review the printed arguments, then run without --dry-run to register.
```

`--dry-run` does not build or change either client. Custom grants default to the
separate `email-custom` registration, account-only restrictions to `email-scoped`;
use `--name` for multiple distinct configurations. Existing registrations remain
untouched, including when their settings differ from the requested arguments.

Start with a demo database:

```sh
./target/release/mail-mcp --db ./demo.sqlite --seed-demo
```

Start against an existing database:

```sh
./target/release/mail-mcp --db /absolute/path/to/mail.sqlite
```

Write operations are disabled by default. Grant independent capabilities with the flags below; `--allow-write` remains a compatibility shorthand granting all four. Account restrictions still apply when `--allow-write` is used. `send_draft`, `queue_send_draft` (with an undo delay of at least 10 seconds) and `schedule_draft` (a future Unix time in milliseconds) each create a request that you approve or reject in the desktop app; nothing is sent before approval. `cancel_scheduled_draft` works only before dispatch claims the message. The server writes protocol messages only to stdout; diagnostics go to stderr.

## Least-privilege access

| Flag | Authority |
| --- | --- |
| No grant flags | Read cached accounts/messages, rendered bodies, attachments and status for accounts with agent access. Attachment reads can fetch bytes from the provider. |
| `--allow-draft` | Create/edit/discard local drafts and attachments (discards restorable for 7 days); manage local snippets. Cannot queue, schedule, or send mail. |
| `--allow-triage` | Archive, mark read/unread, batch triage/undo, snooze, and manage saved splits. Queues provider changes locally. |
| `--allow-sync` | Refresh provider state and flush **already queued triage changes**, including changes made by another app/process. Cannot dispatch the outbox. |
| `--allow-send` | Request sending, queuing or scheduling existing drafts (each waits for approval in the app), cancel pending sends, and dispatch mail already approved or scheduled by you. Does not grant draft editing. |
| `--allow-write` | Grant all four capabilities for compatibility. |
| `--allow-booking-settings` | Independently enable scheduling-settings authorization and reviewed configuration apply. Requires separate browser-approved `booking:settings` permission. Does not grant mail, calendar-event or booking mutations. |

Each flag also needs the matching in-app permission for the account (Drafts,
Archive and snooze, Ask to send, Fetch mail, Calendar changes).
`step_sender_cleanup` needs both Archive and snooze and Fetch mail.

Capabilities combine explicitly. For example, an agent that can prepare drafts
but cannot send them can run:

```sh
./target/release/mail-mcp --db /absolute/path/mail.sqlite --allow-draft --account 'gmail-account-id'
```

Account IDs come from `list_accounts()` on an unscoped connection; use the ID,
not the email address. Repeat `--account ID` for multiple allowed accounts. An
omitted allowlist permits all accounts; a nonempty allowlist restricts both
reads and writes, even with `--allow-write`. `list_accounts`, drafts, thread lists,
search, snoozed mail, outbox, and pending counts expose only allowed accounts.
Message/thread IDs and every thread in a batch are checked before dispatch; a
mixed-account batch is rejected before any change. `sync_all` refreshes only
allowed accounts. Unknown service operations fail closed until an explicit MCP
policy rule is added.

Some objects have no account owner in the current data model. A scoped server
therefore rejects snippet/label/split listing, shared snippet/split writes,
queries using a saved `split_id`, and global outbox dispatch. Use direct cached
search and account-filtered thread listing instead of shared split queries.
Undo checks every recorded thread's account before dispatch, including older
operation IDs. The store separately rejects undo that would overwrite newer
changes. `latest_triage_undo` returns null if its candidate includes a forbidden
account. Scoped search uses the same [structured local search](search.md) semantics as
unscoped search, filtering accounts in SQL before applying the result limit;
other accounts cannot crowd allowed matches out of a global candidate window.

One-shot `--dispatch-due` now requires `--allow-send` (or `--allow-write`) and
rejects `--account`, because the dispatcher does not offer account-scoped claims.
Existing external schedulers must add `--allow-send` explicitly. `--seed-demo`
cannot be combined with `--account` or `--dispatch-due`, and refuses the
default mail store.

These grants constrain this MCP process, not the operating-system user or the
SQLite file. A host with independent shell/file access could still read or edit
that file; use host-level permissions when an agent must not have that access.
The MCP host may discover tools it cannot execute; tool availability is never
treated as authorization. Enforcement occurs at request dispatch, independent
of tool descriptions.

Email bodies and other third-party content returned by MCP are wrapped as labelled untrusted data, and the server instructions tell agents never to follow instructions found in them. The server sanitizes HTML for display, but agents should use the user's instructions to decide whether to draft, archive, or open a link. Sends and bulk changes wait for the user's approval regardless. The default read-only registration is suitable for connecting an agent to a personal mailbox without granting it mail-changing authority.

For a local MCP host, configure the command as `/absolute/path/to/Email/target/release/mail-mcp` and args as `--db`, followed by the desktop app's database path. Authentication is the local process boundary; do not expose this process as an unauthenticated network service.

The one-shot `--allow-send --dispatch-due` mode sends due items and exits without running an MCP server. The outbox holds only mail you scheduled or approved, so this mode never sends a pending or rejected agent request. The desktop app can invoke it while open; the optional macOS launchd and Linux systemd user timers keep it running when Email is closed. A sleeping or offline computer cannot dispatch on schedule. An uncertain provider result is held for review to avoid a duplicate send on the next timer run.

### Selected-conversation actions

`batch_triage(operation_id, kind, thread_ids)` asks the user to approve a change
to the selected conversations; the desktop app applies it with the shared
atomic batch implementation only after approval. `undo_triage(operation_id)`
likewise asks the user to approve reversing an applied batch. Both require
`--allow-triage` (or `--allow-write`) and the Archive and snooze permission; discovery does not
grant write access. Supported kinds are
archive, restore, spam, read, unread. Undo supports placement changes only.
The supplied operation ID must be unique per logical request and reused only
for an identical retry. Call synchronization separately to flush provider work.
See [bulk triage](bulk-triage.md) for concurrency and verification limits.


`latest_triage_undo()` is read-only. It discovers a still-valid placement undo
after restart and returns `{operation_id, kind, count, account_ids}` or null.
It examines the latest 100 non-undone placement batches; `undo_triage` rechecks
that no newer local action would be overwritten before applying an inverse.

### Local snippets

`list_snippets()` reads named plain-text snippets. `upsert_snippet(id?, name,
body)` creates or edits one, and `delete_snippet(id)` removes it. The two writes
require `--allow-draft` (or `--allow-write`); listing remains available in unscoped read-only mode. These
operations invoke no AI, make no network requests, and do not send mail.

Names are trimmed, 1–80 printable characters, and unique under SQLite's ASCII
case-insensitive comparison. Text must be nonblank, contain no NUL, and fit in
64 KiB of UTF-8. The store allows at most 500 snippets. Supply an existing ID to
edit; missing IDs on edit/delete return errors. Existing draft text is unchanged
when its source snippet is edited or removed.

### Partial synchronization

`sync_account(account_id)` requires `--allow-sync` (or `--allow-write`) because it updates cached
provider state and flushes queued changes. Inspect `pending_actions`,
`actions_retry_after_ms`, `warnings`, and optional `sync_error` in its result.
Incoming refresh happens before a bounded action flush, so a completed call can
leave durable changes pending. A retry hint is a suggested delay, not a timer
inside the MCP server. `sync_in_progress:true` reports another active sync lease.
A `sync_error` concerns incoming refresh and can coexist with action progress;
it does not make cached bodies unavailable. See
[service synchronization results](service-api.md#synchronization-results) for the
full result contract. Do not report remote completion merely because a local
triage write or synchronization call returned successfully.

### Granular permission regression checks

```sh
cargo test -p mail-mcp
cargo build -p mail-mcp
python3 crates/mail-mcp/tests/permissions.py target/debug/mail-mcp
```

The JSON-RPC test simulates the in-app consent by writing the local consent
table directly, as a user with file access could. It also checks that refusals
name Agent access, that consent and revocation apply to a running server's
next request, that sends and batches become pending approvals, and that the
one-shot dispatcher finds nothing to send while a request waits.

The JSON-RPC test creates only disposable local fixtures. It checks read-only,
draft-only, triage-only, send-only, and account-restricted broad-write clients;
it schedules a draft in 2100 and cancels it without dispatching or contacting
any provider. Unit tests also check unknown-operation rejection and atomic
mixed-account batch denial. These checks do not prove host/process isolation.

### Durable Gmail sender cleanup

The MCP tools expose the same explicitly confirmed snapshot workflow as the
native app. It handles provider matches that have not been downloaded locally:

1. `create_sender_cleanup(account_id, sender)` creates a local discovery job.
2. `step_sender_cleanup(job_id)` enumerates one bounded provider page per call.
   Wait for `state: ready`; inspect `matched` and `sample_subjects` (samples only
   exist for already cached messages). Respect `retry_at_ms` and error hints.
3. Call `confirm_sender_cleanup(job_id)` for the requested cleanup. It creates
   a request that the user approves or rejects in the desktop app, which shows
   the account, sender, count and samples. Discovery never auto-confirms or
   marks messages read.
4. Further `step_sender_cleanup` calls mark the confirmed snapshot read in
   Gmail, in bounded batches. New arrivals after discovery are excluded.
5. `list_sender_cleanups(account_id?)` and `get_sender_cleanup(job_id)` report
   durable progress. `pause_sender_cleanup`, `resume_sender_cleanup`, and
   `cancel_sender_cleanup` control future work. Pause/cancel cannot recall a
   request already in flight or undo completed changes.

Create, confirm, and controls require `--allow-triage`. A step requires **both**
`--allow-triage --allow-sync`, since it can enumerate or mutate provider mail;
`--allow-write` grants both. Read-only list/get remain available. Under
`--account`, every job operation checks its stored account before dispatch and
listing filters forbidden jobs. Unknown jobs fail closed. These tools never
send mail and currently support Gmail only.

### Recent recipient suggestions

`suggest_recipients({account_id, query, limit?})` is **read-only** and requires no
draft, triage, send or sync grant. `account_id` is required. With `--account`,
that account must be allowed even with `--allow-write`; forbidden accounts are
rejected before lookup. Missing/null account input fails parsing, and unknown
accounts never fall back to global results.

The result is an array of `{email, display_name, formatted}`. `formatted` is one
strictly parsed/escaped mailbox suitable for an explicitly requested draft. The
tool itself does not edit drafts or send mail. Query text is literal, at most
128 UTF-8 bytes, with no control characters; empty queries return `[]`. The
default limit is 6 and positive limits are capped at 20.

Coverage is recent local correspondence: up to 5,000 latest cached messages or
drafts in the requested account and at most 512 matching records parsed. Sender,
To and Cc contribute; Bcc contributes only from saved local drafts, not received
mail or imported/provider drafts. Results are deduplicated by mailbox and ranked
by exact email, email prefix, name/word prefix, substring and recency. This is not
a complete address book, Google People query or OS Contacts lookup. It reads no
message bodies and makes no provider requests.

Rust unit and JSON-RPC regressions cover read-only classification and account
boundaries, not independent host/process isolation.


### Calendar, reminders and signatures

`list_calendar_invitations` (explicit account) and `calendar_invitation` (message ownership) are cached reads. `refresh_calendar_invitation` needs the sync grant and only fetches that message's invitation attachments; it never accepts/declines a meeting.

`list_followups` requires an explicit account. `set_followup` and `cancel_followup` require triage permission and thread ownership. `get_signature` requires an explicit account/address; `upsert_signature` and `delete_signature` require draft permission. None grants send permission. The account allowlist applies even with `--allow-write`.

Reminder status is locally evaluated and can change from waiting to due/replied on read. A reply must be downloaded; auto-replies may count. This is not a remote notification service.

### Connected Google Calendar

`list_calendars` and `list_agenda` read cached calendars/events. `availability`
returns candidate slots and explicit cache-coverage metadata. `sync_calendar`
requires `--allow-sync`. `respond_calendar_invitation` and `create_calendar_event`
require the separate `--allow-calendar-write` flag; `--allow-write` and mail send
permission do not imply it. All tools take an explicit account and respect
`--account` restrictions. Connect Calendar in the desktop UI before syncing;
MCP never starts browser consent. Creation can send attendee invitations and
must not be automatically retried after an uncertain response. See
[connected-calendar](connected-calendar.md) for limitations and safety behavior.


Calendar event editing uses `update_calendar_event` with an exact `expected_etag`
from the current saved agenda. It requires `--allow-calendar-write` independently
of mail permissions and retains account restrictions. Only apply the user's
specific requested edit; do not retry conflicts automatically. Optional create
`location` is also supported. Calendar guests receive provider updates.

## Scheduling and open activity

The MCP exposes read-only `booking_status`, `booking_list`, `booking_link`, `booking_availability`, `booking_analytics` and `tracking_activity`. Connect scheduling through the native app or the explicit setup authorization below. These read tools cannot approve a device, access credentials, change bookings, enable tracking or send mail. Availability is a suggestion, not a reservation; open counts measure image requests, not verified reads.

The activity candidate enriches `tracking_activity` with local subjects, recipients,
conversation identifiers, cached reply evidence, existing follow-up state and a
bounded request timeline. No message content is uploaded to obtain this context.
An explicit `message_id` reads that message's detail; the aggregate view retains
the existing account restrictions. Reading activity does not create or reset a
reminder. Setting/cancelling a follow-up still requires `--allow-triage`, and
preparing/sending a follow-up still requires its separate draft/send permissions.
Legacy request totals do not imply that historical event timestamps are available.
For a reminder requested from a tracked message, pass its `message_id` as optional
`set_followup.sent_message_id`. The service rechecks downloaded replies atomically
with creation and refuses a stale no-reply decision. An account-restricted MCP
checks both the thread and the anchor message; an allowed thread cannot authorize
an anchor from another mailbox.

Hosted scheduling is a separate shared account, so booking tools are unavailable when the server is restricted with `--account`. In that mode `tracking_activity` requires an explicit message ID belonging to a permitted mailbox. Restart an existing MCP client connection after rebuilding to load the new tools.

### Configure scheduling through MCP

`booking_setup_status`, `booking_setup_read` and `booking_setup_preview` are read/preview tools. Existing desktop grants remain read-only for configuration. They do not silently acquire `booking:settings`; full connected-calendar inventory also requires that permission. Start by reading setup status and the current complete configuration.

To allow a specific MCP registration to request and apply scheduling settings:

```sh
./scripts/connect-mcp.sh --allow-booking-settings --name email-settings --build --dry-run
# Review the printed command, then repeat without --dry-run to register.
```

This flag is independent of `--allow-write`, `--allow-send` and `--allow-calendar-write`. It does not grant any of them. Account-restricted registrations cannot use hosted scheduling tools, even with the settings flag. Rebuild/sign with `./scripts/build-mcp-macos.sh` on macOS and restart the MCP connection to load these tools.

The setup sequence is:

1. Call `booking_setup_status()`. If settings permission is missing, call `booking_setup_start({name:"Email MCP scheduling setup"})` on the explicitly enabled registration. Give the returned verification URL/code to the user. The user signs in, reviews the exact settings permission and approves it in their browser. Agents must not approve on the user's behalf. `booking_setup_poll()` follows its returned pending interval and stores an approved bearer only in OS secure storage.
2. Call `booking_setup_read()` to inspect profile, meeting types, revision and available calendars. Google calendar OAuth connections and their destination/conflict selections remain browser-managed in the scheduling studio. MCP cannot add or disconnect Google accounts or change Google grants. Select only calendars already connected and permitted for booking.
3. Call `booking_setup_preview({patch:{profile:{name:"Your name",company:"Your company"}}})`, or supply `meetingTypes` as the **complete** array from the current read, editing only the intended types. Preserve other types and fields. The backend validates policy, account ownership, calendar selection, unique slugs and publishability. A preview returns normalized settings, explicit changes, warnings and an opaque `plan_id`; nothing is applied.
4. Review that concrete result with the user. On their specific approval, call `booking_setup_apply({plan_id:"...",confirm:true})`. The plan is bound to the connected host, private database profile, exact normalized patch and settings revision; it expires in ten minutes and is lost when this MCP server restarts. A changed revision/account requires a new read and preview. A confirmed repeated call returns its saved result; an uncertain write outcome cannot be blindly retried.

Only profile name/company and meeting-type configuration are writable through these tools. Supported type settings are returned by `booking_setup_read`, including duration, timezone, availability, notice/buffer, appearance, reminders, publication and already-connected calendar references. Applying settings never sends email, creates/cancels/reschedules bookings or edits calendar events. Share links using `booking_link` or fresh choices using `booking_availability`; preparing an email with those links still requires the separate draft grant, and sending it requires an explicit mail-send instruction and grant.

For weekday availability, first obtain the complete `settings.meetingTypes` array with `booking_setup_read()`. Choose the intended type by its existing `id`, keep its other fields and all other types, and merge these example fields into that type before previewing the complete edited array:

```json
{
  "zone": "Europe/Paris",
  "weeklyHours": {
    "0": [],
    "1": [{"start": 540, "end": 720}, {"start": 780, "end": 1020}],
    "2": [{"start": 540, "end": 720}, {"start": 780, "end": 1020}],
    "3": [{"start": 540, "end": 720}, {"start": 780, "end": 1020}],
    "4": [{"start": 540, "end": 720}, {"start": 780, "end": 1020}],
    "5": [{"start": 540, "end": 720}],
    "6": []
  },
  "dateOverrides": {
    "2026-12-24": [],
    "2026-12-31": [{"start": 540, "end": 720}]
  },
  "dailyBookingLimit": 4
}
```

Days use Sunday `0` through Saturday `6`. Range bounds are local minutes after midnight: `540` is 09:00, `720` is 12:00, `780` is 13:00 and `1020` is 17:00. This example allows Monday–Thursday mornings and afternoons, Friday mornings, and closes weekends. An empty date override closes that local date; a custom range replaces that day's normal hours. Preserve any other existing date exceptions when adding these examples. `dailyBookingLimit: 0` disables the cap; positive values range from 1 through 50. `weeklyHours: null` uses the legacy `days/startHour/endHour` settings.

Call `booking_setup_preview` with `patch.meetingTypes` set to that entire edited array. Review the returned changes and warnings, then apply its exact `plan_id` with `confirm:true` only when authorized. Published types also need their existing connected account and configured destination; preview rejects an unconfigured destination. To suggest three times in an email afterward, call `booking_availability({meeting_type_id:"the-existing-id"})` and use its returned `url` values in an explicitly requested draft.

Choose client and capabilities explicitly when registering. These commands illustrate settings-only access, or settings plus local draft preparation:

```sh
./scripts/connect-mcp.sh --codex --allow-booking-settings --name email-settings --build --dry-run
./scripts/connect-mcp.sh --claude --allow-booking-settings --allow-draft --name email-settings-drafts --build --dry-run
```

Review the printed command and repeat without `--dry-run` to register. Neither example grants email sending or calendar-event changes. Existing registrations are left untouched, and the browser settings approval is still required after registration.

On Linux, the first browser-approved connection may require the OS to create/unlock Email's secure SecretService collection. There is no plaintext credential fallback. Unapproved PKCE preparation stays in process memory per database profile for at most ten minutes; restarting during authorization requires starting again. macOS retains Email's existing stable Keychain service/signing identity.

Validation covers independent CLI permissions, account restrictions, old read grants, strict requested-versus-issued scope, secret redaction, account/revision/expiry plan binding, one-attempt behavior after unknown outcomes, and local rejection before credentials/network access. No real browser authorization, settings apply, email send or booking mutation is performed by these tests.

### Typed booking forms

Meeting settings also preserve `questions` and `extraGuestsLimit` through the
read → preview → approved apply flow. Merge these fields into the chosen meeting
type in the complete array; keep its ID, calendar references and other types:

```json
{
  "questions": [
    {"id":"topic","type":"select","label":"What would you like to discuss?","required":true,"options":["Product","Partnership"]},
    {"id":"context","type":"textarea","label":"Anything to prepare?","required":false}
  ],
  "extraGuestsLimit": 2
}
```

Supported question types are `text`, `textarea`, `select` and `checkbox`. Use up to
eight questions with stable unique IDs; a select has up to twelve choices.
`questions: null` retains the legacy preparation question, while `questions: []`
removes extra questions. The guest name/email remain required. Set
`extraGuestsLimit` to zero to disable additional invitees or 1–5 to allow them.
A required checkbox requires an affirmative answer. Review the normalized preview
before applying; changing the form affects new bookings, while existing bookings
and their reschedules retain their saved answers and question labels.

Configuration access does not add guest answers or extra invitee addresses to the
local desktop booking cache. These remain in the authorized hosted booking details.

### Shared booking availability schedules

`booking_setup_read` includes `availabilitySchedules` and meeting `scheduleId` references. To edit reusable hours, preview a patch containing the complete schedules array, preserving unaffected entries. To link or detach a meeting, also supply the complete meetingTypes array. A schedule cannot be removed while any saved type references it. Schedule edits affect future availability across all linked types; existing booking details remain unchanged. The browser and MCP share revision conflict checks.
