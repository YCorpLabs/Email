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
        guard let core = CoreBridge() else { fatalError("mail-service unavailable") }
        let features = core.buildFeatures()
        var connected = 0
        let window = AccountSetupWindow(core: core) { connected += 1 }
        window.makeKeyAndOrderFront(nil)
        spin(0.2)
        let titles = window.choices.map { $0.title(features) }
        let steps = window.gmailSteps.arrangedSubviews.flatMap { ($0 as? NSStackView)?.arrangedSubviews ?? [] }
        let links = steps.compactMap { ($0 as? NSButton)?.identifier?.rawValue }

        if mode == "distribution" {
            expect(features.channel == "distribution" && !features.googleOAuthPublic && !features.microsoftAccounts && !features.googleContactPhotos, "distribution library reports its features")
            expect(titles == ["Gmail", "iCloud", "Other"], "one Gmail entry, no Google sign-in, no Outlook")
            expect(!window.choices.contains(.gmail), "Google sign-in is not offered")
            expect(window.selected == .gmailAppPassword, "Gmail opens on the app-password path although a client ID is configured")
            let coming = (window.gmailSteps.arrangedSubviews.first as? NSTextField)?.stringValue
            expect(coming == "One-click Google sign-in is coming once Google finishes reviewing the app.", "one honest line about Google sign-in")
            expect(window.intro.stringValue == "Gmail and iCloud connect with a password created for this app. Never enter your main account password here.", "intro does not promise browser sign-in")
            expect(!window.gmailSteps.isHidden && !window.gmailAbout.isHidden && !window.emailRow!.isHidden, "steps, explanation and fields are visible")
            expect(links == ["https://myaccount.google.com/signinoptions/twosv", "https://myaccount.google.com/apppasswords"], "steps link to 2-Step Verification and App passwords")
            expect(window.helpButton.title == "Learn more about app passwords ↗" && !window.helpButton.isHidden, "help link names app passwords")
            // Stale hidden server fields from another choice never travel with Gmail.
            window.imapHost.stringValue = "imap.example.com"; window.imapPort.stringValue = "143"
            window.email.stringValue = " person@gmail.example "; window.password.stringValue = "abcd efgh ijkl mnop"
            let request = window.imapPayload(for: .gmailAppPassword).payload ?? [:]
            expect(Set(request.keys) == ["op", "provider", "preset", "email", "display_name", "username", "smtp_username", "password", "google_sign_in"]
                   && request["google_sign_in"] as? Bool == false
                   && request["preset"] as? String == "gmail" && request["email"] as? String == "person@gmail.example",
                   "Gmail request names no servers (same keys as Linux)")
            window.email.stringValue = "person@example.com"; window.password.stringValue = "secret"
            window.imapHost.stringValue = " imap.example.com "; window.imapPort.stringValue = " 993 "
            window.smtpHost.stringValue = "smtp.example.com"; window.smtpPort.stringValue = "587 "
            let other = window.imapPayload(for: .other).payload ?? [:]
            expect(other["imap_host"] as? String == "imap.example.com" && other["imap_port"] as? Int == 993 && other["smtp_port"] as? Int == 587,
                   "Other ignores spaces around servers and ports (as Linux)")
            window.imapHost.stringValue = "   "
            expect(window.imapPayload(for: .other).payload == nil, "a blank server name is refused before sending (as Linux)")
            window.email.stringValue = ""; window.password.stringValue = ""
            expect(window.advancedRows.allSatisfy(\.isHidden), "server rows stay hidden")
            expect(editing(window, window.email), "email field has keyboard focus")
            expect(window.switchToAppPassword.isHidden, "no switch link on the app-password choice")
            snapshot(window, "1-app-password")
            window.appearance = NSAppearance(named: .aqua)
            snapshot(window, "1-app-password-light")
            window.appearance = nil

            expect(key(window, "2", 19, command: true) && window.selected == .icloud, "⌘2 selects iCloud")
            snapshot(window, "2-icloud")
            expect(key(window, "1", 18, command: true) && window.selected == .gmailAppPassword && editing(window, window.email), "⌘1 returns to Gmail with focus in the address field")

            func attempt(_ address: String, _ secret: String, _ prefix: String, _ name: String, snapshotName: String? = nil) {
                window.email.stringValue = address
                window.password.stringValue = secret
                window.connectButton.performClick(nil)
                wait(name) { window.status.textColor == .systemRed && window.connectButton.isEnabled }
                expect(window.status.stringValue.hasPrefix(prefix), "\(name): \(window.status.stringValue)")
                expect(!window.status.stringValue.contains(secret.filter { !$0.isWhitespace }), "\(name): password not echoed")
                if let snapshotName { snapshot(window, snapshotName) }
            }
            attempt("fixture.user@gmail.com", "my regular google password", "That isn't a Google app password.", "regular Google password is refused locally", snapshotName: "3-regular-password")
            attempt("fixture.user@gmail.com", "zzzz yyyy xxxx wwww", "Google didn't accept this app password", "wrong app password", snapshotName: "4-wrong-app-password")
            attempt("fixture.2sv@workspace.example", "qrst qrst qrst qrst", "Google needs an app password here", "account needs an app password")
            attempt("fixture.admin@workspace.example", "abcd efgh ijkl mnop", "Your Google Workspace administrator has turned off IMAP", "Workspace admin turned IMAP off", snapshotName: "5-workspace-imap-off")
            attempt("fixture.weblogin@workspace.example", "abcd efgh ijkl mnop", "Google blocked this sign-in", "Google blocked the sign-in")
            expect(core.accountRecords().isEmpty, "failed attempts save no account")

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
            expect(window.status.textColor == .systemRed && window.status.stringValue.hasPrefix("Google didn't accept this app password"), "returning to Gmail shows how its request ended")
            snapshot(window, "5a-failure-shown-on-return")
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
            snapshot(window, "5b-success-shown-on-return")

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
            snapshot(mail, "6-inbox")
            let pane = ContactPane(frame: .zero)
            pane.show(sender: "Maya Fixture <maya@example.com>", recent: [], googleAccountID: accounts[0].id, core: core)
            expect(!pane.pictureMenu().items.map(\.title).contains("Use Google contact photos…"), "contact photos are not offered")
            mail.close()

            let second = AccountSetupWindow(core: core) {}
            second.makeKeyAndOrderFront(nil)
            spin(0.2)
            expect(key(second, "\u{1b}", 53), "Escape is handled")
            wait("Escape closes the window") { !second.isVisible }
        } else {
            expect(features.channel == "development" && features.googleOAuthPublic && features.microsoftAccounts && features.googleContactPhotos, "development library keeps every feature")
            expect(titles == ["Gmail", "Gmail (app password)", "iCloud", "Outlook", "Other"], "development lists Gmail first and keeps Outlook")
            expect(window.selected == .gmail, "Gmail sign-in is selected when a client is configured")
            expect(window.switchToAppPassword.isHidden && window.status.stringValue == "You'll sign in on your provider's website in your browser.", "Google sign-in unchanged in development")
            expect(!(window.gmailSteps.arrangedSubviews.first is NSTextField), "no sign-in-coming line in development")
            snapshot(window, "1-gmail")
            expect(key(window, "2", 19, command: true) && window.selected == .gmailAppPassword, "⌘2 selects Gmail (app password)")
            expect(!window.gmailSteps.isHidden && links.count == 2, "steps are visible")
            snapshot(window, "2-app-password")
            expect(key(window, "4", 21, command: true) && window.selected == .outlook, "⌘4 selects Outlook")
            snapshot(window, "3-outlook")
            let pane = ContactPane(frame: .zero)
            pane.show(sender: "Maya Fixture <maya@example.com>", recent: [], googleAccountID: "demo:personal", core: core)
            expect(pane.pictureMenu().items.map(\.title).contains("Use Google contact photos…"), "contact photos remain in development builds")
            window.close()
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
