#!/usr/bin/env python3
"""Exercise the actual AppKit Agent access window end to end.

A disposable demo store is shared by the real window, the real mail-service
library and the real mail-mcp binary acting as an agent. The check covers the
blocked-attempt prompt after an upgrade, the disclosure, consent, pending
send and bulk-triage requests, approve with undo, reject, the narrow layout,
and turning access off. Demo accounts have no provider configuration, so
nothing can be sent.
"""
from pathlib import Path
import argparse, os, subprocess, tempfile

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--library', type=Path, required=True, help='Built macOS libmail_service.dylib')
parser.add_argument('--mcp', type=Path, required=True, help='Built mail-mcp binary')
parser.add_argument('--snapshots', type=Path, help='Optional directory for PNG captures of key states')
args = parser.parse_args()
library = args.library.resolve(strict=True)
mcp = args.mcp.resolve(strict=True)
root = Path(__file__).resolve().parents[3]
source = root / 'apps/macos/Sources/EmailUI'
check = r'''
    func checkAgentAccess() {
        func expect(_ ok: Bool, _ name: String) { precondition(ok, name); print("PASS \(name)") }
        func wait(_ name: String, _ condition: () -> Bool) {
            let deadline = Date().addingTimeInterval(8)
            while !condition() && Date() < deadline { RunLoop.main.run(until: Date().addingTimeInterval(0.02)) }
            expect(condition(), name)
        }
        func snapshot(_ name: String, _ view: NSView?) {
            guard let dir = ProcessInfo.processInfo.environment["EMAIL_CHECK_SNAPSHOTS"], let view else { return }
            RunLoop.main.run(until: Date().addingTimeInterval(0.3))
            view.layoutSubtreeIfNeeded()
            guard let rep = view.bitmapImageRepForCachingDisplay(in: view.bounds) else { return }
            view.cacheDisplay(in: view.bounds, to: rep)
            try? rep.representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: dir).appendingPathComponent(name + ".png"))
        }
        func agent(_ step: String) -> [String: Any] {
            let env = ProcessInfo.processInfo.environment
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
            let deadline = Date().addingTimeInterval(8)
            while !condition() && Date() < deadline {
                Task { @MainActor in await self.refreshAgentRequests() }
                RunLoop.main.run(until: Date().addingTimeInterval(0.2))
            }
            expect(condition(), name)
        }
        var panel: NSView { agentAccess!.window!.contentView! }
        func all(_ view: NSView) -> [NSView] { [view] + view.subviews.flatMap { all($0) } }
        func button(_ title: String) -> NSButton? { all(panel).compactMap { $0 as? NSButton }.first { $0.title == title && !$0.isHidden } }
        func shows(_ fragment: String) -> Bool { all(panel).compactMap { $0 as? NSTextField }.contains { $0.stringValue.contains(fragment) } }
        func decide(_ title: String) {
            // Decisions within 0.8 s of a re-render are ignored by design.
            RunLoop.main.run(until: Date().addingTimeInterval(0.9))
            button(title)!.performClick(nil)
        }
        func pendingCount() -> Int { ((core!.call(["op": "agent_pending_summary"]) as? [String: Any])?["count"] as? NSNumber)?.intValue ?? -1 }
        func visible(_ button: NSButton?) -> Bool {
            guard let button, let window = button.window else { return false }
            let frame = button.convert(button.bounds, to: nil)
            return frame.width > 20 && frame.minX >= 0 && frame.maxX <= window.contentLayoutRect.width + 1
        }

        makeKeyAndOrderFront(nil)
        // An upgraded registration is refused until the user turns access on.
        let probe = agent("probe")
        expect("\(probe["list_accounts"] ?? "")".contains("Agent access"), "MCP refuses an account without consent and names Agent access")
        poll("sidebar flags the blocked agent") { agentBadge.stringValue == "!" && !agentBadge.isHidden }
        expect(paletteItems().contains { $0.target == .agentAccess }, "palette offers Agent access")
        openAgentAccess()
        wait("panel lists the account as off") { shows("alex@example.com") && shows("Off") && button("Turn On…") != nil }
        expect(shows("Agent access needed") && button("Turn On for alex@example.com…") != nil, "blocked banner offers the consent step")
        snapshot("1-blocked-after-upgrade", panel)

        let disclosure = core!.call(["op": "agent_access_disclosure", "account_id": "demo:personal"]) as! [String: Any]
        let (alert, boxes) = AgentAccessDisclosure.makeAlert(disclosure, changing: nil)
        alert.layout()
        alert.window.center()
        alert.window.orderFront(nil)
        RunLoop.main.run(until: Date().addingTimeInterval(0.4))
        let alertText = all(alert.window.contentView!).compactMap { ($0 as? NSTextField)?.stringValue }.joined(separator: "\n")
        expect(alertText.contains("Messages: senders, recipients, subjects, bodies and attachments"), "disclosure names data categories")
        expect(alertText.contains("that provider's terms and privacy policy apply"), "disclosure says provider terms apply")
        expect(alertText.contains("turn agent access off at any time"), "disclosure says how to revoke")
        expect(boxes.map(\.0) == ["draft", "triage", "send", "sync", "calendar_write"] && boxes.allSatisfy { $0.1.state == .off }, "optional grants start off")
        // NSAlert's own chrome does not render offscreen; capture the disclosure content.
        alert.accessoryView?.wantsLayer = true
        alert.accessoryView?.appearance = alert.window.effectiveAppearance
        alert.accessoryView?.layer?.backgroundColor = NSColor.windowBackgroundColor.cgColor
        snapshot("2-disclosure", alert.accessoryView)
        alert.window.orderOut(nil)

        expect(alert.buttons.map(\.title) == ["Cancel", "Turn On Agent Access"] && alert.buttons[0].keyEquivalent == "\u{1b}" && !alert.buttons.contains { $0.keyEquivalent == "\r" }, "Return never confirms the disclosure; Escape cancels")
        AgentAccessDisclosure.choose = { shown, current in
            precondition(current == nil && shown["version"] as? Int == 1)
            return ["draft", "triage", "send"]
        }
        button("Turn On for alex@example.com…")!.performClick(nil)
        wait("consent is recorded with the chosen grants") { shows("On since") && shows("Read, Drafts, Triage, Send requests") }
        expect(!shows("Agent access needed"), "the upgrade prompt disappears once access is on")
        poll("the sidebar flag clears once access is on") { agentBadge.isHidden }
        let read = agent("read")
        expect((read["get_message"] as? [String: Any])?["id"] as? String == "demo:1", "agent can read after consent")

        let request = agent("request")
        expect((request["send"] as? [String: Any])?["status"] as? String == "pending_approval", "agent send becomes a pending approval")
        expect((request["batch"] as? [String: Any])?["status"] as? String == "pending_approval", "agent bulk archive becomes a pending approval")
        let draftID = request["draft"] as! String
        expect(core!.outbox().isEmpty, "nothing reaches the outbox before approval")
        poll("sidebar counts two requests") { agentBadge.stringValue == "2" }
        wait("panel shows both requests") { button("Approve and Send") != nil && shows("SEND EMAIL") && shows("CHANGE CONVERSATIONS") }
        expect(shows("maya@example.org") && shows("Design review time") && shows("Could we move Friday") && shows("requested by agent-flow-check"), "send card shows recipients, subject, body and client")
        expect(shows("Archive 2 conversations") && shows("Weekend plans"), "batch card shows the action and conversations")
        button("Approve and Send")!.performClick(nil)
        RunLoop.main.run(until: Date().addingTimeInterval(0.3))
        expect(core!.outbox().isEmpty && shows("The list just changed"), "a click right after the list changed is ignored")
        snapshot("3-pending", panel)
        agentAccess!.window!.setContentSize(NSSize(width: 440, height: 640))
        RunLoop.main.run(until: Date().addingTimeInterval(0.4))
        expect(visible(button("Approve and Send")) && visible(button("Reject")), "narrow window keeps the decision buttons inside the window")
        snapshot("4-pending-narrow", panel)
        agentAccess!.window!.setContentSize(NSSize(width: 600, height: 680))

        decide("Approve and Send")
        wait("approval queues the draft with an undo window") { core!.outbox().count == 1 }
        expect(core!.outbox()[0].messageID == draftID && core!.outbox()[0].status == "undoable", "approved draft is undoable")
        wait("main window offers Undo send") { !cancelSendButton.isHidden && lastQueuedMessageID == draftID }
        wait("panel offers Undo Send") { button("Undo Send") != nil && shows("Approved · sending in") }
        snapshot("5-approved-send", panel)
        button("Undo Send")!.performClick(nil)
        wait("Undo Send returns the draft") { core!.outbox().isEmpty && shows("Send cancelled") }

        wait("only the batch remains") { pendingCount() == 1 && button("Approve and Send") == nil }
        decide("Reject")
        wait("rejecting closes the batch without changes") { pendingCount() == 0 && shows("Rejected · nothing was sent or changed") }
        let threads = core!.call(["op": "list_threads", "limit": 50]) as? [[String: Any]] ?? []
        expect(threads.contains { $0["id"] as? String == "demo:2" }, "rejected archive left the conversation in the inbox")
        wait("history lists decisions") { shows("Approved") && shows("Rejected") && shows("Agent access turned on for alex@example.com") }
        snapshot("6-history", panel)

        // A long body is shortened in the card, says so, and expands in full.
        let long = agent("long")
        expect((long["send"] as? [String: Any])?["status"] as? String == "pending_approval", "long draft waits for approval")
        poll("long request appears") { agentBadge.stringValue == "1" }
        wait("card notes the shortened preview") { shows("Only the beginning is shown") && button("Show full message") != nil }
        expect(!shows("TAIL-MARKER"), "collapsed preview hides the end")
        button("Show full message")!.performClick(nil)
        wait("expanded preview shows every line") { shows("TAIL-MARKER") && !shows("Only the beginning is shown") }
        decide("Reject")
        wait("long request rejected") { pendingCount() == 0 }

        // Attachments are listed with a Save button that writes the exact bytes.
        let attached = agent("attach")
        expect((attached["send"] as? [String: Any])?["status"] as? String == "pending_approval", "attachment draft waits for approval")
        poll("attachment request appears") { agentBadge.stringValue == "1" }
        wait("card offers saving the attachment") { shows("notes.txt (") && button("Save…") != nil && shows("The agent attached these files") }
        snapshot("6b-attachment", panel)
        let fileRequest = (core!.call(["op": "list_agent_actions", "status": "pending"]) as? [[String: Any]])?.first
        let fileID = (((fileRequest?["details"] as? [String: Any])?["draft"] as? [String: Any])?["attachments"] as? [[String: Any]])?.first?["id"] as? String ?? ""
        let saved = core!.call(["op": "agent_request_attachment", "id": fileRequest?["id"] as? String ?? "", "attachment_id": fileID]) as? [String: Any]
        expect(saved?["filename"] as? String == "notes.txt" && Data(base64Encoded: saved?["data_base64"] as? String ?? "") == Data("hello from the agent\n".utf8), "attachment bytes match what would be sent")
        decide("Reject")
        wait("attachment request rejected") { pendingCount() == 0 }

        let again = agent("send")
        expect((again["send"] as? [String: Any])?["status"] as? String == "pending_approval", "a new request waits again")
        poll("badge shows the new request") { agentBadge.stringValue == "1" }
        AgentAccessDisclosure.confirmTurnOff = { email in precondition(email == "alex@example.com"); return true }
        wait("Turn Off is available") { button("Turn Off") != nil }
        button("Turn Off")!.performClick(nil)
        wait("turning off rejects waiting requests") { pendingCount() == 0 && button("Turn On…") != nil }
        let refused = agent("read")
        expect("\(refused["get_message"] ?? "")".contains("Agent access is off"), "agent is refused right after turning access off")
        poll("badge clears or flags the new block") { agentBadge.stringValue != "1" }
        snapshot("7-turned-off", panel)
        agentAccess!.close()
        close()
    }
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
    env = dict(os.environ, EMAIL_STORE_PATH=str(p / 'mail.sqlite'), EMAIL_DEMO='1', MAIL_SERVICE_LIBRARY=str(library),
               MAIL_MCP=str(mcp), AGENT_FLOW=str(root / 'crates/mail-mcp/tests/agent_flow.py'))
    if args.snapshots:
        args.snapshots.mkdir(parents=True, exist_ok=True)
        env['EMAIL_CHECK_SNAPSHOTS'] = str(args.snapshots.resolve())
    subprocess.run([str(p / 'check')], env=env, check=True)
    print('PASS: Agent access AppKit check')
