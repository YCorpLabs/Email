#!/usr/bin/env python3
"""Check how a signed-out account shows in the actual AppKit window.

Compiles the real EmailUI sources with a check entry point and opens the launch
demo store through the given mail-service library. The demo's personal account
gets Gmail settings and a saved token that has expired with no refresh token,
so the engine's own sync finds it signed out, as it does when Google answers
invalid_grant. No request leaves the machine. It checks:

- the sidebar row shows a warning mark instead of the unread count;
- a notice with Sign in again sits above the list in All Inboxes and in that
  account's inbox, and not in the other account's inbox;
- a manual refresh (Cmd+R) reports the account in the status line and opens
  no alert (an alert would block this check until it times out);
- Manage accounts marks the account signed out;
- Sign in again opens the browser for that address and waits for it. A fake
  `open` first on PATH records the address instead of opening a browser.

Nothing is put on screen: the window is drawn off-screen with cacheDisplay and
no input is sent. The store, HOME and credentials directory are temporary.
Build the library with `--features test-fixture` so credentials come from
EMAIL_TEST_CREDENTIALS_DIR instead of the Keychain:

  cargo build -p mail-service --features test-fixture

With --snapshots it writes a PNG of each state.
"""
from pathlib import Path
import argparse, hashlib, json, os, subprocess, tempfile

parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
parser.add_argument('--library', type=Path, required=True, help='libmail_service.dylib')
parser.add_argument('--snapshots', type=Path, help='Optional directory for PNG captures of each state')
parser.add_argument('--appearance', choices=['light', 'dark'], default='light')
parser.add_argument('--timeout', type=int, default=180, help='Seconds before the check counts as stuck')
args = parser.parse_args()
library = args.library.resolve(strict=True)
root = Path(__file__).resolve().parents[3]
source = root / 'apps/macos/Sources/EmailUI'
signed_out_id = 'demo:personal'

