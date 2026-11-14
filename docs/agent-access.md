# Agent access

Agents connected through Email's local MCP server (`mail-mcp`) can use an
account only after the user turns on **Agent access** for it, and every send or
bulk change they ask for waits in **Agent requests** until the user approves
it. This page describes the shared desktop behavior. The threat model is in
[mcp-security.md](mcp-security.md); tool details are in [mcp.md](mcp.md).

macOS is the design reference; the GTK shell has the same layout and behavior.

## Agent requests (main window)

A sidebar item **Agent requests**, under Workspace after Outbox. It shows
while agent access is on for any account or something waits, with a count
pill while requests wait.

**List column.** One row per request, oldest first: the agent's name (Claude
Code, Codex, otherwise the name the MCP client reported), the time (as the
macOS inbox shows times: today the time, older the date), a title ("Send:
‹subject›", "Archive 2 conversations", "Wants to use ‹account›") and one
preview line ("To Maya Chen · Hi Maya, could we…", the senders, or "Agent
access is off for this account"). The header says "3 waiting", or nothing
when the list is empty. The search field and filter button are hidden in this
view and come back when you leave it; / does nothing here, and Search mail or
Refine search from the command palette go back to the inbox first.

**Reader.**

- A strip at the top: "‹Agent› wants to send this email" (or "…schedule this
  email", "…archive 2 conversations", "…use ‹account›") with the buttons.
- Send: **Don't send**, **Edit**, **Send ⌘↩** (Ctrl ↵ on Linux). Below the
  strip, the email exactly as it will go out, laid out like the reader:
  subject, From, To/Cc/Bcc/Reply-To ("Name <address>" when the agent gave a
  name, so the address is always visible), When for a scheduled send, the
  whole plain-text body, and attachments as chips; clicking a chip saves a
  copy (quarantined on macOS, never executable on Linux). A note counts
  allowed invisible characters when there are any.
- Bulk change: the conversations as inbox-style rows (sender, date, subject,
  snippet) and buttons with the action's verb: **Don't archive** / **Archive
  2**, **Don't move** / **Move 2**, **Move 2 to Spam**, **Don't mark read** /
  **Mark 2 read**, **Mark 2 unread**; undoing a change: **Don't undo** /
  **Undo**; sender cleanup: **Don't mark read** / **Mark 214 read**.
  "You can undo this afterwards." under archive, move and spam; "This can't be
  undone." under sender cleanup.
- Refused agent: **Ignore** / **Allow…**. Allow… opens the turn-on sheet with
  the level the agent needed preselected (Custom… expanded when it needed
  calendar changes).
- Keys at the bottom: ⌘↩ (Ctrl ↵) approves, E edits a send, ⌫ declines or
  ignores, J/K move to the next request. Other mail shortcuts do nothing in
  this view.

**Approving.** Approval keeps today's semantics: a send is queued with the
10-second undo window (an agent may ask for longer, never shorter), shown in
the reader as "Sending in 10 seconds · Undo" and in the main window's usual
undo control; a scheduled send shows "Scheduled for … · Cancel"; an undoable
bulk change shows "Done · Undo" for 30 seconds and becomes the triage undo.
The next request is selected after a decision. The approve button and its
shortcut wait 0.8 seconds after a request appears (the button is dimmed
meanwhile), so a double press can't approve the next request.

**Edit** closes the request as "Opened to edit" without sending, switches to
Drafts and opens the draft in the composer. Only the user's own Send sends it.
The agent's `get_agent_action` reports the request as edited.

**Refused agents.** A call refused because an account is off, or lacks a
permission the call needed, becomes one request per account while it still
applies: until the account's access changes after the attempt, or the user
chooses Ignore. Ignore keeps that account's refusals quiet while the agent
keeps retrying (within ten minutes of its last attempt). This replaces the
old "Agent access needed" banner.

**Notifications.** A new request posts a desktop notification through the
existing notification setting: the agent's name as the title and "wants to
send “Design review time” to Maya Chen" (or the request's own line) as the
body; with previews off, "Email" / "An agent is waiting for you". Clicking
it opens the main window on that request (`app.open-agent-request` on
Linux).

## Agent access (settings)

The settings window (sidebar **Agent access**, the command palette, and
Manage accounts):

- "Agent access" and "Let AI tools like Claude Code and Codex use your mail.
  Nothing is sent without your OK."
- When requests wait, one line at the top: "2 requests waiting · Review",
  which opens the main window on Agent requests.
- **What agents can do:** one row per account with a popup: Off, Read only,
  Read and organize, Read, organize and write, and Custom… (shown as the value
  when the grants match no level). Turning an account on, or allowing more,
  opens the turn-on sheet with that choice; allowing less applies at once;
  Off asks first ("Turn off agent access for …?") and drops waiting requests.
- **Agents:** Claude Code and Codex with **Connect**, or "✓ Connected" ("Start
  a new session to use it" right after connecting), "Not installed" when the
  CLI isn't found; "Another app · Any app that supports MCP" with **Set up…**,
  which copies the `mail-mcp` command and an `mcpServers` JSON snippet. When
  no helper is found, one line under the group says why and the buttons are
  off.
- Footer: **Activity** (decisions, changes agents made, consent changes and
  refusals) and **Done**.

Connect finds the CLIs on the app's PATH and in the usual install locations
(`~/.local/bin`, `~/.claude/local`, `~/bin`, npm, bun, volta, pnpm and nvm
directories, `/opt/homebrew/bin`, `/usr/local/bin`, `/usr/bin`), runs the
same `mcp add` command as `scripts/connect-mcp.sh`, and never replaces an
existing registration. See [mcp.md](mcp.md#connect-codex-and-claude-code).
The helper is `Contents/Helpers/mail-mcp` in the macOS app and
`/usr/lib/email-linux/mail-mcp` in the Linux packages.

## The turn-on sheet

"Let agents use ‹account›?" with three choices, Read only preselected:

| Choice | Grants |
| --- | --- |
| Read only — Read mail and calendar. | read, Fetch mail |
| Read and organize — Also archive, mark read and snooze. You OK bulk changes. | + Archive and snooze |
| Read, organize and write — Also write drafts and ask to send. You OK every email. | + Drafts, Ask to send |

One note: "What an agent reads goes to its AI provider, like Anthropic or
OpenAI. You can turn this off anytime." Two links: **Custom…** shows the
individual switches (Fetch new mail; Archive, mark read and snooze; Write
drafts; Ask to send mail; Change calendar events · guests get notified, in
the warning color). Calendar changes are only reachable there. **What can
agents see?** shows the full list: messages and attachments (downloaded on
demand), drafts and labels, snoozed/scheduled/queued mail, calendar
invitations and events, recipient suggestions, follow-ups and signatures,
open-tracking, snippets, splits and bookings, the untrusted-content note,
that Email's developer never receives the data, and that a registration's own
permissions still apply. Buttons: **Cancel** and **Allow** (**Save** when the
account is already on). Neither is the default; Escape cancels.

The text comes from `agent_access_disclosure` in mail-service, so both shells
use the same words, and it is disclosure version 2 (it has not shipped;
version 1 consents keep working, see
[mcp-security.md](mcp-security.md#t1-an-agent-reads-mail-the-user-never-meant-to-share)).

## Verified (14 Nov 2026)

Builds: the neutral `unofficial` profile, debug; disposable launch-demo
stores only (two accounts). Agents were the real `mail-mcp` driven by
`crates/mail-mcp/tests/agent_flow.py`, reporting the client names
`claude-code` and `codex-mcp-client`. Connect used fake `claude`/`codex`
scripts; `EMAIL_AGENT_CLI_PATH` limited the search to their directory.

- macOS: `apps/macos/Tests/run-agent-access-checks.py` passed 86 checks in
  dark and in light mode, in the real main window and settings window:
  refused agents as requests with friendly names, the row time in the inbox
  format, no search or filters in the list (and back after leaving), Allow…
  with Read only preselected, Custom… and the full list, the level recorded,
  send/bulk/access rows and reader, the email in full with its attachment,
  the 0.8-second wait (an early click and an early ⌘↩ send nothing), ⌘↩ with
  the undo bar and Undo, E into the composer with the request closed as
  edited and nothing queued, ⌫ declining a bulk change (conversations
  untouched) and ignoring a refused agent, the settings line and Review,
  allowing less without the sheet, Off with its confirmation, Connect's exact
  command, Set up…, Activity and the missing-helper note. Window captures
  were checked by eye.
- Linux: `apps/linux/qa/agent-access.sh` in the verify container under Xvfb,
  openbox and dunst, the running executable matching the built one: the same
  flow by keyboard and clicks (Ctrl+Enter, E, Backspace, J/K), notifications
  for new requests, the database checked after each decision, and captures
  in dark and light checked by eye (including "3 waiting" with no search row,
  an empty header with nothing waiting, and the search row back in Drafts).
- Not verified: a macOS notification (the harness isn't an app bundle, so it
  can't post one), clicking a notification (the Linux action was triggered
  directly), Wayland or GNOME, the AUR build, and a signed release bundle.
