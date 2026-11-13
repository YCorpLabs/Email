# mail-service

The account service behind every Email front end. It combines the
[`mail-core`](../mail-core) cache with the [`mail-imap`](../mail-imap) and
[`mail-cloud`](../mail-cloud) adapters and adds account configuration, OS
credential storage (macOS Keychain, Linux Secret Service), sync coordination,
sanitized HTML rendering, attachments, drafts, scheduled sending and the other
operations a mail client needs.

Two ways to use it:

- **Rust:** `MailService::open(path)` opens a cache and its account settings;
  `MailService::request_json(json)` performs one operation. The Linux GTK app
  and the MCP server work this way.
- **C ABI:** [`include/mail_service.h`](include/mail_service.h) exports
  `mail_service_open`, `mail_service_request_json`, `mail_service_free_string`,
  `mail_service_close` and `mail_service_distribution_profile_json` (the
  build's [distribution profile](../../docs/distribution-profile.md)). The macOS app loads the `cdylib` this way, and any
  language with a C FFI can do the same.

Requests and responses are JSON. Every operation is listed in
[docs/service-api.md](../../docs/service-api.md). Calls can block on the network,
so run them off the UI thread. Passwords and OAuth tokens go only to the OS
credential store, never to SQLite or logs.

Open one store per process. Any store other than the default one gets its own
Scheduling credential, chosen by the first store the process opens.

For a quick look without an account, open a disposable database and send
`{"op":"seed_demo"}` to add three labelled demo messages, or
`{"op":"seed_demo","set":"launch","utc_offset_minutes":60}` for the
[launch demo mailbox](../../docs/demo-mailbox.md). The service refuses to seed
the default mail store, however its path is spelled. Demo messages with files
keep their MIME source in the demo store, which the service reads in place of a
provider. `cargo run --release -p mail-service --example demo_store -- --store
PATH` builds the 100,000-conversation search store and times inbox search.

```sh
cargo test -p mail-service --locked
```

MIT licensed; see [LICENSE](../../LICENSE).
