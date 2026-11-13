# Agent access

Agents connected through Email's local MCP server (`mail-mcp`) can use an
account only after the user turns on **Agent access** for it, and every send or
bulk change they ask for waits for the user's approval. This page describes the
shared desktop behavior. The threat model is in [mcp-security.md](mcp-security.md); tool details are in
[mcp.md](mcp.md).

## Where it is

Both shells have the same entry points and the same window layout. macOS is the
design reference.

| Entry point | macOS | Linux |
| --- | --- | --- |
| Sidebar | **Agent access** below Manage accounts, with a badge: the number of waiting requests, or `!` after an agent was refused | Same, below Manage accounts |
| Command palette | **Agent access** (Settings) | **Agent access…** |
| Manage accounts | **Agent Access…** button in the accounts alert | **Agent access…** button in the Accounts dialog |
| Notification | Clicking an agent notification opens the window | Same, through `app.open-agent-access` |

## The window

From top to bottom:

1. **"Agent access"** and one line: "Let AI tools like Claude Code and Codex
   work with your mail. You approve every send."
2. **"Agent access needed" banner**, shown while a recent refusal still
   applies: an account is off, or a grant was missing after the latest consent
   change. It says which account was blocked (and, for a missing grant, which
   grant) and offers **Turn on for …** (or **Change access for …**) when one
   account is affected. It disappears once the user acts.
3. **Waiting for you**, with a count, shown only while requests wait. One card
   per request, oldest first:
   - Send, queue or schedule: a small kind label (SEND EMAIL, SCHEDULE EMAIL),
     the subject, then To, Cc, Bcc, Reply-To and From as bare addresses (a
     display name is marked "shown as"). A scheduled send also shows When. The
     card notes how many allowed invisible characters (emoji, keycaps, flags,
     Indic and Arabic joiners) the message contains. **Show message** (with
     "· N attachments" when there are any) expands the whole plain-text body,
     each attachment's name, size and type with **Save…** to inspect a copy,
     and When (after approval with the 10-second undo, or a longer
     agent-chosen window). Nothing of the body is cut when expanded. Drafts
     with an HTML body, or with invisible, control or text-direction
     characters outside the contexts real text needs, are refused before they
     reach the window.
   - Bulk change, or undoing one: the action and count, the accounts, up to
     eight conversations (sender and subject), and whether it can be undone.
   - Sender cleanup: sender, unread count, sample subjects and the no-undo note.
   - "Requested by …" (the client name the agent reported) and the time.
     Buttons: **Reject** and **Approve** (**Approve and send**, **Approve and
     schedule**). Neither is the default button, so Return alone never
     approves; a button needs keyboard focus first (Tab), and each platform
     then activates it in its usual way (Space on macOS, Space or Return on
     Linux). A new request never takes an existing card's place, and a
     decision click within 0.8 seconds of the list changing is ignored with an
     explanation.
4. **Connect an agent**: rows for Claude Code, Codex and "Other MCP client".
   - Each CLI row says "Found on this Mac" (Linux: "this computer") or "Not
     installed". The app looks on its PATH and in the usual install
     locations (`~/.local/bin`, `~/.claude/local`, `~/bin`, npm, bun, volta,
     pnpm and nvm directories, `/opt/homebrew/bin`, `/usr/local/bin`,
     `/usr/bin`), since apps started from the Dock or a launcher don't get the
     shell's PATH.
   - "✓ Connected" when `claude mcp get <name>` or `codex mcp get <name>`
     finds the registration; the subtitle says so when it points at another
     copy of `mail-mcp`. Otherwise **Connect** registers the helper with the
     same commands `scripts/connect-mcp.sh` runs (see [mcp.md](mcp.md)) and
     never replaces an existing registration.
   - **Copy** on "Other MCP client" copies the `mail-mcp` command and an
     `mcpServers` JSON snippet with the same arguments.
   - Below the list: "Connected agents can use only the accounts you turn on
     below." Without a helper, the note says why instead and Connect and Copy
     are off: on macOS a build without `Contents/Helpers/mail-mcp` (release
     builds include it; development builds use `EMAIL_MCP_HELPER`, which
     `scripts/dev.sh` sets), on Linux no `mail-mcp` next to `email-linux`, on
     PATH or in the usual locations.
   - The CLIs run only when the window opens and after Connect, on a separate
     engine handle, never on the five-second poll.
5. **Accounts**: one row per account with an on/off switch. On: the grants,
   such as "Read · Drafts · Ask to send", and an **Edit** link that opens the
   turn-on sheet with the current grants. Off: "Off". Switching on opens the
   turn-on sheet; cancelling it puts the switch back. Switching off asks first
   ("Turn off agent access for …?": agents lose access on their next request,
   waiting requests are rejected, mail already approved stays in the Outbox);
   cancelling puts the switch back.
6. **Recent activity**, collapsed: decisions, changes agents made without
   approval, consent changes and refused attempts, newest first, with time,
   status and client.
7. **Footer.** A status line, a temporary **Undo send** (or **Undo**,
   **Cancel send**) button after an approval, and **Done** (Escape). When the
   list re-renders while focus is inside it, focus moves to **Done** on both
   shells. Done and Escape hide the window; reopening it shows the same
   window, with expanded messages still expanded.

