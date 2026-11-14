import AppKit

/// Agent requests in the main window: the list rows and the reader. Requests
/// come from mail-service (`agent_requests`), which also writes their titles,
/// previews and button verbs, so macOS and Linux show the same words.
@MainActor
enum AgentRequestRows {
    /// "20:24" today, otherwise a short date.
    static func time(_ ms: Double?) -> String {
        guard let ms else { return "" }
        let date = Date(timeIntervalSince1970: ms / 1000)
        return Calendar.current.isDateInToday(date)
            ? date.formatted(date: .omitted, time: .shortened)
            : date.formatted(.dateTime.day().month(.abbreviated))
    }

    private static func label(_ text: String, size: CGFloat, weight: NSFont.Weight, color: NSColor) -> NSTextField {
        let label = NSTextField(labelWithString: text)
        label.font = .systemFont(ofSize: size, weight: weight)
        label.textColor = color
        label.maximumNumberOfLines = 1
        label.lineBreakMode = .byTruncatingTail
        label.usesSingleLineMode = true
        label.translatesAutoresizingMaskIntoConstraints = false
        return label
    }

    /// One row: the agent and the time, the title, and one preview line.
    static func cell(_ request: [String: Any]) -> NSTableCellView {
        let cell = NSTableCellView()
        let agent = label(request["agent"] as? String ?? "An agent", size: 12, weight: .semibold, color: EmailTheme.ink.withAlphaComponent(0.85))
        let time = label(Self.time((request["created_at_ms"] as? NSNumber)?.doubleValue), size: 10, weight: .regular, color: EmailTheme.muted)
        let title = label(request["title"] as? String ?? "", size: 12, weight: .semibold, color: EmailTheme.ink)
        let preview = label(request["preview"] as? String ?? "", size: 11, weight: .regular, color: EmailTheme.muted)
        agent.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        time.setContentCompressionResistancePriority(.required, for: .horizontal)
        for view in [agent, time, title, preview] { cell.addSubview(view) }
        NSLayoutConstraint.activate([
            agent.leadingAnchor.constraint(equalTo: cell.leadingAnchor, constant: 20), agent.topAnchor.constraint(equalTo: cell.topAnchor, constant: 6),
            time.trailingAnchor.constraint(equalTo: cell.trailingAnchor, constant: -18), time.centerYAnchor.constraint(equalTo: agent.centerYAnchor),
            agent.trailingAnchor.constraint(lessThanOrEqualTo: time.leadingAnchor, constant: -8),
            title.leadingAnchor.constraint(equalTo: agent.leadingAnchor), title.trailingAnchor.constraint(equalTo: time.trailingAnchor), title.topAnchor.constraint(equalTo: agent.bottomAnchor, constant: 2),
            preview.leadingAnchor.constraint(equalTo: agent.leadingAnchor), preview.trailingAnchor.constraint(equalTo: time.trailingAnchor), preview.topAnchor.constraint(equalTo: title.bottomAnchor, constant: 2),
        ])
        cell.setAccessibilityLabel("\(request["agent"] as? String ?? ""): \(request["title"] as? String ?? "")")
        return cell
    }
}

