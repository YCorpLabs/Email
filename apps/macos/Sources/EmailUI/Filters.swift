import AppKit

// Filters (docs/filters.md): the service's state as the window uses it, and
// the three sheets (first-run setup, the filter editor and Filter settings).
// The sheets reuse the Agent access building blocks so they look the same.

/// What a filter does with mail when it arrives.
enum MailFilterAction: String, CaseIterable, Sendable {
    case inbox
    case skipInbox = "skip_inbox"
    case ignore

    init(_ value: Any?) { self = MailFilterAction(rawValue: value as? String ?? "") ?? .inbox }

    var title: String {
        switch self {
        case .inbox: "Keep in inbox"
        case .skipInbox: "Skip inbox"
        case .ignore: "Ignore"
        }
    }

    var symbol: String {
        switch self {
        case .inbox: "tag"
        case .skipInbox: "tray.and.arrow.down"
        case .ignore: "eye.slash"
        }
    }

    /// The line under "When it arrives" in the editor.
    var explanation: String {
        switch self {
        case .inbox: "Stays in your inbox with a tag."
        case .skipInbox: "Archived. Read it here when you want."
        case .ignore: "Archived and marked read."
        }
    }

    /// The start of the filter view's subtitle.
    var summary: String {
        switch self {
        case .inbox: "Tagged in your inbox"
        case .skipInbox: "Skips your inbox"
        case .ignore: "Ignored"
        }
    }

    var leavesInbox: Bool { self != .inbox }
}

/// The filter on a thread row (`filter` in every thread summary).
struct ThreadFilter: Sendable, Equatable {
    let id: String
    let name: String
    let action: MailFilterAction
    /// The model's few words; empty for mail sorted by hand.
    let reason: String
    let source: String

    init?(_ value: Any?) {
        guard let value = value as? [String: Any], let id = value["id"] as? String, let name = value["name"] as? String else { return nil }
        self.id = id
        self.name = name
        action = MailFilterAction(value["action"])
        reason = (value["reason"] as? String ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        source = value["source"] as? String ?? "model"
    }
}

struct MailFilterInfo: Sendable, Equatable {
    let id: String
    let name: String
    let description: String
    let action: MailFilterAction
    let unread: Int
    let lastWeek: Int
}

struct MailFilterStarter: Sendable, Equatable {
    let key: String
    let name: String
    let description: String
    let action: MailFilterAction
}

/// `filters_status`.
struct FilterStatus: Sendable, Equatable {
    let enabled: Bool
    let filters: [MailFilterInfo]
    let starters: [MailFilterStarter]
    let modelName: String
    let downloadBytes: Int64
    let installed: Bool
    let downloading: Bool
    let downloadedBytes: Int64
    let downloadError: String?
    let sortingError: String?

    init?(_ value: Any?) {
        guard let value = value as? [String: Any] else { return nil }
        enabled = value["enabled"] as? Bool ?? false
        filters = (value["filters"] as? [[String: Any]] ?? []).compactMap { item in
            guard let id = item["id"] as? String, let name = item["name"] as? String else { return nil }
            return MailFilterInfo(id: id, name: name, description: item["description"] as? String ?? "", action: MailFilterAction(item["action"]),
                                  unread: (item["unread"] as? NSNumber)?.intValue ?? 0, lastWeek: (item["last_7_days"] as? NSNumber)?.intValue ?? 0)
        }
        starters = (value["starters"] as? [[String: Any]] ?? []).compactMap { item in
            guard let key = item["key"] as? String, let name = item["name"] as? String else { return nil }
            return MailFilterStarter(key: key, name: name, description: item["description"] as? String ?? "", action: MailFilterAction(item["action"]))
        }
        let model = value["model"] as? [String: Any] ?? [:]
        modelName = model["model_name"] as? String ?? "Email sorter"
        downloadBytes = (model["download_bytes"] as? NSNumber)?.int64Value ?? 0
        installed = model["installed"] as? Bool ?? false
        downloading = model["downloading"] as? Bool ?? false
        downloadedBytes = (model["downloaded_bytes"] as? NSNumber)?.int64Value ?? 0
        downloadError = model["error"] as? String
        sortingError = (value["sorting"] as? [String: Any])?["error"] as? String
    }

