// Standalone fixture checks. Run with run-tracking-activity-checks.py, which compiles
// this file together with the actual TrackingPanel.swift and no production bridge.
import AppKit

@MainActor private enum EmailTheme {
    static let reader = NSColor.windowBackgroundColor
    static let line = NSColor.separatorColor
    static let ink = NSColor.labelColor
    static let muted = NSColor.secondaryLabelColor
}
private struct MailServiceError: LocalizedError { let message: String; var errorDescription: String? { message } }
@MainActor final class CoreBridge {
    var requests: [[String: Any]] = []
    var failRefresh = false
    var rejectReminder = false
    var fixture: [[String: Any]] = []
    func callAsync(_ request: [String: Any]) async throws -> Any {
        requests.append(request)
        let op = request["op"] as? String ?? ""
        if op == "get_message" {
            return fixture.first(where: { $0["message_id"] as? String == request["id"] as? String }).map {
                ["id": $0["message_id"]!, "account_id": $0["account_id"]!, "thread_id": $0["thread_id"]!]
            } ?? [:]
        }
        if op == "set_followup" && rejectReminder { throw MailServiceError(message: "A reply is already cached. Refresh activity before setting a reminder.") }
        if op == "set_followup", let index = fixture.firstIndex(where: { $0["thread_id"] as? String == request["thread_id"] as? String }) {
            fixture[index]["followup"] = ["status": "waiting", "due_at_ms": request["due_at_ms"]!, "only_if_no_reply": true]
        }
        if op == "cancel_followup", let index = fixture.firstIndex(where: { $0["thread_id"] as? String == request["thread_id"] as? String }) { fixture[index]["followup"] = NSNull() }
        if op == "tracking_activity" {
            if request["refresh"] as? Bool == true && failRefresh { throw MailServiceError(message: "Fixture offline") }
            let items = request["message_id"] as? String == nil ? fixture : fixture.filter { $0["message_id"] as? String == request["message_id"] as? String }
            return ["items": items, "cached": request["refresh"] as? Bool != true, "refreshed_at_ms": Date().timeIntervalSince1970 * 1000] as [String: Any]
        }
        return [:]
    }
}

