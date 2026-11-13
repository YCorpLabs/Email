import AppKit
import UniformTypeIdentifiers

private final class ComposeRecipientDocumentView: NSView {
    override var isFlipped: Bool { true }
}

private final class ComposeBodyView: NSTextView {
    override func draw(_ dirtyRect: NSRect) {
        super.draw(dirtyRect)
        if string.isEmpty {
            ("Write your message…" as NSString).draw(
                at: NSPoint(x: textContainerInset.width, y: textContainerInset.height),
                withAttributes: [.font: NSFont.systemFont(ofSize: 14), .foregroundColor: NSColor.placeholderTextColor]
            )
        }
    }

    override func didChangeText() {
        super.didChangeText()
        needsDisplay = true
    }

    override func keyDown(with event: NSEvent) {
        if event.keyCode == 48 {
            if event.modifierFlags.contains(.shift) { window?.selectPreviousKeyView(self) }
            else { window?.selectNextKeyView(self) }
            return
        }
        super.keyDown(with: event)
    }
}

@MainActor
final class ComposePane: NSView, NSTextFieldDelegate {
    private struct FromIdentity: Equatable {
        let accountID: String
        let address: String
        let isDefault: Bool
    }
    private lazy var recipientCompletion = RecipientAutocomplete(core: core, fields: [to, cc, bcc], account: { [weak self] in self?.selectedFromIdentity?.accountID })
    private let core: CoreBridge
    private let accounts: [MailAccount]
    private var fromIdentities: [FromIdentity] = []
    private let preferredFromAddresses: [String]
    private var userPickedFrom = false
    private let onChanged: () -> Void
    private let onQueued: (String, Bool, Bool) -> Void
    private let onDiscarded: (String, String) -> Void
    private let onClose: () -> Void
    private let from = NSPopUpButton()
    private let to = NSTextField()
    private let cc = NSTextField()
    private let bcc = NSTextField()
    private var copyRows: [NSView] = []
    private let subject = NSTextField()
    private let body = ComposeBodyView()
    private let bodyScroll = NSScrollView()
    private let status = NSTextField(labelWithString: "")
    private let attachments = NSStackView()
    private var attachmentsHeight: NSLayoutConstraint?
    private let attach = NSButton(title: "Attach file", target: nil, action: nil)
    private let save = NSButton(title: "Save draft", target: nil, action: nil)
    private let schedule = NSButton(title: "Schedule…", target: nil, action: nil)
    private let tracking = NSButton(checkboxWithTitle: "Track opens", target: nil, action: nil)
    private var trackingChanged = false
    private let booking = NSButton(title: "Booking link", target: nil, action: nil)
    private let signature = NSButton(title: "Signature…", target: nil, action: nil)
    private var signaturePopover: NSPopover?
    private var signatureGeneration = 0
    private var managedSignature: String?
    private let send = NSButton(title: "Send", target: nil, action: nil)
    private let discard = NSButton(title: "", target: nil, action: nil)
    private var attachedFiles: [[String: Any]] = []
    private var draftID: String?
    private var replyThreadID: String?
    private var initialTo = ""
    private var initialCc = ""
    private var initialBcc = ""
    private var initialSubject = ""
    private var initialBody = ""
    private var initialFromAddress = ""
    private let compactReply: Bool
    private let detailsButton = NSButton(title: "Details", target: nil, action: nil)
    private var detailRows: [NSView] = []
    private let recipientScroll = NSScrollView()
    var onDetailsVisibilityChanged: ((Bool) -> Void)?

    init(core: CoreBridge, accounts: [MailAccount], accountID: String? = nil, preferredFromAddresses: [String] = [], recipient: String? = nil, replyTo: String? = nil, replySubject: String? = nil, replyCc: [String] = [], replyAll: Bool = false, replyThreadID: String? = nil, draft: MailDraft? = nil, supportMessage: (subject: String, body: String)? = nil, onChanged: @escaping () -> Void, onQueued: @escaping (String, Bool, Bool) -> Void, onDiscarded: @escaping (String, String) -> Void, onClose: @escaping () -> Void) {
        self.core = core
        self.accounts = draft.map { saved in accounts.filter { $0.id == saved.accountID } } ?? accounts
        self.preferredFromAddresses = draft.map { [ComposerAddressRules.mailbox($0.from)] } ?? preferredFromAddresses
        self.onChanged = onChanged
        self.onQueued = onQueued
        self.onDiscarded = onDiscarded
        self.onClose = onClose
        self.compactReply = replyTo != nil
        self.replyThreadID = replyThreadID
        super.init(frame: .zero)
        translatesAutoresizingMaskIntoConstraints = false
        wantsLayer = true
        layer?.backgroundColor = EmailTheme.cgColor(compactReply ? EmailTheme.compose : EmailTheme.reader, in: effectiveAppearance)
        build(title: draft != nil ? "Edit draft" : (replyTo.map { replyAll ? "Reply all" : "Reply to \(Self.displayName(from: $0))" } ?? "New message"))
        fromIdentities = self.accounts.flatMap { Self.cachedFromIdentities(for: $0) }
        if let draft, !fromIdentities.contains(where: { $0.accountID == draft.accountID && $0.address.caseInsensitiveCompare(ComposerAddressRules.mailbox(draft.from)) == .orderedSame }) {
            fromIdentities.append(FromIdentity(accountID: draft.accountID, address: ComposerAddressRules.mailbox(draft.from), isDefault: false))
        }
        refreshFromPicker(preferredAccountID: accountID ?? draft?.accountID)
        if let replyTo { to.stringValue = replyTo } else if let recipient { to.stringValue = Self.address(from: recipient) }
        cc.stringValue = replyCc.joined(separator: ", ")
        if let replySubject { subject.stringValue = replySubject.lowercased().hasPrefix("re:") ? replySubject : "Re: \(replySubject)" }
        if let draft {
            draftID = draft.id
            to.stringValue = draft.to
            cc.stringValue = draft.cc
            bcc.stringValue = draft.bcc
            subject.stringValue = draft.subject
            body.string = draft.bodyPlain
        }
        if let supportMessage { subject.stringValue = supportMessage.subject; body.string = supportMessage.body }
        initialTo = to.stringValue
        initialCc = cc.stringValue
        initialBcc = bcc.stringValue
        if !compactReply && (!cc.stringValue.isEmpty || !bcc.stringValue.isEmpty) { copyRows.forEach { $0.isHidden = false }; detailsButton.title = "Hide Cc/Bcc" }
        updateRecipientKeyLoop()
        // Prefilled support messages are new drafts too: Esc must preserve them.
        initialSubject = supportMessage == nil ? subject.stringValue : ""
        initialBody = body.string
        initialFromAddress = selectedFromIdentity?.address ?? ""
        if let draftID { Task { @MainActor in
            await loadAttachments()
            // Builds without open tracking make no tracking calls: the service sends
            // any draft without a pixel and drops tracking once the provider accepts it.
            guard offersTracking,
                  let value = try? await core.callAsync(["op":"tracking_get","message_id":draftID]) as? [String:Any],
                  !trackingChanged else { return }
            tracking.state = value["enabled"] as? Bool == true ? .on : .off
        } }
        Task { @MainActor in await loadSendAsAliases() }
        if draft == nil { Task { @MainActor in await loadDefaultSignature() } }
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        applyTheme()
    }

