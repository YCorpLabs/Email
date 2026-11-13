import AppKit
import Contacts
import UniformTypeIdentifiers

private final class FallbackAvatarView: NSView {
    var monogram = "" { didSet { needsDisplay = true } }
    var identity = "" { didSet { needsDisplay = true } }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        needsDisplay = true
    }

    override func draw(_ dirtyRect: NSRect) {
        let palettes: [(UInt32, UInt32, UInt32, UInt32)] = [
            (0xE8DFD7, 0x51453C, 0x3A332F, 0xE8D8CA),
            (0xE4E1E8, 0x4D4655, 0x37333C, 0xDDD4E7),
            (0xDEE3E4, 0x445156, 0x30383B, 0xD1E0E2),
            (0xE8E3D9, 0x544C3D, 0x39362F, 0xE6DCCB)
        ]
        let hash = identity.lowercased().utf8.reduce(UInt64(5381)) { ($0 &* 33) &+ UInt64($1) }
        let palette = palettes[Int(hash % UInt64(palettes.count))]
        let dark = EmailTheme.isDark(effectiveAppearance)
        func color(_ hex: UInt32) -> NSColor {
            NSColor(calibratedRed: CGFloat((hex >> 16) & 0xff) / 255,
                    green: CGFloat((hex >> 8) & 0xff) / 255,
                    blue: CGFloat(hex & 0xff) / 255, alpha: 1)
        }
        let fill = color(dark ? palette.2 : palette.0)
        let ink = color(dark ? palette.3 : palette.1)
        let outline = ink.withAlphaComponent(dark ? 0.16 : 0.1)
        let path = NSBezierPath(roundedRect: bounds.insetBy(dx: 0.5, dy: 0.5), xRadius: 13, yRadius: 13)
        fill.setFill()
        path.fill()
        outline.setStroke()
        path.lineWidth = 1
        path.stroke()
        let font = NSFont.systemFont(ofSize: monogram.count > 1 ? 15 : 18, weight: .semibold)
        let attributes: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: ink, .kern: 0.4]
        let text = monogram as NSString
        let size = text.size(withAttributes: attributes)
        text.draw(at: NSPoint(x: (bounds.width - size.width) / 2,
                              y: (bounds.height - size.height) / 2 + 1), withAttributes: attributes)
    }

    static func initials(for display: String, address: String) -> String {
        let source = display.contains("@") ? (address.split(separator: "@").last.map(String.init) ?? display) : display
        let words = source.split { !$0.isLetter && !$0.isNumber }.map(String.init)
        return words.prefix(2).compactMap(\.first).map { String($0).uppercased() }.joined()
    }
}

@MainActor
final class ContactPane: NSView {
    var onRefreshActivity: (() -> Void)?
    private let openActivity = OpenActivitySummary(frame: .zero)
    var onClose: (() -> Void)?
    var onCompose: ((String) -> Void)?
    var onSearch: ((String) -> Void)?
    var onOpenThread: ((String) -> Void)?

    private let avatar = FallbackAvatarView()
    private let avatarImage = NSImageView()
    private let name = NSTextField(labelWithString: "")
    private let emailButton = NSButton(title: "", target: nil, action: nil)
    private let recentStack = NSStackView()
    private let searchButton = NSButton(title: "View all mail", target: nil, action: nil)
    private let composeButton = NSButton(title: "New message", target: nil, action: nil)
    private let portraitButton = NSButton(title: "Picture", target: nil, action: nil)
    private var email = ""
    private var displayedSender = ""
    private var displayedRecent: [MailThread] = []
    private var recentIDs: [String] = []
    private var photoGeneration = 0
    private var googleAccountID: String?
    private var googleCore: CoreBridge?
    private var googlePhotoEnabled: [String: Bool] = [:]
    private var googlePhotoCache: [String: (image: NSImage?, expires: Date)] = [:]

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        wantsLayer = true
        translatesAutoresizingMaskIntoConstraints = false
        widthAnchor.constraint(equalToConstant: 270).isActive = true