    /// 0…1 while downloading.
    var progress: Double { downloadBytes > 0 ? min(1, Double(downloadedBytes) / Double(downloadBytes)) : 0 }
    var percent: Int { Int((progress * 100).rounded(.down)) }
    var size: String { FilterStatus.bytes(downloadBytes) }

    /// "2.5 GB" (one decimal, in the Mac's number format), or whole MB.
    static func bytes(_ count: Int64) -> String {
        let gigabytes = Double(count) / 1_000_000_000
        if gigabytes >= 1 { return gigabytes.formatted(.number.precision(.fractionLength(1))) + " GB" }
        return (Double(count) / 1_000_000).formatted(.number.precision(.fractionLength(0))) + " MB"
    }

    /// The model as Filter settings names it.
    var modelLine: String { "\(modelName) · \(size) · runs on this Mac" }
}

/// Shared pieces of the filter sheets.
@MainActor
enum FilterSheetParts {
    static let width: CGFloat = 470

    static func panel(title: String) -> (NSWindow, AgentRoot) {
        let panel = NSWindow(contentRect: NSRect(x: 0, y: 0, width: width, height: 360), styleMask: [.titled], backing: .buffered, defer: false)
        panel.title = title
        panel.backgroundColor = EmailTheme.reader
        let root = AgentRoot()
        panel.contentView = root
        return (panel, root)
    }

    static func actionPopUp(_ selected: MailFilterAction) -> AgentPopUp {
        let popup = AgentPopUp()
        for action in MailFilterAction.allCases {
            popup.addItem(withTitle: action.title)
            popup.lastItem?.representedObject = action.rawValue
        }
        popup.selectItem(at: MailFilterAction.allCases.firstIndex(of: selected) ?? 0)
        return popup
    }

    static func action(of popup: NSPopUpButton) -> MailFilterAction {
        MailFilterAction(popup.selectedItem?.representedObject)
    }

    /// The bottom row: optional leading views, then the buttons at the right.
    static func actions(leading: [NSView] = [], trailing: [NSView]) -> NSStackView {
        let spacer = NSView()
        spacer.setContentHuggingPriority(NSLayoutConstraint.Priority(1), for: .horizontal)
        let row = NSStackView(views: leading + [spacer] + trailing)
        row.spacing = 8
        row.alignment = .centerY
        return row
    }

    /// Lay out `content` above `actions` with the Agent access sheet margins.
    static func install(_ content: NSStackView, actions: NSView, in root: NSView) {
        for view in [content, actions] {
            view.translatesAutoresizingMaskIntoConstraints = false
            root.addSubview(view)
        }
        NSLayoutConstraint.activate([
            content.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 24),
            content.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -24),
            content.topAnchor.constraint(equalTo: root.topAnchor, constant: 22),
            actions.topAnchor.constraint(equalTo: content.bottomAnchor, constant: 20),
            actions.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 24),
            actions.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -24),
            actions.bottomAnchor.constraint(equalTo: root.bottomAnchor, constant: -18),
        ])
    }

    static func stack() -> NSStackView {
        let content = NSStackView()
        content.orientation = .vertical
        content.alignment = .leading
        content.spacing = 8
        return content
    }

    static func add(_ view: NSView, to content: NSStackView, after spacing: CGFloat? = nil) {
        if let spacing, let last = content.arrangedSubviews.last { content.setCustomSpacing(spacing, after: last) }
        content.addArrangedSubview(view)
        view.widthAnchor.constraint(equalTo: content.widthAnchor).isActive = true
    }

    /// Size a sheet to its content at the fixed width.
    static func fit(_ panel: NSWindow) {
        guard let root = panel.contentView else { return }
        root.layoutSubtreeIfNeeded()
        let height = root.fittingSize.height
        var frame = panel.frameRect(forContentRect: NSRect(x: 0, y: 0, width: width, height: height))
        frame.origin = NSPoint(x: panel.frame.minX, y: panel.frame.maxY - frame.height)
        panel.setFrame(frame, display: true)
    }
}

/// "Sort mail automatically": the four starter filters with an action each,
/// the download size, and Download and turn on. It closes at once; the
/// download continues in the sidebar.
@MainActor
final class FilterSetupSheet: NSObject {
    private(set) static var presented: FilterSetupSheet?
    let panel: NSWindow
    let cancelButton: AgentButton
    let confirmButton: AgentButton
    private(set) var rows: [(starter: MailFilterStarter, check: NSButton, popup: AgentPopUp)] = []
    private var done: (([(starter: MailFilterStarter, action: MailFilterAction)]?) -> Void)?
    private weak var parent: NSWindow?

