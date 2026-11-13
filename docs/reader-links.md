# Links in the message reader

Clicking a link in a message opens it in the system default handler. The reader
itself never navigates away from the message.

| Navigation | macOS | Linux |
| --- | --- | --- |
| The app's own document load (`about:`) | Loads in the reader | Same |
| Plain click on an `http` or `https` link | Opens in the default browser; reader stays put | Same |
| Plain click on a `mailto` link | Rebuilt with only the recipient and `to`, `cc`, `bcc`, `subject`, `body`, then opened in the default mail app | Same rules and test table |
| Malformed or empty `mailto` link | Nothing | Same |
| Any other scheme (`javascript:`, `file:`, `ftp:`, `data:`, `tel:` …) | Nothing | Same |
| Navigation the user did not start (redirect, meta refresh) | Nothing | Same |
| Command-click | Opens | Ctrl/middle-click expected to do nothing (see Linux below) |
| `target=_blank` that survives sanitizing (none today) | Opens | Expected to do nothing |
| Clicked in-page `#anchor` | Policy cancels it; WebKit still updates the fragment | Allowed (`about:`) |

Linux does this in `new_html_reader` (`apps/linux/src/main.rs`). macOS uses
`ReaderLinkPolicy` (`apps/macos/Sources/EmailUI/ReaderLinkPolicy.swift`) from
the reader's `decidePolicyFor` handler. It allows `about:` only for non-user
loads, and only `.linkActivated` with `http`/`https` (with a host) or a
sanitized `mailto` reaches `NSWorkspace.shared.open`.

The service sanitizer (ammonia defaults) already strips `target`, `javascript:`
and `file:` from message HTML before either shell sees it. The shell policies
are the second layer.

## mailto

Neither shell has an in-app compose route for `mailto:` links, and Email does
not register as a `mailto` handler. On both platforms a clicked `mailto:` link
opens whatever mail app the OS has as default. No parity gap; an in-app route
would be new work.

Before a `mailto:` link reaches the OS, both shells rebuild it
(`ReaderLinkPolicy.sanitizedMailto` on macOS, `reader_links::sanitized_mailto`
on Linux). Some mail clients have honoured `attach=` and attached local files
from a link (Evolution and KMail CVEs), so this is defence in depth:
- Keep the recipient and the `to`, `cc`, `bcc`, `subject` and `body`
  fields, with names matched case-insensitively. Drop everything else
  (`attach`, `attachment`, unknown keys), empty values and the fragment.
- Decode each kept value and re-encode it: only ASCII letters, digits and
  `-._~@,` stay literal. A literal `?`, `&` or `=` inside a value or the
  address, such as `subject=x?attach=…` or `mailto:a@x&attach=…`, therefore
  can't be read as a separate field by a lenient client.
- Open nothing when the link has no recipient in the address or `to`/`cc`/`bcc`
  (empty or only Unicode White_Space), has invalid percent encoding or UTF-8,
  or has a control character. CR, LF and tab are allowed in `body` only.
- `+` is kept as a plus sign; mailto has no form encoding.
- Percent escapes are decoded byte by byte by the shell's own code, not by a
  platform URL library, and every decoded character is kept, including a
  leading U+FEFF (byte-order mark). Foundation's `removingPercentEncoding`
  drops that character on macOS 26 but not on later versions, so macOS doesn't
  use it and gives the same result on every supported version.

Both implementations split the raw string on ASCII `#`, `?`, `&` and `=` and
check the same 34-case table (`mailtoCases` in
`apps/macos/Tests/ReaderLinkPolicyChecks.swift`, `MAILTO_CASES` in
`apps/linux/src/reader_links.rs`; edit both together). The
**Unsubscribe** button also hands message-supplied `mailto:` links to the OS,
so in both shells it skips a candidate the sanitizer rejects (falling back to
the next header entry or page link) and opens the rebuilt link.

Residual risk: a client that percent-decodes the whole link before splitting
it could still read an encoded `%26attach%3D…` inside `subject` or `body` as a
field. That can't be prevented without altering legitimate text.

## macOS tests

- `scripts/check-release-swift.py` runs `apps/macos/Tests/ReaderLinkPolicyChecks.swift`:
  allowed and blocked schemes, user versus non-user navigations, `about:`
  handling and the 34 mailto cases.
- `apps/macos/Tests/run-reader-link-checks.py --library target/release/libmail_service.dylib`
  compiles the actual EmailUI sources into a real `MailWindow`, with a
  disposable `EMAIL_STORE_PATH`, `EMAIL_DEMO=1` and the worktree-built
  `libmail_service.dylib`. It gives the demo message an HTML body with links,
  selects it in the inbox, and clicks links with mouse events that AppKit
  hit-tests to the reader's WKWebView, delivered directly rather than through
  `NSWindow.sendEvent` because the harness window is not key. The opener is
  replaced by a recorder. It checks that:
  - The service-sanitized message keeps the https link and strips `target`,
    `javascript:` and `file:` hrefs.
  - Clicking https, the formerly `target=_blank` link and `mailto` each opens
    exactly that URL once (no second open within 0.75 s). Command-click opens too.
  - A clicked `mailto:` with `subject`, `attach=/etc/hosts`, `X-Extra` and
    `body` opens as `mailto:hello@studio.example?subject=Hi&body=Two%20lines`.
    `mailto:?subject=…` (no recipient) opens nothing.
  - Clicking the stripped script/file text and an in-page `#anchor` opens nothing.
  - With an unsanitized fixture, a `target=_blank` link reaches
    `decidePolicyFor` with `targetFrame == nil` and opens. Cancelling it there
    means WebKit never asks for a new web view, so no `WKUIDelegate` is needed.
  - Unsanitized `javascript:` and `file:` links never reach the policy (WebKit
    blocks them first); `ftp:` reaches it and is cancelled. A meta refresh to
    https arrives as `.other` and opens nothing. Unsanitized `mailto:` and a
    `mailto:` with an encoded CRLF in the address are asserted to reach the
    policy, and open nothing.
  - After every click the reader still shows the same `about:blank` document.
    The anchor click updates only the fragment.
- With `--open-in-browser`, one link to a local `http://127.0.0.1` server opens
  in the real default browser.

## Linux

On Linux the `mailto` sanitizer in `apps/linux/src/reader_links.rs` is used by
the reader's `decide-policy` handler and by Unsubscribe (candidate choice and
launch). Two differences follow from the code:
- The Linux handler only acts on `NavigationAction` decisions. WebKitGTK reports
  `target=_blank` and modifier-clicks as `NewWindowAction`, which falls through
  to WebKit's default. With no `create` handler, that is expected to open
  nothing. Sanitized mail never carries `target`, so normal messages are
  unaffected, but a modifier-click on Linux probably does nothing while macOS
  opens the link.
- Linux allows every `about:` navigation, including clicked in-page anchors.
  macOS cancels clicked `about:` links but WebKit still applies the fragment.
