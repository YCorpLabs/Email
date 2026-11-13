# Discarding drafts

Drafts can be discarded from the Drafts list or from the composer, in both
native shells, without a confirmation dialog. Undo is always available instead.

| Where | macOS | Linux |
| --- | --- | --- |
| Drafts list | ⌫, ⌦ or `#` | Delete, Backspace or `#` |
| Right-click a draft | Edit draft / Discard draft | Edit draft / Discard draft |
| Command palette | Edit draft, Discard draft, Undo discard | same |
| Composer | trash button, palette "Discard draft" | trash button, palette "Discard draft" |
| Undo | `Z` in Drafts, or the "Draft discarded · Undo Z" bar | same |

Selection moves to the draft that took the discarded one's place, and list focus
is kept. The hint bar shows ⌫ Discard, and Z Undo discard after a discard. A
composer discard first saves on-screen edits, so Undo restores exactly what was
visible. An untouched new message has nothing to restore and simply closes. A
discard made outside Drafts (for example, from a reply composer) says to undo
from Drafts.

## Engine behavior

`discard_draft` removes an unsent **local** draft from Drafts, search and its
reply conversation (the conversation itself stays). Its content, attachments and
open-tracking choice are kept in a private snapshot for 7 days.
`restore_discarded_draft` puts it back exactly (single use). These are refused:

- drafts queued, scheduled or being sent (cancel first);
- sent mail and received messages;
- provider-stored drafts (IDs not starting with `local:`).

Email never imports Gmail drafts as drafts: initial sync excludes them, and a
full fetch maps them as ordinary messages. So a discard is entirely local and
nothing comes back on the next sync. MCP exposes both operations under
`--allow-draft`; restores are scoped to the discarded draft's account.
