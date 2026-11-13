#!/usr/bin/env python3
"""Exercise the actual AppKit account window against the synthetic Gmail fixture.

Never contacts Google: the fixture listens on loopback and the libraries are
QA builds (`--features test-fixture`) that redirect imap.gmail.com and
smtp.gmail.com to it. Build the inputs with:

  CARGO_TARGET_DIR=target/qa-dev cargo build -p mail-service --features test-fixture
  EMAIL_RELEASE_CHANNEL=distribution CARGO_TARGET_DIR=target/qa-distribution \\
    cargo build -p mail-service --features test-fixture
  cargo build -p mail-imap --features test-fixture --example gmail_fixture
"""
from pathlib import Path
import argparse, os, subprocess, tempfile, threading

parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
parser.add_argument('--library', type=Path, required=True, help='Development QA libmail_service.dylib')
parser.add_argument('--distribution-library', type=Path, required=True, help='Distribution QA libmail_service.dylib')
parser.add_argument('--fixture', type=Path, required=True, help='gmail_fixture example binary')
parser.add_argument('--snapshots', type=Path, help='Directory for PNG captures of key states')
parser.add_argument('--fixture-log', type=Path, help='Write the fixture command log here')
args = parser.parse_args()
root = Path(__file__).resolve().parents[3]
source = root / 'apps/macos/Sources/EmailUI'

