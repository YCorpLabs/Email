import AppKit

/// A preview-first, resumable sender cleanup. Service owns durable progress.
@MainActor
final class SenderCleanupController: NSWindowController {
    private let core: CoreBridge
    private let stack = NSStackView()
    private let status = NSTextField(wrappingLabelWithString: "")
    private var expandedSamples = Set<String>()
    private var jobs: [[String: Any]] = []
    private var busy = false
    private var pendingControls: [(op: String, id: String)] = []
    private let accountNames: [String: String]
    private var confirming = false
    private var nextAttempt = Date.distantPast
    private var timer: Timer?
    private let onChange: () -> Void
    private let onStart: () -> Void

    init(core: CoreBridge, accounts: [MailAccount], onStart: @escaping () -> Void, onChange: @escaping () -> Void) {
        self.core = core; self.onChange = onChange; self.onStart = onStart
        self.accountNames = Dictionary(accounts.map { ($0.id, $0.email) }, uniquingKeysWith: { first, _ in first })
        let window = CleanupPanel(contentRect: NSRect(x: 0, y: 0, width: 520, height: 300), styleMask: [.borderless], backing: .buffered, defer: false)
        window.title = "Sender cleanup"; window.isOpaque = false; window.backgroundColor = .clear; window.hasShadow = true; window.isMovableByWindowBackground = true
        super.init(window: window)
        let root = CleanupSurface(rounded: true); window.contentView = root
        let heading = label("Clean up", size: 17, weight: .semibold)
        let subtitle = label("Mark unread messages from a sender as read", size: 12, muted: true)
        let close = CleanupActionButton(title: "✕") { [weak self] in self?.close() }
        close.isBordered = false; close.keyEquivalent = "\u{1b}"; close.toolTip = "Close (Esc)"; close.setAccessibilityLabel("Close cleanup")
        let divider = NSBox(); divider.boxType = .separator
        let scroll = NSScrollView(); scroll.hasVerticalScroller = true; scroll.drawsBackground = false
        stack.orientation = .vertical; stack.alignment = .leading; stack.spacing = 12
        stack.setContentHuggingPriority(.required, for: .vertical)
        let document = CleanupDocument(); document.translatesAutoresizingMaskIntoConstraints = false
        document.addSubview(stack); stack.translatesAutoresizingMaskIntoConstraints = false; scroll.documentView = document
        NSLayoutConstraint.activate([document.widthAnchor.constraint(equalTo: scroll.contentView.widthAnchor), stack.leadingAnchor.constraint(equalTo: document.leadingAnchor), stack.trailingAnchor.constraint(equalTo: document.trailingAnchor), stack.topAnchor.constraint(equalTo: document.topAnchor), stack.bottomAnchor.constraint(equalTo: document.bottomAnchor)])
        status.font = .systemFont(ofSize: 11); status.textColor = EmailTheme.muted; status.maximumNumberOfLines = 2
        let footer = NSStackView(); footer.spacing = 12; footer.alignment = .centerY
        footer.addArrangedSubview(label("Changes start only after your confirmation", size: 11, muted: true))
        let spacer = NSView(); spacer.setContentHuggingPriority(.defaultLow, for: .horizontal); footer.addArrangedSubview(spacer)
        let done = CleanupActionButton(title: "Done") { [weak self] in self?.close() }
        done.isBordered = false; done.wantsLayer = true; done.layer?.cornerRadius = 7
        done.layer?.backgroundColor = EmailTheme.cgColor(EmailTheme.accent, in: root.effectiveAppearance)
        done.contentTintColor = EmailTheme.reader; done.font = .systemFont(ofSize: 12, weight: .semibold)
        footer.addArrangedSubview(done)
        for view in [heading, subtitle, close, divider, scroll, status, footer] { view.translatesAutoresizingMaskIntoConstraints = false; root.addSubview(view) }
        NSLayoutConstraint.activate([
            heading.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 22), heading.topAnchor.constraint(equalTo: root.topAnchor, constant: 20),
            close.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -18), close.topAnchor.constraint(equalTo: root.topAnchor, constant: 16), close.widthAnchor.constraint(equalToConstant: 28), close.heightAnchor.constraint(equalToConstant: 28),
            subtitle.leadingAnchor.constraint(equalTo: heading.leadingAnchor), subtitle.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -22), subtitle.topAnchor.constraint(equalTo: heading.bottomAnchor, constant: 5),
            divider.leadingAnchor.constraint(equalTo: root.leadingAnchor), divider.trailingAnchor.constraint(equalTo: root.trailingAnchor), divider.topAnchor.constraint(equalTo: subtitle.bottomAnchor, constant: 18),
            scroll.leadingAnchor.constraint(equalTo: heading.leadingAnchor), scroll.trailingAnchor.constraint(equalTo: subtitle.trailingAnchor), scroll.topAnchor.constraint(equalTo: divider.bottomAnchor, constant: 16), scroll.bottomAnchor.constraint(equalTo: status.topAnchor, constant: -6),
            status.leadingAnchor.constraint(equalTo: heading.leadingAnchor), status.trailingAnchor.constraint(equalTo: subtitle.trailingAnchor), status.bottomAnchor.constraint(equalTo: footer.topAnchor, constant: -8),
            footer.leadingAnchor.constraint(equalTo: heading.leadingAnchor), footer.trailingAnchor.constraint(equalTo: subtitle.trailingAnchor), footer.bottomAnchor.constraint(equalTo: root.bottomAnchor, constant: -16), done.widthAnchor.constraint(equalToConstant: 66), done.heightAnchor.constraint(equalToConstant: 30)
        ])
        timer = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { [weak self] _ in Task { @MainActor in self?.tick() } }
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    func open(accountID: String? = nil, sender: String? = nil) {
        showWindow(nil); window?.center(); window?.makeKeyAndOrderFront(nil)
        status.stringValue = sender == nil ? "Loading cleanup history…" : "Finding unread messages from this sender…"
        guard !busy else { return }; busy = true
        Task { @MainActor in
            defer { finishOperation() }
            do {
                if let accountID, let sender { _ = try await core.callAsync(["op":"create_sender_cleanup", "account_id":accountID, "sender":sender]) }
                try await reload()
                status.stringValue = ""
            } catch { status.stringValue = error.localizedDescription }
        }
    }
    private func reload() async throws {
        jobs = try await core.callAsync(["op":"list_sender_cleanups"]) as? [[String:Any]] ?? []
        render()
    }
    private func tick() {
        guard window?.isVisible == true, !busy, !confirming, Date() >= nextAttempt else { return }
        let now = Date().timeIntervalSince1970 * 1000
        guard let job = jobs.first(where: { ["discovering", "running"].contains($0["state"] as? String ?? "") && (($0["retry_at_ms"] as? NSNumber)?.doubleValue ?? 0) <= now }), let id = job["id"] as? String else { return }
        operate("step_sender_cleanup", id: id)
    }
    private func operate(_ op: String, id: String) {
        if busy {
            guard op != "step_sender_cleanup" else { return }
            // User controls take precedence over automatic next batches.
            pendingControls.removeAll { $0.id == id }
            pendingControls.append((op, id))
            status.stringValue = "Request queued. The current batch may finish; your control runs next."
            return
        }
        busy = true; status.stringValue = "Working… · Pause and Cancel remain available"

        Task { @MainActor in
            defer { finishOperation() }
            do { _ = try await core.callAsync(["op":op, "job_id":id]); try await reload(); onChange(); status.stringValue = "" }
            catch { nextAttempt = Date().addingTimeInterval(5); status.stringValue = error.localizedDescription; try? await reload() }
        }
    }
    private func finishOperation() {
        busy = false
        if !pendingControls.isEmpty {
            let control = pendingControls.removeFirst()
            operate(control.op, id: control.id)
        }
    }
    private func render() {
        stack.arrangedSubviews.forEach { stack.removeArrangedSubview($0); $0.removeFromSuperview() }
        if jobs.isEmpty {
            stack.addArrangedSubview(label("No cleanup history", size: 14, weight: .semibold))
            stack.addArrangedSubview(label("Preview unread messages from the sender of your selected email. Nothing changes until you confirm the count.", size: 12, muted: true))
            let start = CleanupActionButton(title: "Clean up selected sender…") { [weak self] in self?.close(); self?.onStart() }
            start.keyEquivalent = "\r"
            stack.addArrangedSubview(start)
        }
        for job in jobs {
            guard let id = job["id"] as? String else { continue }
            let state = job["state"] as? String ?? "unknown"
            let sender = job["sender"] as? String ?? ""
            let accountID = job["account_id"] as? String ?? ""
            let account = accountNames[accountID] ?? accountID
            let matched = (job["matched"] as? NSNumber)?.intValue ?? 0
            let completed = (job["completed"] as? NSNumber)?.intValue ?? 0
            let skipped = (job["skipped"] as? NSNumber)?.intValue ?? 0
            let card = CleanupSurface(rounded: false)
            let row = NSStackView(); row.orientation = .vertical; row.alignment = .leading; row.spacing = 8; row.translatesAutoresizingMaskIntoConstraints = false
            card.addSubview(row)
            NSLayoutConstraint.activate([row.leadingAnchor.constraint(equalTo: card.leadingAnchor), row.trailingAnchor.constraint(equalTo: card.trailingAnchor), row.topAnchor.constraint(equalTo: card.topAnchor), row.bottomAnchor.constraint(equalTo: card.bottomAnchor)])
            let identity = NSStackView(); identity.alignment = .centerY; identity.spacing = 12; identity.distribution = .fill
            let icon = NSImageView(image: NSImage(systemSymbolName: "envelope", accessibilityDescription: nil)!)
            icon.contentTintColor = EmailTheme.muted; identity.addArrangedSubview(icon); icon.widthAnchor.constraint(equalToConstant: 28).isActive = true; icon.heightAnchor.constraint(equalToConstant: 28).isActive = true
            let names = NSStackView(); names.orientation = .vertical; names.alignment = .leading; names.spacing = 4
            let title = label(sender, size: 13, weight: .semibold); title.lineBreakMode = .byTruncatingMiddle; title.maximumNumberOfLines = 1
            names.addArrangedSubview(title); names.addArrangedSubview(label(account, size: 11, muted: true)); names.setContentHuggingPriority(.defaultLow, for: .horizontal); identity.addArrangedSubview(names)
            identity.addArrangedSubview(label(state == "discovering" ? "Finding matches…" : state.capitalized, size: 11, muted: true))
            row.addArrangedSubview(identity); identity.widthAnchor.constraint(equalTo: row.widthAnchor).isActive = true
            let summary = state == "cancelled" && completed == 0 ? "No messages changed" : state == "ready" ? (matched == 0 ? "No unread messages found" : "Ready to review") : "\(completed.formatted()) marked read · \(skipped.formatted()) skipped"
            row.addArrangedSubview(label(state == "discovering" ? "Finding unread messages…" : "\(matched.formatted()) matches  ·  \(summary)", size: 12, muted: true))
            if state == "running" && matched > 0 {
                let progress = NSProgressIndicator(); progress.isIndeterminate = false; progress.minValue = 0; progress.maxValue = Double(matched); progress.doubleValue = Double(completed + skipped)
                row.addArrangedSubview(progress); progress.widthAnchor.constraint(equalTo: row.widthAnchor).isActive = true
            }
            if let samples = job["sample_subjects"] as? [String], !samples.isEmpty {
                let expanded = expandedSamples.contains(id)
                let toggle = CleanupActionButton(title: expanded ? "⌄  Sample messages" : "›  Sample messages") { [weak self] in
                    guard let self else { return }
                    if !self.expandedSamples.insert(id).inserted { self.expandedSamples.remove(id) }; self.render()
                }
                toggle.isBordered = false; toggle.font = .systemFont(ofSize: 12); toggle.contentTintColor = EmailTheme.ink; toggle.alignment = .left; row.addArrangedSubview(toggle)
                if expanded {
                    row.addArrangedSubview(label("Downloaded examples", size: 11, muted: true))
                    for subject in samples.prefix(3) { row.addArrangedSubview(label(subject, size: 12, muted: true)) }
                }
            }
            if let error = job["last_error"] as? String, !error.isEmpty { row.addArrangedSubview(label(error, size: 12, muted: true)) }
            let actions = NSStackView(); actions.spacing = 8
            func add(_ title: String, _ op: String) { let button = CleanupActionButton(title: title) { [weak self] in self?.operate(op, id: id) }; actions.addArrangedSubview(button) }
            if state == "ready" && matched > 0 {
                let confirm = CleanupActionButton(title: "Review and mark \(matched.formatted()) read…") { [weak self] in
                    guard let self else { return }
                    self.confirming = true
                    defer { self.confirming = false }
                    let alert = NSAlert(); alert.messageText = "Mark \(matched.formatted()) Gmail messages as read?"; alert.informativeText = "Account: \(account)\nExact sender: \(sender)\nOnly messages found during the preview. New arrivals are excluded. Completed changes have no automatic Undo."; alert.addButton(withTitle: "Mark read"); alert.addButton(withTitle: "Cancel")
                    if alert.runModal() == .alertFirstButtonReturn { self.operate("confirm_sender_cleanup", id: id) }
                }; confirm.toolTip = "Review the matched count and mark as read (Return)"
                if jobs.first(where: { ($0["state"] as? String) == "ready" })?["id"] as? String == id { confirm.keyEquivalent = "\r" }
                actions.addArrangedSubview(confirm)
            }
            if state == "discovering" || state == "running" { add("Pause", "pause_sender_cleanup") }
            if state == "paused" { add("Resume", "resume_sender_cleanup") }
            if !["completed", "cancelled"].contains(state) { add("Cancel job", "cancel_sender_cleanup") }
            if !actions.arrangedSubviews.isEmpty { row.addArrangedSubview(actions) }
            let divider = NSBox(); divider.boxType = .separator
            if !stack.arrangedSubviews.isEmpty { stack.addArrangedSubview(divider); divider.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true }
            stack.addArrangedSubview(card)
            card.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true
        }
    }
    private func label(_ text: String, size: CGFloat = 13, weight: NSFont.Weight = .regular, muted: Bool = false) -> NSTextField {
        let field = NSTextField(wrappingLabelWithString: text)
        field.font = .systemFont(ofSize: size, weight: weight); field.textColor = muted ? EmailTheme.muted : EmailTheme.ink
        field.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        return field
    }

}

