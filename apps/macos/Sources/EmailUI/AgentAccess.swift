import AppKit

/// Agent access: per-account consent for agents connected through Email's local
/// MCP server, requests that wait for the user's approval, and agent history.
/// The disclosure text comes from mail-service so macOS and Linux show the
/// same words. Only this window can turn access on or approve a request; the
/// MCP server has no code path for either.
@MainActor
final class AgentAccessController: NSWindowController, NSWindowDelegate {
    /// The composer's undo window; approved agent sends get the same one.
    static let undoSeconds = 10
    private let core: CoreBridge
    private let stack = NSStackView()
    private let status = NSTextField(wrappingLabelWithString: "")
    private let statusAction = AgentButton(title: "") {}
    private let doneButton = AgentButton(title: "Done") {}
    private var statusActionHandler: (() -> Void)?
    private var statusActionExpiry: Timer?
    private var accounts: [[String: Any]] = []
    private var pending: [[String: Any]] = []
    private var history: [[String: Any]] = []
    private var blocked: [String: Any]?
    private var expandedBodies = Set<String>()
    private var busy = false
    private var renderedSignature = ""
    private var presentingDialog = false
    /// Decision clicks right after the list changed are ignored, so a card
    /// that just moved under the pointer cannot be approved by accident.
    private var renderedAt = Date.distantPast
    var onApprovedSend: ((_ messageID: String, _ sendAt: Date, _ scheduled: Bool) -> Void)?
    var onApprovedTriage: ((_ operationID: String, _ accounts: [String]) -> Void)?
    var onCancelledSend: ((_ messageID: String) -> Void)?
    var onUndoneTriage: ((_ operationID: String) -> Void)?
    var onChanged: (() -> Void)?

