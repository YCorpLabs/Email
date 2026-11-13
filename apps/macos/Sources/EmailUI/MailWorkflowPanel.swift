import AppKit

/// Local reminders and cached invitations. Provider calendar sync is a separate capability.
@MainActor
final class MailWorkflowPanel: NSViewController, NSTextFieldDelegate {
    private let core: CoreBridge
    private let calendar: Bool
    private let threadID: String?
    private let messageID: String?
    private let width: CGFloat
    private let onChanged: () -> Void
    private let rows = NSStackView()
    private let notice = NSTextField(wrappingLabelWithString: "")
    private let duration = NSTextField(string: "3d")
    private let conditional = NSButton(checkboxWithTitle: "Only if nobody replies", target: nil, action: nil)
    private let preview = NSTextField(wrappingLabelWithString: "")
    private let save = NSButton(title: "Set follow-up", target: nil, action: nil)
    private var due: Date?
    private var busy = false
    var dismiss: (() -> Void)?

    init(core: CoreBridge, calendar: Bool, threadID: String?, messageID: String?, width: CGFloat, onChanged: @escaping () -> Void) {
        self.core = core; self.calendar = calendar; self.threadID = threadID; self.messageID = messageID; self.width = width; self.onChanged = onChanged
        super.init(nibName: nil, bundle: nil)
    }
    required init?(coder: NSCoder) { fatalError() }
    override func loadView() {
        let root = WorkflowSurface(); root.translatesAutoresizingMaskIntoConstraints = false
        let content = NSStackView(); content.orientation = .vertical; content.alignment = .leading; content.spacing = 12; content.translatesAutoresizingMaskIntoConstraints = false
        root.addSubview(content)
        NSLayoutConstraint.activate([content.topAnchor.constraint(equalTo: root.topAnchor, constant: 20), content.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 20), content.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -20), content.bottomAnchor.constraint(equalTo: root.bottomAnchor, constant: -20), root.widthAnchor.constraint(equalToConstant: width)])
        let heading = NSTextField(labelWithString: calendar ? "Calendar invitations" : "Follow up")
        heading.font = .systemFont(ofSize: 18, weight: .semibold); heading.textColor = EmailTheme.ink; content.addArrangedSubview(heading)
        let hint = NSTextField(wrappingLabelWithString: calendar ? "Meeting details saved from your emails. This is an invitation list, not your live calendar or availability." : "Keep this conversation in view until it needs your attention. A new incoming reply can cancel the reminder.")
        hint.font = .systemFont(ofSize: 12); hint.textColor = EmailTheme.muted; content.addArrangedSubview(hint)
        if calendar {
            let agenda = NSButton(title: "Open connected calendar…", target: self, action: #selector(openAgenda)); content.addArrangedSubview(agenda)
            let scan = NSButton(title: "Check selected email for invitations", target: self, action: #selector(scanEmail)); scan.isEnabled = messageID != nil
            scan.toolTip = "Fetch calendar attachments for this email only. Does not RSVP or add a calendar event."
            content.addArrangedSubview(scan)
        } else {
            let row = NSStackView(); row.spacing = 10
            row.addArrangedSubview(NSTextField(labelWithString: "Remind me in"))
            duration.placeholderString = "5m, 2h, 3d, 1w"; duration.delegate = self; duration.setAccessibilityLabel("Follow-up duration")
            duration.widthAnchor.constraint(greaterThanOrEqualToConstant: 150).isActive = true
            row.addArrangedSubview(duration); content.addArrangedSubview(row)
            conditional.state = .on; content.addArrangedSubview(conditional)
            preview.font = .systemFont(ofSize: 12); preview.textColor = EmailTheme.muted; content.addArrangedSubview(preview)
            save.target = self; save.action = #selector(setFollowup); save.keyEquivalent = "\r"; content.addArrangedSubview(save)
        }
        notice.font = .systemFont(ofSize: 12); notice.textColor = EmailTheme.muted; content.addArrangedSubview(notice)
        rows.orientation = .vertical; rows.alignment = .leading; rows.spacing = 14; rows.translatesAutoresizingMaskIntoConstraints = false
        let document = WorkflowDocument(); document.translatesAutoresizingMaskIntoConstraints = false; document.addSubview(rows)
        let scroll = NSScrollView(); scroll.drawsBackground = false; scroll.hasVerticalScroller = true; scroll.documentView = document; scroll.translatesAutoresizingMaskIntoConstraints = false
        content.addArrangedSubview(scroll)
        NSLayoutConstraint.activate([scroll.widthAnchor.constraint(equalTo: content.widthAnchor), scroll.heightAnchor.constraint(equalToConstant: calendar ? 180 : 90), document.widthAnchor.constraint(equalTo: scroll.contentView.widthAnchor), rows.leadingAnchor.constraint(equalTo: document.leadingAnchor), rows.trailingAnchor.constraint(equalTo: document.trailingAnchor), rows.topAnchor.constraint(equalTo: document.topAnchor), rows.bottomAnchor.constraint(equalTo: document.bottomAnchor)])
        let footer = NSStackView(); footer.spacing = 12
        let local = NSTextField(wrappingLabelWithString: calendar ? "Recurring meetings are not expanded. Times retain the invitation’s timezone." : "Local to this device · checked while Email runs or when reopened.")
        local.font = .systemFont(ofSize: 11); local.textColor = EmailTheme.muted; footer.addArrangedSubview(local)
        let done = NSButton(title: "Done", target: self, action: #selector(close)); done.keyEquivalent = "\u{1b}"; footer.addArrangedSubview(done); content.addArrangedSubview(footer)
        for child in content.arrangedSubviews { child.widthAnchor.constraint(lessThanOrEqualTo: content.widthAnchor).isActive = true }
        view = root; updateDuration(); preferredContentSize = NSSize(width: width, height: root.fittingSize.height)
        Task { await reload() }
    }
    override func viewDidAppear() { super.viewDidAppear(); if !calendar { view.window?.makeFirstResponder(duration) } }
    func controlTextDidChange(_ notification: Notification) { updateDuration() }
    func control(_ control: NSControl, textView: NSTextView, doCommandBy selector: Selector) -> Bool {
        if selector == #selector(NSResponder.insertNewline(_:)) { setFollowup(); return true }
        if selector == #selector(NSResponder.cancelOperation(_:)) { close(); return true }
        return false
    }
    private func updateDuration() {
        let raw = duration.stringValue.trimmingCharacters(in: .whitespacesAndNewlines).lowercased().replacingOccurrences(of: " ", with: "")
        let digits = String(raw.prefix(while: { $0.isNumber })); let unit = String(raw.dropFirst(digits.count))
        let multiplier: Double? = ["m":60, "min":60, "mins":60, "minute":60, "minutes":60, "h":3600, "hour":3600, "hours":3600, "d":86400, "day":86400, "days":86400, "w":604800, "week":604800, "weeks":604800][unit]
        if let count = Double(digits), count > 0, let multiplier, count * multiplier <= 366 * 86400 {
            due = Date().addingTimeInterval(count * multiplier)
            preview.stringValue = "In \(Int(count)) \(multiplier == 60 ? (count == 1 ? "minute" : "minutes") : multiplier == 3600 ? (count == 1 ? "hour" : "hours") : multiplier == 86400 ? (count == 1 ? "day" : "days") : (count == 1 ? "week" : "weeks")) · \(DateFormatter.localizedString(from: due!, dateStyle: .medium, timeStyle: .short))"
        } else { due = nil; preview.stringValue = "Use a duration such as 5m, 2h, 3d or 1w (up to one year)." }
        save.isEnabled = due != nil && !busy
    }
    private func label(_ text: String, strong: Bool = false) -> NSTextField {
        let field = NSTextField(wrappingLabelWithString: text); field.font = .systemFont(ofSize: 12, weight: strong ? .semibold : .regular); field.textColor = strong ? EmailTheme.ink : EmailTheme.muted
        field.widthAnchor.constraint(lessThanOrEqualToConstant: width - 45).isActive = true
        return field
    }
    private func reload() async {
        do {
            rows.arrangedSubviews.forEach { rows.removeArrangedSubview($0); $0.removeFromSuperview() }
            if calendar {
                let items = try await core.callAsync(["op":"list_calendar_invitations", "limit":100]) as? [[String:Any]] ?? []
                if items.isEmpty { rows.addArrangedSubview(label("No invitations saved yet. Select a meeting email and check its attachments.")) }
                for item in items {
                    let event = item["event"] as? [String:Any] ?? [:]; let start = event["start"] as? [String:Any] ?? [:]
                    rows.addArrangedSubview(label(event["summary"] as? String ?? "Untitled meeting", strong: true))
                    let zone = start["all_day"] as? Bool == true ? "All day" : (start["timezone"] as? String ?? "Floating time · timezone not specified")
                    rows.addArrangedSubview(label("\(start["display"] as? String ?? "Time unavailable") · \(zone)"))
                    var details: [String] = []
                    if let method = event["method"] as? String { details.append(method == "CANCEL" ? "Cancelled invitation" : method) }
                    if let status = event["status"] as? String { details.append(status) }
                    if let location = event["location"] as? String, !location.isEmpty { details.append(location) }
                    if !details.isEmpty { rows.addArrangedSubview(label(details.joined(separator: " · "))) }
                    for warning in event["warnings"] as? [String] ?? [] { rows.addArrangedSubview(label(warning)) }
                }
            } else {
                let items = try await core.callAsync(["op":"list_followups"]) as? [[String:Any]] ?? []
                if let item = items.first(where: { $0["thread_id"] as? String == threadID }) {
                    let date = Date(timeIntervalSince1970: ((item["due_at_ms"] as? NSNumber)?.doubleValue ?? 0) / 1000)
                    let state = item["status"] as? String ?? "waiting"
                    rows.addArrangedSubview(label("\(state.capitalized) · \(DateFormatter.localizedString(from: date, dateStyle: .medium, timeStyle: .short))", strong: true))
                    if state == "waiting" || state == "due" { rows.addArrangedSubview(NSButton(title: "Cancel reminder", target: self, action: #selector(cancelFollowup))) }
                } else { rows.addArrangedSubview(label("No reminder set for this conversation.")) }
            }
        } catch { notice.stringValue = error.localizedDescription }
    }
    @objc private func setFollowup() {
        guard let threadID, let due, !busy else { return }; busy = true; updateDuration(); notice.stringValue = "Saving…"
        let onlyIfNoReply = conditional.state == .on
        Task { do { _ = try await core.callAsync(["op":"set_followup","thread_id":threadID,"due_at_ms":Int64(due.timeIntervalSince1970 * 1000),"only_if_no_reply":onlyIfNoReply]); notice.stringValue = "Saved · find this conversation in Follow-ups (G then W)"; onChanged(); await reload() } catch { notice.stringValue = error.localizedDescription }; busy = false; updateDuration() }
    }
    @objc private func cancelFollowup() {
        guard let threadID else { return }
        Task { do { _ = try await core.callAsync(["op":"cancel_followup","thread_id":threadID]); notice.stringValue = "Reminder cancelled"; onChanged(); await reload() } catch { notice.stringValue = error.localizedDescription } }
    }
    @objc private func scanEmail() {
        guard let messageID, !busy else { return }; busy = true; notice.stringValue = "Checking calendar attachments…"
        Task { do { let items = try await core.callAsync(["op":"refresh_calendar_invitation","message_id":messageID]) as? [[String:Any]] ?? []; notice.stringValue = items.isEmpty ? "This email contains no supported calendar invitation." : "Invitation details saved · no RSVP sent"; await reload() } catch { notice.stringValue = "Could not check invitations: \(error.localizedDescription)" }; busy = false }
    }
    @objc private func openAgenda() {
        let parent = view.window?.parent ?? NSApp.mainWindow
        dismiss?()
        AgendaPanel.present(core: core, parent: parent)
    }
    @objc private func close() { dismiss?() }
}
private final class WorkflowSurface: NSView {
    override var isFlipped: Bool { true }
    override func draw(_ dirtyRect: NSRect) { EmailTheme.reader.setFill(); bounds.fill() }
}

private final class WorkflowDocument: NSView { override var isFlipped: Bool { true } }
