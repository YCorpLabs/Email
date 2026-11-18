# Scheduling

Email can connect to a Scheduling account (the hosted booking service named by
the build's [distribution profile](distribution-profile.md)) to add booking
links to emails and show bookings in Calendar. It stays out of sight until you
connect it. macOS is the design reference; the GTK shell has the same states
and behavior.

## Before connecting

Nothing about Scheduling shows in the composer, Calendar or the command
palette. There are two ways in:

- **Manage accounts** lists the service under the mail accounts:
  "Scheduling by YCorp" (or just "Scheduling" for a fork's service, see
  `profile::scheduling_name`), "Not connected", and **Connect**.
- The command palette has **Connect Scheduling…** in Settings, also in the
  composer's palette. It opens Manage accounts.

A build whose profile has no Scheduling origin shows neither.

## Connecting

Connect opens the service's page in the browser and the entry changes to
"Waiting for your browser" with **Cancel** and "Code if asked: ‹code›".
Approval arrives while Manage accounts is open (the entry updates in place) or
after it was closed; then the main window's status line says "Scheduling
connected". A failure (an expired code, a refused grant, the network) shows
in red under the entry until the next action.

The connection asks only for booking access: Email requests `booking:read` and
refuses a grant with any other scope. See [reader privacy](reader-privacy.md).

## Connected

The entry shows the Scheduling account's address, "Connected on this Mac"
("Connected on this computer" on Linux), **Disconnect** and **Manage online**.
Then:

- the composer shows **Booking link** (insert a link or Suggest times);
- Calendar's rail shows **Bookings and links**;
- the palette lists **Bookings and meeting links** and, in the composer,
  **Insert booking link…**. Typing "book" in the composer puts Insert booking
  link… first, so Return inserts a link; in the main window Bookings and
  meeting links comes first. The palette's settings command is now
  **Scheduling**, which opens Manage accounts.

The **Bookings and meeting links** window has meeting links, upcoming bookings
and 30-day analytics. It has no connect screen or Disconnect of its own; those
live in Manage accounts.

**Disconnect** revokes this device's grant without asking first and hides the
booking surfaces again, also in an open composer or Calendar. Connecting again
is one browser approval.

## Where it lives in each shell

| | macOS | Linux |
| --- | --- | --- |
| Manage accounts | An alert. The entry is its accessory view between the account list and the buttons, in the alert's 13 pt text and text column; actions are text links. | The Accounts window. The entry is one more row like the mail accounts: bold name, muted detail lines, a trailing button (Connect, Cancel or Disconnect) and a Manage online link. |
| Connection state | `SchedulingConnection` (`SchedulingConnection.swift`) | `booking::Connection` (`booking.rs`) |
| Booking link | `ComposePane` | composer in `main.rs` |
| Bookings and links | `AgendaPanel` rail | `agenda.rs` rail |
| Palette | `EmailUI.swift` | `parity.rs` |

The two Manage accounts views already differed (Remove an account… on macOS,
Remove per row on Linux); the Scheduling entry follows each one's pattern.
Keyboard: on macOS the entry's links are key views with Full Keyboard Access
(Space runs them) and Return still means Close; the Linux row uses ordinary
buttons. The Linux Accounts window has no Esc binding (unchanged).

## Hosted service

Email sends `scope` with the device-grant request (`desktop_scopes` in
`contracts/scheduling.json`). The hosted service must accept a requested scope
and issue exactly that one. Until that Scheduling release is deployed it
rejects client-chosen scopes, so connecting fails with its error. Grants made
before keep working: the stored credential holds no scope, and their earlier
scope (`booking:read email:tracking`) is still recognized.

## Verified

macOS, 17 Nov 2026, `apps/macos/Tests/run-scheduling-checks.py` with a
development-profile `test-fixture` library (launch demo store, temporary
credentials directory, windows drawn off-screen):

- not connected: no Scheduling sidebar row; no booking commands; Manage
  accounts shows the entry with its three buttons unchanged; "sched" starts
  with Connect Scheduling…; the composer has no Booking link and its "book"
  offers Connect Scheduling… only; Calendar has no Bookings and links;
- waiting: the code and Cancel; the open alert grows to fit; Cancel is a
  focusable link with an accessibility label, and pressing it stops waiting;
- a failure line under the entry;
- connected: an already open composer gains Booking link; "book" in the
  composer gives Insert booking link…, Bookings and meeting links; in the main
  window Bookings and meeting links, Scheduling; the entry shows the account,
  Disconnect and Manage online; Calendar shows Bookings and links;
- Disconnect hides Booking link in an open composer, Bookings and links in
  Calendar, and the palette returns to Connect Scheduling….

Light and dark captures were checked by eye. The reader-privacy harness's
development variant confirms Booking link and the booking commands wait for a
connection.

Linux, 17 Nov 2026, in the `email-linux-dev` container under Xvfb with a
development-profile `qa-fixture` build (the running executable's hash matched
the build), `apps/linux/qa/scheduling.sh`: the Accounts row in all three
states, "sched" in the main palette and "book" in the main and composer
palettes, Booking link in
the composer and Bookings and links in Calendar, not connected and connected.
The `parity.rs` tests cover command visibility, the "book" and "sched"
ranking and the row text.

Not verified: connecting against the hosted service (its requested-scope
release isn't deployed), the live alert re-layout while it runs modally on
screen (only off-screen), VoiceOver and Orca by ear, and the official profile
(it has no Scheduling origin until launch).