    func applyTheme() {
        let appearance = effectiveAppearance
        layer?.backgroundColor = EmailTheme.cgColor(compactReply ? EmailTheme.compose : EmailTheme.reader, in: appearance)
        bodyScroll.backgroundColor = EmailTheme.compose
        bodyScroll.layer?.borderColor = EmailTheme.cgColor(compactReply ? EmailTheme.compose : EmailTheme.line, in: appearance)
        layer?.borderColor = EmailTheme.cgColor(EmailTheme.line, in: appearance)
        body.backgroundColor = EmailTheme.compose
        body.needsDisplay = true
        send.layer?.backgroundColor = EmailTheme.cgColor(EmailTheme.accent, in: appearance)
        let sendInk = EmailTheme.isDark(appearance) ? EmailTheme.canvas : NSColor.white
        send.attributedTitle = NSAttributedString(string: "Send", attributes: [
            .foregroundColor: sendInk, .font: NSFont.systemFont(ofSize: 12, weight: .semibold)
        ])
        for field in [to, cc, bcc, subject] {
            field.layer?.borderColor = EmailTheme.cgColor(field.currentEditor() == nil ? EmailTheme.line : EmailTheme.accent, in: appearance)
        }
    }

    func controlTextDidBeginEditing(_ notification: Notification) {
        (notification.object as? NSTextField)?.layer?.borderWidth = 1
        (notification.object as? NSTextField)?.layer?.borderColor = EmailTheme.cgColor(EmailTheme.accent, in: effectiveAppearance)
    }

    func controlTextDidChange(_ notification: Notification) {
        if let field = notification.object as? NSTextField { recipientCompletion.changed(field) }
    }

    func control(_ control: NSControl, textView: NSTextView, doCommandBy commandSelector: Selector) -> Bool {
        recipientCompletion.command(control, selector: commandSelector)
    }

    @discardableResult func dismissRecipientSuggestions() -> Bool { recipientCompletion.dismiss() }

    func controlTextDidEndEditing(_ notification: Notification) {
        recipientCompletion.dismiss()
        (notification.object as? NSTextField)?.layer?.borderWidth = 0
        (notification.object as? NSTextField)?.layer?.borderColor = EmailTheme.cgColor(EmailTheme.line, in: effectiveAppearance)
    }

    func focusBody(in window: NSWindow) {
        window.makeFirstResponder(body)
    }

    func focusRecipient(in window: NSWindow) {
        revealRecipientDetails()
        focusField(to, in: window)
    }

    func focusCopyRecipient(in window: NSWindow, blind: Bool) {
        revealRecipientDetails()
        copyRows.forEach { $0.isHidden = false }
        updateRecipientKeyLoop()
        focusField(blind ? bcc : cc, in: window)
    }

    /// Where a full composer starts typing: the message once anyone is
    /// addressed (an existing draft, a message to a contact), otherwise To.
    func focusInitialField(in window: NSWindow) {
        if hasRecipients { focusBody(in: window) } else { focusRecipient(in: window) }
    }

    /// AppKit selects a text field's whole value when it takes focus, so the
    /// next key would replace every address. Place the caret at the end.
    private func focusField(_ field: NSTextField, in window: NSWindow) {
        window.makeFirstResponder(field)
        field.currentEditor()?.selectedRange = NSRange(location: (field.stringValue as NSString).length, length: 0)
    }