/// The reader for one request: a strip saying what the agent wants with the
/// buttons, then the email exactly as it will go out (or the conversations a
/// bulk change touches, or the account a refused agent wants).
@MainActor
final class AgentRequestReader: NSView {
    /// The composer's undo window; approved agent sends get the same one.
    static let undoSeconds = 10
    var onApprove: (([String: Any]) -> Void)?
    var onReject: (([String: Any]) -> Void)?
    var onEdit: (([String: Any]) -> Void)?
    var onSaveAttachment: ((_ requestID: String, _ attachmentID: String, _ filename: String) -> Void)?
    private(set) var request: [String: Any]?
    private let strip = NSView()
    private let ask = NSTextField(labelWithString: "")
    private let buttons = NSStackView()
    private let scroll = NSScrollView()
    private let content = NSStackView()
    private let keys = ShortcutStrip()
    private let empty = NSTextField(labelWithString: "Nothing waiting")
    private let undoBar = NSView()
    private let undoText = NSTextField(labelWithString: "")
    private let undoAction = AgentLinkButton(title: "Undo") {}
    private var undoTimer: Timer?
    private(set) var approveButton: AgentButton?
    private var readyTimer: Timer?
    /// A decision is accepted only a moment after a request appears, so a
    /// double press can't approve the next one by accident.
    private(set) var approveReady = false

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        strip.wantsLayer = true
        ask.font = .systemFont(ofSize: 13.5)
        ask.lineBreakMode = .byTruncatingTail
        ask.maximumNumberOfLines = 2
        ask.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        buttons.orientation = .horizontal
        buttons.spacing = 8
        buttons.alignment = .centerY
        buttons.setContentCompressionResistancePriority(.required, for: .horizontal)
        content.orientation = .vertical
        content.alignment = .leading
        content.spacing = 0
        content.translatesAutoresizingMaskIntoConstraints = false
        let document = AgentFlippedView()
        document.translatesAutoresizingMaskIntoConstraints = false
        document.addSubview(content)
        scroll.documentView = document
        scroll.drawsBackground = false
        scroll.hasVerticalScroller = true
        scroll.autohidesScrollers = true
        empty.font = .systemFont(ofSize: 15, weight: .medium)
        empty.textColor = EmailTheme.muted
        for view in [strip, scroll, keys, empty, ask, buttons] as [NSView] { view.translatesAutoresizingMaskIntoConstraints = false }
        strip.addSubview(ask)
        strip.addSubview(buttons)
        for view in [strip, scroll, keys, empty] as [NSView] { addSubview(view) }
        let line = AgentLine()
        line.translatesAutoresizingMaskIntoConstraints = false
        strip.addSubview(line)
        NSLayoutConstraint.activate([
            strip.topAnchor.constraint(equalTo: topAnchor), strip.leadingAnchor.constraint(equalTo: leadingAnchor), strip.trailingAnchor.constraint(equalTo: trailingAnchor),
            ask.leadingAnchor.constraint(equalTo: strip.leadingAnchor, constant: 28), ask.centerYAnchor.constraint(equalTo: strip.centerYAnchor),
            buttons.leadingAnchor.constraint(greaterThanOrEqualTo: ask.trailingAnchor, constant: 12),
            buttons.trailingAnchor.constraint(equalTo: strip.trailingAnchor, constant: -28), buttons.centerYAnchor.constraint(equalTo: strip.centerYAnchor),
            strip.heightAnchor.constraint(greaterThanOrEqualToConstant: 60),
            { let height = strip.heightAnchor.constraint(equalToConstant: 64); height.priority = .defaultHigh; return height }(),
            ask.topAnchor.constraint(greaterThanOrEqualTo: strip.topAnchor, constant: 12),
            line.leadingAnchor.constraint(equalTo: strip.leadingAnchor), line.trailingAnchor.constraint(equalTo: strip.trailingAnchor), line.bottomAnchor.constraint(equalTo: strip.bottomAnchor), line.heightAnchor.constraint(equalToConstant: 1),
            scroll.topAnchor.constraint(equalTo: strip.bottomAnchor), scroll.leadingAnchor.constraint(equalTo: leadingAnchor), scroll.trailingAnchor.constraint(equalTo: trailingAnchor), scroll.bottomAnchor.constraint(equalTo: keys.topAnchor, constant: -4),
            document.widthAnchor.constraint(equalTo: scroll.contentView.widthAnchor),
            content.leadingAnchor.constraint(equalTo: document.leadingAnchor, constant: 28), content.trailingAnchor.constraint(equalTo: document.trailingAnchor, constant: -28),
            content.topAnchor.constraint(equalTo: document.topAnchor, constant: 26), content.bottomAnchor.constraint(equalTo: document.bottomAnchor, constant: -26),
            keys.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 28), keys.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -28), keys.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -4), keys.heightAnchor.constraint(equalToConstant: 32),
            empty.centerXAnchor.constraint(equalTo: centerXAnchor), empty.centerYAnchor.constraint(equalTo: centerYAnchor, constant: -10),
        ])
        undoBar.wantsLayer = true
        undoBar.layer?.cornerRadius = 9
        undoBar.isHidden = true
        undoBar.translatesAutoresizingMaskIntoConstraints = false
        undoText.font = .systemFont(ofSize: 12.5)
        undoText.textColor = EmailTheme.ink
        let undoRow = NSStackView(views: [undoText, undoAction])
        undoRow.spacing = 10
        undoRow.alignment = .centerY
        undoRow.translatesAutoresizingMaskIntoConstraints = false
        undoBar.addSubview(undoRow)
        addSubview(undoBar)
        NSLayoutConstraint.activate([
            undoRow.leadingAnchor.constraint(equalTo: undoBar.leadingAnchor, constant: 14), undoRow.trailingAnchor.constraint(equalTo: undoBar.trailingAnchor, constant: -14),
            undoRow.topAnchor.constraint(equalTo: undoBar.topAnchor, constant: 8), undoRow.bottomAnchor.constraint(equalTo: undoBar.bottomAnchor, constant: -8),
            undoBar.centerXAnchor.constraint(equalTo: centerXAnchor), undoBar.bottomAnchor.constraint(equalTo: keys.topAnchor, constant: -12),
        ])
        show(nil)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        paint()
    }

    /// After a decision: what happened and its undo, until `until` passes.
    func showUndo(_ text: String, action: String, until: Date?, handler: @escaping () -> Void) {
        undoTimer?.invalidate()
        undoText.stringValue = text
        undoAction.title = action
        undoAction.attributedTitle = NSAttributedString(string: action, attributes: [.font: NSFont.systemFont(ofSize: 12.5, weight: .semibold), .foregroundColor: NSColor.linkColor])
        undoAction.handler = handler
        undoBar.isHidden = false
        guard let until else { return }
        let timer = Timer(fire: until, interval: 0, repeats: false) { [weak self] _ in
            MainActor.assumeIsolated { self?.hideUndo() }
        }
        RunLoop.main.add(timer, forMode: .common)
        undoTimer = timer
    }

    func hideUndo() {
        undoTimer?.invalidate()
        undoBar.isHidden = true
    }

    var undoVisible: Bool { !undoBar.isHidden }

    private func paint() {
        layer?.backgroundColor = EmailTheme.cgColor(EmailTheme.reader, in: effectiveAppearance)
        strip.layer?.backgroundColor = EmailTheme.cgColor(EmailTheme.inbox, in: effectiveAppearance)
        undoBar.layer?.backgroundColor = EmailTheme.cgColor(EmailTheme.accentSoft, in: effectiveAppearance)
    }

    /// Show a request, or the empty state. The same request again keeps the
    /// scroll position and the buttons' readiness.
    func show(_ request: [String: Any]?) {
        paint()
        let sameRequest = (request?["id"] as? String) != nil && (request?["id"] as? String) == (self.request?["id"] as? String)
        self.request = request
        guard let request else {
            [strip, scroll, keys].forEach { $0.isHidden = true }
            empty.isHidden = false
            approveButton = nil
            return
        }
        [strip, scroll, keys].forEach { $0.isHidden = false }
        empty.isHidden = true
        let agent = request["agent"] as? String ?? "An agent"
        let text = NSMutableAttributedString(string: agent, attributes: [.font: NSFont.systemFont(ofSize: 13.5, weight: .semibold), .foregroundColor: EmailTheme.ink])
        text.append(NSAttributedString(string: " " + (request["ask"] as? String ?? ""), attributes: [.font: NSFont.systemFont(ofSize: 13.5), .foregroundColor: EmailTheme.muted]))
        ask.attributedStringValue = text
        buildButtons(request)
        buildContent(request)
        let approve = request["approve_label"] as? String ?? "Approve"
        let reject = request["reject_label"] as? String ?? "Reject"
        var items: [(String, String)] = [("⌘↩", approve)]
        if request["can_edit"] as? Bool == true { items.append(("E", "Edit")) }
        items += [("⌫", reject), ("J K", "Next request")]
        keys.items = items
        if !sameRequest {
            scroll.contentView.scroll(to: .zero)
            scroll.reflectScrolledClipView(scroll.contentView)
            setReady(false)
            readyTimer?.invalidate()
            let timer = Timer(timeInterval: 0.8, repeats: false) { [weak self] _ in
                MainActor.assumeIsolated { self?.setReady(true) }
            }
            RunLoop.main.add(timer, forMode: .common)
            readyTimer = timer
        } else {
            setReady(approveReady)
        }
    }

    /// Dimmed, not disabled, so its label stays readable; clicks and ⌘↩ wait
    /// for `approveReady`.
    private func setReady(_ ready: Bool) {
        approveReady = ready
        approveButton?.alphaValue = ready ? 1 : 0.6
    }

    private func buildButtons(_ request: [String: Any]) {
        buttons.arrangedSubviews.forEach { buttons.removeArrangedSubview($0); $0.removeFromSuperview() }
        let reject = AgentButton(title: request["reject_label"] as? String ?? "Reject") { [weak self] in
            guard let self, let request = self.request else { return }
            self.onReject?(request)
        }
        reject.applyGhostStyle()
        buttons.addArrangedSubview(reject)
        if request["can_edit"] as? Bool == true {
            let edit = AgentButton(title: "Edit") { [weak self] in
                guard let self, let request = self.request else { return }
                self.onEdit?(request)
            }
            edit.applySecondaryStyle()
            buttons.addArrangedSubview(edit)
        }
        let approveTitle = request["approve_label"] as? String ?? "Approve"
        let approve = AgentButton(title: approveTitle) { [weak self] in
            guard let self, let request = self.request, self.approveReady else { return }
            self.onApprove?(request)
        }
        approve.applyPrimaryStyle(hint: request["type"] as? String == "access" ? nil : "⌘↩")
        approve.setAccessibilityLabel("\(approveTitle), Command Return")
        buttons.addArrangedSubview(approve)
        approveButton = approve
    }

    private func buildContent(_ request: [String: Any]) {
        content.arrangedSubviews.forEach { content.removeArrangedSubview($0); $0.removeFromSuperview() }
        func add(_ view: NSView, spacing: CGFloat = 0, fill: Bool = true) {
            if let last = content.arrangedSubviews.last { content.setCustomSpacing(spacing, after: last) }
            content.addArrangedSubview(view)
            if fill { view.widthAnchor.constraint(equalTo: content.widthAnchor).isActive = true }
        }
        let details = request["details"] as? [String: Any] ?? [:]
        switch request["type"] as? String {
        case "send":
            let draft = details["draft"] as? [String: Any] ?? [:]
            let subject = (draft["subject"] as? String).flatMap { $0.isEmpty ? nil : $0 } ?? "(no subject)"
            add(Self.text(subject, size: 22, weight: .bold))
            let header = NSGridView()
            header.rowSpacing = 5
            header.columnSpacing = 10
            var rows: [(String, String)] = [("From", draft["from"] as? String ?? "")]
            for (name, key) in [("To", "to"), ("Cc", "cc"), ("Bcc", "bcc"), ("Reply-To", "reply_to")] {
                let value = Self.addresses(details, key)
                if !value.isEmpty || key == "to" { rows.append((name, value)) }
            }
            if request["kind"] as? String == "schedule_draft" {
                let at = (details["send_at_ms"] as? NSNumber)?.doubleValue ?? 0
                rows.append(("When", Date(timeIntervalSince1970: at / 1000).formatted(date: .abbreviated, time: .shortened)))
            }
            for (name, value) in rows {
                let key = Self.text(name, size: 13, color: EmailTheme.muted)
                let field = Self.text(value, size: 13)
                field.isSelectable = true
                header.addRow(with: [key, field])
            }
            header.column(at: 0).width = 58
            header.column(at: 0).xPlacement = .leading
            add(header, spacing: 14)
            let invisible = (details["invisible_count"] as? NSNumber)?.intValue ?? 0
            if invisible > 0 {
                add(Self.text("Contains \(invisible) invisible character\(invisible == 1 ? "" : "s") used by emoji or some scripts.", size: 11.5, color: EmailTheme.muted), spacing: 10)
            }
            let body = draft["body_plain"] as? String ?? ""
            let bodyText = Self.text(body.isEmpty ? "(empty message)" : body, size: 15)
            bodyText.isSelectable = true
            add(bodyText, spacing: 20)
            let files = draft["attachments"] as? [[String: Any]] ?? []
            if !files.isEmpty {
                let chips = NSStackView()
                chips.orientation = .horizontal
                chips.spacing = 8
                for file in files {
                    let filename = file["filename"] as? String ?? "file"
                    let attachmentID = file["id"] as? String ?? ""
                    let requestID = request["id"] as? String ?? ""
                    let chip = AgentAttachmentChip(filename: filename, size: ByteCountFormatter.string(fromByteCount: (file["size"] as? NSNumber)?.int64Value ?? 0, countStyle: .file)) { [weak self] in
                        self?.onSaveAttachment?(requestID, attachmentID, filename)
                    }
                    chips.addArrangedSubview(chip)
                }
                add(chips, spacing: 18, fill: false)
            }
        case "access":
            add(Self.text(request["preview"] as? String ?? "", size: 13.5, color: EmailTheme.muted))
        default:
            let card = AgentRowsCard()
            for thread in request["threads"] as? [[String: Any]] ?? [] {
                card.addRow(sender: Self.senderName(thread["sender"] as? String ?? ""),
                            time: (thread["last_message_at"] as? NSNumber).map { AgentRequestRows.time($0.doubleValue) } ?? "",
                            subject: (thread["subject"] as? String).flatMap { $0.isEmpty ? nil : $0 } ?? "(no subject)",
                            snippet: thread["snippet"] as? String ?? "")
            }
            let count = (details["count"] as? NSNumber)?.intValue ?? 0
            let shown = (request["threads"] as? [[String: Any]] ?? []).count
            add(card)
            if count > shown { add(Self.text("and \(count - shown) more", size: 12.5, color: EmailTheme.muted), spacing: 10) }
            if let note = request["note"] as? String { add(Self.text(note, size: 12.5, color: EmailTheme.muted), spacing: 12) }
        }
    }

    static func text(_ value: String, size: CGFloat, weight: NSFont.Weight = .regular, color: NSColor = EmailTheme.ink) -> NSTextField {
        let field = NSTextField(wrappingLabelWithString: value)
        field.font = .systemFont(ofSize: size, weight: weight)
        field.textColor = color
        field.isSelectable = false
        field.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        return field
    }

    /// "Name <address>" when the agent gave a name, so the real address is
    /// always visible next to it.
    static func addresses(_ details: [String: Any], _ key: String) -> String {
        guard let parts = (details["addresses"] as? [String: Any])?[key] as? [[String: Any]] else {
            return ((details["draft"] as? [String: Any])?[key] as? [String] ?? []).joined(separator: ", ")
        }
        return parts.map { part in
            let address = part["address"] as? String ?? ""
            if let name = part["name"] as? String, !name.isEmpty { return "\(name) <\(address)>" }
            return address
        }.joined(separator: ", ")
    }

    static func senderName(_ sender: String) -> String {
        guard let open = sender.firstIndex(of: "<") else { return sender }
        let name = sender[..<open].trimmingCharacters(in: .whitespaces).trimmingCharacters(in: CharacterSet(charactersIn: "\""))
        return name.isEmpty ? sender : name
    }

    override func layout() {
        super.layout()
        for case let field as NSTextField in content.arrangedSubviews {
            field.preferredMaxLayoutWidth = content.bounds.width
        }
    }
}