    init(status: FilterStatus) {
        let (panel, root) = FilterSheetParts.panel(title: "Sort mail automatically")
        self.panel = panel
        cancelButton = AgentButton(title: "Not now") {}
        confirmButton = AgentButton(title: "Download and turn on") {}
        super.init()
        let content = FilterSheetParts.stack()
        FilterSheetParts.add(AgentAccessController.label("Sort mail automatically", size: 16, weight: .bold), to: content)
        FilterSheetParts.add(AgentAccessController.label("A small AI model on this Mac sorts first emails from people you haven’t written to. Your mail never leaves this Mac.", size: 13, muted: true), to: content)
        let group = AgentGroup(tint: nil)
        for starter in status.starters {
            let check = NSButton(checkboxWithTitle: starter.name, target: self, action: #selector(checkChanged(_:)))
            check.font = .systemFont(ofSize: 13)
            check.state = .on
            check.toolTip = starter.description
            let popup = FilterSheetParts.actionPopUp(starter.action)
            popup.setAccessibilityLabel("What happens to \(starter.name)")
            let spacer = NSView()
            spacer.setContentHuggingPriority(NSLayoutConstraint.Priority(1), for: .horizontal)
            let row = NSStackView(views: [check, spacer, popup])
            row.alignment = .centerY
            row.spacing = 12
            popup.setContentCompressionResistancePriority(.required, for: .horizontal)
            group.addRow(row)
            rows.append((starter, check, popup))
        }
        FilterSheetParts.add(group, to: content, after: 16)
        let size = status.downloadBytes > 0 ? " (\(status.size))" : ""
        FilterSheetParts.add(AgentAccessController.label("Downloads \(status.modelName)\(size) once. You can change filters any time.", size: 12, muted: true), to: content, after: 12)
        cancelButton.keyEquivalent = "\u{1b}"
        cancelButton.applySecondaryStyle()
        cancelButton.handler = { [weak self] in self?.finish(nil) }
        confirmButton.keyEquivalent = "\r"
        confirmButton.applyPrimaryStyle()
        confirmButton.handler = { [weak self] in
            guard let self else { return }
            finish(rows.filter { $0.check.state == .on }.map { ($0.starter, FilterSheetParts.action(of: $0.popup)) })
        }
        FilterSheetParts.install(content, actions: FilterSheetParts.actions(trailing: [cancelButton, confirmButton]), in: root)
        panel.initialFirstResponder = confirmButton
        FilterSheetParts.fit(panel)
    }

    @objc private func checkChanged(_ sender: NSButton) {
        for row in rows where row.check === sender { row.popup.isEnabled = sender.state == .on }
        confirmButton.isEnabled = rows.contains { $0.check.state == .on }
    }

    func present(on parent: NSWindow, done: @escaping ([(starter: MailFilterStarter, action: MailFilterAction)]?) -> Void) {
        self.parent = parent
        self.done = done
        Self.presented = self
        parent.beginSheet(panel)
        panel.makeFirstResponder(confirmButton)
    }

    private func finish(_ chosen: [(starter: MailFilterStarter, action: MailFilterAction)]?) {
        if let parent { parent.endSheet(panel) } else { panel.orderOut(nil) }
        let done = self.done
        self.done = nil
        Self.presented = nil
        done?(chosen)
    }
}

/// The rounded box around "What goes here", like the name field's.
@MainActor
private final class FilterTextBox: NSView {
    override func viewDidChangeEffectiveAppearance() { super.viewDidChangeEffectiveAppearance(); needsDisplay = true }
    override func draw(_ dirtyRect: NSRect) {
        let path = NSBezierPath(roundedRect: bounds.insetBy(dx: 0.5, dy: 0.5), xRadius: 6, yRadius: 6)
        NSColor.textBackgroundColor.setFill()
        path.fill()
        NSColor.separatorColor.setStroke()
        path.lineWidth = 1
        path.stroke()
    }
}

/// A text view whose Return saves the sheet; Option-Return adds a line.
@MainActor
private final class FilterDescriptionView: NSTextView {
    var onReturn: (() -> Void)?
    var onChange: (() -> Void)?
    override func insertNewline(_ sender: Any?) {
        if NSApp.currentEvent?.modifierFlags.contains(.option) == true { super.insertNewline(sender) } else { onReturn?() }
    }
    override func didChangeText() { super.didChangeText(); onChange?() }
}

/// New filter and Edit filter: name, what goes here, when it arrives.
@MainActor
final class FilterEditorSheet: NSObject, NSTextFieldDelegate {
    private(set) static var presented: FilterEditorSheet?
    let panel: NSWindow
    let nameField = NSTextField(string: "")
    private let descriptionView = FilterDescriptionView()
    private let placeholder = NSTextField(labelWithString: "Describe it like you would to a friend: who sends it and what it’s about.")
    let actionPopUp: AgentPopUp
    let explanation = AgentAccessController.label("", size: 12, muted: true)
    let errorLabel = AgentAccessController.label("", size: 12)
    let saveButton: AgentButton
    let cancelButton: AgentButton
    private(set) var deleteButton: AgentButton?
    private let core: CoreBridge
    private let existing: MailFilterInfo?
    private var busy = false
    private weak var parent: NSWindow?
    /// The saved filter's id, or nil when it was deleted; not called on Cancel.
    private var done: ((_ savedID: String?, _ deleted: Bool) -> Void)?

