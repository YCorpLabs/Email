# First run

What Email shows before the first account, on macOS (the reference) and Linux.
Account setup itself is in [account-setup.md](account-setup.md).

## With no account

The window is one welcome: **Welcome to Email**, "Add your Gmail, iCloud or
other email account to get started." (Outlook is named where the build offers
it) and one **Add an account** button in the app's primary style (accent fill;
dark text on a light fill in dark mode, white on dark in light mode).

- The message list is hidden, so its "Connect an account…" line no longer
  repeats the welcome, and the welcome has the window's full width.
- The shortcut strip (J K, Read, Reply, Archive, ⌘K) is hidden until there is
  mail.
- The sidebar keeps **Add account**, **Support & feedback** and **Commands**
  (and Appearance on Linux). Compose, the Workspace folders, Splits, the
  account list, Manage accounts and Agent access return with the first account.
- macOS: the right-edge resize strip no longer draws its 2×32 pt grabber, the
  thin bar half way down the window. The strip still resizes the window.

## Calendar

Calendar connects only to an account that signed in with Google
(`connect_calendar`). The sidebar row, G C and the palette's Calendar entries
appear only when the build offers Google sign-in or an account already uses it.
Official builds with `GOOGLE_OAUTH_PUBLIC=false` and only app-password or IMAP
accounts don't show Calendar; development builds keep it.

## Verified (13 Nov 2026)

Builds: an engine with the official profile and the distribution channel (a
placeholder Google client ID, file credentials, the `test-fixture` feature),
matching the published beta's features; disposable stores only.

- macOS: the real windows, captured by window ID, in dark and light mode at
  1280×780 and 820×640, with an empty store and with `EMAIL_DEMO=1`. The
  welcome, button contrast, hidden list, strip and sidebar rows, and the
  missing grabber were checked by eye. With the demo account the full sidebar,
  Calendar (a Google sign-in demo account) and the shortcut strip are back.
- macOS: `apps/macos/Tests/run-account-setup-checks.py` (126 checks) against
  the fixture: the welcome hides mail-only rows, and after connecting an
  app-password account the sidebar returns without Calendar.
- Linux: the GTK shell under Xvfb in the verify container, the same states and
  sizes (`linux-*` screenshots); the running executable's SHA-256 matched the
  built artifact.
- Linux: `apps/linux/qa/account-setup-fixture.sh` starts on the welcome and,
  after connecting app-password accounts, shows the full sidebar without
  Calendar.
- Not verified: a Wayland or GNOME session and keyboard focus on the Linux
  welcome page.