/// Inbox-style rows inside a rounded card, for the conversations a bulk change
/// would touch.
@MainActor
private final class AgentRowsCard: NSView {
    private let stack = NSStackView()
    override init(frame: NSRect) {
        super.init(frame: frame)
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 0
        stack.translatesAutoresizingMaskIntoConstraints = false
        addSubview(stack)
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: leadingAnchor), stack.trailingAnchor.constraint(equalTo: trailingAnchor),
            stack.topAnchor.constraint(equalTo: topAnchor), stack.bottomAnchor.constraint(equalTo: bottomAnchor),
        ])
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    override var isFlipped: Bool { true }
    override func viewDidChangeEffectiveAppearance() { super.viewDidChangeEffectiveAppearance(); needsDisplay = true }
    override func draw(_ dirtyRect: NSRect) {
        let path = NSBezierPath(roundedRect: bounds.insetBy(dx: 0.5, dy: 0.5), xRadius: 9, yRadius: 9)
        EmailTheme.inbox.setFill()
        path.fill()
        EmailTheme.line.setStroke()
        path.stroke()
    }

    func addRow(sender: String, time: String, subject: String, snippet: String) {
        if !stack.arrangedSubviews.isEmpty {
            let line = AgentLine()
            stack.addArrangedSubview(line)
            line.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true
            line.heightAnchor.constraint(equalToConstant: 1).isActive = true
        }
        let row = NSView()
        func label(_ text: String, size: CGFloat, weight: NSFont.Weight, color: NSColor) -> NSTextField {
            let label = NSTextField(labelWithString: text)
            label.font = .systemFont(ofSize: size, weight: weight)
            label.textColor = color
            label.lineBreakMode = .byTruncatingTail
            label.maximumNumberOfLines = 1
            label.translatesAutoresizingMaskIntoConstraints = false
            label.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
            row.addSubview(label)
            return label
        }
        let who = label(sender, size: 12.5, weight: .semibold, color: EmailTheme.ink)
        let when = label(time, size: 11, weight: .regular, color: EmailTheme.muted)
        when.setContentCompressionResistancePriority(.required, for: .horizontal)
        let what = label(subject, size: 13, weight: .medium, color: EmailTheme.ink)
        let preview = label(snippet, size: 12, weight: .regular, color: EmailTheme.muted)
        NSLayoutConstraint.activate([
            who.leadingAnchor.constraint(equalTo: row.leadingAnchor, constant: 16), who.topAnchor.constraint(equalTo: row.topAnchor, constant: 10),
            when.trailingAnchor.constraint(equalTo: row.trailingAnchor, constant: -16), when.centerYAnchor.constraint(equalTo: who.centerYAnchor),
            who.trailingAnchor.constraint(lessThanOrEqualTo: when.leadingAnchor, constant: -8),
            what.leadingAnchor.constraint(equalTo: who.leadingAnchor), what.trailingAnchor.constraint(equalTo: when.trailingAnchor), what.topAnchor.constraint(equalTo: who.bottomAnchor, constant: 2),
            preview.leadingAnchor.constraint(equalTo: who.leadingAnchor), preview.trailingAnchor.constraint(equalTo: when.trailingAnchor), preview.topAnchor.constraint(equalTo: what.bottomAnchor, constant: 1),
            preview.bottomAnchor.constraint(equalTo: row.bottomAnchor, constant: -10),
        ])
        stack.addArrangedSubview(row)
        row.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true
    }
}

