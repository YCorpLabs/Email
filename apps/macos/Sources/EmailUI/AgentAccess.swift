import AppKit

/// Agent access settings: what agents may do with each account, which agents
/// are connected, and the activity log. Requests live in the main window
/// (Agent requests); this window only links to them. The turn-on sheet's text
/// comes from mail-service so macOS and Linux show the same words. Only the
/// app can turn access on or approve a request; the MCP server can do neither.
@MainActor
final class AgentAccessController: NSWindowController, NSWindowDelegate {
    private let core: CoreBridge
    private let stack = NSStackView()
    private var accounts: [[String: Any]] = []
    private var history: [[String: Any]] = []
    private var waiting = 0
    private var levels: [[String: Any]] = []
    private var disclosureVersion: Any = 0
    /// The `agent_clients` answer: CLIs found, registrations, the helper.
    private var clients: [String: Any]?
    private var clientsLoading = false
    private var connecting: String?
    private var clientErrors: [String: String] = [:]
    /// Connected from this window this session: "Start a new session to use it".
    private var justConnected = Set<String>()
    private weak var setupButton: NSButton?
    private var presentingDialog = false
    var onReviewRequests: (() -> Void)?
    var onChanged: (() -> Void)?

    init(core: CoreBridge) {
        self.core = core
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 620, height: 540),
                              styleMask: [.titled, .closable, .resizable, .miniaturizable],
                              backing: .buffered, defer: false)
        window.title = "Agent access"
        window.minSize = NSSize(width: 480, height: 420)
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
        stack.spacing = 8
        let document = AgentFlippedView()
        document.translatesAutoresizingMaskIntoConstraints = false
        document.addSubview(stack)
        stack.translatesAutoresizingMaskIntoConstraints = false
        scroll.documentView = document
        let activity = AgentLinkButton(title: "Activity") { [weak self] in self?.showActivity() }
        let done = AgentButton(title: "Done") { [weak self] in self?.close() }
        done.applySecondaryStyle()
        done.keyEquivalent = "\u{1b}"
        done.toolTip = "Close (Esc)"
        let spacer = NSView()
        spacer.setContentHuggingPriority(NSLayoutConstraint.Priority(1), for: .horizontal)
        let footer = NSStackView(views: [activity, spacer, done])
        footer.alignment = .centerY
        let divider = AgentLine()
        for view in [scroll, divider, footer] as [NSView] {
            view.translatesAutoresizingMaskIntoConstraints = false
            root.addSubview(view)
        }
        NSLayoutConstraint.activate([
            document.widthAnchor.constraint(equalTo: scroll.contentView.widthAnchor),
            stack.leadingAnchor.constraint(equalTo: document.leadingAnchor, constant: 30),
            stack.trailingAnchor.constraint(equalTo: document.trailingAnchor, constant: -30),
            stack.topAnchor.constraint(equalTo: document.topAnchor, constant: 18),
            stack.bottomAnchor.constraint(equalTo: document.bottomAnchor, constant: -22),
            scroll.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            scroll.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            scroll.topAnchor.constraint(equalTo: root.topAnchor),
            scroll.bottomAnchor.constraint(equalTo: divider.topAnchor),
            divider.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            divider.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            divider.heightAnchor.constraint(equalToConstant: 1),
            divider.bottomAnchor.constraint(equalTo: footer.topAnchor, constant: -12),
            footer.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 30),
            footer.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -30),
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
        Task { await reload() }
        refreshClients()
    }

    func reload(force: Bool = true) async {
        guard let state = try? await core.callAsync(["op": "agent_access_overview", "history_limit": 60]) as? [String: Any] else { return }
        accounts = state["accounts"] as? [[String: Any]] ?? []
        history = state["history"] as? [[String: Any]] ?? []
        levels = state["levels"] as? [[String: Any]] ?? []
        disclosureVersion = state["disclosure_version"] ?? 0
        waiting = ((state["pending"] as? [String: Any])?["total"] as? NSNumber)?.intValue ?? 0
        render()
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
    /// window opens and after Connect.
    func refreshClients() {
        guard !clientsLoading else { return }
        clientsLoading = true
        Task { @MainActor [weak self] in
            guard let self else { return }
            clients = try? await core.callAsync(clientRequest("agent_clients")) as? [String: Any]
            clientsLoading = false
            render()
        }
    }

    // MARK: Rendering

    private func render() {
        stack.arrangedSubviews.forEach { stack.removeArrangedSubview($0); $0.removeFromSuperview() }
        if waiting > 0 {
            let banner = AgentGroup(tint: .systemBlue)
            let text = Self.label("\(waiting) request\(waiting == 1 ? "" : "s") waiting", size: 13)
            let review = AgentLinkButton(title: "Review") { [weak self] in self?.onReviewRequests?() }
            let row = NSStackView(views: [text, Self.label("·", size: 13, muted: true), review])
            row.spacing = 6
            row.alignment = .firstBaseline
            banner.add(row)
            add(banner)
            space(18)
        }
        add(Self.label("Agent access", size: 22, weight: .bold))
        space(2)
        add(Self.label("Let AI tools like Claude Code and Codex use your mail. Nothing is sent without your OK.", size: 13.5, muted: true))
        add(groupLabel("What agents can do"))
        let accountGroup = AgentGroup(tint: nil)
        for account in accounts { accountGroup.addRow(accountRow(account)) }
        if accounts.isEmpty { accountGroup.addRow(Self.label("Add a mail account first.", size: 13, muted: true)) }
        add(accountGroup)
        add(groupLabel("Agents"))
        let agents = AgentGroup(tint: nil)
        let rows = clients?["clients"] as? [[String: Any]] ?? [["id": "claude", "name": "Claude Code"], ["id": "codex", "name": "Codex"]]
        let helperFound = clients?["helper"] is String
        for row in rows {
            let id = row["id"] as? String ?? ""
            let name = row["name"] as? String ?? id
            var sub = clients == nil ? "" : row["subtitle"] as? String ?? ""
            var trailing: NSView?
            if row["connected"] as? Bool == true {
                trailing = Self.connectedLabel()
                if justConnected.contains(id) { sub = "Start a new session to use it" }
            } else if row["installed"] as? Bool == true {
                let button = AgentButton(title: connecting == id ? "Connecting…" : "Connect") { [weak self] in self?.connect(id: id, name: name) }
                button.applySecondaryStyle()
                button.isEnabled = helperFound && connecting == nil
                button.setAccessibilityLabel("Connect \(name)")
                trailing = button
            } else if clients != nil {
                sub = "Not installed"
            }
            if let error = clientErrors[id] { sub = error }
            agents.addRow(Self.listRow(title: name, sub: sub, trailing: trailing, subColor: clientErrors[id] == nil ? EmailTheme.muted : .systemRed))
        }
        let setup = AgentButton(title: "Set up…") { [weak self] in self?.copyManualConfig() }
        setup.applySecondaryStyle()
        setup.isEnabled = clients?["manual"] is [String: Any]
        setup.setAccessibilityLabel("Set up another app: copies the command and JSON config")
        setupButton = setup
        agents.addRow(Self.listRow(title: "Another app", sub: "Any app that supports MCP", trailing: setup))
        add(agents)
        if let problem = clients?["helper_problem"] as? String {
            space(8)
            add(Self.label(problem, size: 12, muted: true))
        }
    }

    private func add(_ view: NSView) {
        stack.addArrangedSubview(view)
        view.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true
    }

    private func space(_ points: CGFloat) {
        if let last = stack.arrangedSubviews.last { stack.setCustomSpacing(points, after: last) }
    }

    private func groupLabel(_ text: String) -> NSView {
        space(22)
        return Self.label(text, size: 12, weight: .semibold, muted: true)
    }

    private func accountRow(_ account: [String: Any]) -> NSView {
        let id = account["account_id"] as? String ?? ""
        let email = account["email"] as? String ?? id
        let level = account["level"] as? String ?? "off"
        let grants = (account["consent"] as? [String: Any])?["grants"] as? [String] ?? []
        let popup = AgentPopUp()
        let choices: [(String, String)] = [("off", "Off")] + levels.map { ($0["id"] as? String ?? "", $0["label"] as? String ?? "") } + [("custom", "Custom…")]
        for (choice, title) in choices {
            popup.addItem(withTitle: title)
            popup.lastItem?.representedObject = choice
        }
        popup.selectItem(at: choices.firstIndex { $0.0 == level } ?? 0)
        popup.setAccessibilityLabel("What agents can do with \(email)")
        popup.handler = { [weak self, weak popup] in
            guard let self, let chosen = popup?.selectedItem?.representedObject as? String else { return }
            self.choose(chosen, accountID: id, email: email, level: level, grants: grants)
        }
        return Self.listRow(title: email, sub: "", trailing: popup)
    }

    /// The popup's choice. Off asks first; turning on or allowing more opens
    /// the sheet with that choice; allowing less applies at once.
    private func choose(_ chosen: String, accountID: String, email: String, level: String, grants: [String]) {
        guard chosen != level || chosen == "custom" else { return }
        if chosen == "off" {
            guard let window else { return render() }
            presentingDialog = true
            AgentAccessDisclosure.confirmTurnOff(email, window) { [weak self] confirmed in
                guard let self else { return }
                presentingDialog = false
                guard confirmed else { return render() }
                Task { @MainActor [weak self] in
                    guard let self else { return }
                    _ = try? await core.callAsync(["op": "revoke_agent_access", "account_id": accountID])
                    await reload()
                    onChanged?()
                }
            }
            return
        }
        let preset = levels.first { $0["id"] as? String == chosen }?["grants"] as? [String]
        if let preset, level != "off", Set(preset).isSubset(of: Set(grants)) {
            Task { @MainActor [weak self] in
                guard let self else { return }
                _ = try? await core.callAsync(["op": "grant_agent_access", "account_id": accountID, "grants": preset, "disclosure_version": disclosureVersion])
                await reload()
                onChanged?()
            }
            return
        }
        guard let window else { return render() }
        AgentAccessFlow.allow(core: core, window: window, accountID: accountID, level: chosen,
                              grants: chosen == "custom" ? (grants.isEmpty ? ["sync"] : grants) : (preset ?? ["sync"]),
                              custom: chosen == "custom", changing: level != "off") { [weak self] _ in
            guard let self else { return }
            Task { @MainActor [weak self] in
                await self?.reload()
                self?.onChanged?()
            }
        }
    }

    private func connect(id: String, name: String) {
        guard connecting == nil else { return }
        connecting = id
        clientErrors[id] = nil
        render()
        Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                _ = try await core.callAsync(clientRequest("connect_agent_client", ["client": id]))
                justConnected.insert(id)
            } catch {
                clientErrors[id] = error.localizedDescription
            }
            connecting = nil
            refreshClients()
        }
    }

    private func copyManualConfig() {
        guard let text = (clients?["manual"] as? [String: Any])?["copy_text"] as? String else { return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
        guard let button = setupButton else { return }
        button.title = "Copied"
        DispatchQueue.main.asyncAfter(deadline: .now() + 2) { [weak button] in
            MainActor.assumeIsolated { button?.title = "Set up…" }
        }
    }

    private func showActivity() {
        guard let window else { return }
        AgentActivitySheet(history: history).present(on: window)
    }

    // MARK: Building blocks

    static func label(_ text: String, size: CGFloat, weight: NSFont.Weight = .regular, muted: Bool = false) -> NSTextField {
        let field = NSTextField(wrappingLabelWithString: text)
        field.font = .systemFont(ofSize: size, weight: weight)
        field.textColor = muted ? EmailTheme.muted : EmailTheme.ink
        field.isSelectable = false
        field.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        return field
    }

    static func connectedLabel() -> NSView {
        let field = NSTextField(labelWithString: "✓ Connected")
        field.font = .systemFont(ofSize: 12.5, weight: .semibold)
        field.textColor = .systemGreen
        field.setAccessibilityLabel("Connected")
        return field
    }

    static func listRow(title: String, sub: String, trailing: NSView?, subColor: NSColor = EmailTheme.muted) -> NSView {
        let names = NSStackView()
        names.orientation = .vertical
        names.alignment = .leading
        names.spacing = 1
        let heading = label(title, size: 13)
        heading.lineBreakMode = .byTruncatingMiddle
        heading.maximumNumberOfLines = 1
        names.addArrangedSubview(heading)
        if !sub.isEmpty {
            let detail = label(sub, size: 12)
            detail.textColor = subColor
            names.addArrangedSubview(detail)
        }
        names.setContentHuggingPriority(NSLayoutConstraint.Priority(1), for: .horizontal)
        names.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        let spacer = NSView()
        spacer.setContentHuggingPriority(NSLayoutConstraint.Priority(1), for: .horizontal)
        let row = NSStackView(views: [names, spacer] + (trailing.map { [$0] } ?? []))
        row.alignment = .centerY
        row.spacing = 12
        trailing?.setContentHuggingPriority(.defaultHigh, for: .horizontal)
        trailing?.setContentCompressionResistancePriority(.required, for: .horizontal)
        return row
    }

    func windowWillClose(_ notification: Notification) {}
}

