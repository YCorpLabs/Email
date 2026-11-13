import AppKit

@MainActor
final class RecipientAutocomplete {
    private struct Suggestion { let email: String; let name: String; let formatted: String }
    private let core: CoreBridge
    private let fields: [NSTextField]
    private let account: () -> String?
    private var selectionObserver: NSObjectProtocol?
    private var task: Task<Void, Never>?
    private var generation = 0
    private weak var field: NSTextField?
    private var context: RecipientCompletionRules.Context?
    private var items: [Suggestion] = []
    private var selection = 0
    private var accepting = false
    private var panel: RecipientSuggestionPanel?
    private var buttons: [RecipientSuggestionButton] = []
    init(core: CoreBridge, fields: [NSTextField], account: @escaping () -> String?) { self.core = core; self.fields = fields; self.account = account }
    @discardableResult func dismiss() -> Bool {
        let visible = panel?.isVisible == true
        generation += 1; task?.cancel(); task = nil
        if let observer = selectionObserver { NotificationCenter.default.removeObserver(observer); selectionObserver = nil }
        if let panel { panel.parent?.removeChildWindow(panel); panel.orderOut(nil) }
        panel = nil; items = []; buttons = []; return visible
    }
    func changed(_ field: NSTextField) {
        guard !accepting else { return }
        dismiss()
        guard fields.contains(where: { $0 === field }), let editor = field.currentEditor() as? NSTextView,
              editor.selectedRange().length == 0, !editor.hasMarkedText(),
              let context = RecipientCompletionRules.context(field.stringValue, caret: editor.selectedRange().location), let accountID = account() else { return }
        let version = generation, text = field.stringValue
        self.field = field; self.context = context
        task = Task { @MainActor [weak self, weak field] in
            try? await Task.sleep(nanoseconds: 120_000_000)
            guard !Task.isCancelled, let self, let field, version == self.generation else { return }
            do {
                let value = try await self.core.callAsync(["op":"suggest_recipients", "account_id":accountID, "query":context.query, "limit":6])
                guard version == self.generation, !Task.isCancelled, field.stringValue == text,
                      self.account() == accountID,
                      RecipientCompletionRules.context(field.stringValue, caret: (field.currentEditor() as? NSTextView)?.selectedRange().location ?? -1) == context, let records = value as? [[String:Any]] else { return }
                var excluded = Set<String>()
                for other in self.fields { excluded.formUnion(RecipientCompletionRules.existing(other.stringValue, excluding: other === field ? context.range : nil)) }
                self.items = records.compactMap { record in
                    guard let email = record["email"] as? String, let formatted = record["formatted"] as? String,
                          !excluded.contains(email.lowercased()), ComposerAddressRules.recipients(formatted)?.count == 1 else { return nil }
                    return Suggestion(email: email, name: record["display_name"] as? String ?? "", formatted: formatted)
                }
                if !self.items.isEmpty { self.selection = 0; self.show(field) }
            } catch { /* Suggestions must never interrupt composing. */ }
        }
    }
    func command(_ control: NSControl, selector: Selector) -> Bool {
        guard panel?.isVisible == true, !items.isEmpty, let field, control === field,
              NSApp.currentEvent?.modifierFlags.intersection([.command,.control,.option]).isEmpty != false else { return false }
        switch NSStringFromSelector(selector) {
        case "moveDown:": selection = (selection+1) % items.count; highlight(); return true
        case "moveUp:": selection = (selection+items.count-1) % items.count; highlight(); return true
        case "insertNewline:": accept(); return true
        case "insertTab:": accept(); field.window?.selectNextKeyView(field); return true
        case "cancelOperation:": return dismiss()
        default: return false
        }
    }
    private func show(_ field: NSTextField) {
        guard let parent = field.window else { return }
        let width = min(440, max(280, field.bounds.width)), height = CGFloat(items.count*42+28)
        let anchor = parent.convertToScreen(field.convert(field.bounds, to: nil))
        let screen = parent.screen?.visibleFrame ?? anchor.insetBy(dx:-1000,dy:-1000)
        let x = max(screen.minX+8,min(anchor.minX,screen.maxX-width-8))
        let y = anchor.minY-height-4 >= screen.minY ? anchor.minY-height-4 : min(anchor.maxY+4,screen.maxY-height)
        let pop = RecipientSuggestionPanel(contentRect:NSRect(x:x,y:y,width:width,height:height),styleMask:[.borderless,.nonactivatingPanel],backing:.buffered,defer:false)
        pop.isOpaque = false; pop.backgroundColor = .clear; pop.hasShadow = true; pop.hidesOnDeactivate = true
        let content = NSStackView(); content.orientation = .vertical; content.spacing = 0; content.alignment = .leading
        content.wantsLayer = true; content.layer?.cornerRadius = 9; content.layer?.masksToBounds = true
        content.layer?.backgroundColor = EmailTheme.cgColor(EmailTheme.compose,in:field.effectiveAppearance)
        content.layer?.borderWidth = 1; content.layer?.borderColor = EmailTheme.cgColor(EmailTheme.line,in:field.effectiveAppearance)
        buttons = []
        for (index,item) in items.enumerated() {
            let button = RecipientSuggestionButton(name:item.name,email:item.email) { [weak self] in self?.selection = index; self?.accept() }
            content.addArrangedSubview(button); button.widthAnchor.constraint(equalTo:content.widthAnchor).isActive = true; button.heightAnchor.constraint(equalToConstant:42).isActive = true; buttons.append(button)
        }
        let hint = NSTextField(labelWithString:"Recent recipients · ↑↓ choose · ↵ add · Tab add & next · Esc dismiss")
        hint.font = .systemFont(ofSize:10); hint.textColor = EmailTheme.muted; hint.alignment = .center
        content.addArrangedSubview(hint); hint.widthAnchor.constraint(equalTo:content.widthAnchor).isActive = true; hint.heightAnchor.constraint(equalToConstant:28).isActive = true
        pop.contentView = content; parent.addChildWindow(pop,ordered:.above); panel = pop; highlight()
        if let editor = field.currentEditor() as? NSTextView {
            selectionObserver = NotificationCenter.default.addObserver(forName: NSTextView.didChangeSelectionNotification, object: editor, queue: .main) { [weak self, weak field] _ in
                Task { @MainActor in
                    guard let self, let field, !self.accepting else { return }
                    if RecipientCompletionRules.context(field.stringValue, caret: (field.currentEditor() as? NSTextView)?.selectedRange().location ?? -1) != self.context { self.dismiss() }
                }
            }
        }
    }
    private func highlight() { for (index,button) in buttons.enumerated() { button.isChosen = index == selection; button.needsDisplay = true } }

