#!/usr/bin/env python3
"""Open the launch demo mailbox in the actual AppKit window and capture it.

Compiles the real EmailUI sources with a check entry point, seeds a disposable
store with EMAIL_DEMO=launch through the real mail-service library, then walks
the states used in the launch recordings: the inbox, a long conversation, an
inline picture, the blocked remote images notice, an invitation, attachments,
search, a split, dark mode and a narrow window. With --snapshots it writes a
PNG of each state, including the reader's WebKit content.

The store and HOME are temporary. No real account, Keychain item or provider
is used, and nothing is sent.
"""
from pathlib import Path
import argparse, os, subprocess, sys, tempfile

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--library', type=Path, required=True, help='libmail_service.dylib, e.g. target/debug/libmail_service.dylib')
parser.add_argument('--snapshots', type=Path, help='Optional directory for PNG captures of each state')
args = parser.parse_args()
library = args.library.resolve(strict=True)
root = Path(__file__).resolve().parents[3]
source = root / 'apps/macos/Sources/EmailUI'

check = r'''
    var checkResult = ""
    var checkDone = false
    var checkImage: NSImage?
    func checkLaunchDemo() {
        let environment = ProcessInfo.processInfo.environment
        func expect(_ ok: Bool, _ name: String) { precondition(ok, name); print("PASS \(name)"); fflush(stdout) }
        func pump(_ seconds: Double = 0.05) { RunLoop.main.run(until: Date().addingTimeInterval(seconds)) }
        func wait(_ name: String, _ condition: () -> Bool) {
            let deadline = Date().addingTimeInterval(10)
            while !condition() && Date() < deadline { pump(0.02) }
            expect(condition(), name)
        }
        func js(_ script: String) -> String {
            checkDone = false; checkResult = ""
            Task { @MainActor in
                let value = try? await self.webView?.evaluateJavaScript(script)
                self.checkResult = value.map { String(describing: $0) } ?? ""
                self.checkDone = true
            }
            let deadline = Date().addingTimeInterval(5)
            while !checkDone && Date() < deadline { pump(0.02) }
            return checkResult
        }
        func write(_ rep: NSBitmapImageRep, _ name: String) {
            guard let directory = environment["EMAIL_CHECK_SNAPSHOTS"] else { return }
            try? rep.representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: directory).appendingPathComponent("macos-\(name).png"))
        }
        func snapshot(_ name: String) {
            guard environment["EMAIL_CHECK_SNAPSHOTS"] != nil, let view = contentView else { return }
            pump(0.5)
            view.layoutSubtreeIfNeeded()
            guard let rep = view.bitmapImageRepForCachingDisplay(in: view.bounds) else { return }
            view.cacheDisplay(in: view.bounds, to: rep)
            // WKWebView draws out of process; composite its own snapshot.
            if let webView, !webView.isHidden, composePane == nil, calendarWorkspace == nil {
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
        func snapshotView(_ view: NSView, _ name: String) {
            pump(0.5)
            view.layoutSubtreeIfNeeded()
            guard let rep = view.bitmapImageRepForCachingDisplay(in: view.bounds) else { return }
            view.cacheDisplay(in: view.bounds, to: rep)
            write(rep, name)
        }
        func select(_ id: String) {
            guard let row = threads.firstIndex(where: { $0.id == id }) else { preconditionFailure("missing demo conversation \(id)") }
            table.selectRowIndexes(IndexSet(integer: row), byExtendingSelection: false)
            table.scrollRowToVisible(row)
        }
        func rendered(_ text: String) -> Bool { currentHTML?.contains(text) == true && currentWebDidCommit && webView?.isHidden == false }
        func attachment(_ name: String) -> Bool { currentAttachments.contains { $0["filename"] as? String == name } }

        makeKeyAndOrderFront(nil)
        setContentSize(NSSize(width: 1440, height: 900))
        appearance = NSAppearance(named: .aqua); applyTheme()
        wait("the launch demo inbox loads") { threads.count >= 60 }
        expect(knownAccounts.count == 2, "two demo accounts")
        expect(Set(splits.map(\.name)) == ["Team", "Notifications", "Newsletters", "Promotions"], "four demo splits")
        expect(threads.first?.subject == "Intro call next week?", "the newest conversation is the meeting request")
        expect(threads.prefix(12).filter(\.unread).count >= 5, "the top of the inbox has unread mail")
        expect(threads.prefix(8).allSatisfy { $0.sender != "Alex Morgan" && !$0.subject.hasPrefix("Accepted:") }, "the top of the inbox shows other people's mail, not drafts or RSVPs")
        pump(3)
        expect(!syncStatus.stringValue.contains("failed"), "demo accounts do not report a sync failure")

        select("demo:launch:elena-intro")
        wait("the meeting request renders") { rendered("30-minute intro call") }
        snapshot("01-inbox")

        select("demo:launch:launch-plan")
        wait("the launch thread renders all six messages") { rendered("we ship on") && readerMessageCount == 6 }
        snapshot("02-thread")

        select("demo:launch:onboarding-v3")
        wait("the onboarding thread renders") { rendered("Skip for now") }
        wait("its inline picture resolves from the demo source") { js("(document.querySelector('img[alt=\"Updated empty state\"]') || {}).naturalWidth") == "640" }
        wait("its attached screen is listed") { attachment("onboarding-v3-welcome.png") }
        snapshot("03-inline-picture")

        select("demo:launch:quiet-0")
        wait("the newsletter renders") { rendered("Designing for the second visit") }
        wait("remote images are blocked with a notice") { !remoteImagesNotice.isHidden }
        expect(js("document.querySelector('img[src^=\"https://img.quietinterface.example\"]').naturalWidth") == "0", "remote newsletter images stay blocked")
        snapshot("04-blocked-images")

        select("demo:launch:invite-go-no-go")
        wait("the invitation renders") { rendered("Launch go/no-go") }
        wait("its invite.ics is listed") { attachment("invite.ics") }
        snapshot("05-invite")
        openWorkflow(calendar: true)
        if let view = workflowPopover?.contentViewController?.view {
            pump(1.5)
            snapshotView(view, "06-invitations-panel")
        }
        workflowPopover?.close()

        select("demo:launch:brightwater-pilot")
        wait("the pilot thread renders") { rendered("six-week pilot") }
        wait("its PDF is listed") { attachment("Brightwater-pilot-summary.pdf") }
        snapshot("07-attachment")

        select("demo:launch:hyttetur")
        wait("the Norwegian thread renders") { rendered("Hyttetur") || rendered("hytta") }
        wait("its photos are listed") { attachment("hytta-utsikt.png") && attachment("nordlys.png") }
        wait("its photos show in the message") { js("(document.querySelector('img[alt=\"Nordlys over fjellet\"]') || {}).naturalWidth") == "640" }
        snapshot("08-norwegian-photos")

        makeFirstResponder(search)
        search.stringValue = "invoice"
        refreshInbox()
        wait("search finds the demo invoices") { threads.count >= 6 && threads.allSatisfy { $0.id?.hasPrefix("demo:launch:") == true } && threads.contains { $0.subject.contains("Cirrusbay") } }
        snapshot("09-search-invoice")
        search.stringValue = ""
        refreshInbox()

        if let split = splits.first(where: { $0.name == "Newsletters" }) {
            showInbox(scope: .split(split.id))
            wait("the Newsletters split lists newsletters") { !threads.isEmpty && threads.allSatisfy { $0.labels.contains("Newsletters") } }
            snapshot("10-split-newsletters")
        }
        showInbox(scope: .pinned)
        wait("favorites list the starred demo threads") { threads.count >= 5 }
        showInbox(scope: .snoozed)
        wait("snoozed lists the demo snoozes") { threads.count >= 4 }
        showInbox(scope: .sent)
        wait("sent lists the demo sent mail") { threads.count >= 15 }
        snapshot("11-sent")
        showInbox(scope: .all)
        wait("the inbox returns") { threads.first?.subject == "Intro call next week?" }

        appearance = NSAppearance(named: .darkAqua); applyTheme()
        select("demo:launch:elena-intro")
        wait("the dark reader renders") { rendered("30-minute intro call") }
        snapshot("12-inbox-dark")
        appearance = NSAppearance(named: .aqua); applyTheme()

        setContentSize(NSSize(width: 980, height: 700))
        select("demo:launch:launch-plan")
        wait("the narrow reader renders") { rendered("we ship on") }
        snapshot("13-narrow")
        close()
    }
'''
main = '''import AppKit
@main struct Check { @MainActor static func main() {
    let app = NSApplication.shared; app.setActivationPolicy(.accessory); app.finishLaunching()
    for key in [RemoteImagePreference.enabledKey, RemoteImagePreference.explicitChoiceKey, RemoteImagePreference.migrationKey] { UserDefaults.standard.removeObject(forKey: key) }
    RemoteImagePreference.prepare(.standard)
    let window = MailWindow(); window.checkLaunchDemo()
} }
'''
anchor = '    @objc private func editSelectedDraft() {'
with tempfile.TemporaryDirectory(prefix='email-launch-demo-') as temp:
    work = Path(temp)
    text = source.joinpath('EmailUI.swift').read_text()
    assert anchor in text
    ui = work / 'EmailUI.swift'
    ui.write_text(text.replace(anchor, check + '\n' + anchor, 1))
    (work / 'Check.swift').write_text(main)
    files = [str(f) for f in source.glob('*.swift') if f.name != 'EmailUI.swift']
    binary = work / 'email-launch-demo-check'
    subprocess.run(['swiftc', '-parse-as-library', '-swift-version', '6', '-o', str(binary),
                    str(ui), str(work / 'Check.swift'), *files], check=True)
    store = work / 'store' / 'mail.sqlite'
    home = work / 'home'
    home.mkdir()
    # A fixed time zone: demo dates and invitations follow it.
    env = dict(os.environ, HOME=str(home), EMAIL_STORE_PATH=str(store), EMAIL_DEMO='launch', TZ='Europe/Oslo',
               MAIL_SERVICE_LIBRARY=str(library))
    if args.snapshots:
        args.snapshots.mkdir(parents=True, exist_ok=True)
        env['EMAIL_CHECK_SNAPSHOTS'] = str(args.snapshots.resolve())
    subprocess.run([str(binary)], env=env, check=True)
print('PASS: launch demo AppKit check')