check = r'''
extension AccountSetupWindow {
    static func runChecks(mode: String) {
        func expect(_ ok: Bool, _ name: String) { precondition(ok, "FAIL [\(mode)] \(name)"); print("PASS [\(mode)] \(name)") }
        func spin(_ seconds: TimeInterval) { RunLoop.main.run(until: Date().addingTimeInterval(seconds)) }
        func wait(_ name: String, timeout: TimeInterval = 20, _ condition: () -> Bool) {
            let deadline = Date().addingTimeInterval(timeout)
            while !condition() && Date() < deadline { spin(0.05) }
            expect(condition(), name)
        }
        func snapshot(_ window: NSWindow, _ name: String) {
            guard let dir = ProcessInfo.processInfo.environment["EMAIL_CHECK_SNAPSHOTS"], let view = window.contentView else { return }
            spin(0.3)
            view.layoutSubtreeIfNeeded()
            // The window, not its content view, paints the background; copy it
            // onto the view's layer for the capture only.
            var background = NSColor.windowBackgroundColor.cgColor
            window.effectiveAppearance.performAsCurrentDrawingAppearance { background = window.backgroundColor.cgColor }
            let previous = view.layer?.backgroundColor
            view.wantsLayer = true
            view.layer?.backgroundColor = background
            defer { view.layer?.backgroundColor = previous }
            guard let rep = view.bitmapImageRepForCachingDisplay(in: view.bounds) else { return }
            view.cacheDisplay(in: view.bounds, to: rep)
            try? rep.representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: dir).appendingPathComponent("\(mode)-\(name).png"))
        }
        func key(_ window: NSWindow, _ characters: String, _ code: UInt16, command: Bool = false) -> Bool {
            let event = NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: command ? [.command] : [], timestamp: 0, windowNumber: window.windowNumber, context: nil, characters: characters, charactersIgnoringModifiers: characters, isARepeat: false, keyCode: code)!
            return window.performKeyEquivalent(with: event)
        }
        func editing(_ window: NSWindow, _ field: NSTextField) -> Bool {
            (window.firstResponder as? NSTextView)?.delegate === field || window.firstResponder === field
        }
        /// Shown on screen: the view and all its ancestors are visible.
        func shown(_ view: NSView) -> Bool {
            var current: NSView? = view
            while let v = current { if v.isHidden { return false }; current = v.superview }
            return true
        }
        func texts(_ view: NSView) -> [String] {
            var found: [String] = []
            if shown(view) {
                if let field = view as? NSTextField { found.append(field.stringValue); found.append(field.placeholderString ?? "") }
                if let button = view as? NSButton { found.append(button.title) }
            }
            return found + view.subviews.flatMap(texts)
        }
        func shownButtons(_ view: NSView) -> [String] {
            ((view as? NSButton).flatMap { shown($0) ? [$0.title] : nil } ?? []) + view.subviews.flatMap(shownButtons)
        }
        guard let core = CoreBridge() else { fatalError("mail-service unavailable") }
        let features = core.buildFeatures()
        var connected = 0
        func open() -> AccountSetupWindow {
            let window = AccountSetupWindow(core: core) { connected += 1 }
            window.makeKeyAndOrderFront(nil)
            spin(0.2)
            return window
        }
        func enter(_ window: AccountSetupWindow, _ address: String) {
            window.email.stringValue = address
            window.connectButton.performClick(nil)
            spin(0.1)
        }
        var window = open()
        let titles = window.choices.map { $0.title(features) }
        let stepLinks = { (w: AccountSetupWindow) in w.steps.arrangedSubviews.flatMap { ($0 as? NSStackView)?.arrangedSubviews ?? [] }.compactMap { ($0 as? NSButton)?.identifier?.rawValue } }

        // Email first, in every build.
        expect(window.title == "Add an account" && window.titleVisibility == .hidden, "one title: the heading")
        expect(window.selected == nil && window.connectButton.title == "Continue", "opens on the email field with Continue")
        expect(window.intro.isHidden && window.passwordRow!.isHidden && window.steps.isHidden, "nothing but the email field before an address")
        expect(editing(window, window.email), "email field has keyboard focus")
        expect(window.providerMenu.itemArray.first?.title == "Choose provider", "small Choose provider control")
        window.connectButton.performClick(nil)
        expect(window.status.textColor == .systemRed && window.status.stringValue == "Enter your email address.", "Continue without an address explains itself")
        snapshot(window, "0-email-first")
        window.close()

        if mode == "distribution" {
            expect(features.channel == "distribution" && !features.googleOAuthPublic && !features.microsoftAccounts && !features.googleContactPhotos, "distribution library reports its features")
            expect(titles == ["Gmail", "iCloud", "Other"], "one Gmail entry, no Google sign-in, no Outlook")

            // The main window before any account is one welcome.
            let empty = MailWindow()
            empty.setContentSize(NSSize(width: 1100, height: 720))
            empty.makeKeyAndOrderFront(nil)
            spin(1.5)
            let welcome = shownButtons(empty.contentView!)
            expect(welcome.contains("Add an account") && welcome.contains("Add account") && welcome.contains("Support & feedback"), "welcome keeps Add account and Support")
            expect(!welcome.contains("Compose") && !welcome.contains("Calendar") && !welcome.contains("Manage accounts") && !welcome.contains("Agent access"), "welcome hides mail-only rows: \(welcome)")
            expect(!texts(empty.contentView!).contains("Connect an account to see your mail."), "no duplicate list message")
            snapshot(empty, "1-welcome")
            empty.close()

            for (address, choice) in [("someone@gmail.com", AccountSetupChoice.gmailAppPassword), ("someone@googlemail.com", .gmailAppPassword),
                                      ("someone@icloud.com", .icloud), ("someone@me.com", .icloud), ("someone@mac.com", .icloud),
                                      ("someone@company.example", .other), ("someone@hotmail.com", .other)] {
                let w = open(); enter(w, address)
                expect(w.selected == choice, "\(address) opens \(choice)")
                w.close()
            }

            window = open()
            enter(window, " person@gmail.example ")
            expect(window.selected == .other, "an unknown domain goes to Other")
            expect(key(window, "1", 18, command: true) && window.selected == .gmailAppPassword, "⌘1 chooses Gmail by hand")
            expect(window.intro.stringValue == "Gmail needs an app password." && !window.steps.isHidden, "Gmail steps show")
            expect(stepLinks(window) == ["https://myaccount.google.com/signinoptions/twosv", "https://myaccount.google.com/apppasswords"], "steps link to 2-Step Verification and App passwords")
            expect(window.password.placeholderString == "16 letters, spaces are fine" && editing(window, window.password), "focus moves to the app password")
            expect(window.about.superview?.isHidden == true && window.aboutToggle.title == "What's an app password?", "explanation starts collapsed")
            expect(window.guideButton.superview?.isHidden == (DistributionProfile.current?.setupGuide == nil), "guide link follows the profile's support channel")
            let visible = texts(window.contentView!).joined(separator: " ")
            expect(!visible.contains("review") && !visible.contains("Never enter") && !visible.contains("imap.gmail.com"), "no review promise, defensive copy or server footnote: \(visible)")
            window.aboutToggle.performClick(nil)
            expect(window.about.superview?.isHidden == false && texts(window.about).contains { $0.contains("Advanced Protection") }, "disclosure explains accounts without app passwords")
            snapshot(window, "2-gmail")
            window.aboutToggle.performClick(nil)
            // Stale hidden server fields from another choice never travel with Gmail.
            window.imapHost.stringValue = "imap.example.com"; window.imapPort.stringValue = "143"
            window.password.stringValue = "abcd efgh ijkl mnop"
            let request = window.imapPayload(for: .gmailAppPassword).payload ?? [:]
            expect(Set(request.keys) == ["op", "provider", "preset", "email", "display_name", "username", "smtp_username", "password"]
                   && request["preset"] as? String == "gmail" && request["email"] as? String == "person@gmail.example",
                   "Gmail request names no servers (same keys as Linux)")
            let icloud = window.imapPayload(for: .icloud).payload ?? [:]
            expect(Set(icloud.keys) == ["op", "provider", "email", "display_name", "password"] && icloud["provider"] as? String == "icloud", "iCloud request leaves servers to the engine (as Linux)")
            window.email.stringValue = "person@example.com"; window.password.stringValue = "secret"
            let guessed = window.imapPayload(for: .other).payload ?? [:]
            expect(guessed["imap_host"] == nil && guessed["smtp_host"] == nil, "Other sends no servers until they are shown (as Linux)")

            expect(key(window, "2", 19, command: true) && window.selected == .icloud, "⌘2 selects iCloud")
            expect(window.intro.stringValue == "iCloud needs an app-specific password." && stepLinks(window) == ["https://account.apple.com/"], "iCloud steps link to the Apple Account")
            snapshot(window, "3-icloud")
            expect(key(window, "3", 20, command: true) && window.selected == .other && window.steps.isHidden, "⌘3 selects Other: no steps")
            expect(window.advancedToggle.superview?.isHidden == true && window.advanced.isHidden, "Other hides server settings until a failure")
            snapshot(window, "4-other")
            expect(key(window, "1", 18, command: true) && window.selected == .gmailAppPassword, "⌘1 returns to Gmail")

            func attempt(_ address: String, _ secret: String, _ message: String, fix: String?, details: String? = nil, _ name: String, snapshotName: String? = nil) {
                window.email.stringValue = address
                window.password.stringValue = secret
                window.connectButton.performClick(nil)
                wait(name) { window.status.textColor == .systemRed && window.connectButton.isEnabled }
                expect(window.status.stringValue == message, "\(name): \(window.status.stringValue)")
                expect(fix == nil ? window.fixButton.isHidden : (!window.fixButton.isHidden && window.fixButton.title == fix! + " ↗"), "\(name): fix link \(window.fixButton.isHidden ? "none" : window.fixButton.title)")
                if let details { expect(!window.detailsToggle.isHidden && window.details.stringValue.contains(details), "\(name): server words behind Details") }
                expect(!window.status.stringValue.contains(secret.filter { !$0.isWhitespace }) && !window.details.stringValue.contains(secret.filter { !$0.isWhitespace }), "\(name): password not echoed")
                if let snapshotName { window.detailsToggle.performClick(nil); snapshot(window, snapshotName); window.detailsToggle.performClick(nil) }
            }
            window.aboutToggle.performClick(nil)
            attempt("fixture.user@gmail.com", "my regular google password", "That looks like your Google password.", fix: "Create an app password instead", details: "16 letters", "regular Google password is refused locally", snapshotName: "5-google-password")
            func frame(_ view: NSView) -> NSRect { view.convert(view.bounds, to: nil) }
            expect(window.about.superview?.isHidden == true, "an error closes the explanation")
            let error = frame(window.status), field = frame(window.password), explanation = frame(window.aboutToggle)
            expect(error.maxY <= field.minY && error.minY > explanation.maxY && abs(error.minX - field.minX) < 4,
                   "the error sits directly under the password field, lined up with it")
            attempt("fixture.user@gmail.com", "zzzz yyyy xxxx wwww", "Wrong email or app password.", fix: "Create a new app password", details: "AUTHENTICATIONFAILED", "wrong app password", snapshotName: "6-wrong-app-password")
            attempt("fixture.2sv@workspace.example", "qrst qrst qrst qrst", "That looks like your Google password.", fix: "Create an app password instead", details: "Application-specific password required", "account needs an app password")
            attempt("fixture.admin@workspace.example", "abcd efgh ijkl mnop", "Your Google Workspace admin has turned off IMAP for this account.", fix: nil, details: "End User Access", "Workspace admin turned IMAP off")
            attempt("fixture.weblogin@workspace.example", "abcd efgh ijkl mnop", "Google blocked this sign-in. Confirm it was you in Gmail, then try again.", fix: "Open Gmail", "Google blocked the sign-in")
            expect(key(window, "2", 19, command: true), "switch to iCloud")
            attempt("someone@icloud.example", "MyApplePassw0rd!", "That looks like your Apple Account password.", fix: "Create an app-specific password instead", "Apple Account password", snapshotName: "7-apple-password")
            attempt("someone@icloud.example", "abcd-efgh-ijkl-mnop", "Wrong email or app-specific password.", fix: "Create a new app-specific password", "wrong app-specific password")
            expect(key(window, "3", 20, command: true), "switch to Other")
            attempt("person@guess.example", "secret", "Wrong email or app password.", fix: nil, details: "imap.guess.example:993 said", "guessed server refuses the password")
            expect(window.advancedToggle.superview?.isHidden == false && window.advanced.isHidden, "Advanced is offered, closed, after a refusal")
            attempt("person@example.invalid", "secret", "Couldn't find the mail servers for example.invalid. Check the server settings.", fix: nil, "guessed servers do not answer", snapshotName: "8-servers-not-found")
            expect(!window.advanced.isHidden && window.imapHost.stringValue == "imap.example.invalid" && window.smtpPort.stringValue == "587", "the guess is shown for editing")
            let typed = window.imapPayload(for: .other).payload ?? [:]
            expect(typed["imap_host"] as? String == "imap.example.invalid" && typed["imap_port"] as? Int == 993, "shown servers are sent")
            window.imapHost.stringValue = "   "
            expect(window.imapPayload(for: .other).payload == nil, "a blank server name is refused before sending (as Linux)")
            expect(core.accountRecords().isEmpty, "failed attempts save no account")
            expect(key(window, "1", 18, command: true), "back to Gmail")

            // A slow request: the same choice can't be submitted twice, others stay usable.
            window.email.stringValue = "fixture.slow@workspace.example"
            window.password.stringValue = "abcd efgh ijkl mnop"
            window.connectButton.performClick(nil)
            expect(!window.connectButton.isEnabled && window.inFlight.contains(.gmailAppPassword), "submitting disables Add account for that choice")
            expect(key(window, "2", 19, command: true) && window.connectButton.isEnabled, "another choice stays usable meanwhile")
            expect(key(window, "1", 18, command: true) && !window.connectButton.isEnabled && window.status.stringValue == "Still connecting…", "returning shows the request still running")
            expect(key(window, "2", 19, command: true), "switch away again")
            wait("slow request finishes") { window.inFlight.isEmpty }
            expect(window.connectButton.isEnabled && window.status.textColor != .systemRed, "its error is not shown on another choice")
            expect(key(window, "1", 18, command: true) && window.connectButton.isEnabled, "Gmail is usable again afterwards")
            expect(window.status.textColor == .systemRed && window.status.stringValue == "Wrong email or app password.", "returning to Gmail shows how its request ended")
            expect(core.accountRecords().isEmpty, "the slow attempt saved nothing")

            // A slow success while another choice is on screen keeps the window and that choice's input.
            window.email.stringValue = "fixture.later@workspace.example"
            window.password.stringValue = "abcd efgh ijkl mnop"
            window.connectButton.performClick(nil)
            expect(key(window, "2", 19, command: true), "switch to iCloud while Gmail connects")
            window.email.stringValue = "someone@icloud.example"
            window.password.stringValue = "half typed"
            wait("background connect finishes") { window.inFlight.isEmpty }
            expect(window.isVisible && connected == 1 && window.selected == .icloud, "a success elsewhere refreshes the app but keeps this window")
            expect(window.email.stringValue == "someone@icloud.example" && window.password.stringValue == "half typed", "what is typed for iCloud is kept")
            expect(key(window, "1", 18, command: true) && window.status.stringValue == "Connected fixture.later@workspace.example." && window.status.textColor != .systemRed, "returning to Gmail shows it connected")

            window.email.stringValue = "fixture.user@gmail.com"
            window.password.stringValue = "abcd efgh ijkl mnop"
            expect(key(window, "\r", 36), "Return submits")
            wait("connects and closes") { connected == 2 && !window.isVisible }
            let all = core.accountRecords()
            expect(all.count == 2 && all.allSatisfy { $0.provider == "imap" }, "both accounts are saved as IMAP")
            let accounts = all.filter { $0.email == "fixture.user@gmail.com" }
            expect(accounts.count == 1, "the account submitted last is saved")
            let synced = (try? core.request(["op": "sync_account", "account_id": accounts[0].id])) as? [String: Any]
            expect(synced?["messages_added"] as? Int == 3, "initial sync downloads the fixture inbox")
            let found = (try? core.request(["op": "search", "query": "roadmap", "account_id": accounts[0].id])) as? [[String: Any]]
            expect(found?.count == 1, "search finds the synced message")

            let mail = MailWindow()
            mail.setContentSize(NSSize(width: 1280, height: 760))
            mail.makeKeyAndOrderFront(nil)
            spin(3)
            let chrome = shownButtons(mail.contentView!)
            expect(chrome.contains("Compose") && chrome.contains("Manage accounts"), "the sidebar comes back with an account")
            expect(!chrome.contains("Calendar"), "no Calendar where none can be connected (app-password accounts, no Google sign-in)")
            snapshot(mail, "9-inbox")
            let pane = ContactPane(frame: .zero)
            pane.show(sender: "Maya Fixture <maya@example.com>", recent: [], googleAccountID: accounts[0].id, core: core)
            expect(!pane.pictureMenu().items.map(\.title).contains("Use Google contact photos…"), "contact photos are not offered")
            mail.close()

            let second = open()
            expect(key(second, "\u{1b}", 53), "Escape is handled")
            wait("Escape closes the window") { !second.isVisible }
        } else {
            expect(features.channel == "development" && features.googleOAuthPublic && features.microsoftAccounts && features.googleContactPhotos, "development library keeps every feature")
            expect(titles == ["Gmail", "Gmail (app password)", "iCloud", "Outlook", "Other"], "development lists Gmail first and keeps Outlook")
            window = open(); enter(window, "someone@gmail.com")
            expect(window.selected == .gmail, "a Gmail address opens Google sign-in when a client is configured")
            expect(window.switchToAppPassword.isHidden && window.intro.stringValue == "You'll sign in to Google in your browser." && window.connectButton.title == "Sign in with browser", "Google sign-in unchanged in development")
            snapshot(window, "1-gmail")
            expect(key(window, "2", 19, command: true) && window.selected == .gmailAppPassword, "⌘2 selects Gmail (app password)")
            expect(!window.steps.isHidden && stepLinks(window).count == 2, "steps are visible")
            snapshot(window, "2-app-password")
            expect(key(window, "4", 21, command: true) && window.selected == .outlook, "⌘4 selects Outlook")
            snapshot(window, "3-outlook")
            window.close()
            window = open(); enter(window, "someone@hotmail.com")
            expect(window.selected == .outlook, "an Outlook address opens Outlook where it is offered")
            window.close()
            let pane = ContactPane(frame: .zero)
            pane.show(sender: "Maya Fixture <maya@example.com>", recent: [], googleAccountID: "demo:personal", core: core)
            expect(pane.pictureMenu().items.map(\.title).contains("Use Google contact photos…"), "contact photos remain in development builds")
        }
        print("ALL PASS [\(mode)]")
    }
}
'''