    init(core: CoreBridge) {
        self.core = core
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 600, height: 680),
                              styleMask: [.titled, .closable, .resizable, .miniaturizable],
                              backing: .buffered, defer: false)
        window.title = "Agent access"
        window.minSize = NSSize(width: 440, height: 420)
        window.isReleasedWhenClosed = false
        window.backgroundColor = EmailTheme.reader
        super.init(window: window)
        window.delegate = self
        let root = AgentRoot()
        window.contentView = root
        let scroll = NSScrollView()
        scroll.hasVerticalScroller = true
        scroll.drawsBackground = false
        scroll.autohidesScrollers = true
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 14
        let document = AgentDocument()
        document.translatesAutoresizingMaskIntoConstraints = false
        document.addSubview(stack)
        stack.translatesAutoresizingMaskIntoConstraints = false
        scroll.documentView = document
        status.font = .systemFont(ofSize: 11.5)
        status.textColor = EmailTheme.muted
        status.maximumNumberOfLines = 3
        status.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        statusAction.isHidden = true
        statusAction.handler = { [weak self] in self?.statusActionHandler?() }
        let done = doneButton
        done.handler = { [weak self] in self?.close() }
        done.keyEquivalent = "\u{1b}"
        done.toolTip = "Close (Esc)"
        let footer = NSStackView(views: [status, statusAction, done])
        footer.spacing = 10
        footer.alignment = .centerY
        status.setContentHuggingPriority(NSLayoutConstraint.Priority(1), for: .horizontal)
        for button in [statusAction, done] { button.setContentHuggingPriority(.defaultHigh, for: .horizontal) }
        let divider = NSBox()
        divider.boxType = .separator
        for view in [scroll, divider, footer] {
            view.translatesAutoresizingMaskIntoConstraints = false
            root.addSubview(view)
        }
        NSLayoutConstraint.activate([
            document.widthAnchor.constraint(equalTo: scroll.contentView.widthAnchor),
            stack.leadingAnchor.constraint(equalTo: document.leadingAnchor, constant: 24),
            stack.trailingAnchor.constraint(equalTo: document.trailingAnchor, constant: -24),
            stack.topAnchor.constraint(equalTo: document.topAnchor, constant: 20),
            stack.bottomAnchor.constraint(equalTo: document.bottomAnchor, constant: -20),
            scroll.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            scroll.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            scroll.topAnchor.constraint(equalTo: root.topAnchor),
            scroll.bottomAnchor.constraint(equalTo: divider.topAnchor),
            divider.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            divider.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            divider.bottomAnchor.constraint(equalTo: footer.topAnchor, constant: -10),
            footer.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 24),
            footer.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -24),
            footer.bottomAnchor.constraint(equalTo: root.bottomAnchor, constant: -12),
        ])
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    var isOpen: Bool { window?.isVisible == true }

    func open() {
        let firstShow = window?.isVisible != true
        showWindow(nil)
        if firstShow { window?.center() }
        window?.makeKeyAndOrderFront(nil)
        Task { await reload(force: true) }
    }

    /// Re-read state. Polls skip rendering when nothing changed, so an open
    /// window does not jump while the user reads a request.
    func reload(force: Bool = false) async {
        do {
            let state = try await core.callAsync(["op": "agent_access_overview", "history_limit": 60]) as? [String: Any] ?? [:]
            let open = state["requests"] as? [[String: Any]] ?? []
            let past = state["history"] as? [[String: Any]] ?? []
            let signature = Self.signature(state: state, pending: open, history: past)
            guard force || signature != renderedSignature else { return }
            // Only a change to the cards arms the click guard; history updates
            // below them must not drop a decision click.
            if open.map({ "\($0["id"] ?? "")" }) != pending.map({ "\($0["id"] ?? "")" }) { renderedAt = Date() }
            accounts = state["accounts"] as? [[String: Any]] ?? []
            blocked = (state["pending"] as? [String: Any])?["blocked"] as? [String: Any]
            pending = open
            history = past
            renderedSignature = signature
            render()
        } catch {
            setStatus(error.localizedDescription, error: true)
        }
    }

    private static func signature(state: [String: Any], pending: [[String: Any]], history: [[String: Any]]) -> String {
        let accounts = (state["accounts"] as? [[String: Any]] ?? []).map { "\($0["account_id"] ?? "")=\(($0["consent"] as? [String: Any])?["granted_at_ms"] ?? "off")" }
        let blocked = ((state["pending"] as? [String: Any])?["blocked"] as? [String: Any])?["id"] as? String ?? ""
        // The revision also changes when a coalesced entry updates in place.
        let revision = (state["pending"] as? [String: Any])?["revision"] as? String ?? ""
        return (accounts + [blocked, revision] + pending.map { "\($0["id"] ?? "")" } + history.prefix(5).map { "\($0["id"] ?? ""):\($0["status"] ?? "")" }).joined(separator: "|")
    }

    // MARK: Rendering

    private func render() {
        // Move focus off controls about to be destroyed; same rule as Linux.
        if let window, let responder = window.firstResponder as? NSView, responder.isDescendant(of: stack) {
            window.makeFirstResponder(doneButton)
        }
        stack.arrangedSubviews.forEach { stack.removeArrangedSubview($0); $0.removeFromSuperview() }
        add(label("Agent access", size: 17, weight: .semibold))
        add(label("AI agents and tools connected through Email's local MCP server. They can use only the accounts you turn on, and sending mail or bulk changes always waits for your approval here.", size: 12, muted: true))
        if let blocked, let summary = blocked["summary"] as? String, let banner = blockedBanner(summary: summary, blocked: blocked) {
            add(banner)
        }
        add(sectionHeading(pending.isEmpty ? "WAITING FOR YOUR APPROVAL" : "WAITING FOR YOUR APPROVAL · \(pending.count)"))
        if pending.isEmpty {
            add(label("No requests are waiting. When an agent asks to send mail or change many conversations, it appears here.", size: 12, muted: true))
        }
        for item in pending { add(pendingCard(item)) }
        add(sectionHeading("ACCOUNTS"))
        if accounts.isEmpty { add(label("Add a mail account first.", size: 12, muted: true)) }
        for account in accounts { add(accountRow(account)) }
        add(label("Turning access off applies to the agent's next request. Each MCP registration's own permissions also still apply.", size: 11, muted: true))
        add(sectionHeading("HISTORY"))
        if history.isEmpty { add(label("No agent activity yet.", size: 12, muted: true)) }
        for item in history { add(historyRow(item)) }
    }

    private func add(_ view: NSView) {
        stack.addArrangedSubview(view)
        view.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true
    }

    private func sectionHeading(_ text: String) -> NSView {
        let heading = label(text, size: 10, weight: .semibold, muted: true)
        stack.setCustomSpacing(18, after: stack.arrangedSubviews.last ?? heading)
        return heading
    }

    private func blockedBanner(summary: String, blocked: [String: Any]) -> NSView? {
        // mail-core decides which accounts a refusal still affects.
        let ids = blocked["affected_account_ids"] as? [String] ?? []
        let affected = accounts.filter { ids.contains($0["account_id"] as? String ?? "") }
        guard !affected.isEmpty else { return nil }
        let card = AgentCard(tint: EmailTheme.accentSoft)
        let column = card.column
        column.addArrangedSubview(label("Agent access needed", size: 12.5, weight: .semibold))
        column.addArrangedSubview(label(summary, size: 12))
        if let reason = blocked["reason"] as? String, !reason.isEmpty { column.addArrangedSubview(label(reason, size: 11, muted: true)) }
        if affected.count == 1, let id = affected[0]["account_id"] as? String, let email = affected[0]["email"] as? String {
            if affected[0]["current"] as? Bool == true, let grants = (affected[0]["consent"] as? [String: Any])?["grants"] as? [String] {
                column.addArrangedSubview(AgentButton(title: "Change Access for \(email)…") { [weak self] in self?.turnOn(accountID: id, email: email, current: grants) })
            } else {
                column.addArrangedSubview(AgentButton(title: "Turn On for \(email)…") { [weak self] in self?.turnOn(accountID: id, email: email, current: nil) })
            }
        } else {
            column.addArrangedSubview(label("Turn on agent access below for each account the agent may use.", size: 11, muted: true))
        }
        return card
    }

    private func pendingCard(_ item: [String: Any]) -> NSView {
        let id = item["id"] as? String ?? ""
        let kind = item["kind"] as? String ?? ""
        let details = item["details"] as? [String: Any] ?? [:]
        let card = AgentCard(tint: nil)
        let column = card.column
        column.addArrangedSubview(label(Self.kindTitle(kind), size: 10, weight: .semibold, muted: true))
        column.addArrangedSubview(label(item["summary"] as? String ?? "", size: 13.5, weight: .semibold))
        column.addArrangedSubview(label(meta(item, details: details), size: 11, muted: true))
        switch kind {
        case "send_draft", "queue_send_draft", "schedule_draft":
            let draft = details["draft"] as? [String: Any] ?? [:]
            column.addArrangedSubview(field("From", draft["from"] as? String ?? ""))
            for (name, key) in [("To", "to"), ("Cc", "cc"), ("Bcc", "bcc"), ("Reply-To", "reply_to")] {
                let value = Self.addresses(details, key)
                if !value.isEmpty || key == "to" { column.addArrangedSubview(field(name, value)) }
            }
            column.addArrangedSubview(field("Subject", (draft["subject"] as? String).flatMap { $0.isEmpty ? nil : $0 } ?? "(no subject)"))
            let files = draft["attachments"] as? [[String: Any]] ?? []
            for file in files {
                let filename = file["filename"] as? String ?? "file"
                let row = NSStackView()
                row.alignment = .centerY
                row.distribution = .fill
                row.spacing = 8
                let text = field("Attachment", "\(filename) (\(Self.size(file["size"])), \(file["mime_type"] as? String ?? "file"))")
                text.setContentHuggingPriority(NSLayoutConstraint.Priority(1), for: .horizontal)
                row.addArrangedSubview(text)
                let attachmentID = file["id"] as? String ?? ""
                let save = AgentButton(title: "Save…") { [weak self] in self?.saveAttachment(actionID: id, attachmentID: attachmentID, filename: filename) }
                save.setContentHuggingPriority(.defaultHigh, for: .horizontal)
                save.setAccessibilityLabel("Save a copy of \(filename)")
                row.addArrangedSubview(save)
                column.addArrangedSubview(row)
                row.widthAnchor.constraint(equalTo: column.widthAnchor).isActive = true
            }
            if !files.isEmpty {
                column.addArrangedSubview(label("The agent attached these files. Save a copy to view one before approving; never run a file you did not expect.", size: 11, muted: true))
            }
            column.addArrangedSubview(field("When", Self.when(kind: kind, details: details)))
            let invisible = (details["invisible_count"] as? NSNumber)?.intValue ?? 0
            if invisible > 0 {
                column.addArrangedSubview(label("Contains \(invisible) invisible character\(invisible == 1 ? "" : "s") used by emoji or some scripts.", size: 11, muted: true))
            }
            let body = draft["body_plain"] as? String ?? ""
            let expanded = expandedBodies.contains(id)
            // Count Unicode scalars and "\n"-separated lines, as Linux does.
            let lines = body.replacingOccurrences(of: "\r\n", with: "\n").split(separator: "\n", omittingEmptySubsequences: false)
            let shortened = body.unicodeScalars.count > 1200 || lines.count > 14
            // Expanded shows every character that would be sent; nothing is cut.
            let opening = String(String.UnicodeScalarView(lines.prefix(14).joined(separator: "\n").unicodeScalars.prefix(1200)))
            // Never clip visually: what the preview holds is shown in full, and
            // anything left out is announced with the note and the toggle.
            let preview = label(body.isEmpty ? "(empty message)" : (expanded ? body : opening + (shortened ? "…" : "")), size: 12)
            preview.maximumNumberOfLines = 0
            preview.lineBreakMode = .byWordWrapping
            let quote = AgentCard(tint: EmailTheme.canvas)
            quote.column.addArrangedSubview(preview)
            column.addArrangedSubview(quote)
            quote.widthAnchor.constraint(equalTo: column.widthAnchor).isActive = true
            if shortened && !expanded {
                column.addArrangedSubview(label("Only the beginning is shown. Show the full message to review everything that would be sent.", size: 11, muted: true))
            }
            if shortened {
                let toggle = AgentButton(title: expanded ? "Show less" : "Show full message") { [weak self] in
                    guard let self else { return }
                    if !self.expandedBodies.insert(id).inserted { self.expandedBodies.remove(id) }
                    self.render()
                }
                toggle.isBordered = false
                toggle.contentTintColor = EmailTheme.accent
                column.addArrangedSubview(toggle)
            }
        case "batch_triage", "undo_triage":
            let count = (details["count"] as? NSNumber)?.intValue ?? 0
            column.addArrangedSubview(field("Accounts", Self.list(details["account_emails"])))
            column.addArrangedSubview(field("Conversations", count.formatted()))
            let threads = details["threads"] as? [[String: Any]] ?? []
            for thread in threads.prefix(8) {
                let sender = MailNotificationPreview.senderName(thread["sender"] as? String ?? "")
                let subject = (thread["subject"] as? String).flatMap { $0.isEmpty ? nil : $0 } ?? "(no subject)"
                column.addArrangedSubview(label("\(sender) — \(subject)", size: 12, muted: true))
            }
            if count > min(threads.count, 8) { column.addArrangedSubview(label("and \((count - min(threads.count, 8)).formatted()) more", size: 11, muted: true)) }
            if kind == "undo_triage" {
                column.addArrangedSubview(label("These conversations return to where they were before the change.", size: 11, muted: true))
            } else if ["archive", "restore", "spam"].contains(details["kind"] as? String ?? "") {
                column.addArrangedSubview(label("You can undo this change after approving.", size: 11, muted: true))
            }
        case "confirm_sender_cleanup":
            column.addArrangedSubview(field("Sender", details["sender"] as? String ?? ""))
            column.addArrangedSubview(field("Unread messages", ((details["matched"] as? NSNumber)?.intValue ?? 0).formatted()))
            for subject in (details["sample_subjects"] as? [String] ?? []).prefix(3) {
                column.addArrangedSubview(label(subject, size: 12, muted: true))
            }
            column.addArrangedSubview(label("Only messages found in the preview are marked read in Gmail. Completed changes have no automatic undo.", size: 11, muted: true))
        default:
            break
        }
        let reject = AgentButton(title: "Reject") { [weak self] in self?.reject(id: id) }
        let approve = AgentButton(title: Self.approveTitle(kind)) { [weak self] in self?.approve(item) }
        approve.applyPrimaryStyle()
        let actions = NSStackView(views: [reject, approve])
        actions.spacing = 8
        reject.setAccessibilityLabel("Reject: \(item["summary"] as? String ?? "")")
        approve.setAccessibilityLabel("\(Self.approveTitle(kind)): \(item["summary"] as? String ?? "")")
        column.addArrangedSubview(actions)
        return card
    }

    private func accountRow(_ account: [String: Any]) -> NSView {
        let id = account["account_id"] as? String ?? ""
        let email = account["email"] as? String ?? id
        let consent = account["consent"] as? [String: Any]
        let current = account["current"] as? Bool ?? false
        let row = NSStackView()
        row.alignment = .centerY
        row.distribution = .fill
        row.spacing = 10
        let names = NSStackView()
        names.orientation = .vertical
        names.alignment = .leading
        names.spacing = 3
        let title = label(email, size: 13, weight: .semibold)
        title.lineBreakMode = .byTruncatingMiddle
        title.maximumNumberOfLines = 1
        names.addArrangedSubview(title)
        let state: String
        if let consent {
            let since = Self.date((consent["granted_at_ms"] as? NSNumber)?.doubleValue)
            let labels = (account["grant_labels"] as? [String] ?? []).joined(separator: ", ")
            state = current ? "On since \(since) · \(labels)" : "Needs review: the disclosure changed since \(since)"
        } else {
            state = "Off"
        }
        names.addArrangedSubview(label(state, size: 11, muted: true))
        names.setContentHuggingPriority(NSLayoutConstraint.Priority(1), for: .horizontal)
        names.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        row.addArrangedSubview(names)
        var buttons: [NSButton] = []
        if consent == nil || !current {
            buttons.append(AgentButton(title: "Turn On…") { [weak self] in self?.turnOn(accountID: id, email: email, current: nil) })
        } else {
            let grants = consent?["grants"] as? [String] ?? []
            buttons.append(AgentButton(title: "Change…") { [weak self] in self?.turnOn(accountID: id, email: email, current: grants) })
            buttons.append(AgentButton(title: "Turn Off") { [weak self] in self?.turnOff(accountID: id, email: email) })
        }
        for button in buttons {
            button.setContentHuggingPriority(.defaultHigh, for: .horizontal)
            button.setContentCompressionResistancePriority(.required, for: .horizontal)
            button.setAccessibilityLabel("\(button.title) agent access for \(email)")
            row.addArrangedSubview(button)
        }
        return row
    }

    private func historyRow(_ item: [String: Any]) -> NSView {
        let column = NSStackView()
        column.orientation = .vertical
        column.alignment = .leading
        column.spacing = 2
        let when = Self.dateTime((item["decided_at_ms"] as? NSNumber ?? item["created_at_ms"] as? NSNumber)?.doubleValue)
        var meta = "\(when) · \(Self.statusTitle(item["status"] as? String ?? ""))"
        let count = ((item["details"] as? [String: Any])?["count"] as? NSNumber)?.intValue ?? 1
        if count > 1 && (item["kind"] as? String) != "batch_triage" && (item["kind"] as? String) != "undo_triage" { meta += " · \(count) times" }
        let attempts = ((item["details"] as? [String: Any])?["attempts"] as? NSNumber)?.intValue ?? 1
        if attempts > 1 { meta += " · \(attempts) attempts" }
        if let client = item["client"] as? String, !client.isEmpty { meta += " · \(client)" }
        column.addArrangedSubview(label(meta, size: 10.5, muted: true))
        column.addArrangedSubview(label(item["summary"] as? String ?? "", size: 12))
        if let error = item["error"] as? String, !error.isEmpty {
            let detail = label(error, size: 11)
            detail.textColor = item["status"] as? String == "failed" ? .systemRed : EmailTheme.muted
            column.addArrangedSubview(detail)
        }
        return column
    }

    private func meta(_ item: [String: Any], details: [String: Any]) -> String {
        var parts: [String] = []
        if let email = details["account_email"] as? String { parts.append(email) }
        if let client = item["client"] as? String, !client.isEmpty { parts.append("requested by \(client)") }
        parts.append(Self.dateTime((item["created_at_ms"] as? NSNumber)?.doubleValue))
        return parts.joined(separator: " · ")
    }

    private func field(_ name: String, _ value: String) -> NSView {
        let text = NSMutableAttributedString(string: "\(name)  ", attributes: [.font: NSFont.systemFont(ofSize: 11.5, weight: .semibold), .foregroundColor: EmailTheme.muted])
        text.append(NSAttributedString(string: value, attributes: [.font: NSFont.systemFont(ofSize: 12), .foregroundColor: EmailTheme.ink]))
        let field = NSTextField(labelWithAttributedString: text)
        field.lineBreakMode = .byWordWrapping
        field.maximumNumberOfLines = 0
        field.isSelectable = true
        field.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        return field
    }

    private func label(_ text: String, size: CGFloat = 13, weight: NSFont.Weight = .regular, muted: Bool = false) -> NSTextField {
        let field = NSTextField(wrappingLabelWithString: text)
        field.font = .systemFont(ofSize: size, weight: weight)
        field.textColor = muted ? EmailTheme.muted : EmailTheme.ink
        field.isSelectable = false
        field.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        return field
    }

    private func setStatus(_ text: String, error: Bool = false) {
        status.stringValue = text
        status.textColor = error ? .systemRed : EmailTheme.muted
    }

    private func showStatusAction(_ title: String?, until: Date? = nil, handler: (() -> Void)? = nil) {
        statusActionExpiry?.invalidate()
        statusAction.title = title ?? ""
        statusAction.isHidden = title == nil
        statusActionHandler = handler
        if let until, title != nil {
            let timer = Timer(fire: until, interval: 0, repeats: false) { [weak self] _ in
                Task { @MainActor in self?.showStatusAction(nil) }
            }
            RunLoop.main.add(timer, forMode: .common)
            statusActionExpiry = timer
        }
    }

    // MARK: Decisions

    private func listJustChanged() -> Bool {
        guard Date().timeIntervalSince(renderedAt) < 0.8 else { return false }
        setStatus("The list just changed. Check the request, then choose again.")
        return true
    }

    private func approve(_ item: [String: Any]) {
        guard !busy, !listJustChanged(), let id = item["id"] as? String else { return }
        let kind = item["kind"] as? String ?? ""
        busy = true
        setStatus("Approving…")
        Task { @MainActor [weak self] in
            guard let self else { return }
            defer { busy = false }
            do {
                let action = try await core.callAsync(["op": "approve_agent_action", "id": id, "undo_seconds": Self.undoSeconds]) as? [String: Any] ?? [:]
                let result = action["result"] as? [String: Any] ?? [:]
                switch kind {
                case "send_draft", "queue_send_draft", "schedule_draft":
                    let messageID = result["message_id"] as? String ?? ""
                    let sendAt = Date(timeIntervalSince1970: ((result["send_at_ms"] as? NSNumber)?.doubleValue ?? 0) / 1000)
                    let scheduled = result["status"] as? String == "scheduled"
                    onApprovedSend?(messageID, sendAt, scheduled)
                    if scheduled {
                        setStatus("Approved · scheduled for \(Self.dateTime(sendAt.timeIntervalSince1970 * 1000)) · in Outbox")
                        showStatusAction("Cancel Send") { [weak self] in self?.cancelSend(messageID) }
                    } else {
                        let seconds = max(1, Int(sendAt.timeIntervalSinceNow.rounded()))
                        setStatus("Approved · sending in \(seconds) seconds")
                        showStatusAction("Undo Send", until: sendAt) { [weak self] in self?.cancelSend(messageID) }
                    }
                case "undo_triage":
                    let changed = (result["changed"] as? NSNumber)?.intValue ?? 0
                    onUndoneTriage?(result["operation_id"] as? String ?? "")
                    setStatus("Approved · change undone for \(changed.formatted()) conversation\(changed == 1 ? "" : "s")")
                    showStatusAction(nil)
                case "batch_triage":
                    let operation = result["operation_id"] as? String ?? ""
                    let changed = (result["changed"] as? NSNumber)?.intValue ?? 0
                    // Only an undoable change becomes the main window's undo.
                    if result["undoable"] as? Bool == true { onApprovedTriage?(operation, item["account_ids"] as? [String] ?? []) }
                    else { onChanged?() }
                    setStatus("Approved · \(changed.formatted()) conversation\(changed == 1 ? "" : "s") updated · saved locally")
                    if result["undoable"] as? Bool == true {
                        showStatusAction("Undo") { [weak self] in self?.undoTriage(operation) }
                    } else { showStatusAction(nil) }
                default:
                    setStatus("Approved · Email marks the messages read in batches. Open Cleanup history to follow progress.")
                    showStatusAction(nil)
                }
                await reload(force: true)
                onChanged?()
            } catch {
                setStatus(error.localizedDescription, error: true)
                showStatusAction(nil)
                await reload(force: true)
            }
        }
    }

    private func saveAttachment(actionID: String, attachmentID: String, filename: String) {
        Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                let file = try await core.callAsync(["op": "agent_request_attachment", "id": actionID, "attachment_id": attachmentID]) as? [String: Any] ?? [:]
                guard let encoded = file["data_base64"] as? String, let data = Data(base64Encoded: encoded) else {
                    setStatus("The attachment could not be read.", error: true)
                    return
                }
                let panel = NSSavePanel()
                panel.nameFieldStringValue = filename
                guard panel.runModal() == .OK, let url = panel.url else { return }
                try data.write(to: url, options: .atomic)
                // The agent chose these bytes: mark the copy as downloaded so
                // Gatekeeper checks it before anything can run it.
                var values = URLResourceValues()
                values.quarantineProperties = [kLSQuarantineTypeKey as String: kLSQuarantineTypeOtherDownload, kLSQuarantineAgentNameKey as String: "Email"]
                var marked = url
                try? marked.setResourceValues(values)
                setStatus("Saved \(url.lastPathComponent). View it with an app you trust before approving, and never run a file you did not expect.")
            } catch { setStatus(error.localizedDescription, error: true) }
        }
    }

    private func reject(id: String) {
        guard !busy, !listJustChanged() else { return }
        busy = true
        Task { @MainActor [weak self] in
            guard let self else { return }
            defer { busy = false }
            do {
                _ = try await core.callAsync(["op": "reject_agent_action", "id": id])
                setStatus("Rejected · nothing was sent or changed")
                showStatusAction(nil)
            } catch { setStatus(error.localizedDescription, error: true) }
            await reload(force: true)
            onChanged?()
        }
    }

    private func cancelSend(_ messageID: String) {
        Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                _ = try await core.callAsync(["op": "cancel_scheduled_draft", "message_id": messageID])
                onCancelledSend?(messageID)
                setStatus("Send cancelled · the message is back in Drafts")
                showStatusAction(nil)
                onChanged?()
            } catch { setStatus(error.localizedDescription, error: true) }
        }
    }

    private func undoTriage(_ operationID: String) {
        Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                _ = try await core.callAsync(["op": "undo_triage", "operation_id": operationID])
                onUndoneTriage?(operationID)
                setStatus("Change undone")
                showStatusAction(nil)
                onChanged?()
            } catch { setStatus(error.localizedDescription, error: true) }
        }
    }

    // MARK: Consent

    private func turnOn(accountID: String, email: String, current: [String]?) {
        guard !presentingDialog else { return }
        presentingDialog = true
        Task { @MainActor [weak self] in
            guard let self else { return }
            defer { presentingDialog = false }
            do {
                guard let disclosure = try await core.callAsync(["op": "agent_access_disclosure", "account_id": accountID]) as? [String: Any],
                      let grants = AgentAccessDisclosure.choose(disclosure, current) else { return }
                _ = try await core.callAsync(["op": "grant_agent_access", "account_id": accountID, "grants": grants,
                                              "disclosure_version": disclosure["version"] ?? 0])
                setStatus(current == nil ? "Agent access is on for \(email)" : "Agent access updated for \(email)")
                await reload(force: true)
                onChanged?()
            } catch { setStatus(error.localizedDescription, error: true) }
        }
    }

    private func turnOff(accountID: String, email: String) {
        guard AgentAccessDisclosure.confirmTurnOff(email) else { return }
        Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                _ = try await core.callAsync(["op": "revoke_agent_access", "account_id": accountID])
                setStatus("Agent access is off for \(email)")
                await reload(force: true)
                onChanged?()
            } catch { setStatus(error.localizedDescription, error: true) }
        }
    }

    // MARK: Formatting

    static func kindTitle(_ kind: String) -> String {
        switch kind {
        case "send_draft", "queue_send_draft": return "SEND EMAIL"
        case "schedule_draft": return "SCHEDULE EMAIL"
        case "batch_triage": return "CHANGE CONVERSATIONS"
        case "undo_triage": return "UNDO A CHANGE"
        case "confirm_sender_cleanup": return "SENDER CLEANUP"
        default: return kind.uppercased()
        }
    }
    static func approveTitle(_ kind: String) -> String {
        switch kind {
        case "send_draft", "queue_send_draft": return "Approve and Send"
        case "schedule_draft": return "Approve and Schedule"
        default: return "Approve"
        }
    }
    static func statusTitle(_ status: String) -> String {
        switch status {
        case "approved": return "Approved"
        case "rejected": return "Rejected"
        case "failed": return "Could not apply"
        case "blocked": return "Blocked"
        case "replaced": return "Replaced by a newer request"
        case "pending": return "Waiting"
        default: return "Done"
        }
    }
    static func when(kind: String, details: [String: Any]) -> String {
        switch kind {
        case "schedule_draft":
            let at = (details["send_at_ms"] as? NSNumber)?.doubleValue ?? 0
            return "Scheduled for \(dateTime(at)), or after a \(undoSeconds)-second undo window if that time has passed"
        case "queue_send_draft":
            let seconds = max((details["undo_seconds"] as? NSNumber)?.intValue ?? undoSeconds, undoSeconds)
            return "After approval, with \(seconds) seconds to undo"
        default:
            return "After approval, with \(undoSeconds) seconds to undo"
        }
    }
    static func list(_ value: Any?) -> String { (value as? [String] ?? []).joined(separator: ", ") }
    /// Bare addresses first; a display name is shown as what it is, since it
    /// can imitate another address.
    static func addresses(_ details: [String: Any], _ key: String) -> String {
        guard let parts = (details["addresses"] as? [String: Any])?[key] as? [[String: Any]] else {
            return list((details["draft"] as? [String: Any])?[key])
        }
        return parts.map { part in
            let address = part["address"] as? String ?? ""
            if let name = part["name"] as? String { return "\(address) (shown as “\(name)”)" }
            return address
        }.joined(separator: ", ")
    }
    static func size(_ value: Any?) -> String {
        ByteCountFormatter.string(fromByteCount: (value as? NSNumber)?.int64Value ?? 0, countStyle: .file)
    }
    static func date(_ ms: Double?) -> String {
        guard let ms else { return "" }
        return Date(timeIntervalSince1970: ms / 1000).formatted(date: .abbreviated, time: .omitted)
    }
    static func dateTime(_ ms: Double?) -> String {
        guard let ms else { return "" }
        return Date(timeIntervalSince1970: ms / 1000).formatted(date: .abbreviated, time: .shortened)
    }

    func windowWillClose(_ notification: Notification) { showStatusAction(nil) }
}