/// Turning an account on or allowing more, from the settings popup or from
/// Allow… on a request: fetch the sheet's text, show it, record the choice.
@MainActor
enum AgentAccessFlow {
    static func allow(core: CoreBridge, window: NSWindow, accountID: String, level: String, grants: [String],
                      custom: Bool, changing: Bool, done: @escaping (Bool) -> Void) {
        Task { @MainActor in
            guard let disclosure = try? await core.callAsync(["op": "agent_access_disclosure", "account_id": accountID]) as? [String: Any] else {
                return done(false)
            }
            AgentAccessDisclosure.choose(disclosure, AgentSheetChoice(level: level, grants: grants, custom: custom, changing: changing), window) { chosen in
                guard let chosen else { return done(false) }
                Task { @MainActor in
                    let granted = (try? await core.callAsync(["op": "grant_agent_access", "account_id": accountID, "grants": chosen,
                                                                "disclosure_version": disclosure["version"] ?? 0])) != nil
                    done(granted)
                }
            }
        }
    }
}

/// What the sheet starts with.
struct AgentSheetChoice {
    var level: String
    var grants: [String]
    var custom: Bool
    var changing: Bool
}

/// The turn-on sheet and the turn-off confirmation.
@MainActor
enum AgentAccessDisclosure {
    /// Replaceable only by the AppKit check harness. `done` gets the chosen
    /// grants, or nil when cancelled.
    static var choose: (_ disclosure: [String: Any], _ initial: AgentSheetChoice, _ parent: NSWindow, _ done: @escaping ([String]?) -> Void) -> Void = { disclosure, initial, parent, done in
        AgentAccessSheet(disclosure: disclosure, initial: initial).present(on: parent, done: done)
    }
    static var confirmTurnOff: (_ email: String, _ parent: NSWindow, _ done: @escaping (Bool) -> Void) -> Void = { email, parent, done in
        makeTurnOffAlert(email).beginSheetModal(for: parent) { done($0 == .alertSecondButtonReturn) }
    }

