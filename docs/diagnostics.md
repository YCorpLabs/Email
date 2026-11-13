# Local diagnostic export

Diagnostic export is explicitly opt-in. It creates a local JSON file; it does not
start the MCP server, open the mail service, run migrations, contact a provider,
read account configuration or the credential vault, or upload/send anything.

Quit Email and other MCP/scheduler processes using the cache before inspecting it.
Run the bundled `mail-mcp` executable with an explicit database and a new output:

```sh
mail-mcp --db /path/to/mail.sqlite --diagnostics /path/to/new-diagnostics.json
```

The destination must not already exist. On macOS/Linux it is created with mode
`0600`; an existing file or final-path symlink is refused. The destination's parent
directory must already exist and is chosen by the user. Sharing the file remains
a separate user action. Diagnostics cannot be combined with MCP permissions,
account selection, seeding, scheduled-send dispatch or other operations.

The versioned report contains the application/component version, a sanitized
build identifier when available, OS/CPU architecture, a fixed inspection code,
and—when inspection succeeds—SQLite `user_version`, a quick integrity-check
result and aggregate counts for accounts, threads, messages, drafts, draft
attachments, pending actions and fixed outbox status categories. Missing tables
produce null counts. A legacy cache is never upgraded to populate this report.
These aggregate counts still describe usage; review the report before sharing.

The report contains no database path, account/booking/message identifiers,
addresses, names, subjects, bodies, attachments, tokens, account configuration,
provider text, raw error messages or schema definitions. Unknown outbox statuses
are counted together rather than exported as text. No automatic analytics or
crash reporting is enabled by this command.

## Inspection limits

The exporter never opens the live database with SQLite. It opens only a regular
input file for reading, rejects final-path symlinks on macOS/Linux, and loads a
bounded copy into a read-only **in-memory** SQLite database. It does not create,
checkpoint or update live WAL/shared-memory files. Only the in-memory copy's
journal-mode header is normalized to let SQLite inspect a clean checkpoint.

A nonempty WAL or rollback journal produces `database_active_journal`, with no
counts or integrity claim. Close the application/MCP processes normally and try
again; do not delete journal files to force an inspection. Files larger than
128 MiB produce `database_size_limit`. A missing source produces
`database_unavailable` and is never created. Invalid/corrupt files, changed
metadata, unsupported schemas and resource limits produce fixed failure codes;
raw database errors are discarded. A report file may therefore be created
successfully even when its `database.code` says inspection was not completed.

Inspected snapshots use bounded SQLite schema/string limits, an approximately
five-million-instruction budget and a two-second SQLite progress deadline. The
JSON output is limited to 16 KiB. File reads and allocations are size bounded;
the SQLite progress deadline is not a hard deadline for filesystem I/O.

The copy is best-effort and does not lock or coordinate with live writers.
Before/after size and modification checks and journal checks detect common
changes, but they do not prove an online transactional snapshot. `integrity: ok`
means SQLite's quick check passed on that checkpoint copy, not a full corruption
assessment, backup certification, migration acceptance or provider reconciliation.
Use a normally stopped cache for meaningful results. Large or active caches may
only yield platform/version information and a fixed not-inspected code.
