# mail-core

The provider-independent core of the Email engine: the normalized mail model
(accounts, messages, threads, actions) and `MailStore`, a SQLite cache with
full-text search, local drafts, a durable outbox for scheduled and undoable
sends, labels, saved split inboxes, snoozes and an action queue that is replayed
against the provider after interrupted sync.

Provider adapters implement the `MailAdapter` trait and return a `SyncBatch`.
`MailStore::run_sync` applies batches and stages cursor resets so a failed
refetch never empties the existing cache. This repository ships two adapters:
[`mail-imap`](../mail-imap) for iCloud and standard IMAP/SMTP, and
[`mail-cloud`](../mail-cloud) for Gmail and Microsoft Graph.

`mail-core` does not know where the desktop apps keep their default store, so
its `seed_demo` (in Rust and the C ABI) seeds whatever store it is given;
`mail-service` refuses to seed the default one.

`mail-core` performs no network I/O and stores no credentials. Front ends that
need account setup, OS credential storage and sync should use
[`mail-service`](../mail-service), which wraps this crate. Non-Rust callers can
use the small JSON C ABI in [`include/mail_core.h`](include/mail_core.h).

```sh
cargo test -p mail-core --locked
```

See [docs/architecture.md](../../docs/architecture.md) for how the crates fit
together. MIT licensed; see [LICENSE](../../LICENSE).