check = r'''
    // Never put a check window on screen.
    override func makeKeyAndOrderFront(_ sender: Any?) {}
    override func orderFront(_ sender: Any?) {}
    override func orderFrontRegardless() {}
    func checkSignedOut() {
        let environment = ProcessInfo.processInfo.environment
        let signedOutID = environment["EMAIL_CHECK_SIGNED_OUT"]!
        func expect(_ ok: Bool, _ name: String) { precondition(ok, name); print("PASS \(name)"); fflush(stdout) }
        func pump(_ seconds: Double = 0.05) { RunLoop.main.run(until: Date().addingTimeInterval(seconds)) }
        func wait(_ name: String, _ seconds: Double = 20, _ condition: () -> Bool) {
            let deadline = Date().addingTimeInterval(seconds)
            while !condition() && Date() < deadline { pump(0.02) }
            expect(condition(), name)
        }
        func write(_ rep: NSBitmapImageRep, _ name: String) {
            guard let directory = environment["EMAIL_CHECK_SNAPSHOTS"] else { return }
            try? rep.representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: directory).appendingPathComponent("\(name).png"))
        }
        func render(_ view: NSView) -> NSBitmapImageRep? {
            view.layoutSubtreeIfNeeded()
            guard let rep = view.bitmapImageRepForCachingDisplay(in: view.bounds) else { return nil }
            view.cacheDisplay(in: view.bounds, to: rep)
            return rep
        }
        func snapshotView(_ view: NSView, _ name: String) {
            guard environment["EMAIL_CHECK_SNAPSHOTS"] != nil else { return }
            pump(0.4)
            if let rep = render(view) { write(rep, name) }
        }
        var webDone = false
        var webImage: NSImage?
        func snapshotMain(_ name: String) {
            guard environment["EMAIL_CHECK_SNAPSHOTS"] != nil, let view = contentView else { return }
            pump(0.8)
            guard let rep = render(view) else { return }
            // WKWebView draws out of process; composite its own snapshot.
            if let webView, !webView.isHidden, calendarWorkspace == nil {
                webDone = false; webImage = nil
                Task { @MainActor in webImage = try? await webView.takeSnapshot(configuration: nil); webDone = true }
                let deadline = Date().addingTimeInterval(5)
                while !webDone && Date() < deadline { pump(0.02) }
                if let image = webImage, let context = NSGraphicsContext(bitmapImageRep: rep) {
                    NSGraphicsContext.saveGraphicsState(); NSGraphicsContext.current = context
                    image.draw(in: webView.convert(webView.bounds, to: view))
                    NSGraphicsContext.restoreGraphicsState()
                }
            }
            write(rep, name)
        }
        func visibleTexts(_ view: NSView) -> [String] {
            guard !view.isHidden else { return [] }
            return ((view as? NSButton).map { [$0.title] } ?? []) + ((view as? NSTextField).map { [$0.stringValue] } ?? []) + view.subviews.flatMap(visibleTexts)
        }
        func sidebarRow(_ id: String) -> NSButton? {
            guard let index = knownAccounts.firstIndex(where: { $0.id == id }) else { return nil }
            return sidebarAccounts.arrangedSubviews.compactMap { $0 as? NSButton }.first { $0.tag == index }
        }
        func showAccount(_ id: String) {
            guard let row = sidebarRow(id) else { preconditionFailure("no sidebar row for \(id)") }
            showAccountClicked(row)
        }
        func noticeShown() -> Bool { !signedOutNotice.isHidden && signedOutNotice.superview?.isHidden == false }

        appearance = NSAppearance(named: environment["EMAIL_CHECK_APPEARANCE"] == "dark" ? .darkAqua : .aqua); applyTheme()
        setContentSize(NSSize(width: 1440, height: 900))
        wait("the launch demo inbox loads") { threads.count >= 60 }
        guard let account = knownAccounts.first(where: { $0.id == signedOutID }) else { preconditionFailure("no \(signedOutID) account") }
        let other = knownAccounts.first { $0.id != signedOutID }!

        // The launch sync finds the saved sign-in rejected.
        wait("the launch sync marks \(account.email) signed out", 30) { knownAccounts.first { $0.id == signedOutID }?.needsSignIn == true }
        expect(knownAccounts.first { $0.id == other.id }?.needsSignIn == false, "the other account stays signed in")
        wait("the launch sync finishes") { !syncInProgress }
        pump(0.3)
        let row = sidebarRow(signedOutID)
        expect(row?.toolTip == "\(account.email) is signed out. Click to sign in again.", "the sidebar row explains the mark (\(row?.toolTip ?? "nil"))")
        expect(row?.accessibilityLabel() == "\(account.email), signed out", "VoiceOver reads the row as signed out")
        expect(row?.subviews.contains { ($0 as? NSTextField)?.stringValue.allSatisfy(\.isNumber) == true } == false, "the row shows no unread count")
        expect(noticeShown(), "All Inboxes shows the signed-out notice")
        expect(signedOutNoticeText.stringValue + signedOutNoticeSuffix.stringValue == "\(account.email) is signed out", "the notice names the account (\(signedOutNoticeText.stringValue)\(signedOutNoticeSuffix.stringValue))")
        expect(signedOutNoticeButton.title == "Sign in again" && signedOutNoticeButton.isEnabled, "the notice offers Sign in again")
        expect(syncStatus.stringValue == "Updated just now · 1 account signed out", "the status line counts it (\(syncStatus.stringValue))")
        snapshotMain("mac-01-all-inboxes")

        // Cmd+R: the signed-out account opens no alert.
        syncAll(showErrors: true)
        wait("a manual refresh finishes without an alert") { !syncInProgress && syncStatus.stringValue != "Syncing…" }
        expect(syncStatus.stringValue == "Updated just now · 1 account signed out", "after Cmd+R the status line still counts it (\(syncStatus.stringValue))")
        expect(syncStatus.toolTip == "\(account.email) is signed out", "the status tooltip names it (\(syncStatus.toolTip ?? "nil"))")

        // The account's own inbox shows it; the other account's does not.
        showAccount(signedOutID)
        wait("the signed-out account's inbox opens") { inboxScope == .account(signedOutID) && !threads.isEmpty }
        expect(noticeShown() && signedOutNoticeAccountID == signedOutID, "the account's inbox shows the notice")
        snapshotMain("mac-02-account-inbox")
        showAccount(other.id)
        wait("the other account's inbox opens") { inboxScope == .account(other.id) && !threads.isEmpty }
        expect(!noticeShown(), "the other account's inbox has no notice")
        expect(!visibleTexts(contentView!).contains(" is signed out"), "no signed-out text in the other account's inbox")

        // Narrow window: the notice keeps the button and shortens the address.
        showAccount(signedOutID)
        wait("back in the signed-out inbox") { inboxScope == .account(signedOutID) }
        setContentSize(NSSize(width: 1000, height: 760))
        pump(0.5)
        contentView?.layoutSubtreeIfNeeded()
        let suffixFrame = signedOutNoticeSuffix.convert(signedOutNoticeSuffix.bounds, to: signedOutNotice)
        expect(suffixFrame.maxX <= signedOutNotice.bounds.maxX - 8 && signedOutNoticeButton.convert(signedOutNoticeButton.bounds, to: signedOutNotice).maxX <= signedOutNotice.bounds.maxX, "narrow window: the text and button stay inside the notice")
        snapshotMain("mac-03-narrow")
        setContentSize(NSSize(width: 1440, height: 900))
        pump(0.3)

        // Manage accounts lists it as signed out.
        let alert = manageAccountsAlert()
        alert.window.appearance = appearance
        alert.layout()
        expect(alert.informativeText.contains("\(account.email) (signed out)"), "Manage accounts marks it (\(alert.informativeText))")
        if let frame = alert.window.contentView?.superview { snapshotView(frame, "mac-05-manage-accounts") }

        // Sign in again opens the browser for this address and waits.
        signInAgainClicked(signedOutNoticeButton)
        wait("Sign in again waits for the browser") { signedOutNoticeButton.title == "Waiting for browser…" && !signedOutNoticeButton.isEnabled }
        expect(syncStatus.stringValue == "Finish signing in to \(account.email) in your browser", "the status line says where to go (\(syncStatus.stringValue))")
        let log = URL(fileURLWithPath: environment["EMAIL_CHECK_OPEN_LOG"]!)
        wait("the browser opens Google sign-in", 10) { ((try? String(contentsOf: log, encoding: .utf8)) ?? "").contains("accounts.google.com") }
        let opened = (try? String(contentsOf: log, encoding: .utf8)) ?? ""
        let hint = account.email.addingPercentEncoding(withAllowedCharacters: .alphanumerics.union(CharacterSet(charactersIn: "-._~"))) ?? account.email
        expect(opened.contains("login_hint=\(hint)") || opened.contains("login_hint=\(account.email)"), "sign-in suggests \(account.email)")
        snapshotMain("mac-04-waiting-for-browser")
        close()
    }
'''
main = '''import AppKit
@main struct Check { @MainActor static func main() {
    let app = NSApplication.shared; app.setActivationPolicy(.accessory); app.finishLaunching()
    for key in [RemoteImagePreference.enabledKey, RemoteImagePreference.explicitChoiceKey, RemoteImagePreference.migrationKey] { UserDefaults.standard.removeObject(forKey: key) }
    RemoteImagePreference.prepare(.standard)
    let window = MailWindow(); window.checkSignedOut()
    exit(0)
} }
'''
anchor = '    @objc private func editSelectedDraft() {'
with tempfile.TemporaryDirectory(prefix='email-signed-out-check-') as temp:
    work = Path(temp)
    text = source.joinpath('EmailUI.swift').read_text()
    assert anchor in text
    (work / 'EmailUI.swift').write_text(text.replace(anchor, check + '\n' + anchor, 1))
    (work / 'Check.swift').write_text(main)
    files = [str(f) for f in source.glob('*.swift') if f.name != 'EmailUI.swift']
    binary = work / 'email-signed-out-check'
    subprocess.run(['swiftc', '-parse-as-library', '-swift-version', '6', '-o', str(binary),
                    str(work / 'EmailUI.swift'), str(work / 'Check.swift'), *files], check=True)

    # Gmail settings and a dead token for the demo's personal account: the
    # access token has expired and there is no refresh token, so the first
    # sync fails locally exactly as a rejected (invalid_grant) sign-in does.
    store = work / 'store'
    store.mkdir(mode=0o700)
    config = store / 'mail.accounts.json'
    config.write_text(json.dumps([{'account_id': signed_out_id, 'generation': 'check', 'provider': 'gmail',
                                   'client_id': 'synthetic-client.apps.googleusercontent.com'}]))
    config.chmod(0o600)
    credentials = work / 'credentials'
    credentials.mkdir(mode=0o700)
    token = credentials / hashlib.sha256(signed_out_id.encode()).hexdigest()
    token.write_text(json.dumps({'access_token': 'synthetic-expired', 'refresh_token': None, 'expires_at_ms': 0}))
    token.chmod(0o600)

    # A fake `open` records the sign-in URL instead of opening a browser.
    bin_dir = work / 'bin'
    bin_dir.mkdir()
    open_log = work / 'open.log'
    fake_open = bin_dir / 'open'
    fake_open.write_text(f'#!/bin/sh\nprintf "%s\\n" "$*" >> "{open_log}"\n')
    fake_open.chmod(0o755)

    home = work / 'home'
    home.mkdir()
    env = dict(os.environ, HOME=str(home), EMAIL_STORE_PATH=str(store / 'mail.sqlite'), EMAIL_DEMO='launch',
               TZ='Europe/Oslo', MAIL_SERVICE_LIBRARY=str(library), EMAIL_CHECK_APPEARANCE=args.appearance,
               EMAIL_TEST_CREDENTIALS_DIR=str(credentials), EMAIL_CHECK_SIGNED_OUT=signed_out_id,
               EMAIL_CHECK_OPEN_LOG=str(open_log), PATH=f'{bin_dir}:{os.environ.get("PATH", "")}')
    if args.snapshots:
        args.snapshots.mkdir(parents=True, exist_ok=True)
        env['EMAIL_CHECK_SNAPSHOTS'] = str(args.snapshots.resolve())
    subprocess.run([str(binary)], env=env, check=True, timeout=args.timeout)
print('PASS: signed-out account AppKit check')