    static func makeTurnOffAlert(_ email: String) -> NSAlert {
        let alert = NSAlert()
        alert.messageText = "Turn off agent access for \(email)?"
        alert.informativeText = "Agents lose access right away, and requests waiting for you are dropped. Mail you already approved stays in the Outbox."
        // Cancel first: Escape cancels and Return never confirms.
        alert.addButton(withTitle: "Cancel")
        alert.addButton(withTitle: "Turn off")
        return alert
    }
}

/// "Let agents use {account}?": three choices with Read only as the default,
/// the provider-terms note, Custom… for the individual switches (calendar
/// changes only there) and "What can agents see?" for the full list.
@MainActor
final class AgentAccessSheet: NSObject {
    /// The sheet being shown; it owns itself until it closes.
    private(set) static var presented: AgentAccessSheet?
    let panel: NSWindow
    private(set) var options: [(id: String, view: AgentOption)] = []
    private(set) var switches: [(id: String, control: NSSwitch)] = []
    let cancelButton: AgentButton
    let confirmButton: AgentButton
    let customLink: AgentLinkButton
    let detailsLink: AgentLinkButton
    private let content = NSStackView()
    private let customBox = NSStackView()
    private let detailsBox = NSStackView()
    private let scroll = NSScrollView()
    private var levels: [(id: String, grants: [String])] = []
    private(set) var customExpanded = false
    private(set) var detailsExpanded = false
    private var done: (([String]?) -> Void)?
    private weak var parent: NSWindow?
    private static let width: CGFloat = 470