    var descriptionText: String {
        get { descriptionView.string }
        set { descriptionView.string = newValue; updatePlaceholder() }
    }

    init(core: CoreBridge, existing: MailFilterInfo?) {
        self.core = core
        self.existing = existing
        let (panel, root) = FilterSheetParts.panel(title: existing == nil ? "New filter" : "Edit filter")
        self.panel = panel
        actionPopUp = FilterSheetParts.actionPopUp(existing?.action ?? .skipInbox)
        saveButton = AgentButton(title: "Save") {}
        cancelButton = AgentButton(title: "Cancel") {}
        super.init()
        let content = FilterSheetParts.stack()
        FilterSheetParts.add(AgentAccessController.label(existing == nil ? "New filter" : "Edit filter", size: 16, weight: .bold), to: content)
        func fieldLabel(_ text: String) -> NSTextField { AgentAccessController.label(text, size: 12, weight: .semibold, muted: true) }
        FilterSheetParts.add(fieldLabel("Name"), to: content, after: 14)
        nameField.font = .systemFont(ofSize: 13)
        nameField.placeholderString = "e.g. Receipts"
        nameField.stringValue = existing?.name ?? ""
        nameField.delegate = self
        nameField.setAccessibilityLabel("Name")
        FilterSheetParts.add(nameField, to: content, after: 6)
        FilterSheetParts.add(fieldLabel("What goes here"), to: content, after: 14)
        let box = FilterTextBox()
        let scroll = NSScrollView()
        scroll.borderType = .noBorder
        scroll.hasVerticalScroller = true
        scroll.autohidesScrollers = true
        scroll.drawsBackground = false
        scroll.translatesAutoresizingMaskIntoConstraints = false
        box.addSubview(scroll)
        descriptionView.isRichText = false
        descriptionView.font = .systemFont(ofSize: 13)
        descriptionView.textColor = .textColor
        descriptionView.drawsBackground = false
        descriptionView.textContainerInset = NSSize(width: 2, height: 5)
        descriptionView.isVerticallyResizable = true
        descriptionView.autoresizingMask = [.width]
        descriptionView.textContainer?.widthTracksTextView = true
        descriptionView.allowsUndo = true
        descriptionView.setAccessibilityLabel("What goes here")
        descriptionView.string = existing?.description ?? ""
        descriptionView.onReturn = { [weak self] in self?.save() }
        descriptionView.onChange = { [weak self] in self?.updatePlaceholder(); self?.errorLabel.isHidden = true }
        scroll.documentView = descriptionView
        placeholder.font = .systemFont(ofSize: 13)
        placeholder.textColor = .placeholderTextColor
        placeholder.lineBreakMode = .byWordWrapping
        placeholder.maximumNumberOfLines = 2
        placeholder.translatesAutoresizingMaskIntoConstraints = false
        scroll.addSubview(placeholder)
        NSLayoutConstraint.activate([
            scroll.leadingAnchor.constraint(equalTo: box.leadingAnchor, constant: 3),
            scroll.trailingAnchor.constraint(equalTo: box.trailingAnchor, constant: -3),
            scroll.topAnchor.constraint(equalTo: box.topAnchor, constant: 2),
            scroll.bottomAnchor.constraint(equalTo: box.bottomAnchor, constant: -2),
            box.heightAnchor.constraint(equalToConstant: 92),
            placeholder.leadingAnchor.constraint(equalTo: scroll.leadingAnchor, constant: 8),
            placeholder.trailingAnchor.constraint(equalTo: scroll.trailingAnchor, constant: -8),
            placeholder.topAnchor.constraint(equalTo: scroll.topAnchor, constant: 6),
        ])
        FilterSheetParts.add(box, to: content, after: 6)
        FilterSheetParts.add(fieldLabel("When it arrives"), to: content, after: 14)
        actionPopUp.setAccessibilityLabel("When it arrives")
        actionPopUp.handler = { [weak self] in self?.updateExplanation() }
        let popupRow = NSStackView(views: [actionPopUp, NSView()])
        popupRow.alignment = .centerY
        FilterSheetParts.add(popupRow, to: content, after: 6)
        FilterSheetParts.add(explanation, to: content, after: 4)
        errorLabel.textColor = EmailTheme.warning
        errorLabel.isHidden = true
        FilterSheetParts.add(errorLabel, to: content, after: 12)
        cancelButton.keyEquivalent = "\u{1b}"
        cancelButton.applySecondaryStyle()
        cancelButton.handler = { [weak self] in self?.close() }
        saveButton.keyEquivalent = "\r"
        saveButton.applyPrimaryStyle()
        saveButton.handler = { [weak self] in self?.save() }
        var leading: [NSView] = []
        if existing != nil {
            let delete = AgentButton(title: "Delete filter") { [weak self] in self?.confirmDelete() }
            delete.applyGhostStyle()
            deleteButton = delete
            leading.append(delete)
        }
        FilterSheetParts.install(content, actions: FilterSheetParts.actions(leading: leading, trailing: [cancelButton, saveButton]), in: root)
        updateExplanation()
        updatePlaceholder()
        panel.initialFirstResponder = existing == nil ? nameField : descriptionView
        FilterSheetParts.fit(panel)
    }

