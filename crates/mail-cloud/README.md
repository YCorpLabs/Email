# Cloud mail transports

`mail-cloud` connects directly to Gmail and Microsoft Graph. It does not persist credentials. The caller supplies an OAuth app registration and stores the returned `TokenSet` in an operating system credential store. After any API call, read `CloudClient::tokens()` and persist refreshed tokens.

## OAuth app registrations

- Google: create an OAuth **Desktop app** client in Google Cloud, enable the Gmail API, and configure the consent screen. Supply the client ID and any desktop client secret through `OAuthConfig`. The app uses a random local `127.0.0.1` port, PKCE, and the `gmail.modify` scope.
- Microsoft: register a public client application with delegated `Mail.ReadWrite`, `Mail.Send`, and `User.Read` permissions. Add a **Mobile and desktop applications** redirect URI of `http://localhost/callback` and allow personal Microsoft accounts if Hotmail/Outlook.com support is wanted. The app uses a random local port, PKCE, and `offline_access`.

Call `authorize_interactive` from a worker thread. It opens the system browser and waits up to five minutes for the callback. Then create `CloudClient`, call `identity()` for the account address/name, and use `sync_batch` with a saved cursor. Message and thread IDs include the account ID namespace. When a Google account is removed, `revoke_google_token` ends its grant at Google; it has a short timeout and callers should treat it as best effort.

## Sync behavior

Normal sync returns one provider page. On an expired Gmail history cursor (404) or Graph delta cursor (410), fresh inbox pages carry `reset_phase: Start`, `Continue`, and `Complete`; a one-page fresh reset uses `StartComplete`. The core store stages pages in SQLite and only replaces the old remote cache after completion; a failed or interrupted refetch leaves the existing inbox intact. Gmail uses a tagged reset cursor and Graph wraps its opaque next link so the next sync can resume staging without accumulating the entire inbox in memory.

Gmail body data and Graph HTML are returned as received. Sanitize HTML before rendering it in a webview. Gmail MIME decoding currently handles inline UTF-8 text/plain and text/html parts, with a 4 MiB decoded text budget and MIME depth/part limits; attachment-backed parts and other charsets need additional support. Attachment downloads are limited to 20 MiB decoded, and provider JSON responses to 32 MiB.

Attachment metadata includes `content_id` for CID resources. Gmail recognizes named MIME parts and unnamed inline images with a Content-ID; Graph includes inline attachments in its listing. Renderers should fetch only the CID resources referenced by the selected message and apply their own resource limit.