    init(disclosure: [String: Any], initial: AgentSheetChoice) {
        panel = NSWindow(contentRect: NSRect(x: 0, y: 0, width: Self.width, height: 400), styleMask: [.titled], backing: .buffered, defer: false)
        cancelButton = AgentButton(title: disclosure["cancel"] as? String ?? "Cancel") {}
        confirmButton = AgentButton(title: (initial.changing ? disclosure["change_confirm"] : disclosure["confirm"]) as? String ?? "Allow") {}
        customLink = AgentLinkButton(title: disclosure["custom_link"] as? String ?? "Custom…") {}
        detailsLink = AgentLinkButton(title: disclosure["details_link"] as? String ?? "What can agents see?") {}
        super.init()
        panel.title = "Agent access"
        panel.backgroundColor = EmailTheme.reader
        let root = AgentRoot()
        panel.contentView = root
        content.orientation = .vertical
        content.alignment = .leading
        content.spacing = 8
        content.translatesAutoresizingMaskIntoConstraints = false
        let document = AgentFlippedView()
        document.translatesAutoresizingMaskIntoConstraints = false
        document.addSubview(content)
        scroll.documentView = document
        scroll.drawsBackground = false
        scroll.hasVerticalScroller = true
        scroll.autohidesScrollers = true
        func add(_ view: NSView, after spacing: CGFloat? = nil) {
            if let spacing, let last = content.arrangedSubviews.last { content.setCustomSpacing(spacing, after: last) }
            content.addArrangedSubview(view)
            view.widthAnchor.constraint(equalTo: content.widthAnchor).isActive = true
        }
        add(AgentAccessController.label(disclosure["title"] as? String ?? "Agent access", size: 16, weight: .bold))
        for level in disclosure["levels"] as? [[String: Any]] ?? [] {
            let id = level["id"] as? String ?? ""
            levels.append((id, level["grants"] as? [String] ?? []))
            let option = AgentOption(title: level["label"] as? String ?? id, sub: level["sub"] as? String ?? "") { [weak self] in self?.pick(level: id) }
            options.append((id, option))
            add(option, after: options.count == 1 ? 12 : nil)
        }
        customBox.orientation = .vertical
        customBox.alignment = .leading
        customBox.spacing = 9
        for grant in disclosure["custom"] as? [[String: Any]] ?? [] {
            let id = grant["id"] as? String ?? ""
            let control = NSSwitch()
            control.controlSize = .mini
            control.target = self
            control.action = #selector(switchChanged(_:))
            control.setAccessibilityLabel(grant["label"] as? String)
            control.toolTip = grant["detail"] as? String
            let text = NSMutableAttributedString(string: grant["label"] as? String ?? id, attributes: [.font: NSFont.systemFont(ofSize: 13), .foregroundColor: EmailTheme.ink])
            if let note = grant["note"] as? String {
                text.append(NSAttributedString(string: "  ·  \(note)", attributes: [.font: NSFont.systemFont(ofSize: 12), .foregroundColor: grant["warning"] as? Bool == true ? EmailTheme.warning : EmailTheme.muted]))
            }
            let label = NSTextField(labelWithAttributedString: text)
            label.toolTip = grant["detail"] as? String
            let row = NSStackView(views: [control, label])
            row.spacing = 9
            row.alignment = .centerY
            customBox.addArrangedSubview(row)
            switches.append((id, control))
        }
        customBox.isHidden = true
        add(customBox, after: 12)
        add(AgentAccessController.label(disclosure["note"] as? String ?? "", size: 12.5, muted: true), after: 14)
        detailsBox.orientation = .vertical
        detailsBox.alignment = .leading
        detailsBox.spacing = 6
        let bullet = NSMutableParagraphStyle()
        bullet.headIndent = ("•  " as NSString).size(withAttributes: [.font: NSFont.systemFont(ofSize: 11.5)]).width
        for item in disclosure["details"] as? [String] ?? [] {
            let row = AgentAccessController.label("", size: 11.5)
            row.attributedStringValue = NSAttributedString(string: "•  \(item)", attributes: [.font: NSFont.systemFont(ofSize: 11.5), .foregroundColor: EmailTheme.ink, .paragraphStyle: bullet])
            detailsBox.addArrangedSubview(row)
            row.widthAnchor.constraint(equalTo: detailsBox.widthAnchor).isActive = true
        }
        detailsBox.isHidden = true
        add(detailsBox, after: 10)

        customLink.handler = { [weak self] in self?.toggleCustom() }
        detailsLink.handler = { [weak self] in self?.toggleDetails() }
        // Neither button is the default: Return never allows access.
        cancelButton.keyEquivalent = "\u{1b}"
        cancelButton.applySecondaryStyle()
        cancelButton.handler = { [weak self] in self?.finish(nil) }
        confirmButton.applyPrimaryStyle()
        confirmButton.handler = { [weak self] in
            guard let self else { return }
            finish(grants)
        }
        let dot = AgentAccessController.label("·", size: 12.5, muted: true)
        let spacer = NSView()
        spacer.setContentHuggingPriority(NSLayoutConstraint.Priority(1), for: .horizontal)
        let actions = NSStackView(views: [customLink, dot, detailsLink, spacer, cancelButton, confirmButton])
        actions.spacing = 8
        actions.alignment = .centerY
        for view in [scroll, actions] as [NSView] {
            view.translatesAutoresizingMaskIntoConstraints = false
            root.addSubview(view)
        }
        NSLayoutConstraint.activate([
            document.widthAnchor.constraint(equalTo: scroll.contentView.widthAnchor),
            content.leadingAnchor.constraint(equalTo: document.leadingAnchor, constant: 24),
            content.trailingAnchor.constraint(equalTo: document.trailingAnchor, constant: -24),
            content.topAnchor.constraint(equalTo: document.topAnchor, constant: 22),
            content.bottomAnchor.constraint(equalTo: document.bottomAnchor, constant: -4),
            scroll.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            scroll.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            scroll.topAnchor.constraint(equalTo: root.topAnchor),
            scroll.bottomAnchor.constraint(equalTo: actions.topAnchor, constant: -16),
            actions.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 24),
            actions.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -24),
            actions.bottomAnchor.constraint(equalTo: root.bottomAnchor, constant: -18),
        ])
        set(grants: initial.grants)
        if initial.custom { toggleCustom() }
        panel.initialFirstResponder = cancelButton
        fit(animate: false)
    }

    /// The grants the switches show; the radio follows when they match a level.
    var grants: [String] { switches.filter { $0.control.state == .on }.map(\.id) }
    var selectedLevel: String? { options.first { $0.view.selected }?.id }

    private func set(grants: [String]) {
        for (id, control) in switches { control.state = grants.contains(id) ? .on : .off }
        syncOptions()
    }

    private func syncOptions() {
        let current = Set(grants)
        for (id, view) in options { view.selected = levels.first { $0.id == id }.map { Set($0.grants) == current } ?? false }
    }

    func pick(level: String) {
        set(grants: levels.first { $0.id == level }?.grants ?? [])
    }

    @objc private func switchChanged(_ sender: NSSwitch) { syncOptions() }

    func toggleCustom() {
        customExpanded.toggle()
        customBox.isHidden = !customExpanded
        fit(animate: panel.isVisible)
    }

    func toggleDetails() {
        detailsExpanded.toggle()
        detailsBox.isHidden = !detailsExpanded
        fit(animate: panel.isVisible)
    }

    /// Size the sheet to its content, up to what fits under the parent window.
    private func fit(animate: Bool) {
        content.layoutSubtreeIfNeeded()
        let maximum = max(380, (parent?.frame.height ?? 720) - 40)
        let height = min(content.fittingSize.height + 26 + 62, maximum)
        var frame = panel.frameRect(forContentRect: NSRect(x: 0, y: 0, width: Self.width, height: height))
        frame.origin = NSPoint(x: panel.frame.minX, y: panel.frame.maxY - frame.height)
        panel.setFrame(frame, display: true, animate: animate)
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

/// One of the sheet's three choices: a radio, a title and a line.
@MainActor
final class AgentOption: NSButton {
    private let handler: () -> Void
    private let radio = AgentRadio()
    var selected = false { didSet { radio.on = selected; needsDisplay = true; setAccessibilityValue(selected ? "selected" : "") } }
    init(title: String, sub: String, handler: @escaping () -> Void) {
        self.handler = handler
        super.init(frame: .zero)
        self.title = ""
        isBordered = false
        target = self
        action = #selector(run)
        setAccessibilityLabel(title)
        setButtonType(.momentaryChange)
        let heading = NSTextField(labelWithString: title)
        heading.font = .systemFont(ofSize: 13, weight: .semibold)
        heading.textColor = EmailTheme.ink
        let detail = NSTextField(wrappingLabelWithString: sub)
        detail.font = .systemFont(ofSize: 12)
        detail.textColor = EmailTheme.muted
        detail.isSelectable = false
        let text = NSStackView(views: [heading, detail])
        text.orientation = .vertical
        text.alignment = .leading
        text.spacing = 1
        let row = NSStackView(views: [radio, text])
        row.alignment = .top
        row.spacing = 11
        row.translatesAutoresizingMaskIntoConstraints = false
        addSubview(row)
        NSLayoutConstraint.activate([
            row.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 12), row.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -12),
            row.topAnchor.constraint(equalTo: topAnchor, constant: 10), row.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -10),
            radio.widthAnchor.constraint(equalToConstant: 15), radio.heightAnchor.constraint(equalToConstant: 15),
        ])
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    @objc private func run() { handler() }
    override var isFlipped: Bool { true }
    override func viewDidChangeEffectiveAppearance() { super.viewDidChangeEffectiveAppearance(); needsDisplay = true }
    override func draw(_ dirtyRect: NSRect) {
        let path = NSBezierPath(roundedRect: bounds.insetBy(dx: 0.5, dy: 0.5), xRadius: 9, yRadius: 9)
        (selected ? NSColor.controlAccentColor.withAlphaComponent(0.12) : EmailTheme.reader).setFill()
        path.fill()
        (selected ? NSColor.controlAccentColor : EmailTheme.line).setStroke()
        path.lineWidth = 1
        path.stroke()
    }
}

