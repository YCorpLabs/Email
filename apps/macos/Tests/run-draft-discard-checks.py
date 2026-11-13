#!/usr/bin/env python3
"""Exercise actual AppKit draft discard/undo against a disposable demo store."""
from pathlib import Path
import argparse, os, subprocess, tempfile

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--library', type=Path, required=True, help='Built macOS libmail_service.dylib')
parser.add_argument('--snapshots', type=Path, help='Optional directory for PNG captures of key states')
args = parser.parse_args()
library = args.library.resolve(strict=True)
root = Path(__file__).resolve().parents[3]
source = root / 'apps/macos/Sources/EmailUI'
check = r'''
    func checkDraftDiscard() {
        func expect(_ ok: Bool, _ name: String) { precondition(ok, name); print("PASS \(name)") }
        func key(_ value: String, _ code: UInt16, shift: Bool = false) -> Bool {
            let event = NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: shift ? [.shift] : [], timestamp: 0, windowNumber: windowNumber, context: nil, characters: value, charactersIgnoringModifiers: value, isARepeat: false, keyCode: code)!
            return handle(event)
        }
        func wait(_ name: String, _ condition: () -> Bool) {
            let deadline = Date().addingTimeInterval(5)
            while !condition() && Date() < deadline { RunLoop.main.run(until: Date().addingTimeInterval(0.02)) }
            expect(condition(), name)
        }
        func snapshot(_ name: String) {
            guard let dir = ProcessInfo.processInfo.environment["EMAIL_CHECK_SNAPSHOTS"], let view = contentView else { return }
            RunLoop.main.run(until: Date().addingTimeInterval(0.3))
            view.layoutSubtreeIfNeeded()
            guard let rep = view.bitmapImageRepForCachingDisplay(in: view.bounds) else { return }
            view.cacheDisplay(in: view.bounds, to: rep)
            try? rep.representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: dir).appendingPathComponent(name + ".png"))
        }
        func selectedID() -> String? { filteredDrafts.indices.contains(table.selectedRow) ? filteredDrafts[table.selectedRow].id : nil }
        func hints() -> String { readerKeyboardHint.items.map { "\($0.0) \($0.1)" }.joined(separator: " | ") }
        makeKeyAndOrderFront(nil)
        let accountID = knownAccounts.first!.id
        for name in ["Alpha", "Bravo", "Charlie"] {
            _ = core!.call(["op": "compose_draft", "account_id": accountID, "to": "reader@example.test", "subject": name, "body_plain": "Body \(name)"])
            Thread.sleep(forTimeInterval: 0.01)
        }
        showDrafts()
        wait("drafts load") { drafts.count == 3 }
        expect(hints().contains("⌫ Discard"), "hint bar names the discard key")
        snapshot("1-drafts")
        expect(key("j", 38) && key("j", 38) && table.selectedRow == 1, "J selects the middle draft")
        let middle = selectedID()!
        let menu = contextMenu(for: 1)
        expect(menu?.items.map(\.title) == ["Edit draft   ↵", "Discard draft   ⌫"], "draft context menu offers edit and discard")
        expect(paletteItems().contains { $0.target == .discardDraft } && paletteItems().contains { $0.target == .editDraft }, "palette offers edit and discard for the selected draft")
        expect(key("\u{7f}", 51), "Delete key handled in Drafts")
        wait("Delete discards the selected draft") { drafts.count == 2 && !draftDiscardBusy && !drafts.contains { $0.id == middle } }
        wait("selection moves to the next draft") { table.selectedRow == 1 && selectedID() != nil }
        expect(core!.call(["op": "get_message", "id": middle]) is NSNull || core!.call(["op": "get_message", "id": middle]) == nil, "discarded draft is gone from the store")
        expect(!triageBar.isHidden && triageCount.stringValue == "Draft discarded" && !triageButtons[3].isHidden && triageButtons[0].isHidden, "undo bar shows only Undo")
        expect(hints().contains("Z Undo discard"), "hint bar offers Z undo")
        expect(paletteItems().firstIndex { $0.target == .undoDiscardDraft }.map { undo in paletteItems().firstIndex { $0.target == .editDraft }.map { undo < $0 } ?? false } == true, "palette suggests Undo discard before draft actions")
        expect(firstResponder === table && composePane == nil, "list keeps focus after discard")
        snapshot("2-after-discard")
        expect(key("z", 6), "Z handled in Drafts")
        wait("Z restores the discarded draft") { drafts.count == 3 && !draftDiscardBusy }
        wait("restored draft is selected") { selectedID() == middle }
        expect(triageBar.isHidden && lastDiscardedDraft == nil, "undo bar hides after restore")
        expect(key("#", 20, shift: true), "# handled in Drafts")
        wait("# discards too") { drafts.count == 2 && !draftDiscardBusy }
        expect(key("z", 6), "second undo handled")
        wait("second undo restores") { drafts.count == 3 && !draftDiscardBusy }
        // Composer discard saves on-screen edits first so undo restores them.
        expect(key("\r", 36) && composePane != nil, "Enter edits the selected draft")
        let editing = selectedID()!
        snapshot("3-composer")
        composePane!.setValue("Edited before discard", forKeyPath: "body.string")
        composePane!.paletteDiscard()
        wait("composer discard closes the editor") { composePane == nil && drafts.count == 2 }
        expect(lastDiscardedDraft?.id == editing, "composer discard is undoable")
        expect(key("z", 6), "undo after composer discard")
        wait("composer discard restores edited text") { drafts.contains { $0.id == editing && $0.bodyPlain == "Edited before discard" } }
        // An untouched new message has nothing to restore and just closes.
        compose()
        expect(composePane != nil, "new message opens")
        composePane!.paletteDiscard()
        wait("untouched new message closes without a draft") { composePane == nil }
        RunLoop.main.run(until: Date().addingTimeInterval(0.2))
        expect((core!.call(["op": "list_drafts"]) as? [[String: Any]])?.count == 3, "untouched discard creates no draft")
        setContentSize(NSSize(width: 760, height: 500))
        expect(key("j", 38) && key("\u{f728}", 117), "narrow layout forward delete handled")
        wait("narrow layout discard") { drafts.count == 2 && !draftDiscardBusy }
        expect(!triageBar.isHidden, "narrow layout undo bar visible")
        snapshot("4-narrow-after-discard")
        close()
    }
'''
with tempfile.TemporaryDirectory(prefix='email-draft-discard-') as temp:
    p = Path(temp)
    ui = p / 'EmailUI.swift'
    ui.write_text(source.joinpath('EmailUI.swift').read_text().replace('    @objc private func editSelectedDraft() {', check + '\n    @objc private func editSelectedDraft() {'))
    compose = p / 'ComposePane.swift'
    # Expose the body for the fixture's on-screen edit (KVC on a private Swift property is unavailable).
    compose_source = source.joinpath('ComposePane.swift').read_text()
    compose_source = compose_source.replace('    func paletteDiscard() { discardClicked() }', '    func paletteDiscard() { discardClicked() }\n    override func setValue(_ value: Any?, forKeyPath keyPath: String) { if keyPath == "body.string", let text = value as? String { body.string = text } else { super.setValue(value, forKeyPath: keyPath) } }')
    compose.write_text(compose_source)
    main = p / 'Check.swift'
    main.write_text('import AppKit\n@main struct Check { @MainActor static func main() { let app = NSApplication.shared; app.setActivationPolicy(.accessory); app.finishLaunching(); let window = MailWindow(); window.checkDraftDiscard() } }')
    files = [str(f) for f in source.glob('*.swift') if f.name not in ('EmailUI.swift', 'ComposePane.swift')]
    subprocess.run(['swiftc', '-parse-as-library', '-swift-version', '6', '-o', str(p / 'check'), str(ui), str(compose), str(main), *files], check=True)
    env = dict(os.environ, EMAIL_STORE_PATH=str(p / 'mail.sqlite'), EMAIL_DEMO='1', MAIL_SERVICE_LIBRARY=str(library))
    if args.snapshots:
        args.snapshots.mkdir(parents=True, exist_ok=True)
        env['EMAIL_CHECK_SNAPSHOTS'] = str(args.snapshots.resolve())
    subprocess.run([str(p / 'check')], env=env, check=True)
