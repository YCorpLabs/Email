import AppKit

private final class SplitEditorPanel: NSPanel {
    override func cancelOperation(_ sender: Any?) { close() }
}

/// A local, previewable saved view. Editing never contacts the mail provider.
@MainActor
final class SplitEditorWindow: NSWindowController, NSTextFieldDelegate {
    private let core: CoreBridge
    private let saved: [MailSplit]
    private var editingID: String?
    private let name = NSTextField(string: "")
    private let rule = NSTextField(string: "")
    private let preview = NSTextField(wrappingLabelWithString: "")
    private let save = NSButton(title: "Create split", target: nil, action: nil)
    private var previewTask: Task<Void, Never>?
    private var generation = 0
    private let didSave: (String) -> Void

    init(core: CoreBridge, existing: MailSplit?, saved: [MailSplit], sender: String?, didSave: @escaping (String) -> Void) {
        self.core = core
        self.saved = saved
        self.didSave = didSave
        let panel = SplitEditorPanel(contentRect: NSRect(x: 0, y: 0, width: 560, height: 460), styleMask: [.titled, .closable], backing: .buffered, defer: false)
        panel.title = "Split inboxes"
        panel.isReleasedWhenClosed = false
        super.init(window: panel)
        let stack = NSStackView()
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 14
        stack.translatesAutoresizingMaskIntoConstraints = false
        panel.contentView!.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: panel.contentView!.leadingAnchor, constant: 24),
            stack.trailingAnchor.constraint(equalTo: panel.contentView!.trailingAnchor, constant: -24),
            stack.topAnchor.constraint(equalTo: panel.contentView!.topAnchor, constant: 24),
            stack.bottomAnchor.constraint(equalTo: panel.contentView!.bottomAnchor, constant: -24)
        ])
        func label(_ text: String, size: CGFloat, bold: Bool = false) -> NSTextField {
            let value = NSTextField(wrappingLabelWithString: text)
            value.font = .systemFont(ofSize: size, weight: bold ? .semibold : .regular)
            return value
        }
        stack.addArrangedSubview(label("Focus on the mail that matters", size: 20, bold: true))
        stack.addArrangedSubview(label("Splits are saved views. Matching mail appears automatically; messages stay in your inbox.", size: 12))
        stack.addArrangedSubview(label("NAME", size: 10, bold: true))
        name.placeholderString = "e.g. Work"
        name.delegate = self
        stack.addArrangedSubview(name)
        name.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true
        stack.addArrangedSubview(label("MATCH ANY OF THESE RULES", size: 10, bold: true))
        rule.placeholderString = "*@example.com, *@example.org"
        rule.delegate = self
        stack.addArrangedSubview(rule)
        rule.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true
        stack.addArrangedSubview(label("Separate alternatives with commas. * matches any text. Addresses match senders and recipients. You can also use from:, to:, subject: or label:.", size: 11))
        if let sender, !sender.isEmpty {
            let shortcuts = NSStackView()
            shortcuts.orientation = .horizontal
            shortcuts.spacing = 8
            let person = NSButton(title: "This sender", target: self, action: #selector(useSender(_:)))
            person.identifier = NSUserInterfaceItemIdentifier("from:\(sender)")
            person.bezelStyle = .rounded
            shortcuts.addArrangedSubview(person)
            if let domain = sender.split(separator: "@").last {
                let domainButton = NSButton(title: "Anyone at \(domain)", target: self, action: #selector(useSender(_:)))
                domainButton.identifier = NSUserInterfaceItemIdentifier("*@\(domain)")
                domainButton.bezelStyle = .rounded
                shortcuts.addArrangedSubview(domainButton)
            }
            stack.addArrangedSubview(shortcuts)
        }
        preview.heightAnchor.constraint(equalToConstant: 72).isActive = true
        preview.maximumNumberOfLines = 4
        preview.lineBreakMode = .byTruncatingTail
        preview.font = .systemFont(ofSize: 12)
        preview.textColor = .secondaryLabelColor
        stack.addArrangedSubview(preview)
        preview.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true
        if !saved.isEmpty {
            stack.addArrangedSubview(label("EXISTING SPLITS · CLICK TO EDIT", size: 10, bold: true))
            let chooser = NSPopUpButton()
            chooser.addItem(withTitle: "Choose a saved split to edit…")
            for split in saved { chooser.addItem(withTitle: "\(split.name) · \(split.query)") }
            chooser.target = self
            chooser.action = #selector(chooseSaved(_:))
            stack.addArrangedSubview(chooser)
            chooser.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true
            for (index, split) in saved.prefix(3).enumerated() {
                let button = NSButton(title: "\(split.name)   ·   \(split.query)", target: self, action: #selector(editSaved(_:)))
                button.tag = index
                button.isBordered = false
                button.alignment = .left
                button.lineBreakMode = .byTruncatingTail
                button.toolTip = split.query
                stack.addArrangedSubview(button)
                button.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true
            }
        }
        let actions = NSStackView()
        actions.orientation = .horizontal
        actions.spacing = 10
        let cancel = NSButton(title: "Cancel", target: self, action: #selector(cancelClicked))
        cancel.bezelStyle = .rounded
        save.target = self
        save.action = #selector(saveClicked)
        save.bezelStyle = .rounded
        save.keyEquivalent = "\r"
        actions.addArrangedSubview(cancel)
        actions.addArrangedSubview(save)
        stack.addArrangedSubview(actions)
        if let existing { load(existing) } else { updatePreview() }
        panel.setContentSize(NSSize(width: 560, height: stack.fittingSize.height + 48))
        panel.center()
        panel.initialFirstResponder = rule
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) is unavailable") }
    @objc private func cancelClicked() { close() }
    @objc private func useSender(_ button: NSButton) {
        rule.stringValue = button.identifier?.rawValue ?? ""
        updatePreview()
        window?.makeFirstResponder(rule)
    }
    @objc private func editSaved(_ button: NSButton) { load(saved[button.tag]) }
    @objc private func chooseSaved(_ chooser: NSPopUpButton) {
        let index = chooser.indexOfSelectedItem - 1
        guard saved.indices.contains(index) else { return }
        load(saved[index])
    }
    private func load(_ split: MailSplit) {
        editingID = split.id
        name.stringValue = split.name
        rule.stringValue = split.query
        save.title = "Save changes"
        updatePreview()
    }
    func controlTextDidChange(_ notification: Notification) { updatePreview() }
    private func updatePreview() {
        generation += 1
        let current = generation
        previewTask?.cancel()
        let query = rule.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !query.isEmpty else { preview.stringValue = "Start with a sender or domain above, or enter your own rules."; save.isEnabled = false; return }
        preview.stringValue = "Checking cached mail…"
        save.isEnabled = false
        previewTask = Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                try await Task.sleep(for: .milliseconds(150))
                let rows = try await core.callAsync(["op": "preview_split", "query": query]) as? [[String: Any]] ?? []
                guard current == generation else { return }
                let duplicate = saved.first { $0.id != editingID && canonical($0.query) == canonical(query) }
                if let duplicate {
                    preview.stringValue = "Already used by ‘\(duplicate.name)’. Select that split below to edit it."
                    return
                }
                let subjects = rows.prefix(3).compactMap { $0["subject"] as? String }.joined(separator: "\n")
                preview.stringValue = "\(rows.count)\(rows.count == 500 ? "+" : "") matching conversations in cached mail\(subjects.isEmpty ? "" : "\n" + subjects)"
                save.isEnabled = true
            } catch {
                guard current == generation, !Task.isCancelled else { return }
                preview.stringValue = error.localizedDescription
            }
        }
    }
    private func canonical(_ value: String) -> String {
        value.split(whereSeparator: { $0 == "," || $0 == "\n" }).map { $0.trimmingCharacters(in: .whitespaces).lowercased() }.joined(separator: ",")
    }
    @objc private func saveClicked() {
        let query = rule.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        let enteredName = name.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        let chosenName = enteredName.isEmpty ? String(query.prefix(80)) : enteredName
        save.isEnabled = false
        Task { @MainActor in
            do {
                var payload: [String: Any] = ["op": "upsert_split", "name": chosenName, "query": query]
                if let editingID { payload["id"] = editingID }
                if let result = try await core.callAsync(payload) as? [String: Any], let id = result["id"] as? String {
                    didSave(id)
                    close()
                }
            } catch { preview.stringValue = error.localizedDescription; save.isEnabled = true }
        }
    }
}