    private func updatePlaceholder() { placeholder.isHidden = !descriptionView.string.isEmpty }
    private func updateExplanation() { explanation.stringValue = FilterSheetParts.action(of: actionPopUp).explanation }

    func controlTextDidChange(_ obj: Notification) { errorLabel.isHidden = true }

    /// Return in the name field saves.
    func control(_ control: NSControl, textView: NSTextView, doCommandBy commandSelector: Selector) -> Bool {
        if commandSelector == #selector(NSResponder.insertNewline(_:)) { save(); return true }
        if commandSelector == #selector(NSResponder.cancelOperation(_:)) { close(); return true }
        return false
    }

    func present(on parent: NSWindow, done: @escaping (_ savedID: String?, _ deleted: Bool) -> Void) {
        self.parent = parent
        self.done = done
        Self.presented = self
        parent.beginSheet(panel)
        panel.makeFirstResponder(existing == nil ? nameField : descriptionView)
    }

    func showError(_ message: String) {
        // The engine prefixes validation errors with their kind.
        let prefix = "invalid action: "
        var message = message.hasPrefix(prefix) ? String(message.dropFirst(prefix.count)) : message
        if let first = message.first { message = first.uppercased() + message.dropFirst() }
        if !message.hasSuffix(".") { message += "." }
        errorLabel.stringValue = message
        errorLabel.isHidden = false
        FilterSheetParts.fit(panel)
    }

    func save() {
        guard !busy else { return }
        let name = nameField.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        let description = descriptionView.string.trimmingCharacters(in: .whitespacesAndNewlines)
        if name.isEmpty { showError("Give the filter a name."); panel.makeFirstResponder(nameField); return }
        if description.isEmpty { showError("Say what goes here, so the model knows what to look for."); panel.makeFirstResponder(descriptionView); return }
        busy = true
        saveButton.isEnabled = false
        var request: [String: Any] = ["op": "upsert_filter", "name": name, "description": description, "action": FilterSheetParts.action(of: actionPopUp).rawValue]
        if let id = existing?.id { request["id"] = id }
        Task { @MainActor in
            defer { busy = false; saveButton.isEnabled = true }
            do {
                let saved = try await core.callAsync(request) as? [String: Any]
                finish(saved?["id"] as? String ?? existing?.id, deleted: false)
            } catch {
                showError(error.localizedDescription)
            }
        }
    }