@MainActor
private final class CleanupActionButton: NSButton {
    private let handler: () -> Void
    init(title: String, handler: @escaping () -> Void) { self.handler = handler; super.init(frame: .zero); self.title = title; bezelStyle = .rounded; target = self; action = #selector(run) }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    @objc private func run() { handler() }
}

// Flipped document coordinates keep the first card at the top, even when the
// scroll viewport is taller than its content. Plain NSView starts at the bottom.
private final class CleanupDocument: NSView { override var isFlipped: Bool { true } }
private final class CleanupSurface: NSView {
    let rounded: Bool
    init(rounded: Bool) { self.rounded = rounded; super.init(frame: .zero) }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    override var isFlipped: Bool { true }
    override func viewDidChangeEffectiveAppearance() { super.viewDidChangeEffectiveAppearance(); needsDisplay = true }
    override func draw(_ dirtyRect: NSRect) {
        let path = NSBezierPath(roundedRect: bounds.insetBy(dx: 0.5, dy: 0.5), xRadius: rounded ? 12 : 0, yRadius: rounded ? 12 : 0)
        EmailTheme.reader.setFill(); path.fill()
        if rounded { EmailTheme.line.setStroke(); path.lineWidth = 1; path.stroke() }
    }
}

private final class CleanupPanel: NSPanel { override var canBecomeKey: Bool { true } }
