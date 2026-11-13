import AppKit

/// Tracking stays approximate; conversation context and follow-ups use downloaded mail.
@MainActor final class TrackingActivityView: NSViewController {
    private let core: CoreBridge
    private let initialMessageID: String?
    private var selectedMessageID: String?
    private var single: Bool { selectedMessageID != nil }
    private let headingLabel = NSTextField(labelWithString: "Recent opens")
    private let back = TrackingActionButton(title: "Back")
    private let filters = NSSegmentedControl(labels: ["All", "Awaiting reply", "Has activity"], trackingMode: .selectOne, target: nil, action: nil)
    private let rows = NSStackView()
    private let scroll = NSScrollView()
    private let status = NSTextField(wrappingLabelWithString: "")
    private let freshness = NSTextField(labelWithString: "Saved activity")
    private let spinner = NSProgressIndicator()
    private var generation = 0
    private var latest: [[String: Any]] = []
    private var detailItem: [String: Any]?
    private var refreshedAt: Double?
    private var tick: Timer?
    private var busy = false
    var dismiss: (() -> Void)?
    var openConversation: (([String: Any]) -> String?)?
    var onChanged: (() -> Void)?

    /// Whether this build offers Track opens (the profile's `OPEN_TRACKING`),
    /// which decides the empty-feed hint.
    private let offersTrackOpens: Bool

    init(core: CoreBridge, messageID: String?, offersTrackOpens: Bool = false) {
        self.core = core; initialMessageID = messageID; selectedMessageID = messageID
        self.offersTrackOpens = offersTrackOpens
        super.init(nibName: nil, bundle: nil)
    }
    required init?(coder: NSCoder) { fatalError() }