private final class AgentRadio: NSView {
    var on = false { didSet { needsDisplay = true } }
    override func viewDidChangeEffectiveAppearance() { super.viewDidChangeEffectiveAppearance(); needsDisplay = true }
    override func draw(_ dirtyRect: NSRect) {
        let circle = NSBezierPath(ovalIn: bounds.insetBy(dx: 1, dy: 1))
        if on {
            NSColor.controlAccentColor.setFill()
            circle.fill()
            NSColor.white.setFill()
            NSBezierPath(ovalIn: bounds.insetBy(dx: 5, dy: 5)).fill()
        } else {
            EmailTheme.muted.setStroke()
            circle.lineWidth = 1.5
            circle.stroke()
        }
    }
}

/// The activity log: decisions, changes agents made, consent changes and
/// refusals, newest first.
@MainActor
final class AgentActivitySheet: NSObject {
    private static var presented: AgentActivitySheet?
    let panel: NSWindow
    private weak var parent: NSWindow?

    init(history: [[String: Any]]) {
        panel = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 520, height: 460), styleMask: [.titled], backing: .buffered, defer: false)
        super.init()
        panel.backgroundColor = EmailTheme.reader
        let root = AgentRoot()
        panel.contentView = root
        let stack = NSStackView()
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 10
        stack.translatesAutoresizingMaskIntoConstraints = false
        stack.addArrangedSubview(AgentAccessController.label("Activity", size: 16, weight: .bold))
        if history.isEmpty { stack.addArrangedSubview(AgentAccessController.label("Nothing yet.", size: 12.5, muted: true)) }
        for item in history { stack.addArrangedSubview(Self.row(item)) }
        let document = AgentFlippedView()
        document.translatesAutoresizingMaskIntoConstraints = false
        document.addSubview(stack)
        let scroll = NSScrollView()
        scroll.documentView = document
        scroll.drawsBackground = false
        scroll.hasVerticalScroller = true
        scroll.autohidesScrollers = true
        let done = AgentButton(title: "Done") { [weak self] in self?.finish() }
        done.applySecondaryStyle()
        done.keyEquivalent = "\u{1b}"
        for view in [scroll, done] as [NSView] {
            view.translatesAutoresizingMaskIntoConstraints = false
            root.addSubview(view)
        }
        NSLayoutConstraint.activate([
            document.widthAnchor.constraint(equalTo: scroll.contentView.widthAnchor),
            stack.leadingAnchor.constraint(equalTo: document.leadingAnchor, constant: 24),
            stack.trailingAnchor.constraint(equalTo: document.trailingAnchor, constant: -24),
            stack.topAnchor.constraint(equalTo: document.topAnchor, constant: 20),
            stack.bottomAnchor.constraint(equalTo: document.bottomAnchor, constant: -10),
            scroll.leadingAnchor.constraint(equalTo: root.leadingAnchor), scroll.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            scroll.topAnchor.constraint(equalTo: root.topAnchor), scroll.bottomAnchor.constraint(equalTo: done.topAnchor, constant: -12),
            done.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -24), done.bottomAnchor.constraint(equalTo: root.bottomAnchor, constant: -16),
        ])
    }

    static func row(_ item: [String: Any]) -> NSView {
        let column = NSStackView()
        column.orientation = .vertical
        column.alignment = .leading
        column.spacing = 2
        let when = Date(timeIntervalSince1970: ((item["decided_at_ms"] as? NSNumber ?? item["created_at_ms"] as? NSNumber)?.doubleValue ?? 0) / 1000)
            .formatted(date: .abbreviated, time: .shortened)
        var meta = "\(when) · \(statusTitle(item["status"] as? String ?? ""))"
        if let client = item["client"] as? String, !client.isEmpty { meta += " · \(client)" }
        column.addArrangedSubview(AgentAccessController.label(meta, size: 10.5, muted: true))
        column.addArrangedSubview(AgentAccessController.label(item["summary"] as? String ?? "", size: 12))
        if let error = item["error"] as? String, !error.isEmpty {
            let detail = AgentAccessController.label(error, size: 11, muted: true)
            if item["status"] as? String == "failed" { detail.textColor = .systemRed }
            column.addArrangedSubview(detail)
        }
        return column
    }

    static func statusTitle(_ status: String) -> String {
        switch status {
        case "approved": return "Approved"
        case "rejected": return "Declined"
        case "edited": return "Opened to edit"
        case "failed": return "Could not apply"
        case "blocked": return "Blocked"
        case "replaced": return "Replaced by a newer request"
        case "pending": return "Waiting"
        default: return "Done"
        }
    }

    func present(on parent: NSWindow) {
        self.parent = parent
        Self.presented = self
        parent.beginSheet(panel)
    }

    private func finish() {
        parent?.endSheet(panel)
        Self.presented = nil
    }
}

