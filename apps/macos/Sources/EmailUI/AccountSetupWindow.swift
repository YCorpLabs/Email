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

    /// Display order. Official releases do not offer Google sign-in
    /// (`googleOAuthPublic` is off); Gmail then goes straight to the
    /// app-password path. Outlook/Hotmail stays out of distribution builds
    /// until it has been tested with real accounts.
    static func available(_ features: BuildFeatures) -> [AccountSetupChoice] {
        let gmail: [AccountSetupChoice] = features.googleOAuthPublic ? [.gmail, .gmailAppPassword] : [.gmailAppPassword]
        return gmail + [.icloud] + (features.microsoftAccounts ? [.outlook] : []) + [.other]
    }

    /// The Gmail choice for a Google address.
    static func initial(_ features: BuildFeatures, googleSignInConfigured: Bool) -> AccountSetupChoice {
        features.googleOAuthPublic && googleSignInConfigured ? .gmail : .gmailAppPassword
    }

    /// The choice for the engine's `setup_provider` answer. Everything that is
    /// not clearly Google or Apple goes to Other.
    static func forProvider(_ provider: String, features: BuildFeatures, googleSignInConfigured: Bool) -> AccountSetupChoice {
        switch provider {
        case "gmail": initial(features, googleSignInConfigured: googleSignInConfigured)
        case "icloud": .icloud
        case "outlook" where features.microsoftAccounts: .outlook
        default: .other
        }
    }
}

/// The steps, field and short explanation for a provider that needs a
/// password made for Email. Linux shows the same text (`account_setup.rs`).
struct AppPasswordGuide: Sendable {
    let intro: String
    let steps: [(text: String, url: String?)]
    let fieldLabel: String
    let placeholder: String
    let disclosure: String
    let about: [String]
}

enum AccountSetupCopy {
    static let title = "Add an account"
    static let chooseProvider = "Choose provider"
    static let continueTitle = "Continue"
    static let addAccount = "Add account"
    static let guideLink = "Need help? Step-by-step guide ↗"
    static let details = "Details"
    static let advanced = "Advanced"
    static let enterEmail = "Enter your email address."
    static let or = "or"

    static let gmail = AppPasswordGuide(
        intro: "Gmail needs an app password.",
        steps: [("Turn on 2-Step Verification ↗", "https://myaccount.google.com/signinoptions/twosv"),
                ("Create an app password ↗", "https://myaccount.google.com/apppasswords"),
                ("Paste it below", nil)],
        fieldLabel: "App password",
        placeholder: "16 letters, spaces are fine",
        disclosure: "What's an app password?",
        about: ["It's a 16-letter code just for Email, stored in your Keychain.",
                "You can revoke it in your Google Account. Changing your Google password revokes it too.",
                "No App passwords option? Google hides it for accounts that use only security keys, Advanced Protection, and some work or school accounts.",
                "Email connects to imap.gmail.com and smtp.gmail.com."])

    static let icloud = AppPasswordGuide(
        intro: "iCloud needs an app-specific password.",
        steps: [("Sign in to your Apple Account ↗", "https://account.apple.com/"),
                ("Create an app-specific password", nil),
                ("Paste it below", nil)],
        fieldLabel: "App password",
        placeholder: "abcd-efgh-ijkl-mnop",
        disclosure: "What's an app-specific password?",
        about: ["It's a password Apple makes for one app, here Email, stored in your Keychain.",
                "Create it under Sign-In and Security. Your Apple Account needs two-factor authentication.",
                "You can revoke it at account.apple.com. Changing your Apple Account password revokes it too.",
                "Email connects to imap.mail.me.com and smtp.mail.me.com."])

    /// Other: no steps, one line, servers found automatically.
    static let otherIntro = "Use your email password or an app password."
    static let otherFieldLabel = "Password"
    static let otherPlaceholder = ""

    static func oauthIntro(_ choice: AccountSetupChoice) -> String {
        choice == .outlook ? "You'll sign in to Microsoft in your browser." : "You'll sign in to Google in your browser."
    }
}

