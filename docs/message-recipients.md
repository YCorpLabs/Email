# Message recipients

The reader displays the message To header, not the connected account address. Aliases remain visible. On both macOS and Linux, activate the To/date summary in the reader header to open a transient popover with selectable From, To, Cc, available Bcc, Reply-To and the full timestamp. The current message has no separate Message details block below the header. Older Mac conversation cards retain independent details; Linux updates the header disclosure when a historical message is selected.

Recipient metadata is persisted in an additive nullable SQLite column. Gmail, Microsoft Graph and IMAP imports retain Cc, available Bcc and Reply-To. RFC mailbox parsing preserves quoted commas and repeated headers. Bcc cannot be inferred when the provider does not supply it.

Existing Gmail cache entries fetch missing recipient headers once on opening the conversation. This metadata-only request uses the shared Gmail quota budget and updates only recipient fields. Cached bodies appear immediately; background results are checked against the current selection and do not replace the reader document or change read/archive state. Newly synced messages already contain the fields. Legacy Microsoft/IMAP entries receive additional fields when fetched through their normal sync path.

Regression coverage includes Gmail quoted names, aliases, repeated Cc headers and Reply-To, plus recipient metadata persistence across reopening the database without changing message bodies or read state.
