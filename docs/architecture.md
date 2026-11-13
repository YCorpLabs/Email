# Architecture

## Product requirements

- macOS and Linux desktop first; Windows and iPhone later.
- Fast, keyboard-first unified inbox for Gmail, Microsoft, iCloud, and generic IMAP/SMTP.
- Native app chrome; isolated system webview for message HTML.
- Local cache and search; background sync; offline action queue.
- A local MCP server that uses the same actions as the UI.

## Ownership

- `mail-core` owns normalized account, folder, thread, message, and action models.
- Adapters translate Gmail API, Microsoft Graph, and IMAP/SMTP into the core model.
- UI shells consume core APIs and own only platform interactions and visual state.
- MCP consumes the same core API. It must not bypass account permissions or action checks.

## Rendering

Parse MIME, select the correct body alternative, resolve CID resources, sanitize HTML, and present it in WKWebView/WebKitGTK with scripts and unrequested remote loads blocked. Preserve safe styling and sender colors. Use a plain-text fallback.

## Performance rules

Load cached thread summaries before network sync; page and virtualize message lists; fetch bodies and attachments on demand; bound sync concurrency per account; keep idle timers sparse. Measure cold start, idle RSS/PSS and CPU, scroll response, search latency, and open-message cost on both platforms.

## Integration contract

The core should expose stable operations for list accounts, list threads, get message, search, compose draft, send draft, archive, mark read, move, label, and sync. All mutating actions should return a structured result and be safe to retry or reconcile after interrupted sync.
