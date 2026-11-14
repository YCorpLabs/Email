# Email documentation

Start with [Building and running Email](building.md): it gets a fresh clone
running with demo data, no account or signing setup needed.

Project policies live in the repository root:
[contributing](../CONTRIBUTING.md), [support](../SUPPORT.md),
[security reporting](../SECURITY.md), [trademarks](../TRADEMARKS.md) and
[forking](../FORKING.md). Coding agents also follow the
[agent guide](../AGENTS.md).

## Using Email

- [Connect Gmail with an app password](gmail-app-password.md).
- [Microsoft 365 admin consent](microsoft-365-admin-consent.md), for builds
  that offer Microsoft accounts.
- [Delete your data](delete-your-data.md).
- [Support and feedback](support-feedback.md).

## Contributor guides

### Build, run and test

- [Building and running Email](building.md): requirements, demo mode, macOS
  and Linux builds, the Linux container build, tests, a map of the source tree
  and common errors.
- [Launch demo mailbox](demo-mailbox.md): a synthetic, busy mailbox for
  recordings and screenshots, a 100,000-conversation search store, and the
  agent demo.
- [Distribution profiles](distribution-profile.md): how a build's identity and
  services are chosen; plain builds use the neutral `unofficial` profile.
- Release packaging: [release versions](release-versions.md),
  [macOS distribution builds](macos-release.md),
  [macOS automatic updates](macos-updates.md) and the
  [AUR package](linux-aur.md).

### How the engine works

- [Architecture](architecture.md): requirements, ownership between crates and
  apps, rendering and performance rules.
- [Repository boundaries](repository-layout.md): what lives here and what lives
  in the separate Scheduling and website projects.
- [Native service JSON API](service-api.md): every `mail-service` operation,
  for front ends using Rust or the C ABI.
- [Local MCP server](mcp.md): tools, permission grants and registration.
- [Local search](search.md) and its [benchmark](benchmarks/search-100k.md),
  [Gmail history sync](gmail-metadata-sync.md),
  [message workflow correctness](message-workflows.md) and
  [reader responsiveness](reader-responsiveness.md).
- Crate READMEs: [mail-core](../crates/mail-core/README.md),
  [mail-imap](../crates/mail-imap/README.md),
  [mail-cloud](../crates/mail-cloud/README.md) (includes OAuth app
  registration) and [mail-service](../crates/mail-service/README.md).

### Security and privacy

- The root [README](../README.md#privacy-and-security-model) summarizes HTML
  isolation, credential storage and resource limits.
- [Reader privacy defaults](reader-privacy.md) and
  [links in the message reader](reader-links.md).
- [MCP](mcp.md) documents the read-only default and each write grant;
  [MCP security](mcp-security.md) is the threat model, and
  [agent access](agent-access.md) covers per-account agent permissions.
- [Local diagnostic export](diagnostics.md) lists exactly what a diagnostic
  file contains.

### Product behavior and design

- [Interface direction](design.md) and [app icon](app-icon.md).
- [Account setup](account-setup.md): Gmail app passwords, the Google sign-in
  gate and which features each build offers.
- [First run](first-run.md): the welcome before the first account and when
  Calendar is offered.
- Features: [archive](archive.md), [bulk triage](bulk-triage.md),
  [desktop notifications](desktop-notifications.md),
  [discarding drafts](draft-discard.md), [draft navigation](draft-navigation.md),
  [follow-up reminders](followups.md), [message recipients](message-recipients.md),
  [split inboxes](split-inboxes.md), [unread counts and sender cleanup](unread-cleanup.md)
  and [connected calendar](connected-calendar.md).
