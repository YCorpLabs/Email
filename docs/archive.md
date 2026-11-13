# Archive

Open Archive in the sidebar, or press G then A. Select a conversation and press E (or the Move to Inbox toolbar button) to restore it. The same E key archives mail while browsing the inbox. Moving a message updates the local view immediately and queues the provider operation. Gmail restore adds INBOX to the provider thread; archived mail is not deleted from Gmail.

Cached archived messages remain readable offline. Gmail archive pages load in the background, with additional pages fetched near the list bottom. Pages are limited to 25 full messages and use the shared Gmail quota budget. Gmail query excludes inbox, spam, trash and drafts; sent messages can appear because they are also outside the inbox. Archive cursor progress is stored separately from normal sync state. Other providers currently expose locally cached archived messages; older remote archive imports for them are not implemented.

Linux sidebar translucency uses RGBA background painting, not whole-window opacity. Blur is supplied by the compositor and is not globally enabled by the app. The Linux icon set consists of app-owned Cairo paths; Apple-native SF Symbols remain on macOS.

The Mac archive importer uses a dedicated background service connection, so it can progress independently of a long inbox sync while sharing the same per-account request budget. Empty/loading and retry status are inline, and the reader shortcut footer changes E to Move to Inbox in Archive.
