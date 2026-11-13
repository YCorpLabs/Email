#!/usr/bin/env python3
"""Exercise actual AppKit remote-image blocking and profile-gated open tracking.

Compiles the real EmailUI sources once and drives them against two mail-service
libraries: one built with the `official` distribution profile (Scheduling on,
OPEN_TRACKING=false, the public case) and one with `development`
(OPEN_TRACKING=true). The shell reads open tracking from the engine's profile,
so the library alone selects the variant. Stores, HOME and the preferences
domain are disposable; the installed app, its Keychain items and real accounts
are never used and nothing is sent.
"""
from pathlib import Path
import argparse, os, shutil, subprocess, sys, tempfile

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--library', type=Path, required=True, help='libmail_service.dylib built with EMAIL_DISTRIBUTION_PROFILE=official (public case)')
parser.add_argument('--dev-library', type=Path, help='libmail_service.dylib built with EMAIL_DISTRIBUTION_PROFILE=development; required for the dev variant')
parser.add_argument('--snapshots', type=Path, help='Optional directory for PNG captures of key states')
parser.add_argument('--variant', choices=('public', 'dev', 'both'), default='both')
args = parser.parse_args()
library = args.library.resolve(strict=True)
if args.variant in ('dev', 'both') and not args.dev_library:
    parser.error('--dev-library is required so the dev variant runs against the service it ships with')