    private func accept() {
        guard let field, let context,
              RecipientCompletionRules.context(field.stringValue, caret: (field.currentEditor() as? NSTextView)?.selectedRange().location ?? -1) == context,
              items.indices.contains(selection) else { return }
        let formatted = items[selection].formatted
        guard let result = RecipientCompletionRules.inserting(formatted,in:field.stringValue,context:context) else { return }
        accepting = true
        defer { accepting = false }
        dismiss()
        if let editor = field.currentEditor() as? NSTextView {
            let insertion = (context.range.location == 0 ? "" : " ") + formatted
            editor.breakUndoCoalescing()
            editor.insertText(insertion, replacementRange: context.range)
            editor.setSelectedRange(NSRange(location: result.caret, length: 0))
            editor.breakUndoCoalescing()
        } else { field.stringValue = result.text }

    }
}
private final class RecipientSuggestionPanel: NSPanel {
    override var canBecomeKey: Bool { false }
    override var canBecomeMain: Bool { false }
}
@MainActor private final class RecipientSuggestionButton: NSButton {
    let recipientName: String, recipientEmail: String
    let handler: () -> Void
    var isChosen = false
    init(name:String,email:String,handler:@escaping () -> Void) {
        recipientName = name; recipientEmail = email; self.handler = handler
        super.init(frame:.zero); isBordered = false; target = self; action = #selector(choose)
        setAccessibilityLabel(name.isEmpty ? email : "\(name), \(email)")
    }
    required init?(coder:NSCoder) { fatalError("init(coder:) has not been implemented") }
    override var acceptsFirstResponder: Bool { false }
    override var isFlipped: Bool { true }
    @objc private func choose() { handler() }
    override func draw(_ dirtyRect:NSRect) {
        (isChosen ? EmailTheme.accentSoft : EmailTheme.compose).setFill(); bounds.fill()
        let style = NSMutableParagraphStyle(); style.lineBreakMode = .byTruncatingMiddle
        ((recipientName.isEmpty ? recipientEmail : recipientName) as NSString).draw(in:NSRect(x:12,y:recipientName.isEmpty ? 13:6,width:bounds.width-24,height:18),withAttributes:[.font:NSFont.systemFont(ofSize:12,weight:.medium),.foregroundColor:EmailTheme.ink,.paragraphStyle:style])
        if !recipientName.isEmpty { (recipientEmail as NSString).draw(in:NSRect(x:12,y:23,width:bounds.width-24,height:15),withAttributes:[.font:NSFont.systemFont(ofSize:10),.foregroundColor:EmailTheme.muted,.paragraphStyle:style]) }
    }
}