    private var hasRecipients: Bool {
        [to, cc, bcc].contains { !$0.stringValue.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
    }

    static func ownAddresses(for accounts: [MailAccount]) -> [String] {
        accounts.flatMap { [$0.email] + cachedFromIdentities(for: $0).map(\.address) }
    }


    func chooseFrom(in window: NSWindow) {
        revealRecipientDetails()
        window.makeFirstResponder(from)
        from.performClick(nil)
    }

    func paletteSend() { sendClicked() }
    func paletteSaveDraft() { saveClicked() }
    func paletteDiscard() { discardClicked() }
    func paletteSchedule() { scheduleClicked() }
    func paletteAttach() { attachClicked() }
    func paletteSignature() { signatureClicked() }
    /// Track opens comes from the engine's distribution profile: only a profile
    /// with `OPEN_TRACKING` (development, never a distribution) offers it.
    var offersTracking: Bool { DistributionProfile.current?.openTracking == true }
    func paletteTracking() {
        guard offersTracking else { return }
        tracking.state = tracking.state == .on ? .off : .on; trackingToggled()
    }
    func paletteBooking() { guard DistributionProfile.current?.hasScheduling == true else { return }; BookingPanel.present(core: core, insert: { [weak self] url in self?.insertSnippet(url) }) }

    private var selectedFromIdentity: FromIdentity? {
        let index = from.indexOfSelectedItem
        return fromIdentities.indices.contains(index) ? fromIdentities[index] : nil
    }

    private static func sendAsCacheKey(_ accountID: String) -> String { "Email.sendAs.v1." + accountID }

    /// Address metadata is local-only; never cache access tokens in preferences.
    private static func cachedFromIdentities(for account: MailAccount) -> [FromIdentity] {
        guard account.provider == "gmail",
              let entries = UserDefaults.standard.array(forKey: sendAsCacheKey(account.id)) as? [[String: Any]] else {
            return [FromIdentity(accountID: account.id, address: account.email, isDefault: true)]
        }
        let identities = entries.compactMap { entry -> FromIdentity? in
            guard let address = entry["email"] as? String,
                  ComposerAddressRules.recipients(address)?.count == 1,
                  ComposerAddressRules.mailbox(address) == address else { return nil }
            return FromIdentity(accountID: account.id, address: address, isDefault: entry["is_default"] as? Bool ?? false)
        }
        return identities.isEmpty ? [FromIdentity(accountID: account.id, address: account.email, isDefault: true)] : identities
    }

    @objc private func fromChanged() {
        userPickedFrom = true; recipientCompletion.dismiss(); signaturePopover?.close()
        Task { @MainActor in await loadDefaultSignature() }
    }

    private func refreshFromPicker(preferredAccountID: String?) {
        let old = selectedFromIdentity
        let target: FromIdentity? = {
            if userPickedFrom, let old, fromIdentities.contains(old) { return old }
            if let preferredAccountID {
                for address in preferredFromAddresses {
                    if let match = fromIdentities.first(where: { $0.accountID == preferredAccountID && $0.address.caseInsensitiveCompare(address) == .orderedSame }) { return match }
                }
                if let fallback = fromIdentities.first(where: { $0.accountID == preferredAccountID && $0.isDefault }) { return fallback }
                if let fallback = fromIdentities.first(where: { $0.accountID == preferredAccountID }) { return fallback }
            }
            if let old, fromIdentities.contains(old) { return old }
            return fromIdentities.first(where: \.isDefault) ?? fromIdentities.first
        }()
        from.removeAllItems()
        from.addItems(withTitles: fromIdentities.map { $0.address })
        if let target, let index = fromIdentities.firstIndex(of: target) { from.selectItem(at: index) }
        from.toolTip = "Choose any verified From address on this account"
        if old != selectedFromIdentity { Task { @MainActor in await loadDefaultSignature() } }
    }

    private func loadSendAsAliases() async {
        let selectedAccountID = selectedFromIdentity?.accountID
        for account in accounts where account.provider == "gmail" {
            do {
                let value = try await core.callAsync(["op": "list_send_as", "account_id": account.id])
                guard let aliases = value as? [[String: Any]] else { continue }
                let available = aliases.compactMap { item -> FromIdentity? in
                    guard let address = item["email"] as? String, !address.isEmpty else { return nil }
                    return FromIdentity(accountID: account.id, address: address, isDefault: item["is_default"] as? Bool ?? false)
                }
                guard !available.isEmpty else { continue }
                UserDefaults.standard.set(available.map { identity -> [String: Any] in ["email": identity.address, "is_default": identity.isDefault] }, forKey: Self.sendAsCacheKey(account.id))
                fromIdentities.removeAll { $0.accountID == account.id }
                let insertion = accounts.prefix { $0.id != account.id }.reduce(0) { count, preceding in
                    count + fromIdentities.filter { $0.accountID == preceding.id }.count
                }
                fromIdentities.insert(contentsOf: available, at: min(insertion, fromIdentities.count))
                refreshFromPicker(preferredAccountID: selectedAccountID)
            } catch {
                // The account's primary address remains usable when alias lookup is offline.
                status.stringValue = "Could not load additional From addresses."
            }
        }
    }

    private static func address(from sender: String) -> String {
        if let start = sender.firstIndex(of: "<"), let end = sender[start...].firstIndex(of: ">") {
            return String(sender[sender.index(after: start)..<end])
        }
        return sender
    }

    private static func displayName(from sender: String) -> String {
        let name = sender.split(separator: "<", maxSplits: 1).first.map(String.init)?
            .trimmingCharacters(in: .whitespacesAndNewlines)
            .trimmingCharacters(in: CharacterSet(charactersIn: "\"")) ?? ""
        return name.isEmpty ? address(from: sender) : name
    }

    private func build(title: String) {
        let stack = NSStackView()
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 13
        stack.translatesAutoresizingMaskIntoConstraints = false
        addSubview(stack)
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: leadingAnchor, constant: compactReply ? 22 : 28),
            stack.trailingAnchor.constraint(equalTo: trailingAnchor, constant: compactReply ? -22 : -28),
            stack.topAnchor.constraint(equalTo: topAnchor, constant: compactReply ? 14 : 26),
            stack.bottomAnchor.constraint(equalTo: bottomAnchor, constant: compactReply ? -12 : -24)
        ])
        if compactReply { stack.spacing = 7 }
        let header = NSStackView()
        header.orientation = .horizontal
        header.alignment = .centerY
        let heading = NSTextField(labelWithString: title)
        heading.font = .systemFont(ofSize: compactReply ? 15 : 22, weight: .semibold)
        let spacer = NSView()
        spacer.setContentHuggingPriority(.defaultLow, for: .horizontal)
        let close = NSButton(title: "Close", target: self, action: #selector(closeClicked))
        close.isBordered = false
        close.toolTip = "Save draft and return to message (Esc)"
        close.font = .systemFont(ofSize: 11, weight: .medium)
        close.contentTintColor = EmailTheme.muted
        if compactReply {
            close.title = ""
            close.image = NSImage(systemSymbolName: "xmark", accessibilityDescription: "Close reply")
            close.imagePosition = .imageOnly
            close.contentTintColor = .secondaryLabelColor
            close.setAccessibilityLabel("Close reply")
        }
        header.addArrangedSubview(heading)
        header.addArrangedSubview(spacer)
        detailsButton.title = compactReply ? "Details" : "Cc/Bcc"
        detailsButton.isBordered = false
        detailsButton.font = .systemFont(ofSize: 11, weight: .medium)
        detailsButton.contentTintColor = .secondaryLabelColor
        detailsButton.toolTip = "Edit recipients, Cc and Bcc (or find them in Commands)"
        detailsButton.target = self
        detailsButton.action = #selector(toggleDetailsClicked)
        header.addArrangedSubview(detailsButton)
        header.addArrangedSubview(close)
        stack.addArrangedSubview(header)
        header.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true
        if !compactReply {
            let keyboardHint = ShortcutStrip()
            keyboardHint.items = [("Tab", "Next field"), ("⇧Tab", "Previous"), ("⌘↵", "Send"), ("Esc", "Close")]
            stack.addArrangedSubview(keyboardHint)
            keyboardHint.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true
            keyboardHint.heightAnchor.constraint(equalToConstant: 32).isActive = true
        }

        from.target = self
        from.action = #selector(fromChanged)
        to.placeholderString = "Recipient email"
        cc.placeholderString = "Cc recipients"
        bcc.placeholderString = "Bcc recipients (hidden from other recipients)"
        subject.placeholderString = "Subject"
        for field in [to, cc, bcc, subject] {
            field.delegate = self
            field.focusRingType = .none
            field.wantsLayer = true
            field.layer?.cornerRadius = 5
            field.layer?.borderWidth = 0
            field.isBordered = false
            field.drawsBackground = false
            field.font = .systemFont(ofSize: 13)
        }
        let recipientRows = compactReply ? NSStackView() : stack
        if compactReply {
            recipientRows.orientation = .vertical; recipientRows.alignment = .leading; recipientRows.spacing = 7
            recipientRows.translatesAutoresizingMaskIntoConstraints = false
            let document = ComposeRecipientDocumentView(); document.translatesAutoresizingMaskIntoConstraints = false
            document.addSubview(recipientRows)
            recipientScroll.documentView = document
            recipientScroll.hasVerticalScroller = true; recipientScroll.autohidesScrollers = true
            recipientScroll.drawsBackground = false; recipientScroll.borderType = .noBorder
            recipientScroll.translatesAutoresizingMaskIntoConstraints = false
            recipientScroll.isHidden = true
            stack.addArrangedSubview(recipientScroll)
            NSLayoutConstraint.activate([
                recipientScroll.widthAnchor.constraint(equalTo: stack.widthAnchor),
                recipientScroll.heightAnchor.constraint(lessThanOrEqualToConstant: 160),
                document.widthAnchor.constraint(equalTo: recipientScroll.contentView.widthAnchor),
                recipientRows.leadingAnchor.constraint(equalTo: document.leadingAnchor),
                recipientRows.trailingAnchor.constraint(equalTo: document.trailingAnchor),
                recipientRows.topAnchor.constraint(equalTo: document.topAnchor),
                recipientRows.bottomAnchor.constraint(equalTo: document.bottomAnchor)
            ])
            let minimumHeight = recipientScroll.heightAnchor.constraint(greaterThanOrEqualToConstant: 100)
            minimumHeight.priority = NSLayoutConstraint.Priority(650); minimumHeight.isActive = true
            let preferredHeight = recipientScroll.heightAnchor.constraint(equalToConstant: 160)
            preferredHeight.priority = NSLayoutConstraint.Priority(700); preferredHeight.isActive = true
        }
        for (label, field) in [("From", from as NSView), ("To", to as NSView), ("Cc", cc as NSView), ("Bcc", bcc as NSView), ("Subject", subject as NSView)] {
            let row = NSStackView()
            row.orientation = .horizontal
            row.alignment = .centerY
            row.spacing = 12
            row.heightAnchor.constraint(greaterThanOrEqualToConstant: 30).isActive = true
            let name = NSTextField(labelWithString: label)
            name.font = .systemFont(ofSize: 12, weight: .medium)
            name.textColor = .secondaryLabelColor
            name.widthAnchor.constraint(equalToConstant: 55).isActive = true
            field.setContentHuggingPriority(.defaultLow, for: .horizontal)
            row.addArrangedSubview(name)
            row.addArrangedSubview(field)
            recipientRows.addArrangedSubview(row)
            row.widthAnchor.constraint(equalTo: recipientRows.widthAnchor).isActive = true
            if label == "Cc" || label == "Bcc" { copyRows.append(row); row.isHidden = true }
            if compactReply { row.isHidden = true; detailRows.append(row) }
        }
        attachments.orientation = .vertical
        attachments.alignment = .leading
        attachments.spacing = 3
        attachments.translatesAutoresizingMaskIntoConstraints = false
        let attachmentScroll = NSScrollView()
        let attachmentDocument = NSView()
        attachmentDocument.translatesAutoresizingMaskIntoConstraints = false
        attachmentScroll.documentView = attachmentDocument
        attachmentDocument.addSubview(attachments)
        attachmentScroll.hasVerticalScroller = true
        attachmentScroll.drawsBackground = false
        attachmentScroll.borderType = .noBorder
        attachmentScroll.translatesAutoresizingMaskIntoConstraints = false
        stack.addArrangedSubview(attachmentScroll)
        attachmentScroll.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true
        NSLayoutConstraint.activate([
            attachmentDocument.widthAnchor.constraint(equalTo: attachmentScroll.contentView.widthAnchor),
            attachments.leadingAnchor.constraint(equalTo: attachmentDocument.leadingAnchor),
            attachments.trailingAnchor.constraint(equalTo: attachmentDocument.trailingAnchor),
            attachments.topAnchor.constraint(equalTo: attachmentDocument.topAnchor),
            attachments.bottomAnchor.constraint(equalTo: attachmentDocument.bottomAnchor)
        ])
        attachmentsHeight = attachmentScroll.heightAnchor.constraint(equalToConstant: 0)
        attachmentsHeight?.isActive = true
        let messageLabel = NSTextField(labelWithString: "Message")
        messageLabel.font = .systemFont(ofSize: 12, weight: .medium)
        messageLabel.textColor = .secondaryLabelColor
        if !compactReply {
            stack.addArrangedSubview(messageLabel)
            stack.setCustomSpacing(8, after: messageLabel)
        }
        let scroll = bodyScroll
        scroll.hasVerticalScroller = true
        scroll.drawsBackground = true
        scroll.backgroundColor = EmailTheme.compose
        scroll.borderType = .noBorder
        scroll.wantsLayer = true
        scroll.layer?.cornerRadius = 8
        scroll.layer?.borderColor = EmailTheme.cgColor(EmailTheme.line, in: effectiveAppearance)
        scroll.layer?.borderWidth = compactReply ? 0 : 1
        scroll.layer?.masksToBounds = true
        body.frame = NSRect(x: 0, y: 0, width: 600, height: 300)
        body.minSize = NSSize(width: 0, height: 0)
        body.maxSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude)
        body.isVerticallyResizable = true
        body.isHorizontallyResizable = false
        body.autoresizingMask = [.width]
        body.textContainer?.widthTracksTextView = true
        body.textContainer?.containerSize = NSSize(width: 600, height: CGFloat.greatestFiniteMagnitude)
        scroll.documentView = body
        body.font = .systemFont(ofSize: 14)
        body.textColor = .textColor
        body.backgroundColor = scroll.backgroundColor
        if #available(macOS 15.2, *) { body.writingToolsBehavior = .none }
        body.textContainerInset = NSSize(width: 16, height: compactReply ? 8 : 16)
        body.isRichText = false
        body.allowsUndo = true
        updateRecipientKeyLoop()
        stack.addArrangedSubview(scroll)
        scroll.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true
        scroll.setContentHuggingPriority(.defaultLow, for: .vertical)
        scroll.setContentCompressionResistancePriority(.defaultLow, for: .vertical)
        let minimumBodyHeight = scroll.heightAnchor.constraint(greaterThanOrEqualToConstant: compactReply ? 80 : 100)
        if compactReply { minimumBodyHeight.priority = .defaultHigh }
        minimumBodyHeight.isActive = true
        scroll.heightAnchor.constraint(greaterThanOrEqualToConstant: 0).isActive = true
        status.font = .systemFont(ofSize: 12)
        status.textColor = .secondaryLabelColor
        status.maximumNumberOfLines = 2
        stack.addArrangedSubview(status)

        let actions = NSStackView()
        actions.orientation = .horizontal
        actions.spacing = 10
        let actionSpacer = NSView()
        actionSpacer.setContentHuggingPriority(.defaultLow, for: .horizontal)
        save.target = self
        save.action = #selector(saveClicked)
        attach.target = self
        attach.action = #selector(attachClicked)
        attach.image = NSImage(systemSymbolName: "paperclip", accessibilityDescription: nil)
        attach.imagePosition = .imageLeading
        schedule.target = self
        schedule.action = #selector(scheduleClicked)
        tracking.target = self
        tracking.action = #selector(trackingToggled)
        tracking.toolTip = "Privacy protection and mail scanners can affect open tracking. Requires Scheduling connection."
        tracking.font = .systemFont(ofSize:11)
        booking.target = self
        booking.action = #selector(bookingClicked)
        booking.toolTip = "Insert a booking link · also in Commands"
        signature.target = self
        signature.action = #selector(signatureClicked)
        signature.toolTip = "Edit or insert a signature for the selected From address"
        do {
            for button in [attach, schedule, signature, booking, save] {
                button.isBordered = false
                button.font = .systemFont(ofSize: 11, weight: .medium)
                button.contentTintColor = .secondaryLabelColor
            }
        }
        discard.target = self
        discard.action = #selector(discardClicked)
        discard.isBordered = false
        discard.image = NSImage(systemSymbolName: "trash", accessibilityDescription: "Discard draft")
        discard.imagePosition = .imageOnly
        discard.contentTintColor = .secondaryLabelColor
        discard.toolTip = "Discard draft · undo from Drafts with Z"
        discard.setAccessibilityLabel("Discard draft")
        send.target = self
        send.action = #selector(sendClicked)
        send.isBordered = false
        send.wantsLayer = true
        send.layer?.cornerRadius = 7
        send.widthAnchor.constraint(equalToConstant: 62).isActive = true
        send.heightAnchor.constraint(equalToConstant: 28).isActive = true
        send.toolTip = "Send (⌘Return)"
        attach.toolTip = "Add an attachment"
        schedule.toolTip = "Schedule this message"
        save.toolTip = "Save this message to Drafts"
        send.keyEquivalent = "\r"
        send.keyEquivalentModifierMask = [.command]
        if compactReply {
            actions.addArrangedSubview(attach)
            actions.addArrangedSubview(schedule)
            actions.addArrangedSubview(signature)
            actions.addArrangedSubview(booking)
            actions.addArrangedSubview(actionSpacer)
            actions.addArrangedSubview(discard)
            actions.addArrangedSubview(save)
            actions.addArrangedSubview(send)
        } else {
            actions.addArrangedSubview(attach)
            actions.addArrangedSubview(signature)
            actions.addArrangedSubview(booking)
            actions.addArrangedSubview(actionSpacer)
            actions.addArrangedSubview(discard)
            actions.addArrangedSubview(save)
            actions.addArrangedSubview(schedule)
            actions.addArrangedSubview(send)
        }
        // Track opens needs a profile with OPEN_TRACKING (which implies its
        // Scheduling service); booking links need the Scheduling service.
        if offersTracking { stack.addArrangedSubview(tracking) }
        stack.addArrangedSubview(actions)
        booking.isHidden = DistributionProfile.current?.hasScheduling != true
        actions.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true
        applyTheme()
    }

    @objc private func trackingToggled() {
        guard offersTracking else { return }
        trackingChanged = true
        status.stringValue = tracking.state == .on ? "Open tracking on" : "Open tracking off"
        guard tracking.state == .on else { return }
        Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                let connection = try await core.callAsync(["op":"booking_status"]) as? [String:Any] ?? [:]
                if connection["connected"] as? Bool != true && tracking.state == .on {
                    tracking.state = .off
                    status.stringValue = "Connect scheduling, then enable Track opens."
                    BookingPanel.present(core:core)
                }
            } catch { tracking.state = .off; status.stringValue = error.localizedDescription }
        }
    }
    @objc private func bookingClicked() { paletteBooking() }
    @objc private func closeClicked() { recipientCompletion.dismiss(); requestClose() }

    private func updateRecipientKeyLoop() {
        from.nextKeyView = to
        to.nextKeyView = copyRows.first?.isHidden == false ? cc : subject
        cc.nextKeyView = bcc
        bcc.nextKeyView = subject
        subject.nextKeyView = body
        body.nextKeyView = compactReply && detailRows.first?.isHidden == true ? detailsButton : from
    }

    private func revealRecipientDetails() {
        if compactReply {
            recipientScroll.isHidden = false
            detailRows.forEach { $0.isHidden = false }; detailsButton.title = "Hide details"
            onDetailsVisibilityChanged?(true)
        }
        updateRecipientKeyLoop()
    }

    @objc private func toggleDetailsClicked() {
        if compactReply {
            let show = detailRows.first?.isHidden ?? true
            recipientScroll.isHidden = !show
            detailRows.forEach { $0.isHidden = !show }
            detailsButton.title = show ? "Hide details" : "Details"
            onDetailsVisibilityChanged?(show)
        } else {
            let show = copyRows.first?.isHidden ?? true
            copyRows.forEach { $0.isHidden = !show }
            detailsButton.title = show ? "Hide Cc/Bcc" : "Cc/Bcc"
        }
        updateRecipientKeyLoop()
    }

    func insertSnippet(_ text: String) {
        window?.makeFirstResponder(body)
        body.insertText(text, replacementRange: body.selectedRange())
    }

    private func loadDefaultSignature() async {
        signatureGeneration += 1
        let generation = signatureGeneration
        guard draftID == nil, let identity = selectedFromIdentity else { return }
        let original = body.string
        guard original.isEmpty || original == managedSignature else { return }
        do {
            let value = try await core.callAsync(["op": "get_signature", "account_id": identity.accountID, "email": identity.address])
            guard generation == signatureGeneration, selectedFromIdentity == identity, draftID == nil, body.string == original else { return }
            let preference = value as? [String: Any]
            let enabled = preference?[compactReply ? "for_reply" : "for_new"] as? Bool ?? false
            let text = enabled ? preference?["body"] as? String : nil
            let rendered = text.map { "\n\n-- \n" + $0 } ?? ""
            body.string = rendered; body.needsDisplay = true
            managedSignature = rendered.isEmpty ? nil : rendered
            initialBody = rendered
            body.setSelectedRange(NSRange(location: 0, length: 0))
        } catch {
            // Signature lookup never blocks drafting or sending.
        }
    }

    @objc private func signatureClicked() {
        guard let identity = selectedFromIdentity else { return }
        recipientCompletion.dismiss()
        if signaturePopover?.isShown == true { signaturePopover?.close(); return }
        Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                let value = try await core.callAsync(["op": "get_signature", "account_id": identity.accountID, "email": identity.address])
                guard selectedFromIdentity == identity, window != nil else { return }
                let panel = SignatureEditor(address: identity.address, initial: value as? [String: Any], save: { [weak self] text, forNew, forReply in
                    guard let self else { return }
                    _ = try await core.callAsync(["op": "upsert_signature", "account_id": identity.accountID, "email": identity.address, "body": text, "for_new": forNew, "for_reply": forReply])
                    await loadDefaultSignature()
                    status.stringValue = "Signature saved for \(identity.address)"
                }, remove: { [weak self] in
                    guard let self else { return }
                    _ = try await core.callAsync(["op": "delete_signature", "account_id": identity.accountID, "email": identity.address])
                    await loadDefaultSignature()
                    status.stringValue = "Signature removed"
                }, insert: { [weak self] text in
                    guard let self else { return }
                    insertSnippet("\n\n-- \n" + text)
                    managedSignature = nil
                })
                let popover = NSPopover(); popover.behavior = .transient; popover.contentViewController = panel
                _ = panel.view; popover.contentSize = panel.preferredContentSize
                panel.dismiss = { [weak self, weak popover] in
                    popover?.close(); self?.window?.makeFirstResponder(self?.body)
                }
                signaturePopover = popover
                popover.show(relativeTo: signature.bounds, of: signature, preferredEdge: .maxY)
            } catch { status.stringValue = error.localizedDescription }
        }
    }

    func saveBodyAsSnippet() {
        guard !body.string.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            status.stringValue = "Write a message before saving a snippet."; return
        }
        let alert = NSAlert()
        alert.messageText = "Save as snippet"
        alert.informativeText = "Reuse this message text in any reply or draft. Snippets stay on this device."
        let name = NSTextField(frame: NSRect(x: 0, y: 0, width: 300, height: 26))
        name.placeholderString = "Snippet name"
        alert.accessoryView = name
        alert.addButton(withTitle: "Save snippet")
        alert.addButton(withTitle: "Cancel")
        alert.window.initialFirstResponder = name
        guard alert.runModal() == .alertFirstButtonReturn else { return }
        do {
            _ = try core.request(["op":"upsert_snippet", "name":name.stringValue, "body":body.string])
            status.stringValue = "Snippet saved · find it in Commands → Insert snippet"
        } catch { status.stringValue = error.localizedDescription }
        window?.makeFirstResponder(body)
    }

    private var hasUnsavedChanges: Bool {
        to.stringValue != initialTo || cc.stringValue != initialCc || bcc.stringValue != initialBcc || subject.stringValue != initialSubject || body.string != initialBody || selectedFromIdentity?.address != initialFromAddress || trackingChanged
    }

    func requestClose() {
        recipientCompletion.dismiss()
        guard hasUnsavedChanges else { onClose(); return }
        if recipientsWereCleared && !confirmSaveWithoutRecipients() { return }
        saveClicked()
    }

    /// Closing saves silently, so a stray key that wiped every address would
    /// otherwise be saved without the user noticing.
    private var recipientsWereCleared: Bool {
        let hadRecipients = [initialTo, initialCc, initialBcc].contains { !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
        return hadRecipients && !hasRecipients
    }

    /// Returns true to save without recipients. The first button restores the
    /// recipients from when the draft was opened and keeps the composer open.
    private func confirmSaveWithoutRecipients() -> Bool {
        let alert = NSAlert()
        alert.messageText = "Save this draft without recipients?"
        alert.informativeText = "To, Cc and Bcc are now empty. Restore the recipients it had when you opened it, or save it without any."
        let restore = alert.addButton(withTitle: "Restore Recipients")
        restore.keyEquivalent = "\u{1b}"
        alert.addButton(withTitle: "Save Without Recipients").keyEquivalent = ""
        guard alert.runModal() == .alertSecondButtonReturn else {
            to.stringValue = initialTo
            cc.stringValue = initialCc
            bcc.stringValue = initialBcc
            if !initialCc.isEmpty || !initialBcc.isEmpty { copyRows.forEach { $0.isHidden = false }; updateRecipientKeyLoop() }
            if let window { focusBody(in: window) }
            return false
        }
        return true
    }

    /// Discard without a confirmation: unsaved edits are saved first so the
    /// Drafts undo restores exactly what was on screen. An untouched new
    /// message has nothing worth restoring and simply closes.
    @objc private func discardClicked() {
        recipientCompletion.dismiss()
        guard draftID != nil || hasUnsavedChanges else { onClose(); return }
        guard let payload = payload(requireRecipient: false) else { return }
        let changed = hasUnsavedChanges
        let title = subject.stringValue
        busy(true, message: "Discarding draft…")
        Task { @MainActor in
            do {
                let id = changed || draftID == nil ? try await persist(payload) : draftID!
                _ = try await core.callAsync(["op": "discard_draft", "message_id": id])
                onDiscarded(id, title)
            } catch {
                busy(false, message: error.localizedDescription)
                status.textColor = .systemRed
            }
        }
    }

    private func payload(requireRecipient: Bool) -> [String: Any]? {
        guard from.indexOfSelectedItem >= 0, from.indexOfSelectedItem < fromIdentities.count else {
            status.stringValue = "Connect an account first."
            return nil
        }
        let recipient = to.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        let copy = cc.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        let blindCopy = bcc.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        let populated = [recipient,copy,blindCopy].filter { !$0.isEmpty }
        guard !requireRecipient || (!populated.isEmpty && populated.allSatisfy { ComposerAddressRules.recipients($0) != nil }) else {
            status.textColor = .systemRed
            status.stringValue = "Enter valid To, Cc or Bcc addresses, separated by commas."
            return nil
        }
        let identity = fromIdentities[from.indexOfSelectedItem]
        var request: [String: Any] = ["op": "compose_draft", "account_id": identity.accountID, "from": identity.address, "to": recipient, "cc": copy, "bcc": blindCopy, "subject": subject.stringValue, "body_plain": body.string]
        if let replyThreadID { request["reply_thread_id"] = replyThreadID }
        return request
    }

    private func busy(_ active: Bool, message: String) {
        tracking.isEnabled = !active
        save.isEnabled = !active
        discard.isEnabled = !active
        attach.isEnabled = !active
        schedule.isEnabled = !active
        send.isEnabled = !active
        from.isEnabled = !active
        status.textColor = .secondaryLabelColor
        status.stringValue = message
    }

    private func persist(_ payload: [String: Any]) async throws -> String {
        if let draftID {
            var update = payload
            update["op"] = "update_draft"
            update["message_id"] = draftID
            _ = try await core.callAsync(update)
            if trackingChanged { _ = try await core.callAsync(["op":"tracking_set","message_id":draftID,"enabled":tracking.state == .on]); trackingChanged = false }
            return draftID
        }
        let result = try await core.callAsync(payload)
        guard let id = (result as? [String: Any])?["id"] as? String else { throw MailServiceError(message: "The mail engine did not return a draft ID.") }
        draftID = id
        if tracking.state == .on { _ = try await core.callAsync(["op":"tracking_set","message_id":id,"enabled":true]) }
        trackingChanged = false
        return id
    }

    @objc private func saveClicked() {
        guard let payload = payload(requireRecipient: false) else { return }
        busy(true, message: "Saving draft…")
        Task { @MainActor in
            do {
                _ = try await persist(payload)
                onChanged()
                onClose()
            } catch {
                busy(false, message: error.localizedDescription)
                status.textColor = .systemRed
            }
        }
    }

    private static func isFeedback(_ payload: [String: Any]) -> Bool {
        DistributionProfile.current?.isFeedbackRecipient(payload["to"] as? String ?? "") == true
    }

    @objc private func sendClicked() {
        guard let payload = payload(requireRecipient: true) else { return }
        busy(true, message: "Queuing with 10 seconds to undo…")
        Task { @MainActor in
            do {
                let id = try await persist(payload)
                _ = try await core.callAsync(["op": "queue_send_draft", "message_id": id, "undo_seconds": 10])
                onClose()
                onQueued(id, false, Self.isFeedback(payload))
            } catch {
                busy(false, message: draftID == nil ? error.localizedDescription : "Draft saved. Could not queue: \(error.localizedDescription)")
                status.textColor = .systemRed
            }
        }
    }

    @objc private func scheduleClicked() {
        guard let payload = payload(requireRecipient: true) else { return }
        let alert = NSAlert()
        alert.messageText = "Send later"
        alert.informativeText = "The message stays in Outbox until this time. You can cancel it before it sends."
        let picker = NSDatePicker(frame: NSRect(x: 0, y: 0, width: 290, height: 28))
        picker.datePickerStyle = .textFieldAndStepper
        picker.datePickerElements = [.yearMonthDay, .hourMinute]
        picker.minDate = Date().addingTimeInterval(60)
        picker.dateValue = Date().addingTimeInterval(86_400)
        alert.accessoryView = picker
        alert.addButton(withTitle: "Schedule")
        alert.addButton(withTitle: "Cancel")
        guard alert.runModal() == .alertFirstButtonReturn else { return }
        let chosen = picker.dateValue
        guard chosen > Date().addingTimeInterval(30) else { status.stringValue = "Choose a future time."; return }
        busy(true, message: "Scheduling…")
        Task { @MainActor in
            do {
                let id = try await persist(payload)
                _ = try await core.callAsync(["op": "schedule_draft", "message_id": id, "send_at_ms": Int64(chosen.timeIntervalSince1970 * 1000)])
                onClose()
                onQueued(id, true, Self.isFeedback(payload))
            } catch {
                busy(false, message: error.localizedDescription)
                status.textColor = .systemRed
            }
        }
    }

    @objc private func attachClicked() {
        guard let window else { return }
        let panel = NSOpenPanel()
        panel.canChooseFiles = true
        panel.canChooseDirectories = false
        panel.allowsMultipleSelection = true
        panel.message = "Choose files to attach (up to 10 files, 25 MB total)."
        panel.beginSheetModal(for: window) { [weak self] result in
            guard result == .OK else { return }
            Task { @MainActor [weak self] in await self?.attachFiles(panel.urls) }
        }
    }

    private func attachFiles(_ urls: [URL]) async {
        guard let payload = payload(requireRecipient: false) else { return }
        guard attachedFiles.count + urls.count <= 10 else {
            status.stringValue = "A draft can have up to 10 attachments."
            return
        }
        busy(true, message: "Adding attachments…")
        do {
            let id = try await persist(payload)
            var total = attachedFiles.reduce(Int64(0)) { $0 + ((($1["size"] as? NSNumber)?.int64Value) ?? 0) }
            for url in urls {
                let fileName = url.lastPathComponent
                let scoped = url.startAccessingSecurityScopedResource()
                defer { if scoped { url.stopAccessingSecurityScopedResource() } }
                let attributes = try FileManager.default.attributesOfItem(atPath: url.path)
                let size = (attributes[.size] as? NSNumber)?.int64Value ?? 0
                guard size > 0, size <= 20 * 1024 * 1024, total + size <= 25 * 1024 * 1024 else {
                    throw MailServiceError(message: "Attachments must be at most 20 MB each and 25 MB total.")
                }
                let data = try await Task.detached(priority: .userInitiated) { try Data(contentsOf: url) }.value
                guard data.count <= 20 * 1024 * 1024, total + Int64(data.count) <= 25 * 1024 * 1024 else {
                    throw MailServiceError(message: "Attachment size changed while reading the file.")
                }
                let mimeType = UTType(filenameExtension: url.pathExtension)?.preferredMIMEType ?? "application/octet-stream"
                _ = try await core.callAsync(["op": "add_draft_attachment", "message_id": id, "filename": fileName,
                                              "mime_type": mimeType, "content_base64": data.base64EncodedString()])
                total += Int64(data.count)
            }
            await loadAttachments()
            busy(false, message: "Attachments ready")
        } catch {
            await loadAttachments()
            busy(false, message: error.localizedDescription)
            status.textColor = .systemRed
        }
    }

    private func loadAttachments() async {
        guard let draftID else { return }
        do {
            attachedFiles = try await core.callAsync(["op": "list_draft_attachments", "message_id": draftID]) as? [[String: Any]] ?? []
            refreshAttachmentRows()
        } catch {
            status.textColor = .systemRed
            status.stringValue = "Attachments unavailable: \(error.localizedDescription)"
        }
    }

    private func refreshAttachmentRows() {
        attachments.arrangedSubviews.forEach { $0.removeFromSuperview() }
        for (index, file) in attachedFiles.enumerated() {
            let row = NSStackView()
            row.orientation = .horizontal
            row.alignment = .centerY
            row.spacing = 8
            let name = NSTextField(labelWithString: file["filename"] as? String ?? "Attachment")
            name.font = .systemFont(ofSize: 11)
            name.lineBreakMode = .byTruncatingMiddle
            let bytes = (file["size"] as? NSNumber)?.int64Value ?? 0
            let size = NSTextField(labelWithString: ByteCountFormatter.string(fromByteCount: bytes, countStyle: .file))
            size.font = .systemFont(ofSize: 10)
            size.textColor = .secondaryLabelColor
            let remove = NSButton(title: "Remove", target: self, action: #selector(removeAttachmentClicked(_:)))
            remove.isBordered = false
            remove.font = .systemFont(ofSize: 10)
            remove.tag = index
            row.addArrangedSubview(name)
            row.addArrangedSubview(size)
            row.addArrangedSubview(remove)
            attachments.addArrangedSubview(row)
        }
        attachmentsHeight?.constant = attachedFiles.isEmpty ? 0 : CGFloat(min(80, attachedFiles.count * 24 + 4))
    }

    @objc private func removeAttachmentClicked(_ sender: NSButton) {
        guard let draftID, attachedFiles.indices.contains(sender.tag),
              let attachmentID = attachedFiles[sender.tag]["id"] as? String else { return }
        busy(true, message: "Removing attachment…")
        Task { @MainActor in
            do {
                _ = try await core.callAsync(["op": "remove_draft_attachment", "message_id": draftID, "attachment_id": attachmentID])
                await loadAttachments()
                busy(false, message: "Attachment removed")
            } catch {
                busy(false, message: error.localizedDescription)
                status.textColor = .systemRed
            }
        }
    }
}
