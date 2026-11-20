#!/usr/bin/env python3
"""Exercise Filters in the real AppKit main window, without showing a window.

Compiles the real EmailUI sources with a check entry point. The main window
and its sheets stay off screen (transparent, far outside every display) and
are captured with cacheDisplay, so the person using the Mac sees nothing.

Two runs, each on a disposable store:

* setup: a fresh launch demo with filters off. The sidebar offers Set up
  filters; the setup sheet lists the starters; Escape closes it; Return turns
  filters on with a changed action. With --models (an installed model), the
  real model then sorts the mailbox on this Mac and the check waits for it.
  With --real-download and an empty models directory, the real download
  starts, shows progress in the sidebar and is cancelled after a few percent.
* sorted: a copy of --sorted-store (filters on, mail already sorted). The
  filter view, the reader strip, Move to inbox and Move to, palette commands,
  the editor (with an engine validation error) and Filter settings.

With --snapshots DIR, PNGs of the key states go to DIR.
"""
from pathlib import Path
import argparse, os, shutil, subprocess, tempfile

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--library', type=Path, required=True, help='libmail_service.dylib, e.g. target/debug/libmail_service.dylib')
parser.add_argument('--models', type=Path, help='Models directory with the installed model (sorting runs for real)')
parser.add_argument('--sorted-store', type=Path, help='A store with filters on and sorted mail (copied, never changed)')
parser.add_argument('--real-download', action='store_true', help='Start the real model download into an empty directory and cancel it')
parser.add_argument('--snapshots', type=Path, help='Optional directory for PNG captures')
parser.add_argument('--only', choices=['setup', 'sorted'], help='Run one scenario')
args = parser.parse_args()
library = args.library.resolve(strict=True)
root = Path(__file__).resolve().parents[3]
source = root / 'apps/macos/Sources/EmailUI'