/// The consent disclosure. Read access is required and always on; every other
/// grant is opt-in. Returns the chosen grants, or nil when cancelled.
@MainActor
enum AgentAccessDisclosure {
    /// Replaceable only by the AppKit check harness, which cannot click a modal alert.
    static var choose: ([String: Any], [String]?) -> [String]? = { present($0, changing: $1) }
    static var confirmTurnOff: (String) -> Bool = { email in
        let alert = NSAlert()
        alert.messageText = "Turn off agent access for \(email)?"
        alert.informativeText = "Agents lose access to this account on their next request. Requests waiting for your approval are rejected. Mail you already approved stays in the Outbox, where you can cancel it."
        // Cancel first: Escape cancels and Return never confirms. Turning off needs a click.
        alert.addButton(withTitle: "Cancel")
        alert.addButton(withTitle: "Turn Off")
        return alert.runModal() == .alertSecondButtonReturn
    }

    static func present(_ disclosure: [String: Any], changing current: [String]?) -> [String]? {
        let (alert, boxes) = makeAlert(disclosure, changing: current)
        guard alert.runModal() == .alertSecondButtonReturn else { return nil }
        return boxes.filter { $0.1.state == .on }.map(\.0)
    }

    static func makeAlert(_ disclosure: [String: Any], changing current: [String]?) -> (NSAlert, [(String, NSButton)]) {
        let width: CGFloat = 430
        let alert = NSAlert()
        alert.messageText = (current == nil ? disclosure["title"] : disclosure["change_title"]) as? String ?? "Agent access"
        alert.informativeText = ""
        let column = NSStackView()
        column.orientation = .vertical
        column.alignment = .leading
        column.spacing = 8
        func text(_ value: String, size: CGFloat = 12, weight: NSFont.Weight = .regular, muted: Bool = false) -> NSTextField {
            let field = NSTextField(wrappingLabelWithString: value)
            field.font = .systemFont(ofSize: size, weight: weight)
            field.textColor = muted ? .secondaryLabelColor : .labelColor
            field.preferredMaxLayoutWidth = width
            field.isSelectable = false
            return field
        }
        column.addArrangedSubview(text(disclosure["intro"] as? String ?? ""))
        for category in disclosure["data_categories"] as? [String] ?? [] {
            column.addArrangedSubview(text("•  \(category)"))
        }
        if let note = disclosure["shared_note"] as? String { column.addArrangedSubview(text(note, muted: true)) }
        column.addArrangedSubview(text(disclosure["provider_terms"] as? String ?? "", weight: .semibold))
        column.addArrangedSubview(text(disclosure["safeguards"] as? String ?? ""))
        column.addArrangedSubview(text(disclosure["grants_heading"] as? String ?? "", weight: .semibold))
        let read = NSButton(checkboxWithTitle: disclosure["read_label"] as? String ?? "Read this account's data (required)", target: nil, action: nil)
        read.state = .on
        read.isEnabled = false
        column.addArrangedSubview(read)
        var boxes: [(String, NSButton)] = []
        for grant in disclosure["grants"] as? [[String: Any]] ?? [] {
            guard let id = grant["id"] as? String else { continue }
            let box = NSButton(checkboxWithTitle: grant["label"] as? String ?? id, target: nil, action: nil)
            box.state = current?.contains(id) == true ? .on : .off
            box.setAccessibilityHelp(grant["detail"] as? String)
            column.addArrangedSubview(box)
            let detail = text(grant["detail"] as? String ?? "", size: 11, muted: true)
            column.addArrangedSubview(detail)
            column.setCustomSpacing(10, after: detail)
            boxes.append((id, box))
        }
        column.addArrangedSubview(text(disclosure["revoke"] as? String ?? "", size: 11, muted: true))
        column.translatesAutoresizingMaskIntoConstraints = false
        column.widthAnchor.constraint(equalToConstant: width).isActive = true
        column.layoutSubtreeIfNeeded()
        column.frame = NSRect(origin: .zero, size: column.fittingSize)
        column.translatesAutoresizingMaskIntoConstraints = true
        alert.accessoryView = column
        // Cancel first: Escape cancels and Return never confirms. Consent needs a deliberate click.
        alert.addButton(withTitle: "Cancel")
        alert.addButton(withTitle: (current == nil ? disclosure["confirm"] : disclosure["change_confirm"]) as? String ?? "Turn On Agent Access")
        return (alert, boxes)
    }
}

