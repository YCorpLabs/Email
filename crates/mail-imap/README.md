# IMAP and SMTP connector

`mail-imap` connects iCloud and generic IMAP accounts to `mail-core`. It uses TLS for every connection. `ImapConfig::icloud` fills Apple's documented IMAP (993, implicit TLS) and SMTP (587, STARTTLS) settings; it uses the mailbox local part for IMAP and the full address for SMTP. The user supplies an app-specific password at runtime. The caller owns OS credential storage and must never persist `ImapConfig.secret` to the mail database.

## API

- `ImapConnector::new(config, provider)` validates configuration. Provider must be `Icloud` or `Imap`.
- `test_connection()` logs into IMAP, selects INBOX, and verifies SMTP with NOOP. It blocks the caller.
- `MailAdapter::sync()` pages through new and older INBOX messages. IDs include account ID, UIDVALIDITY and UID. The cursor is persisted by `MailStore::run_sync`.
- `send(from, to, subject, plain, html)` sends via authenticated TLS SMTP. It blocks the caller.
- `apply_action(store, action)` handles mark-read and UID MOVE for archive/move when the server advertises MOVE. It refuses a copy/delete fallback that could expunge unrelated messages.
- `attachment_metadata(raw)` lists MIME attachment and inline resource metadata without retaining bytes.

Call all network methods on a background worker. `body_html` is raw sender-supplied HTML. Sanitize and isolate it before rendering. The connector currently syncs INBOX only; it does not reconcile remote deletions/moves, send drafts automatically, or download attachment payloads into `mail-core`. These need to be built before claiming full mailbox parity.

The unit tests use MIME fixtures and cursor simulations. No live mailbox has been exercised in this repository because no account credentials are present.