dev_library = args.dev_library.resolve(strict=True) if args.dev_library else None
root = Path(__file__).resolve().parents[3]
source = root / 'apps/macos/Sources/EmailUI'
seeder = root / 'scripts/seed-reader-privacy-fixture.py'
import importlib.util
_spec = importlib.util.spec_from_file_location('reader_privacy_fixture', seeder)
fixture = importlib.util.module_from_spec(_spec); _spec.loader.exec_module(fixture)
check = r'''
    var checkResult = ""
    var checkDone = false
    var checkImage: NSImage?
    func checkReaderPrivacy() {
        let environment = ProcessInfo.processInfo.environment
        let openTracking = DistributionProfile.current?.openTracking == true
        let variant = openTracking ? "dev" : "public"
        let tracked = environment["EMAIL_CHECK_TRACKED"] == "1"
        func expect(_ ok: Bool, _ name: String) { precondition(ok, "\(variant): \(name)"); print("PASS [\(variant)\(tracked ? " tracked" : "")] \(name)"); fflush(stdout) }
        func pump(_ seconds: Double = 0.05) { RunLoop.main.run(until: Date().addingTimeInterval(seconds)) }
        func wait(_ name: String, _ condition: () -> Bool) {
            let deadline = Date().addingTimeInterval(8)
            while !condition() && Date() < deadline { pump(0.02) }
            expect(condition(), name)
        }
        func key(_ value: String, _ code: UInt16) -> Bool {
            let event = NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: [], timestamp: 0, windowNumber: windowNumber, context: nil, characters: value, charactersIgnoringModifiers: value, isARepeat: false, keyCode: code)!
            return handle(event)
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
        func policy() -> String { js("document.querySelector('meta[http-equiv=\"Content-Security-Policy\"]').content") }
        func snapshot(_ name: String) {
            guard let directory = environment["EMAIL_CHECK_SNAPSHOTS"], let view = contentView else { return }
            pump(0.4)
            view.layoutSubtreeIfNeeded()
            guard let rep = view.bitmapImageRepForCachingDisplay(in: view.bounds) else { return }
            view.cacheDisplay(in: view.bounds, to: rep)
            // WKWebView draws out of process; composite its own snapshot.
            if let webView, !webView.isHidden, composePane == nil {
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
            try? rep.representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: directory).appendingPathComponent("\(variant)\(tracked ? "-tracked" : "")-\(name).png"))
        }
        func select(_ subject: String) {
            guard let row = threads.firstIndex(where: { $0.subject == subject }) else { preconditionFailure("missing fixture \(subject)") }
            table.selectRowIndexes(IndexSet(integer: row), byExtendingSelection: false)
        }
        let defaults = UserDefaults.standard
        makeKeyAndOrderFront(nil)
        setContentSize(NSSize(width: 1280, height: 800))
        appearance = NSAppearance(named: .aqua); applyTheme()
        expect(!RemoteImagePreference.isEnabled(defaults), "remote images are blocked by default")
        expect(remoteImagesNotice.isHidden, "notice starts hidden")
        let available = (core?.call(["op": "tracking_get", "message_id": "demo:1"]) as? [String: Any])?["available"] as? Bool
        expect(variant == environment["EMAIL_CHECK_VARIANT"], "the library carries the intended distribution profile")
        expect(DistributionProfile.current?.hasScheduling == true, "the profile has a Scheduling service")
        expect(available == openTracking, "the engine's tracking availability matches its profile")

        if !tracked {
            select("Reader privacy fixture")
            wait("fixture newsletter renders") { currentHTML?.contains("Remote hero image") == true && currentWebDidCommit && webView?.isHidden == false }
            expect(!remoteImagesButton.isHidden, "image control is shown for remote images")
            expect(!remoteImagesNotice.isHidden && remoteImagesNoticeButton.title == "Load images", "blocked notice offers Load images")
            expect(remoteImagesNotice.frame.maxY <= account.frame.minY && readerBody.frame.maxY <= remoteImagesNotice.frame.minY, "notice sits between header and message")
            expect(policy().contains("img-src data:;"), "blocked document allows only inline images")
            // Resolve the cid: image the way the bounded inliner does (demo mail
            // has no provider attachments), through the reader's own update path.
            func resolveInline() {
                pump(1.5) // let the reader's own inline-image attempt finish first
                let before = currentHTML!
                let after = before.replacingOccurrences(of: "cid:logo@fixture.example", with: environment["EMAIL_CHECK_INLINE_DATA"]!)
                expect(after != before, "fixture keeps its cid: image through the sanitizer")
                currentHTML = after
                pendingInlineImageSources = Self.inlineImageSources(before: before, after: after)
                applyPendingInlineImages()
                wait("bounded cid: image renders while remote images are blocked") { js("document.querySelector('img[alt=\"Inline logo\"]').naturalWidth") == "24" }
            }
            resolveInline()
            expect(js("document.querySelector('img[alt=\"Remote hero image\"]').naturalWidth") == "0", "remote image stays blocked")
            expect(!RemoteImagePreference.containsRemoteImages(currentHTML!.replacingOccurrences(of: "https://images.example", with: "cid:x")), "inline data: images do not count as remote")
            snapshot("1-blocked-light")
            appearance = NSAppearance(named: .darkAqua); applyTheme()
            wait("dark reader keeps the notice") { !remoteImagesNotice.isHidden && currentWebDidCommit }
            snapshot("2-blocked-dark")
            appearance = NSAppearance(named: .aqua); applyTheme()
            wait("light reader reloads") { currentWebDidCommit }

            makeFirstResponder(table)
            remoteImagesNoticeButton.performClick(nil)
            wait("Load images reloads with remote images allowed") { policy().contains("img-src data: https:;") }
            wait("inline image still renders after loading remote images") { js("document.querySelector('img[alt=\"Inline logo\"]').naturalWidth") == "24" }
            expect(remoteImagesNotice.isHidden, "notice hides after loading this email")
            expect(firstResponder === table, "keyboard focus stays in the message list")
            expect(!RemoteImagePreference.isEnabled(defaults), "loading one email keeps the blocking default")
            expect(remoteImagesButton.accessibilityLabel() == "Remote images allowed", "image control reports allowed")
            snapshot("3-loaded")

            select("Design review on Friday")
            wait("another email renders") { currentHTML?.contains("Remote hero image") == false && currentWebDidCommit }
            expect(remoteImagesNotice.isHidden && remoteImagesButton.isHidden, "no notice for mail without remote images")
            select("Reader privacy fixture")
            wait("returning blocks again") { currentHTML?.contains("Remote hero image") == true && currentWebDidCommit && !remoteImagesNotice.isHidden }
            resolveInline()

            // With Full Keyboard Access a click or Tab leaves the button focused.
            let buttonTookFocus = makeFirstResponder(remoteImagesNoticeButton)
            remoteImagesNoticeButton.performClick(nil)
            wait("Load images from a focused button reloads") { remoteImagesNotice.isHidden && policy().contains("https:") }
            if buttonTookFocus {
                expect(firstResponder === table, "focus returns to the message list, not the hidden button")
            } else {
                // Without Full Keyboard Access the button never takes focus; this
                // run cannot exercise the return path, so say so instead of passing.
                print("SKIP [\(variant)] focus return: Full Keyboard Access is off, the button cannot take focus"); fflush(stdout)
            }
            select("Design review on Friday")
            wait("another email renders again") { currentHTML?.contains("Remote hero image") == false && currentWebDidCommit }
            select("Reader privacy fixture")
            wait("returning blocks once more") { currentHTML?.contains("Remote hero image") == true && currentWebDidCommit && !remoteImagesNotice.isHidden }
            resolveInline()

            toggleImagesForMessage(NSMenuItem())
            wait("menu: load images in this email") { remoteImagesNotice.isHidden && policy().contains("https:") }
            toggleImagesForMessage(NSMenuItem())
            wait("menu: block images in this email") { !remoteImagesNotice.isHidden && policy().contains("img-src data:;") }

            makeFirstResponder(table)
            expect(key("i", 34), "I is handled in the list")
            expect(RemoteImagePreference.isEnabled(defaults) && defaults.bool(forKey: RemoteImagePreference.explicitChoiceKey), "I remembers loading by default as a choice")
            wait("I reloads with remote images") { remoteImagesNotice.isHidden && policy().contains("https:") }
            expect(paletteItems().contains { $0.title == "Block remote images by default" }, "palette offers blocking by default")
            expect(key("i", 34) && !RemoteImagePreference.isEnabled(defaults), "I toggles back to blocking")
            wait("blocking by default shows the notice") { !remoteImagesNotice.isHidden && policy().contains("img-src data:;") }
            toggleImagesGloballyClicked(NSMenuItem())
            expect(RemoteImagePreference.isEnabled(defaults), "menu: load remote images by default")
            toggleImagesGloballyClicked(NSMenuItem())
            expect(!RemoteImagePreference.isEnabled(defaults), "menu: block remote images by default")
            wait("notice returns") { !remoteImagesNotice.isHidden }

            for width in [900.0, 760.0] {
                setContentSize(NSSize(width: width, height: 560))
                contentView?.layoutSubtreeIfNeeded()
                pump(0.3)
                let button = remoteImagesNoticeButton.convert(remoteImagesNoticeButton.bounds, to: remoteImagesNotice)
                expect(!remoteImagesNotice.isHidden && remoteImagesNotice.bounds.contains(button) && button.width >= remoteImagesNoticeButton.intrinsicContentSize.width - 1, "Load images stays whole at \(Int(width)) points")
                expect(remoteImagesNotice.frame.maxX <= (remoteImagesNotice.superview?.bounds.maxX ?? 0), "notice fits the reader at \(Int(width)) points")
                snapshot("4-narrow-\(Int(width))")
            }
            setContentSize(NSSize(width: 1280, height: 800))
        }

        // Open tracking follows the build.
        compose()
        expect(composePane != nil, "composer opens")
        pump(0.5)
        expect(composePane!.trackingOptionShown == openTracking && composePane!.offersTracking == openTracking, "Track opens appears only with the development profile")
        expect(paletteItems().contains { $0.target == .toggleTracking } == openTracking, "palette tracking toggle appears only with the development profile")
        snapshot("5-composer")
        composePane!.paletteDiscard()
        wait("untouched composer closes") { composePane == nil }
        let savedActivity = (core?.call(["op": "tracking_capability"]) as? [String: Any])?["saved_activity"] as? Bool
        expect(savedActivity == tracked, "the service reports saved activity only with tracked sent mail")
        wait("saved-activity check settles") { offersOpenActivity == (openTracking || tracked) }
        expect(paletteItems().contains { $0.target == .trackingActivity } == (openTracking || tracked), "Recent opens appears with the development profile or tracked mail")

        if tracked {
            // Saved counts only: keep the contact pane (which refreshes from the
            // hosted service) closed. Fixture tracking IDs are not UUIDs, so any
            // hosted refresh fails closed before a credential is read.
            contactPanePreference = false; updateContactPaneVisibility()
            currentMessageID = "demo:tracked-sent"
            wait("tracked sent mail keeps its open count") { !trackingActivityButton.isHidden && trackingActivityButton.title == " 3" }
            currentMessageID = nil
            showDrafts()
            wait("tracked draft is listed") { filteredDrafts.contains { $0.subject == "Tracked draft (fixture)" } }
            let row = filteredDrafts.firstIndex { $0.subject == "Tracked draft (fixture)" }!
            table.selectRowIndexes(IndexSet(integer: row), byExtendingSelection: false)
            makeFirstResponder(table)
            expect(key("\r", 36) && composePane != nil, "Enter edits the tracked draft")
            composePane!.paletteSaveDraft()
            func trackedDraft() -> Bool { ((core?.call(["op": "tracking_get", "message_id": "demo:tracked-draft"]) as? [String: Any])?["enabled"] as? Bool) == true }
            pump(1.5)
            expect(trackedDraft(), openTracking ? "development profile keeps a draft's tracking on save" : "public composer makes no tracking call; the service drops it after an accepted send")
        }
        close()
    }
'''
compose_accessor = '    var trackingOptionShown: Bool { tracking.superview != nil }\n'
main = '''import AppKit
@main struct Check { @MainActor static func main() {
    if CommandLine.arguments.contains("--prepare") { _ = CoreBridge()?.call(["op": "seed_demo"]); return }
    let app = NSApplication.shared; app.setActivationPolicy(.accessory); app.finishLaunching()
    for key in [RemoteImagePreference.enabledKey, RemoteImagePreference.explicitChoiceKey, RemoteImagePreference.migrationKey] { UserDefaults.standard.removeObject(forKey: key) }
    RemoteImagePreference.prepare(.standard)
    let window = MailWindow(); window.checkReaderPrivacy()
    for key in [RemoteImagePreference.enabledKey, RemoteImagePreference.explicitChoiceKey, RemoteImagePreference.migrationKey] { UserDefaults.standard.removeObject(forKey: key) }
} }
'''
variants = ('public', 'dev') if args.variant == 'both' else (args.variant,)
with tempfile.TemporaryDirectory(prefix='email-reader-privacy-') as temp:
    work = Path(temp)
    ui = work / 'EmailUI.swift'
    ui.write_text(source.joinpath('EmailUI.swift').read_text().replace('    @objc private func editSelectedDraft() {', check + '\n    @objc private func editSelectedDraft() {', 1))
    pane = work / 'ComposePane.swift'
    pane_source = source.joinpath('ComposePane.swift').read_text()
    assert '    func paletteBooking()' in pane_source
    pane.write_text(pane_source.replace('    func paletteBooking()', compose_accessor + '    func paletteBooking()', 1))
    (work / 'Check.swift').write_text(main)
    files = [str(f) for f in source.glob('*.swift') if f.name not in ('EmailUI.swift', 'ComposePane.swift')]
    if args.snapshots:
        args.snapshots.mkdir(parents=True, exist_ok=True)
    compiled = work / 'email-reader-privacy'
    subprocess.run(['swiftc', '-parse-as-library', '-swift-version', '6', '-o', str(compiled),
                    str(ui), str(pane), str(work / 'Check.swift'), *files], check=True)
    for variant in variants:
        # A distinct executable name gives each variant its own preferences domain.
        binary = work / f'email-reader-privacy-{variant}'
        shutil.copy2(compiled, binary)
        for tracked in (False, True):
            store = work / f'{variant}-{"tracked" if tracked else "plain"}' / 'mail.sqlite'
            store.parent.mkdir()
            home = store.parent / 'home'
            home.mkdir()
            # A disposable HOME keeps credential coordination files out of the real one.
            env = dict(os.environ, HOME=str(home), EMAIL_STORE_PATH=str(store),
                       MAIL_SERVICE_LIBRARY=str(dev_library if variant == 'dev' else library),
                       EMAIL_CHECK_VARIANT=variant,
                       EMAIL_CHECK_INLINE_DATA='data:image/png;base64,' + fixture.INLINE_PNG)
            env.pop('EMAIL_DEMO', None)
            subprocess.run([str(binary), '--prepare'], env=env, check=True)
            subprocess.run([sys.executable, str(seeder), '--store', str(store), *(['--tracked'] if tracked else [])], check=True)
            if args.snapshots:
                env['EMAIL_CHECK_SNAPSHOTS'] = str(args.snapshots.resolve())
            if tracked:
                env['EMAIL_CHECK_TRACKED'] = '1'
            subprocess.run([str(binary)], env=env, check=True)
print('Reader privacy checks passed for: ' + ', '.join(variants))
