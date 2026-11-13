# Launch demo mailbox

A realistic, fully synthetic mailbox for screenshots, recordings and trying
the app. It opens in either desktop app on a fresh, isolated store, and the
local MCP server can triage it.

## Record with it

One command per platform, from the repository root. Each run builds the app,
creates a new private store in the temporary directory and opens it there. Your
real mail store, accounts and Keychain items are never used.

| What | macOS | Linux |
| --- | --- | --- |
| Launch demo (about 270 conversations) | `./scripts/dev.sh --demo` | `./scripts/dev.sh --demo` |
| Search shot (launch demo plus 100,000 older conversations) | `./scripts/dev.sh --demo search` | `./scripts/dev.sh --demo search` |
| The three original demo messages | `./scripts/dev.sh --demo small` | `./scripts/dev.sh --demo small` |

Add `--release` for optimized binaries. `./scripts/dev.sh` without `--demo`
still opens the three demo messages in `dist/demo/mail.sqlite`, as before.

The command prints the store path. Delete it afterwards (`rm -r <directory>`);
the search store is about 160 MB. Without the script, set both variables on an
app you built yourself:

```sh
EMAIL_STORE_PATH=/absolute/new/dir/mail.sqlite EMAIL_DEMO=launch <app>
```

`EMAIL_DEMO=1` (or `small`) keeps adding the three original messages. Every
value is ignored unless `EMAIL_STORE_PATH` is set, and `mail-service` refuses
demo data in the default store however it is spelled.

### Before you record

- **Start from a fresh store.** Dates are relative to the moment the store is
  seeded ("today", "yesterday", "this week"), in your Mac's or desktop's time
  zone. A second launch on the same store keeps it as it is.
- **Record after about 08:00.** Today's newest mail arrived within the last few
  hours; shortly after midnight most of it is labelled yesterday.
- **Sending is not possible.** The demo accounts have no mail server. Sync
  succeeds with nothing to fetch, and archive, snooze, star and labels apply
  locally. Scheduled mail stays in the Outbox until its time; on a store older
  than a day, a due item fails to send.
- **Don't open `invite.ics` in Calendar** on the recording Mac: it would add
  the demo event to your real calendar. The in-app Calendar invitations list
  is safe.
- **Daylight saving.** Dates use your UTC offset at seeding time. A store
  seeded in the week before a clock change shows future events an hour off.
- **Load images shows placeholders.** Newsletters reference images on
  `.example` and `.test` hosts, which do not exist, so the "Remote images are
  blocked" notice is real but loading them shows coloured blocks with alt text.
  Inline (`cid:`) pictures are stored in the demo and always render.

## What is in it

Two accounts for Alex Morgan, who leads product at Fernhill, an invented
software company in Oslo: work (`alex@fernhill.example`) and personal
(`alex@example.com`).

- **Today:** a meeting request from Elena Rossi asking for times next week
  (for the Scheduling "insert open times" flow), Maya Chen's onboarding screens
  with an inline picture, the six-message "Launch plan: Fernhill 2.0" thread,
  the go/no-go invitation, a cloud invoice, an investor reply, a design
  newsletter with blocked remote images, and Erik Haugen's cabin-trip photos
  (in Norwegian).
- **Work:** threads with several replies (pricing copy, release notes, a
  sync-engine RFC, a customer pilot with a PDF report, a podcast invitation,
  the offsite, hiring), a penetration test report, a support digest with a CSV,
  and Norwegian notes from the board meeting and the accountant (MVA, PDF).
- **Invitations:** six calendar invitations (`invite.ics`, including a weekly
  recurring one and a cancellation), listed under **Calendar invitations**, plus
  RSVP replies.
- **Notifications:** about 70 code-review and CI messages from one sender
  (useful for sender cleanup), uptime alerts and document comments.
- **Receipts and travel:** monthly invoices with PDFs, payment receipts, a
  flight, hotel and train ticket, a speaker pass, a payslip, an electricity
  bill and a bank e-invoice notice.
- **Newsletters and promotions:** five newsletters and four shops, most with
  remote images and unsubscribe headers.
- **Personal and Norwegian:** family dinner, the housing co-op dugnad, a
  dentist reminder, the library, a climbing gym and a coffee receipt with an
  inline logo.
- **State:** labels (Work, Launch, Receipts, Travel, Newsletters, Promotions,
  Notifications, Support, Family), splits (Team, Notifications, Newsletters,
  Promotions), six favourites, five snoozed, two follow-ups, about 20 sent
  conversations, four drafts (one a reply), three scheduled sends, a work
  signature and three snippets.

About 70 conversations are in the inbox and about 20 unread; the rest are
archived or sent. Content is fixed: two runs differ only in their dates.

**Fictional only.** People, companies and domains are invented; every address
and link uses `example.com`, `example.org`, `example.net`, an `.example` name or
`.test`, which a test checks. Pictures, PDFs and invitations are generated in
code (`crates/mail-core/src/demo/files.rs`); nothing is downloaded or copied.