fixture_log = []
with tempfile.TemporaryDirectory(prefix='email-account-setup-') as temp:
    p = Path(temp)
    ca = p / 'fixture-ca.pem'
    server = subprocess.Popen([str(args.fixture.resolve(strict=True)), str(ca)], stdout=subprocess.PIPE, text=True)
    env_lines = {}
    while len(env_lines) < 2:
        line = server.stdout.readline()
        if not line:
            raise SystemExit('fixture exited early')
        fixture_log.append(line)
        if line.startswith('EMAIL_TEST_FIXTURE_'):
            name, value = line.strip().split('=', 1)
            env_lines[name] = value
    threading.Thread(target=lambda: fixture_log.extend(iter(server.stdout.readline, '')), daemon=True).start()
    # The fixture also answers for iCloud and a guessed Other server, so their
    # refusals are checked without any real provider.
    imap_port = env_lines['EMAIL_TEST_FIXTURE_ENDPOINTS'].split('imap.gmail.com:993=localhost:')[1].split(',')[0]
    env_lines['EMAIL_TEST_FIXTURE_ENDPOINTS'] += f',imap.mail.me.com:993=localhost:{imap_port},imap.guess.example:993=localhost:{imap_port}'
    try:
        ui = p / 'AccountSetupWindow.swift'
        ui.write_text(source.joinpath('AccountSetupWindow.swift').read_text() + check)
        main = p / 'Check.swift'
        main.write_text('import AppKit\n@main struct Check { @MainActor static func main() { let app = NSApplication.shared; app.setActivationPolicy(.accessory); app.finishLaunching(); AccountSetupWindow.runChecks(mode: ProcessInfo.processInfo.environment["EMAIL_CHECK_MODE"]!) } }')
        files = [str(f) for f in source.glob('*.swift') if f.name != 'AccountSetupWindow.swift']
        subprocess.run(['swiftc', '-parse-as-library', '-swift-version', '6', '-o', str(p / 'check'), str(ui), str(main), *files], check=True)
        for mode, library in (('development', args.library), ('distribution', args.distribution_library)):
            env = {k: v for k, v in os.environ.items() if not k.startswith('EMAIL_')}
            # File credentials keep the run out of the macOS Keychain entirely.
            env.update(env_lines, EMAIL_CHECK_MODE=mode, EMAIL_STORE_PATH=str(p / mode / 'mail.sqlite'),
                       EMAIL_TEST_CREDENTIALS_DIR=str(p / mode / 'credentials'),
                       MAIL_SERVICE_LIBRARY=str(library.resolve(strict=True)),
                       EMAIL_GOOGLE_CLIENT_ID='fixture-not-a-real-client.apps.googleusercontent.com')
            if args.snapshots:
                args.snapshots.mkdir(parents=True, exist_ok=True)
                env['EMAIL_CHECK_SNAPSHOTS'] = str(args.snapshots.resolve())
            subprocess.run([str(p / 'check')], env=env, check=True)
    finally:
        server.terminate()
        server.wait()
        if args.fixture_log:
            args.fixture_log.write_text(''.join(fixture_log))
