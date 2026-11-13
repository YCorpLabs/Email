# Local search benchmark

Quote the [13 November 2026](#realistic-corpus-13-november-2026) numbers. The
[9 November 2026 benchmark](#synthetic-corpus-9-november-2026-superseded) below
them only measured the best case and does not describe real mail.

## Realistic corpus, 13 November 2026

### Result

On the launch demo's 100,000-conversation search store, a search for a common
word in the inbox (the request both desktop apps send) now takes 3–5 ms at the
median and under 7 ms at P95 with a warm cache; it took 17–146 ms before. A
word with no matches, or one that only a few messages contain, takes under
0.5 ms (about 50 ms before). Nothing about the results changed: same
conversations, same order, same pages.

The first search after the store's pages leave every cache (after a restart,
for example) reads from disk and takes about 75–310 ms for common words.

### What was slow, and what changed

Search shows conversations newest first, 100 at a time, filtered to the open
view (inbox, a split, a label, and so on). Before this change it chose between
two plans by counting up to 1,001 full-text matches:

- **More than 1,000 matching messages:** walk the view's conversations newest
  first and run a separate full-text `MATCH` for every message on the way. At
  about 1% frequency (`Brightwater`, `Elena`) that is roughly 7,500 probes of
  about 17 µs each for one page: 126–146 ms.
- **1,000 or fewer:** join the matches to the conversations and let SQLite pick
  the order. With the inbox scope it picked the inbox index and checked every
  inbox conversation against the match list instead of starting from the
  matches: about 50 ms even when nothing matched (0.06 ms without the scope).

`EXPLAIN QUERY PLAN` on the 100k store, before (snooze checks omitted):

```text
-- 1,000 or fewer matches (nonexistentterm, inbox)
SEARCH t USING INDEX threads_inbox_time (is_inbox=?)
SEARCH m USING COVERING INDEX messages_thread (thread_id=?)
LIST SUBQUERY 2
  SCAN message_search VIRTUAL TABLE INDEX 0:M4
USE TEMP B-TREE FOR DISTINCT
USE TEMP B-TREE FOR LAST TERM OF ORDER BY

-- More than 1,000 matches (Brightwater, inbox)
SEARCH t USING INDEX threads_inbox_time (is_inbox=?)
CORRELATED SCALAR SUBQUERY 3
  SEARCH m USING COVERING INDEX messages_thread (thread_id=?)
  SCAN message_search EXISTS VIRTUAL TABLE INDEX 0:=M4
USE TEMP B-TREE FOR LAST TERM OF ORDER BY
```

Now (`crates/mail-core/src/search.rs`) there are two plans, both written so
SQLite cannot reorder them:

- **Matches** (rare words): read the full-text matches, look up each match's
  message and conversation, keep those in the view and sort them. A `CROSS
  JOIN` from a materialized match set makes the matches the outer loop.

  ```text
  MATERIALIZE hits
    SCAN message_search VIRTUAL TABLE INDEX 0:M4
    SEARCH m USING INTEGER PRIMARY KEY (rowid=?)
  SCAN hits
  SEARCH t USING INDEX sqlite_autoindex_threads_1 (id=?)
  USE TEMP B-TREE FOR ORDER BY
  ```

- **Walk** (common words): build the list of matching message IDs once, then
  walk the view newest first and stop after one page. Checking a conversation
  against the list replaces the per-message `MATCH`.

  ```text
  SEARCH t USING INDEX threads_inbox_time (is_inbox=?)
  CORRELATED SCALAR SUBQUERY 3
    SEARCH m USING COVERING INDEX messages_thread (thread_id=?)
    LIST SUBQUERY 2
      SCAN message_search VIRTUAL TABLE INDEX 0:M4
      CREATE BLOOM FILTER
  USE TEMP B-TREE FOR LAST TERM OF ORDER BY
  ```

**Choosing.** Looking up one match costs about as much as walking four
conversations. A walk finds a page after about `page × messages ÷ matches`
conversations if matches are spread through the mailbox, so lookups win while
`matches² ≤ page × messages ÷ 4` (about 1,660 matching messages for the first
page of 100 in this store; more for later pages). The search counts matching
message IDs (never message text) up to that bound, and further up to a cap
(about 25,000 here, at most 32,768) for the next step.

**Views that hold few of the matches.** A word can be common in the mailbox and
rare in a split, label or Favorites. Between the bound and the cap, the walk
runs under SQLite's progress handler. Once it has used four times its expected
steps and the rest of it would cost more than looking up every match, it stops
and the matches are looked up instead. Counting is in SQLite virtual machine
steps, so the choice does not depend on timing. Above the cap a lookup cannot
win, and the walk always runs to the end. Follow-ups are sorted by due date,
so their walk always reads every conversation; there, lookups are used up to
one tenth of the conversation count.

**Calibration.** Measured on the 100k store with SQLite's per-statement VM step
counter while forcing each plan: a lookup takes about 27 steps per match at
about 45 ns per step; a walk about 25 steps per conversation at about 16 ns per
step. The constants are `LOOKUP_STEPS = 100` (in walk steps), `WALK_STEPS =
25`, `LIST_STEPS = 5` and `EXPLORE_FACTOR = 4`. The per-view table below shows
the times each plan gives.

### Results are unchanged

`cargo test -p mail-core --lib search::` runs every plan (lookups, walk, the
automatic choice) against the previous query assembly, kept in the test module,
on the launch demo plus 3,000 older conversations: 51 queries (common, rare,
absent, prefixes as you type, phrases, several words, every operator,
Norwegian with and without diacritics, punctuation) in 17 views (inbox, all
mail, not inbox, account inbox, account all mail, unread, Favorites, Snoozed,
Follow-ups, two labels, Archive, Sent, four splits), each at four pages (100 at
0, 100 at 100, 37 at 250, 500 at 0): 3,468 searches, each run by both plans
and the automatic choice and compared with the previous assembly on every
field of every row. Other tests check the query plans above, the automatic choice,
a stopped walk on 20,000 conversations, and that a stopped walk leaves no
progress handler behind.

On 100,000 conversations (`cargo test --release -p mail-core --lib
plans_match_legacy_on_100k -- --ignored`), the same comparison at two pages
(100 at 0, 37 at 250): 1,734 searches, 5,202 comparisons, all equal.

### Method

- **Machine:** Apple M3 Max (16 cores), 48 GB, internal SSD (APFS), macOS
  27.0.1. Not idle: other applications, a Linux VM and other builds kept the
  one-minute load average between 5 and 14 during the runs.
- **Build:** release, Rust 1.99.0, rusqlite 0.41.1 with bundled SQLite 3.54.1.
  Before: the engine at `1bec82b` (main). After: the engine at this change.
  Both binaries were built from the same `demo_store` example, so only the
  engine differs.
- **Store:** `cargo run --release -p mail-service --example demo_store --
  --store <new dir>/mail.sqlite`: the launch demo plus 100,000 older
  conversations, 110,162 messages, 160 MB. All of the history is in the inbox.
  A common word matches 1–18% of messages (`invoice` 18,037 messages,
  `faktura` 6,176, `Ingrid` 5,721, `Brightwater` 1,374, `Elena` 1,354).
- **Request:** `MailService::request_json` with the search request the apps
  send (`is_inbox: true`, limit 100, offset 0), or without `is_inbox` as the
  MCP server sends it. It includes reading labels, snooze and pin state for
  each row and the JSON result, not the apps' 120 ms typing debounce or drawing.
- **Warm:** `--bench-only`; each query runs 31 times, the first is reported
  separately and P50/P95 come from the other 30. Three rounds, alternating the
  before and after binaries on the same store; tables show the median of the
  three rounds.
- **Cold:** `--bench-only --cold`, one run per binary; 30 samples per query, P50
  the 16th and P95 the 29th sorted sample as for warm; each sample the first request
  on a newly opened service over a fresh APFS clone of the store. A clone
  shares the store's blocks on disk but none of its cached pages, so the
  request reads from the SSD. (On Linux, `--cold` drops the store's cached
  pages with `posix_fadvise` instead; not run for this record.)

### Inbox, warm

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

`from:ingrid` and `is:unread subject:invoice` have no free-text word, so they
use the unchanged operator walk: `subject:` and `from:` are substring matches
that the full-text index cannot answer.

### All folders (as the MCP server searches), warm

| Query | Before P50 ms | Before P95 ms | After P50 ms | After P95 ms |
| --- | ---: | ---: | ---: | ---: |
| `invoice` | 18.2 | 19.4 | 3.6 | 4.2 |
| `faktura` | 45.6 | 46.4 | 2.9 | 4.0 |
| `Ingrid` | 44.0 | 45.5 | 2.7 | 3.4 |
| `Brightwater` | 118.2 | 124.5 | 4.4 | 6.5 |
| `Elena` | 137.7 | 140.7 | 4.7 | 6.1 |
| `from:ingrid` | 2.8 | 4.0 | 2.6 | 3.6 |
| `is:unread subject:invoice` | 23.7 | 25.8 | 23.5 | 25.9 |
| `"launch plan"` | 0.41 | 0.65 | 0.44 | 0.51 |
| `nonexistentterm` | 0.06 | 0.07 | 0.07 | 0.08 |

### Typing, warm

Each keystroke is its own search once the apps' 120 ms debounce passes.
Prefixes are separate words to the full-text index, so most match nothing.
Inbox P50 in ms, median of two rounds:

| Typed | Before | After |
| --- | ---: | ---: |
| `i`, `in` | 5.9, 6.5 | 5.4, 4.8 |
| `inv` … `invoic` | 49–50 | under 0.1 |
| `invoice` | 16.3 | 3.9 |
| `E` … `Elen` | 49–51 | under 0.1 |
| `Elena` | 142.1 | 4.8 |
| `f` … `faktur` | 49–51 | under 0.1 |
| `faktura` | 43.6 | 3.3 |

### Cold

| Query | Inbox before P50/P95 ms | Inbox after P50/P95 ms | All folders before P50/P95 ms | All folders after P50/P95 ms |
| --- | ---: | ---: | ---: | ---: |
| `invoice` | 95 / 97 | 79 / 84 | 91 / 94 | 75 / 91 |
| `faktura` | 167 / 179 | 120 / 177 | 164 / 181 | 113 / 116 |
| `Ingrid` | 165 / 172 | 117 / 118 | 162 / 171 | 113 / 117 |
| `Brightwater` | 266 / 291 | 270 / 306 | 247 / 258 | 268 / 285 |
| `Elena` | 289 / 301 | 268 / 306 | 271 / 294 | 266 / 279 |
| `from:ingrid` | 116 / 120 | 115 / 122 | 116 / 118 | 114 / 123 |
| `is:unread subject:invoice` | 365 / 417 | 361 / 372 | 371 / 383 | 367 / 478 |
| `"launch plan"` | 419 / 435 | 6.2 / 7.2 | 5.8 / 6.3 | 6.1 / 6.7 |
| `nonexistentterm` | 418 / 437 | 3.0 / 4.0 | 2.8 / 3.0 | 3.0 / 3.3 |
| inbox listing | 1.6 / 2.6 | 1.8 / 2.2 | 1.5 / 1.8 | 1.4 / 1.8 |

Cold time is reading pages from the SSD; load average 6–8. Words at about 1%
(`Brightwater`, `Elena`) are about as slow as before (−7% to +9% here). An
earlier run, which took the 15th rather than the 16th of 30 sorted samples as
P50, had them 4–21% slower. Looking up 1,374 matches reads message and
conversation rows scattered through the file, where the old walk read
conversations in file order.

### Other views (engine only)

`cargo test --release -p mail-core --lib plan_timings_on_100k -- --ignored
--nocapture` times the engine alone (no JSON) in every view the apps search, on
a fresh 100,000-conversation corpus where some history is archived, labelled,
starred, snoozed or followed up. Median of 9 warm runs, before → after, in ms;
the test prints 10 queries per view and which plan ran. Same machine, load
average about 4–9.

| View | `invoice` | `faktura` | `Brightwater` | `the` | `nonexistentterm` | `invoice from:cirrusbay` |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Inbox | 14.6 → 3.2 | 43.1 → 2.7 | 131.0 → 2.4 | 4.0 → 8.3 | 41.5 → 0.08 | 6.8 → 6.0 |
| All mail | 17.3 → 3.1 | 43.7 → 2.1 | 113.5 → 2.1 | 3.6 → 8.1 | 0.06 → 0.07 | 5.7 → 5.0 |
| Not in inbox | 14.2 → 3.0 | 40.4 → 2.3 | 149.2 → 2.1 | 3.6 → 8.1 | 11.7 → 0.07 | 5.2 → 4.6 |
| Work account inbox | 13.0 → 3.2 | 96.0 → 4.6 | 103.5 → 2.4 | 3.2 → 8.2 | 36.2 → 0.08 | 6.3 → 5.6 |
| Personal account, all mail | 20.8 → 3.1 | 15.3 → 1.6 | 596.1 → 1.6 | 7.4 → 8.2 | 6.7 → 0.07 | 18.4 → 22.0 |
| Unread in inbox | 19.2 → 7.9 | 52.2 → 13.9 | 71.5 → 1.7 | 5.0 → 9.0 | 14.4 → 0.09 | 19.0 → 17.7 |
| Favorites | 21.9 → 9.8 | 54.8 → 15.5 | 127.3 → 2.4 | 5.7 → 9.8 | 0.07 → 0.08 | 38.8 → 21.3 |
| Snoozed | 17.9 → 7.8 | 65.5 → 12.8 | 69.0 → 1.4 | 5.2 → 9.5 | 0.06 → 0.08 | 21.3 → 19.2 |
| Follow-ups | 18.3 → 18.2 | 17.3 → 7.7 | 17.8 → 1.6 | 18.1 → 24.1 | 0.38 → 0.39 | 15.8 → 18.2 |
| Label Receipts | 3.4 → 3.2 | 5.8 → 2.0 | 324.6 → 1.8 | 9.4 → 9.3 | 0.07 → 0.07 | 4.1 → 4.4 |
| Label Work | 21.4 → 29.7 | 21.4 → 13.6 | 21.2 → 1.5 | 21.2 → 27.2 | 0.06 → 0.07 | 19.9 → 20.2 |
| Archive | 14.8 → 3.5 | 40.9 → 3.3 | 163.3 → 2.0 | 3.8 → 8.2 | 16.1 → 0.08 | 6.9 → 6.3 |
| Sent | 21.9 → 5.2 | 60.0 → 9.1 | 49.3 → 3.2 | 3.8 → 8.9 | 0.07 → 0.07 | 52.4 → 25.2 |
| Split Newsletters | 38.0 → 37.4 | 37.8 → 18.4 | 37.7 → 1.9 | 38.8 → 46.3 | 38.6 → 0.10 | 38.7 → 24.2 |
| Split Notifications | 855.4 → 100.7 | 713.3 → 36.9 | 738.0 → 7.7 | 6.3 → 11.0 | 258.1 → 0.22 | 267.0 → 34.3 |
| Split Promotions | 37.5 → 36.8 | 39.0 → 17.2 | 39.1 → 2.4 | 38.0 → 45.0 | 38.1 → 0.11 | 37.5 → 23.6 |
| Split Team | 15.2 → 4.7 | 118.1 → 12.2 | 96.3 → 3.1 | 3.2 → 8.2 | 51.5 → 0.10 | 62.2 → 23.6 |

Across these 17 views and 10 queries, total engine time went from 9.5 s to
1.8 s. Slower than before: very common words (`the`, see Limits) in every
view, `Alex` (38% of messages) in Sent and the Team split (3.5 → 5.8 ms),
and two cases where both plans cost about the same and the walk stopped
partway before the lookup: `invoice` in the Work label (21 → 30 ms) and
`invoice from:cirrusbay` across the personal account (18 → 22 ms). In the
Notifications split, `invoice` went from 855 to 101 ms because the walk
stopped and looked the matches up.

### Memory, sync

Peak resident memory of the warm inbox benchmark process: 14.5 MB before,
15.2 MB after (the match list and match set are SQLite temporary tables, held
in its page cache). Seeding the 100,000 conversations, which uses the same
message write path as sync, took 9.7–9.8 s with both binaries and peaked at
about 17 MB in both; no write, sync or index code changed.

### Limits

- **Very common words.** A word in more than about half of all messages (`the`
  in 74,000 of 110,000) takes about 8 ms of engine time instead of about 4 ms in
  the inbox, and 5–8 ms more in views walked to the end (Follow-ups, the Work
  label, the Newsletters and Promotions splits), because the search counts
  matches up to the cap and the walk builds the list of every match first. The
  old per-message probe was cheaper for such words. They are rarely searched
  alone.
- **Common words in narrow views with expensive split rules.** `Alex` (42,000
  messages, above the cap) in the Notifications split, whose address rules read
  every message's recipients, still walks the whole inbox: about 265 ms of
  engine time, down from about 705 ms. Looking the matches up would take about
  160 ms.
- **Cold start.** The first, uncached search still reads from disk: 75–310 ms
  for common words. Words at about 1% gained nothing cold and were up to a
  fifth slower in one of two runs (see Cold).
- **Operator-only searches** (`is:unread subject:invoice`) are unchanged.
- **Not measured:** Linux (the engine is shared; the container ran the tests,
  not the benchmark), real mailboxes, mailboxes much larger than 100,000
  conversations, and drawing the results in either app.

## Synthetic corpus, 9 November 2026 (superseded)

**Limitation:** this corpus puts "invoice" and "newsletter" in every message,
so the first 100 conversations always match and every common-word search stops
after 100 rows. It measures only that best case. On realistic mail, where a
common word is in 1–18% of messages, the same engine took 17–146 ms for word
searches (above). Do not quote 2.7 ms as search latency.

Build `dev-f99989a10e9e`. Release Rust build, macOS ARM64, SQLite cache only.
The included [runner](search-100k.rs) creates a temporary database, seeds 100,000
synthetic single-message conversations across four accounts, requests 100 results
for each query, then removes the database. It retains the demo seed as fixture data.
Create a temporary Cargo binary project depending on this repository's
`crates/mail-core`, copy the runner into `src/main.rs`, and run
`cargo run --release -- 100000` to reproduce the current implementation.

Each query runs 31 times: the first is reported separately, then P50/P95 are
selected from 30 sorted warm samples. First does not mean cold disk/OS cache.

| Query | Interim P50 ms | Final P50 ms | Final P95 ms |
| --- | ---: | ---: | ---: |
| invoice | 269.244 | 2.703 | 2.937 |
| newsletter | 270.275 | 2.686 | 2.901 |
| from:billing@example.org | 92.074 | 0.800 | 1.053 |
| is:unread subject:invoice | 83.335 | 1.503 | 1.739 |
| to:alex@example.com | 284.589 | 0.535 | 0.610 |
| nonexistentterm | 0.027 | 0.043 | 0.048 |
| empty scope listing | not measured | 0.467 | 0.483 |

The baseline is our interim structured-search JOIN plan, not any competitor.
The final implementation probes bounded FTS candidates and chooses between
selective FTS joins and indexed conversation-order scans. This avoids sorting
large common-term result sets before returning a small page.

Raw [baseline](email-search-bench-100k.txt) and
[final output](email-search-bench-100k-after.txt) are preserved. Seed timings
are not comparable ingestion benchmarks: concurrent builds and new indexes
changed their conditions. These numbers exclude UI debounce/rendering, IPC,
provider requests, real heterogeneous mailbox distributions and memory profiling.
They support a roughly 100-fold common-query engine improvement against the
interim plan, not a global superiority or production-latency claim.
