import AppKit

/// Account choices offered in setup. Keep in sync with the Linux account dialog
/// (`apps/linux/src/account_setup.rs`).
enum AccountSetupChoice: CaseIterable, Sendable {
    case gmail, gmailAppPassword, icloud, outlook, other

    /// Without Google sign-in the app-password path is the only Gmail entry,
    /// so it is simply "Gmail".
    func title(_ features: BuildFeatures) -> String {
        switch self {
        case .gmail: "Gmail"
        case .gmailAppPassword: features.googleOAuthPublic ? "Gmail (app password)" : "Gmail"
        case .icloud: "iCloud"
        case .outlook: "Outlook"
        case .other: "Other"
        }
    }

    /// Choices that connect over IMAP/SMTP with a password made for this app.
    var usesPassword: Bool { self == .gmailAppPassword || self == .icloud || self == .other }

    /// Display order. Official releases do not offer Google sign-in until
    /// Google's review passes (`googleOAuthPublic`); Gmail then goes straight to
    /// the app-password path. Outlook/Hotmail stays out of distribution builds
    /// until it has been tested with real accounts.
    static func available(_ features: BuildFeatures) -> [AccountSetupChoice] {
        let gmail: [AccountSetupChoice] = features.googleOAuthPublic ? [.gmail, .gmailAppPassword] : [.gmailAppPassword]
        return gmail + [.icloud] + (features.microsoftAccounts ? [.outlook] : []) + [.other]
    }

    /// The choice selected when the window opens.
    static func initial(_ features: BuildFeatures, googleSignInConfigured: Bool) -> AccountSetupChoice {
        features.googleOAuthPublic && googleSignInConfigured ? .gmail : .gmailAppPassword
    }
}

/// Gmail through the generic IMAP path. The Rust preset (`ImapConfig::gmail`)
/// owns the servers; the request names none.
enum GmailAppPasswordPreset {
    static let twoStepURL = "https://myaccount.google.com/signinoptions/twosv"
    static let appPasswordsURL = "https://myaccount.google.com/apppasswords"
    static let helpURL = "https://support.google.com/accounts/answer/185833"
    static let steps: [(text: String, link: String?, url: String?)] = [
        ("Turn on 2-Step Verification for your Google Account.", "Open 2-Step Verification ↗", twoStepURL),
        ("Create an app password and name it “Email”.", "Open App passwords ↗", appPasswordsURL),
        ("Paste the 16-letter password below. Spaces don't matter.", nil, nil),
    ]
    /// Shown when this build offers no Google sign-in.
    static let signInComing = "One-click Google sign-in is coming once Google finishes reviewing the app."
    static let about = "An app password is a 16-letter code that lets Email sign in to Gmail's mail servers without your Google password. Email keeps it in your Mac's Keychain and sends it only to Google. Remove it on the App passwords page to revoke access; changing your Google password revokes it too."
    static func unavailable(_ features: BuildFeatures) -> String {
        "No App passwords option? Google doesn't offer them for accounts that use only security keys or Advanced Protection, or for some work and school accounts. "
            + (features.googleOAuthPublic ? "Use Gmail sign-in instead, or ask your administrator." : "Ask your administrator, or wait for one-click Google sign-in.")
    }
}

@MainActor
final class AccountSetupWindow: NSWindow {
    private let core: CoreBridge
    private let onConnected: () -> Void
    private let features: BuildFeatures
    private let choices: [AccountSetupChoice]
    private var selected: AccountSetupChoice
    private let content = NSStackView()
    private let intro = NSTextField(wrappingLabelWithString: "")
    private let displayName = NSTextField()
    private let email = NSTextField()
    private let username = NSTextField()
    private let smtpUsername = NSTextField()
    private let password = NSSecureTextField()
    private let imapHost = NSTextField()
    private let imapPort = NSTextField()
    private let smtpHost = NSTextField()
    private let smtpPort = NSTextField()
    private let imapSecurity = NSPopUpButton()
    private let smtpSecurity = NSPopUpButton()
    private let gmailSteps = NSStackView()
    private let gmailAbout = NSStackView()
    private let status = NSTextField(wrappingLabelWithString: "")
    private let switchToAppPassword = NSButton(title: "Use Gmail (app password) instead →", target: nil, action: nil)
    private let connectButton = NSButton(title: "Connect account", target: nil, action: nil)
    private let helpButton = NSButton(title: "How to get an app password", target: nil, action: nil)
    private var providerButtons: [(choice: AccountSetupChoice, button: NSButton)] = []
    private let imapFields = NSStackView()
    private let oauthFields = NSStackView()
    private var displayNameRow: NSView?
    private var emailRow: NSView?
    private var advancedRows: [NSView] = []
    private var signInFailed = false
    /// Choices with a request still running. Another choice can be used
    /// meanwhile; the same one cannot be submitted twice.
    private var inFlight: Set<AccountSetupChoice> = []
    /// How a request ended while another choice was on screen. Shown when
    /// that choice is selected again (same as Linux).
    private var outcomes: [AccountSetupChoice: (message: String, problem: Bool)] = [:]
    private var appearanceObservation: NSKeyValueObservation?

