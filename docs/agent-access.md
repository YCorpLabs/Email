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

1. **Heading and one-line explanation.**
2. **"Agent access needed" banner**, shown while a recent refusal still
   applies: an account is off, or a grant was missing after the latest consent
   change. It offers **Turn On for …** (or **Change Access for …**) when one
   account is affected. It disappears once the user acts.
3. **Waiting for your approval.** One card per request:
   - Send, queue or schedule: From, To, Cc, Bcc and Reply-To as bare
     addresses (a display name is marked "shown as"), Subject, each
     attachment's name, size and type with **Save…** to inspect a copy, When (after approval with the 10-second undo, a longer agent-chosen
     window, or the scheduled time), and the body. A long body shows its first
     14 lines with a note; **Show full message** shows the whole plain-text
     body. Drafts with an HTML body, or with invisible, control or
     text-direction characters outside the contexts real text needs (emoji,
     keycaps, flags, Indic and Arabic joiners), are refused before they reach
     the window. The card states how many allowed invisible characters the
     message contains.
   - Bulk change, or undoing one: the action and count, the accounts, up to
     eight conversations (sender and subject), and whether it can be undone.
   - Sender cleanup: sender, unread count, sample subjects and the no-undo note.
   - The account, the client name the agent reported ("requested by …") and the
     time. Buttons: **Reject** and **Approve** (**Approve and Send**,
     **Approve and Schedule**). Neither is the default button, so Return alone
     never approves; a button needs keyboard focus first (Tab), and each
     platform then activates it in its usual way (Space on macOS, Space or
     Return on Linux). Requests are listed oldest first, so a new one never takes an
     existing card's place, and a decision click within 0.8 seconds of the list
     changing is ignored with an explanation.
4. **Accounts.** Each account shows Off, or "On since … · Read, Drafts, …",
   with **Turn On…**, or **Change…** and **Turn Off**.
5. **History.** Decisions, changes agents made without approval, consent
   changes and refused attempts, newest first, with time, status and client.
6. **Footer.** A status line, a temporary **Undo Send** (or **Undo**,
   **Cancel Send**) button after an approval, and **Done** (Escape). When the
   list re-renders while focus is inside it, focus moves to **Done** on both
   shells. Done and Escape hide the window; reopening it shows the same
   window, with expanded messages still expanded.

The disclosure dialog shows the title, the data categories, a note on data not
tied to one account, the provider-terms sentence, the safeguards, a checked and
disabled **Read** box, one unchecked box per optional grant with its
explanation, and how to turn access off. Text comes from
`agent_access_disclosure` in mail-service, so both shells use the same words.
**Turn Off** asks for confirmation first. Both dialogs start on Cancel and
Escape cancels: macOS lists Cancel first, so NSAlert gives it Escape and no
button Return; Linux focuses Cancel, so Return cancels until the user moves
focus.

After an approval the main window also shows the usual undo control: the
reader's **Undo send** button and status for sends, and the triage bar's
**Undo** for bulk changes.

The window polls `agent_pending_summary` every five seconds and re-reads its
contents (`agent_access_overview`) only when the summary's revision changes.
Notifications follow the existing notification setting and the previews
setting: with previews off, an alert says only that an agent is waiting.

## Platform differences

Window chrome is native: a titled AppKit window on macOS, a decorated GTK
window on Linux. Primary buttons use each shell's existing primary style
(accent on macOS, ink on Linux, as in the composer). Confirmations use
`NSAlert` on macOS and `gtk::AlertDialog` or a modal window on Linux.
Notification-seen state is stored in user defaults on macOS and in
`mail.agent-notifications` next to the database on Linux.