check = r'''
    // Never on screen: every way of ordering this window keeps it hidden.
    override func makeKeyAndOrderFront(_ sender: Any?) { checkOffscreen() }
    override func orderFront(_ sender: Any?) { checkOffscreen() }
    override func orderFrontRegardless() { checkOffscreen() }
    override func constrainFrameRect(_ frameRect: NSRect, to screen: NSScreen?) -> NSRect { frameRect }
    func checkOffscreen() {
        alphaValue = 0; ignoresMouseEvents = true
        setFrameOrigin(NSPoint(x: -40000, y: -40000)); orderBack(nil)
    }
    func checkFilters() {
        setvbuf(stdout, nil, _IONBF, 0)
        let env = ProcessInfo.processInfo.environment
        let scenario = env["FILTER_CHECK_SCENARIO"] ?? "sorted"
        func expect(_ ok: Bool, _ name: String) { precondition(ok, name); print("PASS \(name)") }
        func pump(_ seconds: Double = 0.05) { RunLoop.main.run(until: Date().addingTimeInterval(seconds)) }
        func wait(_ name: String, timeout: Double = 10, _ condition: () -> Bool) {
            let deadline = Date().addingTimeInterval(timeout)
            while !condition() && Date() < deadline { pump(0.02) }
            expect(condition(), name)
        }
        func all(_ view: NSView) -> [NSView] { [view] + view.subviews.flatMap { all($0) } }
        func texts(_ view: NSView) -> [String] { all(view).filter { !$0.isHiddenOrHasHiddenAncestor }.compactMap { ($0 as? NSTextField)?.stringValue } }
        func titles(_ view: NSView) -> [String] { all(view).filter { !$0.isHiddenOrHasHiddenAncestor }.compactMap { ($0 as? NSButton)?.title } }
        func key(_ panel: NSWindow, _ characters: String, _ code: UInt16) -> Bool {
            let event = NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: [], timestamp: 0, windowNumber: panel.windowNumber,
                                         context: nil, characters: characters, charactersIgnoringModifiers: characters, isARepeat: false, keyCode: code)!
            return panel.performKeyEquivalent(with: event)
        }
        func write(_ rep: NSBitmapImageRep, _ name: String) {
            guard let directory = env["EMAIL_CHECK_SNAPSHOTS"] else { return }
            try? rep.representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: directory).appendingPathComponent("\(name).png"))
        }
        func snapshot(_ name: String) {
            guard env["EMAIL_CHECK_SNAPSHOTS"] != nil, let view = contentView else { return }
            pump(0.6)
            view.layoutSubtreeIfNeeded()
            guard let rep = view.bitmapImageRepForCachingDisplay(in: view.bounds) else { return }
            view.cacheDisplay(in: view.bounds, to: rep)
            if let webView, !webView.isHidden, !readerBody.isHidden {
                var image: NSImage?
                var done = false
                Task { @MainActor in image = try? await webView.takeSnapshot(configuration: nil); done = true }
                let deadline = Date().addingTimeInterval(5)
                while !done && Date() < deadline { pump(0.02) }
                if let image, let context = NSGraphicsContext(bitmapImageRep: rep) {
                    NSGraphicsContext.saveGraphicsState(); NSGraphicsContext.current = context
                    image.draw(in: webView.convert(webView.bounds, to: view))
                    NSGraphicsContext.restoreGraphicsState()
                }
            }
            write(rep, name)
        }
        func snapshotPanel(_ panel: NSWindow, _ name: String) {
            guard let view = panel.contentView else { return }
            pump(0.4)
            view.layoutSubtreeIfNeeded()
            guard let rep = view.bitmapImageRepForCachingDisplay(in: view.bounds) else { return }
            view.cacheDisplay(in: view.bounds, to: rep)
            write(rep, name)
        }
        func sidebarRows() -> [String] {
            sidebarFilters.arrangedSubviews.filter { !$0.isHidden }.map { row in
                let button = row as? NSButton
                let own = button?.title ?? ""
                let inner = texts(row).filter { !$0.isEmpty }
                return own.isEmpty ? inner.joined(separator: " ") : own
            }
        }
        func rowText(_ row: Int) -> [String] {
            guard let cell = tableView(table, viewFor: nil, row: row) else { return [] }
            return texts(cell)
        }
        func query(_ payload: [String: Any]) -> [[String: Any]] { core!.call(payload) as? [[String: Any]] ?? [] }

        NSApp.appearance = NSAppearance(named: .aqua)
        setContentSize(NSSize(width: 1440, height: 900))
        checkOffscreen()
        wait("filters status loads") { filterStatus != nil }

        if scenario == "setup" {
            wait("the inbox loads") { !threads.isEmpty }
            expect(sidebarRows() == ["Set up filters"], "filters off: one Set up filters row, got \(sidebarRows())")
            snapshot("01-sidebar-set-up-filters")
            openFilterSetup()
            wait("Set up filters opens the setup sheet") { FilterSetupSheet.presented != nil && attachedSheet != nil }
            var sheet = FilterSetupSheet.presented!
            let sheetText = texts(sheet.panel.contentView!).joined(separator: "\n")
            expect(sheetText.contains("Sort mail automatically"), "setup title")
            expect(sheetText.contains("A small AI model on this Mac sorts first emails from people you haven’t written to. Your mail never leaves this Mac."), "setup line")
            expect(sheetText.contains("Downloads Email sorter (\(FilterStatus.bytes(156_603_310))) once. You can change filters any time."), "download size line: \(sheetText)")
            expect(sheet.rows.map(\.starter.name) == ["Cold outreach", "Promotions", "Newsletters", "Notifications"], "four starters")
            expect(sheet.rows.allSatisfy { $0.check.state == .on }, "all starters checked")
            expect(sheet.rows.map { FilterSheetParts.action(of: $0.popup) } == [.ignore, .skipInbox, .skipInbox, .inbox], "starter actions as defaults")
            expect(sheet.cancelButton.title == "Not now" && sheet.cancelButton.keyEquivalent == "\u{1b}", "Not now on Escape")
            expect(sheet.confirmButton.title == "Download and turn on" && sheet.confirmButton.keyEquivalent == "\r", "Download and turn on on Return")
            snapshotPanel(sheet.panel, "02-setup-sheet")
            expect(key(sheet.panel, "\u{1b}", 53), "Escape handled by the sheet")
            wait("Escape closes the sheet, nothing changes") { FilterSetupSheet.presented == nil && attachedSheet == nil && filterStatus?.enabled == false }
            expect(firstResponder === table, "focus returns to the list")
            openFilterSetup()
            wait("the sheet opens again") { FilterSetupSheet.presented != nil }
            sheet = FilterSetupSheet.presented!
            sheet.rows[2].check.performClick(nil)
            expect(!sheet.rows[2].popup.isEnabled, "an unchecked starter's menu is off")
            sheet.rows[1].popup.selectItem(withTitle: "Ignore")
            expect(key(sheet.panel, "\r", 36), "Return handled by the sheet")
            wait("Return closes the sheet at once") { FilterSetupSheet.presented == nil && attachedSheet == nil }
            wait("filters are on with the chosen starters") {
                filterStatus?.enabled == true && filterStatus?.filters.map(\.name) == ["Cold outreach", "Promotions", "Notifications"]
            }
            expect(filterStatus?.filters.first { $0.name == "Promotions" }?.action == .ignore, "the changed action was saved")
            if env["FILTER_CHECK_REAL_DOWNLOAD"] == "1" {
                wait("the real download shows progress in the sidebar", timeout: 60) { filterStatus?.downloading == true && (filterStatus?.percent ?? 0) >= 1 }
                expect(sidebarRows().first?.hasPrefix("Getting ready… ") == true, "progress row first: \(sidebarRows())")
                snapshot("03-downloading-real")
                _ = core!.call(["op": "filters_cancel_download"])
                wait("cancelling stops the download", timeout: 30) { filterDownloadTask == nil && filterStatus?.downloading == false }
                expect(sidebarRows().first == "Download model to start sorting", "not downloaded row: \(sidebarRows())")
                snapshot("03b-not-downloaded")
            } else if filterStatus?.installed == true {
                // The real model sorts the launch mailbox on this Mac.
                wait("sorting starts") { sortTask != nil }
                wait("sorting finishes", timeout: 600) { sortTask == nil }
                print("Sorting: \((core!.call(["op": "filters_status"]) as? [String: Any])?["sorting"] ?? "")")
                let cold = filterStatus!.filters.first { $0.name == "Cold outreach" }!
                let pitches = query(["op": "list_filter_threads", "filter_id": cold.id, "limit": 50])
                expect(!pitches.isEmpty, "the model found cold pitches: \(pitches.compactMap { $0["subject"] as? String })")
                expect(pitches.allSatisfy { $0["is_inbox"] as? Bool == false }, "ignored pitches left the inbox")
                expect(threads.allSatisfy { $0.filter == nil || $0.filter?.action == .inbox }, "the inbox only shows unfiltered or tagged mail")
                print("Sorted: " + filterStatus!.filters.map { "\($0.name) \($0.lastWeek)" }.joined(separator: ", "))
                snapshot("01b-sorted-inbox")
            }
            // The progress row, from a known state.
            var fake = (core!.call(["op": "filters_status"]) as? [String: Any])!
            var model = fake["model"] as! [String: Any]
            model["installed"] = false; model["downloading"] = true
            model["downloaded_bytes"] = Int64(Double((model["download_bytes"] as! NSNumber).int64Value) * 0.425)
            fake["model"] = model
            stopFilterPoll()
            filterStatus = FilterStatus(fake)
            refreshFilterSidebar()
            expect(sidebarRows().first == "Getting ready… 42%", "progress row: \(sidebarRows())")
            snapshot("03-downloading")
            model["downloading"] = false
            fake["model"] = model
            filterStatus = FilterStatus(fake)
            refreshFilterSidebar()
            expect(sidebarRows().first == "Download model to start sorting", "missing model row: \(sidebarRows())")
            snapshot("03c-not-downloaded")
            model["error"] = "The connection was lost"
            fake["model"] = model
            filterStatus = FilterStatus(fake)
            refreshFilterSidebar()
            expect(sidebarRows().first == "Download failed – Retry", "failed row: \(sidebarRows())")
            snapshot("03d-download-failed")
            close()
            return
        }

        // sorted
        let filters = filterStatus!.filters
        let cold = filters.first { $0.name == "Cold outreach" }!
        let promotions = filters.first { $0.name == "Promotions" }!
        expect(filterStatus?.enabled == true, "the sorted store has filters on")
        wait("the inbox loads") { !threads.isEmpty }
        let rows = sidebarRows()
        expect(rows.prefix(4).map { $0.components(separatedBy: " ").first ?? "" } == ["Cold", "Promotions", "Newsletters", "Notifications"], "one row per filter in order: \(rows)")
        expect(rows.last == "New filter", "New filter last")
        expect(rows[0] == "Cold outreach", "no count on an Ignore filter: \(rows[0])")
        expect(!rows.contains { $0.hasPrefix("Getting ready") || $0.hasPrefix("Download") }, "no status row with the model installed")
        // Inbox: Notifications tag like a label.
        let tagged = threads.firstIndex { $0.filter?.name == "Notifications" }
        expect(tagged != nil, "a Notifications conversation in the inbox")
        expect(rowText(tagged!).contains { $0.hasPrefix("#Notifications · ") }, "Keep in inbox filters tag the row: \(rowText(tagged!))")
        expect(!threads.contains { $0.filter?.action.leavesInbox == true }, "no skipped or ignored mail in the inbox")
        select(max(0, tagged! - 3))
        table.scrollRowToVisible(tagged!)
        pump(0.3)
        snapshot("05-inbox-notifications-tag")

        // The Cold outreach view.
        showInbox(scope: .filter(cold.id))
        wait("the filter view lists its mail") { threads.count >= 3 && threads.allSatisfy { $0.filter?.id == cold.id } }
        expect(inboxTitle?.stringValue == "Cold outreach", "title is the filter name")
        expect(syncStatus.stringValue.hasPrefix("Ignored · ") && syncStatus.stringValue.hasSuffix(" this week"), "subtitle: \(syncStatus.stringValue)")
        expect(search.isHidden && searchFiltersButton.isHidden && scopeTabs.isHidden && !filterEditButton.isHidden, "no search or tabs; Edit shows")
        let pitch = threads.firstIndex { $0.subject.contains("mobile app") } ?? 0
        // The sorter records no reason, so rows keep their preview; a
        // recorded reason (older stores) takes the preview's place.
        let reason = threads[pitch].filter!.reason
        expect(reason.isEmpty ? rowText(pitch).contains { $0.hasPrefix("Hi Alex") } : rowText(pitch).contains(reason), "the row shows the reason or the preview: \(rowText(pitch))")
        select(pitch)
        wait("the reader strip shows the filter") { !filterReaderBar.isHidden && texts(filterReaderBar).contains { $0 == "Cold outreach" || $0.hasPrefix("Cold outreach   ") } }
        expect(!filterReaderBar.moveToInbox.isHidden && filterReaderBar.moveTo.title == "Move to", "Move to inbox and Move to")
        expect(FilterReaderBar.destinations(for: threads[pitch].filter!, filters: filters).map(\.title) == ["Promotions", "Newsletters", "Notifications", "Inbox"], "Move to lists the other filters and Inbox")
        let notifications = filters.first { $0.name == "Notifications" }!
        let tagFilter = ThreadFilter(["id": notifications.id, "name": "Notifications", "action": "inbox", "reason": "", "source": "model"])!
        expect(FilterReaderBar.destinations(for: tagFilter, filters: filters).last?.title == "No filter", "a tag filter offers No filter")
        let palette = allPaletteItems().map(\.title)
        expect(palette.contains("Move to inbox") && palette.contains("Move to Promotions") && !palette.contains("Move to Cold outreach"), "palette move commands")
        expect(palette.contains("Filter settings") && palette.contains("Go to Cold outreach") && palette.contains("New filter"), "palette filter commands")
        wait("the message renders") { !(webView?.isHidden ?? true) && !(webView?.isLoading ?? true) }
        snapshot("04-filter-view")
        NSApp.appearance = NSAppearance(named: .darkAqua)
        pump(0.5)
        if let thread = threads.indices.contains(selection) ? threads[selection] : nil { show(thread) }
        wait("the message renders in dark mode") { !(webView?.isLoading ?? true) }
        snapshot("08-filter-view-dark")
        NSApp.appearance = NSAppearance(named: .aqua)
        pump(0.5)
        show(threads[selection])
        wait("light again") { !(webView?.isLoading ?? true) }

        // The editor.
        editFilterClickedForCheck()
        wait("Edit opens the editor") { FilterEditorSheet.presented != nil && attachedSheet != nil }
        var editor = FilterEditorSheet.presented!
        expect(editor.nameField.stringValue == "Cold outreach" && editor.descriptionText.hasPrefix("Unsolicited sales pitches"), "editor shows the filter")
        expect(editor.explanation.stringValue == "Archived and marked read.", "the Ignore line")
        editor.actionPopUp.selectItem(withTitle: "Skip inbox")
        editor.actionPopUp.handler()
        expect(editor.explanation.stringValue == "Archived. Read it here when you want.", "the line follows the menu")
        editor.actionPopUp.selectItem(withTitle: "Ignore")
        editor.actionPopUp.handler()
        expect(editor.deleteButton?.title == "Delete filter" && editor.cancelButton.keyEquivalent == "\u{1b}" && editor.saveButton.keyEquivalent == "\r", "Delete, Cancel on Escape, Save on Return")
        snapshotPanel(editor.panel, "06-filter-editor")
        expect(key(editor.panel, "\u{1b}", 53), "Escape handled by the editor")
        wait("Escape closes the editor") { FilterEditorSheet.presented == nil && attachedSheet == nil }
        expect(firstResponder === table, "focus back on the list after the editor")
        editFilter(nil)
        wait("New filter opens an empty editor") { FilterEditorSheet.presented != nil }
        editor = FilterEditorSheet.presented!
        expect(editor.deleteButton == nil && editor.nameField.stringValue.isEmpty, "no Delete on a new filter")
        editor.nameField.stringValue = "Inbox"
        editor.descriptionText = "Anything"
        editor.save()
        wait("the engine's validation shows inline") { !editor.errorLabel.isHidden && editor.errorLabel.stringValue == "\"Inbox\" is already used for your inbox." }
        snapshotPanel(editor.panel, "06b-filter-editor-error")
        editor.nameField.stringValue = "Receipts"
        editor.descriptionText = "Receipts and invoices from shops and services I bought from."
        editor.actionPopUp.selectItem(withTitle: "Keep in inbox")
        editor.save()
        wait("Save creates the filter and opens it") { FilterEditorSheet.presented == nil && currentFilter?.name == "Receipts" }
        expect(sidebarRows().contains("Receipts"), "the new filter is in the sidebar")
        let receipts = currentFilter!
        _ = core!.call(["op": "delete_filter", "id": receipts.id])
        showInbox(scope: .filter(cold.id))
        wait("back in Cold outreach") { threads.count >= 3 && threads.allSatisfy { $0.filter?.id == cold.id } }
        refreshFilters()
        wait("the deleted filter left the sidebar") { !sidebarRows().contains("Receipts") }

        // Filter settings.
        openFilterSettings()
        wait("Filter settings opens") { FilterSettingsSheet.presented != nil && attachedSheet != nil }
        let settings = FilterSettingsSheet.presented!
        let settingsText = texts(settings.panel.contentView!).joined(separator: "\n")
        expect(settingsText.contains("Sort mail automatically") && settings.enabledSwitch?.state == .on, "the on switch")
        expect(settingsText.contains("Email sorter · \(FilterStatus.bytes(156_603_310)) · runs on this Mac") && titles(settings.panel.contentView!).contains("Remove"), "the model with Remove: \(settingsText) \(titles(settings.panel.contentView!))")
        expect(filters.allSatisfy { filter in settingsText.contains(filter.name) }, "the filters are listed")
        snapshotPanel(settings.panel, "07-filter-settings")
        expect(key(settings.panel, "\u{1b}", 53), "Escape handled by settings")
        wait("Escape closes settings") { FilterSettingsSheet.presented == nil && attachedSheet == nil }
        expect(firstResponder === table, "focus back on the list after settings")

        // Move to inbox from the strip: the row leaves, the next one is selected.
        let before = threads.count
        select(pitch)
        wait("the pitch is shown") { !filterReaderBar.isHidden }
        let movedID = threads[pitch].id!
        let nextID = threads.indices.contains(pitch + 1) ? threads[pitch + 1].id : threads[max(0, pitch - 1)].id
        filterReaderBar.moveToInbox.performClick(nil)
        wait("Move to inbox takes it out of the filter") { threads.count == before - 1 && !threads.contains { $0.id == movedID } }
        wait("the next conversation is selected") { threads.indices.contains(selection) && threads[selection].id == nextID }
        let restored = query(["op": "search", "query": "", "limit": 500, "offset": 0, "is_inbox": true]).first { $0["id"] as? String == movedID }
        expect(restored != nil && restored?["filter"] is NSNull, "it is back in the inbox without a filter")
        // Move to from the inbox: a tagged notification into Promotions.
        showInbox(scope: .all)
        wait("inbox again") { threads.contains { $0.filter?.name == "Notifications" } }
        let receipt = threads.firstIndex { $0.filter?.name == "Notifications" }!
        let receiptID = threads[receipt].id!
        select(receipt)
        wait("a tagged conversation shows the strip without Move to inbox") { !filterReaderBar.isHidden && filterReaderBar.moveToInbox.isHidden }
        moveSelectionToFilter(promotions.id)
        wait("Move to Promotions takes it out of the inbox") { !threads.contains { $0.id == receiptID } }
        showInbox(scope: .filter(promotions.id))
        wait("and it is in Promotions") { threads.contains { $0.id == receiptID } }
        close()
    }
    func editFilterClickedForCheck() { editCurrentFilterClicked() }
'''
main = '''import AppKit
@main struct Check { @MainActor static func main() {
    let app = NSApplication.shared; app.setActivationPolicy(.accessory); app.finishLaunching()
    app.appearance = NSAppearance(named: .aqua)
    let window = MailWindow(); window.checkFilters()
} }
'''
anchor = '    @objc private func agentAccessClicked() { openAgentAccess() }'
with tempfile.TemporaryDirectory(prefix='email-filters-check-') as temp:
    work = Path(temp)
    text = source.joinpath('EmailUI.swift').read_text()
    assert anchor in text
    (work / 'EmailUI.swift').write_text(text.replace(anchor, check + '\n' + anchor, 1))
    (work / 'Check.swift').write_text(main)
    files = [str(f) for f in source.glob('*.swift') if f.name != 'EmailUI.swift']
    binary = work / 'email-filters-check'
    subprocess.run(['swiftc', '-parse-as-library', '-swift-version', '6', '-target', 'arm64-apple-macos13.0', '-o', str(binary),
                    str(work / 'EmailUI.swift'), str(work / 'Check.swift'), *files], check=True)
    home = work / 'home'
    home.mkdir()
    base = dict(os.environ, HOME=str(home), MAIL_SERVICE_LIBRARY=str(library), TZ='Europe/Oslo')
    if args.snapshots:
        args.snapshots.mkdir(parents=True, exist_ok=True)
        base['EMAIL_CHECK_SNAPSHOTS'] = str(args.snapshots.resolve())
    if args.only in (None, 'setup'):
        # Turning filters on downloads a missing model: never by accident.
        assert args.models or args.real_download, '--models (or --real-download) is needed for the setup run'
        models = work / 'empty-models'
        models.mkdir()
        env = dict(base, FILTER_CHECK_SCENARIO='setup', EMAIL_STORE_PATH=str(work / 'setup' / 'mail.sqlite'), EMAIL_DEMO='launch',
                   EMAIL_MODELS_DIR=str(args.models.resolve() if args.models and not args.real_download else models))
        if args.real_download:
            env['FILTER_CHECK_REAL_DOWNLOAD'] = '1'
        (work / 'setup').mkdir(mode=0o700)
        subprocess.run([str(binary)], env=env, check=True)
        print('PASS: filters setup')
    if args.only in (None, 'sorted'):
        assert args.sorted_store and args.models, '--sorted-store and --models are needed for the sorted run'
        store = work / 'sorted'
        store.mkdir(mode=0o700)
        for suffix in ['', '-wal', '-shm']:
            original = Path(str(args.sorted_store) + suffix)
            if original.exists():
                shutil.copy(original, store / ('mail.sqlite' + suffix))
        env = dict(base, FILTER_CHECK_SCENARIO='sorted', EMAIL_STORE_PATH=str(store / 'mail.sqlite'), EMAIL_MODELS_DIR=str(args.models.resolve()))
        subprocess.run([str(binary)], env=env, check=True)
        print('PASS: filters in the main window')