/// An attachment as it will go out; clicking saves a copy to look at first.
@MainActor
private final class AgentAttachmentChip: NSButton {
    private let handler: () -> Void
    init(filename: String, size: String, handler: @escaping () -> Void) {
        self.handler = handler
        super.init(frame: .zero)
        isBordered = false
        title = ""
        target = self
        action = #selector(run)
        wantsLayer = true
        layer?.cornerRadius = 8
        layer?.borderWidth = 1
        toolTip = "Save a copy"
        setAccessibilityLabel("Save a copy of \(filename)")
        let icon = NSImageView(image: NSImage(systemSymbolName: "doc.fill", accessibilityDescription: nil) ?? NSImage())
        icon.contentTintColor = .systemRed
        icon.symbolConfiguration = .init(pointSize: 18, weight: .regular)
        let name = NSTextField(labelWithString: filename)
        name.font = .systemFont(ofSize: 12.5)
        name.textColor = EmailTheme.ink
        name.lineBreakMode = .byTruncatingMiddle
        let meta = NSTextField(labelWithString: size)
        meta.font = .systemFont(ofSize: 11.5)
        meta.textColor = EmailTheme.muted
        let text = NSStackView(views: [name, meta])
        text.orientation = .vertical
        text.alignment = .leading
        text.spacing = 1
        let row = NSStackView(views: [icon, text])
        row.spacing = 8
        row.alignment = .centerY
        row.translatesAutoresizingMaskIntoConstraints = false
        addSubview(row)
        NSLayoutConstraint.activate([
            row.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 12), row.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -14),
            row.topAnchor.constraint(equalTo: topAnchor, constant: 8), row.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -8),
            name.widthAnchor.constraint(lessThanOrEqualToConstant: 260),
        ])
        paint()
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    @objc private func run() { handler() }
    override func viewDidChangeEffectiveAppearance() { super.viewDidChangeEffectiveAppearance(); paint() }
    private func paint() { layer?.borderColor = EmailTheme.cgColor(EmailTheme.line, in: effectiveAppearance) }
}