    private func confirmDelete() {
        guard let existing, !busy else { return }
        let alert = NSAlert()
        alert.messageText = "Delete “\(existing.name)”?"
        alert.informativeText = "Its conversations stay where they are. Nothing is deleted from your mail."
        alert.addButton(withTitle: "Cancel")
        alert.addButton(withTitle: "Delete filter")
        alert.beginSheetModal(for: panel) { [weak self] response in
            guard response == .alertSecondButtonReturn else { return }
            MainActor.assumeIsolated { self?.delete() }
        }
    }

    func delete() {
        guard let existing, !busy else { return }
        busy = true
        Task { @MainActor in
            defer { busy = false }
            do {
                _ = try await core.callAsync(["op": "delete_filter", "id": existing.id])
                finish(nil, deleted: true)
            } catch { showError(error.localizedDescription) }
        }
    }

    func close() {
        if let parent { parent.endSheet(panel) } else { panel.orderOut(nil) }
        done = nil
        Self.presented = nil
    }

    private func finish(_ savedID: String?, deleted: Bool) {
        let done = self.done
        close()
        done?(savedID, deleted)
    }
}

/// Filter settings: sorting on or off, the model, and the filters.
@MainActor
final class FilterSettingsSheet: NSObject {
    private(set) static var presented: FilterSettingsSheet?
    let panel: NSWindow
    let doneButton: AgentButton
    private let content = FilterSheetParts.stack()
    private(set) var status: FilterStatus
    private weak var parent: NSWindow?
    private(set) var enabledSwitch: NSSwitch?
    var onSetEnabled: ((Bool) -> Void)?
    var onDownload: (() -> Void)?
    var onCancelDownload: (() -> Void)?
    var onRemoveModel: (() -> Void)?
    var onEdit: ((MailFilterInfo?) -> Void)?
    var onClose: (() -> Void)?

    init(status: FilterStatus) {
        self.status = status
        let (panel, root) = FilterSheetParts.panel(title: "Filter settings")
        self.panel = panel
        doneButton = AgentButton(title: "Done") {}
        super.init()
        doneButton.applySecondaryStyle()
        doneButton.keyEquivalent = "\u{1b}"
        doneButton.handler = { [weak self] in self?.close() }
        FilterSheetParts.install(content, actions: FilterSheetParts.actions(trailing: [doneButton]), in: root)
        render()
    }

    func update(_ status: FilterStatus) {
        guard status != self.status else { return }
        self.status = status
        render()
    }

    private func render() {
        content.arrangedSubviews.forEach { content.removeArrangedSubview($0); $0.removeFromSuperview() }
        FilterSheetParts.add(AgentAccessController.label("Filters", size: 16, weight: .bold), to: content)
        let general = AgentGroup(tint: nil)
        let toggle = NSSwitch()
        toggle.controlSize = .small
        toggle.state = status.enabled ? .on : .off
        toggle.target = self
        toggle.action = #selector(toggled(_:))
        toggle.setAccessibilityLabel("Sort mail automatically")
        enabledSwitch = toggle
        general.addRow(AgentAccessController.listRow(title: "Sort mail automatically", sub: "", trailing: toggle))
        var sub = ""
        var trailing: NSView
        if status.downloading {
            sub = "Downloading… \(status.percent)%"
            let cancel = AgentLinkButton(title: "Cancel download") { [weak self] in self?.onCancelDownload?() }
            trailing = cancel
        } else if status.installed {
            trailing = AgentLinkButton(title: "Remove") { [weak self] in self?.confirmRemove() }
        } else {
            if let error = status.downloadError { sub = "Download failed: \(error)" } else { sub = "Not downloaded" }
            trailing = AgentLinkButton(title: status.downloadError == nil ? "Download" : "Retry") { [weak self] in self?.onDownload?() }
        }
        general.addRow(AgentAccessController.listRow(title: status.modelLine, sub: sub, trailing: trailing))
        FilterSheetParts.add(general, to: content, after: 14)
        FilterSheetParts.add(AgentAccessController.label("Your filters", size: 12, weight: .semibold, muted: true), to: content, after: 18)
        let list = AgentGroup(tint: nil)
        for filter in status.filters {
            let row = FilterSettingsRow(filter: filter) { [weak self] in self?.onEdit?(filter) }
            list.addRow(row)
        }
        let add = AgentLinkButton(title: "New filter") { [weak self] in self?.onEdit?(nil) }
        add.isEnabled = status.filters.count < 8
        list.addRow(NSStackView(views: [add]))
        FilterSheetParts.add(list, to: content, after: 8)
        FilterSheetParts.fit(panel)
    }

