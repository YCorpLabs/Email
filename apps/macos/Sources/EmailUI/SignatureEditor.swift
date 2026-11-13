import AppKit

@MainActor
final class SignatureEditor: NSViewController, NSTextViewDelegate {
    private let address: String
    private let initial: [String: Any]?
    private let saveAction: (String, Bool, Bool) async throws -> Void
    private let removeAction: () async throws -> Void
    private let insertAction: (String) -> Void
    private let editor = SignatureTextView()
    private let newMessages = NSButton(checkboxWithTitle: "New messages", target: nil, action: nil)
    private let replies = NSButton(checkboxWithTitle: "Replies", target: nil, action: nil)
    private let save = NSButton(title: "Save", target: nil, action: nil)
    private let insert = NSButton(title: "Insert", target: nil, action: nil)
    private let remove = NSButton(title: "Remove", target: nil, action: nil)
    private let feedback = NSTextField(wrappingLabelWithString: "")
    var dismiss: (() -> Void)?

    init(address: String, initial: [String: Any]?, save: @escaping (String, Bool, Bool) async throws -> Void, remove: @escaping () async throws -> Void, insert: @escaping (String) -> Void) {
        self.address = address; self.initial = initial
        saveAction = save; removeAction = remove; insertAction = insert
        super.init(nibName: nil, bundle: nil)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    override func loadView() {
        let root = SignatureSurface(frame: NSRect(x: 0, y: 0, width: 420, height: 360))
        let stack = NSStackView(); stack.orientation = .vertical; stack.alignment = .leading; stack.spacing = 12; stack.translatesAutoresizingMaskIntoConstraints = false
        root.addSubview(stack)
        NSLayoutConstraint.activate([stack.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 20), stack.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -20), stack.topAnchor.constraint(equalTo: root.topAnchor, constant: 20), stack.bottomAnchor.constraint(equalTo: root.bottomAnchor, constant: -20)])
        let title = NSTextField(labelWithString: "Signature"); title.font = .systemFont(ofSize: 17, weight: .semibold); title.textColor = EmailTheme.ink; stack.addArrangedSubview(title)
        let identity = NSTextField(labelWithString: address); identity.font = .systemFont(ofSize: 12); identity.textColor = EmailTheme.muted; identity.lineBreakMode = .byTruncatingMiddle; stack.addArrangedSubview(identity)
        let scroll = NSScrollView(); scroll.hasVerticalScroller = true; scroll.borderType = .noBorder; scroll.drawsBackground = true; scroll.backgroundColor = EmailTheme.compose
        editor.isRichText = false; editor.font = .systemFont(ofSize: 13); editor.textColor = EmailTheme.ink; editor.backgroundColor = EmailTheme.compose; editor.textContainerInset = NSSize(width: 10, height: 10)
        editor.isVerticallyResizable = true; editor.isHorizontallyResizable = false; editor.autoresizingMask = [.width]; editor.textContainer?.widthTracksTextView = true
        editor.frame = NSRect(x: 0, y: 0, width: 380, height: 150)
        editor.string = initial?["body"] as? String ?? ""; editor.delegate = self
        editor.setAccessibilityLabel("Signature text")
        editor.close = { [weak self] in self?.dismiss?() }; editor.save = { [weak self] in self?.saveClicked() }
        scroll.documentView = editor; stack.addArrangedSubview(scroll)
        scroll.widthAnchor.constraint(equalToConstant: 380).isActive = true; scroll.heightAnchor.constraint(equalToConstant: 150).isActive = true
        newMessages.state = initial?["for_new"] as? Bool == false ? .off : .on
        replies.state = initial?["for_reply"] as? Bool == false ? .off : .on
        let options = NSStackView(views: [newMessages, replies]); options.spacing = 18; stack.addArrangedSubview(options)
        let hint = NSTextField(labelWithString: "Tab next · ⌘Return save · Esc close"); hint.font = .systemFont(ofSize: 11); hint.textColor = EmailTheme.muted; stack.addArrangedSubview(hint)
        feedback.font = .systemFont(ofSize: 11); feedback.textColor = .systemRed; feedback.maximumNumberOfLines = 2; feedback.heightAnchor.constraint(greaterThanOrEqualToConstant: 14).isActive = true; stack.addArrangedSubview(feedback)
        save.target = self; save.action = #selector(saveClicked); save.keyEquivalent = "\r"; save.keyEquivalentModifierMask = [.command]
        insert.target = self; insert.action = #selector(insertClicked)
        remove.target = self; remove.action = #selector(removeClicked); remove.isEnabled = initial != nil
        let cancel = NSButton(title: "Cancel", target: self, action: #selector(cancelClicked)); cancel.keyEquivalent = "\u{1b}"
        let spacer = NSView(); spacer.setContentHuggingPriority(.defaultLow, for: .horizontal)
        let footer = NSStackView(views: [remove, spacer, cancel, insert, save]); footer.spacing = 8; stack.addArrangedSubview(footer); footer.widthAnchor.constraint(equalToConstant: 380).isActive = true
        editor.nextKeyView = newMessages; newMessages.nextKeyView = replies; replies.nextKeyView = insert; insert.nextKeyView = save; save.nextKeyView = cancel; cancel.nextKeyView = editor
        view = root; validate(); preferredContentSize = NSSize(width: 420, height: root.fittingSize.height)
    }
    override func viewDidAppear() { super.viewDidAppear(); view.window?.makeFirstResponder(editor) }
    func textDidChange(_ notification: Notification) { validate() }
    private func validate() {
        let body = editor.string
        let valid = !body.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && body.utf8.count <= 16 * 1024
        save.isEnabled = valid; insert.isEnabled = valid
        feedback.stringValue = body.utf8.count > 16 * 1024 ? "Keep signatures under 16 KiB." : ""
    }
    private func busy(_ value: Bool) { save.isEnabled = !value; insert.isEnabled = !value; remove.isEnabled = !value && initial != nil; editor.isEditable = !value }
    @objc private func cancelClicked() { dismiss?() }
    @objc private func saveClicked() {
        guard save.isEnabled else { return }
        let text = editor.string, forNew = newMessages.state == .on, forReply = replies.state == .on
        busy(true)
        Task { @MainActor in
            do { try await saveAction(text, forNew, forReply); dismiss?() }
            catch { busy(false); validate(); feedback.stringValue = error.localizedDescription }
        }
    }
    @objc private func removeClicked() {
        guard remove.isEnabled else { return }; busy(true)
        Task { @MainActor in
            do { try await removeAction(); dismiss?() }
            catch { busy(false); validate(); feedback.stringValue = error.localizedDescription }
        }
    }
    @objc private func insertClicked() { guard insert.isEnabled else { return }; insertAction(editor.string); dismiss?() }
}

private final class SignatureSurface: NSView {
    override var isFlipped: Bool { true }
    override func draw(_ dirtyRect: NSRect) { EmailTheme.reader.setFill(); bounds.fill() }
}

private final class SignatureTextView: NSTextView {
    var close: (() -> Void)?
    var save: (() -> Void)?
    override func keyDown(with event: NSEvent) {
        if event.keyCode == 53 { close?(); return }
        if event.keyCode == 36 && event.modifierFlags.contains(.command) { save?(); return }
        if event.keyCode == 48 {
            if event.modifierFlags.contains(.shift) { window?.selectPreviousKeyView(self) }
            else { window?.selectNextKeyView(self) }
            return
        }
        super.keyDown(with: event)
    }
}