@MainActor
final class AccountSetupWindow: NSWindow, NSTextFieldDelegate {
    private let core: CoreBridge
    private let onConnected: () -> Void
    private let features: BuildFeatures
    let choices: [AccountSetupChoice]
    /// `nil` until the address is entered or a provider chosen: only the
    /// email field shows.
    private(set) var selected: AccountSetupChoice?
    /// The person picked the provider; typing another address keeps it.
    private var pickedByHand = false
    private let content = NSStackView()
    let email = NSTextField()
    let providerMenu = NSPopUpButton(frame: .zero, pullsDown: true)
    let intro = NSTextField(wrappingLabelWithString: "")
    let steps = NSStackView()
    let password = NSSecureTextField()
    private let passwordLabel = NSTextField(labelWithString: "")
    private(set) var passwordRow: NSView?
    /// Status and errors, under the password field.
    private var problemRow: NSView?
    let aboutToggle = NSButton(title: "", target: nil, action: nil)
    let about = NSStackView()
    let guideButton = NSButton(title: AccountSetupCopy.guideLink, target: nil, action: nil)
    let advancedToggle = NSButton(title: AccountSetupCopy.advanced, target: nil, action: nil)
    let advanced = NSStackView()
    let displayName = NSTextField()
    let username = NSTextField()
    let smtpUsername = NSTextField()
    let imapHost = NSTextField()
    let imapPort = NSTextField()
    let smtpHost = NSTextField()
    let smtpPort = NSTextField()
    let imapSecurity = NSPopUpButton()
    let smtpSecurity = NSPopUpButton()
    let status = NSTextField(wrappingLabelWithString: "")
    let fixButton = NSButton(title: "", target: nil, action: nil)
    let detailsToggle = NSButton(title: AccountSetupCopy.details, target: nil, action: nil)
    let details = NSTextField(wrappingLabelWithString: "")
    let switchToAppPassword = NSButton(title: "Use Gmail (app password) instead →", target: nil, action: nil)
    let connectButton = NSButton(title: AccountSetupCopy.continueTitle, target: nil, action: nil)
    /// "Continue with Google" and the "or" divider, on the first screen only.
    let googleButton = GoogleSignInButton()
    private var googleRows: [NSView] = []
    /// The Email row and provider menu, hidden while a browser sign-in waits.
    private var emailRows: [NSView] = []
    private var fixURL: URL?
    private var signInFailed = false
    /// The server rows for Other: shown when the guess failed or opened by hand.
    private var serversShown = false
    /// Choices with a request still running. Another choice can be used
    /// meanwhile; the same one cannot be submitted twice.
    private(set) var inFlight: Set<AccountSetupChoice> = []
    /// How a request ended while another choice was on screen. Shown when
    /// that choice is selected again (same as Linux).
    private var outcomes: [AccountSetupChoice: (message: String, problem: SetupProblem?)] = [:]

    private static let width: CGFloat = 520
    private static let fieldWidth: CGFloat = 340
    private static let labelWidth: CGFloat = 104