### The turn-on sheet

A sheet on the Agent access window (a modal window on Linux), shown when an
account is switched on, from the banner, and from Edit:

- "Let agents use {email}?" and "Agents can read this account's mail, drafts
  and calendar on this Mac."
- Checkboxes: **Read mail and calendar · required** (checked and locked),
  **Write drafts**, **Archive, mark read and snooze · you approve bulk
  changes**, **Ask to send mail · you approve each one**, **Fetch new mail**,
  **Change calendar events · no approval; guests get notified** (the note in
  the warning color). Every optional box starts unchecked: reading is the
  only default. Each box's tooltip and accessibility description give the
  full explanation.
- A callout: "What an agent reads goes to its AI provider, such as Anthropic
  or OpenAI, under their terms."
- "You can turn this off anytime in Agent access."
- **What exactly can agents see?** expands the full list: messages and
  attachments (downloaded on demand), drafts and labels, snoozed, scheduled
  and queued mail, calendar invitations and events, recipient suggestions,
  follow-ups, signatures, open-tracking, snippets, splits and bookings, the
  untrusted-content note, that Email's developer never receives the data, and
  that an MCP registration's own permissions still apply.
- **Cancel** and **Turn on** (**Save** when editing). Neither is the default;
  Escape cancels. On macOS Cancel has Escape and the sheet starts with focus
  on Cancel; on Linux Cancel has focus.

All text comes from `agent_access_disclosure` in mail-service, so both shells
use the same words. The account summary uses the short grant names from
`agent_grant_label` (Drafts, Archive and snooze, Ask to send, Fetch mail,
Calendar changes), which MCP refusals use too.

The sheet's text is disclosure version 2. Consents recorded under version 1
(the previous dialog) keep working: `AGENT_DISCLOSURE_MIN_VERSION` stays 1
because the new text names the same data, grants and safeguards. See
[mcp-security.md](mcp-security.md#t1-an-agent-reads-mail-the-user-never-meant-to-share).

After an approval the main window also shows the usual undo control: the
reader's **Undo send** button and status for sends, and the triage bar's
**Undo** for bulk changes.

The window polls `agent_pending_summary` every five seconds and re-reads its
contents (`agent_access_overview`) only when the summary's revision changes.
Notifications follow the existing notification setting and the previews
setting: with previews off, an alert says only that an agent is waiting.

## Platform differences

Window chrome is native: a titled AppKit window on macOS, a decorated GTK
window on Linux. The turn-on sheet is an AppKit sheet on macOS and a modal
window on Linux. Primary buttons use each shell's existing primary style
(accent on macOS, ink on Linux, as in the composer); other buttons in the
window are bordered on macOS and on a soft fill on Linux, where buttons are
otherwise flat. Switches are `NSSwitch` and `gtk::Switch`. The turn-off
confirmation uses `NSAlert` on macOS and `gtk::AlertDialog` on Linux.
Notification-seen state is stored in user defaults on macOS and in
`mail.agent-notifications` next to the database on Linux.

## Verified (13 Nov 2026)

Builds: the neutral `unofficial` profile, debug; disposable demo stores only.
The window checks used fake `claude`/`codex` scripts: `EMAIL_AGENT_CLI_PATH`
limits the search to their directory, so no real CLI or configuration was
found, run or changed.

- Real CLIs, macOS: `agent_clients` and `connect_agent_client` through the
  engine library with the installed Claude Code and Codex, a clean
  environment and a throwaway `HOME`. Both showed "Found on this Mac", Connect
  registered `email-unofficial` with the expected command in each (checked
  with `claude mcp get` and `codex mcp get` against that `HOME`), both then
  showed connected to this helper, and a second Connect changed nothing. The
  real configuration files have no trace of it.

- macOS: `apps/macos/Tests/run-agent-access-checks.py` (the real window,
  real mail-service and real `mail-mcp`) passed in dark and in light mode:
  the blocked prompt, the sheet's text, locked Read and unchecked defaults,
  Escape/Return, the full list, Cancel putting the switch back, turning on
  with three grants, Edit, pending send and batch cards, the body behind
  Show message, the click guard, the narrow window, approve with Undo send,
  reject, Recent activity, a long body, an attachment and its bytes,
  Connect's exact registration command, Copy, the turn-off confirmation and
  its cancel, turning off, and the missing-helper note. Real window captures
  (`screencapture -l`) of each state were checked by eye.
- Linux: `apps/linux/qa/agent-access.sh` in the verify container under Xvfb
  and openbox, with the running executable's SHA-256 matching the built one:
  the same states in dark (blocked, sheet collapsed and expanded, cancel,
  three grants, account on, pending, expanded, narrow, approve and Undo send,
  reject, Recent activity, Connect, the turn-off confirmation, an attachment,
  no helper) and the main ones in light. The captures were checked by eye.
- Not verified: Connect from the window against real CLIs (the window used
  the fakes; the real CLIs were exercised through the same engine operations),
  real CLIs on Linux, a signed release bundle's `Contents/Helpers/mail-mcp`
  (the checks used `EMAIL_MCP_HELPER`), Wayland or GNOME, and keyboard-only use
  of the Linux sheet.
