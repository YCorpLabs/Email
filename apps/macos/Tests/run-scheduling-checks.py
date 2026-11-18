#!/usr/bin/env python3
"""Check the Scheduling opt-in in the actual AppKit windows.

Compiles the real EmailUI sources with a check entry point and opens the launch
demo store through the given mail-service library. With a library whose
profile names a Scheduling service (development), it checks:

- not connected: no booking UI anywhere; one Scheduling entry in Manage
  accounts and Connect Scheduling… in the command palette (also in the
  composer's);
- waiting for the browser: the entry shows the code and Cancel, and the alert
  grows to fit;
- connected: the entry shows the account, Disconnect and Manage online; an
  open composer gains Booking link; Calendar shows Bookings and links; "book"
  in the composer palette puts Insert booking link… first, in the main palette
  Bookings and meeting links;
- disconnecting hides it all again, also in an open composer.

With a library without a Scheduling service (the neutral profile) it checks
that none of it exists. The connection state is set directly; the Scheduling
service, a browser and the network are never used.

Nothing is put on screen: the windows are drawn off-screen with cacheDisplay
and no input is sent. The store, HOME and credentials directory are temporary.
Build the library with `--features test-fixture` so the saved-connection
lookup reads EMAIL_TEST_CREDENTIALS_DIR instead of the Keychain:

  EMAIL_DISTRIBUTION_PROFILE=development cargo build -p mail-service --features test-fixture

With --snapshots it writes a PNG of each state.
"""
from pathlib import Path
import argparse, os, subprocess, tempfile

parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
parser.add_argument('--library', type=Path, required=True, help='libmail_service.dylib')
parser.add_argument('--snapshots', type=Path, help='Optional directory for PNG captures of each state')
parser.add_argument('--appearance', choices=['light', 'dark'], default='light')
parser.add_argument('--icon', type=Path, help='Optional app icon PNG for the Manage accounts alert, as the app bundle has')
args = parser.parse_args()
library = args.library.resolve(strict=True)
root = Path(__file__).resolve().parents[3]
source = root / 'apps/macos/Sources/EmailUI'