    @objc private func toggled(_ sender: NSSwitch) { onSetEnabled?(sender.state == .on) }

    private func confirmRemove() {
        let alert = NSAlert()
        alert.messageText = "Remove the sorting model?"
        alert.informativeText = "This frees \(status.size). Filters keep what they sorted; new mail stays in your inbox until you download the model again."
        alert.addButton(withTitle: "Cancel")
        alert.addButton(withTitle: "Remove")
        alert.beginSheetModal(for: panel) { [weak self] response in
            guard response == .alertSecondButtonReturn else { return }
            MainActor.assumeIsolated { self?.onRemoveModel?() }
        }
    }

    func present(on parent: NSWindow) {
        self.parent = parent
        Self.presented = self
        parent.beginSheet(panel)
        panel.makeFirstResponder(doneButton)
    }

    func close() {
        if let sheet = panel.attachedSheet { panel.endSheet(sheet) }
        if let parent { parent.endSheet(panel) } else { panel.orderOut(nil) }
        Self.presented = nil
        onClose?()
    }
}

/// One filter in Filter settings: its name and action; click to edit.
@MainActor
private final class FilterSettingsRow: NSButton {
    private let handler: () -> Void
    init(filter: MailFilterInfo, handler: @escaping () -> Void) {
        self.handler = handler
        super.init(frame: .zero)
        title = ""
        isBordered = false
        target = self
        action = #selector(run)
        setAccessibilityLabel("Edit \(filter.name), \(filter.action.title)")
        let icon = NSImageView(image: NSImage(systemSymbolName: filter.action.symbol, accessibilityDescription: nil) ?? NSImage())
        icon.contentTintColor = EmailTheme.muted
        icon.symbolConfiguration = .init(pointSize: 12, weight: .regular)
        let name = AgentAccessController.label(filter.name, size: 13)
        name.maximumNumberOfLines = 1
        name.lineBreakMode = .byTruncatingTail
        let action = AgentAccessController.label(filter.action.title, size: 12, muted: true)
        action.setContentCompressionResistancePriority(.required, for: .horizontal)
        action.setContentHuggingPriority(.required, for: .horizontal)
        let chevron = NSImageView(image: NSImage(systemSymbolName: "chevron.right", accessibilityDescription: nil) ?? NSImage())
        chevron.contentTintColor = EmailTheme.muted
        chevron.symbolConfiguration = .init(pointSize: 10, weight: .medium)
        let spacer = NSView()
        spacer.setContentHuggingPriority(NSLayoutConstraint.Priority(1), for: .horizontal)
        let row = NSStackView(views: [icon, name, spacer, action, chevron])
        row.alignment = .centerY
        row.spacing = 8
        row.translatesAutoresizingMaskIntoConstraints = false
        addSubview(row)
        NSLayoutConstraint.activate([
            icon.widthAnchor.constraint(equalToConstant: 18),
            row.leadingAnchor.constraint(equalTo: leadingAnchor), row.trailingAnchor.constraint(equalTo: trailingAnchor),
            row.topAnchor.constraint(equalTo: topAnchor), row.bottomAnchor.constraint(equalTo: bottomAnchor),
            heightAnchor.constraint(greaterThanOrEqualToConstant: 18),
        ])
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    @objc private func run() { handler() }
    override func resetCursorRects() { addCursorRect(bounds, cursor: .pointingHand) }
}

/// The strip at the top of the reader for a filtered conversation: the
/// filter and the model's reason, Move to inbox and Move to.
@MainActor
final class FilterReaderBar: NSView {
    private let text = NSTextField(labelWithString: "")
    let moveToInbox: AgentButton
    let moveTo: AgentButton
    private let buttons = NSStackView()
    var onMove: ((_ filterID: String?) -> Void)?
    private(set) var thread: MailThread?
    private var destinations: [(title: String, filterID: String?)] = []