    init(core: CoreBridge, onConnected: @escaping () -> Void) {
        self.core = core
        self.onConnected = onConnected
        features = core.buildFeatures()
        choices = AccountSetupChoice.available(features)
        selected = .gmailAppPassword
        super.init(contentRect: NSRect(x: 0, y: 0, width: 560, height: 400), styleMask: [.titled, .closable], backing: .buffered, defer: false)
        title = "Connect an account"
        backgroundColor = EmailTheme.reader
        isReleasedWhenClosed = false
        build()
        select(AccountSetupChoice.initial(features, googleSignInConfigured: !oauthClientID(for: .gmail).isEmpty))
        center()
        // Chip colors are layer colors; repaint them when light/dark mode changes.
        appearanceObservation = observe(\.effectiveAppearance) { window, _ in
            MainActor.assumeIsolated { window.paintChoices() }
        }
    }

    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        if event.modifierFlags.intersection([.command, .control, .option]) == .command,
           let digit = event.charactersIgnoringModifiers?.first?.wholeNumberValue,
           choices.indices.contains(digit - 1) {
            select(choices[digit - 1])
            return true
        }
        return super.performKeyEquivalent(with: event)
    }

    private static func secondary(_ text: String, size: CGFloat = 11) -> NSTextField {
        let label = NSTextField(wrappingLabelWithString: text)
        label.font = .systemFont(ofSize: size)
        label.textColor = .secondaryLabelColor
        label.preferredMaxLayoutWidth = 500
        return label
    }

    private func link(_ title: String, url: String) -> NSButton {
        let button = NSButton(title: title, target: self, action: #selector(openLink(_:)))
        button.isBordered = false
        button.contentTintColor = EmailTheme.accent
        button.font = .systemFont(ofSize: 12, weight: .medium)
        button.toolTip = url
        button.identifier = NSUserInterfaceItemIdentifier(url)
        return button
    }

    private func build() {
        content.orientation = .vertical
        content.alignment = .leading
        content.spacing = 12
        content.translatesAutoresizingMaskIntoConstraints = false
        contentView = NSView()
        contentView?.addSubview(content)
        NSLayoutConstraint.activate([
            content.leadingAnchor.constraint(equalTo: contentView!.leadingAnchor, constant: 30),
            content.trailingAnchor.constraint(equalTo: contentView!.trailingAnchor, constant: -30),
            content.topAnchor.constraint(equalTo: contentView!.topAnchor, constant: 28)
        ])
        let headline = NSTextField(labelWithString: "Add an email account")
        headline.font = .systemFont(ofSize: 20, weight: .semibold)
        content.addArrangedSubview(headline)
        intro.stringValue = switch (features.googleOAuthPublic, features.microsoftAccounts) {
        case (true, true): "Gmail and Outlook sign in through your browser. Gmail (app password) and iCloud use a password created for this app. Never enter your main account password here."
        case (true, false): "Gmail signs in through your browser. Gmail (app password) and iCloud use a password created for this app. Never enter your main account password here."
        case (false, true): "Outlook signs in through your browser. Gmail and iCloud use a password created for this app. Never enter your main account password here."
        case (false, false): "Gmail and iCloud connect with a password created for this app. Never enter your main account password here."
        }
        intro.textColor = .secondaryLabelColor
        intro.font = .systemFont(ofSize: 12)
        intro.preferredMaxLayoutWidth = 500
        content.addArrangedSubview(intro)
        let row = NSStackView()
        row.orientation = .horizontal
        row.spacing = 8
        for (index, choice) in choices.enumerated() {
            let button = NSButton(title: choice.title(features), target: self, action: #selector(providerButtonClicked(_:)))
            button.tag = index
            button.keyEquivalent = String(index + 1)
            button.keyEquivalentModifierMask = [.command]
            button.toolTip = "Choose \(choice.title(features)) (⌘\(index + 1))"
            button.isBordered = false
            button.wantsLayer = true
            button.layer?.cornerRadius = 8
            button.widthAnchor.constraint(greaterThanOrEqualToConstant: 76).isActive = true
            button.heightAnchor.constraint(equalToConstant: 34).isActive = true
            providerButtons.append((choice, button))
            row.addArrangedSubview(button)
        }
        content.addArrangedSubview(row)

        // Gmail (app password): three numbered steps with direct links.
        gmailSteps.orientation = .vertical
        gmailSteps.alignment = .leading
        gmailSteps.spacing = 8
        if !features.googleOAuthPublic {
            gmailSteps.addArrangedSubview(Self.secondary(GmailAppPasswordPreset.signInComing, size: 12))
        }
        for (index, step) in GmailAppPasswordPreset.steps.enumerated() {
            let line = NSStackView()
            line.orientation = .horizontal
            line.alignment = .firstBaseline
            line.spacing = 8
            let number = NSTextField(labelWithString: "\(index + 1)")
            number.font = .monospacedDigitSystemFont(ofSize: 12, weight: .semibold)
            number.textColor = EmailTheme.accent
            number.widthAnchor.constraint(equalToConstant: 14).isActive = true
            let text = NSTextField(wrappingLabelWithString: step.text)
            text.font = .systemFont(ofSize: 12)
            text.preferredMaxLayoutWidth = step.link == nil ? 470 : 300
            line.addArrangedSubview(number)
            line.addArrangedSubview(text)
            if let title = step.link, let url = step.url { line.addArrangedSubview(link(title, url: url)) }
            gmailSteps.addArrangedSubview(line)
        }
        content.addArrangedSubview(gmailSteps)

        displayName.placeholderString = "Optional"
        email.placeholderString = "you@example.com"
        username.placeholderString = "Usually your email address"
        smtpUsername.placeholderString = "Leave blank to use the mail username"
        password.placeholderString = "Provider-issued app password"
        imapHost.placeholderString = "imap.example.com"
        imapPort.placeholderString = "993"
        smtpHost.placeholderString = "smtp.example.com"
        smtpPort.placeholderString = "587"
        imapSecurity.addItems(withTitles: ["TLS", "STARTTLS"])
        smtpSecurity.addItems(withTitles: ["STARTTLS", "TLS"])
        let nameRow = self.row("Display name", displayName)
        let addressRow = self.row("Email address", email)
        displayNameRow = nameRow
        emailRow = addressRow
        content.addArrangedSubview(nameRow)
        content.addArrangedSubview(addressRow)

        imapFields.orientation = .vertical
        imapFields.alignment = .leading
        imapFields.spacing = 10
        for (label, field) in [("Mail username", username), ("SMTP username", smtpUsername), ("App password", password), ("IMAP server", imapHost), ("IMAP port", imapPort), ("IMAP security", imapSecurity), ("SMTP server", smtpHost), ("SMTP port", smtpPort), ("SMTP security", smtpSecurity)] {
            let fieldRow = self.row(label, field)
            imapFields.addArrangedSubview(fieldRow)
            if label != "App password" { advancedRows.append(fieldRow) }
        }
        content.addArrangedSubview(imapFields)
        oauthFields.orientation = .vertical
        oauthFields.alignment = .leading
        oauthFields.spacing = 10
        oauthFields.addArrangedSubview(Self.secondary("Sign-in opens on your provider's website. Email receives a revocable access token, never your account password."))
        content.addArrangedSubview(oauthFields)

        gmailAbout.orientation = .vertical
        gmailAbout.alignment = .leading
        gmailAbout.spacing = 6
        gmailAbout.addArrangedSubview(Self.secondary(GmailAppPasswordPreset.about))
        gmailAbout.addArrangedSubview(Self.secondary(GmailAppPasswordPreset.unavailable(features)))
        content.addArrangedSubview(gmailAbout)

        helpButton.isBordered = false
        helpButton.contentTintColor = EmailTheme.accent
        helpButton.target = self
        helpButton.action = #selector(openHelp)
        content.addArrangedSubview(helpButton)

        status.font = .systemFont(ofSize: 12)
        status.textColor = .secondaryLabelColor
        status.preferredMaxLayoutWidth = 500
        content.addArrangedSubview(status)
        switchToAppPassword.isBordered = false
        switchToAppPassword.contentTintColor = EmailTheme.accent
        switchToAppPassword.font = .systemFont(ofSize: 12, weight: .semibold)
        switchToAppPassword.target = self
        switchToAppPassword.action = #selector(switchToAppPasswordClicked)
        content.addArrangedSubview(switchToAppPassword)
        let buttons = NSStackView()
        buttons.orientation = .horizontal
        buttons.spacing = 10
        let cancel = NSButton(title: "Cancel", target: self, action: #selector(cancelClicked))
        cancel.keyEquivalent = "\u{1b}"
        connectButton.target = self
        connectButton.action = #selector(connectClicked)
        connectButton.keyEquivalent = "\r"
        buttons.addArrangedSubview(cancel)
        buttons.addArrangedSubview(connectButton)
        content.addArrangedSubview(buttons)
    }

    private func row(_ title: String, _ field: NSView) -> NSView {
        let stack = NSStackView()
        stack.orientation = .horizontal
        stack.alignment = .centerY
        stack.spacing = 12
        let label = NSTextField(labelWithString: title)
        label.font = .systemFont(ofSize: 12, weight: .medium)
        label.widthAnchor.constraint(equalToConstant: 110).isActive = true
        field.widthAnchor.constraint(equalToConstant: 345).isActive = true
        stack.addArrangedSubview(label)
        stack.addArrangedSubview(field)
        return stack
    }

    @objc private func providerButtonClicked(_ sender: NSButton) {
        guard choices.indices.contains(sender.tag) else { return }
        select(choices[sender.tag])
    }

    @objc private func switchToAppPasswordClicked() {
        select(.gmailAppPassword)
        makeFirstResponder(email.stringValue.isEmpty ? email : password)
    }

    private func select(_ choice: AccountSetupChoice) {
        selected = choices.contains(choice) ? choice : (choices.first ?? .other)
        signInFailed = false
        updateProvider()
        if let outcome = outcomes.removeValue(forKey: selected) {
            if outcome.problem {
                signInFailed = selected == .gmail
                showProblem(outcome.message)
            } else {
                status.stringValue = outcome.message
            }
        }
    }

    private func paintChoices() {
        for (option, button) in providerButtons {
            let isSelected = option == selected
            var unselected = NSColor.clear.cgColor
            effectiveAppearance.performAsCurrentDrawingAppearance {
                unselected = NSColor.quaternaryLabelColor.withAlphaComponent(0.12).cgColor
            }
            button.layer?.backgroundColor = isSelected ? EmailTheme.cgColor(EmailTheme.accentSoft, in: effectiveAppearance) : unselected
            // A ring makes the current choice obvious in both appearances.
            button.layer?.borderWidth = isSelected ? 1.5 : 0
            button.layer?.borderColor = EmailTheme.cgColor(EmailTheme.accent, in: effectiveAppearance)
            button.attributedTitle = NSAttributedString(string: "  \(option.title(features))  ", attributes: [
                .foregroundColor: isSelected ? EmailTheme.accent : NSColor.labelColor,
                .font: NSFont.systemFont(ofSize: 12, weight: isSelected ? .semibold : .medium)
            ])
        }
    }

    private func updateProvider() {
        let choice = selected
        password.stringValue = ""
        paintChoices()
        imapFields.isHidden = !choice.usesPassword
        advancedRows.forEach { $0.isHidden = choice != .other }
        oauthFields.isHidden = choice.usesPassword
        emailRow?.isHidden = !choice.usesPassword
        displayNameRow?.isHidden = choice != .other
        gmailSteps.isHidden = choice != .gmailAppPassword
        gmailAbout.isHidden = choice != .gmailAppPassword
        helpButton.isHidden = choice != .icloud && choice != .gmailAppPassword
        helpButton.title = choice == .gmailAppPassword ? "Learn more about app passwords ↗" : "Get an iCloud app password ↗"
        switch choice {
        case .icloud:
            imapHost.stringValue = "imap.mail.me.com"
            imapPort.stringValue = "993"
            smtpHost.stringValue = "smtp.mail.me.com"
            smtpPort.stringValue = "587"
            imapSecurity.selectItem(at: 0)
            smtpSecurity.selectItem(at: 0)
            email.placeholderString = "you@icloud.com"
            password.placeholderString = "iCloud app-specific password"
        case .gmailAppPassword:
            email.placeholderString = "you@gmail.com"
            password.placeholderString = "abcd efgh ijkl mnop"
        case .other:
            imapHost.stringValue = ""
            imapPort.stringValue = "993"
            smtpHost.stringValue = ""
            smtpPort.stringValue = "587"
            imapSecurity.selectItem(at: 0)
            smtpSecurity.selectItem(at: 0)
            email.placeholderString = "you@example.com"
            password.placeholderString = "Provider-issued app password"
        case .gmail, .outlook:
            break
        }
        connectButton.title = choice.usesPassword ? "Add account" : "Sign in with browser"
        let oauthReady = choice.usesPassword || !oauthClientID(for: choice).isEmpty
        connectButton.isEnabled = oauthReady && !inFlight.contains(choice)
        status.textColor = .secondaryLabelColor
        let idle: String = switch choice {
        case .icloud: "Use a password created for this app in your Apple Account. Never enter your Apple Account password."
        case .gmailAppPassword: "Email connects directly to imap.gmail.com and smtp.gmail.com. Your regular Google password won't work here."
        case .other: "Use a provider-issued app password and the server settings from your provider."
        case .gmail where !oauthReady: "Google sign-in isn't configured in this build. Use Gmail (app password) instead."
        case .gmail, .outlook: oauthReady ? "You'll sign in on your provider's website in your browser." : "Sign-in isn't configured in this build yet. The app developer must register Email with this provider."
        }
        status.stringValue = inFlight.contains(choice) ? "Still connecting…" : idle
        updateSwitch()
        resizeToFit(recenter: true)
        if choice.usesPassword { makeFirstResponder(email) }
    }

    /// Offer the app-password path wherever Google sign-in may not work for this person.
    private func updateSwitch(waiting: Bool = false) {
        let unconfigured = oauthClientID(for: .gmail).isEmpty
        switchToAppPassword.isHidden = !(selected == .gmail && (unconfigured || waiting || signInFailed))
    }

    /// Fit the window to the visible rows. Errors grow it downward in place.
    private func resizeToFit(recenter: Bool) {
        contentView?.layoutSubtreeIfNeeded()
        let height = ceil(content.fittingSize.height) + 56
        let top = frame.maxY
        setContentSize(NSSize(width: 560, height: max(300, height)))
        if recenter {
            center()
        } else {
            setFrameTopLeftPoint(NSPoint(x: frame.minX, y: top))
        }
    }

    private func oauthClientID(for choice: AccountSetupChoice) -> String {
        guard choice == .gmail || choice == .outlook else { return "" }
        let bundleKey = choice == .gmail ? "EmailGoogleOAuthClientID" : "EmailMicrosoftOAuthClientID"
        let environmentKey = choice == .gmail ? "EMAIL_GOOGLE_CLIENT_ID" : "EMAIL_MICROSOFT_CLIENT_ID"
        let configured = (Bundle.main.object(forInfoDictionaryKey: bundleKey) as? String)
            ?? ProcessInfo.processInfo.environment[environmentKey]
            ?? ""
        return configured.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private func oauthClientSecret(for choice: AccountSetupChoice) -> String? {
        guard choice == .gmail else { return nil }
        let configured = (Bundle.main.object(forInfoDictionaryKey: "EmailGoogleOAuthClientSecret") as? String)
            ?? ProcessInfo.processInfo.environment["EMAIL_GOOGLE_CLIENT_SECRET"]
        return configured?.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    @objc private func openLink(_ sender: NSButton) {
        if let raw = sender.identifier?.rawValue, let url = URL(string: raw) { NSWorkspace.shared.open(url) }
    }

    @objc private func openHelp() {
        let url = selected == .gmailAppPassword ? GmailAppPasswordPreset.helpURL : "https://support.apple.com/en-us/102654"
        if let url = URL(string: url) { NSWorkspace.shared.open(url) }
    }

    @objc private func cancelClicked() { close() }

    private func showProblem(_ message: String) {
        status.stringValue = message
        status.textColor = .systemRed
        updateSwitch()
        resizeToFit(recenter: false)
    }

    /// The `add_imap_account` request, identical to the Linux payload
    /// (`apps/linux/src/account_setup.rs`, `imap_payload`). `nil` with a message
    /// when the form is incomplete.
    func imapPayload(for choice: AccountSetupChoice) -> (payload: [String: Any]?, problem: String?) {
        let address = email.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        let hasPassword = !password.stringValue.trimmingCharacters(in: .whitespaces).isEmpty
        if choice == .gmailAppPassword {
            // The service owns Gmail's servers and usernames.
            guard address.contains("@"), hasPassword else {
                return (nil, "Enter your Gmail address and the 16-letter app password from step 2.")
            }
            return (["op": "add_imap_account", "provider": "imap", "preset": "gmail",
                     "email": address, "display_name": address, "username": address,
                     "smtp_username": address, "password": password.stringValue,
                     "google_sign_in": choices.contains(.gmail) && !oauthClientID(for: .gmail).isEmpty], nil)
        }
        let typedLogin = username.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        // iCloud's username field is hidden: never send one left over from Other.
        let login = choice == .icloud
            ? String(address.split(separator: "@").first ?? "")
            : (typedLogin.isEmpty ? address : typedLogin)
        // Spaces around server names and ports are ignored, as on Linux.
        let imapServer = imapHost.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        let smtpServer = smtpHost.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        guard address.contains("@"), !login.isEmpty, hasPassword,
              !imapServer.isEmpty, !smtpServer.isEmpty,
              let imap = Int(imapPort.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)), (1...65535).contains(imap),
              let smtp = Int(smtpPort.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)), (1...65535).contains(smtp) else {
            return (nil, choice == .icloud
                ? "Enter your iCloud address and app-specific password."
                : "Enter a valid email, provider-issued app password, server names, and ports.")
        }
        var payload: [String: Any] = [
            "op": "add_imap_account", "provider": choice == .icloud ? "icloud" : "imap",
            "email": address,
            "display_name": displayName.stringValue.isEmpty ? address : displayName.stringValue,
            "username": login,
            "password": password.stringValue,
            "imap_host": imapServer, "imap_port": imap,
            "imap_security": imapSecurity.titleOfSelectedItem == "TLS" ? "implicit_tls" : "start_tls",
            "smtp_host": smtpServer, "smtp_port": smtp,
            "smtp_security": smtpSecurity.titleOfSelectedItem == "TLS" ? "implicit_tls" : "start_tls"
        ]
        let smtpLogin = smtpUsername.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        if !smtpLogin.isEmpty, choice == .other { payload["smtp_username"] = smtpLogin }
        else if choice != .icloud { payload["smtp_username"] = login }
        return (payload, nil)
    }

    @objc private func connectClicked() {
        let choice = selected
        guard !inFlight.contains(choice) else { return }
        let payload: [String: Any]
        if choice.usesPassword {
            let request = imapPayload(for: choice)
            guard let imapPayload = request.payload else {
                showProblem(request.problem ?? "")
                return
            }
            payload = imapPayload
        } else {
            let clientID = oauthClientID(for: choice)
            guard !clientID.isEmpty else {
                showProblem("Sign-in isn't configured in this build yet.")
                return
            }
            var oauthPayload: [String: Any] = ["op": "start_oauth", "provider": choice == .gmail ? "gmail" : "microsoft", "client_id": clientID]
            if let secret = oauthClientSecret(for: choice), !secret.isEmpty {
                oauthPayload["client_secret"] = secret
            }
            payload = oauthPayload
        }
        inFlight.insert(choice)
        connectButton.isEnabled = false
        status.textColor = .secondaryLabelColor
        status.stringValue = switch choice {
        case .gmailAppPassword: "Connecting to Gmail…"
        case .icloud, .other: "Checking your mail server…"
        case .gmail: "Waiting for browser sign-in… If Google shows “Access blocked”, close that tab and use an app password."
        case .outlook: "Waiting for browser sign-in…"
        }
        updateSwitch(waiting: choice == .gmail)
        resizeToFit(recenter: false)
        Task { @MainActor in
            defer { inFlight.remove(choice) }
            do {
                let account = try await core.callAsync(payload)
                if selected == choice {
                    password.stringValue = ""
                    status.stringValue = "Connected."
                    onConnected()
                    close()
                } else {
                    // Keep the window and whatever is typed for the other choice.
                    let address = (account as? [String: Any])?["email"] as? String
                    outcomes[choice] = (address.map { "Connected \($0)." } ?? "Connected.", false)
                    onConnected()
                }
            } catch {
                inFlight.remove(choice) // before deciding which button state to show
                // Another choice is on screen: keep the message for when it returns.
                guard selected == choice else {
                    outcomes[choice] = (error.localizedDescription, true)
                    connectButton.isEnabled = (selected.usesPassword || !oauthClientID(for: selected).isEmpty)
                        && !inFlight.contains(selected)
                    return
                }
                signInFailed = choice == .gmail
                showProblem(error.localizedDescription)
                connectButton.isEnabled = true
            }
        }
    }
}
