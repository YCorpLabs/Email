#!/usr/bin/env python3
"""Exercise Agent requests in the real AppKit main window and the Agent access
settings window, end to end.

A disposable launch-demo store (two accounts) is shared by the real windows,
the real mail-service library and the real mail-mcp binary acting as agents
(Claude Code and Codex by their reported names). The check covers refused
agents becoming "wants to use" requests, Allow… with the three-choice sheet
(Read only first, Custom…, the full list), send and bulk requests in the main
window with the email as it will go out, ⌘↩ with the undo window, E (Edit) into
the composer without sending, ⌫ on a bulk change and on a refused agent, the
settings popup (allowing less, turning off), Connect with a fake Claude Code,
Set up…, Activity and the missing-helper note. Demo accounts have no provider
configuration, so nothing can be sent.

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
        setvbuf(stdout, nil, _IONBF, 0)
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
            settle(0.7)
            let process = Process()
            process.executableURL = URL(fileURLWithPath: "/usr/sbin/screencapture")
            process.arguments = ["-x", "-o", "-l", String(window.windowNumber), "\(dir)/mac-\(env["EMAIL_CHECK_APPEARANCE"] ?? "dark")-\(name).png"]
            try? process.run()
            process.waitUntilExit()
        }
        func agent(_ step: String, account: String = "demo:personal", client: String = "claude-code", threads: [String] = []) -> [String: Any] {
            let process = Process()
            process.executableURL = URL(fileURLWithPath: "/usr/bin/python3")
            process.arguments = [env["AGENT_FLOW"]!, env["MAIL_MCP"]!, env["EMAIL_STORE_PATH"]!, step, account]
            process.environment = env.merging(["AGENT_FLOW_CLIENT": client, "AGENT_FLOW_THREADS": threads.joined(separator: ",")]) { $1 }
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
        func all(_ view: NSView) -> [NSView] { [view] + view.subviews.flatMap { all($0) } }
        func button(_ title: String, in root: NSView) -> NSButton? {
            all(root).compactMap { $0 as? NSButton }.first { $0.title == title && !$0.isHiddenOrHasHiddenAncestor && !($0 is NSSwitch) && !($0 is NSPopUpButton) }
        }
        func texts(_ root: NSView) -> [String] { all(root).filter { !$0.isHiddenOrHasHiddenAncestor }.compactMap { ($0 as? NSTextField)?.stringValue } }
        func shows(_ fragment: String, in root: NSView) -> Bool { texts(root).contains { $0.contains(fragment) } }
        var reader: NSView { agentRequestReader }
        var settings: NSView { agentAccess!.window!.contentView! }
        func popup(_ email: String) -> NSPopUpButton? { all(settings).compactMap { $0 as? NSPopUpButton }.first { $0.accessibilityLabel() == "What agents can do with \(email)" } }
        func key(_ characters: String, code: UInt16, modifiers: NSEvent.ModifierFlags = []) -> Bool {
            let event = NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: modifiers, timestamp: 0, windowNumber: windowNumber,
                                         context: nil, characters: characters, charactersIgnoringModifiers: characters, isARepeat: false, keyCode: code)!
            return handle(event)
        }
        func selectRequest(_ fragment: String) {
            let index = agentRequests.firstIndex { ($0["title"] as? String ?? "").contains(fragment) }!
            table.selectRowIndexes(IndexSet(integer: index), byExtendingSelection: false)
            settle(0.2)
        }
        func ready() { RunLoop.main.run(until: Date().addingTimeInterval(0.9)) }
        var sheet: AgentAccessSheet? { AgentAccessSheet.presented }
        func pendingCount() -> Int { ((core!.call(["op": "agent_pending_summary"]) as? [String: Any])?["total"] as? NSNumber)?.intValue ?? -1 }

        if env["EMAIL_CHECK_APPEARANCE"] == "light" { NSApp.appearance = NSAppearance(named: .aqua) }
        else { NSApp.appearance = NSAppearance(named: .darkAqua) }
        setContentSize(NSSize(width: 1240, height: 760))
        center()
        makeKeyAndOrderFront(nil)
        settle()
        // Refused agents become requests. Nothing is on yet, so the requests
        // item appears only once something waits.
        expect(agentRequestsSidebarButton?.isHidden == true, "no Agent requests item before access or requests")
        let refusedClaude = agent("use", account: "demo:personal")
        expect("\(refusedClaude["list_threads"] ?? "")".contains("Agent access is off"), "MCP refuses an account that is off")
        _ = agent("use", account: "demo:work", client: "codex-mcp-client")
        poll("the requests item shows with a count") { agentRequestsSidebarButton?.isHidden == false && !agentRequestsPill.isHidden && agentRequestsPill.text == "2" }
        showAgentRequests()
        wait("the list shows both refused agents") { agentRequests.count == 2 && agentRequestReader.request != nil }
        expect(agentRequests.map { $0["title"] as? String ?? "" } == ["Wants to use alex@example.com", "Wants to use alex@fernhill.example"], "access request titles")
        expect(agentRequests.map { $0["agent"] as? String ?? "" } == ["Claude Code", "Codex"], "friendly agent names")
        expect(shows("Claude Code wants to use alex@example.com", in: reader) || shows("wants to use alex@example.com", in: reader), "strip says what the agent wants")
        expect(button("Ignore", in: reader) != nil && button("Allow…", in: reader) != nil, "Ignore and Allow… on a refused agent")

        // Allow… opens the sheet with Read only chosen.
        ready()
        button("Allow…", in: reader)!.performClick(nil)
        wait("Allow… opens the turn-on sheet") { sheet != nil && attachedSheet != nil }
        let sheetView = sheet!.panel.contentView!
        let sheetText = texts(sheetView).joined(separator: "\n")
        expect(sheetText.contains("Let agents use alex@example.com?"), "sheet title names the account")
        expect(sheet!.options.map(\.id) == ["read", "organize", "write"] && sheet!.selectedLevel == "read", "three choices, Read only chosen")
        expect(sheet!.grants == ["sync"], "Read only means read and fetch")
        expect(sheetText.contains("What an agent reads goes to its AI provider, like Anthropic or OpenAI. You can turn this off anytime."), "provider terms and how to turn off")
        expect(sheet!.cancelButton.keyEquivalent == "\u{1b}" && sheet!.confirmButton.keyEquivalent.isEmpty && sheet!.confirmButton.title == "Allow", "Return never allows; Escape cancels")
        expect(!sheetText.contains("Change calendar events") && !sheetText.contains("Open-tracking"), "Custom and the full list start closed")
        capture("05-sheet-default", sheet!.panel)
        sheet!.customLink.performClick(nil)
        settle()
        expect(shows("Change calendar events", in: sheetView) && shows("guests get notified", in: sheetView), "Custom… shows the switches, with calendar changes")
        expect(sheet!.switches.map(\.id) == ["sync", "triage", "draft", "send", "calendar_write"], "every permission has a switch")
        capture("06-sheet-custom", sheet!.panel)
        sheet!.customLink.performClick(nil)
        sheet!.detailsLink.performClick(nil)
        settle()
        let full = texts(sheetView).joined(separator: "\n")
        for fragment in ["senders, recipients, subjects, bodies and attachments", "downloaded from your mail provider", "Drafts and labels", "Snoozed, scheduled and queued mail", "Calendar invitations", "Recipient suggestions, follow-up reminders and signatures", "Open-tracking activity", "snippets and saved splits", "bookings", "untrusted", "MCP registration"] {
            expect(full.contains(fragment), "full list names: \(fragment)")
        }
        capture("07-sheet-details", sheet!.panel)
        sheet!.pick(level: "write")
        expect(Set(sheet!.grants) == ["sync", "triage", "draft", "send"] && sheet!.selectedLevel == "write", "Read, organize and write adds drafts and asking to send")
        sheet!.confirmButton.performClick(nil)
        wait("the refused agent's request goes away once allowed") { attachedSheet == nil && agentRequests.count == 1 }
        let status = core!.call(["op": "agent_access_status"]) as? [String: Any] ?? [:]
        expect((status["accounts"] as? [[String: Any]] ?? []).contains { $0["email"] as? String == "alex@example.com" && $0["level"] as? String == "write" }, "consent recorded at that level")

        // Sends and bulk changes wait in the main window.
        let personal = (core!.call(["op": "list_threads", "account_id": "demo:personal", "limit": 2]) as? [[String: Any]] ?? []).compactMap { $0["id"] as? String }
        let request = agent("showcase", threads: personal)
        expect((request["send"] as? [String: Any])?["status"] as? String == "pending_approval", "agent send waits for approval")
        expect((request["batch"] as? [String: Any])?["status"] as? String == "pending_approval", "agent bulk archive waits for approval")
        let draftID = request["draft"] as? String ?? ""
        func queued(_ id: String) -> OutboxItem? { core!.outbox().first { $0.messageID == id } }
        expect(queued(draftID) == nil, "nothing reaches the outbox before approval")
        poll("the count shows three") { agentRequestsPill.text == "3" }
        wait("the list shows send, bulk and the refused agent") { agentRequests.count == 3 }
        expect(agentRequests.map { $0["title"] as? String ?? "" } == ["Send: Design review time", "Archive 2 conversations", "Wants to use alex@fernhill.example"], "oldest first, refused agents last")
        selectRequest("Send:")
        expect(!agentRequestReader.approveReady && (agentRequestReader.approveButton?.alphaValue ?? 1) < 1, "Send waits a moment after a request appears")
        agentRequestReader.approveButton!.performClick(nil)
        settle(0.2)
        expect(queued(draftID) == nil && agentRequests.count == 3, "an early click sends nothing")
        expect(key("\r", code: 36, modifiers: .command) && queued(draftID) == nil, "an early ⌘↩ sends nothing")
        ready()
        expect(agentRequestReader.approveReady, "then Send is ready")
        expect(shows("wants to send this email", in: reader) && button("Don't send", in: reader) != nil && button("Edit", in: reader) != nil && button("Send", in: reader) != nil, "send strip: Don't send, Edit, Send")
        expect(shows("Design review time", in: reader) && shows("Maya Chen <maya@example.org>", in: reader) && shows("Morning got eaten by the launch prep.", in: reader), "the email as it will go out, in full")
        expect(all(reader).contains { $0.accessibilityLabel() == "Save a copy of review-notes.pdf" }, "attachments as chips")
        expect((agentRequests[0]["preview"] as? String ?? "").hasPrefix("To Maya Chen · Hi Maya, Could we move Friday's"), "list preview: recipient and the start of the body")
        let sentAt = Date(timeIntervalSince1970: ((agentRequests[0]["created_at_ms"] as? NSNumber)?.doubleValue ?? 0) / 1000)
        expect(texts(AgentRequestRows.cell(agentRequests[0])).contains(MailListTime.string(for: sentAt)), "request time as the inbox shows times")
        expect(search.isHidden && searchFiltersButton.isHidden && syncStatus.stringValue == "3 waiting", "no search or filters, just the count")
        _ = key("/", code: 44)
        expect((firstResponder as? NSText)?.delegate !== search, "/ doesn't reach the hidden search")
        capture("01-requests-send", self)
        selectRequest("Archive")
        ready()
        expect(shows("wants to archive 2 conversations", in: reader) && button("Don't archive", in: reader) != nil && button("Archive 2", in: reader) != nil, "bulk strip uses the verb")
        expect(shows("You can undo this afterwards.", in: reader), "undo line")
        capture("02-requests-bulk", self)
        selectRequest("Wants to use")
        ready()
        expect(shows("wants to use alex@fernhill.example", in: reader) && shows("Agent access is off for this account", in: reader), "refused agent reader")
        capture("03-requests-access", self)
        showInbox(scope: .all)
        settle()
        expect(!search.isHidden && !searchFiltersButton.isHidden && !search.isHiddenOrHasHiddenAncestor, "search and filters come back after leaving")

        openAgentAccess()
        wait("settings link to waiting requests") { shows("3 requests waiting", in: settings) && button("Review", in: settings) != nil }
        expect(popup("alex@example.com")?.titleOfSelectedItem == "Read, organize and write" && popup("alex@fernhill.example")?.titleOfSelectedItem == "Off", "one popup per account")
        expect(popup("alex@example.com")!.itemTitles == ["Off", "Read only", "Read and organize", "Read, organize and write", "Custom…"], "popup choices")
        wait("agents rows") { shows("Claude Code", in: settings) && shows("Not installed", in: settings) && button("Connect", in: settings) != nil && button("Set up…", in: settings) != nil }
        expect(!shows("Waiting for you", in: settings) && !shows("Agent access needed", in: settings), "no requests or banner in settings")
        capture("04-settings-waiting", agentAccess!.window)
        button("Review", in: settings)!.performClick(nil)
        settle()
        expect(isVisible && folder == .agentRequests, "Review opens the main window on Agent requests")

        // ⌘↩ sends with the undo window.
        selectRequest("Send:")
        ready()
        expect(key("\r", code: 36, modifiers: .command), "⌘↩ handled")
        wait("approval queues the draft with the undo window") { queued(draftID) != nil && agentRequestReader.undoVisible }
        expect(queued(draftID)?.status == "undoable" && lastQueuedMessageID == draftID, "the approved draft can be undone")
        wait("the next request is selected") { agentRequests.count == 2 && (agentRequestReader.request?["title"] as? String) == "Archive 2 conversations" }
        capture("08-sent-undo", self)
        button("Undo", in: reader)!.performClick(nil)
        wait("Undo returns the draft") { queued(draftID) == nil && !agentRequestReader.undoVisible }

        // E opens the draft in the composer and closes the request without sending.
        let again = agent("send")
        poll("a new send request") { agentRequestsPill.text == "3" }
        showAgentRequests()
        wait("it is listed") { agentRequests.count == 3 }
        selectRequest("Send:")
        ready()
        expect(key("e", code: 14), "E handled")
        wait("Edit opens the draft in the composer") { folder == .drafts && composePane != nil }
        let edited = core!.call(["op": "list_agent_actions", "status": "edited"]) as? [[String: Any]] ?? []
        expect(edited.count == 1 && queued(again["draft"] as? String ?? "") == nil, "the request closed as edited, nothing sent")
        expect(((edited.first?["details"] as? [String: Any])?["request"] as? [String: Any])?["message_id"] as? String == again["draft"] as? String, "the composer has the agent's draft")
        dismissComposePane()

        // ⌫ declines a bulk change and ignores a refused agent.
        showAgentRequests()
        wait("back to two requests") { agentRequests.count == 2 }
        selectRequest("Archive")
        ready()
        expect(key("\u{7f}", code: 51), "⌫ handled")
        wait("the bulk change is declined") { agentRequests.count == 1 }
        let threads = core!.call(["op": "list_threads", "account_id": "demo:personal", "limit": 50]) as? [[String: Any]] ?? []
        expect(personal.allSatisfy { id in threads.contains { $0["id"] as? String == id } }, "declined archive left the conversations alone")
        ready()
        expect(key("\u{7f}", code: 51), "⌫ on the refused agent")
        wait("Ignore clears it") { agentRequests.isEmpty && agentRequestsPill.isHidden }
        expect(agentRequestsSidebarButton?.isHidden == false, "the item stays while an account is on")

        // Settings: allowing less applies at once; Off asks first.
        Task { await agentAccess!.reload() }
        wait("settings shows nothing waiting") { !shows("requests waiting", in: settings) && !shows("request waiting", in: settings) }
        let personalPopup = popup("alex@example.com")!
        personalPopup.selectItem(withTitle: "Read only")
        _ = personalPopup.target?.perform(personalPopup.action, with: personalPopup)
        wait("allowing less needs no sheet") { agentAccess!.window!.attachedSheet == nil && popup("alex@example.com")?.titleOfSelectedItem == "Read only" && (core!.call(["op": "agent_access_status"]) as? [String: Any]).map { (($0["accounts"] as? [[String: Any]]) ?? []).contains { $0["email"] as? String == "alex@example.com" && $0["level"] as? String == "read" } } == true }
        AgentAccessDisclosure.confirmTurnOff = { email, _, done in precondition(email == "alex@example.com"); done(true) }
        let offPopup = popup("alex@example.com")!
        offPopup.selectItem(withTitle: "Off")
        _ = offPopup.target?.perform(offPopup.action, with: offPopup)
        func level(_ email: String) -> String? {
            ((core!.call(["op": "agent_access_status"]) as? [String: Any])?["accounts"] as? [[String: Any]] ?? []).first { $0["email"] as? String == email }?["level"] as? String
        }
        wait("turning off") { level("alex@example.com") == "off" && popup("alex@example.com")?.titleOfSelectedItem == "Off" }
        let refused = agent("use", account: "demo:personal")
        expect("\(refused["list_threads"] ?? "")".contains("Agent access is off"), "agent is refused right after turning off")

        // Connect with the fake Claude Code; Set up… copies the config.
        button("Connect", in: settings)!.performClick(nil)
        wait("Claude Code connected") { shows("✓ Connected", in: settings) && shows("Start a new session to use it", in: settings) }
        let log = (try? String(contentsOfFile: env["FAKE_CLI_DIR"]! + "/claude.log", encoding: .utf8)) ?? ""
        let add = log.split(separator: "\n").first { $0.hasPrefix("mcp add") }.map(String.init) ?? ""
        expect(add.hasPrefix("mcp add --scope user --transport stdio ") && add.contains(" -- \(env["EMAIL_MCP_HELPER"]!) --db \(env["EMAIL_STORE_PATH"]!) --allow-draft --allow-triage --allow-send --allow-sync --allow-calendar-write") && !add.contains("booking"), "Connect runs the script's registration")
        _ = core!.call(["op": "dismiss_agent_access_request", "account_id": "demo:personal"])
        Task { await agentAccess!.reload() }
        settle(0.6)
        capture("09-settings-nothing-waiting", agentAccess!.window)
        let previousClipboard = NSPasteboard.general.string(forType: .string)
        button("Set up…", in: settings)!.performClick(nil)
        let copied = NSPasteboard.general.string(forType: .string) ?? ""
        expect(copied.contains("Command:\n\(env["EMAIL_MCP_HELPER"]!)") && copied.contains("\"mcpServers\""), "Set up… copies the command and JSON")
        NSPasteboard.general.clearContents()
        if let previousClipboard { NSPasteboard.general.setString(previousClipboard, forType: .string) }
        button("Activity", in: settings)!.performClick(nil)
        wait("Activity lists decisions") { agentAccess!.window!.attachedSheet != nil && shows("Approved", in: agentAccess!.window!.attachedSheet!.contentView!) && shows("Opened to edit", in: agentAccess!.window!.attachedSheet!.contentView!) }
        capture("10-activity", agentAccess!.window!.attachedSheet)
        agentAccess!.window!.endSheet(agentAccess!.window!.attachedSheet!)

        // Without a helper, the agents rows say why.
        unsetenv("EMAIL_MCP_HELPER")
        agentAccess!.refreshClients()
        wait("missing helper is explained") { shows("doesn't include the mail-mcp helper", in: settings) && button("Set up…", in: settings)?.isEnabled == false }
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
    env = dict(os.environ, EMAIL_STORE_PATH=str(p / 'mail.sqlite'), EMAIL_DEMO='launch', MAIL_SERVICE_LIBRARY=str(library),
               MAIL_MCP=str(mcp), AGENT_FLOW=str(root / 'crates/mail-mcp/tests/agent_flow.py'),
               EMAIL_AGENT_CLI_PATH=str(clis), FAKE_CLI_DIR=str(clis), EMAIL_MCP_HELPER=str(mcp),
               EMAIL_CHECK_APPEARANCE=args.appearance)
    if args.snapshots:
        args.snapshots.mkdir(parents=True, exist_ok=True)
        env['EMAIL_CHECK_SNAPSHOTS'] = str(args.snapshots.resolve())
    subprocess.run([str(p / 'check')], env=env, check=True)
    print('PASS: Agent access AppKit check')
