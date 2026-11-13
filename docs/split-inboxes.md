# Split inboxes

Splits are local saved inbox views. They do not move mail, modify provider labels, or create server-side forwarding rules. New matching cached messages appear automatically. The editor previews up to 500 matching cached conversations and lists saved splits for editing; previews make no provider requests.

- `*@example.com, *@example.org`: sender or recipient addresses at either domain.
- `from:*@example.org`: senders at this domain.
- `to:team@example.org`: messages addressed to an alias.
- `subject:invoice`: subject contains invoice.
- `label:Invoices`: exact label.
- `unread`: unread inbox conversations.
- `design review`: text in sender, subject, preview or label.

Comma/newline-separated rules are OR alternatives. Only `*` is a wildcard: SQL `%` and `_` remain literal. Addresses compare case-insensitively after removing display names; domain rules include the `@` boundary. Up to 16 alternatives / 256 bytes are allowed. Saved and previewed views use the same matcher, account/unread filters, snooze exclusions and pagination. Duplicate equivalent rule strings receive an error identifying the existing split. Splits currently include inbox mail, excluding snoozed conversations.

## Gmail pacing

The transport shares an in-process request budget across short-lived clients using an opaque credential digest. Full message requests cost 60 units; pacing targets 4,800 units/minute, below Google's newer 6,000 units/minute/user/project limit. A quota-specific HTTP 403 or HTTP 429 starts a shared cooldown, honors numeric Retry-After up to one hour, and leaves pending actions queued. Cached reads and local actions do not wait for this network budget. The regular sync timer retries; temporary throttling is shown inline rather than as a Mac modal error. This does not coordinate arbitrary external processes or other apps sharing Google's project/user quota, so rate-limit recovery remains necessary.