## The search shot

`./scripts/dev.sh --demo search` (or the example directly) adds 100,000 older
conversations in the same style: invoices and receipts, code notifications,
newsletters, promotions, colleagues, customers, Norwegian personal mail and
travel. They are dated from three weeks to four years ago, mostly read, and in
the inbox below the curated mail, because the apps search the open folder.

```sh
cargo run --release -p mail-service --example demo_store -- \
  --store /absolute/new/dir/mail.sqlite [--messages 100000]
```

It prints a timing table for the request the apps send when you search the
inbox (`search` with `is_inbox: true`, limit 100), through
`MailService::request_json`. `--bench-only` times an existing store,
`--all-folders` drops the inbox scope (as the MCP server searches), `--cold`
times each search as the first request on an uncached copy of the store, and
`--query` replaces the default queries.

Measured 13 November 2026 on an Apple M3 Max, release build, 100,000 older
conversations and 110,162 messages in all (160 MB). Warm: 31 runs per query,
P50 and P95 of the last 30, median of three rounds. Before is main at
`1bec82b`; after is the query-plan change described in the
[search benchmark](benchmarks/search-100k.md), which has the method, cold-cache
and typing numbers and the other views.

| Query | Results | Before P50 ms | Before P95 ms | After P50 ms | After P95 ms |
| --- | ---: | ---: | ---: | ---: | ---: |
| `invoice` | 100 | 16.9 | 17.7 | 3.9 | 4.5 |
| `faktura` | 100 | 44.0 | 45.3 | 3.3 | 4.3 |
| `Ingrid` | 100 | 42.1 | 43.5 | 3.3 | 4.2 |
| `Brightwater` | 100 | 126.2 | 132.5 | 4.9 | 6.6 |
| `Elena` | 100 | 146.2 | 164.7 | 4.6 | 6.0 |
| `from:ingrid` | 100 | 3.1 | 4.1 | 3.0 | 3.4 |
| `is:unread subject:invoice` | 76 | 23.8 | 25.9 | 24.0 | 26.3 |
| `"launch plan"` | 1 | 50.2 | 51.7 | 0.43 | 0.51 |
| `nonexistentterm` | 0 | 49.8 | 53.8 | 0.08 | 0.09 |
| inbox listing (no query) | 100 | 0.58 | 0.61 | 0.59 | 0.66 |

Results are the same conversations in the same order. The first search after
the store leaves every cache (a cold start) reads from disk: about 75–310 ms
for the word searches above on this Mac.

The older [synthetic benchmark](benchmarks/search-100k.md#synthetic-corpus-9-november-2026-superseded)
put "invoice" in every message, so its 2.7 ms only measured the best case.

## The agent demo

1. Start the demo: `./scripts/dev.sh --demo`. It also builds `mail-mcp` and
   writes `mail.claude-mcp.json` next to the store, and prints its path.
2. In Email, open **Agent access** in the sidebar. For each demo account choose
   **Turn On…**, read the disclosure and tick **Drafts** and **Triage** (add
   **Send requests** only if you want the approval card for a send; sending
   itself fails, since there is no mail server).
3. In another terminal: `claude --mcp-config <store directory>/mail.claude-mcp.json`.
   This adds an `email-demo` server for that session only and leaves your
   Claude Code configuration alone. The server runs with `--allow-draft
   --allow-triage` against the demo store.
4. Ask Claude Code to triage, for example: "Go through my inbox, archive the
   promotions and newsletters I haven't opened, draft a reply to Elena with
   three times next week, and tell me what needs me today."
5. Drafts appear in **Drafts** at once. Bulk archive and any send wait in
   **Agent access** for **Approve** or **Reject**.

Agent access is never turned on by seeding: every store, including demo
stores, starts with all accounts off. Before step 2 the server answers "No
account has agent access turned on…", which is also a good shot.

## For contributors

- Content: `crates/mail-core/src/demo/` (`content.rs` for the conversations,
  `bulk.rs` for the search history, `files.rs` for MIME, PNG, PDF and ICS).
  `MailStore::seed_launch_demo(now_ms, utc_offset_minutes)` writes it once; a
  store that already has the work account is left alone.
- Requests: `{"op":"seed_demo","set":"launch","utc_offset_minutes":60}`;
  `{"op":"seed_demo"}` is unchanged. The apps send the local offset.
- Demo files: each demo message with files keeps its MIME source in
  `demo_raw_messages`, a table only demo stores have. `mail-service` reads it
  instead of a provider for attachments, inline pictures and invitation refresh,
  using the same parser as IMAP mail. Sync treats a `demo:` account without
  provider settings as having nothing to fetch and its local changes as final.
- Checks: `cargo test -p mail-core demo`, `cargo test -p mail-service
  launch_demo`, `python3 apps/macos/Tests/run-launch-demo-checks.py --library
  target/debug/libmail_service.dylib [--snapshots DIR]` (macOS), and
  `apps/linux/qa/launch-demo.sh` in the Linux container (see its header).