@MainActor private extension TrackingActivityView {
    func fixtureSnapshot(_ path: String, width: CGFloat, window: NSWindow) throws {
        window.setContentSize(NSSize(width: width, height: 580))
        view.frame = NSRect(x: 0, y: 0, width: width, height: 580)
        view.layoutSubtreeIfNeeded(); view.displayIfNeeded()
        assert(filters.isHidden || filters.frame.maxX <= view.bounds.width - 15, "Filter overflow")
        assert(view.bounds.width <= width + 1, "Window cannot shrink to requested width")
        assert(rows.bounds.width > 0 && rows.bounds.width <= width - 35, "Content width invalid")
        guard let bitmap = view.bitmapImageRepForCachingDisplay(in: view.bounds) else { fatalError("No bitmap") }
        view.cacheDisplay(in: view.bounds, to: bitmap)
        try bitmap.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: path))
    }
    func fixtureCheck(core: CoreBridge, window: NSWindow) async throws {
        let windowCount = NSApp.windows.count
        load(refresh: false); try await Task.sleep(for: .milliseconds(60))
        func subjects() -> [String] { rows.arrangedSubviews.compactMap { ($0 as? TrackingActionButton)?.title } }
        assert(subjects().count == 3, "All filter must include every fixture")
        filters.selectedSegment = 1; filterChanged()
        assert(subjects().count == 2, "Awaiting reply excludes cached reply")
        filters.selectedSegment = 2; filterChanged()
        assert(subjects().count == 2, "Activity includes reply-only message")
        filters.selectedSegment = 0; filterChanged()
        try fixtureSnapshot(NSTemporaryDirectory() + "email-activity-macos-feed-480.png", width: 480, window: window)
        try fixtureSnapshot(NSTemporaryDirectory() + "email-activity-macos-feed-360.png", width: 360, window: window)
        func key(_ code: UInt16, _ characters: String) -> NSEvent {
            NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: [], timestamp: 0,
                windowNumber: window.windowNumber, context: nil, characters: characters,
                charactersIgnoringModifiers: characters, isARepeat: false, keyCode: code)!
        }
        focusActivity()
        assert((window.firstResponder as? NSButton)?.title == subjects()[0])
        assert(handleKeyboard(key(38, "j")))
        assert((window.firstResponder as? NSButton)?.title == subjects()[1])
        assert(handleKeyboard(key(40, "k")))
        assert((window.firstResponder as? NSButton)?.title == subjects()[0])
        assert(handleKeyboard(key(36, "\r")))
        let selected = core.fixture[0]
        showDetail(selected); try await Task.sleep(for: .milliseconds(60))
        assert(detailItem?["message_id"] as? String == "m1")
        assert(!back.isHidden && filters.isHidden)
        assert(NSApp.windows.count == windowCount, "Activity navigation must stay in its existing host window")
        let button = descendant(view, identifier: NSUserInterfaceItemIdentifier("activity-open"))!
        window.makeFirstResponder(button)
        render()
        assert((window.firstResponder as? NSView)?.identifier?.rawValue == "activity-open", "Rerender must preserve action focus")
        core.failRefresh = true; load(refresh: true); try await Task.sleep(for: .milliseconds(60))
        assert(detailItem?["pixel_requests"] as? Int == 4, "Offline refresh keeps cached data")
        assert(!status.isHidden && status.stringValue.contains("saved activity"))
        try fixtureSnapshot(NSTemporaryDirectory() + "email-activity-macos-detail-360.png", width: 360, window: window)
        core.failRefresh = false
        mutateReminder(selected, cancel: false); try await Task.sleep(for: .milliseconds(60))
        let request = core.requests.last(where: { $0["op"] as? String == "set_followup" })!
        assert(request["only_if_no_reply"] as? Bool == true)
        assert(request["sent_message_id"] as? String == "m1", "Reminder must be anchored to selected sent message")
        assert((request["due_at_ms"] as? Int64 ?? 0) > Int64(Date().addingTimeInterval(2.9 * 86400).timeIntervalSince1970 * 1000))
        assert(activeReminder(detailItem!) != nil)
        mutateReminder(detailItem!, cancel: true); try await Task.sleep(for: .milliseconds(60))
        assert(activeReminder(detailItem!) == nil)
        core.rejectReminder = true; core.fixture[0]["reply_received"] = true
        mutateReminder(selected, cancel: false); try await Task.sleep(for: .milliseconds(60))
        assert(activeReminder(detailItem!) == nil && detailItem?["reply_received"] as? Bool == true)
        assert(!status.isHidden && status.stringValue.contains("reply is already cached"))
        assert(core.requests.last?["op"] as? String == "tracking_activity" && core.requests.last?["refresh"] as? Bool == false)
        core.rejectReminder = false
        open(selected); try await Task.sleep(for: .milliseconds(60))
        assert(status.stringValue == "Close your draft before opening another conversation.", "Blocked navigation should remain visible")
        cancelOperation(nil); try await Task.sleep(for: .milliseconds(60))
        assert(selectedMessageID == nil && !filters.isHidden, "Escape from detail returns to feed")
        let returnTarget = NSButton(title: "Mail", target: nil, action: nil)
        view.addSubview(returnTarget)
        var dismissed = false
        dismiss = { dismissed = true; window.makeFirstResponder(returnTarget) }
        focusActivity()
        assert(handleKeyboard(key(53, "\u{1b}")))
        assert(dismissed && window.firstResponder === returnTarget, "Escape closes the pane and restores host focus")
        assert(!core.requests.contains { ["send_draft", "sync_account", "booking_connect_start"].contains($0["op"] as? String ?? "") })
        print("PASS: filters, timeline detail, 360/480 widths, retained focus, offline cache, reply-aware anchored reminder create/cancel/refusal, blocked draft navigation, Escape, no provider writes")
    }
}

