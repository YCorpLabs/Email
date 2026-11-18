# Filters

Filters sort incoming mail into named piles: Cold outreach, Promotions,
Newsletters, Notifications, or anything the person describes. Each filter is a
name, a plain-language description and an action. A small decision model
reads the first mail from strangers and scores it against each filter's
description; the best fit wins if it is sure enough. It runs on the computer;
mail never leaves it.

Splits are different: they are saved searches that never move mail. Filters
decide where mail goes.

## What happens to mail

| Action | In the app | On the server |
| --- | --- | --- |
| Keep in inbox | Stays in the inbox, tagged with the filter | Nothing |
| Skip inbox | Leaves the inbox; read it from the filter | Archived |
| Ignore | Leaves the inbox, marked read | Archived and marked read |

Nothing is deleted. Every filtered conversation can be moved back to the inbox
or to another filter, from the reader bar or the command palette.

## Safety rules

These hold before any model output is used, in this order:

1. **People you know never reach the model.** Mail from an address the account
   has written to, from the account's own company domain (not public domains
   like gmail.com), or in a conversation the person replied in, stays in the
   inbox untouched.
2. **Hand sorting wins.** Moving a conversation sets a rule for that sender:
   later mail from them goes to the same place without the model.
3. **Unsure means inbox.** A model decision below 80% leaves mail in the
   inbox; Ignore needs 95%, because it also marks mail read.
4. **Follow-ups follow.** A new message from the same stranger in a filtered
   conversation ("just bumping this") is filtered again without the model.
   A conversation the person replied in is never re-filtered.
5. **Sorting reaches back 14 days** when filters are first turned on, and only
   covers inbox mail. Snoozed mail is sorted when it wakes.

Message text is untrusted. The model never generates text: it returns one
probability per filter, so an email can at most push itself into the wrong
filter, never instruct the app. Rows sorted by the model show the preview,
not a reason; the reader bar names the filter.

## The model

`crates/mail-ai` runs the Email sorter: multilingual-e5-small (MIT, 118M
parameters, about 100 languages) fine-tuned as a cross-encoder. It reads a
filter's description and the start of an email together and returns the
probability that the email belongs there; each filter is scored separately,
so custom filters work the same way as the starters and order does not
matter. Sender, subject, the first ~1,200 characters and a bulk-mail marker
(List-Unsubscribe) are what it sees.

- **Size:** 140 MB of weights (word embeddings int8, the rest f16) plus the
  17 MB e5 tokenizer, 157 MB in total.
- **Memory:** about 0.5 GB peak while sorting; dropped when the backlog is
  empty.
- **Speed:** CPU only through `candle`, about 0.1–0.2 s per email with the
  four starters on an M3 Max; the whole launch demo sorts in 7 s.
- **Files:** one safetensors file (`format: email-sorter-1` in its metadata)
  and the tokenizer, checked against size and SHA-256 before use; downloads
  resume. Stored once per profile in `<data directory>/Models` (override
  with `EMAIL_MODELS_DIR`).

### Training

Trained on synthetic mail, no real mail: 2,900 emails written for the
purpose (pitches, people who want to buy, investors, press, students,
receipts, bookings, newsletters, promotions), about half English, the rest
Norwegian, German, Spanish, Danish, Swedish, French, Dutch and a few others, each paired with the starter descriptions
and with paraphrased and custom descriptions so that filters a person writes
also work. Training code and data stay outside this repository.

### Measured accuracy

`cargo run --release -p mail-ai --example sort_eval -- <sorter.safetensors> <tokenizer.json> [records]`
applies the four starters with the app's thresholds (80%, 95% for Ignore).
"Real mail hidden" is mail from a person that would leave the inbox.

| Set | Email sorter | Qwen3 4B (previous engine) |
| --- | --- | --- |
| 30 built-in emails | 24/30, real mail hidden 0 | 27/30, 0 |
| Blind set, 200 emails nobody trained on | 186/200, real mail hidden 2 | 154/200 |

The two hidden emails on the blind set are borderline: a headhunter
recruiting the reader personally and a fund asking to invest in the reader's
practice, both scored 97% Cold outreach. Raising the Ignore threshold to 98%
hides none but drops the score to 155/200, so 95% stays. On the built-in set
the sorter leaves more pitches in the inbox than the 4B model did; it never
moves a person's mail there.