    override init(frame: NSRect) {
        moveToInbox = AgentButton(title: "Move to inbox") {}
        moveTo = AgentButton(title: "Move to") {}
        super.init(frame: frame)
        wantsLayer = true
        text.lineBreakMode = .byTruncatingTail
        text.maximumNumberOfLines = 1
        text.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        moveToInbox.applySecondaryStyle()
        moveToInbox.handler = { [weak self] in self?.onMove?(nil) }
        moveToInbox.toolTip = "Back to the inbox; later mail from this sender stays there"
        moveTo.applyGhostStyle()
        moveTo.image = NSImage(systemSymbolName: "chevron.down", accessibilityDescription: nil)?.withSymbolConfiguration(.init(pointSize: 9, weight: .semibold))
        moveTo.imagePosition = .imageTrailing
        moveTo.contentTintColor = EmailTheme.ink.withAlphaComponent(0.8)
        moveTo.handler = { [weak self] in self?.showMenu() }
        moveTo.setAccessibilityLabel("Move to another filter")
        buttons.orientation = .horizontal
        buttons.spacing = 6
        buttons.alignment = .centerY
        buttons.setContentCompressionResistancePriority(.required, for: .horizontal)
        buttons.addArrangedSubview(moveToInbox)
        buttons.addArrangedSubview(moveTo)
        let line = AgentLine()
        for view in [text, buttons, line] as [NSView] {
            view.translatesAutoresizingMaskIntoConstraints = false
            addSubview(view)
        }
        NSLayoutConstraint.activate([
            text.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 28), text.centerYAnchor.constraint(equalTo: centerYAnchor),
            buttons.leadingAnchor.constraint(greaterThanOrEqualTo: text.trailingAnchor, constant: 12),
            buttons.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -22), buttons.centerYAnchor.constraint(equalTo: centerYAnchor),
            line.leadingAnchor.constraint(equalTo: leadingAnchor), line.trailingAnchor.constraint(equalTo: trailingAnchor),
            line.bottomAnchor.constraint(equalTo: bottomAnchor), line.heightAnchor.constraint(equalToConstant: 1),
        ])
        paint()
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        paint()
        if let thread { show(thread, filters: destinationsFilters) }
    }

    private func paint() {
        layer?.backgroundColor = EmailTheme.cgColor(EmailTheme.inbox, in: effectiveAppearance)
    }

    private var destinationsFilters: [MailFilterInfo] = []

    /// The menu's choices for a conversation in `filter`: the other filters,
    /// then the inbox (named No filter when the filter keeps mail there).
    static func destinations(for filter: ThreadFilter, filters: [MailFilterInfo]) -> [(title: String, filterID: String?)] {
        filters.filter { $0.id != filter.id }.map { ($0.name, Optional($0.id)) }
            + [(filter.action == .inbox ? "No filter" : "Inbox", nil)]
    }

    func show(_ thread: MailThread, filters: [MailFilterInfo]) {
        guard let filter = thread.filter else { return }
        self.thread = thread
        destinationsFilters = filters
        let value = NSMutableAttributedString(string: filter.name, attributes: [.font: NSFont.systemFont(ofSize: 13, weight: .semibold), .foregroundColor: EmailTheme.ink])
        if !filter.reason.isEmpty {
            value.append(NSAttributedString(string: "   " + filter.reason, attributes: [.font: NSFont.systemFont(ofSize: 13), .foregroundColor: EmailTheme.muted]))
        }
        text.attributedStringValue = value
        text.toolTip = filter.reason.isEmpty ? nil : filter.reason
        moveToInbox.isHidden = thread.isInbox
        destinations = Self.destinations(for: filter, filters: filters)
    }

    func showMenu() {
        let menu = NSMenu()
        for (index, destination) in destinations.enumerated() {
            let item = NSMenuItem(title: destination.title, action: #selector(choose(_:)), keyEquivalent: "")
            item.target = self
            item.tag = index
            if destination.filterID == nil, destinations.count > 1 { menu.addItem(.separator()) }
            menu.addItem(item)
        }
        menu.popUp(positioning: nil, at: NSPoint(x: 0, y: moveTo.bounds.height + 4), in: moveTo)
    }

    @objc private func choose(_ sender: NSMenuItem) {
        guard destinations.indices.contains(sender.tag) else { return }
        onMove?(destinations[sender.tag].filterID)
    }
}