@MainActor
private final class AgentButton: NSButton {
    var handler: () -> Void
    init(title: String, handler: @escaping () -> Void) {
        self.handler = handler
        super.init(frame: .zero)
        self.title = title
        bezelStyle = .rounded
        target = self
        action = #selector(run)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    @objc private func run() { handler() }
    func applyPrimaryStyle() {
        isBordered = false
        wantsLayer = true
        layer?.cornerRadius = 7
        font = .systemFont(ofSize: 12, weight: .semibold)
        updatePrimaryColors()
        heightAnchor.constraint(equalToConstant: 26).isActive = true
        widthAnchor.constraint(greaterThanOrEqualToConstant: intrinsicContentSize.width + 24).isActive = true
    }
    private func updatePrimaryColors() {
        guard !isBordered else { return }
        layer?.backgroundColor = EmailTheme.cgColor(EmailTheme.accent, in: effectiveAppearance)
        let dark = effectiveAppearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
        attributedTitle = NSAttributedString(string: title, attributes: [.foregroundColor: dark ? EmailTheme.canvas : NSColor.white, .font: font ?? .systemFont(ofSize: 12, weight: .semibold)])
    }
    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        updatePrimaryColors()
    }
}

/// A padded, bordered surface whose `column` holds the content.
private final class AgentCard: NSView {
    let column = NSStackView()
    private let tint: NSColor?
    init(tint: NSColor?) {
        self.tint = tint
        super.init(frame: .zero)
        column.orientation = .vertical
        column.alignment = .leading
        column.spacing = 6
        column.translatesAutoresizingMaskIntoConstraints = false
        addSubview(column)
        NSLayoutConstraint.activate([
            column.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 14),
            column.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -14),
            column.topAnchor.constraint(equalTo: topAnchor, constant: 12),
            column.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -12),
        ])
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    override var isFlipped: Bool { true }
    override func viewDidChangeEffectiveAppearance() { super.viewDidChangeEffectiveAppearance(); needsDisplay = true }
    override func draw(_ dirtyRect: NSRect) {
        let path = NSBezierPath(roundedRect: bounds.insetBy(dx: 0.5, dy: 0.5), xRadius: 10, yRadius: 10)
        (tint ?? EmailTheme.reader).setFill()
        path.fill()
        EmailTheme.line.setStroke()
        path.lineWidth = 1
        path.stroke()
    }
    override func layout() {
        super.layout()
        // Wrapping labels need their available width to compute height.
        for case let field as NSTextField in column.arrangedSubviews {
            field.preferredMaxLayoutWidth = column.bounds.width
        }
    }
}

// Flipped coordinates keep the first section at the top of the scroll view.
private final class AgentDocument: NSView { override var isFlipped: Bool { true } }
/// Paints the window surface itself so it follows light and dark appearance.
private final class AgentRoot: NSView {
    override func viewDidChangeEffectiveAppearance() { super.viewDidChangeEffectiveAppearance(); needsDisplay = true }
    override func draw(_ dirtyRect: NSRect) { EmailTheme.reader.setFill(); bounds.fill() }
}