check = r'''
    // Never put a check window on screen.
    override func makeKeyAndOrderFront(_ sender: Any?) {}
    override func orderFront(_ sender: Any?) {}
    override func orderFrontRegardless() {}
    var checkDone = false
    var checkImage: NSImage?
    func checkScheduling() {
        let environment = ProcessInfo.processInfo.environment
        let service = DistributionProfile.current?.hasScheduling == true
        let connection = SchedulingConnection.shared
        func expect(_ ok: Bool, _ name: String) { precondition(ok, name); print("PASS \(name)"); fflush(stdout) }
        func pump(_ seconds: Double = 0.05) { RunLoop.main.run(until: Date().addingTimeInterval(seconds)) }
        func wait(_ name: String, _ condition: () -> Bool) {
            let deadline = Date().addingTimeInterval(15)
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
        func snapshotMain(_ name: String) {
            guard environment["EMAIL_CHECK_SNAPSHOTS"] != nil, let view = contentView else { return }
            pump(0.6)
            guard let rep = render(view) else { return }
            // WKWebView draws out of process; composite its own snapshot.
            if let webView, !webView.isHidden, calendarWorkspace == nil {
                checkDone = false; checkImage = nil
                Task { @MainActor in self.checkImage = try? await webView.takeSnapshot(configuration: nil); self.checkDone = true }
                let deadline = Date().addingTimeInterval(5)
                while !checkDone && Date() < deadline { pump(0.02) }
                if let image = checkImage, let context = NSGraphicsContext(bitmapImageRep: rep) {
                    NSGraphicsContext.saveGraphicsState(); NSGraphicsContext.current = context
                    image.draw(in: webView.convert(webView.bounds, to: view))
                    NSGraphicsContext.restoreGraphicsState()
                }
            }
            write(rep, name)
        }
        func texts(_ view: NSView) -> [String] {
            ((view as? NSButton).map { [$0.title] } ?? []) + ((view as? NSTextField).map { [$0.stringValue] } ?? []) + view.subviews.flatMap(texts)
        }
        func visibleTexts(_ view: NSView) -> [String] {
            guard !view.isHidden else { return [] }
            return ((view as? NSButton).map { [$0.title] } ?? []) + ((view as? NSTextField).map { [$0.stringValue] } ?? []) + view.subviews.flatMap(visibleTexts)
        }
        func select(_ id: String) {
            guard let row = threads.firstIndex(where: { $0.id == id }) else { preconditionFailure("missing demo conversation \(id)") }
            table.selectRowIndexes(IndexSet(integer: row), byExtendingSelection: false)
            table.scrollRowToVisible(row)
        }
        func rendered(_ text: String) -> Bool { currentHTML?.contains(text) == true && currentWebDidCommit && webView?.isHidden == false }
        // The palette as ⌘K fills it, without ordering the panel on screen.
        func paletteTitles(_ query: String) -> [String] {
            clearGoTo()
            palettePage = .root
            paletteRootQuery = ""
            paletteResults = []
            paletteSearch.stringValue = query
            palette.appearance = appearance
            refreshPalette()
            return paletteResults.map(\.title)
        }
        func snapshotPalette(_ query: String, _ name: String) {
            _ = paletteTitles(query)
            if let view = palette.contentView?.superview ?? palette.contentView { snapshotView(view, name) }
        }
        let icon = environment["EMAIL_CHECK_ICON"].flatMap { NSImage(contentsOfFile: $0) }
        func alertWindow() -> NSAlert {
            let alert = manageAccountsAlert()
            if let icon { alert.icon = icon }
            alert.window.appearance = appearance
            alert.layout()
            return alert
        }
        func snapshotAlert(_ alert: NSAlert, _ name: String) {
            if let frame = alert.window.contentView?.superview { snapshotView(frame, name) }
        }
        func entryTexts(_ alert: NSAlert) -> [String] { (alert.accessoryView as? SchedulingAccountEntry)?.texts ?? [] }
        func calendarBookingsShown() -> Bool { calendarWorkspaceView.map(visibleTexts)?.contains("Bookings and links") == true }

        appearance = NSAppearance(named: environment["EMAIL_CHECK_APPEARANCE"] == "dark" ? .darkAqua : .aqua); applyTheme()
        setContentSize(NSSize(width: 1440, height: 900))
        wait("the launch demo inbox loads") { threads.count >= 60 }
        pump(2) // the saved connection loads after launch
        select("demo:launch:elena-intro")
        wait("the meeting request renders") { rendered("30-minute intro call") }

        // Nothing about Scheduling in the main window before connecting.
        expect(!connection.isConnected, "a fresh store has no Scheduling connection")
        expect(!texts(contentView!).contains("Scheduling"), "no Scheduling row in the sidebar")
        expect(!paletteItems().contains { $0.target == .bookings || $0.target == .insertBooking }, "no booking commands before connecting")
        snapshotMain("mac-04-main-window-not-connected")

        guard service else {
            let alert = alertWindow()
            expect(alert.accessoryView == nil, "without a Scheduling service Manage accounts lists mail accounts only")
            expect(!paletteTitles("sched").contains("Connect Scheduling…"), "without a Scheduling service there is no Connect Scheduling…")
            reply()
            wait("the reply box opens") { composePane != nil }
            expect(composePane?.bookingButtonShown == false, "without a Scheduling service there is no Booking link")
            dismissComposePane()
            close()
            return
        }

        // Manage accounts, not connected.
        let alert = alertWindow()
        let entry = alert.accessoryView as? SchedulingAccountEntry
        expect(entry != nil, "Manage accounts has a Scheduling entry")
        expect(entryTexts(alert) == [connection.name, "Not connected", "·", "Connect"], "not connected: name, Not connected · Connect (\(entryTexts(alert)))")
        expect(connection.name == "Scheduling by YCorp", "the development profile's service is Scheduling by YCorp")
        expect(alert.buttons.map(\.title) == ["Close", "Remove an account…", "Agent Access…"], "the alert keeps its buttons")
        snapshotAlert(alert, "mac-01-manage-accounts-not-connected")
        let compactHeight = alert.window.frame.height

        let sched = paletteTitles("sched")
        expect(sched.first == "Connect Scheduling…", "palette: \"sched\" starts with Connect Scheduling… (\(sched))")
        expect(!paletteTitles("").contains("Scheduling"), "palette: no Scheduling command before connecting")
        snapshotPalette("sched", "mac-07-palette-sched-not-connected")

        // Waiting for the browser: the same alert follows and grows.
        connection.setForChecks(.waiting(code: "WDJB-MJHT"))
        pump(0.2)
        expect(entryTexts(alert) == [connection.name, "Waiting for your browser", "·", "Cancel", "Code if asked: WDJB-MJHT"], "waiting: code and Cancel (\(entryTexts(alert)))")
        expect(alert.window.frame.height > compactHeight, "the open alert grows for the extra line")
        snapshotAlert(alert, "mac-02-manage-accounts-waiting")
        // The links work like buttons for VoiceOver and the keyboard. Cancel
        // stays local, so pressing it here contacts nothing.
        func links(_ view: NSView) -> [NSTextField] {
            ((view as? NSTextField).flatMap { $0.accessibilityRole() == .link ? [$0] : nil } ?? []) + view.subviews.flatMap(links)
        }
        let cancel = links(alert.accessoryView!).first { $0.stringValue == "Cancel" }
        expect(cancel?.acceptsFirstResponder == true && cancel?.accessibilityLabel() == "Stop connecting \(connection.name)", "Cancel is a link VoiceOver and the keyboard can use")
        expect(cancel?.accessibilityPerformPress() == true && connection.state == .disconnected, "pressing Cancel stops waiting")
        connection.setForChecks(.waiting(code: "WDJB-MJHT"))
        let waitingBook = paletteTitles("book")
        expect(!connection.isConnected && waitingBook.first == "Connect Scheduling…" && !waitingBook.contains("Bookings and meeting links"), "waiting: still no booking commands (\(waitingBook))")

        // A problem shows under the entry.
        connection.setForChecks(.disconnected, problem: "The connection expired. Connect to try again.")
        pump(0.2)
        expect(entryTexts(alert).last == "The connection expired. Connect to try again.", "a failure shows under the entry")
        snapshotAlert(alert, "mac-13-manage-accounts-problem")
        connection.setForChecks(.disconnected)

        // A composer and Calendar opened before connecting.
        showCalendarWorkspace()
        pump(1.5)
        expect(calendarWorkspace != nil && !calendarBookingsShown(), "Calendar: no Bookings and links before connecting")
        snapshotMain("mac-11-calendar-not-connected")
        closeCalendarWorkspace()
        select("demo:launch:elena-intro")
        wait("the meeting request renders again") { rendered("30-minute intro call") }
        reply()
        wait("the reply box opens") { composePane != nil }
        composePane?.checkSetBody("Hi Elena,\n\nHappy to. Pick any time that suits you next week:\n\n")
        expect(composePane?.bookingButtonShown == false, "composer: no Booking link before connecting")
        let composerBook = paletteTitles("book")
        expect(composerBook == ["Connect Scheduling…"], "composer palette: \"book\" offers Connect Scheduling… only (\(composerBook))")
        snapshotMain("mac-05-composer-not-connected")
        snapshotPalette("book", "mac-08-palette-composer-book-not-connected")

        // Connected, while the composer is open.
        connection.setForChecks(.connected(account: "alex@fernhill.example"))
        pump(0.2)
        expect(composePane?.bookingButtonShown == true, "composer: Booking link appears in an open composer")
        let connectedBook = paletteTitles("book")
        expect(connectedBook.prefix(2) == ["Insert booking link…", "Bookings and meeting links"], "composer palette: \"book\" puts Insert booking link… first (\(connectedBook))")
        expect(!connectedBook.contains("Connect Scheduling…"), "composer palette: no Connect Scheduling… once connected")
        snapshotMain("mac-06-composer-connected")
        snapshotPalette("book", "mac-10-palette-composer-book-connected")
        dismissComposePane()
        wait("the composer closes") { composePane == nil }

        let mainBook = paletteTitles("book")
        expect(mainBook.first == "Bookings and meeting links" && mainBook.contains("Scheduling"), "main palette: \"book\" starts with Bookings and meeting links, then Scheduling (\(mainBook))")
        snapshotPalette("book", "mac-09-palette-book-connected")
        expect(entryTexts(alert) == [connection.name, "alex@fernhill.example", "Connected on this Mac", "Disconnect", "·", "Manage online"], "connected: account, Disconnect · Manage online (\(entryTexts(alert)))")
        snapshotAlert(alert, "mac-03-manage-accounts-connected")
        let freshAlert = alertWindow()
        expect(entryTexts(freshAlert) == entryTexts(alert), "a newly opened Manage accounts shows the same entry")

        showCalendarWorkspace()
        pump(1.5)
        expect(calendarBookingsShown(), "Calendar: Bookings and links once connected")
        snapshotMain("mac-12-calendar-connected")
        closeCalendarWorkspace()

        // Disconnecting hides it again, also in an open composer and Calendar.
        select("demo:launch:elena-intro")
        wait("the meeting request renders after Calendar") { rendered("30-minute intro call") }
        reply()
        wait("the reply box opens after connecting") { composePane != nil }
        expect(composePane?.bookingButtonShown == true, "composer: Booking link in a new composer while connected")
        connection.setForChecks(.disconnected)
        pump(0.2)
        expect(composePane?.bookingButtonShown == false, "composer: Booking link goes away on Disconnect")
        expect(entryTexts(alert) == [connection.name, "Not connected", "·", "Connect"], "Manage accounts follows Disconnect")
        expect(paletteTitles("book") == ["Connect Scheduling…"], "composer palette: back to Connect Scheduling…")
        dismissComposePane()
        wait("the composer closes again") { composePane == nil }
        showCalendarWorkspace()
        pump(1)
        expect(!calendarBookingsShown(), "Calendar: Bookings and links goes away on Disconnect")
        closeCalendarWorkspace()
        close()
    }
'''
main = '''import AppKit
@main struct Check { @MainActor static func main() {
    let app = NSApplication.shared; app.setActivationPolicy(.accessory); app.finishLaunching()
    for key in [RemoteImagePreference.enabledKey, RemoteImagePreference.explicitChoiceKey, RemoteImagePreference.migrationKey] { UserDefaults.standard.removeObject(forKey: key) }
    RemoteImagePreference.prepare(.standard)
    let window = MailWindow(); window.checkScheduling()
} }
'''
compose_accessor = '''    var bookingButtonShown: Bool { !booking.isHidden && booking.superview != nil }
    func checkSetBody(_ text: String) { body.string = text }
'''
anchor = '    @objc private func editSelectedDraft() {'
compose_anchor = '    func paletteBooking()'
with tempfile.TemporaryDirectory(prefix='email-scheduling-check-') as temp:
    work = Path(temp)
    text = source.joinpath('EmailUI.swift').read_text()
    assert anchor in text
    (work / 'EmailUI.swift').write_text(text.replace(anchor, check + '\n' + anchor, 1))
    compose = source.joinpath('ComposePane.swift').read_text()
    assert compose_anchor in compose
    (work / 'ComposePane.swift').write_text(compose.replace(compose_anchor, compose_accessor + compose_anchor, 1))
    (work / 'Check.swift').write_text(main)
    files = [str(f) for f in source.glob('*.swift') if f.name not in ('EmailUI.swift', 'ComposePane.swift')]
    binary = work / 'email-scheduling-check'
    subprocess.run(['swiftc', '-parse-as-library', '-swift-version', '6', '-o', str(binary),
                    str(work / 'EmailUI.swift'), str(work / 'ComposePane.swift'), str(work / 'Check.swift'), *files], check=True)
    home = work / 'home'
    home.mkdir()
    env = dict(os.environ, HOME=str(home), EMAIL_STORE_PATH=str(work / 'store' / 'mail.sqlite'), EMAIL_DEMO='launch',
               TZ='Europe/Oslo', MAIL_SERVICE_LIBRARY=str(library), EMAIL_CHECK_APPEARANCE=args.appearance,
               EMAIL_TEST_CREDENTIALS_DIR=str(work / 'credentials'))
    if args.icon:
        env['EMAIL_CHECK_ICON'] = str(args.icon.resolve(strict=True))
    if args.snapshots:
        args.snapshots.mkdir(parents=True, exist_ok=True)
        env['EMAIL_CHECK_SNAPSHOTS'] = str(args.snapshots.resolve())
    subprocess.run([str(binary)], env=env, check=True)
print('PASS: Scheduling opt-in AppKit check')
