#!/usr/bin/env python3
"""Exercise the actual AppKit Agent access window end to end.

A disposable demo store is shared by the real window, the real mail-service
library and the real mail-mcp binary acting as an agent. The check covers the
blocked-attempt prompt after an upgrade, the turn-on sheet and its defaults,
consent through the account switch, pending send and bulk-triage requests,
approve with undo, reject, Recent activity, the narrow layout, Connect with
fake Claude Code and Codex CLIs, and turning access off. Demo accounts have no
provider configuration, so nothing can be sent.

The fake CLIs live in a temporary directory, and EMAIL_AGENT_CLI_PATH limits
the search to it, so the real claude and codex (and their settings) are never
found or run.

With --snapshots DIR, real window captures (screencapture -l) of the key
states go to DIR, named mac-<appearance>-*.png; --appearance picks dark or
light. Capturing needs Screen Recording permission for the terminal.
"""
from pathlib import Path
import argparse, os, subprocess, tempfile

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--library', type=Path, required=True, help='Built macOS libmail_service.dylib')
parser.add_argument('--mcp', type=Path, required=True, help='Built mail-mcp binary')
parser.add_argument('--snapshots', type=Path, help='Optional directory for PNG captures of key states')
parser.add_argument('--appearance', choices=['dark', 'light'], default='dark')
args = parser.parse_args()
library = args.library.resolve(strict=True)
mcp = args.mcp.resolve(strict=True)
root = Path(__file__).resolve().parents[3]
source = root / 'apps/macos/Sources/EmailUI'
check = r'''
    func checkAgentAccess() {
        let env = ProcessInfo.processInfo.environment
        func expect(_ ok: Bool, _ name: String) { precondition(ok, name); print("PASS \(name)") }
        func wait(_ name: String, _ condition: () -> Bool) {
            let deadline = Date().addingTimeInterval(10)
            while !condition() && Date() < deadline { RunLoop.main.run(until: Date().addingTimeInterval(0.02)) }
            expect(condition(), name)
        }
        func settle(_ seconds: Double = 0.4) { RunLoop.main.run(until: Date().addingTimeInterval(seconds)) }
        /// A real capture of one window by its window number.
        func capture(_ name: String, _ window: NSWindow?) {
            guard let dir = env["EMAIL_CHECK_SNAPSHOTS"], let window else { return }
            settle(0.6)
            let process = Process()
            process.executableURL = URL(fileURLWithPath: "/usr/sbin/screencapture")
            process.arguments = ["-x", "-o", "-l", String(window.windowNumber), "\(dir)/mac-\(env["EMAIL_CHECK_APPEARANCE"] ?? "dark")-\(name).png"]
            try? process.run()
            process.waitUntilExit()
        }
        func agent(_ step: String) -> [String: Any] {
            let process = Process()
            process.executableURL = URL(fileURLWithPath: "/usr/bin/python3")
            process.arguments = [env["AGENT_FLOW"]!, env["MAIL_MCP"]!, env["EMAIL_STORE_PATH"]!, step]
            let pipe = Pipe()
            process.standardOutput = pipe
            try! process.run()
            let data = pipe.fileHandleForReading.readDataToEndOfFile()
            process.waitUntilExit()
            return (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] ?? [:]
        }
        func poll(_ name: String, _ condition: () -> Bool) {
            let deadline = Date().addingTimeInterval(10)
            while !condition() && Date() < deadline {
                Task { @MainActor in await self.refreshAgentRequests() }
                RunLoop.main.run(until: Date().addingTimeInterval(0.2))
            }
            expect(condition(), name)
        }
        var window: NSWindow { agentAccess!.window! }
        var panel: NSView { window.contentView! }
        func all(_ view: NSView) -> [NSView] { [view] + view.subviews.flatMap { all($0) } }
        func button(_ title: String, in root: NSView? = nil) -> NSButton? {
            all(root ?? panel).compactMap { $0 as? NSButton }.first { $0.title == title && !$0.isHidden && !($0 is NSSwitch) }
        }
        func texts(_ root: NSView) -> [String] { all(root).filter { !$0.isHiddenOrHasHiddenAncestor }.compactMap { ($0 as? NSTextField)?.stringValue } }
        func shows(_ fragment: String) -> Bool { texts(panel).contains { $0.contains(fragment) } }
        func toggle(_ email: String) -> NSSwitch? { all(panel).compactMap { $0 as? NSSwitch }.first { $0.accessibilityLabel() == "Agent access for \(email)" } }
        func decide(_ title: String) {
            // Decisions within 0.8 s of a re-render are ignored by design.
            settle(0.9)
            button(title)!.performClick(nil)
        }
        func pendingCount() -> Int { ((core!.call(["op": "agent_pending_summary"]) as? [String: Any])?["count"] as? NSNumber)?.intValue ?? -1 }
        func visible(_ button: NSButton?) -> Bool {
            guard let button, let window = button.window else { return false }
            let frame = button.convert(button.bounds, to: nil)
            return frame.width > 20 && frame.minX >= 0 && frame.maxX <= window.contentLayoutRect.width + 1
        }
        var sheet: AgentAccessSheet? { AgentAccessSheet.presented }
        func boxes() -> [(String, NSButton)] { sheet?.boxes ?? [] }

        if env["EMAIL_CHECK_APPEARANCE"] == "light" { NSApp.appearance = NSAppearance(named: .aqua) }
        else { NSApp.appearance = NSAppearance(named: .darkAqua) }
        makeKeyAndOrderFront(nil)
        // An upgraded registration is refused until the user turns access on.
        let probe = agent("probe")
        expect("\(probe["list_accounts"] ?? "")".contains("Agent access"), "MCP refuses an account without consent and names Agent access")
        poll("sidebar flags the blocked agent") { agentBadge.stringValue == "!" && !agentBadge.isHidden }
        expect(paletteItems().contains { $0.target == .agentAccess }, "palette offers Agent access")
        openAgentAccess()
        wait("panel lists the account as off") { shows("alex@example.com") && shows("Off") && toggle("alex@example.com")?.state == .off }
        expect(shows("Let AI tools like Claude Code and Codex work with your mail. You approve every send."), "one-line lead")
        expect(!shows("Waiting for you"), "no waiting section while nothing is pending")
        expect(shows("Agent access needed") && button("Turn on for alex@example.com…") != nil, "blocked banner offers the consent step")
        // Connect: the fake claude is found, codex is not installed.
        wait("Connect rows show what is installed") { shows("Found on this Mac") && shows("Not installed") && button("Connect") != nil }
        expect(button("Copy") != nil && button("Copy")!.isEnabled, "other clients can copy a configuration")
        expect(!shows("Read · "), "an account that is off lists no grants")
        capture("01-blocked-nothing-pending", window)

        // The sheet from the switch: every optional grant starts unchecked.
        toggle("alex@example.com")!.performClick(nil)
        wait("switching on opens the turn-on sheet") { window.attachedSheet != nil && sheet != nil }
        let sheetView = sheet!.panel.contentView!
        let sheetText = texts(sheetView).joined(separator: "\n")
        expect(sheetText.contains("Let agents use alex@example.com?"), "sheet title names the account")
        expect(sheetText.contains("Agents can read this account's mail, drafts and calendar on this Mac."), "sheet lead says what agents read")
        expect(sheetText.contains("What an agent reads goes to its AI provider, such as Anthropic or OpenAI, under their terms."), "sheet states the provider's terms")
        expect(sheetText.contains("You can turn this off anytime in Agent access."), "sheet says how to turn access off")
        let titles = all(sheetView).compactMap { $0 as? NSButton }.map(\.title)
        expect(titles.contains { $0.hasPrefix("Read mail and calendar") } && titles.contains { $0.hasPrefix("Change calendar events") }, "sheet lists every grant")
        expect(boxes().map(\.0) == ["draft", "triage", "send", "sync", "calendar_write"] && boxes().allSatisfy { $0.1.state == .off }, "optional grants start unchecked")
        let readBox = all(sheetView).compactMap { $0 as? NSButton }.first { $0.title.hasPrefix("Read mail and calendar") }!
        expect(readBox.state == .on && !readBox.isEnabled, "reading is required and locked")
        expect(sheet!.cancelButton.keyEquivalent == "\u{1b}" && sheet!.confirmButton.keyEquivalent.isEmpty && sheet!.confirmButton.title == "Turn on", "Return never confirms; Escape cancels")
        expect(!sheetText.contains("Open-tracking activity"), "the full list starts collapsed")
        capture("02-sheet-collapsed", sheet!.panel)
        button("What exactly can agents see?", in: sheetView)!.performClick(nil)
        settle(0.5)
        let expandedText = texts(sheetView).joined(separator: "\n")
        for fragment in ["senders, recipients, subjects, bodies and attachments", "downloaded from your mail provider", "Drafts and labels", "Snoozed, scheduled and queued mail", "Calendar invitations", "Recipient suggestions, follow-up reminders and signatures", "Open-tracking activity", "snippets and saved splits", "bookings", "untrusted", "MCP registration"] {
            expect(expandedText.contains(fragment), "full list names: \(fragment)")
        }
        capture("03-sheet-expanded", sheet!.panel)
        button("Cancel", in: sheetView)!.performClick(nil)
        wait("Cancel closes the sheet and the switch goes back off") { window.attachedSheet == nil && toggle("alex@example.com")?.state == .off }

        // Turn on from the banner with three grants.
        button("Turn on for alex@example.com…")!.performClick(nil)
        wait("banner opens the same sheet") { sheet != nil && window.attachedSheet != nil }
        for (id, box) in boxes() where ["draft", "triage", "send"].contains(id) { box.state = .on }
        sheet!.confirmButton.performClick(nil)
        wait("consent is recorded with the chosen grants") { shows("Read · Drafts · Archive and snooze · Ask to send") && button("Edit") != nil && toggle("alex@example.com")?.state == .on }
        expect(!shows("Agent access needed"), "the upgrade prompt disappears once access is on")
        poll("the sidebar flag clears once access is on") { agentBadge.isHidden }
        capture("04-account-on", window)
        let read = agent("read")
        expect((read["get_message"] as? [String: Any])?["id"] as? String == "demo:1", "agent can read after consent")

        // Edit opens the sheet with the current grants checked.
        button("Edit")!.performClick(nil)
        wait("Edit opens the sheet with current grants") { sheet != nil && boxes().filter { $0.1.state == .on }.map(\.0) == ["draft", "triage", "send"] && sheet!.confirmButton.title == "Save" }
        button("Cancel", in: sheet!.panel.contentView!)!.performClick(nil)
        wait("Cancel keeps the grants") { window.attachedSheet == nil && shows("Read · Drafts · Archive and snooze · Ask to send") }

        let request = agent("request")
        expect((request["send"] as? [String: Any])?["status"] as? String == "pending_approval", "agent send becomes a pending approval")
        expect((request["batch"] as? [String: Any])?["status"] as? String == "pending_approval", "agent bulk archive becomes a pending approval")
        let draftID = request["draft"] as! String
        expect(core!.outbox().isEmpty, "nothing reaches the outbox before approval")
        poll("sidebar counts two requests") { agentBadge.stringValue == "2" }
        wait("panel shows both requests") { button("Approve and send") != nil && shows("SEND EMAIL") && shows("CHANGE CONVERSATIONS") && shows("Waiting for you") }
        expect(shows("Design review time") && shows("To  maya@example.org") && shows("From  alex@example.com") && shows("Requested by agent-flow-check"), "send card shows subject, To, From and client")
        expect(!shows("Could we move Friday"), "the body waits behind Show message")
        expect(shows("Archive 2 conversations") && shows("Weekend plans"), "batch card shows the action and conversations")
        button("Approve and send")!.performClick(nil)
        settle(0.3)
        expect(core!.outbox().isEmpty && shows("The list just changed"), "a click right after the list changed is ignored")
        capture("05-pending", window)
        button("Show message")!.performClick(nil)
        wait("Show message reveals the body") { shows("Could we move Friday") && button("Hide message") != nil && shows("After approval, with 10 seconds to undo") }
        capture("06-pending-expanded", window)
        window.setContentSize(NSSize(width: 440, height: 640))
        settle()
        expect(visible(button("Approve and send")) && visible(button("Reject")), "narrow window keeps the decision buttons inside the window")
        capture("07-pending-narrow", window)
        window.setContentSize(NSSize(width: 600, height: 680))

        decide("Approve and send")
        wait("approval queues the draft with an undo window") { core!.outbox().count == 1 }
        expect(core!.outbox()[0].messageID == draftID && core!.outbox()[0].status == "undoable", "approved draft is undoable")
        wait("main window offers Undo send") { !cancelSendButton.isHidden && lastQueuedMessageID == draftID }
        wait("panel offers Undo send") { button("Undo send") != nil && shows("Approved · sending in") }
        button("Undo send")!.performClick(nil)
        wait("Undo send returns the draft") { core!.outbox().isEmpty && shows("Send cancelled") }

        wait("only the batch remains") { pendingCount() == 1 && button("Approve and send") == nil }
        decide("Reject")
        wait("rejecting closes the batch without changes") { pendingCount() == 0 && shows("Rejected · nothing was sent or changed") }
        let threads = core!.call(["op": "list_threads", "limit": 50]) as? [[String: Any]] ?? []
        expect(threads.contains { $0["id"] as? String == "demo:2" }, "rejected archive left the conversation in the inbox")
        wait("the waiting section goes away") { !shows("Waiting for you") }
        expect(!shows("Agent access turned on for alex@example.com"), "Recent activity starts collapsed")
        button("Recent activity")!.performClick(nil)
        wait("Recent activity lists decisions") { shows("Approved") && shows("Rejected") && shows("Agent access turned on for alex@example.com") }
        capture("08-recent-activity", window)
        button("Recent activity")!.performClick(nil)

        // A long body is hidden until Show message, then shown in full.
        let long = agent("long")
        expect((long["send"] as? [String: Any])?["status"] as? String == "pending_approval", "long draft waits for approval")
        poll("long request appears") { agentBadge.stringValue == "1" }
        wait("long request card") { shows("Long note") && button("Show message") != nil }
        expect(!shows("TAIL-MARKER"), "collapsed card hides the body")
        button("Show message")!.performClick(nil)
        wait("expanded card shows every line") { shows("TAIL-MARKER") }
        decide("Reject")
        wait("long request rejected") { pendingCount() == 0 }

        // Attachments: Show message says how many, and Save writes the exact bytes.
        let attached = agent("attach")
        expect((attached["send"] as? [String: Any])?["status"] as? String == "pending_approval", "attachment draft waits for approval")
        poll("attachment request appears") { agentBadge.stringValue == "1" }
        wait("card says it has an attachment") { button("Show message · 1 attachment") != nil }
        button("Show message · 1 attachment")!.performClick(nil)
        wait("expanded card offers saving the attachment") { shows("notes.txt (") && button("Save…") != nil && shows("The agent attached these files") }
        capture("09-attachment", window)
        let fileRequest = (core!.call(["op": "list_agent_actions", "status": "pending"]) as? [[String: Any]])?.first
        let fileID = (((fileRequest?["details"] as? [String: Any])?["draft"] as? [String: Any])?["attachments"] as? [[String: Any]])?.first?["id"] as? String ?? ""
        let saved = core!.call(["op": "agent_request_attachment", "id": fileRequest?["id"] as? String ?? "", "attachment_id": fileID]) as? [String: Any]
        expect(saved?["filename"] as? String == "notes.txt" && Data(base64Encoded: saved?["data_base64"] as? String ?? "") == Data("hello from the agent\n".utf8), "attachment bytes match what would be sent")
        decide("Reject")
        wait("attachment request rejected") { pendingCount() == 0 }

        // Connect registers the helper in the fake Claude Code, like the script.
        button("Connect")!.performClick(nil)
        wait("Claude Code shows as connected") { shows("Connected") && button("Connect") == nil && shows("Claude Code is connected") }
        let log = (try? String(contentsOfFile: env["FAKE_CLI_DIR"]! + "/claude.log", encoding: .utf8)) ?? ""
        let add = log.split(separator: "\n").first { $0.hasPrefix("mcp add") }.map(String.init) ?? ""
        expect(add.hasPrefix("mcp add --scope user --transport stdio ") && add.contains(" -- \(env["EMAIL_MCP_HELPER"]!) --db \(env["EMAIL_STORE_PATH"]!) --allow-draft --allow-triage --allow-send --allow-sync --allow-calendar-write") && !add.contains("booking"), "Connect runs the script's registration with every mail permission")
        capture("10-connected", window)
        // Copy puts the command and JSON on the pasteboard; the user's clipboard is restored.
        let previousClipboard = NSPasteboard.general.string(forType: .string)
        button("Copy")!.performClick(nil)
        let copied = NSPasteboard.general.string(forType: .string) ?? ""
        expect(copied.contains("Command:\n\(env["EMAIL_MCP_HELPER"]!)") && copied.contains("\"mcpServers\"") && copied.contains("--allow-calendar-write"), "Copy gives the command and a JSON config")
        NSPasteboard.general.clearContents()
        if let previousClipboard { NSPasteboard.general.setString(previousClipboard, forType: .string) }

        // Turning off from the switch asks first and rejects waiting requests.
        let again = agent("send")
        expect((again["send"] as? [String: Any])?["status"] as? String == "pending_approval", "a new request waits again")
        poll("badge shows the new request") { agentBadge.stringValue == "1" }
        let alert = AgentAccessDisclosure.makeTurnOffAlert("alex@example.com")
        expect(alert.buttons.map(\.title) == ["Cancel", "Turn off"] && alert.buttons[0].keyEquivalent == "\u{1b}" && !alert.buttons.contains { $0.keyEquivalent == "\r" }, "turn-off confirmation: Escape cancels, Return never confirms")
        alert.beginSheetModal(for: window) { _ in }
        settle(0.6)
        capture("11-turn-off-confirm", alert.window)
        window.endSheet(alert.window, returnCode: .alertFirstButtonReturn)
        settle()
        AgentAccessDisclosure.confirmTurnOff = { email, _, done in precondition(email == "alex@example.com"); done(false) }
        toggle("alex@example.com")!.performClick(nil)
        wait("cancelling the confirmation keeps access on") { toggle("alex@example.com")?.state == .on && pendingCount() == 1 }
        AgentAccessDisclosure.confirmTurnOff = { email, _, done in precondition(email == "alex@example.com"); done(true) }
        toggle("alex@example.com")!.performClick(nil)
        wait("turning off rejects waiting requests") { pendingCount() == 0 && toggle("alex@example.com")?.state == .off && shows("Off") }
        let refused = agent("read")
        expect("\(refused["get_message"] ?? "")".contains("Agent access is off"), "agent is refused right after turning access off")
        poll("badge clears or flags the new block") { agentBadge.stringValue != "1" }

        // Without a helper, Connect says why instead of offering a broken button.
        unsetenv("EMAIL_MCP_HELPER")
        agentAccess!.refreshClients()
        wait("missing helper is explained") { shows("doesn't include the mail-mcp helper") && button("Copy")?.isEnabled == false }
        capture("12-no-helper", window)
        agentAccess!.close()
        close()
    }
'''
fake_cli = r'''#!/bin/sh
# A stand-in for the claude or codex CLI. It only writes next to itself.
dir="$(cd "$(dirname "$0")" && pwd)"
name="$(basename "$0")"
printf '%s\n' "$*" >> "$dir/$name.log"
case "$1 $2" in
  "mcp get")
    if [ -f "$dir/$name.registered" ] && grep -qx "$3" "$dir/$name.registered"; then echo "$3:"; cat "$dir/$name.command"; exit 0; fi
    echo "No MCP server found with name: $3" >&2; exit 1 ;;
  "mcp add")
    server="$3"; [ "$3" = "--scope" ] && server="$7"
    echo "$server" >> "$dir/$name.registered"; printf 'Command: %s\n' "$*" > "$dir/$name.command"; exit 0 ;;
esac
exit 2
'''
with tempfile.TemporaryDirectory(prefix='email-agent-access-') as temp:
    p = Path(temp)
    ui = p / 'EmailUI.swift'
    anchor = '    @objc private func agentAccessClicked() { openAgentAccess() }'
    text = source.joinpath('EmailUI.swift').read_text()
    assert anchor in text
    ui.write_text(text.replace(anchor, check + '\n' + anchor))
    main = p / 'Check.swift'
    main.write_text('import AppKit\n@main struct Check { @MainActor static func main() { let app = NSApplication.shared; app.setActivationPolicy(.accessory); app.finishLaunching(); let window = MailWindow(); window.checkAgentAccess() } }')
    files = [str(f) for f in source.glob('*.swift') if f.name != 'EmailUI.swift']
    subprocess.run(['swiftc', '-parse-as-library', '-swift-version', '6', '-o', str(p / 'check'), str(ui), str(main), *files], check=True)
    # Only the fake Claude Code exists; Codex shows as not installed.
    clis = p / 'bin'
    clis.mkdir()
    (clis / 'claude').write_text(fake_cli)
    (clis / 'claude').chmod(0o755)
    env = dict(os.environ, EMAIL_STORE_PATH=str(p / 'mail.sqlite'), EMAIL_DEMO='1', MAIL_SERVICE_LIBRARY=str(library),
               MAIL_MCP=str(mcp), AGENT_FLOW=str(root / 'crates/mail-mcp/tests/agent_flow.py'),
               EMAIL_AGENT_CLI_PATH=str(clis), FAKE_CLI_DIR=str(clis), EMAIL_MCP_HELPER=str(mcp),
               EMAIL_CHECK_APPEARANCE=args.appearance)
    if args.snapshots:
        args.snapshots.mkdir(parents=True, exist_ok=True)
        env['EMAIL_CHECK_SNAPSHOTS'] = str(args.snapshots.resolve())
    subprocess.run([str(p / 'check')], env=env, check=True)
    print('PASS: Agent access AppKit check')