    init(core: CoreBridge, onConnected: @escaping () -> Void) {
        self.core = core
        self.onConnected = onConnected
        features = core.buildFeatures()
        choices = AccountSetupChoice.available(features)
        super.init(contentRect: NSRect(x: 0, y: 0, width: Self.width, height: 240), styleMask: [.titled, .closable, .fullSizeContentView], backing: .buffered, defer: false)
        // One title: the heading inside the window. The title bar keeps the
        // name for the Window menu and accessibility.
        title = AccountSetupCopy.title
        titleVisibility = .hidden
        titlebarAppearsTransparent = true
        backgroundColor = EmailTheme.reader
        isReleasedWhenClosed = false
        build()
        showChoice()
        center()
        initialFirstResponder = email
        makeFirstResponder(email)
    }

    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        if event.modifierFlags.intersection([.command, .control, .option]) == .command,
           let digit = event.charactersIgnoringModifiers?.first?.wholeNumberValue,
           choices.indices.contains(digit - 1) {
            choose(choices[digit - 1])
            return true
        }
        return super.performKeyEquivalent(with: event)
    }

    // MARK: Building

    private static func secondary(_ text: String, size: CGFloat = 11) -> NSTextField {
        let label = NSTextField(wrappingLabelWithString: text)
        label.font = .systemFont(ofSize: size)
        label.textColor = .secondaryLabelColor
        label.preferredMaxLayoutWidth = width - 60
        return label
    }

    /// A hairline, "or", a hairline: as wide as the fields.
    private static func orDivider() -> NSView {
        func hairline() -> NSBox {
            let line = NSBox()
            line.boxType = .custom
            line.borderWidth = 0
            line.fillColor = EmailTheme.line
            line.heightAnchor.constraint(equalToConstant: 1).isActive = true
            return line
        }
        let left = hairline(), right = hairline()
        let or = NSTextField(labelWithString: AccountSetupCopy.or)
        or.font = .systemFont(ofSize: 12)
        or.textColor = .secondaryLabelColor
        let divider = NSStackView(views: [left, or, right])
        divider.orientation = .horizontal
        divider.alignment = .centerY
        divider.spacing = 10
        left.widthAnchor.constraint(equalTo: right.widthAnchor).isActive = true
        divider.widthAnchor.constraint(equalToConstant: fieldWidth).isActive = true
        return divider
    }

    private func link(_ title: String, url: String) -> NSButton {
        let button = NSButton(title: title, target: self, action: #selector(openLink(_:)))
        button.isBordered = false
        button.contentTintColor = .linkColor
        button.font = .systemFont(ofSize: 13)
        button.toolTip = url
        button.identifier = NSUserInterfaceItemIdentifier(url)
        button.setAccessibilityRole(.link)
        return button
    }

    private func disclosure(_ button: NSButton, action: Selector) {
        button.target = self
        button.action = action
        button.isBordered = false
        button.font = .systemFont(ofSize: 12)
        button.contentTintColor = .secondaryLabelColor
        button.imagePosition = .imageLeading
        button.imageHugsTitle = true
        setDisclosure(button, open: false)
    }

    private func setDisclosure(_ button: NSButton, open: Bool) {
        let symbol = open ? "chevron.down" : "chevron.right"
        button.image = NSImage(systemSymbolName: symbol, accessibilityDescription: nil)?
            .withSymbolConfiguration(.init(pointSize: 9, weight: .semibold))
        button.setAccessibilityExpanded(open)
    }

    private func row(_ title: String, _ field: NSView, label: NSTextField? = nil) -> NSStackView {
        let stack = NSStackView()
        stack.orientation = .horizontal
        stack.alignment = .centerY
        stack.spacing = 12
        let label = label ?? NSTextField(labelWithString: title)
        label.stringValue = title
        label.font = .systemFont(ofSize: 13)
        label.alignment = .right
        label.widthAnchor.constraint(equalToConstant: Self.labelWidth).isActive = true
        field.widthAnchor.constraint(equalToConstant: Self.fieldWidth).isActive = true
        stack.addArrangedSubview(label)
        stack.addArrangedSubview(field)
        return stack
    }

    /// Indents a view to line up with the fields.
    private func indented(_ view: NSView) -> NSStackView {
        let stack = NSStackView()
        stack.orientation = .horizontal
        stack.alignment = .top
        stack.spacing = 0
        let spacer = NSView()
        spacer.widthAnchor.constraint(equalToConstant: Self.labelWidth + 12).isActive = true
        stack.addArrangedSubview(spacer)
        stack.addArrangedSubview(view)
        return stack
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
            content.topAnchor.constraint(equalTo: contentView!.topAnchor, constant: 40)
        ])
        let headline = NSTextField(labelWithString: AccountSetupCopy.title)
        headline.font = .systemFont(ofSize: 20, weight: .semibold)
        headline.setAccessibilityRole(.staticText)
        content.addArrangedSubview(headline)
        content.setCustomSpacing(18, after: headline)

        // Google sign-in first where the build offers it, then email for the rest.
        if googleSignInOffered {
            googleButton.target = self
            googleButton.action = #selector(continueWithGoogle)
            googleButton.widthAnchor.constraint(equalToConstant: Self.fieldWidth).isActive = true
            let google = indented(googleButton)
            content.addArrangedSubview(google)
            content.setCustomSpacing(14, after: google)
            let divider = indented(Self.orDivider())
            content.addArrangedSubview(divider)
            content.setCustomSpacing(14, after: divider)
            googleRows = [google, divider]
        }

        email.placeholderString = "you@example.com"
        email.delegate = self
        let emailRow = row("Email", email)
        content.addArrangedSubview(emailRow)
        content.setCustomSpacing(4, after: emailRow)

        // Small "Choose provider" control for custom domains on Google or Apple.
        providerMenu.isBordered = false
        providerMenu.font = .systemFont(ofSize: 11)
        providerMenu.contentTintColor = .secondaryLabelColor
        (providerMenu.cell as? NSPopUpButtonCell)?.arrowPosition = .arrowAtBottom
        providerMenu.toolTip = "Choose your provider by hand (⌘1–⌘\(choices.count))"
        providerMenu.addItem(withTitle: AccountSetupCopy.chooseProvider)
        for (index, choice) in choices.enumerated() {
            providerMenu.addItem(withTitle: choice.title(features))
            providerMenu.lastItem?.keyEquivalent = String(index + 1)
            providerMenu.lastItem?.keyEquivalentModifierMask = [.command]
            providerMenu.lastItem?.tag = index
        }
        providerMenu.target = self
        providerMenu.action = #selector(providerMenuChanged(_:))
        let providerRow = indented(providerMenu)
        content.addArrangedSubview(providerRow)
        content.setCustomSpacing(14, after: providerRow)
        emailRows = [emailRow, providerRow]

        intro.font = .systemFont(ofSize: 13, weight: .medium)
        intro.preferredMaxLayoutWidth = Self.width - 60
        content.addArrangedSubview(intro)
        content.setCustomSpacing(8, after: intro)
        steps.orientation = .vertical
        steps.alignment = .leading
        steps.spacing = 4
        content.addArrangedSubview(steps)

        password.delegate = self
        let passwordRow = row("", password, label: passwordLabel)
        self.passwordRow = passwordRow
        content.addArrangedSubview(passwordRow)

        // Status and errors sit right under the field they are about: one
        // line, the fix link, and the server's words behind Details.
        status.font = .systemFont(ofSize: 13)
        status.preferredMaxLayoutWidth = Self.fieldWidth
        status.isSelectable = true
        fixButton.target = self
        fixButton.action = #selector(openFix)
        fixButton.isBordered = false
        fixButton.font = .systemFont(ofSize: 13, weight: .medium)
        fixButton.contentTintColor = .linkColor
        fixButton.setAccessibilityRole(.link)
        disclosure(detailsToggle, action: #selector(toggleDetails))
        details.font = .monospacedSystemFont(ofSize: 11, weight: .regular)
        details.textColor = .secondaryLabelColor
        details.isSelectable = true
        details.preferredMaxLayoutWidth = Self.fieldWidth
        details.isHidden = true
        let problemBox = NSStackView(views: [status, fixButton, detailsToggle, details])
        problemBox.orientation = .vertical
        problemBox.alignment = .leading
        problemBox.spacing = 3
        problemBox.setCustomSpacing(6, after: fixButton)
        let problemRow = indented(problemBox)
        self.problemRow = problemRow
        content.addArrangedSubview(problemRow)
        content.setCustomSpacing(10, after: passwordRow)

        disclosure(aboutToggle, action: #selector(toggleAbout))
        content.addArrangedSubview(indented(aboutToggle))
        content.setCustomSpacing(4, after: content.arrangedSubviews.last!)
        about.orientation = .vertical
        about.alignment = .leading
        about.spacing = 4
        content.addArrangedSubview(indented(about))

        disclosure(advancedToggle, action: #selector(toggleAdvanced))
        content.addArrangedSubview(indented(advancedToggle))
        advanced.orientation = .vertical
        advanced.alignment = .leading
        advanced.spacing = 8
        displayName.placeholderString = "Optional"
        username.placeholderString = "Usually your email address"
        smtpUsername.placeholderString = "Same as the mail username"
        imapHost.placeholderString = "imap.example.com"
        imapPort.placeholderString = "993"
        smtpHost.placeholderString = "smtp.example.com"
        smtpPort.placeholderString = "587"
        imapSecurity.addItems(withTitles: ["TLS", "STARTTLS"])
        smtpSecurity.addItems(withTitles: ["STARTTLS", "TLS"])
        for (label, field) in [("IMAP server", imapHost), ("IMAP port", imapPort), ("IMAP security", imapSecurity),
                               ("SMTP server", smtpHost), ("SMTP port", smtpPort), ("SMTP security", smtpSecurity),
                               ("Mail username", username), ("SMTP username", smtpUsername), ("Display name", displayName)] as [(String, NSView)] {
            advanced.addArrangedSubview(row(label, field))
        }
        advanced.isHidden = true
        content.addArrangedSubview(advanced)

        guideButton.target = self
        guideButton.action = #selector(openGuide)
        guideButton.isBordered = false
        guideButton.font = .systemFont(ofSize: 12)
        guideButton.contentTintColor = .linkColor
        guideButton.setAccessibilityRole(.link)
        content.addArrangedSubview(indented(guideButton))


        switchToAppPassword.isBordered = false
        switchToAppPassword.contentTintColor = .linkColor
        switchToAppPassword.font = .systemFont(ofSize: 12, weight: .semibold)
        switchToAppPassword.target = self
        switchToAppPassword.action = #selector(switchToAppPasswordClicked)
        content.addArrangedSubview(switchToAppPassword)

        // Dialog buttons at the bottom right, Return on the default button.
        let buttons = NSStackView()
        buttons.orientation = .horizontal
        buttons.spacing = 10
        let cancel = NSButton(title: "Cancel", target: self, action: #selector(cancelClicked))
        cancel.keyEquivalent = "\u{1b}"
        connectButton.target = self
        connectButton.action = #selector(connectClicked)
        connectButton.keyEquivalent = "\r"
        connectButton.bezelStyle = .rounded
        buttons.addArrangedSubview(cancel)
        buttons.addArrangedSubview(connectButton)
        let footer = NSStackView()
        footer.orientation = .horizontal
        let flexible = NSView()
        flexible.setContentHuggingPriority(.init(1), for: .horizontal)
        footer.addArrangedSubview(flexible)
        footer.addArrangedSubview(buttons)
        footer.widthAnchor.constraint(equalToConstant: Self.width - 60).isActive = true
        content.setCustomSpacing(20, after: switchToAppPassword)
        content.addArrangedSubview(footer)
    }

    // MARK: State

    private var guide: AppPasswordGuide? {
        switch selected {
        case .gmailAppPassword: AccountSetupCopy.gmail
        case .icloud: AccountSetupCopy.icloud
        default: nil
        }
    }

    private var guideSection: String? {
        switch selected {
        case .gmailAppPassword, .gmail: "gmail"
        case .icloud: "icloud"
        case .other: "other"
        default: nil
        }
    }

    /// Show what the current choice needs; nothing else.
    private func showChoice() {
        let choice = selected
        let guide = guide
        googleRows.forEach { $0.isHidden = choice != nil }
        for item in providerMenu.itemArray.dropFirst() {
            item.state = choices.indices.contains(item.tag) && choices[item.tag] == choice ? .on : .off
        }
        intro.isHidden = choice == nil
        intro.stringValue = guide?.intro ?? (choice == .other ? AccountSetupCopy.otherIntro : choice.map(AccountSetupCopy.oauthIntro) ?? "")
        steps.arrangedSubviews.forEach { $0.removeFromSuperview() }
        for (index, step) in (guide?.steps ?? []).enumerated() {
            let line = NSStackView()
            line.orientation = .horizontal
            line.alignment = .firstBaseline
            line.spacing = 6
            let number = NSTextField(labelWithString: "\(index + 1).")
            number.font = .monospacedDigitSystemFont(ofSize: 13, weight: .regular)
            number.textColor = .secondaryLabelColor
            number.alignment = .right
            number.widthAnchor.constraint(equalToConstant: 18).isActive = true
            line.addArrangedSubview(number)
            if let url = step.url {
                line.addArrangedSubview(link(step.text, url: url))
            } else {
                let text = NSTextField(labelWithString: step.text)
                text.font = .systemFont(ofSize: 13)
                line.addArrangedSubview(text)
            }
            steps.addArrangedSubview(line)
        }
        steps.isHidden = guide == nil
        let usesPassword = choice?.usesPassword == true
        passwordRow?.isHidden = !usesPassword
        passwordLabel.stringValue = guide?.fieldLabel ?? AccountSetupCopy.otherFieldLabel
        password.placeholderString = guide?.placeholder ?? AccountSetupCopy.otherPlaceholder
        aboutToggle.superview?.isHidden = guide == nil
        aboutToggle.title = guide?.disclosure ?? ""
        about.arrangedSubviews.forEach { $0.removeFromSuperview() }
        for line in guide?.about ?? [] {
            let label = Self.secondary(line, size: 12)
            label.preferredMaxLayoutWidth = Self.fieldWidth
            about.addArrangedSubview(label)
        }
        about.superview?.isHidden = true
        setDisclosure(aboutToggle, open: false)
        let guideURL = guideSection.flatMap { DistributionProfile.current?.setupGuideURL($0) }
        guideButton.superview?.isHidden = guideURL == nil
        guideButton.toolTip = guideURL?.absoluteString
        advancedToggle.superview?.isHidden = choice != .other || !advancedOffered
        advanced.isHidden = choice != .other || !serversShown
        setDisclosure(advancedToggle, open: serversShown)
        if choice == nil {
            connectButton.title = AccountSetupCopy.continueTitle
            connectButton.isEnabled = true
        } else if let choice {
            connectButton.title = usesPassword ? AccountSetupCopy.addAccount : "Sign in with browser"
            let oauthReady = usesPassword || !oauthClientID(for: choice).isEmpty
            connectButton.isEnabled = oauthReady && !inFlight.contains(choice)
        }
        clearProblem()
        if let choice, inFlight.contains(choice) {
            showStatus("Still connecting…")
        } else if let choice, !choice.usesPassword, oauthClientID(for: choice).isEmpty {
            showStatus(choice == .gmail
                ? "Google sign-in isn't configured in this build. Use Gmail (app password) instead."
                : "Sign-in isn't configured in this build yet.")
        }
        updateSwitch()
        updateWaiting()
        resizeToFit(recenter: false)
    }

    /// While the browser sign-in waits, only the wait message, the app-password
    /// link and Cancel apply. The address, provider menu, intro and the disabled
    /// sign-in button come back when it fails or is cancelled in the browser.
    private func updateWaiting() {
        let waiting = selected.map { !$0.usesPassword && inFlight.contains($0) } ?? false
        emailRows.forEach { $0.isHidden = waiting }
        intro.isHidden = selected == nil || waiting
        connectButton.isHidden = waiting
        // With no fields left, the wait message lines up with the title and link.
        (problemRow as? NSStackView)?.arrangedSubviews.first?.isHidden = waiting
        status.preferredMaxLayoutWidth = waiting ? Self.width - 60 : Self.fieldWidth
        if waiting, let focused = firstResponder as? NSView, focused.isHiddenOrHasHiddenAncestor {
            makeFirstResponder(nil)
        }
    }

    /// The Advanced disclosure appears for Other once a first attempt failed.
    private var advancedOffered = false

    private func choose(_ choice: AccountSetupChoice, byHand: Bool = true) {
        let next = choices.contains(choice) ? choice : (choices.last ?? .other)
        if byHand { pickedByHand = true }
        if next != selected {
            selected = next
            password.stringValue = ""
            signInFailed = false
            serversShown = false
            advancedOffered = false
            showChoice()
        }
        if let outcome = outcomes.removeValue(forKey: next) {
            if let problem = outcome.problem { signInFailed = next == .gmail; show(problem) }
            else { showStatus(outcome.message) }
        }
        if next.usesPassword {
            makeFirstResponder(email.stringValue.contains("@") ? password : email)
        } else {
            makeFirstResponder(connectButton)
        }
    }

    /// The provider for the typed address, asked of the engine (no network).
    private func provider(for address: String) -> AccountSetupChoice {
        let answer = core.call(["op": "setup_provider", "email": address]) as? [String: Any]
        return AccountSetupChoice.forProvider(answer?["provider"] as? String ?? "other", features: features,
                                              googleSignInConfigured: !oauthClientID(for: .gmail).isEmpty)
    }

    func controlTextDidChange(_ notification: Notification) {
        guard (notification.object as? NSTextField) === email, selected != nil, !pickedByHand else { return }
        // Follow the address while the person hasn't chosen a provider by hand.
        let address = email.stringValue.trimmingCharacters(in: .whitespaces)
        guard address.contains("@") else { return }
        let next = provider(for: address)
        if next != selected { selected = next; serversShown = false; advancedOffered = false; showChoice() }
    }

    private func fillServers(_ guess: ServerGuess) {
        imapHost.stringValue = guess.imapHost
        imapPort.stringValue = String(guess.imapPort)
        imapSecurity.selectItem(withTitle: guess.imapImplicitTLS ? "TLS" : "STARTTLS")
        smtpHost.stringValue = guess.smtpHost
        smtpPort.stringValue = String(guess.smtpPort)
        smtpSecurity.selectItem(withTitle: guess.smtpImplicitTLS ? "TLS" : "STARTTLS")
    }

    // MARK: Status and problems

    private func clearProblem() {
        problemRow?.isHidden = true
        status.stringValue = ""
        status.textColor = .secondaryLabelColor
        status.isHidden = true
        fixButton.isHidden = true
        fixURL = nil
        detailsToggle.isHidden = true
        details.isHidden = true
        details.stringValue = ""
    }

    private func showStatus(_ message: String) {
        clearProblem()
        status.stringValue = message
        status.isHidden = message.isEmpty
        problemRow?.isHidden = message.isEmpty
        resizeToFit(recenter: false)
    }

    /// One line, a fix link if there is one, and the server's words behind Details.
    private func show(_ problem: SetupProblem) {
        clearProblem()
        problemRow?.isHidden = false
        // The explanation stays closed so the error sits right under the field.
        about.superview?.isHidden = true
        setDisclosure(aboutToggle, open: false)
        status.stringValue = problem.message
        status.textColor = .systemRed
        status.isHidden = false
        if let label = problem.actionLabel, let url = problem.actionURL {
            fixURL = url
            fixButton.title = label + " ↗"
            fixButton.toolTip = url.absoluteString
            fixButton.isHidden = false
        }
        details.stringValue = problem.detail
        detailsToggle.isHidden = problem.detail.isEmpty
        setDisclosure(detailsToggle, open: false)
        if selected == .other {
            advancedOffered = true
            if let servers = problem.servers {
                fillServers(servers)
                serversShown = true
            }
            advancedToggle.superview?.isHidden = false
            advanced.isHidden = !serversShown
            setDisclosure(advancedToggle, open: serversShown)
        }
        updateSwitch()
        resizeToFit(recenter: false)
    }

    private func showProblem(_ message: String) {
        show(SetupProblem(["code": "invalid", "message": message])!)
    }

    /// Offer the app-password path wherever Google sign-in may not work for this person.
    private func updateSwitch(waiting: Bool = false) {
        let unconfigured = oauthClientID(for: .gmail).isEmpty
        switchToAppPassword.isHidden = !(selected == .gmail && (unconfigured || waiting || signInFailed))
    }

    /// Fit the window to the visible rows, growing downward in place.
    private func resizeToFit(recenter: Bool) {
        contentView?.layoutSubtreeIfNeeded()
        let height = ceil(content.fittingSize.height) + 64
        let top = frame.maxY
        setContentSize(NSSize(width: Self.width, height: max(200, height)))
        if recenter { center() } else if top > 0 { setFrameTopLeftPoint(NSPoint(x: frame.minX, y: top)) }
    }

    // MARK: Actions

    @objc private func providerMenuChanged(_ sender: NSPopUpButton) {
        guard let item = sender.selectedItem, choices.indices.contains(item.tag), item !== sender.itemArray.first else { return }
        choose(choices[item.tag])
    }

    @objc private func toggleAbout() {
        guard let wrapper = about.superview else { return }
        wrapper.isHidden.toggle()
        setDisclosure(aboutToggle, open: !wrapper.isHidden)
        resizeToFit(recenter: false)
    }

    @objc private func toggleAdvanced() {
        serversShown.toggle()
        if serversShown, imapHost.stringValue.isEmpty {
            let address = email.stringValue.trimmingCharacters(in: .whitespaces)
            imapHost.placeholderString = "imap." + (address.split(separator: "@").last.map(String.init) ?? "example.com")
            smtpHost.placeholderString = "smtp." + (address.split(separator: "@").last.map(String.init) ?? "example.com")
        }
        advanced.isHidden = !serversShown
        setDisclosure(advancedToggle, open: serversShown)
        resizeToFit(recenter: false)
    }

    @objc private func toggleDetails() {
        details.isHidden.toggle()
        setDisclosure(detailsToggle, open: !details.isHidden)
        resizeToFit(recenter: false)
    }

    @objc private func switchToAppPasswordClicked() {
        choose(.gmailAppPassword)
    }

    /// Google sign-in, offered only when the build allows it and has a client.
    private var googleSignInOffered: Bool {
        choices.contains(.gmail) && !oauthClientID(for: .gmail).isEmpty
    }

    /// Straight to the browser: no address needed, Google supplies it.
    @objc private func continueWithGoogle() {
        choose(.gmail)
        connectClicked()
    }

    /// A typed gmail.com or googlemail.com address, so Google skips its
    /// account chooser. Anything else could be someone else's account.
    private func googleLoginHint() -> String? {
        let address = email.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        guard address.contains("@") else { return nil }
        let answer = core.call(["op": "setup_provider", "email": address]) as? [String: Any]
        return answer?["provider"] as? String == "gmail" ? address : nil
    }

    @objc private func openLink(_ sender: NSButton) {
        if let raw = sender.identifier?.rawValue, let url = URL(string: raw) { NSWorkspace.shared.open(url) }
    }

    @objc private func openGuide() {
        if let url = guideSection.flatMap({ DistributionProfile.current?.setupGuideURL($0) }) { NSWorkspace.shared.open(url) }
    }

    @objc private func openFix() {
        if let fixURL { NSWorkspace.shared.open(fixURL) }
    }

    @objc private func cancelClicked() { close() }

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

    /// The `add_imap_account` request, identical to the Linux payload
    /// (`apps/linux/src/account_setup.rs`, `imap_payload`). `nil` with a message
    /// when the form is incomplete. Other sends no servers until they are
    /// shown; the engine then guesses them from the address.
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
                     "smtp_username": address, "password": password.stringValue], nil)
        }
        if choice == .icloud {
            guard address.contains("@"), hasPassword else { return (nil, "Enter your iCloud address and app-specific password.") }
            // The engine's iCloud preset owns the servers and username.
            return (["op": "add_imap_account", "provider": "icloud", "email": address, "display_name": address,
                     "password": password.stringValue], nil)
        }
        guard serversShown else {
            guard address.contains("@"), hasPassword else { return (nil, "Enter your email address and password.") }
            return (["op": "add_imap_account", "provider": "imap", "email": address,
                     "display_name": displayName.stringValue.isEmpty ? address : displayName.stringValue,
                     "password": password.stringValue], nil)
        }
        let typedLogin = username.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        let login = typedLogin.isEmpty ? address : typedLogin
        // Spaces around server names and ports are ignored, as on Linux.
        let imapServer = imapHost.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        let smtpServer = smtpHost.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        guard address.contains("@"), !login.isEmpty, hasPassword,
              !imapServer.isEmpty, !smtpServer.isEmpty,
              let imap = Int(imapPort.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)), (1...65535).contains(imap),
              let smtp = Int(smtpPort.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)), (1...65535).contains(smtp) else {
            return (nil, "Enter a valid email, provider-issued app password, server names, and ports.")
        }
        let smtpLogin = smtpUsername.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        return (["op": "add_imap_account", "provider": "imap", "email": address,
                 "display_name": displayName.stringValue.isEmpty ? address : displayName.stringValue,
                 "username": login, "smtp_username": smtpLogin.isEmpty ? login : smtpLogin,
                 "password": password.stringValue,
                 "imap_host": imapServer, "imap_port": imap,
                 "imap_security": imapSecurity.titleOfSelectedItem == "TLS" ? "implicit_tls" : "start_tls",
                 "smtp_host": smtpServer, "smtp_port": smtp,
                 "smtp_security": smtpSecurity.titleOfSelectedItem == "TLS" ? "implicit_tls" : "start_tls"], nil)
    }

    @objc private func connectClicked() {
        guard let choice = selected else {
            // Email first: the address decides which steps to show.
            let address = email.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
            guard address.contains("@") else { showProblem(AccountSetupCopy.enterEmail); return }
            choose(provider(for: address), byHand: false)
            return
        }
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
            if choice == .gmail, let hint = googleLoginHint() {
                oauthPayload["login_hint"] = hint
            }
            payload = oauthPayload
        }
        inFlight.insert(choice)
        connectButton.isEnabled = false
        let waiting = switch choice {
        case .gmailAppPassword: "Connecting to Gmail…"
        case .icloud: "Connecting to iCloud…"
        case .other: "Checking your mail server…"
        case .gmail: "Waiting for browser sign-in… If Google shows “Access blocked”, close that tab and use an app password."
        case .outlook: "Waiting for browser sign-in…"
        }
        showStatus(waiting)
        updateSwitch(waiting: choice == .gmail)
        updateWaiting()
        // The switch link can appear now; fit it, or it clips the buttons.
        resizeToFit(recenter: false)
        Task { @MainActor in
            defer { inFlight.remove(choice) }
            do {
                let account = try await core.callAsync(payload)
                if selected == choice {
                    password.stringValue = ""
                    showStatus("Connected.")
                    onConnected()
                    close()
                } else {
                    // Keep the window and whatever is typed for the other choice.
                    let address = (account as? [String: Any])?["email"] as? String
                    outcomes[choice] = (address.map { "Connected \($0)." } ?? "Connected.", nil)
                    onConnected()
                }
            } catch {
                inFlight.remove(choice) // before deciding which button state to show
                let problem = (error as? MailServiceError)?.setupProblem
                    ?? SetupProblem(["code": "other", "message": error.localizedDescription])!
                // Another choice is on screen: keep the message for when it returns.
                guard selected == choice else {
                    outcomes[choice] = (problem.message, problem)
                    if let selected {
                        connectButton.isEnabled = (selected.usesPassword || !oauthClientID(for: selected).isEmpty)
                            && !inFlight.contains(selected)
                    }
                    return
                }
                signInFailed = choice == .gmail
                updateWaiting()
                show(problem)
                connectButton.isEnabled = true
                if !choice.usesPassword { makeFirstResponder(connectButton) }
            }
        }
    }
}
