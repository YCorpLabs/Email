import AppKit

@MainActor
final class SearchFilterPanel: NSViewController, NSTextFieldDelegate {
    private let base: String
    private let initialFields: [String: String]
    private let initialRead: Int
    private let initialAttachments: Bool
    private let onApply: (String) -> Void
    private var fields: [String: NSTextField] = [:]
    private let read = NSPopUpButton()
    private let attachments = NSButton(checkboxWithTitle: "Has attachments", target: nil, action: nil)
    private let preview = NSTextField(wrappingLabelWithString: "")
    private let apply = NSButton(title: "Apply filters", target: nil, action: nil)
    var dismiss: (() -> Void)?

    init(base: String, onApply: @escaping (String) -> Void) {
        let initial = SearchFilterRules.decompose(base: base)
        self.base = initial?.remainder ?? base
        self.initialFields = initial?.fields ?? [:]; self.initialRead = initial?.read ?? 0; self.initialAttachments = initial?.attachments ?? false
        self.onApply = onApply
        super.init(nibName: nil, bundle: nil)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    override func loadView() {
        let content = NSStackView(); content.orientation = .vertical; content.alignment = .leading; content.spacing = 12
        content.translatesAutoresizingMaskIntoConstraints = false
        let heading = NSTextField(labelWithString: "Refine search"); heading.font = .systemFont(ofSize: 17, weight: .semibold); heading.textColor = EmailTheme.ink
        content.addArrangedSubview(heading)
        let hint = NSTextField(wrappingLabelWithString: "Downloaded mail in your current view. Existing filters are filled in; other search terms stay unchanged.")
        hint.font = .systemFont(ofSize: 12); hint.textColor = EmailTheme.muted; content.addArrangedSubview(hint)
        for (key, title, placeholder) in [("from", "From", "Name or email"), ("to", "To", "Recipient or alias"), ("cc", "Cc", "Copied recipient"), ("subject", "Subject", "Words or phrase"), ("after", "On or after", "YYYY-MM-DD"), ("before", "Before", "YYYY-MM-DD")] {
            let row = NSStackView(); row.spacing = 12; row.alignment = .centerY
            let label = NSTextField(labelWithString: title); label.font = .systemFont(ofSize: 12); label.textColor = EmailTheme.muted
            label.widthAnchor.constraint(equalToConstant: 76).isActive = true
            let entry = NSTextField(); entry.placeholderString = placeholder; entry.delegate = self
            entry.focusRingType = .none; entry.isBezeled = true; entry.bezelStyle = .roundedBezel; entry.drawsBackground = true; entry.backgroundColor = EmailTheme.compose; entry.textColor = EmailTheme.ink
            entry.wantsLayer = true; entry.layer?.cornerRadius = 5; entry.layer?.borderWidth = 1; entry.layer?.borderColor = EmailTheme.cgColor(EmailTheme.line, in: NSApp.effectiveAppearance)
            entry.heightAnchor.constraint(equalToConstant: 26).isActive = true
            entry.font = .systemFont(ofSize: 13); entry.setAccessibilityLabel(title); entry.stringValue = initialFields[key] ?? ""; fields[key] = entry
            row.addArrangedSubview(label); row.addArrangedSubview(entry); content.addArrangedSubview(row)
            row.widthAnchor.constraint(equalToConstant: 390).isActive = true
        }
        read.addItems(withTitles: ["Any read status", "Unread", "Read"]); read.selectItem(at: initialRead); read.target = self; read.action = #selector(update)
        attachments.state = initialAttachments ? .on : .off
        attachments.target = self; attachments.action = #selector(update)
        let options = NSStackView(views: [read, attachments]); options.spacing = 16; content.addArrangedSubview(options)
        preview.font = .monospacedSystemFont(ofSize: 11, weight: .regular); preview.textColor = EmailTheme.muted
        preview.maximumNumberOfLines = 3; preview.lineBreakMode = .byTruncatingTail; content.addArrangedSubview(preview)
        preview.widthAnchor.constraint(equalToConstant: 390).isActive = true
        let cancel = NSButton(title: "Cancel", target: self, action: #selector(cancel)); cancel.keyEquivalent = "\u{1b}"
        apply.target = self; apply.action = #selector(submit); apply.keyEquivalent = "\r"
        let spacer = NSView(); spacer.setContentHuggingPriority(.defaultLow, for: .horizontal)
        let footer = NSStackView(views: [spacer, cancel, apply]); footer.spacing = 8; content.addArrangedSubview(footer); footer.widthAnchor.constraint(equalToConstant: 390).isActive = true
        let root = SearchFilterSurface(frame: NSRect(x: 0, y: 0, width: 430, height: 500)); root.addSubview(content)
        NSLayoutConstraint.activate([content.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 20), content.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -20), content.topAnchor.constraint(equalTo: root.topAnchor, constant: 20), content.bottomAnchor.constraint(equalTo: root.bottomAnchor, constant: -20)])
        view = root; update(); preferredContentSize = NSSize(width: 430, height: root.fittingSize.height)
    }
    override func viewDidAppear() { super.viewDidAppear(); view.window?.makeFirstResponder(fields["from"]) }
    func controlTextDidChange(_ notification: Notification) { update() }
    func control(_ control: NSControl, textView: NSTextView, doCommandBy commandSelector: Selector) -> Bool {
        if commandSelector == #selector(NSResponder.insertNewline(_:)) { submit(); return true }
        if commandSelector == #selector(NSResponder.cancelOperation(_:)) { cancel(); return true }
        return false
    }
    func controlTextDidBeginEditing(_ notification: Notification) { (notification.object as? NSTextField)?.layer?.borderColor = EmailTheme.cgColor(EmailTheme.accent, in: view.effectiveAppearance) }
    func controlTextDidEndEditing(_ notification: Notification) { (notification.object as? NSTextField)?.layer?.borderColor = EmailTheme.cgColor(EmailTheme.line, in: view.effectiveAppearance) }
    @objc private func update() {
        do {
            let query = try SearchFilterRules.query(base: base, fields: fields.mapValues(\.stringValue), read: read.indexOfSelectedItem, attachments: attachments.state == .on)
            preview.stringValue = query.isEmpty ? "Choose filters above · Tab moves between fields" : query
            preview.textColor = EmailTheme.muted
            apply.isEnabled = fields.contains { key, entry in entry.stringValue.trimmingCharacters(in: .whitespacesAndNewlines) != (initialFields[key] ?? "") } || read.indexOfSelectedItem != initialRead || (attachments.state == .on) != initialAttachments
        } catch { preview.stringValue = error.localizedDescription; preview.textColor = .systemRed; apply.isEnabled = false }
    }
    @objc private func cancel() { dismiss?() }
    @objc private func submit() {
        guard apply.isEnabled, let query = try? SearchFilterRules.query(base: base, fields: fields.mapValues(\.stringValue), read: read.indexOfSelectedItem, attachments: attachments.state == .on) else { return }
        onApply(query); dismiss?()
    }
}

private final class SearchFilterSurface: NSView {
    override var isFlipped: Bool { true }
    override func draw(_ dirtyRect: NSRect) { EmailTheme.reader.setFill(); bounds.fill() }
    override func viewDidChangeEffectiveAppearance() { super.viewDidChangeEffectiveAppearance(); needsDisplay = true }
}