        let heading = Self.label("CONTACT", size: 10, weight: .semibold, color: EmailTheme.muted)
        let close = NSButton(title: "", target: self, action: #selector(closeClicked))
        close.image = NSImage(systemSymbolName: "xmark", accessibilityDescription: "Close contact pane")
        close.imagePosition = .imageOnly
        close.isBordered = false
        close.contentTintColor = EmailTheme.muted
        close.toolTip = "Hide contact pane (0)"

        avatarImage.imageScaling = .scaleProportionallyUpOrDown
        avatarImage.wantsLayer = true
        avatarImage.layer?.cornerRadius = 23
        avatarImage.layer?.masksToBounds = true
        avatarImage.isHidden = true
        name.font = .systemFont(ofSize: 14, weight: .semibold)
        name.textColor = EmailTheme.ink
        name.maximumNumberOfLines = 2
        name.lineBreakMode = .byTruncatingTail
        emailButton.isBordered = false
        emailButton.alignment = .left
        emailButton.font = .systemFont(ofSize: 11.5)
        emailButton.contentTintColor = EmailTheme.muted
        emailButton.target = self
        emailButton.action = #selector(composeClicked)
        emailButton.toolTip = "Write to this address"

        composeButton.image = NSImage(systemSymbolName: "square.and.pencil", accessibilityDescription: nil)
        composeButton.imagePosition = .imageLeading
        composeButton.isBordered = false
        composeButton.font = .systemFont(ofSize: 11.5, weight: .medium)
        composeButton.target = self
        composeButton.action = #selector(composeClicked)
        composeButton.title = "Write"
        searchButton.image = NSImage(systemSymbolName: "magnifyingglass", accessibilityDescription: nil)
        searchButton.imagePosition = .imageLeading
        searchButton.isBordered = false
        searchButton.font = .systemFont(ofSize: 11.5, weight: .medium)
        searchButton.title = "Find mail"
        searchButton.target = self
        searchButton.action = #selector(searchClicked)
        portraitButton.image = NSImage(systemSymbolName: "person.crop.circle", accessibilityDescription: nil)
        portraitButton.imagePosition = .imageOnly
        portraitButton.title = ""
        portraitButton.setAccessibilityLabel("Sender picture options")
        portraitButton.isBordered = false
        portraitButton.font = .systemFont(ofSize: 11.5, weight: .medium)
        portraitButton.target = self
        portraitButton.action = #selector(pictureClicked)
        portraitButton.toolTip = "Set or remove a local picture for this sender"
        let recentHeading = Self.label("RECENT MAIL", size: 10, weight: .semibold, color: EmailTheme.muted)
        recentStack.orientation = .vertical
        recentStack.alignment = .leading
        recentStack.spacing = 0
        for view in [heading, close, avatar, avatarImage, name, emailButton, composeButton, searchButton, portraitButton, recentHeading, recentStack] {
            view.translatesAutoresizingMaskIntoConstraints = false
            addSubview(view)
        }
        NSLayoutConstraint.activate([
            heading.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 22), heading.topAnchor.constraint(equalTo: topAnchor, constant: 28),
            close.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -16), close.centerYAnchor.constraint(equalTo: heading.centerYAnchor), close.widthAnchor.constraint(equalToConstant: 24), close.heightAnchor.constraint(equalToConstant: 24),
            avatar.leadingAnchor.constraint(equalTo: heading.leadingAnchor), avatar.topAnchor.constraint(equalTo: heading.bottomAnchor, constant: 27), avatar.widthAnchor.constraint(equalToConstant: 46), avatar.heightAnchor.constraint(equalToConstant: 46),
            avatarImage.leadingAnchor.constraint(equalTo: avatar.leadingAnchor), avatarImage.trailingAnchor.constraint(equalTo: avatar.trailingAnchor), avatarImage.topAnchor.constraint(equalTo: avatar.topAnchor), avatarImage.bottomAnchor.constraint(equalTo: avatar.bottomAnchor),
            name.leadingAnchor.constraint(equalTo: avatar.trailingAnchor, constant: 12), name.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -18), name.topAnchor.constraint(equalTo: avatar.topAnchor, constant: 1),
            emailButton.leadingAnchor.constraint(equalTo: name.leadingAnchor), emailButton.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -18), emailButton.topAnchor.constraint(equalTo: name.bottomAnchor, constant: 3), emailButton.heightAnchor.constraint(equalToConstant: 20),
            composeButton.leadingAnchor.constraint(equalTo: heading.leadingAnchor), composeButton.topAnchor.constraint(equalTo: avatar.bottomAnchor, constant: 18), composeButton.heightAnchor.constraint(equalToConstant: 30),
            searchButton.leadingAnchor.constraint(equalTo: composeButton.trailingAnchor, constant: 17), searchButton.centerYAnchor.constraint(equalTo: composeButton.centerYAnchor), searchButton.heightAnchor.constraint(equalToConstant: 30),
            portraitButton.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -18), portraitButton.centerYAnchor.constraint(equalTo: composeButton.centerYAnchor), portraitButton.heightAnchor.constraint(equalToConstant: 30), portraitButton.widthAnchor.constraint(equalToConstant: 26),
            recentHeading.leadingAnchor.constraint(equalTo: heading.leadingAnchor), recentHeading.topAnchor.constraint(equalTo: composeButton.bottomAnchor, constant: 34),
            recentStack.leadingAnchor.constraint(equalTo: heading.leadingAnchor), recentStack.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -20), recentStack.topAnchor.constraint(equalTo: recentHeading.bottomAnchor, constant: 10)
        ])
        let activityScroll = NSScrollView()
        activityScroll.drawsBackground = false; activityScroll.hasVerticalScroller = true; activityScroll.autohidesScrollers = true
        activityScroll.translatesAutoresizingMaskIntoConstraints = false
        let document = ContactActivityDocument(); document.translatesAutoresizingMaskIntoConstraints = false
        openActivity.translatesAutoresizingMaskIntoConstraints = false; document.addSubview(openActivity)
        activityScroll.documentView = document; addSubview(activityScroll)
        NSLayoutConstraint.activate([
            activityScroll.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 22), activityScroll.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -20),
            activityScroll.topAnchor.constraint(equalTo: recentStack.bottomAnchor, constant: 24), activityScroll.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -16),
            document.widthAnchor.constraint(equalTo: activityScroll.contentView.widthAnchor),
            openActivity.leadingAnchor.constraint(equalTo: document.leadingAnchor), openActivity.trailingAnchor.constraint(equalTo: document.trailingAnchor),
            openActivity.topAnchor.constraint(equalTo: document.topAnchor), openActivity.bottomAnchor.constraint(equalTo: document.bottomAnchor)
        ])
        openActivity.onRefresh = { [weak self] in self?.onRefreshActivity?() }
        applyTheme()
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is unavailable") }

    func showOpenActivity(_ item: [String: Any]?) { openActivity.show(item) }
    func focusOpenActivity() { openActivity.focusRefresh() }

    func applyTheme() {
        layer?.backgroundColor = EmailTheme.cgColor(EmailTheme.reader, in: effectiveAppearance)
        layer?.borderColor = EmailTheme.cgColor(EmailTheme.line, in: effectiveAppearance)
        layer?.borderWidth = 0.5
        avatar.needsDisplay = true
        composeButton.contentTintColor = EmailTheme.accent
        searchButton.contentTintColor = EmailTheme.muted
        portraitButton.contentTintColor = EmailTheme.muted
    }

    func show(sender: String, recent: [MailThread], googleAccountID: String? = nil, core: CoreBridge? = nil) {
        displayedSender = sender
        displayedRecent = recent
        self.googleAccountID = googleAccountID
        googleCore = core
        let address = Self.address(from: sender)
        email = address
        let display = Self.displayName(from: sender)
        name.stringValue = display
        name.toolTip = display
        emailButton.title = address
        emailButton.toolTip = "Write to \(address)"
        avatar.monogram = FallbackAvatarView.initials(for: display, address: address)
        avatar.identity = address.split(separator: "@").last.map(String.init) ?? address
        avatarImage.image = nil
        avatarImage.isHidden = true
        photoGeneration += 1
        let generation = photoGeneration
        if let custom = CustomPortraitStore.shared.image(for: address) {
            avatarImage.image = custom
            avatarImage.isHidden = false
        } else {
            BrandLogoStore.shared.image(for: address, displayName: display) { [weak self] image in
                guard let self, self.photoGeneration == generation, self.avatarImage.image == nil else { return }
                self.avatarImage.image = image
                self.avatarImage.isHidden = image == nil
            }
            loadContactPhoto(for: address)
            if let googleAccountID, let core, core.buildFeatures().googleContactPhotos {
                loadGooglePhoto(for: address, accountID: googleAccountID, core: core, generation: generation)
            }
        }
        let available = recent.filter { $0.id != nil }
        recentIDs = available.compactMap(\.id)
        recentStack.arrangedSubviews.forEach { $0.removeFromSuperview() }
        if recentIDs.isEmpty {
            recentStack.addArrangedSubview(Self.label("No earlier mail from this sender", size: 11, weight: .regular, color: EmailTheme.muted))
        } else {
            for (index, thread) in available.enumerated() {
                let button = NSButton(title: thread.subject.isEmpty ? "(No subject)" : thread.subject, target: self, action: #selector(recentClicked(_:)))
                button.tag = index
                button.isBordered = false
                button.alignment = .left
                button.font = .systemFont(ofSize: 11.5)
                button.contentTintColor = EmailTheme.ink
                button.toolTip = thread.subject
                button.lineBreakMode = .byTruncatingTail
                button.widthAnchor.constraint(equalToConstant: 210).isActive = true
                button.heightAnchor.constraint(equalToConstant: 36).isActive = true
                recentStack.addArrangedSubview(button)
            }
        }
    }

    static func address(from sender: String) -> String {
        if let open = sender.lastIndex(of: "<"), let close = sender[open...].firstIndex(of: ">") {
            return String(sender[sender.index(after: open)..<close]).trimmingCharacters(in: .whitespacesAndNewlines)
        }
        return sender.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private static func displayName(from sender: String) -> String {
        guard let open = sender.firstIndex(of: "<") else { return sender }
        let value = sender[..<open].trimmingCharacters(in: .whitespacesAndNewlines).trimmingCharacters(in: CharacterSet(charactersIn: "\""))
        return value.isEmpty ? address(from: sender) : value
    }

    private static func label(_ text: String, size: CGFloat, weight: NSFont.Weight, color: NSColor) -> NSTextField {
        let view = NSTextField(labelWithString: text)
        view.font = .systemFont(ofSize: size, weight: weight)
        view.textColor = color
        return view
    }

    private func loadContactPhoto(for address: String) {
        let generation = photoGeneration
        let status = CNContactStore.authorizationStatus(for: .contacts)
        guard status == .authorized else { return }
        DispatchQueue.global(qos: .utility).async { [weak self] in
            let store = CNContactStore()
            let keys: [CNKeyDescriptor] = [CNContactThumbnailImageDataKey as CNKeyDescriptor]
            let contact = try? store.unifiedContacts(matching: CNContact.predicateForContacts(matchingEmailAddress: address), keysToFetch: keys).first
            let image = contact?.thumbnailImageData.flatMap(NSImage.init(data:))
            DispatchQueue.main.async { [weak self] in
                guard let self, self.photoGeneration == generation else { return }
                if let image {
                    self.avatarImage.image = image
                    self.avatarImage.isHidden = false
                }
            }
        }
    }

    private func loadGooglePhoto(for address: String, accountID: String, core: CoreBridge, generation: Int) {
        let key = "\(accountID):\(address.lowercased())"
        if let cached = googlePhotoCache[key], cached.expires > Date() {
            if let image = cached.image {
                avatarImage.image = image
                avatarImage.isHidden = false
            }
            return
        }
        Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                let enabled: Bool
                if let known = googlePhotoEnabled[accountID] {
                    enabled = known
                } else {
                    let state = try await core.callAsync(["op": "google_photo_state", "account_id": accountID]) as? [String: Any]
                    enabled = state?["enabled"] as? Bool ?? false
                    googlePhotoEnabled[accountID] = enabled
                }
                guard enabled else { return }
                let result = try await core.callAsync(["op": "google_contact_photo", "account_id": accountID, "address": address]) as? [String: Any]
                let image = (result?["data_base64"] as? String)
                    .flatMap { Data(base64Encoded: $0) }
                    .flatMap { NSImage(data: $0) }
                googlePhotoCache[key] = (image, Date().addingTimeInterval(image == nil ? 1800 : 86400))
                guard self.photoGeneration == generation, let image,
                      CustomPortraitStore.shared.image(for: address) == nil else { return }
                avatarImage.image = image
                avatarImage.isHidden = false
            } catch {
                // Mail and locally cached pictures remain usable if People API is unavailable.
            }
        }
    }

    @objc private func closeClicked() { onClose?() }
    @objc private func composeClicked() { onCompose?(email) }
    @objc private func searchClicked() { onSearch?(email) }
    @objc private func pictureClicked() {
        pictureMenu().popUp(positioning: nil, at: NSPoint(x: 0, y: portraitButton.bounds.maxY), in: portraitButton)
    }

    func pictureMenu() -> NSMenu {
        let menu = NSMenu(title: "Sender picture")
        // Distribution builds do not request Contacts access, which would need its own Google review.
        if let googleAccountID, let googleCore, googleCore.buildFeatures().googleContactPhotos,
           googlePhotoEnabled[googleAccountID] != true {
            let enable = NSMenuItem(title: "Use Google contact photos…", action: #selector(enableGooglePhotos), keyEquivalent: "")
            enable.target = self
            menu.addItem(enable)
            menu.addItem(.separator())
        }
        let domain = email.split(separator: "@").last.map(String.init) ?? email
        let senderItem = NSMenuItem(title: "Set for this address…", action: #selector(setPicture(_:)), keyEquivalent: "")
        senderItem.tag = 0
        senderItem.target = self
        menu.addItem(senderItem)
        let domainItem = NSMenuItem(title: "Set for @\(domain)…", action: #selector(setPicture(_:)), keyEquivalent: "")
        domainItem.tag = 1
        domainItem.target = self
        menu.addItem(domainItem)
        if CustomPortraitStore.shared.hasImage(for: email, scope: .sender) {
            let remove = NSMenuItem(title: "Remove address picture", action: #selector(removePicture(_:)), keyEquivalent: "")
            remove.tag = 0
            remove.target = self
            menu.addItem(remove)
        }
        if CustomPortraitStore.shared.hasImage(for: email, scope: .domain) {
            let remove = NSMenuItem(title: "Remove @\(domain) picture", action: #selector(removePicture(_:)), keyEquivalent: "")
            remove.tag = 1
            remove.target = self
            menu.addItem(remove)
        }
        return menu
    }

    @objc private func enableGooglePhotos() {
        guard let accountID = googleAccountID, let core = googleCore else { return }
        let alert = NSAlert()
        alert.messageText = "Use Google contact photos?"
        alert.informativeText = "Google will ask for read-only access to Contacts and Other contacts in this Gmail account. Email uses that access only to find sender pictures. Some Gmail pictures may still be unavailable."
        alert.addButton(withTitle: "Continue in browser")
        alert.addButton(withTitle: "Cancel")
        guard alert.runModal() == .alertFirstButtonReturn else { return }
        Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                _ = try await core.callAsync(["op": "enable_google_photos", "account_id": accountID])
                googlePhotoEnabled[accountID] = true
                googlePhotoCache = googlePhotoCache.filter { !$0.key.hasPrefix("\(accountID):") }
                if googleAccountID == accountID {
                    show(sender: displayedSender, recent: displayedRecent, googleAccountID: accountID, core: core)
                }
            } catch {
                let problem = NSAlert()
                problem.messageText = "Google photos unavailable"
                problem.informativeText = error.localizedDescription
                problem.runModal()
            }
        }
    }

    @objc private func setPicture(_ item: NSMenuItem) {
        let scope: CustomPortraitStore.Scope = item.tag == 0 ? .sender : .domain
        let selectedAddress = email
        let panel = NSOpenPanel()
        panel.allowedContentTypes = [.png, .jpeg, .heic]
        panel.allowsMultipleSelection = false
        panel.canChooseDirectories = false
        panel.prompt = "Use Picture"
        let chosen: (NSApplication.ModalResponse) -> Void = { [weak self] response in
            guard response == .OK, let self, let source = panel.url else { return }
            do {
                try CustomPortraitStore.shared.save(source: source, for: selectedAddress, scope: scope)
                if self.email == selectedAddress { self.show(sender: self.displayedSender, recent: self.displayedRecent, googleAccountID: self.googleAccountID, core: self.googleCore) }
            } catch {
                let alert = NSAlert()
                alert.messageText = "Could not use that picture"
                alert.informativeText = error.localizedDescription
                alert.runModal()
            }
        }
        if let window { panel.beginSheetModal(for: window, completionHandler: chosen) }
        else { panel.begin(completionHandler: chosen) }
    }

    @objc private func removePicture(_ item: NSMenuItem) {
        let scope: CustomPortraitStore.Scope = item.tag == 0 ? .sender : .domain
        CustomPortraitStore.shared.remove(for: email, scope: scope)
        show(sender: displayedSender, recent: displayedRecent, googleAccountID: googleAccountID, core: googleCore)
    }
    @objc private func recentClicked(_ sender: NSButton) {
        guard recentIDs.indices.contains(sender.tag) else { return }
        onOpenThread?(recentIDs[sender.tag])
    }
}

private final class ContactActivityDocument: NSView { override var isFlipped: Bool { true } }
