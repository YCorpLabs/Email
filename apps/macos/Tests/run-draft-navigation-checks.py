#!/usr/bin/env python3
from pathlib import Path
import os, subprocess, tempfile
import argparse
parser=argparse.ArgumentParser(description='Exercise actual AppKit draft navigation with a disposable demo store.')
parser.add_argument('--library', type=Path, required=True, help='Built macOS libmail_service.dylib')
args=parser.parse_args()
library=args.library.resolve(strict=True)
root=Path(__file__).resolve().parents[3]
source=root/'apps/macos/Sources/EmailUI'
check=r'''
    func checkDraftNavigation() {
        func expect(_ ok: Bool, _ name: String) { precondition(ok, name); print("PASS \(name)") }
        func key(_ value: String, _ code: UInt16) -> Bool {
            let event = NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: [], timestamp: 0, windowNumber: windowNumber, context: nil, characters: value, charactersIgnoringModifiers: value, isARepeat: false, keyCode: code)!
            return handle(event)
        }
        makeKeyAndOrderFront(nil)
        folder = .drafts
        selection = -1
        let accountID = knownAccounts.first!.id
        drafts = (0..<3).map { MailDraft(id: "fixture-\($0)", accountID: accountID, from: "", to: "reader@example.test", cc: "", bcc: "", subject: "Draft \($0)", bodyPlain: "Body \($0)") }
        table.reloadData()
        table.deselectAll(nil)
        makeFirstResponder(table)
        expect(key("j", 38), "first J handled")
        expect(table.selectedRow == 0 && selection == 0, "first J selects first draft")
        expect(composePane == nil && firstResponder === table, "selection keeps list focus")
        expect(bodyText.string == "Body 0" && currentHTML?.contains("Body 0") == true, "draft body preview uses the mail renderer")
        expect(key("j", 38) && table.selectedRow == 1, "second J advances")
        expect(key("k", 40) && table.selectedRow == 0, "K goes back")
        expect(key("k", 40) && table.selectedRow == 0, "upper boundary")
        expect(key("j", 38) && key("j", 38) && key("j", 38) && table.selectedRow == 2, "lower boundary")
        expect(key("\r", 36) && composePane != nil, "Enter edits selected draft")
        expect((firstResponder as? NSTextView)?.isFieldEditor == false, "addressed draft opens in the message body, not To")
        expect(!key("j", 38) && table.selectedRow == 2, "composer preserves typing keys")
        composePane!.focusRecipient(in: self)
        let toEditor = firstResponder as? NSTextView
        expect(toEditor?.isFieldEditor == true && toEditor?.selectedRange() == NSRange(location: ("reader@example.test" as NSString).length, length: 0), "To focus places the caret at the end without selecting the address")
        expect(key("\u{1b}", 53) && composePane == nil && firstResponder === table, "Escape returns to list")
        expect(key("k", 40) && table.selectedRow == 1, "navigation after editor dismissal")
        editSelectedDraft()
        expect(composePane != nil, "double-click action edits draft")
        dismissComposePane()
        setContentSize(NSSize(width: 760, height: 500))
        expect(key("j", 38) && table.selectedRow == 2 && composePane == nil, "narrow layout navigation")
        close()
    }
'''
with tempfile.TemporaryDirectory(prefix='email-draft-check-') as temp:
    p=Path(temp)
    ui=p/'EmailUI.swift'
    ui.write_text(source.joinpath('EmailUI.swift').read_text().replace('    @objc private func editSelectedDraft() {',check+'\n    @objc private func editSelectedDraft() {'))
    main=p/'Check.swift'
    main.write_text('import AppKit\n@main struct Check { @MainActor static func main() { let app = NSApplication.shared; app.setActivationPolicy(.accessory); app.finishLaunching(); let window = MailWindow(); window.checkDraftNavigation() } }')
    args=['swiftc','-parse-as-library','-swift-version','6','-o',str(p/'check'),str(ui),str(main),*[str(f) for f in source.glob('*.swift') if f.name!='EmailUI.swift']]
    subprocess.run(args,check=True)
    env=dict(os.environ,EMAIL_STORE_PATH=str(p/'mail.sqlite'),EMAIL_DEMO='1',MAIL_SERVICE_LIBRARY=str(library))
    subprocess.run([str(p/'check')],env=env,check=True)