@main private struct TrackingActivityChecks {
    @MainActor static func main() async throws {
        _ = NSApplication.shared
        NSApp.setActivationPolicy(.prohibited)
        let now = Int64(Date().timeIntervalSince1970 * 1000)
        let core = CoreBridge()
        let base: [String: Any] = ["account_id": "a1", "recipient": "Jordan <jordan@example.test>", "sent_at_ms": now - 3_600_000,
            "tracking_status": "active", "reply_coverage": "cached", "timeline_complete": true, "followup": NSNull()]
        core.fixture = [("m1", "Product discussion and next steps for the upcoming launch", 4, false), ("m2", "A reply without recorded opens", 0, true), ("m3", "Waiting for a response", 0, false)].map { id, subject, count, replied in
            var item = base
            item.merge(["message_id": id, "thread_id": "t-" + id, "subject": subject, "pixel_requests": count,
                "reply_received": replied, "last_activity_at_ms": now, "last_requested_at": count > 0 ? now - 60_000 : NSNull(),
                "timeline": [["kind": "sent", "at_ms": now - 3_600_000], ["kind": "image_request", "at_ms": now - 60_000, "count": 4]]]) { _, value in value }
            return item
        }
        let controller = TrackingActivityView(core: core, messageID: nil)
        controller.openConversation = { _ in "Close your draft before opening another conversation." }
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 480, height: 580), styleMask: [.borderless], backing: .buffered, defer: false)
        window.appearance = NSAppearance(named: .darkAqua)
        window.contentView = controller.view
        try await controller.fixtureCheck(core: core, window: window)
        let summary = OpenActivitySummary(frame: NSRect(x: 0, y: 0, width: 230, height: 400))
        let host = NSView(frame: NSRect(x: 0, y: 0, width: 270, height: 440))
        window.contentView = host; window.setContentSize(NSSize(width: 270, height: 440))
        host.addSubview(summary); summary.translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([
            summary.leadingAnchor.constraint(equalTo: host.leadingAnchor, constant: 20), summary.trailingAnchor.constraint(equalTo: host.trailingAnchor, constant: -20),
            summary.topAnchor.constraint(equalTo: host.topAnchor, constant: 20)
        ])
        func labels(_ view: NSView) -> [String] { (view as? NSTextField).map { [$0.stringValue] } ?? view.subviews.flatMap(labels) }
        var selected = core.fixture[0]; selected["reply_received"] = false
        summary.show(selected); window.layoutIfNeeded(); host.layoutSubtreeIfNeeded(); summary.layoutSubtreeIfNeeded()
        assert(labels(summary).contains("Opened 4 times"))
        assert(!labels(summary).contains(selected["subject"] as? String ?? ""), "Do not repeat reader context in the contact card")
        assert(abs(summary.bounds.width - 230) <= 1 && summary.frame.maxX <= 251, "Summary frame \(summary.frame), host \(host.frame)")
        if let bitmap = host.bitmapImageRepForCachingDisplay(in: host.bounds) {
            host.cacheDisplay(in: host.bounds, to: bitmap)
            try bitmap.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: NSTemporaryDirectory() + "email-contact-activity-macos-270.png"))
        }
        var refreshed = false; summary.onRefresh = { refreshed = true }
        summary.focusRefresh()
        (window.firstResponder as? NSButton)?.performClick(nil)
        assert(refreshed, "The inline refresh control is keyboard accessible")
        let historyTimes = (1...5).map { 1_790_000_000_000.0 + Double($0) * 60_000 }
        selected["timeline"] = historyTimes.map { ["kind": "image_request", "at_ms": $0, "count": 1] as [String: Any] }
        summary.show(selected)
        for timestamp in historyTimes {
            assert(labels(summary).contains("Opened\n\(TrackingActivityView.exact(timestamp))"), "All available opens, including entries older than the newest three, remain visible")
        }
        var next = core.fixture[1]; next["timeline"] = []
        summary.show(next)
        assert(labels(summary).contains("No opens recorded") && labels(summary).contains("Reply received"))
        assert(!labels(summary).contains("Opened 4 times"), "Changing messages must clear prior activity")
        summary.show(nil); assert(summary.isHidden, "Untracked messages do not show an activity section")
        print("PASS: compact contact activity at 270 pixels, no duplicated context, keyboard refresh, message switch and untracked clearing")
    }
}