@MainActor
final class AgentButton: NSButton {
    var handler: () -> Void
    private enum Style { case system, primary, secondary, ghost }
    private var style = Style.system
    private var hint: String?
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
    /// The plain label; a styled button draws it (and its hint) itself.
    private var label = ""
    override var title: String {
        get { style == .system ? super.title : label }
        set {
            label = newValue
            if style == .system { super.title = newValue } else { paint() }
        }
    }
    override var isEnabled: Bool { didSet { if style != .system { paint() } } }

    /// Filled with the accent: the action the strip or sheet is for.
    func applyPrimaryStyle(hint: String? = nil) { self.hint = hint; adopt(.primary) }
    /// A soft fill, like the mockup's ordinary buttons.
    func applySecondaryStyle() { adopt(.secondary) }
    /// Text only: the "don't" choice.
    func applyGhostStyle() { adopt(.ghost) }

    private func adopt(_ style: Style) {
        self.style = style
        isBordered = false
        wantsLayer = true
        layer?.cornerRadius = 7
        heightAnchor.constraint(equalToConstant: 28).isActive = true
        paint()
    }

    private func paint() {
        let dark = EmailTheme.isDark(effectiveAppearance)
        let (fill, ink): (NSColor, NSColor) = switch style {
        case .primary: (EmailTheme.accent, dark ? EmailTheme.canvas : .white)
        case .secondary: (EmailTheme.accentSoft, EmailTheme.ink)
        case .ghost, .system: (.clear, EmailTheme.ink.withAlphaComponent(0.8))
        }
        layer?.backgroundColor = EmailTheme.cgColor(fill, in: effectiveAppearance)
        alphaValue = isEnabled ? 1 : 0.55
        let font = NSFont.systemFont(ofSize: 12.5, weight: style == .primary ? .semibold : .medium)
        let text = NSMutableAttributedString(string: label, attributes: [.foregroundColor: ink, .font: font])
        if let hint {
            text.append(NSAttributedString(string: "  \(hint)", attributes: [.foregroundColor: ink.withAlphaComponent(0.55), .font: NSFont.systemFont(ofSize: 10.5)]))
        }
        attributedTitle = text
        let width = text.size().width + 26
        if let existing = constraints.first(where: { $0.identifier == "agent-button-width" }) { existing.constant = width } else {
            let constraint = widthAnchor.constraint(greaterThanOrEqualToConstant: width)
            constraint.identifier = "agent-button-width"
            constraint.isActive = true
        }
    }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        if style != .system { paint() }
    }
}

