# Local search

Inbox, account, archive, favorites, labels, snoozed views, saved splits, and MCP
use the same Rust search engine. Searching reads the SQLite cache only. It does
not fetch provider results, attachments, headers, or bodies while you type.
Results cover downloaded mail, not necessarily every message in the provider.
Older mail that has not been cached cannot match; a missing result is not proof
that the provider has no matching email.

## Syntax

Terms combine with AND. Use double quotes around a phrase or a field value
containing spaces. Examples:

```text
invoice "design review"
from:maya@example.org to:alex@example.com
cc:"Casey Copy" subject:"Design review"
label:"Work Invoices" is:unread has:attachment
in:archive after:2026-01-01 before:2026-02-01
is:starred
```

| Form | Meaning |
| --- | --- |
| Plain words | Literal FTS tokens in subject, sender, or plain-text body. All words must match the same cached message. |
| `"quoted phrase"` | Contiguous FTS phrase, using the cache tokenizer. |
| `from:value` | Sender display name/address contains the value. |
| `to:value` | Any cached To recipient contains the value. |
| `cc:value` | Any cached Cc recipient contains the value; Bcc is excluded. |
| `subject:value` | Subject contains the value. |
| `label:"name"` | Exact cached thread-label name, ignoring ASCII case. |
| `is:unread`, `is:read` | Conversation has unread messages, or has none. |
| `is:starred` | Conversation is locally favorited or has the provider's cached STARRED label. |
| `in:inbox`, `in:archive`, `in:spam`, `in:trash` | Cached conversation location; archive excludes spam/trash/sent/drafts. `in:junk` aliases spam. |
| `has:attachment` | Cached attachment flag is true, or a local draft has an attached file. Unknown flags do not match and never trigger a fetch. |
| `after:YYYY-MM-DD` | Cached message timestamp on or after UTC midnight of that date. |
| `before:YYYY-MM-DD` | Cached message timestamp strictly before UTC midnight of that date. |

Sender, recipient, and subject substring comparison uses SQLite's built-in
`lower`, which folds ASCII case; non-ASCII case variants are not fully folded
in these field filters. Plain full-text searches use FTS5's Unicode tokenizer.
Addresses and names are searched as stored, including display names. `*` is not
a search wildcard; saved split rules retain their separate wildcard grammar.
Unknown `prefix:value` text remains a literal full-text term rather than gaining
operator authority. Boolean OR, negation, fuzzy spelling, attachment contents,
Bcc search, and provider-wide search are not implemented.

Recognized operators with missing/invalid values return a clear error and do
not broaden the query. Unclosed quotes and invalid calendar dates also return
errors. Queries are bounded to 4,096 UTF-8 bytes and 64 terms. Backslash escapes
the following character inside quotes. Every SQL value is parameter bound;
FTS words/phrases are quoted rather than exposing raw FTS expression syntax.

## Scope and pagination

`MailStore::search_scoped(query, limit, offset, &SearchScope)` applies account,
inbox, unread, favorite, snooze, label, folder, and split constraints **before**
LIMIT/OFFSET. A view scope intersects query operators; it never silently changes
the selected account or broadens an agent's allowed accounts. Conflicting
filters correctly produce no results. Account allowlists distinguish `None`
(unrestricted) from `Some([])` (no accounts).

An empty `search_scoped` query lists its scope. Legacy `search`,
`search_in_folder`, and MCP `search_for_accounts` wrappers preserve their
empty-query/no-results behavior. Each page contains at most 500 conversations,
ordered by latest message time descending and conversation ID for stable ties.
The same conversation appears once even if several messages match. Sender,
recipient, subject, date, attachment, and plain-text predicates apply to the
same message; labels, placement, favorites, and unread state apply to its
conversation.

Inbox scope excludes future snoozes unless `snoozed_only` is requested. Archive
view uses the cached Archive folder, rather than treating every non-inbox
message as archived. A saved split uses the existing comma/newline OR rules and
wildcards, then intersects the search expression; searching cannot discard the
split's membership condition.

`has:attachment` and `cc:` rely on cached metadata. Old caches with unknown
attachment flags or missing recipient headers can temporarily have false
negatives until ordinary synchronization/backfill supplies the metadata. Search
never blocks on repairing those records.

## Validation

`cargo test -p mail-core` covers recognized operators, quoted phrases, strict
UTC date boundaries and leap days, malformed input, SQL/FTS-looking literals,
unknown attachment/header metadata, Bcc exclusion, draft attachments, mixed
account candidate windows, pre-limit unread/favorite/label/split filtering,
archive isolation, snoozes, stable pagination, and existing saved split rules.
Service and shell checks cover the shared request fields and view routing.


Rare words start from their full-text matches; common words walk the open
view newest first against a match list built once, and a walk through a view
that holds few of the matches switches to looking them up. `cargo test -p
mail-core --lib search::` compares every plan with the previous query
assembly on the launch demo corpus (results, order and pages) and checks the
SQLite query plans. Measurements on a realistic 100,000-conversation store,
cold and warm, are in [the 100k local-search benchmark](benchmarks/search-100k.md).

## Visual filter controls

Use the filter button beside search, **Refine search…** in Commands, or
Cmd+Shift+F on Mac / Ctrl+Shift+F on Linux. Fill From, To, Cc, Subject, On or
after, Before, read status and Has attachments; inspect the generated query
before applying it. Dates require strict `YYYY-MM-DD` Gregorian validation.
Invalid input disables Apply. Return is intended to apply from an edited field;
Escape cancels without replacing the current query.

Unique supported filters already present populate the controls. Quoted literals,
unsupported operators and duplicate predicates stay in the raw remainder,
preserving their meaning. Conflicting read filters, edits conflicting with raw
duplicate fields, or invalid/inverted dates produce errors; ambiguity never
silently deletes constraints. Emptying an extracted field intentionally removes
that field's constraint while retaining the other terms.

Coverage remains downloaded mail in the active view/account/split, with no
provider search.
