import AppKit

/// Agent access: requests waiting for the user's approval, connecting Claude
/// Code or Codex to Email's local MCP server, per-account consent, and recent
/// agent activity. The turn-on sheet's text comes from mail-service so macOS
/// and Linux show the same words. Only this window can turn access on or
/// approve a request; the MCP server has no code path for either.
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
    private var historyExpanded = false
    /// The `agent_clients` answer: CLIs found, registrations, the helper.
    private var clients: [String: Any]?
    private var clientsLoading = false
    private var connecting: String?
    private weak var copyButton: NSButton?
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
        stack.spacing = 10
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
            stack.topAnchor.constraint(equalTo: document.topAnchor, constant: 22),
            stack.bottomAnchor.constraint(equalTo: document.bottomAnchor, constant: -22),
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
        refreshClients()
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

    /// The helper inside the app bundle. Development builds run outside a
    /// bundle; mail-service then looks at `EMAIL_MCP_HELPER`.
    static var bundledHelper: String? {
        let bundle = Bundle.main.bundleURL
        guard bundle.pathExtension == "app" else { return nil }
        let helper = bundle.appendingPathComponent("Contents/Helpers/mail-mcp").path
        return FileManager.default.isExecutableFile(atPath: helper) ? helper : nil
    }

    private func clientRequest(_ op: String, _ extra: [String: Any] = [:]) -> [String: Any] {
        var payload = extra
        payload["op"] = op
        if let helper = Self.bundledHelper { payload["helper_path"] = helper }
        return payload
    }

    /// Look for Claude Code and Codex again. Runs their CLIs, so only when the
    /// window opens and after Connect, never on the five-second poll.
    func refreshClients() {
        guard !clientsLoading else { return }
        clientsLoading = true
        Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                clients = try await core.callAsync(clientRequest("agent_clients")) as? [String: Any]
            } catch {
                setStatus(error.localizedDescription, error: true)
            }
            clientsLoading = false
            render()
        }
    }

    // MARK: Rendering

    private func render() {
        // Move focus off controls about to be destroyed; same rule as Linux.
        if let window, let responder = window.firstResponder as? NSView, responder.isDescendant(of: stack) {
            window.makeFirstResponder(doneButton)
        }
        stack.arrangedSubviews.forEach { stack.removeArrangedSubview($0); $0.removeFromSuperview() }
        add(label("Agent access", size: 20, weight: .semibold))
        stack.setCustomSpacing(4, after: stack.arrangedSubviews.last!)
        add(label("Let AI tools like Claude Code and Codex work with your mail. You approve every send.", size: 12.5, muted: true))
        if let blocked, let summary = blocked["summary"] as? String, let banner = blockedBanner(summary: summary, blocked: blocked) {
            space(18)
            add(banner)
        }
        if !pending.isEmpty {
            add(sectionHeading("Waiting for you", count: pending.count))
            for item in pending { add(pendingCard(item)) }
        }
        add(sectionHeading("Connect an agent"))
        add(connectList())
        add(connectNote())
        add(sectionHeading("Accounts"))
        if accounts.isEmpty {
            add(label("Add a mail account first.", size: 12, muted: true))
        } else {
            add(accountList())
        }
        space(14)
        let toggle = AgentDisclosureButton(title: "Recent activity", expanded: historyExpanded, size: 13) { [weak self] in
            guard let self else { return }
            historyExpanded.toggle()
            render()
        }
        add(Self.leading(toggle))
        if historyExpanded {
            if history.isEmpty { add(label("Nothing yet.", size: 12, muted: true)) }
            for item in history { add(historyRow(item)) }
        }
    }

    private func add(_ view: NSView) {
        stack.addArrangedSubview(view)
        view.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true
    }

    /// A full-width row holding `view` at its natural width on the left.
    static func leading(_ view: NSView) -> NSView {
        let spacer = NSView()
        spacer.setContentHuggingPriority(NSLayoutConstraint.Priority(1), for: .horizontal)
        let row = NSStackView(views: [view, spacer])
        row.spacing = 0
        return row
    }

    private func space(_ points: CGFloat) {
        if let last = stack.arrangedSubviews.last { stack.setCustomSpacing(points, after: last) }
    }

    private func sectionHeading(_ text: String, count: Int? = nil) -> NSView {
        space(26)
        let title = label(text, size: 13, weight: .semibold)
        title.setContentHuggingPriority(.required, for: .horizontal)
        var views: [NSView] = [title]
        if let count { views.append(AgentBadge(text: count > 99 ? "99+" : String(count))) }
        let spacer = NSView()
        spacer.setContentHuggingPriority(NSLayoutConstraint.Priority(1), for: .horizontal)
        views.append(spacer)
        let row = NSStackView(views: views)
        row.spacing = 7
        row.alignment = .centerY
        if let count { row.setAccessibilityLabel("\(text), \(count)") }
        return row
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
        // The refusal is written for the agent; for a missing grant its first
        // sentence says which one, which is all the user needs here.
        if blocked["needs_access"] as? Bool == false, let reason = blocked["reason"] as? String,
           let sentence = reason.components(separatedBy: ". ").first, !sentence.isEmpty {
            column.addArrangedSubview(label(sentence.hasSuffix(".") ? sentence : sentence + ".", size: 11.5, muted: true))
        }
        if affected.count == 1, let id = affected[0]["account_id"] as? String, let email = affected[0]["email"] as? String {
            if affected[0]["current"] as? Bool == true, let grants = (affected[0]["consent"] as? [String: Any])?["grants"] as? [String] {
                column.addArrangedSubview(AgentButton(title: "Change access for \(email)…") { [weak self] in self?.turnOn(accountID: id, email: email, current: grants) })
            } else {
                column.addArrangedSubview(AgentButton(title: "Turn on for \(email)…") { [weak self] in self?.turnOn(accountID: id, email: email, current: nil) })
            }
        } else {
            column.addArrangedSubview(label("Turn on each account the agent may use under Accounts.", size: 11, muted: true))
        }
        return card
    }

    private func pendingCard(_ item: [String: Any]) -> NSView {
        let id = item["id"] as? String ?? ""
        let kind = item["kind"] as? String ?? ""
        let details = item["details"] as? [String: Any] ?? [:]
        let card = AgentCard(tint: nil)
        let column = card.column
        column.spacing = 5
        column.addArrangedSubview(kindLabel(Self.kindTitle(kind)))
        var toggle: NSButton?
        switch kind {
        case "send_draft", "queue_send_draft", "schedule_draft":
            let draft = details["draft"] as? [String: Any] ?? [:]
            let subject = (draft["subject"] as? String).flatMap { $0.isEmpty ? nil : $0 } ?? "(no subject)"
            let headline = label(subject, size: 14, weight: .semibold)
            column.addArrangedSubview(headline)
            column.setCustomSpacing(7, after: headline)
            for (name, key) in [("To", "to"), ("Cc", "cc"), ("Bcc", "bcc"), ("Reply-To", "reply_to")] {
                let value = Self.addresses(details, key)
                if !value.isEmpty || key == "to" { column.addArrangedSubview(field(name, value)) }
            }
            column.addArrangedSubview(field("From", draft["from"] as? String ?? ""))
            if kind == "schedule_draft" { column.addArrangedSubview(field("When", Self.when(kind: kind, details: details))) }
            let invisible = (details["invisible_count"] as? NSNumber)?.intValue ?? 0
            if invisible > 0 {
                column.addArrangedSubview(label("Contains \(invisible) invisible character\(invisible == 1 ? "" : "s") used by emoji or some scripts.", size: 11, muted: true))
            }
            let files = draft["attachments"] as? [[String: Any]] ?? []
            let expanded = expandedBodies.contains(id)
            if expanded {
                let body = draft["body_plain"] as? String ?? ""
                // Expanded shows every character that would be sent; nothing is cut.
                let preview = label(body.isEmpty ? "(empty message)" : body, size: 12)
                preview.maximumNumberOfLines = 0
                preview.lineBreakMode = .byWordWrapping
                preview.isSelectable = true
                let quote = AgentCard(tint: EmailTheme.canvas)
                quote.column.addArrangedSubview(preview)
                column.setCustomSpacing(10, after: column.arrangedSubviews.last!)
                column.addArrangedSubview(quote)
                quote.widthAnchor.constraint(equalTo: column.widthAnchor).isActive = true
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
                if kind != "schedule_draft" { column.addArrangedSubview(field("When", Self.when(kind: kind, details: details))) }
            }
            let attachments = files.isEmpty ? "" : " · \(files.count) attachment\(files.count == 1 ? "" : "s")"
            toggle = AgentDisclosureButton(title: (expanded ? "Hide message" : "Show message") + attachments, expanded: expanded, size: 12) { [weak self] in
                guard let self else { return }
                if !expandedBodies.insert(id).inserted { expandedBodies.remove(id) }
                render()
            }
        case "batch_triage", "undo_triage":
            column.addArrangedSubview(label(item["summary"] as? String ?? "", size: 14, weight: .semibold))
            let count = (details["count"] as? NSNumber)?.intValue ?? 0
            column.addArrangedSubview(field("Accounts", Self.list(details["account_emails"])))
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
            column.addArrangedSubview(label(item["summary"] as? String ?? "", size: 14, weight: .semibold))
            column.addArrangedSubview(field("Sender", details["sender"] as? String ?? ""))
            column.addArrangedSubview(field("Unread messages", ((details["matched"] as? NSNumber)?.intValue ?? 0).formatted()))
            for subject in (details["sample_subjects"] as? [String] ?? []).prefix(3) {
                column.addArrangedSubview(label(subject, size: 12, muted: true))
            }
            column.addArrangedSubview(label("Only messages found in the preview are marked read in Gmail. Completed changes have no automatic undo.", size: 11, muted: true))
        default:
            column.addArrangedSubview(label(item["summary"] as? String ?? "", size: 14, weight: .semibold))
        }
        column.addArrangedSubview(label(meta(item), size: 11, muted: true))
        let reject = AgentButton(title: "Reject") { [weak self] in self?.reject(id: id) }
        let approve = AgentButton(title: Self.approveTitle(kind)) { [weak self] in self?.approve(item) }
        approve.applyPrimaryStyle()
        reject.setAccessibilityLabel("Reject: \(item["summary"] as? String ?? "")")
        approve.setAccessibilityLabel("\(Self.approveTitle(kind)): \(item["summary"] as? String ?? "")")
        let spacer = NSView()
        spacer.setContentHuggingPriority(NSLayoutConstraint.Priority(1), for: .horizontal)
        let actions = NSStackView(views: [toggle, spacer, reject, approve].compactMap { $0 })
        actions.spacing = 8
        actions.alignment = .centerY
        for button in [reject, approve] { button.setContentCompressionResistancePriority(.required, for: .horizontal) }
        column.setCustomSpacing(10, after: column.arrangedSubviews.last!)
        column.addArrangedSubview(actions)
        actions.widthAnchor.constraint(equalTo: column.widthAnchor).isActive = true
        return card
    }

    // MARK: Connect an agent

    private func connectList() -> NSView {
        let card = AgentCard(tint: nil)
        let column = card.column
        column.spacing = 10
        let rows = clients?["clients"] as? [[String: Any]] ?? [
            ["id": "claude", "name": "Claude Code"], ["id": "codex", "name": "Codex"],
        ]
        let helperFound = clients?["helper"] is String
        for row in rows {
            let id = row["id"] as? String ?? ""
            let name = row["name"] as? String ?? id
            let checking = clients == nil
            let subtitle = checking ? "Checking…" : row["subtitle"] as? String ?? ""
            var trailing: NSView?
            if row["connected"] as? Bool == true {
                trailing = connectedLabel()
            } else if row["installed"] as? Bool == true {
                let busy = connecting == id
                let button = AgentButton(title: busy ? "Connecting…" : "Connect") { [weak self] in self?.connect(id: id, name: name) }
                button.isEnabled = helperFound && connecting == nil
                button.setAccessibilityLabel("Connect \(name)")
                trailing = button
            }
            addListRow(column, title: name, subtitle: subtitle, trailing: trailing)
        }
        let copy = AgentButton(title: "Copy") { [weak self] in self?.copyManualConfig() }
        copy.isEnabled = clients?["manual"] is [String: Any]
        copy.setAccessibilityLabel("Copy the command and JSON config for another MCP client")
        copyButton = copy
        addListRow(column, title: "Other MCP client", subtitle: "Copy the command and a JSON config", trailing: copy)
        return card
    }

    private func connectNote() -> NSView {
        if let problem = clients?["helper_problem"] as? String {
            return label(problem, size: 11.5, muted: true)
        }
        return label("Connected agents can use only the accounts you turn on below.", size: 11.5, muted: true)
    }

    private func connectedLabel() -> NSView {
        let text = NSMutableAttributedString(string: "✓ ", attributes: [.font: NSFont.systemFont(ofSize: 12.5, weight: .semibold), .foregroundColor: NSColor.systemGreen])
        text.append(NSAttributedString(string: "Connected", attributes: [.font: NSFont.systemFont(ofSize: 12.5, weight: .medium), .foregroundColor: EmailTheme.ink]))
        let field = NSTextField(labelWithAttributedString: text)
        field.setAccessibilityLabel("Connected")
        return field
    }

    /// A row in a list card: title and subtitle on the left, a control on the
    /// right, and a hairline above every row but the first.
    private func addListRow(_ column: NSStackView, title: String, subtitle: NSView, trailing: NSView?) {
        if !column.arrangedSubviews.isEmpty {
            let line = NSBox()
            line.boxType = .separator
            column.addArrangedSubview(line)
            line.widthAnchor.constraint(equalTo: column.widthAnchor).isActive = true
        }
        let names = NSStackView()
        names.orientation = .vertical
        names.alignment = .leading
        names.spacing = 2
        let heading = label(title, size: 13, weight: .semibold)
        heading.lineBreakMode = .byTruncatingMiddle
        heading.maximumNumberOfLines = 1
        names.addArrangedSubview(heading)
        names.addArrangedSubview(subtitle)
        names.setContentHuggingPriority(NSLayoutConstraint.Priority(1), for: .horizontal)
        names.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        let row = NSStackView(views: [names] + (trailing.map { [$0] } ?? []))
        row.alignment = .centerY
        row.distribution = .fill
        row.spacing = 12
        trailing?.setContentHuggingPriority(.defaultHigh, for: .horizontal)
        trailing?.setContentCompressionResistancePriority(.required, for: .horizontal)
        column.addArrangedSubview(row)
        row.widthAnchor.constraint(equalTo: column.widthAnchor).isActive = true
    }

    private func addListRow(_ column: NSStackView, title: String, subtitle: String, trailing: NSView?) {
        addListRow(column, title: title, subtitle: label(subtitle, size: 11.5, muted: true), trailing: trailing)
    }

    private func connect(id: String, name: String) {
        guard connecting == nil else { return }
        connecting = id
        render()
        Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                let result = try await core.callAsync(clientRequest("connect_agent_client", ["client": id])) as? [String: Any] ?? [:]
                setStatus(result["already_registered"] as? Bool == true
                          ? "\(name) already had Email set up."
                          : "\(name) is connected. Start a new \(name) session to use it.")
            } catch {
                setStatus(error.localizedDescription, error: true)
            }
            connecting = nil
            refreshClients()
        }
    }

    private func copyManualConfig() {
        guard let text = (clients?["manual"] as? [String: Any])?["copy_text"] as? String else { return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
        setStatus("Copied the mail-mcp command and a JSON config. Paste whichever your client takes.")
        guard let button = copyButton else { return }
        button.title = "Copied"
        DispatchQueue.main.asyncAfter(deadline: .now() + 2) { [weak button] in
            MainActor.assumeIsolated { button?.title = "Copy" }
        }
    }

    // MARK: Accounts

    private func accountList() -> NSView {
        let card = AgentCard(tint: nil)
        card.column.spacing = 10
        for account in accounts {
            let id = account["account_id"] as? String ?? ""
            let email = account["email"] as? String ?? id
            let consent = account["consent"] as? [String: Any]
            let on = consent != nil && account["current"] as? Bool == true
            let subtitle: NSView
            if on {
                let summary = label(account["grant_summary"] as? String ?? "Read", size: 11.5, muted: true)
                summary.setContentHuggingPriority(.defaultHigh, for: .horizontal)
                let grants = consent?["grants"] as? [String] ?? []
                let edit = AgentLinkButton(title: "Edit") { [weak self] in self?.turnOn(accountID: id, email: email, current: grants) }
                edit.setAccessibilityLabel("Edit agent access for \(email)")
                let spacer = NSView()
                spacer.setContentHuggingPriority(NSLayoutConstraint.Priority(1), for: .horizontal)
                let row = NSStackView(views: [summary, edit, spacer])
                row.spacing = 6
                row.alignment = .firstBaseline
                subtitle = row
            } else if consent != nil {
                subtitle = label("Off · the terms changed; switch on to review them", size: 11.5, muted: true)
            } else {
                subtitle = label("Off", size: 11.5, muted: true)
            }
            let toggle = AgentSwitch()
            toggle.state = on ? .on : .off
            toggle.setAccessibilityLabel("Agent access for \(email)")
            toggle.handler = { [weak self] isOn in
                if isOn { self?.turnOn(accountID: id, email: email, current: nil) }
                else { self?.turnOff(accountID: id, email: email) }
            }
            addListRow(card.column, title: email, subtitle: subtitle, trailing: toggle)
        }
        return card
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

    private func meta(_ item: [String: Any]) -> String {
        var parts: [String] = []
        if let client = item["client"] as? String, !client.isEmpty { parts.append("Requested by \(client)") }
        parts.append(Self.dateTime((item["created_at_ms"] as? NSNumber)?.doubleValue))
        return parts.joined(separator: " · ")
    }

    private func kindLabel(_ text: String) -> NSTextField {
        let field = label("", size: 10, weight: .semibold, muted: true)
        field.attributedStringValue = NSAttributedString(string: text, attributes: [.font: NSFont.systemFont(ofSize: 10, weight: .semibold), .foregroundColor: EmailTheme.muted, .kern: 0.6])
        return field
    }

    private func field(_ name: String, _ value: String) -> NSView {
        let text = NSMutableAttributedString(string: "\(name)  ", attributes: [.font: NSFont.systemFont(ofSize: 12, weight: .regular), .foregroundColor: EmailTheme.muted])
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
                        showStatusAction("Cancel send") { [weak self] in self?.cancelSend(messageID) }
                    } else {
                        let seconds = max(1, Int(sendAt.timeIntervalSinceNow.rounded()))
                        setStatus("Approved · sending in \(seconds) seconds")
                        showStatusAction("Undo send", until: sendAt) { [weak self] in self?.cancelSend(messageID) }
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

    /// Opens the turn-on sheet: from the switch (`current` nil), or from Edit
    /// with the current grants. Cancelling puts the switch back.
    private func turnOn(accountID: String, email: String, current: [String]?) {
        guard !presentingDialog, let window else { render(); return }
        presentingDialog = true
        Task { @MainActor [weak self] in
            guard let self else { return }
            let disclosure: [String: Any]
            do {
                disclosure = try await core.callAsync(["op": "agent_access_disclosure", "account_id": accountID]) as? [String: Any] ?? [:]
            } catch {
                presentingDialog = false
                setStatus(error.localizedDescription, error: true)
                render()
                return
            }
            AgentAccessDisclosure.choose(disclosure, current, window) { [weak self] grants in
                guard let self else { return }
                presentingDialog = false
                guard let grants else { render(); return }
                Task { @MainActor [weak self] in
                    guard let self else { return }
                    do {
                        _ = try await core.callAsync(["op": "grant_agent_access", "account_id": accountID, "grants": grants,
                                                      "disclosure_version": disclosure["version"] ?? 0])
                        setStatus(current == nil ? "Agent access is on for \(email)" : "Agent access updated for \(email)")
                        await reload(force: true)
                        onChanged?()
                    } catch {
                        setStatus(error.localizedDescription, error: true)
                        render()
                    }
                }
            }
        }
    }

    private func turnOff(accountID: String, email: String) {
        guard !presentingDialog, let window else { render(); return }
        presentingDialog = true
        AgentAccessDisclosure.confirmTurnOff(email, window) { [weak self] confirmed in
            guard let self else { return }
            presentingDialog = false
            guard confirmed else { render(); return }
            Task { @MainActor [weak self] in
                guard let self else { return }
                do {
                    _ = try await core.callAsync(["op": "revoke_agent_access", "account_id": accountID])
                    setStatus("Agent access is off for \(email)")
                    await reload(force: true)
                    onChanged?()
                } catch {
                    setStatus(error.localizedDescription, error: true)
                    render()
                }
            }
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
        case "send_draft", "queue_send_draft": return "Approve and send"
        case "schedule_draft": return "Approve and schedule"
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
    static func dateTime(_ ms: Double?) -> String {
        guard let ms else { return "" }
        return Date(timeIntervalSince1970: ms / 1000).formatted(date: .abbreviated, time: .shortened)
    }

    func windowWillClose(_ notification: Notification) { showStatusAction(nil) }
}

/// The turn-on sheet and the turn-off confirmation. Read is required and
/// always on; every other grant starts unchecked.
@MainActor
enum AgentAccessDisclosure {
    /// Replaceable only by the AppKit check harness. `done` gets the chosen
    /// grants, or nil when cancelled.
    static var choose: (_ disclosure: [String: Any], _ current: [String]?, _ parent: NSWindow, _ done: @escaping ([String]?) -> Void) -> Void = { disclosure, current, parent, done in
        let sheet = AgentAccessSheet(disclosure: disclosure, changing: current)
        sheet.present(on: parent, done: done)
    }
    static var confirmTurnOff: (_ email: String, _ parent: NSWindow, _ done: @escaping (Bool) -> Void) -> Void = { email, parent, done in
        let alert = makeTurnOffAlert(email)
        alert.beginSheetModal(for: parent) { done($0 == .alertSecondButtonReturn) }
    }

    static func makeTurnOffAlert(_ email: String) -> NSAlert {
        let alert = NSAlert()
        alert.messageText = "Turn off agent access for \(email)?"
        alert.informativeText = "Agents lose access on their next request, and requests waiting for you are rejected. Mail you already approved stays in the Outbox."
        // Cancel first: Escape cancels and Return never confirms. Turning off needs a click.
        alert.addButton(withTitle: "Cancel")
        alert.addButton(withTitle: "Turn off")
        return alert
    }
}

/// "Let agents use {email}?": the grants, the provider-terms callout, how to
/// turn access off, and the full list of what agents can read one click away.
@MainActor
final class AgentAccessSheet: NSObject, NSWindowDelegate {
    /// The sheet being shown; it owns itself until it closes.
    private(set) static var presented: AgentAccessSheet?
    let panel: NSWindow
    private(set) var boxes: [(String, NSButton)] = []
    let cancelButton: AgentButton
    let confirmButton: AgentButton
    private let content = NSStackView()
    private let details = NSStackView()
    private let scroll = NSScrollView()
    private var detailsToggle: AgentDisclosureButton?
    private(set) var detailsExpanded = false
    private var done: (([String]?) -> Void)?
    private weak var parent: NSWindow?
    private static let width: CGFloat = 460

    init(disclosure: [String: Any], changing current: [String]?) {
        panel = NSWindow(contentRect: NSRect(x: 0, y: 0, width: Self.width, height: 500), styleMask: [.titled], backing: .buffered, defer: false)
        cancelButton = AgentButton(title: disclosure["cancel"] as? String ?? "Cancel") {}
        confirmButton = AgentButton(title: (current == nil ? disclosure["confirm"] : disclosure["change_confirm"]) as? String ?? "Turn on") {}
        super.init()
        panel.title = "Agent access"
        panel.backgroundColor = EmailTheme.reader
        panel.delegate = self
        let root = AgentRoot()
        panel.contentView = root
        content.orientation = .vertical
        content.alignment = .leading
        content.spacing = 8
        content.translatesAutoresizingMaskIntoConstraints = false
        let document = AgentDocument()
        document.translatesAutoresizingMaskIntoConstraints = false
        document.addSubview(content)
        scroll.documentView = document
        scroll.drawsBackground = false
        scroll.hasVerticalScroller = true
        scroll.autohidesScrollers = true

        func text(_ value: String, size: CGFloat = 12.5, weight: NSFont.Weight = .regular, color: NSColor = EmailTheme.ink) -> NSTextField {
            let field = NSTextField(wrappingLabelWithString: value)
            field.font = .systemFont(ofSize: size, weight: weight)
            field.textColor = color
            field.isSelectable = false
            field.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
            return field
        }
        func add(_ view: NSView, after spacing: CGFloat? = nil) {
            if let spacing, let last = content.arrangedSubviews.last { content.setCustomSpacing(spacing, after: last) }
            content.addArrangedSubview(view)
            view.widthAnchor.constraint(equalTo: content.widthAnchor).isActive = true
        }
        func checkbox(_ label: String, note: String?, warning: Bool) -> NSButton {
            let box = NSButton(checkboxWithTitle: label, target: nil, action: nil)
            let title = NSMutableAttributedString(string: label, attributes: [.font: NSFont.systemFont(ofSize: 13), .foregroundColor: EmailTheme.ink])
            if let note {
                title.append(NSAttributedString(string: "  ·  \(note)", attributes: [.font: NSFont.systemFont(ofSize: 12), .foregroundColor: warning ? EmailTheme.warning : EmailTheme.muted]))
            }
            box.attributedTitle = title
            return box
        }

        add(text(disclosure["title"] as? String ?? "Agent access", size: 16, weight: .semibold))
        add(text(disclosure["lead"] as? String ?? "", color: EmailTheme.muted), after: 4)
        let read = disclosure["read"] as? [String: Any] ?? [:]
        let readBox = checkbox(read["label"] as? String ?? "Read mail and calendar", note: read["note"] as? String, warning: false)
        readBox.state = .on
        readBox.isEnabled = false
        readBox.setAccessibilityHelp("Required. Agents can read this account's data stored on this Mac.")
        add(readBox, after: 16)
        for grant in disclosure["grants"] as? [[String: Any]] ?? [] {
            guard let id = grant["id"] as? String else { continue }
            let box = checkbox(grant["label"] as? String ?? id, note: grant["note"] as? String, warning: grant["warning"] as? Bool == true)
            // Reading is the only default: every optional grant starts off.
            box.state = current?.contains(id) == true ? .on : .off
            box.toolTip = grant["detail"] as? String
            box.setAccessibilityHelp(grant["detail"] as? String)
            add(box, after: 9)
            boxes.append((id, box))
        }
        let callout = AgentCard(tint: EmailTheme.accentSoft)
        callout.column.addArrangedSubview(text(disclosure["provider_terms"] as? String ?? "", size: 12))
        add(callout, after: 16)
        add(text(disclosure["revoke"] as? String ?? "", size: 11.5, color: EmailTheme.muted), after: 10)
        let toggle = AgentDisclosureButton(title: disclosure["details_title"] as? String ?? "What exactly can agents see?", expanded: false, size: 12) {}
        toggle.handler = { [weak self] in self?.toggleDetails() }
        detailsToggle = toggle
        add(AgentAccessController.leading(toggle), after: 10)
        details.orientation = .vertical
        details.alignment = .leading
        details.spacing = 6
        let bullet = NSMutableParagraphStyle()
        bullet.headIndent = ("•  " as NSString).size(withAttributes: [.font: NSFont.systemFont(ofSize: 11.5)]).width
        for item in disclosure["details"] as? [String] ?? [] {
            let row = text("", size: 11.5)
            row.attributedStringValue = NSAttributedString(string: "•  \(item)", attributes: [.font: NSFont.systemFont(ofSize: 11.5), .foregroundColor: EmailTheme.ink, .paragraphStyle: bullet])
            details.addArrangedSubview(row)
            row.widthAnchor.constraint(equalTo: details.widthAnchor).isActive = true
        }
        details.isHidden = true
        add(details, after: 6)

        // Neither button is the default: Return never turns access on.
        cancelButton.keyEquivalent = "\u{1b}"
        cancelButton.handler = { [weak self] in self?.finish(nil) }
        confirmButton.applyPrimaryStyle()
        confirmButton.handler = { [weak self] in
            guard let self else { return }
            finish(boxes.filter { $0.1.state == .on }.map(\.0))
        }
        let spacer = NSView()
        spacer.setContentHuggingPriority(NSLayoutConstraint.Priority(1), for: .horizontal)
        let buttons = NSStackView(views: [spacer, cancelButton, confirmButton])
        buttons.spacing = 8
        buttons.alignment = .centerY
        for view in [scroll, buttons] as [NSView] {
            view.translatesAutoresizingMaskIntoConstraints = false
            root.addSubview(view)
        }
        NSLayoutConstraint.activate([
            document.widthAnchor.constraint(equalTo: scroll.contentView.widthAnchor),
            content.leadingAnchor.constraint(equalTo: document.leadingAnchor, constant: 24),
            content.trailingAnchor.constraint(equalTo: document.trailingAnchor, constant: -24),
            content.topAnchor.constraint(equalTo: document.topAnchor, constant: 22),
            content.bottomAnchor.constraint(equalTo: document.bottomAnchor, constant: -8),
            scroll.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            scroll.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            scroll.topAnchor.constraint(equalTo: root.topAnchor),
            scroll.bottomAnchor.constraint(equalTo: buttons.topAnchor, constant: -12),
            buttons.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 24),
            buttons.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -24),
            buttons.bottomAnchor.constraint(equalTo: root.bottomAnchor, constant: -18),
        ])
        panel.initialFirstResponder = cancelButton
        fit(animate: false)
    }

    /// Size the sheet to its content, up to what fits under the parent window.
    private func fit(animate: Bool) {
        content.layoutSubtreeIfNeeded()
        let maximum = max(380, (parent?.frame.height ?? 720) - 40)
        let height = min(content.fittingSize.height + 30 + 62, maximum)
        var frame = panel.frameRect(forContentRect: NSRect(x: 0, y: 0, width: Self.width, height: height))
        frame.origin = NSPoint(x: panel.frame.minX, y: panel.frame.maxY - frame.height)
        panel.setFrame(frame, display: true, animate: animate)
    }

    func toggleDetails() {
        detailsExpanded.toggle()
        details.isHidden = !detailsExpanded
        detailsToggle?.setExpanded(detailsExpanded)
        fit(animate: panel.isVisible)
    }

    func present(on parent: NSWindow, done: @escaping ([String]?) -> Void) {
        self.parent = parent
        self.done = done
        Self.presented = self
        fit(animate: false)
        parent.beginSheet(panel)
        panel.makeFirstResponder(cancelButton)
    }

    private func finish(_ grants: [String]?) {
        if let parent { parent.endSheet(panel) } else { panel.orderOut(nil) }
        let done = self.done
        self.done = nil
        Self.presented = nil
        done?(grants)
    }
}

@MainActor
final class AgentButton: NSButton {
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

/// A small text button in the accent color, such as an account's Edit.
@MainActor
private final class AgentLinkButton: NSButton {
    private let handler: () -> Void
    init(title: String, handler: @escaping () -> Void) {
        self.handler = handler
        super.init(frame: .zero)
        self.title = title
        isBordered = false
        target = self
        action = #selector(run)
        attributedTitle = NSAttributedString(string: title, attributes: [.font: NSFont.systemFont(ofSize: 11.5, weight: .semibold), .foregroundColor: EmailTheme.ink, .underlineStyle: NSUnderlineStyle.single.rawValue])
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    @objc private func run() { handler() }
}

/// A disclosure row: a title with a chevron that turns when it opens.
@MainActor
final class AgentDisclosureButton: NSButton {
    var handler: () -> Void
    private let size: CGFloat
    private let label: String
    init(title: String, expanded: Bool, size: CGFloat, handler: @escaping () -> Void) {
        self.handler = handler
        self.size = size
        self.label = title
        super.init(frame: .zero)
        self.title = title
        isBordered = false
        imagePosition = .imageTrailing
        alignment = .left
        target = self
        action = #selector(run)
        setButtonType(.momentaryChange)
        setExpanded(expanded)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    @objc private func run() { handler() }
    func setExpanded(_ expanded: Bool) {
        attributedTitle = NSAttributedString(string: label, attributes: [.font: NSFont.systemFont(ofSize: size, weight: .semibold), .foregroundColor: EmailTheme.ink])
        let configuration = NSImage.SymbolConfiguration(pointSize: size - 2, weight: .semibold)
        image = NSImage(systemSymbolName: expanded ? "chevron.down" : "chevron.right", accessibilityDescription: nil)?.withSymbolConfiguration(configuration)
        contentTintColor = EmailTheme.muted
        setAccessibilityValue(expanded ? "expanded" : "collapsed")
    }
}

/// The on/off switch on an account row.
@MainActor
private final class AgentSwitch: NSSwitch {
    var handler: (Bool) -> Void = { _ in }
    override init(frame: NSRect) {
        super.init(frame: frame)
        target = self
        action = #selector(changed)
        controlSize = .small
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    @objc private func changed() { handler(state == .on) }
}

/// The count next to "Waiting for you".
private final class AgentBadge: NSView {
    private let text: String
    init(text: String) {
        self.text = text
        super.init(frame: .zero)
        setContentHuggingPriority(.required, for: .horizontal)
        setContentCompressionResistancePriority(.required, for: .horizontal)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    private var attributes: [NSAttributedString.Key: Any] {
        [.font: NSFont.monospacedDigitSystemFont(ofSize: 11, weight: .semibold), .foregroundColor: EmailTheme.canvas]
    }
    override var intrinsicContentSize: NSSize {
        let size = (text as NSString).size(withAttributes: attributes)
        return NSSize(width: max(18, ceil(size.width) + 12), height: 18)
    }
    override func viewDidChangeEffectiveAppearance() { super.viewDidChangeEffectiveAppearance(); needsDisplay = true }
    override func draw(_ dirtyRect: NSRect) {
        EmailTheme.accent.setFill()
        NSBezierPath(roundedRect: bounds, xRadius: bounds.height / 2, yRadius: bounds.height / 2).fill()
        let size = (text as NSString).size(withAttributes: attributes)
        (text as NSString).draw(at: NSPoint(x: (bounds.width - size.width) / 2, y: (bounds.height - size.height) / 2), withAttributes: attributes)
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