    override func loadView() {
        let root = TrackingSurface()
        let stack = NSStackView(); stack.orientation = .vertical; stack.alignment = .leading; stack.spacing = 10
        stack.translatesAutoresizingMaskIntoConstraints = false; root.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.topAnchor.constraint(equalTo: root.topAnchor, constant: 18),
            stack.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 20),
            stack.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -20),
            stack.bottomAnchor.constraint(equalTo: root.bottomAnchor, constant: -16),
        ])
        headingLabel.font = .systemFont(ofSize: 16, weight: .semibold); headingLabel.textColor = EmailTheme.ink
        back.image = NSImage(systemSymbolName: "chevron.left", accessibilityDescription: "Back to recent opens")
        back.imagePosition = .imageOnly; back.toolTip = "Back to recent opens (Esc)"
        back.actionBlock = { [weak self] in self?.showFeed() }; back.identifier = NSUserInterfaceItemIdentifier("activity-back")
        let refresh = TrackingActionButton(title: "Refresh")
        refresh.image = NSImage(systemSymbolName: "arrow.clockwise", accessibilityDescription: nil)
        refresh.imagePosition = .imageOnly; refresh.toolTip = "Check for new activity"; refresh.setAccessibilityLabel("Refresh activity")
        refresh.identifier = NSUserInterfaceItemIdentifier("activity-refresh")
        refresh.actionBlock = { [weak self] in self?.load(refresh: true) }
        spinner.style = .spinning; spinner.controlSize = .small; spinner.isDisplayedWhenStopped = false
        let info = NSButton(image: NSImage(systemSymbolName: "info.circle", accessibilityDescription: "About open tracking")!, target: nil, action: nil)
        info.isBordered = false
        info.toolTip = "Privacy protection and mail scanners can affect open tracking."
        info.setAccessibilityLabel("About open tracking"); info.setAccessibilityHelp(info.toolTip)
        let close = TrackingActionButton(title: "Close")
        close.image = NSImage(systemSymbolName: "xmark", accessibilityDescription: "Close recent opens")
        close.imagePosition = .imageOnly; close.toolTip = "Close recent opens (Esc)"
        close.actionBlock = { [weak self] in self?.dismiss?() }
        let header = NSStackView(views: [back, headingLabel, NSView(), info, spinner, refresh, close])
        header.orientation = .horizontal; header.alignment = .centerY; header.spacing = 8
        stack.addArrangedSubview(header); header.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true
        filters.selectedSegment = 0; filters.target = self; filters.action = #selector(filterChanged)
        filters.setAccessibilityLabel("Activity filter"); filters.identifier = NSUserInterfaceItemIdentifier("activity-filter")
        stack.addArrangedSubview(filters)
        filters.widthAnchor.constraint(lessThanOrEqualTo: stack.widthAnchor).isActive = true
        rows.orientation = .vertical; rows.alignment = .leading; rows.spacing = 10; rows.translatesAutoresizingMaskIntoConstraints = false
        let doc = TrackingDocument(); doc.translatesAutoresizingMaskIntoConstraints = false; doc.addSubview(rows)
        scroll.drawsBackground = false; scroll.hasVerticalScroller = true; scroll.autohidesScrollers = true
        scroll.documentView = doc; scroll.translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([
            doc.widthAnchor.constraint(equalTo: scroll.contentView.widthAnchor),
            rows.leadingAnchor.constraint(equalTo: doc.leadingAnchor), rows.trailingAnchor.constraint(equalTo: doc.trailingAnchor),
            rows.topAnchor.constraint(equalTo: doc.topAnchor), rows.bottomAnchor.constraint(equalTo: doc.bottomAnchor),
        ])
        stack.addArrangedSubview(scroll)
        scroll.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true
        scroll.heightAnchor.constraint(greaterThanOrEqualToConstant: 150).isActive = true
        freshness.font = .systemFont(ofSize: 11); freshness.textColor = EmailTheme.muted
        stack.addArrangedSubview(freshness)
        status.font = .systemFont(ofSize: 12); status.textColor = EmailTheme.muted; status.isHidden = true
        stack.addArrangedSubview(status); status.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true
        view = root; render()
    }

    override func viewDidAppear() {
        super.viewDidAppear(); load(refresh: true)
        tick?.invalidate()
        let timer = Timer(timeInterval: 30, repeats: true) { [weak self] _ in Task { @MainActor in self?.updateFreshness() } }
        RunLoop.main.add(timer, forMode: .common); tick = timer
    }
    override func viewWillDisappear() { super.viewWillDisappear(); tick?.invalidate(); tick = nil; generation += 1 }
    override func cancelOperation(_ sender: Any?) { if single && initialMessageID == nil { showFeed() } else { dismiss?() } }
    func focusActivity() {
        if let first = rows.arrangedSubviews.first(where: { $0 is TrackingActionButton }) { view.window?.makeFirstResponder(first) }
        else { view.window?.makeFirstResponder(refreshButton ?? filters) }
    }
    private var refreshButton: NSView? { descendant(view, identifier: NSUserInterfaceItemIdentifier("activity-refresh")) }
    func handleKeyboard(_ event: NSEvent) -> Bool {
        if event.keyCode == 53 { cancelOperation(nil); return true }
        guard event.modifierFlags.intersection([.command, .control, .option]).isEmpty else { return false }
        let buttons = rows.arrangedSubviews.compactMap { $0 as? TrackingActionButton }.filter { $0.isEnabled }
        let key = event.charactersIgnoringModifiers?.lowercased() ?? ""
        if event.keyCode == 125 || event.keyCode == 126 || key == "j" || key == "k" {
            guard !buttons.isEmpty else { return true }
            let forward = event.keyCode == 125 || key == "j"
            let current = buttons.firstIndex { $0 === view.window?.firstResponder }
            let index = current.map { min(buttons.count - 1, max(0, $0 + (forward ? 1 : -1))) } ?? (forward ? 0 : buttons.count - 1)
            view.window?.makeFirstResponder(buttons[index]); buttons[index].scrollToVisible(buttons[index].bounds)
            return true
        }
        if event.keyCode == 36, let button = view.window?.firstResponder as? TrackingActionButton { button.performClick(nil); return true }
        return false
    }
    @objc private func filterChanged() { render(); scrollToTop() }

    private func load(refresh: Bool) {
        generation += 1; let serial = generation; let selected = selectedMessageID
        if refresh { spinner.startAnimation(nil) }
        Task { [weak self] in
            guard let self else { return }
            var payload: [String: Any] = ["op": "tracking_activity", "refresh": false]
            if let selected { payload["message_id"] = selected }
            do {
                let cached = try await core.callAsync(payload) as? [String: Any] ?? [:]
                guard serial == generation else { return }
                accept(cached)
                if refresh {
                    payload["refresh"] = true
                    let fresh = try await core.callAsync(payload) as? [String: Any] ?? [:]
                    guard serial == generation else { return }
                    accept(fresh)
                }
                spinner.stopAnimation(nil); showStatus(nil)
            } catch {
                guard serial == generation else { return }
                spinner.stopAnimation(nil)
                showStatus(refresh ? "Couldn't check for new activity. Showing saved activity." : error.localizedDescription)
            }
        }
    }
    private func accept(_ data: [String: Any]) {
        let items = data["items"] as? [[String: Any]] ?? []
        if single { detailItem = items.first }
        else { latest = items.sorted { number($0, "last_activity_at_ms", fallback: "last_requested_at") > number($1, "last_activity_at_ms", fallback: "last_requested_at") } }
        refreshedAt = (data["refreshed_at_ms"] as? NSNumber)?.doubleValue
        render()
    }
    private func showFeed() {
        guard !busy else { return }
        generation += 1; selectedMessageID = nil; detailItem = nil; showStatus(nil); render(); scrollToTop(); load(refresh: false)
        view.window?.makeFirstResponder(filters)
    }
    private func showDetail(_ item: [String: Any]) {
        guard let id = item["message_id"] as? String, !busy else { return }
        selectedMessageID = id; detailItem = item; showStatus(nil); render(); scrollToTop(); load(refresh: true)
        view.window?.makeFirstResponder(back)
    }
    private func render() {
        let focused = (view.window?.firstResponder as? NSView)?.identifier
        let position = scroll.contentView.bounds.origin
        rows.arrangedSubviews.forEach { rows.removeArrangedSubview($0); $0.removeFromSuperview() }
        back.isHidden = !single || initialMessageID != nil; filters.isHidden = single
        headingLabel.stringValue = single ? "Open activity" : "Recent opens"
        if single { renderDetail() } else { renderFeed() }
        updateFreshness(); view.layoutSubtreeIfNeeded()
        scroll.contentView.scroll(to: position); scroll.reflectScrolledClipView(scroll.contentView)
        if let focused, let target = descendant(view, identifier: focused) { view.window?.makeFirstResponder(target) }
    }
    private func renderFeed() {
        let filtered = latest.filter { item in
            switch filters.selectedSegment {
            case 1: return !(item["reply_received"] as? Bool ?? false)
            case 2: return number(item, "pixel_requests") > 0 || (item["reply_received"] as? Bool ?? false)
            default: return true
            }
        }
        if filtered.isEmpty {
            add(text(latest.isEmpty ? (offersTrackOpens ? "No tracked messages yet. Enable Track opens in a draft before sending." : "No tracked messages.") : "No messages match this filter.", muted: true)); return
        }
        for (index, item) in filtered.prefix(200).enumerated() {
            if index > 0 { add(trackingSeparator()) }
            let subject = displaySubject(item)
            let button = TrackingActionButton(title: subject); button.isBordered = false; button.alignment = .left
            button.font = .systemFont(ofSize: 13, weight: .medium); button.contentTintColor = EmailTheme.ink
            button.lineBreakMode = .byTruncatingTail; button.toolTip = subject
            button.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
            button.setContentHuggingPriority(.defaultLow, for: .horizontal)
            button.identifier = NSUserInterfaceItemIdentifier("activity-message-\(item["message_id"] as? String ?? "")")
            button.actionBlock = { [weak self] in self?.showDetail(item) }; add(button)
            if let recipient = item["recipient"] as? String, !recipient.isEmpty { add(text(recipient, muted: true, size: 12)) }
            let count = Int(number(item, "pixel_requests"))
            let summary = (item["reply_received"] as? Bool ?? false) ? "Reply received" : "Awaiting reply"
            add(text("\(summary) · \(count == 0 ? "No opens recorded" : (count == 1 ? "Opened once" : "Opened \(count) times"))", muted: true, size: 12))
            if let timing = Self.timing(item) { add(text(timing, muted: true, size: 12)) }
            if let reminder = activeReminder(item) { add(text(reminderText(reminder), muted: true, size: 12)) }
        }
    }
    private func renderDetail() {
        guard let item = detailItem else { add(text("No tracked activity is available for this message.", muted: true)); return }
        let subject = text(displaySubject(item), size: 15, weight: .semibold); add(subject)
        if let recipient = item["recipient"] as? String, !recipient.isEmpty { add(text(recipient, muted: true, size: 12)) }
        let count = Int(number(item, "pixel_requests"))
        add(text(count == 0 ? "No opens recorded" : (count == 1 ? "Opened once" : "Opened \(count) times"), size: 20, weight: .semibold))
        add(text((item["reply_received"] as? Bool ?? false) ? "Reply received" : "No reply in downloaded mail", muted: true, size: 12))
        if item["tracking_status"] as? String == "expired" { add(text("Tracking expired. Saved activity remains available.", muted: true, size: 12)) }
        if openConversation != nil {
            let button = action("Open conversation", id: "activity-open") { [weak self] in self?.open(item) }; add(button)
        }
        if let reminder = activeReminder(item) {
            add(text(reminderText(reminder), muted: true, size: 12))
            add(action("Cancel reminder", id: "activity-cancel-reminder") { [weak self] in self?.mutateReminder(item, cancel: true) })
        } else if !(item["reply_received"] as? Bool ?? false), item["thread_id"] as? String != nil {
            add(action("Remind in 3 days if no reply", id: "activity-set-reminder") { [weak self] in self?.mutateReminder(item, cancel: false) })
        }
        add(trackingSeparator())
        let events = (item["timeline"] as? [[String: Any]] ?? []).sorted { number($0, "at_ms") > number($1, "at_ms") }
        for event in events {
            let kind = event["kind"] as? String ?? ""
            let label: String
            switch kind {
            case "sent": label = "Sent"
            case "reply_received": label = "Reply received"
            case "image_request":
                let requests = Int(number(event, "count")); label = requests > 1 ? "Opened \(requests) times" : "Opened"
            default: continue
            }
            let time = number(event, "at_ms")
            let line = text("\(label)\n\(Self.exact(time))", muted: false, size: 12)
            line.toolTip = Self.exact(time); add(line)
        }
        if item["timeline_complete"] as? Bool != true {
            add(text("Earlier opens may only have saved totals. The timeline can be incomplete.", muted: true, size: 11))
        }
    }
    private func open(_ item: [String: Any]) {
        guard !busy, let id = item["message_id"] as? String else { return }
        busy = true; render()
        Task { [weak self] in
            guard let self else { return }
            do {
                guard let message = try await core.callAsync(["op": "get_message", "id": id]) as? [String: Any],
                      message["thread_id"] as? String == item["thread_id"] as? String,
                      message["account_id"] as? String == item["account_id"] as? String else { throw MailServiceError(message: "This conversation is no longer in the local cache.") }
                if let error = openConversation?(item) { showStatus(error) }
            } catch { showStatus(error.localizedDescription) }
            busy = false; render()
        }
    }
    private func mutateReminder(_ item: [String: Any], cancel: Bool) {
        guard !busy, let thread = item["thread_id"] as? String else { return }
        busy = true; render()
        Task { [weak self] in
            guard let self else { return }
            do {
                var request: [String: Any] = ["op": cancel ? "cancel_followup" : "set_followup", "thread_id": thread]
                if !cancel { request["due_at_ms"] = Int64(Date().addingTimeInterval(3 * 86400).timeIntervalSince1970 * 1000); request["only_if_no_reply"] = true; request["sent_message_id"] = item["message_id"] }
                _ = try await core.callAsync(request); onChanged?(); busy = false; load(refresh: false)
                showStatus(cancel ? "Reminder cancelled." : "Reminder saved. A downloaded reply resolves it.")
            } catch {
                let message = error.localizedDescription
                busy = false
                var payload: [String: Any] = ["op": "tracking_activity", "refresh": false]
                if let selectedMessageID { payload["message_id"] = selectedMessageID }
                if let data = try? await core.callAsync(payload) as? [String: Any] { accept(data) }
                render(); showStatus(message)
            }
        }
    }
    private func activeReminder(_ item: [String: Any]) -> [String: Any]? {
        guard let reminder = item["followup"] as? [String: Any], ["waiting", "due"].contains(reminder["status"] as? String ?? "") else { return nil }; return reminder
    }
    private func reminderText(_ reminder: [String: Any]) -> String {
        let due = number(reminder, "due_at_ms")
        return "\(reminder["status"] as? String == "due" ? "Reminder due" : "Reminder set") · \(Self.exact(due))"
    }
    private func action(_ title: String, id: String, block: @escaping () -> Void) -> NSButton {
        let button = TrackingActionButton(title: title); button.identifier = NSUserInterfaceItemIdentifier(id); button.actionBlock = block; button.isEnabled = !busy; return button
    }
    private func add(_ child: NSView) {
        rows.addArrangedSubview(child); child.widthAnchor.constraint(equalTo: rows.widthAnchor).isActive = true
    }
    private func text(_ value: String, muted: Bool = false, size: CGFloat = 13, weight: NSFont.Weight = .regular) -> NSTextField {
        let label = NSTextField(wrappingLabelWithString: value); label.font = .systemFont(ofSize: size, weight: weight)
        label.textColor = muted ? EmailTheme.muted : EmailTheme.ink; label.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        return label
    }
    private func displaySubject(_ item: [String: Any]) -> String {
        let value = (item["subject"] as? String ?? "").trimmingCharacters(in: .whitespacesAndNewlines); return value.isEmpty ? "(No subject)" : value
    }
    private func updateFreshness() {
        freshness.stringValue = refreshedAt.map { "Last checked \(Self.relative($0))" } ?? "Saved activity · not checked yet"
        freshness.toolTip = refreshedAt.map(Self.exact)
    }
    private func showStatus(_ value: String?) { status.stringValue = value ?? ""; status.isHidden = value == nil }
    private func scrollToTop() { scroll.contentView.scroll(to: .zero); scroll.reflectScrolledClipView(scroll.contentView) }
    private func descendant(_ node: NSView, identifier: NSUserInterfaceItemIdentifier) -> NSView? {
        if node.identifier == identifier { return node }; return node.subviews.lazy.compactMap { self.descendant($0, identifier: identifier) }.first
    }
    private func number(_ item: [String: Any], _ key: String, fallback: String? = nil) -> Double {
        (item[key] as? NSNumber)?.doubleValue ?? fallback.flatMap { (item[$0] as? NSNumber)?.doubleValue } ?? 0
    }
    static func timing(_ item: [String: Any]?) -> String? {
        guard let last = (item?["last_requested_at"] as? NSNumber)?.doubleValue else { return nil }
        return "Last opened \(relative(last))"
    }
    static func exact(_ millis: Double) -> String {
        Date(timeIntervalSince1970: millis / 1000).formatted(date: .abbreviated, time: .standard)
    }
    static func relative(_ millis: Double) -> String {
        let date = Date(timeIntervalSince1970: millis / 1000)
        if abs(Date().timeIntervalSince(date)) < 60 { return "just now" }
        let formatter = RelativeDateTimeFormatter(); formatter.unitsStyle = .full
        return formatter.localizedString(for: date, relativeTo: Date())
    }
}

@MainActor private final class TrackingActionButton: NSButton {
    var actionBlock: (() -> Void)?
    init(title: String) {
        super.init(frame: .zero); self.title = title; bezelStyle = .rounded; target = self; action = #selector(invoke)
    }
    required init?(coder: NSCoder) { fatalError() }
    @objc private func invoke() { actionBlock?() }
}
private final class TrackingSurface: NSView {
    override var isFlipped: Bool { true }
    override func draw(_ dirtyRect: NSRect) { EmailTheme.reader.setFill(); bounds.fill(); EmailTheme.line.setFill(); NSRect(x: 0, y: 0, width: 1, height: bounds.height).fill() }
}
private final class TrackingDocument: NSView { override var isFlipped: Bool { true } }


@MainActor private func trackingSeparator() -> NSBox { let box = NSBox(); box.boxType = .separator; return box }