/// A hairline in the theme's line color.
final class AgentLine: NSView {
    override func viewDidChangeEffectiveAppearance() { super.viewDidChangeEffectiveAppearance(); needsDisplay = true }
    override func draw(_ dirtyRect: NSRect) { EmailTheme.line.setFill(); bounds.fill() }
}

final class AgentFlippedView: NSView { override var isFlipped: Bool { true } }

/// The count next to Agent requests in the sidebar.
@MainActor
final class AgentPill: NSView {
    var text = "" { didSet { invalidateIntrinsicContentSize(); needsDisplay = true; setAccessibilityLabel("\(text) waiting") } }
    private var attributes: [NSAttributedString.Key: Any] {
        [.font: NSFont.monospacedDigitSystemFont(ofSize: 10, weight: .semibold), .foregroundColor: NSColor.white]
    }
    override var intrinsicContentSize: NSSize {
        let size = (text as NSString).size(withAttributes: attributes)
        return NSSize(width: max(16, ceil(size.width) + 9), height: 16)
    }
    override func draw(_ dirtyRect: NSRect) {
        NSColor.controlAccentColor.setFill()
        NSBezierPath(roundedRect: bounds, xRadius: bounds.height / 2, yRadius: bounds.height / 2).fill()
        let size = (text as NSString).size(withAttributes: attributes)
        (text as NSString).draw(at: NSPoint(x: (bounds.width - size.width) / 2, y: (bounds.height - size.height) / 2), withAttributes: attributes)
    }
}