On the launch demo mailbox (`cargo run --release -p mail-service --example
filters_demo`), all four cold pitches are caught (including a two-message
follow-up and a recruiter), receipts, invoices, bookings and alerts are
tagged Notifications, newsletters and promotions are told apart, and no mail
from a person leaves the inbox.

### Where the model lives

The weights are published at
[huggingface.co/YCorpLabs/email-sorter](https://huggingface.co/YCorpLabs/email-sorter)
(MIT), pinned in `crates/mail-ai/src/catalog.rs` to commit
`cbfafb430c4c7abac6e442bd82b8d8658f99d0dd`; the tokenizer comes from
`intfloat/multilingual-e5-small` at a pinned revision. A new model version is
a new file (`email-sorter-2.safetensors`) in the same repository, with a new
pinned URL, size and SHA-256. A fresh download of both files took 7 s on a
fast connection (18 Nov 2026).

## Desktop UI

Both shells implement the same structure (macOS is the reference):

- **Sidebar, FILTERS section** (between the workspace items and SPLITS).
  - Off: one row, **Set up filters**.
  - On: one row per filter with the action's icon and a count: unread for
    Keep in inbox and Skip inbox, none for Ignore. Then **New filter**.
  - While the model downloads: a progress row (**Getting ready… 42%**).
- **Setup sheet** (first run): what it does in one line, the four starter
  filters with a checkbox and an action menu each, the download size, and
  **Download and turn on**. The sheet closes at once; the download continues
  in the sidebar and sorting starts when it finishes.
- **Filter view**: the list shows the filter's conversations, newest first,
  with a reason in place of the preview when one was recorded (the current
  model records none, so rows show the preview). The header says what the
  filter does and links to **Edit**.
- **Reader bar** on a filtered conversation: the filter name (and reason, if
  any),
  **Move to inbox** when it left the inbox (back to the inbox and remember the
  sender) and **Move to** (the other filters, then Inbox, or No filter for a
  Keep-in-inbox filter).
- **Filter editor**: name, "What goes here", "When it arrives" (Keep in inbox,
  Skip inbox, Ignore), Delete.
- **Filters settings** (palette: "Filter settings"): on/off, the model with
  its size and **Remove model**.
- Inbox rows of Keep-in-inbox filters show the filter name as a tag.
- Desktop notifications are not shown for mail a filter moved out of the
  inbox.

## Service API

All operations go through the JSON API (`docs/service-api.md`).

- `{"op":"filters_status"}` → `{enabled, filters:[{id,name,description,action,position,unread,last_7_days}], starters:[{key,name,description,action}], model:{id,model_name,license,download_bytes,memory_bytes,installed,downloading,downloaded_bytes,error}, accelerator, system_memory_bytes, sorting:{running,remaining,last_sorted_at_ms,error}}`. Cheap; poll it while downloading.
- `{"op":"filters_setup","starter_keys":["cold_outreach",…]}` → status. Turns filters on; the first time, creates the chosen starters.
- `{"op":"filters_set_enabled","enabled":false}` → status.
- `{"op":"upsert_filter","id":null,"name":"…","description":"…","action":"inbox"|"skip_inbox"|"ignore"}` → filter. Up to 8 filters; names are unique and cannot be "Inbox" or "People".
- `{"op":"delete_filter","id":"…"}` and `{"op":"reorder_filters","ids":[…]}`.
- `{"op":"filters_download_model"}` → blocks until installed, failed or cancelled; progress in `filters_status`. `{"op":"filters_cancel_download"}`, `{"op":"filters_remove_model"}`.
- `{"op":"filters_wake","budget_ms":20000}` → `{decisions:[{thread_id,filter_id,moved}], moved, remaining, model_missing, error}`. Call after each sync and again while `remaining` > 0. Only one sort runs per process; a concurrent call returns `busy`.
- `{"op":"list_filter_threads","filter_id":"…","limit":100,"offset":0}` → thread summaries.
- `{"op":"move_to_filter","thread_id":"…","filter_id":null}` → `{thread_id, filter_id, moved}`. `null` means the inbox.
- Every thread summary carries `filter`: `{id,name,action,reason,source}` or null.

`mail-mcp` offers `list_filters`, `list_filter_threads`, `upsert_filter` and
`move_to_filter`; it never downloads the model or sorts.

## Verification

### Engine (18 Nov 2026)

`cargo test --workspace --locked` (filter storage, contact gate, thresholds,
hand sorting, follow-ups, service operations without a model, MCP grants),
`sort_eval` with the Email sorter (numbers above; the Rust engine matches
the PyTorch model exactly on both test sets) and `filters_demo` on the
launch demo (7 s for the whole mailbox, all four pitches caught, no mail
from a person moved).

With the Email sorter (18 Nov 2026, evening): `run-filters-checks.py` with the
sorter installed passes all checks on macOS, including real sorting inside
the app (Cold outreach 4, Promotions 5, Notifications 23), the setup sheet
("Downloads Email sorter (157 MB) once") and Filter settings; Linux shell
unit tests pass in the container (`gigabytes` now shows MB below 1 GB, as on
macOS). A real download through `filters_download_model` into an empty
models directory finished and sorted the demo the same way. Not checked with
the new sorter: the Linux shell visually, a sort inside the Linux app, and a
download started from either app's UI.

### Linux shell (18 Nov 2026, previous Qwen3 4B engine)

`apps/linux/src/filters.rs`, with hooks in `main.rs` and `parity.rs`.
Differences from macOS: the narrow GTK sidebar puts the download percentage
where counts go ("Getting ready…" with "41%" on the right) and wraps
"Download model to start sorting" and "Download failed – Retry" onto two
lines; the settings sheet shows only the first clause of a download error,
with the full text in its tooltip.

Verified in an Ubuntu 24.04 container (Xvfb, 1440×900, disposable stores,
`EMAIL_MODELS_DIR` pointing at an empty, a partial or a placeholder model
directory; `huggingface.co` pointed at an unroutable address so nothing was
downloaded), running executable checked against the built one:

- Sidebar: Set up filters; the setup sheet (copy, four starters, actions,
  size line, Not now / Download and turn on, Esc and Enter); Getting ready…
  with progress while a download resumes from a partial file; Download
  failed – Retry after the connection timed out; Download model to start
  sorting; Sorting is off after switching sorting off; filter rows with
  icons and unread counts (none for Ignore); New filter; the right-click
  menu (Edit filter…, Delete filter…).
- Filter view on a store sorted by the real model: title, "Ignored · 3 this
  week", Edit, reasons in place of previews, no search or scope controls.
- Reader bar: name and reason, Move to inbox (hidden for Keep-in-inbox),
  Move to with the other filters and Inbox / No filter; moving updates the
  list, selects the next conversation and records the sender rule.
- Inbox rows show `#Notifications` before the conversation's labels.
- Editor: Edit and New filter, the action note, the service's validation
  error inline, Enter saves, Esc cancels; delete confirmation where Enter
  takes the focused Cancel.
- Filter settings: switch, model row (Download / Downloading… with Cancel
  download / Retry / Remove), your filters, New filter, Done.
- Palette: Move to inbox, Move to <filter>, Go to <filter>, New filter,
  Filter settings.

Not verified on Linux: a real download and a sort with the real model (the
container never loads the model), the remove-model flow, and desktop
notifications being held until sorting finishes (no notification server in
the container); that logic is covered by reading only.

### macOS shell, 18 Nov 2026 (previous Qwen3 4B engine)

`python3 apps/macos/Tests/run-filters-checks.py --library target/release/libmail_service.dylib --models <models dir> --sorted-store <sorted store>`
compiles the real EmailUI sources and drives the main window off screen
(captures with cacheDisplay; no window is shown). Verified: Set up filters with
filters off; the setup sheet (copy, four starters, size line, Escape closes,
Return turns on with a changed action and an unchecked starter); the real
model sorting the launch demo inside the app on Metal (Cold outreach 3,
Promotions 9, Notifications 13, nothing skipped left in the inbox); the
progress, missing-model and failed rows (from a set status); the filter view
(title, subtitle, reasons on rows, no search, Edit); the reader strip, Move to
inbox (row leaves, next row selected, back in the inbox without a filter) and
Move to from the inbox; palette commands; the editor (Escape, inline engine
error, save opens the new filter); Filter settings (Escape); focus back on the
list after each sheet; notifications skip mail a filter moved out of the inbox
(`NotificationTrackerChecks`). Not verified on macOS: progress from a real
model download (the sidebar rows were checked from a set status), Remove model, the delete
confirmations, right-click menus and the menu of Move to (its choices are
checked, the popup itself is not opened).