/// A text link in the accent color, such as Activity, Review or Custom….
@MainActor
final class AgentLinkButton: NSButton {
    var handler: () -> Void
    init(title: String, handler: @escaping () -> Void) {
        self.handler = handler
        super.init(frame: .zero)
        self.title = title
        isBordered = false
        target = self
        action = #selector(run)
        attributedTitle = NSAttributedString(string: title, attributes: [.font: NSFont.systemFont(ofSize: 12.5), .foregroundColor: NSColor.linkColor])
        setContentHuggingPriority(.required, for: .horizontal)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    @objc private func run() { handler() }
}

/// The account popup; runs `handler` when the choice changes.
@MainActor
private final class AgentPopUp: NSPopUpButton {
    var handler: () -> Void = {}
    init() {
        super.init(frame: .zero, pullsDown: false)
        target = self
        action = #selector(changed)
        controlSize = .regular
        font = .systemFont(ofSize: 12.5)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    @objc private func changed() { handler() }
}

/// A rounded group of rows with hairlines between them.
@MainActor
private final class AgentGroup: NSView {
    private let stack = NSStackView()
    private let tint: NSColor?
    init(tint: NSColor?) {
        self.tint = tint
        super.init(frame: .zero)
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 0
        stack.translatesAutoresizingMaskIntoConstraints = false
        addSubview(stack)
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 14), stack.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -14),
            stack.topAnchor.constraint(equalTo: topAnchor), stack.bottomAnchor.constraint(equalTo: bottomAnchor),
        ])
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    override var isFlipped: Bool { true }
    override func viewDidChangeEffectiveAppearance() { super.viewDidChangeEffectiveAppearance(); needsDisplay = true }
    override func draw(_ dirtyRect: NSRect) {
        let path = NSBezierPath(roundedRect: bounds, xRadius: 10, yRadius: 10)
        (tint.map { $0.withAlphaComponent(0.16) } ?? EmailTheme.inbox).setFill()
        path.fill()
    }

    /// A padded row.
    func add(_ view: NSView) { addRow(view) }

    func addRow(_ view: NSView) {
        if !stack.arrangedSubviews.isEmpty {
            let line = AgentLine()
            stack.addArrangedSubview(line)
            line.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true
            line.heightAnchor.constraint(equalToConstant: 1).isActive = true
        }
        let holder = NSView()
        view.translatesAutoresizingMaskIntoConstraints = false
        holder.addSubview(view)
        NSLayoutConstraint.activate([
            view.leadingAnchor.constraint(equalTo: holder.leadingAnchor), view.trailingAnchor.constraint(equalTo: holder.trailingAnchor),
            view.topAnchor.constraint(equalTo: holder.topAnchor, constant: 11), view.bottomAnchor.constraint(equalTo: holder.bottomAnchor, constant: -11),
        ])
        stack.addArrangedSubview(holder)
        holder.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true
    }
}

/// Paints the window surface itself so it follows light and dark appearance.
final class AgentRoot: NSView {
    override func viewDidChangeEffectiveAppearance() { super.viewDidChangeEffectiveAppearance(); needsDisplay = true }
    override func draw(_ dirtyRect: NSRect) { EmailTheme.reader.setFill(); bounds.fill() }
}
