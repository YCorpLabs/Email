import AppKit
import WebKit
import PDFKit

enum EmailTheme {
    static func isDark(_ appearance: NSAppearance) -> Bool {
        appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
    }
    private static func adaptive(light: UInt32, dark: UInt32) -> NSColor {
        NSColor(name: nil) { appearance in
            let hex = isDark(appearance) ? dark : light
            return NSColor(calibratedRed: CGFloat((hex >> 16) & 0xff) / 255,
                           green: CGFloat((hex >> 8) & 0xff) / 255,
                           blue: CGFloat(hex & 0xff) / 255, alpha: 1)
        }
    }
    static let canvas = adaptive(light: 0xF5F4F2, dark: 0x1A1B1C)
    static let sidebar = adaptive(light: 0xF2F1EF, dark: 0x242526)
    static let inbox = adaptive(light: 0xFAFAF9, dark: 0x1F2021)
    static let reader = adaptive(light: 0xFFFFFF, dark: 0x1B1C1D)
    static let ink = adaptive(light: 0x262626, dark: 0xECEBE8)
    static let muted = adaptive(light: 0x777875, dark: 0xA3A39F)
    static let accent = adaptive(light: 0x393A3D, dark: 0xE3E3E6)
    static let accentSoft = adaptive(light: 0xEAEAEC, dark: 0x303134)
    static let line = adaptive(light: 0xE5E4E1, dark: 0x353637)
    static let compose = adaptive(light: 0xFFFFFF, dark: 0x262728)
    /// Cautions, such as an agent permission that takes effect without approval.
    static let warning = adaptive(light: 0xA4500C, dark: 0xF2A541)
    static func cgColor(_ color: NSColor, in appearance: NSAppearance) -> CGColor {
        var resolved = color.cgColor
        appearance.performAsCurrentDrawingAppearance { resolved = color.cgColor }
        return resolved
    }
}

final class ShortcutStrip: NSView {
    var items: [(String, String)] = [] {
        didSet {
            needsDisplay = true
            setAccessibilityElement(true)
            setAccessibilityRole(.group)
            toolTip = items.map { "\($0.0): \($0.1)" }.joined(separator: " · ")
            setAccessibilityLabel(items.map { "\($0.0): \($0.1)" }.joined(separator: ", "))
        }
    }
    override var isFlipped: Bool { true }
    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        needsDisplay = true
    }
    override func draw(_ dirtyRect: NSRect) {
        EmailTheme.line.setFill()
        NSRect(x: 0, y: 0, width: bounds.width, height: 0.5).fill()
        var x: CGFloat = 0
        for (key, label) in items {
            let keyAttributes: [NSAttributedString.Key: Any] = [
                .font: NSFont.monospacedSystemFont(ofSize: 9, weight: .medium),
                .foregroundColor: EmailTheme.ink
            ]
            let labelAttributes: [NSAttributedString.Key: Any] = [
                .font: NSFont.systemFont(ofSize: 10), .foregroundColor: EmailTheme.muted
            ]
            let keySize = (key as NSString).size(withAttributes: keyAttributes)
            let labelSize = (label as NSString).size(withAttributes: labelAttributes)
            let keyWidth = keySize.width + 10
            guard x + keyWidth + 6 + labelSize.width <= bounds.width else { break }
            let rect = NSRect(x: x, y: 8, width: keyWidth, height: 17)
            let cap = NSBezierPath(roundedRect: rect, xRadius: 4, yRadius: 4)
            EmailTheme.accentSoft.withAlphaComponent(0.55).setFill()
            cap.fill()
            EmailTheme.line.setStroke()
            cap.lineWidth = 0.5
            cap.stroke()
            (key as NSString).draw(at: NSPoint(x: x + 5, y: 10), withAttributes: keyAttributes)
            (label as NSString).draw(at: NSPoint(x: x + keyWidth + 6, y: 10), withAttributes: labelAttributes)
            x += keyWidth + labelSize.width + 24
        }
    }
}

private final class MailRowView: NSTableRowView {
    override func drawSelection(in dirtyRect: NSRect) {
        guard selectionHighlightStyle != .none else { return }
        EmailTheme.accentSoft.setFill()
        bounds.fill()
        EmailTheme.accent.withAlphaComponent(0.65).setFill()
        NSRect(x: 0, y: 0, width: 2, height: bounds.height).fill()
    }
}

private final class InboxTableView: NSTableView {
    var contextMenuForRow: ((Int) -> NSMenu?)?

    override func menu(for event: NSEvent) -> NSMenu? {
        let row = row(at: convert(event.locationInWindow, from: nil))
        guard row >= 0 else { return nil }
        selectRowIndexes(IndexSet(integer: row), byExtendingSelection: false)
        return contextMenuForRow?(row)
    }
}

private final class FlippedDocumentView: NSView {
    override var isFlipped: Bool { true }
}

private final class ShortcutBadge: NSTextField {
    override func hitTest(_ point: NSPoint) -> NSView? { nil }
}

private let preferredWindowWidthKey = "EmailPreferredWindowWidth.v8"
private let appearanceModeKey = "EmailAppearanceMode"

private final class ResizeContainerView: NSView {
    weak var resizeEdge: NSView?
    weak var leftResizeEdge: NSView?
    var onAppearanceChange: (() -> Void)?

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        onAppearanceChange?()
    }

    override func hitTest(_ point: NSPoint) -> NSView? {
        let local = convert(point, from: superview)
        if bounds.contains(local), local.x <= bounds.minX + 12 {
            return leftResizeEdge
        }
        if bounds.contains(local), local.x >= bounds.maxX - 28 {
            return resizeEdge
        }
        return super.hitTest(point)
    }
}

private final class RecipientSummaryField: NSTextField {
    var onActivate: (() -> Void)?
    override var acceptsFirstResponder: Bool { true }
    override func resetCursorRects() { addCursorRect(bounds, cursor: .pointingHand) }
    override func mouseDown(with event: NSEvent) { onActivate?() }
    override func accessibilityPerformPress() -> Bool { onActivate?(); return true }
    override func keyDown(with event: NSEvent) {
        if event.keyCode == 36 || event.keyCode == 49 { onActivate?() }
        else { super.keyDown(with: event) }
    }
}

private final class LeftEdgeResizeView: NSView {
    private var initialMouseX: CGFloat = 0
    private var initialFrame: NSRect = .zero

    override var mouseDownCanMoveWindow: Bool { false }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
    override func resetCursorRects() {
        addCursorRect(bounds, cursor: .resizeLeftRight)
    }
    override func mouseDown(with event: NSEvent) {
        guard let window else { return }
        initialFrame = window.frame
        initialMouseX = event.locationInWindow.x + window.frame.minX
    }
    override func mouseDragged(with event: NSEvent) {
        guard let window else { return }
        let mouseX = event.locationInWindow.x + window.frame.minX
        let proposedX = initialFrame.minX + mouseX - initialMouseX
        let maximumX = initialFrame.maxX - window.minSize.width
        var frame = initialFrame
        frame.origin.x = min(proposedX, maximumX)
        frame.size.width = initialFrame.maxX - frame.origin.x
        window.setFrame(frame, display: true)
        UserDefaults.standard.set(Double(window.frame.width), forKey: preferredWindowWidthKey)
    }
    override func mouseUp(with event: NSEvent) {
        guard let window else { return }
        UserDefaults.standard.set(Double(window.frame.width), forKey: preferredWindowWidthKey)
    }
}

private final class RightEdgeResizeView: NSView {
    private var initialMouseX: CGFloat = 0
    private var initialFrame: NSRect = .zero

    override var mouseDownCanMoveWindow: Bool { false }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
    override func resetCursorRects() {
        addCursorRect(bounds, cursor: .resizeLeftRight)
    }
    override func mouseDown(with event: NSEvent) {
        guard let window else { return }
        initialMouseX = event.locationInWindow.x
        initialFrame = window.frame
    }
    override func mouseDragged(with event: NSEvent) {
        guard let window else { return }
        var frame = initialFrame
        frame.size.width = max(window.minSize.width, initialFrame.width + event.locationInWindow.x - initialMouseX)
        window.setFrame(frame, display: true)
        UserDefaults.standard.set(Double(window.frame.width), forKey: preferredWindowWidthKey)
    }
    override func mouseUp(with event: NSEvent) {
        guard let window else { return }
        UserDefaults.standard.set(Double(window.frame.width), forKey: preferredWindowWidthKey)
    }
    // Invisible like the left strip: macOS windows show no resize grabber.
}

struct MailThread: Sendable {
    let id: String?
    let accountID: String?
    let account: String
    let sender: String
    let subject: String
    let preview: String
    let time: String
    var unread: Bool
    let html: String
    let labels: [String]
    var pinned: Bool
    let snoozedUntil: Date?

    init(id: String? = nil, accountID: String? = nil, account: String, sender: String, subject: String, preview: String, time: String, unread: Bool, html: String, labels: [String] = [], pinned: Bool = false, snoozedUntil: Date? = nil) {
        self.id = id
        self.accountID = accountID
        self.account = account
        self.sender = sender
        self.subject = subject
        self.preview = preview
        self.time = time
        self.unread = unread
        self.html = html
        self.labels = labels
        self.pinned = pinned
        self.snoozedUntil = snoozedUntil
    }
}

let fixtureThreads: [MailThread] = [
    .init(account: "work@gmail.example", sender: "Maya Chen", subject: "The new direction", preview: "I pulled together the notes from yesterday…", time: "10:42", unread: true, html: "<p>Hey,</p><p>The new direction feels <strong>much clearer</strong> now.</p><p>We should keep the experience focused, make frequent actions instant, and leave room for personal workflows.</p><p>Would love your thoughts before Friday.</p><p>— Maya</p>"),
    .init(account: "hello@icloud.example", sender: "Studio Field", subject: "A quieter kind of interface", preview: "A few visual references for the next round.", time: "09:18", unread: true, html: "<p>Good morning,</p><p>The common thread is <em>restraint</em>: generous space, confident type, and color used only where it helps.</p><blockquote>Make the important thing impossible to miss.</blockquote><p>— Studio Field</p>"),
    .init(account: "work@gmail.example", sender: "Jonas Richter", subject: "Re: Friday sync", preview: "Perfect, 11 works for me.", time: "Yesterday", unread: false, html: "<p>Perfect, 11 works for me. I'll send a calendar invite.</p><p>Cheers,<br>Jonas</p>"),
    .init(account: "personal@outlook.example", sender: "Rail & Co.", subject: "Your itinerary is ready", preview: "Your upcoming trip details are all in one place.", time: "Yesterday", unread: false, html: "<h2>Your itinerary is ready</h2><p>Your upcoming trip details are all in one place.</p><p>Have a wonderful journey.</p>"),
    .init(account: "hello@icloud.example", sender: "Alex Morgan", subject: "A little update from us", preview: "We shipped the first version this morning…", time: "Tue", unread: false, html: "<p>We shipped the first version this morning and wanted to share a little update.</p><p>Thank you for your thoughtful feedback.</p>")
]

@MainActor
final class MailWindow: NSWindow, NSWindowDelegate, WKNavigationDelegate, NSTableViewDataSource, NSTableViewDelegate, NSSearchFieldDelegate {
    private enum Folder { case inbox, drafts, outbox, agentRequests }
    private enum InboxScope: Equatable, Sendable { case all, unread, pinned, snoozed, followups, archived, sent, account(String), split(String), label(String) }
    private var folder: Folder = .inbox
    private var inboxScope: InboxScope = .all
    private var threads: [MailThread] = []
    private var drafts: [MailDraft] = []
    private var outboxItems: [OutboxItem] = []
    private var splits: [MailSplit] = []
    private var splitEditor: SplitEditorWindow?
    private var knownAccounts: [MailAccount] = []
    private var inboxLoadGeneration = 0
    private var searchRefreshTask: Task<Void, Never>?
    private var inboxLimit = 100
    private var canLoadOlder = true
    private var isLoadingOlder = false
    private var nextOlderNetworkSyncAt = Date.distantPast
    private let core: CoreBridge?
    private var senderCleanup: SenderCleanupController?
    private let usesCore: Bool
    private var selection = 0
    private var suppressSelectionChanges = false
    private var displayedThreadID: String?
    private var pendingRemovedThreadIDs = Set<String>()
    private var pendingReadStates: [String: Bool] = [:]
    private var markedThreadIDs = Set<String>()
    private var markedScopeKey = ""
    private var triageBusy = false
    private var lastUndoBatch: (id: String, accounts: [String])?
    private var lastDiscardedDraft: (id: String, subject: String)?
    private var draftDiscardBusy = false
    private let triageBar = NSStackView()
    private let triageCount = NSTextField(labelWithString: "")
    private var triageButtons: [NSButton] = []
    private let table = InboxTableView()
    private let inboxScroll = NSScrollView()
    private let inboxEmpty = NSTextField(labelWithString: "")
    private let search = NSSearchField()
    private let searchFiltersButton = NSButton()
    private var searchFilterPopover: NSPopover?
    private let scopeTabs = NSStackView()
    private var scopeTabButtons: [NSButton] = []
    private var unreadCounts: [String: Int] = [:]
    private let subject = NSTextField(labelWithString: "")
    private let sender = NSTextField(labelWithString: "")
    private let account = RecipientSummaryField(labelWithString: "")
    private var recipientSummaryMessage: [String: Any]?
    private var recipientPopover: NSPopover?
    private let attachmentsButton = NSButton(title: "", target: nil, action: nil)
    private let remoteImagesButton = NSButton(title: "Load images", target: nil, action: nil)
    // Shown above the message while its remote images are blocked.
    private let remoteImagesNotice = NSView()
    private let remoteImagesNoticeButton = NSButton(title: "Load images", target: nil, action: nil)
    private var readerBodyBelowHeader: NSLayoutConstraint?
    private var readerBodyBelowNotice: NSLayoutConstraint?
    private let unsubscribeButton = NSButton(title: "Unsubscribe", target: nil, action: nil)
    private var unsubscribeURL: URL?
    private var currentHTML: String?
    private var pendingInlineImageSources: [String: String] = [:]
    private var currentWebDidCommit = false
    // Reopening a message should not sanitize the same HTML through Rust again.
    // Keep this bounded so large newsletters cannot grow the app without limit.
    private var renderedBodyCache: [String: [String: Any]] = [:]
    private var renderedBodyOrder: [String] = []
    private var readerMessageCount = 0
    private var readerMode = false
    private let readerKeyboardHint = ShortcutStrip()
    private var awaitingReaderTop = false
    private var allowRemoteImagesForMessage = false
    private var currentMessageID: String? { didSet { if currentMessageID != oldValue { updateTrackingButton() } } }
    private var currentAttachments: [[String: Any]] = []
    private let syncStatus = NSTextField(labelWithString: "")
    private var sidebarAccounts = NSStackView()
    private var sidebarSplits = NSStackView()
    private var accountSetupWindow: AccountSetupWindow?
    private var composePane: ComposePane?
    /// An open composer blocks update restarts until the user closes it.
    var hasOpenComposer: Bool { composePane != nil }
    private var readerView: NSView?
    private weak var sidebarPanel: NSView?
    private weak var composeSidebarButton: NSButton?
    private weak var inboxPanel: NSView?
    private weak var readerPanel: NSView?
    private var inboxTitle: NSTextField?
    private var inboxSidebarButton: NSButton?
    private var draftsSidebarButton: NSButton?
    private var outboxSidebarButton: NSButton?
    private var archiveSidebarButton: NSButton?
    private var followupsSidebarButton: NSButton?
    private var calendarWorkspace: AgendaPanel?
    private var cachedCalendarWorkspace: AgendaPanel?
    private var calendarHiddenMailViews: [(NSView, Bool)] = []
    private var calendarWorkspaceView: NSView?
    private var calendarSidebarButton: NSButton?
    /// Sidebar rows that do nothing until an account exists (see `updateAccountChrome`).
    private var accountOnlySidebarViews: [NSView] = []
    private var workflowPopover: NSPopover?
    private var archiveBrowseStarted = false
    private var unreadSidebarButton: NSButton?
    private var pinnedSidebarButton: NSButton?
    private var snoozedSidebarButton: NSButton?
    private let labelsButton = NSButton(title: "Labels", target: nil, action: nil)
    private let trackingActivityButton = NSButton(title: "Recent opens", target: nil, action: nil)
    private let archiveButton = NSButton(title: "Archive", target: nil, action: nil)
    private let replyButton = NSButton(title: "Reply", target: nil, action: nil)
    private let replyAllButton = NSButton(title: "Reply all", target: nil, action: nil)
    private let contactButton = NSButton(title: "Contact", target: nil, action: nil)
    private let contactPane = ContactPane(frame: .zero)
    private let activityHost = NSView()
    private var activityController: TrackingActivityView?
    /// Public builds offer Recent opens only for mail already sent with
    /// tracking from a development build; they cannot create new records.
    private var offersOpenActivity = DistributionProfile.current?.openTracking == true
    private weak var activityReturnResponder: NSResponder?
    private var contactPanePreference: Bool?
    private var contactHistory: [String: [MailThread]] = [:]
    private var pendingContactThreadID: String?
    private let pinButton = NSButton(title: "Favorite", target: nil, action: nil)
    private let snoozeButton = NSButton(title: "Snooze", target: nil, action: nil)
    private var lastQueuedMessageID: String?
    private var sentSidebarButton: NSButton?
    private let cancelSendButton = NSButton(title: "Undo send", target: nil, action: nil)
    private let readerActions = NSStackView()
    private let readerBody = NSView()
    private var readerBodyBottomConstraint: NSLayoutConstraint?
    private var replyBodyBottomConstraint: NSLayoutConstraint?
    private let emptyState = NSStackView()
    private let emptyTitle = NSTextField(labelWithString: "")
    private let emptyDetail = NSTextField(labelWithString: "")
    private let emptyAction = NSButton(title: "Connect an account", target: nil, action: nil)
    private let bodyText = NSTextView()
    private let bodyScroll = NSScrollView()
    private let attachmentCard = NSView()
    private let attachmentName = NSTextField(labelWithString: "")
    private let attachmentMeta = NSTextField(labelWithString: "")
    private let attachmentSave = NSButton(title: "Save file…", target: nil, action: nil)
    private var webView: WKWebView?
    private var currentWebNavigation: WKNavigation?
    private let palette = NSPanel(contentRect: NSRect(x: 0, y: 0, width: 540, height: 456), styleMask: [.titled, .nonactivatingPanel], backing: .buffered, defer: false)
    private let paletteSearch = NSSearchField()
    private let paletteScroll = NSScrollView()
    private let paletteRows = FlippedDocumentView()
    private let paletteHeading = NSTextField(labelWithString: "COMMANDS")
    private let paletteEscapeHint = NSTextField(labelWithString: "ESC TO CLOSE")
    private let paletteFooter = NSTextField(labelWithString: "↑ ↓ navigate   ↵ run   esc close")
    private var paletteButtons: [NSButton] = []
    private var paletteSelection = 0
    private enum PalettePage: Equatable { case root, labels, labelDestinations, snooze, accounts, snippets }
    private enum PaletteTarget: Hashable {
        case search, searchHelp, searchFilters, archive, markRead, markUnread, bulkReadSender, bulkReadSenderDownloaded, cleanupJobs, reply, replyAll, compose, drafts, sync, undoTriage, selectLoaded, clearSelection
        case editDraft, discardDraft, undoDiscardDraft, discardComposerDraft
        case addAccount, manageAccounts, shortcuts, unsubscribe, labels, snooze
        case inbox, unread, snoozed, outbox, sent, newSplit, spam, favorite, favorites
        case switchAccount, browseLabels, account(String), split(String), goToLabel(String)
        case toggleLabel(String), createLabel(String), snoozeUntil(Date), unsnooze
        case toggleImages, contact, appearance(Int), notifications, testNotifications, notificationPreviews
        case sendDraft, saveDraft, scheduleDraft, attachFile, chooseFrom, focusRecipient, focusCc, focusBcc, focusBody
        case agentAccess, agentRequests
        case snippets, saveSnippet, insertSnippet(String), about, support, feedback, invitations, followup, followups, signature, bookings, insertBooking, trackingActivity, toggleTracking
        case calendarSearch, calendarCreate, calendarRefresh, calendarView(Int), calendarMove(Int), calendarToday, calendarJump, calendarHelp, calendarDate(Date)

        var requiresScheduling: Bool {
            switch self {
            case .bookings, .insertBooking, .trackingActivity, .toggleTracking: return true
            default: return false
            }
        }
    }
    private struct PaletteItem {
        let target: PaletteTarget
        let title: String
        let section: String
        let shortcut: String
        let detail: String
        let aliases: [String]
    }
    private var palettePage: PalettePage = .root
    private var paletteRootQuery = ""
    private var paletteLabels: [String] = []
    private var paletteResults: [PaletteItem] = []
    private var paletteLoadGeneration = 0
    private weak var paletteReturnResponder: NSResponder?
    private var eventMonitor: Any?
    private var messageLoadGeneration = 0
    private var dispatchTimer: Timer?
    private var mailSyncTimer: Timer?
    private var agentAccess: AgentAccessController?
    private weak var agentSidebarButton: NSButton?
    private var agentPollInFlight = false
    private var agentRevision = ""
    /// Agent requests in the main window: waiting approvals and refused agents.
    private var agentRequests: [[String: Any]] = []
    private weak var agentRequestsSidebarButton: NSButton?
    private let agentRequestsPill = AgentPill()
    private let agentRequestReader = AgentRequestReader()
    private var controlsBelowSearch: NSLayoutConstraint?
    private var controlsBelowStatus: NSLayoutConstraint?
    private var agentAccessOn = false
    private var agentRequestTotal = 0
    private var agentRequestBusy = false
    private var agentUndoTimer: Timer?
    private var queuedActionRetryTimer: Timer?
    private var pendingNotificationThreadID: String?
    private var pendingActivityThread: MailThread?
    private var pendingActivityGeneration: Int?
    private var pendingActivitySelection: (threadID: String, messageID: String)?
    private var activityReaderMessageID: String?
    private var syncInProgress = false
    private var recipientHeaderScript: String?
    private var awaitingGoTo = false
    private var goToBadges: [ShortcutBadge] = []
    private var mailboxCountBadges: [NSTextField] = []

    init() {
        let bridge = CoreBridge()
        if let demo = IsolatedStore.demoRequest, let bridge {
            do { _ = try bridge.request(demo) }
            catch { fputs("Demo messages were not added: \(error.localizedDescription)\n", stderr) }
        }
        _ = bridge?.buildFeatures()
        let stored = bridge?.threads() ?? []
        knownAccounts = bridge?.accountRecords() ?? []
        splits = bridge?.splits() ?? []
        core = bridge
        usesCore = bridge != nil
        threads = bridge != nil ? stored : (ProcessInfo.processInfo.environment["EMAIL_FIXTURE"] == "1" ? fixtureThreads : [])
        let availableWidth = NSScreen.main?.visibleFrame.width ?? 1400
        let initialWidth = min(1600, max(1100, availableWidth * 0.92))
        super.init(contentRect: NSRect(x: 0, y: 0, width: initialWidth, height: 760), styleMask: [.titled, .closable, .miniaturizable, .resizable], backing: .buffered, defer: false)
        title = "Email" // Keep the accessible/window-menu name without a separate title strip.
        titleVisibility = .hidden
        titlebarAppearsTransparent = true
        styleMask.insert(.fullSizeContentView)
        titlebarSeparatorStyle = .none
        minSize = NSSize(width: 760, height: 500)
        maxSize = NSSize(width: 10000, height: 10000)
        collectionBehavior.insert(.fullScreenPrimary)
        center()
        delegate = self
        backgroundColor = EmailTheme.canvas
        buildUI()
        (contentView as? ResizeContainerView)?.onAppearanceChange = { [weak self] in self?.applyTheme() }
        applyTheme()
        initialFirstResponder = table
        makeFirstResponder(table)
        buildPalette()
        checkSavedOpenActivity()
        restoreTriageUndo()
        refreshInbox()
        updateEmptyInbox()
        updateAccountChrome()
        syncStatus.stringValue = usesCore ? "" : "Mail engine unavailable — build mail-service first"
        if let first = threads.first {
            table.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false)
            show(first)
        } else { clearReader() }
        eventMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            guard let self else { return event }
            return self.handle(event) ? nil : event
        }
        let timer = Timer(timeInterval: 5, repeats: true) { [weak self] _ in
            Task { @MainActor [weak self] in await self?.dispatchScheduledMail() }
            Task { @MainActor [weak self] in await self?.refreshAgentRequests() }
            Task { @MainActor [weak self] in
                if let core = self?.core { await MailNotifications.shared.recordFollowups(core: core); await MailNotifications.shared.recordOpens(core: core) }
            }
        }
        RunLoop.main.add(timer, forMode: .common)
        dispatchTimer = timer
        Task { @MainActor [weak self] in await self?.dispatchScheduledMail() }
        Task { @MainActor [weak self] in await self?.refreshAgentRequests() }
        let mailTimer = Timer(timeInterval: 60, repeats: true) { [weak self] _ in
            Task { @MainActor [weak self] in self?.syncAll(showErrors: false) }
        }
        RunLoop.main.add(mailTimer, forMode: .common)
        mailSyncTimer = mailTimer
        if !knownAccounts.isEmpty {
            Task { @MainActor [weak self] in self?.syncAll(showErrors: false) }
        }
    }


    private func buildUI() {
        let container = ResizeContainerView()
        contentView = container
        let root = NSStackView()
        root.orientation = .horizontal
        root.spacing = 0
        root.distribution = .fill
        root.translatesAutoresizingMaskIntoConstraints = false
        container.addSubview(root)
        NSLayoutConstraint.activate([
            root.leadingAnchor.constraint(equalTo: container.leadingAnchor),
            root.trailingAnchor.constraint(equalTo: container.trailingAnchor),
            root.topAnchor.constraint(equalTo: container.topAnchor),
            root.bottomAnchor.constraint(equalTo: container.bottomAnchor)
        ])

        let rail = NSView()
        sidebarPanel = rail
        rail.wantsLayer = true
        rail.layer?.backgroundColor = NSColor.clear.cgColor
        rail.translatesAutoresizingMaskIntoConstraints = false
        rail.widthAnchor.constraint(equalToConstant: 164).isActive = true
        let sidebarMaterial = NSVisualEffectView()
        sidebarMaterial.material = .sidebar
        sidebarMaterial.blendingMode = .behindWindow
        sidebarMaterial.state = .followsWindowActiveState
        sidebarMaterial.translatesAutoresizingMaskIntoConstraints = false
        rail.addSubview(sidebarMaterial)
        NSLayoutConstraint.activate([
            sidebarMaterial.leadingAnchor.constraint(equalTo: rail.leadingAnchor),
            sidebarMaterial.trailingAnchor.constraint(equalTo: rail.trailingAnchor),
            sidebarMaterial.topAnchor.constraint(equalTo: rail.topAnchor),
            sidebarMaterial.bottomAnchor.constraint(equalTo: rail.bottomAnchor)
        ])
        let railStack = NSStackView()
        railStack.orientation = .vertical
        railStack.alignment = .leading
        railStack.spacing = 3
        railStack.translatesAutoresizingMaskIntoConstraints = false
        let sidebarScroll = NSScrollView()
        sidebarScroll.hasVerticalScroller = true
        sidebarScroll.drawsBackground = false
        sidebarScroll.borderType = .noBorder
        sidebarScroll.translatesAutoresizingMaskIntoConstraints = false
        let sidebarDocument = FlippedDocumentView()
        sidebarDocument.translatesAutoresizingMaskIntoConstraints = false
        sidebarScroll.documentView = sidebarDocument
        let commandButton = NSButton(title: "Commands", target: self, action: #selector(openCommandsClicked(_:)))
        commandButton.image = NSImage(systemSymbolName: "command", accessibilityDescription: nil)
        commandButton.imagePosition = .imageLeading
        commandButton.isBordered = false
        commandButton.alignment = .left
        commandButton.font = .systemFont(ofSize: 11, weight: .medium)
        commandButton.contentTintColor = EmailTheme.muted
        commandButton.toolTip = "Command menu (⌘K)"
        commandButton.translatesAutoresizingMaskIntoConstraints = false
        let commandHint = ShortcutBadge(labelWithString: "⌘K")
        commandHint.font = .monospacedSystemFont(ofSize: 10, weight: .medium)
        commandHint.textColor = EmailTheme.muted
        commandHint.translatesAutoresizingMaskIntoConstraints = false
        commandButton.addSubview(commandHint)
        rail.addSubview(sidebarScroll)
        rail.addSubview(commandButton)
        sidebarDocument.addSubview(railStack)
        NSLayoutConstraint.activate([
            sidebarScroll.leadingAnchor.constraint(equalTo: rail.leadingAnchor),
            sidebarScroll.trailingAnchor.constraint(equalTo: rail.trailingAnchor),
            sidebarScroll.topAnchor.constraint(equalTo: rail.topAnchor),
            sidebarScroll.bottomAnchor.constraint(equalTo: commandButton.topAnchor, constant: -8),
            commandButton.leadingAnchor.constraint(equalTo: rail.leadingAnchor, constant: 14),
            commandButton.trailingAnchor.constraint(equalTo: rail.trailingAnchor, constant: -14),
            commandButton.bottomAnchor.constraint(equalTo: rail.bottomAnchor, constant: -10),
            commandButton.heightAnchor.constraint(equalToConstant: 29),
            commandHint.trailingAnchor.constraint(equalTo: commandButton.trailingAnchor, constant: -8),
            commandHint.centerYAnchor.constraint(equalTo: commandButton.centerYAnchor),
            sidebarDocument.widthAnchor.constraint(equalTo: sidebarScroll.contentView.widthAnchor),
            sidebarDocument.heightAnchor.constraint(greaterThanOrEqualTo: sidebarScroll.contentView.heightAnchor),
            railStack.leadingAnchor.constraint(equalTo: sidebarDocument.leadingAnchor, constant: 14),
            railStack.trailingAnchor.constraint(equalTo: sidebarDocument.trailingAnchor, constant: -14),
            railStack.topAnchor.constraint(equalTo: sidebarDocument.topAnchor, constant: 42),
            railStack.bottomAnchor.constraint(lessThanOrEqualTo: sidebarDocument.bottomAnchor, constant: -24)
        ])
        let brand = heading("Email", size: 16, weight: .semibold, color: EmailTheme.ink)
        railStack.addArrangedSubview(brand)
        railStack.setCustomSpacing(16, after: brand)
        let composeButton = sidebarButton("Compose", symbol: "square.and.pencil", action: #selector(composeClicked(_:)))
        composeSidebarButton = composeButton
        composeButton.layer?.backgroundColor = EmailTheme.cgColor(EmailTheme.accentSoft, in: effectiveAppearance)
        composeButton.toolTip = "New message (C)"
        railStack.addArrangedSubview(composeButton)
        railStack.setCustomSpacing(22, after: composeButton)
        let workspaceHeading = heading("WORKSPACE", size: 10, weight: .semibold, color: EmailTheme.muted)
        railStack.addArrangedSubview(workspaceHeading)
        let allInboxes = sidebarButton("All Inboxes", symbol: "tray.full", action: #selector(showInboxClicked), active: true)
        let unreadFolder = sidebarButton("Unread", symbol: "envelope.badge", action: #selector(showUnreadClicked))
        let pinnedFolder = sidebarButton("Favorites", symbol: "star", action: #selector(showPinnedClicked))
        let followupsFolder = sidebarButton("Follow-ups", symbol: "arrowshape.turn.up.left", action: #selector(showFollowupsClicked))
        let invitationsFolder = sidebarButton("Calendar", symbol: "calendar", action: #selector(showInvitationsClicked))
        calendarSidebarButton = invitationsFolder
        let snoozedFolder = sidebarButton("Snoozed", symbol: "clock", action: #selector(showSnoozedClicked))
        let draftFolder = sidebarButton("Drafts", symbol: "square.and.pencil", action: #selector(showDraftsClicked))
        let archiveFolder = sidebarButton("Archive", symbol: "archivebox", action: #selector(showArchiveClicked))
        let sentFolder = sidebarButton("Sent", symbol: "paperplane.fill", action: #selector(showSentClicked))
        sentSidebarButton = sentFolder
        sentFolder.toolTip = "Sent mail — G then T (downloaded messages)"
        let outboxFolder = sidebarButton("Outbox", symbol: "paperplane", action: #selector(showOutboxClicked))
        let agentRequestsFolder = sidebarButton("Agent requests", symbol: "sparkles", action: #selector(showAgentRequestsClicked))
        agentRequestsFolder.toolTip = "Agent requests: what agents are waiting for you to OK"
        agentRequestsPill.translatesAutoresizingMaskIntoConstraints = false
        agentRequestsPill.isHidden = true
        agentRequestsFolder.addSubview(agentRequestsPill)
        NSLayoutConstraint.activate([
            agentRequestsPill.trailingAnchor.constraint(equalTo: agentRequestsFolder.trailingAnchor, constant: -2),
            agentRequestsPill.centerYAnchor.constraint(equalTo: agentRequestsFolder.centerYAnchor),
        ])
        agentRequestsFolder.isHidden = true
        agentRequestsSidebarButton = agentRequestsFolder
        for (button, key) in [(allInboxes, "i"), (unreadFolder, "u"), (pinnedFolder, "f"), (snoozedFolder, "s"), (draftFolder, "d"), (outboxFolder, "o"), (archiveFolder, "a"), (followupsFolder, "w"), (invitationsFolder, "c"), (sentFolder, "t")] {
            button.toolTip = "\(button.title) — g then \(key)"
            let badge = ShortcutBadge(frame: .zero)
            badge.stringValue = key
            badge.font = .monospacedSystemFont(ofSize: 11, weight: .medium)
            badge.textColor = button === allInboxes ? EmailTheme.accent : EmailTheme.muted
            badge.alignment = .center
            badge.isEditable = false
            badge.isSelectable = false
            badge.isBordered = false
            badge.drawsBackground = false
            badge.isHidden = true
            badge.translatesAutoresizingMaskIntoConstraints = false
            button.addSubview(badge)
            NSLayoutConstraint.activate([
                badge.trailingAnchor.constraint(equalTo: button.trailingAnchor, constant: -10),
                badge.centerYAnchor.constraint(equalTo: button.centerYAnchor),
                badge.widthAnchor.constraint(equalToConstant: 12),
                badge.heightAnchor.constraint(equalToConstant: 16)
            ])
            goToBadges.append(badge)
        }
        for button in [allInboxes, followupsFolder] {
            let badge = NSTextField(labelWithString: "")
            badge.font = .monospacedDigitSystemFont(ofSize: 10, weight: .medium)
            badge.textColor = EmailTheme.muted
            badge.alignment = .right
            badge.translatesAutoresizingMaskIntoConstraints = false
            button.addSubview(badge)
            NSLayoutConstraint.activate([
                badge.trailingAnchor.constraint(equalTo: button.trailingAnchor, constant: -6),
                badge.centerYAnchor.constraint(equalTo: button.centerYAnchor),
                badge.widthAnchor.constraint(lessThanOrEqualToConstant: 28)
            ])
            mailboxCountBadges.append(badge)
        }
        inboxSidebarButton = allInboxes
        unreadSidebarButton = unreadFolder
        pinnedSidebarButton = pinnedFolder
        followupsSidebarButton = followupsFolder
        snoozedSidebarButton = snoozedFolder
        draftsSidebarButton = draftFolder
        outboxSidebarButton = outboxFolder
        archiveSidebarButton = archiveFolder
        railStack.addArrangedSubview(allInboxes)
        railStack.addArrangedSubview(archiveFolder)
        railStack.addArrangedSubview(snoozedFolder)
        railStack.addArrangedSubview(followupsFolder)
        railStack.addArrangedSubview(invitationsFolder)
        railStack.addArrangedSubview(draftFolder)
        railStack.addArrangedSubview(sentFolder)
        railStack.addArrangedSubview(outboxFolder)
        railStack.addArrangedSubview(agentRequestsFolder)
        railStack.setCustomSpacing(19, after: railStack.arrangedSubviews.last!)
        let splitsHeading = heading("SPLITS", size: 10, weight: .semibold, color: EmailTheme.muted)
        railStack.addArrangedSubview(splitsHeading)
        sidebarSplits.orientation = .vertical
        sidebarSplits.alignment = .leading
        sidebarSplits.spacing = 6
        railStack.addArrangedSubview(sidebarSplits)
        let newSplit = sidebarButton("New split", symbol: "plus", action: #selector(createSplitClicked), primary: true)
        railStack.addArrangedSubview(newSplit)
        railStack.setCustomSpacing(19, after: newSplit)
        let accountsHeading = heading("ACCOUNTS", size: 10, weight: .semibold, color: EmailTheme.muted)
        railStack.addArrangedSubview(accountsHeading)
        sidebarAccounts.orientation = .vertical
        sidebarAccounts.alignment = .leading
        sidebarAccounts.spacing = 10
        railStack.addArrangedSubview(sidebarAccounts)
        refreshSidebar()
        railStack.setCustomSpacing(11, after: sidebarAccounts)
        let addAccount = sidebarButton("Add account", symbol: "plus", action: #selector(addAccountClicked), primary: true)
        railStack.addArrangedSubview(addAccount)
        let manageAccounts = sidebarButton("Manage accounts", symbol: "gearshape", action: #selector(manageAccountsClicked))
        railStack.addArrangedSubview(manageAccounts)
        let agentButton = sidebarButton("Agent access", symbol: "checkmark.shield", action: #selector(agentAccessClicked))
        agentButton.toolTip = "Agent access: what AI agents may do with each account"
        agentSidebarButton = agentButton
        railStack.addArrangedSubview(agentButton)
        railStack.addArrangedSubview(sidebarButton("Support & feedback", symbol: "questionmark.circle", action: #selector(supportClicked)))
        // Without an account only Add account, Support & feedback and Commands
        // do something; the rest returns with the first account.
        accountOnlySidebarViews = [composeButton, workspaceHeading, allInboxes, archiveFolder, snoozedFolder, followupsFolder,
                                   draftFolder, sentFolder, outboxFolder, splitsHeading, sidebarSplits, newSplit,
                                   accountsHeading, sidebarAccounts, manageAccounts, agentButton]
        root.addArrangedSubview(rail)

        let inbox = NSView()
        inboxPanel = inbox
        inbox.wantsLayer = true
        inbox.layer?.backgroundColor = EmailTheme.cgColor(EmailTheme.inbox, in: effectiveAppearance)
        inbox.layer?.borderColor = EmailTheme.cgColor(EmailTheme.line, in: effectiveAppearance)
        inbox.layer?.borderWidth = 0.5
        inbox.translatesAutoresizingMaskIntoConstraints = false
        inbox.widthAnchor.constraint(equalToConstant: 360).isActive = true
        let inboxTitle = heading("Inbox", size: 18, weight: .semibold, color: EmailTheme.ink)
        self.inboxTitle = inboxTitle
        inboxTitle.translatesAutoresizingMaskIntoConstraints = false
        let syncButton = NSButton(title: "", target: self, action: #selector(syncClicked))
        syncButton.image = NSImage(systemSymbolName: "arrow.clockwise", accessibilityDescription: "Sync")
        syncButton.toolTip = "Sync all accounts (⌘R)"
        syncButton.isBordered = false
        syncButton.font = .systemFont(ofSize: 17, weight: .medium)
        syncButton.contentTintColor = EmailTheme.muted
        syncButton.translatesAutoresizingMaskIntoConstraints = false
        syncStatus.font = .systemFont(ofSize: 11)
        syncStatus.textColor = EmailTheme.muted
        syncStatus.translatesAutoresizingMaskIntoConstraints = false
        search.placeholderString = "Search downloaded mail  /"
        search.toolTip = Self.searchSyntaxHelp
        search.translatesAutoresizingMaskIntoConstraints = false
        search.delegate = self
        searchFiltersButton.image = NSImage(systemSymbolName: "line.3.horizontal.decrease", accessibilityDescription: "Search filters")
        searchFiltersButton.isBordered = false; searchFiltersButton.contentTintColor = EmailTheme.muted
        searchFiltersButton.target = self; searchFiltersButton.action = #selector(openSearchFilters)
        searchFiltersButton.toolTip = "Refine search (⌘⇧F)"; searchFiltersButton.setAccessibilityLabel("Refine search")
        searchFiltersButton.translatesAutoresizingMaskIntoConstraints = false
        search.focusRingType = .none
        search.wantsLayer = true
        search.layer?.cornerRadius = 7
        search.layer?.borderWidth = 1
        search.layer?.borderColor = EmailTheme.cgColor(EmailTheme.line, in: effectiveAppearance)
        scopeTabs.orientation = .horizontal
        scopeTabs.alignment = .centerY
        scopeTabs.spacing = 5
        scopeTabs.translatesAutoresizingMaskIntoConstraints = false
        for (title, action, key) in [("All", #selector(showInboxClicked), "i"), ("Unread", #selector(showUnreadClicked), "u"), ("Favorites", #selector(showPinnedClicked), "f")] {
            let button = NSButton(title: title, target: self, action: action)
            button.isBordered = false
            button.wantsLayer = true
            button.layer?.cornerRadius = 6
            button.toolTip = "Go to \(title) (G then \(key.uppercased()))"
            button.setAccessibilityLabel("Show \(title.lowercased()) mail")
            let badge = ShortcutBadge(labelWithString: key)
            badge.font = .monospacedSystemFont(ofSize: 10, weight: .medium)
            badge.alignment = .center
            badge.isHidden = true
            badge.translatesAutoresizingMaskIntoConstraints = false
            button.addSubview(badge)
            NSLayoutConstraint.activate([
                button.heightAnchor.constraint(equalToConstant: 27),
                button.widthAnchor.constraint(equalToConstant: title == "Favorites" ? 96 : (title == "Unread" ? 126 : 62)),
                badge.trailingAnchor.constraint(equalTo: button.trailingAnchor, constant: -8),
                badge.centerYAnchor.constraint(equalTo: button.centerYAnchor)
            ])
            scopeTabs.addArrangedSubview(button)
            scopeTabButtons.append(button)
            goToBadges.append(badge)
        }
        triageBar.orientation = .horizontal
        triageBar.spacing = 6
        triageBar.alignment = .centerY
        triageBar.translatesAutoresizingMaskIntoConstraints = false
        triageCount.font = .systemFont(ofSize: 11, weight: .medium)
        triageCount.textColor = EmailTheme.ink
        triageBar.addArrangedSubview(triageCount)
        for (title, action) in [("All", #selector(selectLoadedClicked)), ("Read", #selector(readMarkedClicked)), ("Archive", #selector(archiveMarkedClicked)), ("Undo Z", #selector(undoTriageClicked)), ("Clear", #selector(clearMarkedClicked))] {
            let button = NSButton(title: title, target: self, action: action)
            button.isBordered = false
            button.font = .systemFont(ofSize: 11, weight: .medium)
            button.contentTintColor = EmailTheme.ink
            button.heightAnchor.constraint(equalToConstant: 27).isActive = true
            triageBar.addArrangedSubview(button)
            triageButtons.append(button)
        }
        triageBar.isHidden = true
        let threadColumn = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("threads"))
        threadColumn.width = 360
        table.addTableColumn(threadColumn)
        table.columnAutoresizingStyle = .uniformColumnAutoresizingStyle
        table.headerView = nil
        table.rowHeight = 58
        table.intercellSpacing = .zero
        table.selectionHighlightStyle = .regular
        table.backgroundColor = EmailTheme.inbox
        table.usesAlternatingRowBackgroundColors = false
        table.gridStyleMask = []
        table.dataSource = self
        table.delegate = self
        table.target = self
        table.doubleAction = #selector(editSelectedDraft)
        table.contextMenuForRow = { [weak self] row in self?.contextMenu(for: row) }
        inboxScroll.documentView = table
        inboxScroll.hasVerticalScroller = true
        inboxScroll.drawsBackground = true
        inboxScroll.backgroundColor = EmailTheme.inbox
        inboxScroll.translatesAutoresizingMaskIntoConstraints = false
        inboxScroll.contentView.postsBoundsChangedNotifications = true
        NotificationCenter.default.addObserver(self, selector: #selector(inboxDidScroll(_:)), name: NSView.boundsDidChangeNotification, object: inboxScroll.contentView)
        inboxEmpty.font = .systemFont(ofSize: 13)
        inboxEmpty.textColor = EmailTheme.muted
        inboxEmpty.alignment = .center
        inboxEmpty.maximumNumberOfLines = 2
        inboxEmpty.translatesAutoresizingMaskIntoConstraints = false
        inbox.addSubview(inboxTitle)
        inbox.addSubview(syncButton)
        inbox.addSubview(syncStatus)
        inbox.addSubview(search)
        inbox.addSubview(searchFiltersButton)
        let inboxControls = NSStackView(views: [scopeTabs, triageBar])
        inboxControls.orientation = .vertical
        inboxControls.alignment = .leading
        inboxControls.spacing = 6
        inboxControls.translatesAutoresizingMaskIntoConstraints = false
        inbox.addSubview(inboxControls)
        // Agent requests hide the search field; the list then moves up under the count.
        let belowSearch = inboxControls.topAnchor.constraint(equalTo: search.bottomAnchor, constant: 12)
        controlsBelowSearch = belowSearch
        controlsBelowStatus = inboxControls.topAnchor.constraint(equalTo: syncStatus.bottomAnchor, constant: 8)
        inbox.addSubview(inboxScroll)
        inbox.addSubview(inboxEmpty)
        NSLayoutConstraint.activate([
            inboxTitle.leadingAnchor.constraint(equalTo: inbox.leadingAnchor, constant: 20), inboxTitle.topAnchor.constraint(equalTo: inbox.topAnchor, constant: 20),
            syncButton.trailingAnchor.constraint(equalTo: inbox.trailingAnchor, constant: -18), syncButton.centerYAnchor.constraint(equalTo: inboxTitle.centerYAnchor),
            syncStatus.leadingAnchor.constraint(equalTo: inboxTitle.leadingAnchor), syncStatus.topAnchor.constraint(equalTo: inboxTitle.bottomAnchor, constant: 4),
            search.leadingAnchor.constraint(equalTo: inbox.leadingAnchor, constant: 18), search.trailingAnchor.constraint(equalTo: searchFiltersButton.leadingAnchor, constant: -6), search.topAnchor.constraint(equalTo: syncStatus.bottomAnchor, constant: 12),
            searchFiltersButton.trailingAnchor.constraint(equalTo: inbox.trailingAnchor, constant: -18), searchFiltersButton.centerYAnchor.constraint(equalTo: search.centerYAnchor), searchFiltersButton.widthAnchor.constraint(equalToConstant: 24), searchFiltersButton.heightAnchor.constraint(equalToConstant: 24),
            inboxControls.leadingAnchor.constraint(equalTo: search.leadingAnchor),
            inboxControls.trailingAnchor.constraint(lessThanOrEqualTo: search.trailingAnchor),
            belowSearch,
            inboxScroll.leadingAnchor.constraint(equalTo: inbox.leadingAnchor), inboxScroll.trailingAnchor.constraint(equalTo: inbox.trailingAnchor), inboxScroll.topAnchor.constraint(equalTo: inboxControls.bottomAnchor, constant: 8), inboxScroll.bottomAnchor.constraint(equalTo: inbox.bottomAnchor),
            inboxEmpty.centerXAnchor.constraint(equalTo: inbox.centerXAnchor), inboxEmpty.topAnchor.constraint(equalTo: inboxControls.bottomAnchor, constant: 82), inboxEmpty.widthAnchor.constraint(lessThanOrEqualToConstant: 290)
        ])
        root.addArrangedSubview(inbox)

        let reader = NSView()
        readerPanel = reader
        readerView = reader
        reader.wantsLayer = true
        reader.layer?.backgroundColor = EmailTheme.cgColor(EmailTheme.reader, in: effectiveAppearance)
        reader.translatesAutoresizingMaskIntoConstraints = false
        subject.font = .systemFont(ofSize: 20, weight: .semibold)
        subject.textColor = EmailTheme.ink
        subject.maximumNumberOfLines = 2
        subject.lineBreakMode = .byTruncatingTail
        subject.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        subject.translatesAutoresizingMaskIntoConstraints = false
        sender.font = .systemFont(ofSize: 13, weight: .medium)
        sender.textColor = EmailTheme.ink
        sender.maximumNumberOfLines = 1
        sender.lineBreakMode = .byTruncatingMiddle
        sender.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        sender.translatesAutoresizingMaskIntoConstraints = false
        account.onActivate = { [weak self] in self?.showRecipientPopover() }
        account.setAccessibilityRole(.button)
        account.setAccessibilityLabel("Message recipients and details")
        account.font = .systemFont(ofSize: 11)
        account.textColor = EmailTheme.muted
        account.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        account.translatesAutoresizingMaskIntoConstraints = false
        readerActions.orientation = .horizontal
        readerActions.alignment = .centerY
        readerActions.spacing = 4
        readerActions.translatesAutoresizingMaskIntoConstraints = false
        contactButton.image = NSImage(systemSymbolName: "person.crop.circle", accessibilityDescription: nil)
        contactButton.imagePosition = .imageOnly
        contactButton.title = ""
        contactButton.isBordered = false
        contactButton.contentTintColor = EmailTheme.muted
        contactButton.toolTip = "Show or hide contact pane (0)"
        contactButton.setAccessibilityLabel("Contact pane")
        contactButton.target = self
        contactButton.action = #selector(toggleContactPaneClicked(_:))
        contactButton.widthAnchor.constraint(equalToConstant: 28).isActive = true
        contactButton.heightAnchor.constraint(equalToConstant: 26).isActive = true
        readerActions.addArrangedSubview(contactButton)
        trackingActivityButton.target = self
        trackingActivityButton.action = #selector(showTrackingActivity)
        trackingActivityButton.image = NSImage(systemSymbolName:"eye",accessibilityDescription:"Recent opens")
        trackingActivityButton.font = .systemFont(ofSize: 12, weight: .medium)
        trackingActivityButton.contentTintColor = EmailTheme.muted
        trackingActivityButton.isHidden = true
        trackingActivityButton.imagePosition = .imageOnly
        trackingActivityButton.isBordered = false
        trackingActivityButton.toolTip = "Recent opens"
        trackingActivityButton.widthAnchor.constraint(greaterThanOrEqualToConstant:26).isActive = true
        readerActions.addArrangedSubview(trackingActivityButton)
        for (button, symbol, action) in [(archiveButton, "archivebox", #selector(archiveClicked(_:))), (replyButton, "arrowshape.turn.up.left", #selector(replyClicked(_:))), (replyAllButton, "arrowshape.turn.up.left.2", #selector(replyAllClicked(_:))), (pinButton, "star", #selector(togglePinClicked(_:))), (labelsButton, "tag", #selector(showLabelsClicked(_:))), (snoozeButton, "clock", #selector(snoozeClicked(_:)))] {
            button.image = NSImage(systemSymbolName: symbol, accessibilityDescription: nil)
            button.imagePosition = .imageOnly
            button.title = ""
            button.isBordered = false
            button.contentTintColor = EmailTheme.muted
            button.toolTip = button == archiveButton ? "Archive (E)" : (button == replyButton ? "Reply (R)" : (button == pinButton ? "Add to favorites (F)" : (button == labelsButton ? "Labels (L)" : "Snooze (S)")))
            if button == replyAllButton { button.toolTip = "Reply all (⇧R)" }
            button.setAccessibilityLabel(button == archiveButton ? "Archive" : (button == replyButton ? "Reply" : (button == pinButton ? "Favorite" : (button == labelsButton ? "Labels" : "Snooze"))))
            if button == replyAllButton { button.setAccessibilityLabel("Reply all") }
            button.widthAnchor.constraint(equalToConstant: 28).isActive = true
            button.heightAnchor.constraint(equalToConstant: 26).isActive = true
            button.target = self
            button.action = action
            readerActions.addArrangedSubview(button)
        }
        cancelSendButton.image = NSImage(systemSymbolName: "arrow.uturn.backward", accessibilityDescription: nil)
        cancelSendButton.imagePosition = .imageLeading
        cancelSendButton.font = .systemFont(ofSize: 11, weight: .medium)
        cancelSendButton.target = self
        cancelSendButton.action = #selector(cancelScheduledClicked(_:))
        cancelSendButton.isHidden = true
        readerActions.addArrangedSubview(cancelSendButton)
        attachmentsButton.image = NSImage(systemSymbolName: "paperclip", accessibilityDescription: "Attachments")
        attachmentsButton.imagePosition = .imageOnly
        attachmentsButton.isBordered = false
        attachmentsButton.contentTintColor = EmailTheme.muted
        attachmentsButton.setAccessibilityLabel("Attachments")
        attachmentsButton.alphaValue = 0
        attachmentsButton.isEnabled = false
        attachmentsButton.target = self
        attachmentsButton.action = #selector(showAttachments(_:))
        attachmentsButton.translatesAutoresizingMaskIntoConstraints = false
        attachmentsButton.widthAnchor.constraint(equalToConstant: 28).isActive = true
        attachmentsButton.heightAnchor.constraint(equalToConstant: 26).isActive = true
        readerActions.addArrangedSubview(attachmentsButton)
        remoteImagesButton.image = NSImage(systemSymbolName: "photo", accessibilityDescription: nil)
        remoteImagesButton.imagePosition = .imageOnly
        remoteImagesButton.title = ""
        remoteImagesButton.isBordered = false
        remoteImagesButton.contentTintColor = EmailTheme.muted
        remoteImagesButton.toolTip = "Image options. Remote images can notify the sender."
        remoteImagesButton.setAccessibilityLabel("Load images")
        remoteImagesButton.widthAnchor.constraint(equalToConstant: 28).isActive = true
        remoteImagesButton.heightAnchor.constraint(equalToConstant: 26).isActive = true
        remoteImagesButton.isHidden = true
        remoteImagesButton.target = self
        remoteImagesButton.action = #selector(loadImagesClicked(_:))
        readerActions.addArrangedSubview(remoteImagesButton)
        unsubscribeButton.isBordered = false
        unsubscribeButton.contentTintColor = EmailTheme.muted
        unsubscribeButton.font = .systemFont(ofSize: 11, weight: .medium)
        unsubscribeButton.toolTip = "Unsubscribe from this sender (⌘U)"
        unsubscribeButton.isHidden = true
        unsubscribeButton.target = self
        unsubscribeButton.action = #selector(unsubscribeClicked(_:))
        readerActions.addArrangedSubview(unsubscribeButton)
        buildRemoteImagesNotice()
        readerBody.translatesAutoresizingMaskIntoConstraints = false
        bodyText.isEditable = false
        bodyText.isSelectable = true
        bodyText.drawsBackground = false
        bodyText.font = .systemFont(ofSize: 15)
        bodyText.textColor = EmailTheme.ink
        bodyText.textContainerInset = NSSize(width: 6, height: 4)
        bodyScroll.documentView = bodyText
        bodyScroll.hasVerticalScroller = true
        bodyScroll.drawsBackground = false
        bodyScroll.translatesAutoresizingMaskIntoConstraints = false
        readerBody.addSubview(bodyScroll)
        attachmentCard.wantsLayer = true
        attachmentCard.layer?.cornerRadius = 10
        attachmentCard.layer?.borderWidth = 1
        attachmentCard.layer?.borderColor = EmailTheme.cgColor(EmailTheme.line, in: effectiveAppearance)
        attachmentCard.layer?.backgroundColor = EmailTheme.cgColor(EmailTheme.compose, in: effectiveAppearance)
        attachmentCard.translatesAutoresizingMaskIntoConstraints = false
        attachmentCard.isHidden = true
        let attachmentIcon = NSImageView(image: NSImage(systemSymbolName: "doc.zipper", accessibilityDescription: "Attachment") ?? NSImage(systemSymbolName: "doc", accessibilityDescription: "Attachment")!)
        attachmentIcon.contentTintColor = EmailTheme.muted
        attachmentIcon.translatesAutoresizingMaskIntoConstraints = false
        attachmentName.font = .systemFont(ofSize: 13, weight: .semibold)
        attachmentName.textColor = EmailTheme.ink
        attachmentName.lineBreakMode = .byTruncatingMiddle
        attachmentName.translatesAutoresizingMaskIntoConstraints = false
        attachmentMeta.font = .systemFont(ofSize: 11)
        attachmentMeta.textColor = EmailTheme.muted
        attachmentMeta.lineBreakMode = .byTruncatingTail
        attachmentMeta.translatesAutoresizingMaskIntoConstraints = false
        attachmentSave.bezelStyle = .rounded
        attachmentSave.font = .systemFont(ofSize: 12, weight: .medium)
        attachmentSave.target = self
        attachmentSave.action = #selector(saveAttachmentOnlyClicked)
        attachmentSave.translatesAutoresizingMaskIntoConstraints = false
        attachmentCard.addSubview(attachmentIcon)
        attachmentCard.addSubview(attachmentName)
        attachmentCard.addSubview(attachmentMeta)
        attachmentCard.addSubview(attachmentSave)
        readerBody.addSubview(attachmentCard)
        let preferredAttachmentWidth = attachmentCard.widthAnchor.constraint(equalToConstant: 520)
        preferredAttachmentWidth.priority = .defaultHigh
        preferredAttachmentWidth.isActive = true
        NSLayoutConstraint.activate([
            attachmentCard.leadingAnchor.constraint(equalTo: readerBody.leadingAnchor, constant: 6),
            attachmentCard.topAnchor.constraint(equalTo: readerBody.topAnchor, constant: 12),
            attachmentCard.trailingAnchor.constraint(lessThanOrEqualTo: readerBody.trailingAnchor, constant: -6),
            attachmentCard.widthAnchor.constraint(lessThanOrEqualToConstant: 560),
            attachmentCard.heightAnchor.constraint(equalToConstant: 76),
            attachmentIcon.leadingAnchor.constraint(equalTo: attachmentCard.leadingAnchor, constant: 16),
            attachmentIcon.centerYAnchor.constraint(equalTo: attachmentCard.centerYAnchor),
            attachmentIcon.widthAnchor.constraint(equalToConstant: 24),
            attachmentIcon.heightAnchor.constraint(equalToConstant: 26),
            attachmentName.leadingAnchor.constraint(equalTo: attachmentIcon.trailingAnchor, constant: 12),
            attachmentName.trailingAnchor.constraint(lessThanOrEqualTo: attachmentSave.leadingAnchor, constant: -12),
            attachmentName.topAnchor.constraint(equalTo: attachmentCard.topAnchor, constant: 17),
            attachmentMeta.leadingAnchor.constraint(equalTo: attachmentName.leadingAnchor),
            attachmentMeta.trailingAnchor.constraint(equalTo: attachmentName.trailingAnchor),
            attachmentMeta.topAnchor.constraint(equalTo: attachmentName.bottomAnchor, constant: 4),
            attachmentSave.trailingAnchor.constraint(equalTo: attachmentCard.trailingAnchor, constant: -16),
            attachmentSave.centerYAnchor.constraint(equalTo: attachmentCard.centerYAnchor)
        ])
        reader.addSubview(subject)
        reader.addSubview(sender)
        reader.addSubview(account)
        reader.addSubview(readerActions)
        reader.addSubview(remoteImagesNotice)
        reader.addSubview(readerBody)
        emptyState.orientation = .vertical
        emptyState.alignment = .centerX
        emptyState.spacing = 14
        emptyState.translatesAutoresizingMaskIntoConstraints = false
        emptyTitle.font = .systemFont(ofSize: 16, weight: .semibold)
        emptyTitle.textColor = EmailTheme.ink
        emptyTitle.alignment = .center
        emptyState.addArrangedSubview(emptyTitle)
        emptyDetail.font = .systemFont(ofSize: 13)
        emptyDetail.textColor = EmailTheme.muted
        emptyDetail.alignment = .center
        emptyDetail.maximumNumberOfLines = 3
        emptyDetail.lineBreakMode = .byWordWrapping
        emptyDetail.preferredMaxLayoutWidth = 420
        emptyState.addArrangedSubview(emptyDetail)
        emptyDetail.widthAnchor.constraint(equalTo: emptyState.widthAnchor).isActive = true
        emptyState.setCustomSpacing(25, after: emptyDetail)
        emptyAction.target = self
        emptyAction.action = #selector(addAccountClicked)
        emptyAction.isBordered = false
        emptyAction.wantsLayer = true
        emptyAction.layer?.cornerRadius = 9
        paintEmptyAction()
        emptyAction.widthAnchor.constraint(equalToConstant: 144).isActive = true
        emptyAction.heightAnchor.constraint(equalToConstant: 34).isActive = true
        emptyState.addArrangedSubview(emptyAction)
        reader.addSubview(emptyState)
        emptyState.widthAnchor.constraint(lessThanOrEqualTo: reader.widthAnchor, multiplier: 0.75).isActive = true
        let preferredEmptyWidth = emptyState.widthAnchor.constraint(equalToConstant: 440)
        preferredEmptyWidth.priority = .defaultLow
        preferredEmptyWidth.isActive = true
        readerKeyboardHint.translatesAutoresizingMaskIntoConstraints = false
        updateReaderShortcuts()
        reader.addSubview(readerKeyboardHint)
        agentRequestReader.isHidden = true
        agentRequestReader.translatesAutoresizingMaskIntoConstraints = false
        agentRequestReader.onApprove = { [weak self] request in self?.approveAgentRequest(request) }
        agentRequestReader.onReject = { [weak self] request in self?.rejectAgentRequest(request) }
        agentRequestReader.onEdit = { [weak self] request in self?.editAgentRequest(request) }
        agentRequestReader.onSaveAttachment = { [weak self] requestID, attachmentID, filename in
            self?.saveAgentAttachment(requestID: requestID, attachmentID: attachmentID, filename: filename)
        }
        reader.addSubview(agentRequestReader)
        NSLayoutConstraint.activate([
            agentRequestReader.leadingAnchor.constraint(equalTo: reader.leadingAnchor),
            agentRequestReader.trailingAnchor.constraint(equalTo: reader.trailingAnchor),
            agentRequestReader.topAnchor.constraint(equalTo: reader.topAnchor),
            agentRequestReader.bottomAnchor.constraint(equalTo: reader.bottomAnchor),
        ])
        NSLayoutConstraint.activate([
            readerKeyboardHint.leadingAnchor.constraint(equalTo: reader.leadingAnchor, constant: 28),
            readerKeyboardHint.trailingAnchor.constraint(equalTo: reader.trailingAnchor, constant: -28),
            readerKeyboardHint.bottomAnchor.constraint(equalTo: reader.bottomAnchor, constant: -4),
            readerKeyboardHint.heightAnchor.constraint(equalToConstant: 32)
        ])
        let readerBottom = readerBody.bottomAnchor.constraint(equalTo: readerKeyboardHint.topAnchor, constant: -10)
        readerBodyBottomConstraint = readerBottom
        let bodyBelowHeader = readerBody.topAnchor.constraint(equalTo: account.bottomAnchor, constant: 16)
        readerBodyBelowHeader = bodyBelowHeader
        readerBodyBelowNotice = readerBody.topAnchor.constraint(equalTo: remoteImagesNotice.bottomAnchor, constant: 10)
        NSLayoutConstraint.activate([
            subject.leadingAnchor.constraint(equalTo: reader.leadingAnchor, constant: 28), subject.trailingAnchor.constraint(equalTo: reader.trailingAnchor, constant: -28), subject.topAnchor.constraint(equalTo: reader.topAnchor, constant: 26),
            sender.leadingAnchor.constraint(equalTo: subject.leadingAnchor), sender.trailingAnchor.constraint(lessThanOrEqualTo: subject.trailingAnchor), sender.topAnchor.constraint(equalTo: subject.bottomAnchor, constant: 14),
            account.leadingAnchor.constraint(equalTo: subject.leadingAnchor), account.trailingAnchor.constraint(lessThanOrEqualTo: readerActions.leadingAnchor, constant: -12), account.topAnchor.constraint(equalTo: sender.bottomAnchor, constant: 4),
            readerActions.trailingAnchor.constraint(equalTo: subject.trailingAnchor), readerActions.centerYAnchor.constraint(equalTo: account.centerYAnchor), readerActions.heightAnchor.constraint(equalToConstant: 26),
            readerBody.leadingAnchor.constraint(equalTo: reader.leadingAnchor, constant: 22), readerBody.trailingAnchor.constraint(equalTo: reader.trailingAnchor, constant: -22), bodyBelowHeader, readerBottom,
            remoteImagesNotice.leadingAnchor.constraint(equalTo: subject.leadingAnchor), remoteImagesNotice.trailingAnchor.constraint(equalTo: subject.trailingAnchor), remoteImagesNotice.topAnchor.constraint(equalTo: account.bottomAnchor, constant: 14), remoteImagesNotice.heightAnchor.constraint(equalToConstant: 30),
            emptyState.centerXAnchor.constraint(equalTo: reader.centerXAnchor), emptyState.centerYAnchor.constraint(equalTo: reader.centerYAnchor, constant: -10), emptyState.widthAnchor.constraint(lessThanOrEqualToConstant: 440),
            bodyScroll.leadingAnchor.constraint(equalTo: readerBody.leadingAnchor), bodyScroll.trailingAnchor.constraint(equalTo: readerBody.trailingAnchor), bodyScroll.topAnchor.constraint(equalTo: readerBody.topAnchor), bodyScroll.bottomAnchor.constraint(equalTo: readerBody.bottomAnchor)
        ])
        root.addArrangedSubview(reader)
        reader.setContentHuggingPriority(.defaultLow, for: .horizontal)
        reader.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        contactPane.isHidden = true
        contactPane.onRefreshActivity = { [weak self] in self?.updateTrackingButton(refresh: true) }
        contactPane.onClose = { [weak self] in self?.contactPanePreference = false; self?.updateContactPaneVisibility() }
        contactPane.onCompose = { [weak self] address in
            guard let self, let core = self.core else { return }
            self.showComposePane(core: core, recipient: address)
        }
        contactPane.onSearch = { [weak self] address in
            guard let self else { return }
            self.showInbox(scope: .all)
            self.search.stringValue = "from:" + address
            self.refreshInbox()
        }
        contactPane.onOpenThread = { [weak self] id in
            guard let self else { return }
            if let row = self.threads.firstIndex(where: { $0.id == id }) { self.select(row) }
            else {
                let address = ContactPane.address(from: self.sender.stringValue)
                self.showInbox(scope: .all)
                self.pendingContactThreadID = id
                self.search.stringValue = "from:" + address
                self.refreshInbox()
            }
        }
        root.addArrangedSubview(contactPane)
        activityHost.isHidden = true
        activityHost.translatesAutoresizingMaskIntoConstraints = false
        let activityWidth = activityHost.widthAnchor.constraint(equalToConstant: 360)
        activityWidth.priority = .defaultHigh; activityWidth.isActive = true
        activityHost.widthAnchor.constraint(greaterThanOrEqualToConstant: 320).isActive = true
        activityHost.setContentHuggingPriority(.defaultHigh, for: .horizontal)
        root.addArrangedSubview(activityHost)

        let resizeEdge = RightEdgeResizeView()
        container.resizeEdge = resizeEdge
        resizeEdge.translatesAutoresizingMaskIntoConstraints = false
        container.addSubview(resizeEdge, positioned: .above, relativeTo: root)
        NSLayoutConstraint.activate([
            resizeEdge.trailingAnchor.constraint(equalTo: container.trailingAnchor),
            resizeEdge.topAnchor.constraint(equalTo: container.topAnchor),
            resizeEdge.bottomAnchor.constraint(equalTo: container.bottomAnchor),
            resizeEdge.widthAnchor.constraint(equalToConstant: 16)
        ])

        let leftResizeEdge = LeftEdgeResizeView()
        container.leftResizeEdge = leftResizeEdge
        leftResizeEdge.translatesAutoresizingMaskIntoConstraints = false
        container.addSubview(leftResizeEdge, positioned: .above, relativeTo: root)
        NSLayoutConstraint.activate([
            leftResizeEdge.leadingAnchor.constraint(equalTo: container.leadingAnchor),
            leftResizeEdge.topAnchor.constraint(equalTo: container.topAnchor),
            leftResizeEdge.bottomAnchor.constraint(equalTo: container.bottomAnchor),
            leftResizeEdge.widthAnchor.constraint(equalToConstant: 16)
        ])
    }

    func restorePreferredWidth() {
        guard let screen = screen ?? NSScreen.main else { return }
        let savedWidth = UserDefaults.standard.double(forKey: preferredWindowWidthKey)
        let preferredWidth = savedWidth > 0 ? savedWidth : min(1600, screen.visibleFrame.width * 0.92)
        let width = min(screen.visibleFrame.width, max(minSize.width, preferredWidth))
        var frame = self.frame
        frame.size.width = width
        frame.origin.x = screen.visibleFrame.minX + (screen.visibleFrame.width - width) / 2
        setFrame(frame, display: true)
    }

    func applyTheme() {
        let resolved: (NSColor) -> CGColor = { EmailTheme.cgColor($0, in: self.effectiveAppearance) }
        backgroundColor = EmailTheme.canvas
        sidebarPanel?.layer?.backgroundColor = NSWorkspace.shared.accessibilityDisplayShouldReduceTransparency ? resolved(EmailTheme.sidebar) : NSColor.clear.cgColor
        composeSidebarButton?.layer?.backgroundColor = resolved(EmailTheme.accentSoft)
        inboxPanel?.layer?.backgroundColor = resolved(EmailTheme.inbox)
        inboxPanel?.layer?.borderColor = resolved(EmailTheme.line)
        readerPanel?.layer?.backgroundColor = resolved(EmailTheme.reader)
        contactPane.applyTheme()
        readerKeyboardHint.needsDisplay = true
        paintEmptyAction()
        attachmentCard.layer?.backgroundColor = resolved(EmailTheme.compose)
        attachmentCard.layer?.borderColor = resolved(EmailTheme.line)
        remoteImagesNotice.layer?.backgroundColor = resolved(EmailTheme.inbox)
        remoteImagesNotice.layer?.borderColor = resolved(EmailTheme.line)
        emptyAction.layer?.backgroundColor = resolved(EmailTheme.accent)
        table.backgroundColor = EmailTheme.inbox
        inboxScroll.backgroundColor = EmailTheme.inbox
        search.layer?.borderColor = resolved(search.currentEditor() == nil ? EmailTheme.line : EmailTheme.accent)
        composePane?.applyTheme()
        updateSidebarSelection()
        let selected = table.selectedRow
        table.reloadData()
        if selected >= 0 && selected < table.numberOfRows {
            table.selectRowIndexes(IndexSet(integer: selected), byExtendingSelection: false)
        }
        table.needsDisplay = true
        if let html = currentHTML { showHTML(html) }
    }

    private func buildPalette() {
        palette.title = "Commands"
        palette.titleVisibility = .hidden
        palette.titlebarAppearsTransparent = true
        palette.isFloatingPanel = true
        palette.level = .floating
        palette.backgroundColor = EmailTheme.sidebar
        palette.hasShadow = true
        palette.setContentSize(NSSize(width: 540, height: 500))
        paletteHeading.frame = NSRect(x: 28, y: 450, width: 350, height: 18)
        paletteHeading.font = .systemFont(ofSize: 11, weight: .semibold)
        paletteHeading.textColor = EmailTheme.muted
        palette.contentView?.addSubview(paletteHeading)
        paletteEscapeHint.frame = NSRect(x: 392, y: 450, width: 120, height: 18)
        paletteEscapeHint.alignment = .right
        paletteEscapeHint.font = .systemFont(ofSize: 10, weight: .medium)
        paletteEscapeHint.textColor = EmailTheme.muted
        palette.contentView?.addSubview(paletteEscapeHint)
        paletteSearch.placeholderString = "What would you like to do?"
        paletteSearch.frame = NSRect(x: 24, y: 386, width: 492, height: 32)
        paletteSearch.font = .systemFont(ofSize: 16)
        paletteSearch.focusRingType = .none
        paletteSearch.delegate = self
        // Filtering uses text-change notifications; execution is a deliberate
        // Return/search-button action, never the search field's delayed typing action.
        paletteSearch.sendsSearchStringImmediately = false
        paletteSearch.sendsWholeSearchString = true
        paletteSearch.target = self
        paletteSearch.action = #selector(runSelectedPaletteItem(_:))
        palette.contentView?.addSubview(paletteSearch)
        paletteScroll.frame = NSRect(x: 20, y: 44, width: 500, height: 322)
        paletteScroll.hasVerticalScroller = true
        paletteScroll.autohidesScrollers = true
        paletteScroll.drawsBackground = false
        paletteScroll.borderType = .noBorder
        paletteRows.frame = NSRect(x: 0, y: 0, width: 484, height: 322)
        paletteScroll.documentView = paletteRows
        palette.contentView?.addSubview(paletteScroll)
        paletteFooter.frame = NSRect(x: 28, y: 12, width: 484, height: 18)
        paletteFooter.alignment = .right
        paletteFooter.font = .systemFont(ofSize: 10)
        paletteFooter.textColor = EmailTheme.muted
        palette.contentView?.addSubview(paletteFooter)
        refreshPalette()
    }

    private func paletteItem(_ target: PaletteTarget, _ title: String, _ section: String,
                             shortcut: String = "", detail: String = "", aliases: [String] = []) -> PaletteItem {
        PaletteItem(target: target, title: title, section: section, shortcut: shortcut, detail: detail, aliases: aliases)
    }

    /// Scheduling commands exist only when the profile names a Scheduling service.
    private func paletteItems() -> [PaletteItem] {
        let scheduling = DistributionProfile.current?.hasScheduling == true
        return allPaletteItems().filter { scheduling || !$0.target.requiresScheduling }
    }

    private func allPaletteItems() -> [PaletteItem] {
        var items: [PaletteItem] = [paletteItem(.bookings, "Bookings and meeting links", "SCHEDULING", aliases: ["booking", "analytics", "scheduling"])]
        if offersOpenActivity { items.insert(paletteItem(.trackingActivity, "Recent opens", "ANALYTICS", aliases: ["tracking", "opens", "email analytics", "awaiting reply"]), at: 0) }
        switch palettePage {
        case .root:
            if calendarWorkspace != nil {
                return [
                    paletteItem(.calendarMove(1), "Next calendar period", "NAVIGATE", shortcut: "L / →", aliases: ["next week", "next month", "next year", "next day"]),
                    paletteItem(.calendarMove(-1), "Previous calendar period", "NAVIGATE", shortcut: "H / ←", aliases: ["last week", "previous month", "previous year", "previous day"]),
                    paletteItem(.calendarToday, "Today", "NAVIGATE", shortcut: "T"),
                    paletteItem(.calendarJump, "Go to date…", "NAVIGATE", shortcut: "B"),
                    paletteItem(.calendarView(0), "Day view", "VIEW", shortcut: "D / 1"),
                    paletteItem(.calendarView(1), "Week view", "VIEW", shortcut: "W / 2"),
                    paletteItem(.calendarView(2), "Month view", "VIEW", shortcut: "M / 3"),
                    paletteItem(.calendarView(3), "Year view", "VIEW", shortcut: "Y / 4"),
                    paletteItem(.bookings, "Bookings and meeting links", "SCHEDULING", aliases: ["booking", "scheduling", "analytics"]),
                    paletteItem(.calendarHelp, "Calendar keyboard shortcuts", "HELP", shortcut: "?"),
                    paletteItem(.calendarSearch, "Search calendar", "CALENDAR", shortcut: "/"),
                    paletteItem(.calendarCreate, "New event", "CALENDAR", shortcut: "C"),
                    paletteItem(.calendarRefresh, "Refresh calendar", "CALENDAR", shortcut: "⌘R"),
                    paletteItem(.inbox, "Go to Inbox", "NAVIGATE", shortcut: "G I"),
                    paletteItem(.compose, "Compose email", "MAIL"),
                    paletteItem(.shortcuts, "Keyboard shortcuts", "HELP", shortcut: "?")
                ]
            }
            if composePane != nil {
                if composePane?.offersTracking == true { items.append(paletteItem(.toggleTracking, "Toggle email open tracking", "COMPOSE", aliases: ["track opens", "pixel", "analytics"])) }
                items.append(paletteItem(.insertBooking, "Insert booking link…", "COMPOSE", aliases: ["meeting", "schedule", "availability"]))
                items.append(paletteItem(.signature, "Signature…", "COMPOSE", aliases: ["signatures", "identity", "footer"]))
                items.append(paletteItem(.sendDraft, "Send message", "SUGGESTED", detail: "10 seconds to undo", aliases: ["send email", "deliver"]))
                items.append(paletteItem(.focusBody, "Write message", "SUGGESTED", aliases: ["body", "message text"]))
                items.append(paletteItem(.chooseFrom, "Choose From address", "COMPOSE", aliases: ["change sender", "switch identity", "send as", "alias"]))
                items.append(paletteItem(.focusRecipient, "Edit recipient", "COMPOSE", aliases: ["to", "address"]))
                items.append(paletteItem(.focusCc, "Edit Cc recipients", "COMPOSE", aliases: ["copy", "carbon copy"]))
                items.append(paletteItem(.focusBcc, "Edit Bcc recipients", "COMPOSE", aliases: ["blind copy", "hidden recipients"]))
                items.append(paletteItem(.attachFile, "Attach file", "COMPOSE", aliases: ["attachment", "upload"]))
                items.append(paletteItem(.snippets, "Insert snippet…", "COMPOSE", aliases: ["template", "saved reply", "reuse text"]))
                items.append(paletteItem(.saveSnippet, "Save message as snippet…", "COMPOSE", aliases: ["create template", "save reply"]))
                items.append(paletteItem(.scheduleDraft, "Schedule send", "COMPOSE", aliases: ["send later"]))
                items.append(paletteItem(.saveDraft, "Save draft", "COMPOSE", aliases: ["save message"]))
                items.append(paletteItem(.discardComposerDraft, "Discard draft", "COMPOSE", detail: "Z in Drafts to undo", aliases: ["delete draft", "trash draft", "remove draft", "throw away"]))
                items.append(paletteItem(.shortcuts, "Keyboard shortcuts", "HELP", shortcut: "?"))
                return items
            }
            if folder == .drafts {
                if lastDiscardedDraft != nil {
                    items.append(paletteItem(.undoDiscardDraft, "Undo discard", "SUGGESTED", shortcut: "Z", detail: "Restore the last discarded draft", aliases: ["undo", "restore draft", "recover draft", "undelete"]))
                }
                if filteredDrafts.indices.contains(table.selectedRow) {
                    items.append(paletteItem(.editDraft, "Edit draft", "SUGGESTED", shortcut: "↵", aliases: ["open draft", "continue writing"]))
                    items.append(paletteItem(.discardDraft, "Discard draft", "ACTIONS", shortcut: "⌫", detail: "Z to undo", aliases: ["delete draft", "trash draft", "remove draft", "throw away"]))
                }
            }
            if let _ = lastUndoBatch, folder == .inbox {
                items.append(paletteItem(.undoTriage, "Undo last move", "SUGGESTED", shortcut: "Z", detail: "Restore the original folders", aliases: ["undo", "unarchive", "recover"]))
            }
            if !markedThreadIDs.isEmpty {
                let count = markedThreadIDs.count
                items.append(paletteItem(.markRead, "Mark \(count) selected as read", "SELECTION", shortcut: "M", aliases: ["bulk read", "mass mark read", "mark all read"]))
                items.append(paletteItem(.markUnread, "Mark \(count) selected as unread", "SELECTION", shortcut: "U"))
                items.append(paletteItem(.archive, inboxScope == .archived ? "Move \(count) selected to Inbox" : "Archive \(count) selected", "SELECTION", shortcut: inboxScope == .archived ? "⇧E" : "E"))
                items.append(paletteItem(.spam, "Move \(count) selected to Spam", "SELECTION", shortcut: "!", aliases: ["junk"]))
                items.append(paletteItem(.selectLoaded, "Select all loaded conversations", "SELECTION", shortcut: "⌘A"))
                items.append(paletteItem(.clearSelection, "Clear selection", "SELECTION", shortcut: "Esc"))
                return items
            }
            if folder == .inbox && !threads.isEmpty {
                items.append(paletteItem(.selectLoaded, "Select all loaded conversations", "ACTIONS", shortcut: "⌘A", aliases: ["select", "bulk", "mass"]))
            }
            let selected = folder == .inbox && threads.indices.contains(selection) && threads[selection].id != nil
            if selected {
                let thread = threads[selection]
                items.append(paletteItem(.reply, "Reply", "SUGGESTED", shortcut: "R", aliases: ["respond", "answer"]))
                items.append(paletteItem(.replyAll, "Reply all", "ACTIONS", shortcut: "⇧R", aliases: ["respond everyone", "reply group"]))
                items.append(paletteItem(.archive, inboxScope == .archived ? "Move to Inbox" : "Archive", "SUGGESTED", shortcut: inboxScope == .archived ? "⇧E" : "E", detail: inboxScope == .archived ? "Restore to the inbox" : "Remove from the inbox", aliases: ["done", "clear", "finish"]))
                items.append(paletteItem(thread.unread ? .markRead : .markUnread,
                                         thread.unread ? "Mark as read" : "Mark as unread", "SUGGESTED",
                                         shortcut: thread.unread ? "M" : "U", aliases: ["read", "unread"]))
                items.append(paletteItem(.bulkReadSender, "Bulk mark sender as read…", "ACTIONS", detail: "Preview all unread mail from this sender, including older Gmail messages", aliases: ["bulk", "newsletter", "cleanup", "mass read", "mark all read", "sender cleanup"]))
                items.append(paletteItem(.bulkReadSenderDownloaded, "Mark downloaded mail from sender as read…", "ACTIONS", detail: "Only mail already saved on this device", aliases: ["downloaded sender", "local cleanup"]))
                items.append(paletteItem(.favorite, thread.pinned ? "Remove from favorites" : "Add to favorites",
                                         "ACTIONS", shortcut: "F", aliases: ["star", "pin", "important"]))
                items.append(paletteItem(.labels, "Add or remove label", "ACTIONS", shortcut: "L", aliases: ["tag", "categorize"]))
                items.append(paletteItem(thread.snoozedUntil == nil ? .snooze : .unsnooze,
                                         thread.snoozedUntil == nil ? "Snooze" : "Remove snooze", "ACTIONS",
                                         shortcut: thread.snoozedUntil == nil ? "S" : "", aliases: ["later", "remind me"]))
                items.append(paletteItem(.spam, "Move to Spam", "ACTIONS", shortcut: "!", aliases: ["junk", "report spam"]))
                if unsubscribeURL != nil {
                    items.append(paletteItem(.unsubscribe, "Unsubscribe", "ACTIONS", shortcut: "⌘U", aliases: ["mailing list", "newsletter"]))
                }
                items.append(paletteItem(.contact, contactPane.isHidden ? "Show contact" : "Hide contact", "ACTIONS", shortcut: "0", aliases: ["sender", "person"]))
            }
            items.append(paletteItem(.search, "Search mail", selected ? "ACTIONS" : "SUGGESTED", shortcut: "/", aliases: ["find", "lookup"]))
            items.append(paletteItem(.compose, "New message", selected ? "ACTIONS" : "SUGGESTED", shortcut: "C", aliases: ["compose", "write email", "draft"]))
            items.append(paletteItem(.sync, "Sync all accounts", "ACTIONS", shortcut: "⌘R", aliases: ["refresh", "fetch mail"]))
            items.append(paletteItem(.inbox, "Go to Inbox", "GO TO", shortcut: "G I", aliases: ["all mail", "all inboxes"]))
            items.append(paletteItem(.unread, "Go to Unread", "GO TO", shortcut: "G U"))
            items.append(paletteItem(.favorites, "Go to Favorites", "GO TO", shortcut: "G F", aliases: ["starred", "pinned"]))
            items.append(paletteItem(.snoozed, "Go to Snoozed", "GO TO", shortcut: "G S"))
            items.append(paletteItem(.drafts, "Go to Drafts", "GO TO", shortcut: "G D"))
            items.append(paletteItem(.sent, "Go to Sent", "GO TO", shortcut: "G T", aliases: ["sent mail", "sent messages"]))
            items.append(paletteItem(.outbox, "Go to Outbox", "GO TO", shortcut: "G O", aliases: ["scheduled", "undo send"]))
            if agentAccessOn || agentRequestTotal > 0 {
                items.append(paletteItem(.agentRequests, "Go to Agent requests", "GO TO", detail: agentRequestTotal > 0 ? "\(agentRequestTotal) waiting" : "", aliases: ["approve", "approval", "agent", "claude", "codex"]))
            }
            for split in splits {
                items.append(paletteItem(.split(split.id), "Go to \(split.name)", "GO TO", detail: "Split inbox", aliases: [split.query]))
            }
            for account in knownAccounts {
                items.append(paletteItem(.account(account.id), "Go to \(account.email)", "GO TO", detail: "Account inbox", aliases: [account.displayName]))
            }
            for label in paletteLabels {
                items.append(paletteItem(.goToLabel(label), "Go to #\(label)", "GO TO", detail: "Label", aliases: [label]))
            }
            items.append(paletteItem(.browseLabels, "Browse labels", "GO TO", aliases: ["go to label", "folders"]))
            items.append(paletteItem(.switchAccount, "Switch account", "GO TO", aliases: ["accounts", "change account"]))
            items.append(paletteItem(.newSplit, "New split inbox", "SETTINGS", aliases: ["create split", "saved search"]))
            items.append(paletteItem(.addAccount, "Add account", "SETTINGS", aliases: ["connect email", "sign in"]))
            items.append(paletteItem(.notifications, MailNotifications.shared.enabled ? "Disable notifications" : "Enable notifications", "SETTINGS", aliases: ["alerts", "banners", "notifications"]))
            items.append(paletteItem(.testNotifications, "Send test notification", "SETTINGS", aliases: ["test alerts", "test banner", "notifications"]))
            items.append(paletteItem(.notificationPreviews, MailNotifications.shared.previews ? "Hide sender and subject in notifications" : "Show sender and subject in notifications", "SETTINGS", aliases: ["notification previews", "alert privacy"]))
            items.append(paletteItem(.manageAccounts, "Manage accounts", "SETTINGS", aliases: ["account settings"]))
            items.append(paletteItem(.agentAccess, "Agent access", "SETTINGS", aliases: ["mcp", "ai", "agent", "codex", "claude", "assistant", "connect"]))
            items.append(paletteItem(.toggleImages, RemoteImagePreference.isEnabled(.standard) ? "Block remote images by default" : "Load remote images by default", "SETTINGS", shortcut: "I", aliases: ["privacy", "pictures", "tracking"]))
            items.append(paletteItem(.appearance(0), "Appearance: Follow System", "SETTINGS", aliases: ["theme", "system"]))
            items.append(paletteItem(.appearance(1), "Appearance: Light", "SETTINGS", aliases: ["theme", "light mode"]))
            items.append(paletteItem(.appearance(2), "Appearance: Dark", "SETTINGS", aliases: ["theme", "dark mode"]))
            items.append(paletteItem(.shortcuts, "Keyboard shortcuts", "SETTINGS", shortcut: "?", aliases: ["help", "keys"]))
            items.append(paletteItem(.support, "Ask for support…", "HELP", aliases: ["help", "bug", "logs", "screenshot"]))
            items.append(paletteItem(.feedback, "Give feedback…", "HELP", aliases: ["suggestion", "idea", "request"]))
            items.append(paletteItem(.about, "About Email", "SETTINGS", detail: EmailBuildIdentity.value, aliases: ["version", "build", "updated"]))
            items.append(paletteItem(.searchFilters, "Refine search…", "MAIL", aliases: ["filters", "advanced search", "from", "to", "cc", "dates", "attachments"]))
            items.append(paletteItem(.searchHelp, "Search syntax and coverage", "MAIL", aliases: ["from", "to", "cc", "label", "local", "cache", "operators"]))
            if calendarAvailable { items.append(paletteItem(.invitations, "Calendar invitations…", "CALENDAR", aliases: ["agenda", "meeting", "ics", "event"])) }
            items.append(paletteItem(.followup, "Follow up unless they reply…", "MAIL", aliases: ["waiting", "reminder", "followup"]))
            items.append(paletteItem(.followups, "Go to Follow-ups", "GO TO", shortcut: "G W", aliases: ["waiting for reply", "due"]))
            items.append(paletteItem(.cleanupJobs, "Cleanup history…", "MAIL", aliases: ["jobs", "progress", "resume cleanup"]))
        case .labels:
            let active = threads.indices.contains(selection) ? Set(threads[selection].labels) : []
            for label in paletteLabels {
                items.append(paletteItem(.toggleLabel(label), label, "LABELS", detail: active.contains(label) ? "Applied · Enter to remove" : "Enter to apply", aliases: ["tag"] ))
            }
            let name = paletteSearch.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
            if !name.isEmpty && !paletteLabels.contains(where: { $0.caseInsensitiveCompare(name) == .orderedSame }) {
                items.append(paletteItem(.createLabel(name), "Create label “\(name)”", "CREATE", detail: "Add to this email"))
            }
        case .labelDestinations:
            for label in paletteLabels {
                items.append(paletteItem(.goToLabel(label), label, "LABELS", detail: "Open matching mail"))
            }
        case .snooze:
            let now = Date()
            let calendar = Calendar.current
            let tomorrow = calendar.date(byAdding: .day, value: 1, to: calendar.startOfDay(for: now)) ?? now.addingTimeInterval(86_400)
            let morning = calendar.date(byAdding: .hour, value: 9, to: tomorrow) ?? tomorrow
            for (index, choice) in [
                ("In 3 hours", now.addingTimeInterval(10_800)),
                ("Tomorrow morning", morning),
                ("Next week", calendar.date(byAdding: .day, value: 7, to: now) ?? now.addingTimeInterval(604_800))
            ].enumerated() {
                items.append(paletteItem(.snoozeUntil(choice.1), choice.0, "SNOOZE UNTIL",
                                         shortcut: String(index + 1),
                                         detail: DateFormatter.localizedString(from: choice.1, dateStyle: .medium, timeStyle: .short)))
            }
        case .snippets:
            let snippets = (core?.call(["op":"list_snippets"]) as? [[String:Any]]) ?? []
            for snippet in snippets {
                guard let name = snippet["name"] as? String, let body = snippet["body"] as? String else { continue }
                items.append(paletteItem(.insertSnippet(body), name, "INSERT SNIPPET", detail: String(body.replacingOccurrences(of: "\n", with: " ").prefix(100))))
            }
        case .accounts:
            for account in knownAccounts {
                items.append(paletteItem(.account(account.id), account.email, "ACCOUNTS", detail: account.provider, aliases: [account.displayName]))
            }
            items.append(paletteItem(.addAccount, "Add account", "ACCOUNTS"))
        }
        return items
    }

    private func paletteNormalized(_ value: String) -> String {
        let folded = value.folding(options: [.caseInsensitive, .diacriticInsensitive], locale: .current)
        return folded.split(whereSeparator: { !$0.isLetter && !$0.isNumber }).joined(separator: " ")
    }

    private func paletteScore(_ item: PaletteItem, query: String) -> Int? {
        let title = paletteNormalized(item.title)
        let aliases = item.aliases.map(paletteNormalized)
        if title == query { return 1000 }
        if title.hasPrefix(query) { return 850 }
        if title.split(separator: " ").contains(where: { $0.hasPrefix(query) }) { return 700 }
        if aliases.contains(where: { $0 == query }) { return 680 }
        if aliases.contains(where: { $0.hasPrefix(query) }) { return 620 }
        if title.contains(query) { return 510 }
        if aliases.contains(where: { $0.contains(query) }) { return 440 }
        let queryWords = query.split(separator: " ").map(String.init)
        let candidateWords = ([title] + aliases).flatMap { $0.split(separator: " ").map(String.init) }
        if !queryWords.isEmpty && queryWords.allSatisfy({ word in
            candidateWords.contains { candidate in
                let tolerance = word.count >= 5 ? 2 : 1
                return candidate.hasPrefix(word) ||
                    (word.count >= 4 && abs(word.count - candidate.count) <= tolerance &&
                     paletteEditDistance(word, candidate) <= tolerance)
            }
        }) { return 310 }
        let haystack = title + " " + aliases.joined(separator: " ") + " " + paletteNormalized(item.detail)
        let words = query.split(separator: " ")
        if words.allSatisfy({ word in haystack.split(separator: " ").contains(where: { $0.hasPrefix(word) }) }) { return 360 }
        guard query.count >= 3 else { return nil }
        var remaining = haystack[...]
        var gaps = 0
        for character in query {
            guard let found = remaining.firstIndex(of: character) else { return nil }
            gaps += remaining.distance(from: remaining.startIndex, to: found)
            remaining = remaining[remaining.index(after: found)...]
        }
        return gaps <= query.count * 3 ? max(1, 180 - gaps) : nil
    }

    private func paletteEditDistance(_ left: String, _ right: String) -> Int {
        let a = Array(left), b = Array(right)
        var previous = Array(0...b.count)
        for (index, character) in a.enumerated() {
            var current = [index + 1] + Array(repeating: 0, count: b.count)
            for (column, other) in b.enumerated() {
                current[column + 1] = min(previous[column + 1] + 1,
                                          current[column] + 1,
                                          previous[column] + (character == other ? 0 : 1))
            }
            previous = current
        }
        return previous[b.count]
    }

    private func reminderForQuery(_ query: String) -> PaletteItem? {
        guard calendarWorkspace == nil else { return nil }
        guard folder == .inbox, threads.indices.contains(selection), threads[selection].id != nil else { return nil }
        if let date = SnoozeDateQuery.parse(query) {
            let when = DateFormatter.localizedString(from: date, dateStyle: .medium, timeStyle: .short)
            let normalized = SnoozeDateQuery.normalize(query)
            let label = normalized.hasPrefix("today") ? "Today" : (Calendar.current.isDateInToday(date) ? "Tonight" : "Tomorrow")
            let clock = DateFormatter.localizedString(from: date, dateStyle: .none, timeStyle: .short)
            return paletteItem(.snoozeUntil(date), "\(label) at \(clock)", "SUGGESTED", detail: "Snooze until \(when) · Return to confirm")
        }
        var words = query.split(separator: " ").map(String.init)
        guard let verb = words.first,
              ["remind", "snooze", "snooz", "snoz", "snoze"].contains(verb) ||
                (verb.count >= 4 && ["remind", "snooze"].contains(where: { paletteEditDistance(verb, $0) <= 2 })) else { return nil }
        words.removeFirst()
        if words.first == "me" { words.removeFirst() }
        if words.first == "in" || words.first == "for" { words.removeFirst() }
        guard (1...2).contains(words.count) else { return nil }
        let quantity = words[0]
        let digits = String(quantity.prefix(while: { $0.isNumber }))
        guard let amount = Int(digits), amount > 0 else { return nil }
        let unit = words.count == 2 ? words[1] : String(quantity.dropFirst(digits.count))
        let seconds: TimeInterval
        let unitName: String
        switch unit {
        case "", "m", "minute", "minutes", "min", "mins": seconds = 60; unitName = amount == 1 ? "minute" : "minutes"
        case "h", "hour", "hours", "hr", "hrs": seconds = 3_600; unitName = amount == 1 ? "hour" : "hours"
        case "d", "day", "days": seconds = 86_400; unitName = amount == 1 ? "day" : "days"
        case "w", "week", "weeks": seconds = 604_800; unitName = amount == 1 ? "week" : "weeks"
        default: return nil
        }
        guard Double(amount) * seconds <= 366 * 86_400 else { return nil }
        let date = Date().addingTimeInterval(Double(amount) * seconds)
        let when = DateFormatter.localizedString(from: date, dateStyle: .medium, timeStyle: .short)
        return paletteItem(.snoozeUntil(date), "Remind me in \(amount) \(unitName)", "SUGGESTED", detail: "Snooze until \(when)")
    }

    private func snoozeSuggestion(_ raw: String) -> PaletteItem? {
        guard palettePage == .snooze, folder == .inbox,
              threads.indices.contains(selection), threads[selection].id != nil else { return nil }
        let query = SnoozeDateQuery.normalize(raw)
        guard !query.isEmpty else { return nil }
        if let duration = reminderForQuery("snooze \(query)"), case .snoozeUntil(let until) = duration.target {
            let when = DateFormatter.localizedString(from: until, dateStyle: .medium, timeStyle: .short)
            return paletteItem(.snoozeUntil(until),
                               duration.title.replacingOccurrences(of: "Remind me in ", with: "In "),
                               "YOUR TIME", detail: "Until \(when)")
        }
        let date: Date?
        let title: String
        if query.hasPrefix("tomorrow") {
            let time = query.dropFirst("tomorrow".count).trimmingCharacters(in: .whitespaces)
            let tomorrow = Calendar.current.date(byAdding: .day, value: 1, to: Date()) ?? Date().addingTimeInterval(86_400)
            date = snoozeTime(time.isEmpty ? "9" : time, on: tomorrow)
            title = "Tomorrow at \(date.map { DateFormatter.localizedString(from: $0, dateStyle: .none, timeStyle: .short) } ?? "09:00")"
        } else {
            let formats = ["yyyy-MM-dd HH:mm", "d/M/yyyy HH:mm", "d MMM yyyy HH:mm", "d MMM HH:mm"]
            var parsed: Date?
            for format in formats {
                let formatter = DateFormatter()
                formatter.locale = .current
                formatter.isLenient = false
                formatter.dateFormat = format
                let input = format == "d MMM HH:mm" ? "\(query) \(Calendar.current.component(.year, from: Date()))" : query
                formatter.dateFormat = format == "d MMM HH:mm" ? "d MMM HH:mm yyyy" : format
                if let value = formatter.date(from: input) { parsed = value; break }
            }
            date = parsed
            title = parsed.map { "On \(DateFormatter.localizedString(from: $0, dateStyle: .medium, timeStyle: .short))" } ?? ""
        }
        guard let date, date > Date() else { return nil }
        let when = DateFormatter.localizedString(from: date, dateStyle: .medium, timeStyle: .short)
        return paletteItem(.snoozeUntil(date), title, "YOUR TIME", detail: "Until \(when)")
    }

    private func snoozeTime(_ value: String, on day: Date) -> Date? {
        let pieces = value.trimmingCharacters(in: .whitespaces).split(separator: ":")
        guard (1...2).contains(pieces.count), let hour = Int(pieces[0]), (0...23).contains(hour) else { return nil }
        let minute = pieces.count == 2 ? Int(pieces[1]) : 0
        guard let minute, (0...59).contains(minute) else { return nil }
        return Calendar.current.date(bySettingHour: hour, minute: minute, second: 0, of: day)
    }

    private func calendarDateSuggestion(_ raw: String) -> PaletteItem? {
        guard calendarWorkspace != nil, palettePage == .root,
              let date = AgendaDateQuery.parse(raw) else { return nil }
        let formatter = DateFormatter()
        formatter.dateFormat = "EEEE, d MMMM yyyy"
        let day = formatter.string(from: date)
        return paletteItem(.calendarDate(date), "Go to \(day)", "DATE", shortcut: "↵",
                           detail: "Open this date in your current calendar view")
    }

    private func refreshPalette() {
        let paletteHeight: CGFloat = palettePage == .snooze ? 390 : 500
        if palette.contentView?.frame.height != paletteHeight {
            palette.setContentSize(NSSize(width: 540, height: paletteHeight))
        }
        paletteHeading.frame.origin.y = paletteHeight - 50
        paletteEscapeHint.frame.origin.y = paletteHeight - 50
        paletteSearch.frame.origin.y = paletteHeight - 114
        paletteScroll.frame.size.height = paletteHeight - 178
        let previouslySelected = paletteResults.indices.contains(paletteSelection) ? paletteResults[paletteSelection].target : nil
        paletteRows.subviews.forEach { $0.removeFromSuperview() }
        paletteButtons.removeAll()
        let query = paletteNormalized(paletteSearch.stringValue.trimmingCharacters(in: .whitespacesAndNewlines))
        let items = paletteItems()
        if query.isEmpty {
            paletteResults = Array(items.filter { item in
                if case .goToLabel = item.target { return palettePage != .root }
                return true
            }.prefix(80))
        } else {
            let ranked = items.compactMap { item -> (PaletteItem, Int)? in
                guard let score = paletteScore(item, query: query) else { return nil }
                return (item, score)
            }.sorted { lhs, rhs in
                lhs.1 == rhs.1 ? lhs.0.title.localizedStandardCompare(rhs.0.title) == .orderedAscending : lhs.1 > rhs.1
            }.prefix(40).map(\.0)
            paletteResults = ((palettePage == .snooze ? snoozeSuggestion(paletteSearch.stringValue) : (palettePage == .root ? (calendarDateSuggestion(paletteSearch.stringValue) ?? reminderForQuery(query)) : nil)).map { [$0] } ?? []) + ranked
        }
        paletteSelection = paletteResults.firstIndex(where: { $0.target == previouslySelected }) ?? 0
        paletteHeading.stringValue = switch palettePage {
        case .root: "COMMANDS"
        case .labels: "LABEL THIS EMAIL"
        case .labelDestinations: "GO TO LABEL"
        case .snooze: "SNOOZE UNTIL"
        case .accounts: "SWITCH ACCOUNT"
        case .snippets: "INSERT SNIPPET"
        }
        paletteEscapeHint.stringValue = palettePage == .root ? "ESC TO CLOSE" : "ESC TO GO BACK"
        paletteSearch.placeholderString = palettePage == .snooze ? "Try 5m, tomorrow 9, or 12 Nov 14:30" :
            (palettePage == .root ? "What would you like to do?" : "Search \(paletteHeading.stringValue.lowercased())")
        paletteFooter.stringValue = palettePage == .snooze
            ? (paletteSearch.stringValue.isEmpty ? "1–3 choose   ↑ ↓ navigate   ↵ snooze   esc back" : "↵ snooze   esc back")
            : "↑ ↓ navigate   ↵ run   esc close"
        var offset: CGFloat = 6
        var lastSection = ""
        for (position, item) in paletteResults.enumerated() {
            if query.isEmpty, item.section != lastSection {
                let section = ShortcutBadge(labelWithString: item.section)
                section.frame = NSRect(x: 18, y: offset + 5, width: 430, height: 17)
                section.font = .systemFont(ofSize: 10, weight: .semibold)
                section.textColor = EmailTheme.muted
                paletteRows.addSubview(section)
                offset += 27
                lastSection = item.section
            }
            let rowHeight: CGFloat = item.detail.isEmpty ? 44 : 54
            let button = NSButton(title: "", target: self, action: #selector(runPaletteButton(_:)))
            button.tag = position
            button.frame = NSRect(x: 6, y: offset, width: 470, height: rowHeight)
            button.isBordered = false
            button.setAccessibilityLabel(item.title)
            button.wantsLayer = true
            button.layer?.cornerRadius = 8
            let title = ShortcutBadge(labelWithString: item.title)
            title.frame = NSRect(x: 14, y: item.detail.isEmpty ? 12 : 7, width: 350, height: 20)
            title.font = .systemFont(ofSize: 14, weight: .medium)
            title.textColor = EmailTheme.ink
            title.lineBreakMode = .byTruncatingTail
            button.addSubview(title)
            if !item.detail.isEmpty {
                let detail = ShortcutBadge(labelWithString: item.detail)
                detail.frame = NSRect(x: 14, y: 29, width: 350, height: 16)
                detail.font = .systemFont(ofSize: 11)
                detail.textColor = EmailTheme.muted
                detail.lineBreakMode = .byTruncatingTail
                button.addSubview(detail)
            }
            if !item.shortcut.isEmpty {
                let badge = ShortcutBadge(labelWithString: item.shortcut)
                badge.frame = NSRect(x: 386, y: (rowHeight - 18) / 2, width: 66, height: 18)
                badge.alignment = .right
                badge.font = .monospacedSystemFont(ofSize: 11, weight: .medium)
                badge.textColor = EmailTheme.muted
                button.addSubview(badge)
            }
            paletteRows.addSubview(button)
            paletteButtons.append(button)
            offset += rowHeight + 4
        }
        if paletteResults.isEmpty {
            let message = switch palettePage {
            case .labels: "Type a label name to create it"
            case .accounts: "No accounts match"
            case .snippets: "Save a message as a snippet from Commands"
            case .snooze: "Try 5m, tomorrow 9, or 12 Nov 14:30"
            default: "No matching commands"
            }
            let empty = NSTextField(labelWithString: message)
            empty.frame = NSRect(x: 22, y: 28, width: 440, height: 24)
            empty.font = .systemFont(ofSize: 14)
            empty.textColor = EmailTheme.muted
            paletteRows.addSubview(empty)
        }
        paletteRows.frame = NSRect(x: 0, y: 0, width: 484, height: max(322, offset + 6))
        updatePaletteSelection()
        if query.isEmpty { paletteScroll.contentView.scroll(to: .zero) }
        paletteScroll.reflectScrolledClipView(paletteScroll.contentView)
    }

    private func updatePaletteSelection() {
        for (position, button) in paletteButtons.enumerated() {
            button.layer?.backgroundColor = position == paletteSelection
                ? EmailTheme.cgColor(EmailTheme.accentSoft, in: effectiveAppearance)
                : NSColor.clear.cgColor
        }
        if paletteButtons.indices.contains(paletteSelection) {
            paletteButtons[paletteSelection].scrollToVisible(paletteButtons[paletteSelection].bounds)
        }
    }

    @objc private func runSelectedPaletteItem(_ sender: Any?) {
        guard palette.isVisible, paletteResults.indices.contains(paletteSelection) else { return }
        runPaletteItem(paletteResults[paletteSelection])
    }

    // Native field-editor command routing also works when a nonactivating panel
    // receives an event without becoming NSApp's reported key window.
    func control(_ control: NSControl, textView: NSTextView, doCommandBy commandSelector: Selector) -> Bool {
        guard control === paletteSearch else { return false }
        switch NSStringFromSelector(commandSelector) {
        case "insertNewline:", "insertLineBreak:": runSelectedPaletteItem(control); return true
        case "moveDown:":
            paletteSelection = min(paletteSelection + 1, max(0, paletteResults.count - 1)); updatePaletteSelection(); return true
        case "moveUp:":
            paletteSelection = max(0, paletteSelection - 1); updatePaletteSelection(); return true
        case "cancelOperation:": backOrClosePalette(); return true
        default: return false
        }
    }

    @objc private func runPaletteButton(_ sender: NSButton) {
        guard paletteResults.indices.contains(sender.tag) else { return }
        runPaletteItem(paletteResults[sender.tag])
    }

    private func openPalettePage(_ page: PalettePage) {
        if palettePage == .root { paletteRootQuery = paletteSearch.stringValue }
        palettePage = page
        paletteResults = []
        paletteSearch.stringValue = ""
        refreshPalette()
        paletteSearch.becomeFirstResponder()
    }

    private func closePalette() {
        palette.orderOut(nil)
        makeKeyAndOrderFront(nil)
        if let paletteReturnResponder { makeFirstResponder(paletteReturnResponder) }
    }

    private func backOrClosePalette() {
        if palettePage == .root { closePalette(); return }
        palettePage = .root
        paletteResults = []
        paletteSearch.stringValue = paletteRootQuery
        refreshPalette()
        paletteSearch.becomeFirstResponder()
    }

    private func runPaletteItem(_ item: PaletteItem) {
        if calendarWorkspace != nil {
            switch item.target {
            case .calendarSearch, .calendarCreate, .calendarRefresh, .calendarView, .calendarMove, .calendarToday, .calendarJump, .calendarHelp, .calendarDate, .inbox, .compose, .shortcuts: break
            default: return
            }
        }
        switch item.target {
        case .labels: openPalettePage(.labels); return
        case .browseLabels: openPalettePage(.labelDestinations); return
        case .snooze: openPalettePage(.snooze); return
        case .switchAccount: openPalettePage(.accounts); return
        case .snippets: openPalettePage(.snippets); return
        default: break
        }
        closePalette()
        switch item.target {
        case .calendarView(let index): calendarWorkspace?.setMode(index)
        case .calendarMove(let offset): calendarWorkspace?.move(offset)
        case .calendarDate(let date): calendarWorkspace?.goToDate(date)
        case .calendarToday: calendarWorkspace?.goToday()
        case .calendarJump: calendarWorkspace?.jumpToDate()
        case .calendarHelp: calendarWorkspace?.showNavigationHelp()
        case .calendarSearch: calendarWorkspace?.focusSearch()
        case .calendarCreate: calendarWorkspace?.createEvent()
        case .calendarRefresh: calendarWorkspace?.refreshEvents()
        case .support: showSupport()
        case .feedback: showSupport(feedback: true)
        case .about: showNotice("Email 0.1.0\nBuild: \(EmailBuildIdentity.value)")
        case .saveSnippet: composePane?.saveBodyAsSnippet()
        case .insertSnippet(let text): composePane?.insertSnippet(text)
        // Agent requests have no search field; searching goes back to the inbox.
        case .search: if folder == .agentRequests { showInbox(scope: .all) }; search.becomeFirstResponder()
        case .searchHelp: showNotice(Self.searchSyntaxHelp)
        case .searchFilters: if folder == .agentRequests { showInbox(scope: .all) }; openSearchFilters()
        case .invitations: showCalendarWorkspace()
        case .followup: openWorkflow(calendar: false)
        case .followups: showInbox(scope: .followups)
        case .trackingActivity: showTrackingActivityPane(messageID: nil)
        case .toggleTracking: composePane?.paletteTracking()
        case .bookings: showBookingPanel()
        case .insertBooking: composePane?.paletteBooking()
        case .signature: composePane?.paletteSignature()
        case .archive: archiveSelection()
        case .undoTriage: undoTriageClicked()
        case .selectLoaded: selectLoadedClicked()
        case .clearSelection: clearMarkedClicked()
        case .bulkReadSender: bulkReadSender()
        case .bulkReadSenderDownloaded: bulkReadSender(downloadedOnly: true)
        case .cleanupJobs: openCleanupJobs()
        case .markRead: setRead(true)
        case .markUnread: setRead(false)
        case .reply: reply()
        case .replyAll: reply(all: true)
        case .compose: compose()
        case .drafts: showDrafts()
        case .editDraft: editSelectedDraft()
        case .discardDraft: discardSelectedDraft()
        case .undoDiscardDraft: undoDiscardDraft()
        case .discardComposerDraft: composePane?.paletteDiscard()
        case .sync: syncAll()
        case .addAccount: addAccount()
        case .notifications: MailNotifications.shared.toggleEnabled()
        case .testNotifications: MailNotifications.shared.sendTest()
        case .notificationPreviews:
            MailNotifications.shared.togglePreviews()
            syncStatus.stringValue = MailNotifications.shared.previews ? "Notifications show sender and subject" : "Notifications hide sender and subject"
        case .manageAccounts: manageAccounts()
        case .agentAccess: openAgentAccess()
        case .agentRequests: showAgentRequests()
        case .shortcuts: showNotice("g then i/u/f/a/s/d/o: Inbox, Unread, Favorites, Archive, Snoozed, Drafts, Outbox\nj / k: next / previous email · Ctrl-D / Ctrl-U: half-page down / up\n/: search · Esc: leave search · e: archive · !: spam · m/u: read/unread\nf: favorite · r/⇧r: reply/reply all · c: compose · l: labels · s: snooze\nx: select · ⇧J/⇧K: extend selection · ⌘A: select loaded results\nz: undo archive/move · ⌘K: commands · ⌘R: sync · ⌘U: unsubscribe\nIn Drafts: ↵ edit · ⌫ or # discard · z: undo discard")
        case .unsubscribe: unsubscribeSelection()
        case .inbox: showInbox(scope: .all)
        case .unread: showInbox(scope: .unread)
        case .snoozed: showInbox(scope: .snoozed)
        case .outbox: showOutbox()
        case .sent: showInbox(scope: .sent)
        case .newSplit: editSplit(nil)
        case .spam: moveToSpamSelection()
        case .favorite: togglePinSelection()
        case .favorites: showInbox(scope: .pinned)
        case .account(let id): showInbox(scope: .account(id))
        case .split(let id): showInbox(scope: .split(id))
        case .goToLabel(let label): showLabel(label)
        case .toggleLabel(let label), .createLabel(let label):
            let sender = NSMenuItem()
            sender.representedObject = label
            toggleLabelClicked(sender)
        case .snoozeUntil(let date): snoozeSelection(until: date)
        case .unsnooze: snoozeClicked(snoozeButton)
        case .toggleImages: setLoadRemoteImagesGlobally(!RemoteImagePreference.isEnabled(.standard))
        case .contact: toggleContactPane()
        case .appearance(let mode): (NSApp.delegate as? AppDelegate)?.setAppearanceMode(mode)
        case .sendDraft: composePane?.paletteSend()
        case .saveDraft: composePane?.paletteSaveDraft()
        case .scheduleDraft: composePane?.paletteSchedule()
        case .attachFile: composePane?.paletteAttach()
        case .chooseFrom: if let composePane { composePane.chooseFrom(in: self) }
        case .focusRecipient: if let composePane { composePane.focusRecipient(in: self) }
        case .focusCc: if let composePane { composePane.focusCopyRecipient(in: self, blind: false) }
        case .focusBcc: if let composePane { composePane.focusCopyRecipient(in: self, blind: true) }
        case .focusBody: if let composePane { composePane.focusBody(in: self) }
        case .labels, .browseLabels, .snooze, .switchAccount, .snippets: break
        }
    }

    @objc private func bulkReadSenderClicked(_ sender: NSMenuItem) { bulkReadSender() }
    private func openCleanupJobs(accountID: String? = nil, sender: String? = nil) {
        guard let core else { return }
        if senderCleanup == nil { senderCleanup = SenderCleanupController(core: core, accounts: knownAccounts, onStart: { [weak self] in self?.bulkReadSender() }) { [weak self] in self?.refreshInbox() } }
        senderCleanup?.open(accountID: accountID, sender: sender)
    }
    private func bulkReadSender(downloadedOnly: Bool = false) {
        guard folder == .inbox, threads.indices.contains(selection), let core,
              let accountID = threads[selection].accountID else { return }
        let sender = threads[selection].sender
        if !downloadedOnly && knownAccounts.first(where: { $0.id == accountID })?.provider.lowercased().contains("gmail") == true && !accountID.hasPrefix("demo:") {
            openCleanupJobs(accountID: accountID, sender: sender)
            return
        }
        Task { @MainActor in
            var savedLocally = false
            do {
                let ids = try await core.callAsync(["op":"unread_from_sender", "account_id":accountID, "sender":sender]) as? [String] ?? []
                let alert = NSAlert()
                alert.messageText = "Mark \(ids.count.formatted()) conversations as read?"
                alert.informativeText = "From \(sender) in this account. Includes all downloaded matching conversations, beyond the visible page. Mail not downloaded yet is excluded. Changes sync to your provider in the background."
                alert.addButton(withTitle: "Mark all as read")
                alert.addButton(withTitle: "Cancel")
                guard !ids.isEmpty, alert.runModal() == .alertFirstButtonReturn else {
                    if ids.isEmpty { showNotice("No unread downloaded conversations from this sender.") }
                    return
                }
                _ = try await core.callAsync(["op":"bulk_mark_read", "operation_id":"bulk:" + UUID().uuidString, "account_id":accountID, "thread_ids":ids])
                savedLocally = true
                refreshInbox()
                syncStatus.stringValue = "Marked \(ids.count.formatted()) conversations read; syncing…"
                if !accountID.hasPrefix("demo:") {
                    let result = try await core.callAsync(["op":"sync_account", "account_id":accountID]); scheduleQueuedActionRetry(result)
                    refreshInbox()
                }
            } catch {
                if savedLocally {
                    syncStatus.stringValue = "Changes saved locally · sync will retry"
                    syncStatus.toolTip = error.localizedDescription
                } else { showNotice(error.localizedDescription) }
            }
        }
    }

    private func setRead(_ read: Bool) {
        if !markedThreadIDs.isEmpty { performTriage(read ? "read" : "unread"); return }
        guard folder == .inbox, selection < threads.count, let id = threads[selection].id, let core else { return }
        let accountID = threads[selection].accountID
        pendingReadStates[id] = read
        threads[selection].unread = !read
        if read && inboxScope == .unread {
            removeSelectedThreadFromInbox()
        } else {
            table.reloadData(forRowIndexes: IndexSet(integer: selection), columnIndexes: IndexSet(integer: 0))
        }
        Task { @MainActor in
            do {
                _ = try await core.callAsync(["op": "apply_action", "operation_id": UUID().uuidString, "action": ["type": "mark_read", "thread_id": id, "read": read]])
                pendingReadStates.removeValue(forKey: id)
                refreshInbox()
                if let accountID {
                    do { let result = try await core.callAsync(["op": "sync_account", "account_id": accountID]); scheduleQueuedActionRetry(result) }
                    catch {
                        syncStatus.stringValue = "Change queued; sync failed"
                        syncStatus.toolTip = error.localizedDescription
                    }
                }
            } catch {
                pendingReadStates.removeValue(forKey: id)
                refreshInbox()
                showNotice(error.localizedDescription)
            }
        }
    }

    @objc private func togglePinClicked(_ sender: NSButton) { togglePinSelection() }

    private func togglePinSelection() {
        guard folder == .inbox, threads.indices.contains(selection), let id = threads[selection].id, let core else { return }
        let pinned = !threads[selection].pinned
        threads[selection].pinned = pinned
        pinButton.image = NSImage(systemSymbolName: pinned ? "star.fill" : "star", accessibilityDescription: nil)
        pinButton.contentTintColor = pinned ? EmailTheme.accent : EmailTheme.muted
        pinButton.toolTip = pinned ? "Remove from favorites (F)" : "Add to favorites (F)"
        if !pinned && inboxScope == .pinned { removeSelectedThreadFromInbox() }
        else { table.reloadData(forRowIndexes: IndexSet(integer: selection), columnIndexes: IndexSet(integer: 0)) }
        Task { @MainActor in
            do {
                _ = try await core.callAsync(["op": "set_thread_pinned", "thread_id": id, "pinned": pinned])
                refreshInbox()
            } catch {
                refreshInbox()
                showNotice(error.localizedDescription)
            }
        }
    }

    @objc private func showLabelsClicked(_ sender: NSButton) {
        guard folder == .inbox, selection < threads.count, let core else { return }
        let selected = threads[selection]
        let labels = (core.call(["op": "list_labels"]) as? [[String: Any]] ?? [])
            .compactMap { $0["name"] as? String }
        let allLabels = Array(Set(labels + selected.labels)).sorted { $0.localizedStandardCompare($1) == .orderedAscending }
        let menu = NSMenu(title: "Labels")
        for label in allLabels {
            let item = NSMenuItem(title: label, action: #selector(toggleLabelClicked(_:)), keyEquivalent: "")
            item.state = selected.labels.contains(label) ? .on : .off
            item.target = self
            item.representedObject = label
            menu.addItem(item)
        }
        if !allLabels.isEmpty { menu.addItem(.separator()) }
        let create = NSMenuItem(title: "New label…", action: #selector(createLabelClicked(_:)), keyEquivalent: "")
        create.target = self
        menu.addItem(create)
        DispatchQueue.main.async { [weak sender] in
            guard let sender else { return }
            menu.popUp(positioning: nil, at: NSPoint(x: 0, y: sender.bounds.height), in: sender)
        }
    }

    @objc private func toggleLabelClicked(_ sender: NSMenuItem) {
        guard folder == .inbox, selection < threads.count, let threadID = threads[selection].id,
              let label = sender.representedObject as? String, let core else { return }
        let removing = threads[selection].labels.contains(label)
        Task { @MainActor in
            do {
                _ = try await core.callAsync(["op": "apply_action", "operation_id": UUID().uuidString,
                                              "action": ["type": removing ? "remove_label" : "add_label", "thread_id": threadID, "label": label]])
                refreshInbox()
            } catch { showNotice(error.localizedDescription) }
        }
    }

    @objc private func createLabelClicked(_ sender: NSMenuItem) {
        let alert = NSAlert()
        alert.messageText = "New label"
        let field = NSTextField(frame: NSRect(x: 0, y: 0, width: 280, height: 26))
        field.placeholderString = "Label name"
        alert.accessoryView = field
        alert.addButton(withTitle: "Add label")
        alert.addButton(withTitle: "Cancel")
        guard alert.runModal() == .alertFirstButtonReturn else { return }
        let label = field.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !label.isEmpty else { return }
        let item = NSMenuItem()
        item.representedObject = label
        toggleLabelClicked(item)
    }

    @objc private func snoozeClicked(_ sender: NSButton) {
        guard folder == .inbox, selection < threads.count, let threadID = threads[selection].id, let core else { return }
        if threads[selection].snoozedUntil != nil {
            Task { @MainActor in
                do {
                    _ = try await core.callAsync(["op": "unsnooze_thread", "thread_id": threadID])
                    refreshInbox()
                } catch { showNotice(error.localizedDescription) }
            }
            return
        }
        openCommands()
        openPalettePage(.snooze)
    }

    private func snoozeSelection(until date: Date) {
        guard date > Date(), folder == .inbox, selection < threads.count,
              let threadID = threads[selection].id, let core else { return }
        Task { @MainActor in
            do {
                _ = try await core.callAsync(["op": "snooze_thread", "thread_id": threadID, "wake_at_ms": Int64(date.timeIntervalSince1970 * 1000)])
                refreshInbox()
            } catch { showNotice(error.localizedDescription) }
        }
    }

    @objc private func unsubscribeClicked(_ sender: NSButton) { unsubscribeSelection() }

    private func unsubscribeSelection() {
        guard folder == .inbox, selection < threads.count else { return }
        let selected = threads[selection]
        // mailto links go through the same sanitizer as reader link clicks.
        guard let url = unsubscribeURL.flatMap(ReaderLinkPolicy.externalURL) else {
            showNotice("No unsubscribe link was found in this message.")
            return
        }
        let alert = NSAlert()
        alert.messageText = "Unsubscribe from \(displayName(selected.sender))?"
        alert.informativeText = "Open the unsubscribe link at \(url.host ?? url.scheme ?? "this address") in your browser. Some senders ask you to confirm there."
        alert.addButton(withTitle: "Open unsubscribe link")
        alert.addButton(withTitle: "Cancel")
        if alert.runModal() == .alertFirstButtonReturn { NSWorkspace.shared.open(url) }
    }

    private static func unsubscribeURL(from header: String) -> URL? {
        let pattern = try! NSRegularExpression(pattern: #"<([^<>]+)>"#)
        let range = NSRange(header.startIndex..<header.endIndex, in: header)
        let values = pattern.matches(in: header, range: range).compactMap { match -> URL? in
            guard let valueRange = Range(match.range(at: 1), in: header) else { return nil }
            return URL(string: String(header[valueRange]))
        }
        return values.first { $0.scheme?.lowercased() == "https" && $0.host != nil && $0.user == nil && $0.password == nil }
            ?? values.first { $0.scheme?.lowercased() == "mailto" && ReaderLinkPolicy.externalURL($0) != nil }
    }

    private func refreshSidebar() {
        splits = core?.splits() ?? []
        sidebarSplits.arrangedSubviews.forEach { $0.removeFromSuperview() }
        for (index, split) in splits.enumerated() {
            let button = sidebarButton(split.name, symbol: "line.3.horizontal.decrease", action: #selector(showSplitClicked(_:)), active: calendarWorkspace == nil && inboxScope == .split(split.id) && folder == .inbox)
            button.tag = index
            button.toolTip = split.query
            let menu = NSMenu()
            let edit = NSMenuItem(title: "Edit split…", action: #selector(editSplitClicked(_:)), keyEquivalent: "")
            edit.target = self
            edit.representedObject = split.id
            menu.addItem(edit)
            let remove = NSMenuItem(title: "Delete split", action: #selector(deleteSplitClicked(_:)), keyEquivalent: "")
            remove.target = self
            remove.representedObject = split.id
            menu.addItem(remove)
            button.menu = menu
            sidebarSplits.addArrangedSubview(button)
        }
        sidebarAccounts.arrangedSubviews.forEach { $0.removeFromSuperview() }
        if knownAccounts.isEmpty {
            sidebarAccounts.addArrangedSubview(heading("No accounts yet", size: 11, weight: .regular, color: EmailTheme.muted))
        }
        for (index, account) in knownAccounts.enumerated() {
            let button = sidebarButton(account.email, symbol: "person.crop.circle", action: #selector(showAccountClicked(_:)), active: calendarWorkspace == nil && inboxScope == .account(account.id) && folder == .inbox)
            button.tag = index
            let count = unreadCounts[account.id] ?? 0
            if count > 0 {
                button.title = ""
                let label = NSTextField(labelWithString: account.email)
                label.font = .systemFont(ofSize: 11.5)
                label.textColor = EmailTheme.ink
                label.lineBreakMode = .byTruncatingMiddle
                let badge = NSTextField(labelWithString: count.formatted())
                badge.font = .monospacedDigitSystemFont(ofSize: 10, weight: .medium)
                badge.textColor = EmailTheme.muted
                badge.alignment = .right
                for field in [label, badge] { field.translatesAutoresizingMaskIntoConstraints = false; button.addSubview(field) }
                NSLayoutConstraint.activate([
                    label.leadingAnchor.constraint(equalTo: button.leadingAnchor, constant: 21),
                    label.trailingAnchor.constraint(equalTo: badge.leadingAnchor, constant: -5),
                    label.centerYAnchor.constraint(equalTo: button.centerYAnchor),
                    badge.trailingAnchor.constraint(equalTo: button.trailingAnchor, constant: -6),
                    badge.widthAnchor.constraint(equalToConstant: 35),
                    badge.centerYAnchor.constraint(equalTo: button.centerYAnchor)
                ])
                button.setAccessibilityLabel("\(account.email), \(count.formatted()) unread")
            }
            button.toolTip = "\(account.email) — \(count.formatted()) unread downloaded inbox conversations"
            sidebarAccounts.addArrangedSubview(button)
        }
    }

    private func refreshInbox() {
        guard folder == .inbox else { return }
        let query = search.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        let scopeKey = "\(inboxScope):\(query)"
        if scopeKey != markedScopeKey {
            markedScopeKey = scopeKey
            markedThreadIDs.removeAll()
            updateTriageBar()
        }
        inboxLoadGeneration += 1
        let generation = inboxLoadGeneration
        let requestedLimit = inboxLimit
        let scope = inboxScope
        if let core {
            Task { @MainActor in
                do {
                    let result = try await Task.detached(priority: .userInitiated) {
                        let accountID: String? = if case .account(let id) = scope { id } else { nil }
                        let splitID: String? = if case .split(let id) = scope { id } else { nil }
                        let unreadOnly = scope == .unread
                        let snoozed = scope == .snoozed
                        let pinned = scope == .pinned
                        let accounts = try core.accountRecordsChecked()
                        let label: String? = if case .label(let name) = scope { name } else { nil }
                        let rows = try core.threadsChecked(query: query.isEmpty ? nil : query, limit: requestedLimit, accountID: accountID, splitID: splitID, unreadOnly: unreadOnly, snoozed: snoozed, pinned: pinned, followups: scope == .followups, label: label, archived: scope == .archived, sent: scope == .sent)
                        let raw = core.call(["op": "unread_counts"]) as? [String: Any] ?? [:]
                        var counts = (raw["accounts"] as? [String: Int]) ?? [:]
                        counts["__total"] = raw["total"] as? Int ?? 0
                        let followups = raw["followups"] as? [String: Int] ?? [:]
                        counts["__followup_due"] = followups["due"] ?? 0
                        counts["__followup_active"] = followups["active"] ?? 0
                        return (accounts, rows, counts)
                    }.value
                    guard generation == inboxLoadGeneration else { return }
                    // The user may have moved to another message while the cache
                    // query was running. Preserve the selection at completion,
                    // never the one from when this refresh started.
                    let selectedID = threads.indices.contains(selection) ? threads[selection].id : nil
                    knownAccounts = result.0
                    unreadCounts = result.2
                    updateMailboxCounts()
                    if scopeTabButtons.count > 1 {
                        let count = unreadCounts["__total"] ?? 0
                        scopeTabButtons[1].title = "Unread \(count.formatted())"
                        scopeTabButtons[1].toolTip = "Unread conversations in downloaded inboxes"
                    }
                    updateAccountChrome()
                    threads = result.1.filter { thread in
                        guard let id = thread.id else { return true }
                        if pendingRemovedThreadIDs.contains(id) { return false }
                        if inboxScope == .unread, pendingReadStates[id] == true { return false }
                        return true
                    }.map { thread in
                        var thread = thread
                        if let id = thread.id, let read = pendingReadStates[id] { thread.unread = !read }
                        return thread
                    }
                    applyInboxSnapshot(selectedID: selectedID)
                    if !query.isEmpty { syncStatus.stringValue = "\(threads.count.formatted()) downloaded conversations · Esc clears search"; syncStatus.toolTip = Self.searchSyntaxHelp }
                    else if syncStatus.stringValue.hasPrefix("Search:") || syncStatus.stringValue.hasPrefix("Searching") { syncStatus.stringValue = "Downloaded mail"; syncStatus.toolTip = nil }
                    if let pendingID = pendingContactThreadID {
                        pendingContactThreadID = nil
                        if let row = threads.firstIndex(where: { $0.id == pendingID }) { select(row) }
                    }
                } catch {
                    guard generation == inboxLoadGeneration else { return }
                    isLoadingOlder = false
                    syncStatus.stringValue = "Search: \(error.localizedDescription)"
                    syncStatus.toolTip = error.localizedDescription + "\n" + Self.searchSyntaxHelp
                }
            }
        } else {
            let selectedID = threads.indices.contains(selection) ? threads[selection].id : nil
            threads = ProcessInfo.processInfo.environment["EMAIL_FIXTURE"] == "1" ? fixtureThreads : []
            applyInboxSnapshot(selectedID: selectedID)
        }
    }

    private func applyInboxSnapshot(selectedID: String?) {
        if let target = pendingActivityThread {
            if folder == .inbox, inboxScope == .sent, pendingActivityGeneration == inboxLoadGeneration,
               !threads.contains(where: { $0.id == target.id }) { threads.insert(target, at: 0) }
            pendingActivityThread = nil; pendingActivityGeneration = nil
        }
        markedThreadIDs.formIntersection(Set(threads.compactMap { $0.id }))
        updateTriageBar()
        suppressSelectionChanges = true
        table.reloadData()
        refreshSidebar()
        updateEmptyInbox()
        isLoadingOlder = false
        let desiredID = pendingNotificationThreadID ?? selectedID
        pendingNotificationThreadID = nil
        if let desiredID, let index = threads.firstIndex(where: { $0.id == desiredID }) { select(index) }
        else if !threads.isEmpty { select(min(selection, threads.count - 1)) }
        suppressSelectionChanges = false
        if threads.isEmpty { clearReader() }
        else if displayedThreadID == nil || threads[selection].id != displayedThreadID { show(threads[selection]) }
    }

    @objc private func addAccountClicked() { addAccount() }
    @objc private func showInboxClicked() {
        showInbox(scope: .all)
    }
    @objc private func showUnreadClicked() { showInbox(scope: .unread) }
    @objc private func showPinnedClicked() { showInbox(scope: .pinned) }
    private func showLabel(_ label: String) { showInbox(scope: .label(label)) }
    @objc private func showArchiveClicked() {
        showInbox(scope: .archived)
        syncStatus.stringValue = "Loading archived mail…"
        let restart = !archiveBrowseStarted
        archiveBrowseStarted = true
        guard let core else { return }
        Task { @MainActor in
            do {
                let result = try await core.callAsync(["op": "sync_archives", "restart": restart]) as? [[String: Any]] ?? []
                if inboxScope == .archived {
                    refreshInbox()
                    let failures = result.compactMap { $0["error"] as? String }
                    syncStatus.stringValue = failures.isEmpty ? "Archived mail updated" : "Archive will retry shortly"
                    syncStatus.toolTip = failures.joined(separator: "\n")
                }
            } catch {
                if inboxScope == .archived { syncStatus.stringValue = "Archive could not load"; syncStatus.toolTip = error.localizedDescription }
            }
        }
    }
    @objc private func showFollowupsClicked() { showInbox(scope: .followups) }
    @objc private func showInvitationsClicked() { showCalendarWorkspace() }
    /// Public builds offer Recent opens only when tracked sent mail is cached.
    /// Checked at launch and when the palette opens until found; the
    /// local-only history read runs off the main thread.
    private func checkSavedOpenActivity() {
        // Public builds cannot create tracked mail, so once offered it stays.
        guard DistributionProfile.current?.openTracking != true, DistributionProfile.current?.hasScheduling == true,
              !offersOpenActivity, let core else { return }
        Task { @MainActor [weak self] in
            guard let data = try? await core.callAsync(["op": "tracking_capability"]) as? [String: Any] else { return }
            // An open palette keeps its rows (and selection); the next one lists it.
            self?.offersOpenActivity = data["saved_activity"] as? Bool == true
        }
    }

    @objc private func showTrackingActivity() {
        if !contactPane.isHidden { updateTrackingButton(refresh: true); contactPane.focusOpenActivity() }
        else { showTrackingActivityPane(messageID: currentMessageID) }
    }

    private func showTrackingActivityPane(messageID: String?) {
        guard let core else { return }
        closeCalendarWorkspace()
        activityReturnResponder = firstResponder
        activityController?.viewWillDisappear()
        activityHost.subviews.forEach { $0.removeFromSuperview() }
        let controller = TrackingActivityView(core: core, messageID: messageID, offersTrackOpens: DistributionProfile.current?.openTracking == true)
        activityController = controller
        controller.dismiss = { [weak self] in self?.closeActivityPane() }
        controller.openConversation = { [weak self] item in
                guard let self else { return "The mail window is unavailable." }
                guard self.composePane == nil else { return "Close your draft before opening another conversation." }
                guard let threadID = item["thread_id"] as? String, let accountID = item["account_id"] as? String,
                      self.knownAccounts.contains(where: { $0.id == accountID }) else { return "This account is no longer available." }
                let account = self.knownAccounts.first { $0.id == accountID }
                let sent = (item["sent_at_ms"] as? NSNumber)?.doubleValue ?? 0
                let target = MailThread(id: threadID, accountID: accountID, account: account?.email ?? accountID,
                    sender: item["recipient"] as? String ?? "", subject: item["subject"] as? String ?? "",
                    preview: "", time: Date(timeIntervalSince1970: sent / 1000).formatted(date: .abbreviated, time: .shortened),
                    unread: false, html: "")
                self.pendingActivityThread = target
                self.pendingActivitySelection = (threadID, item["message_id"] as? String ?? "")
                self.pendingNotificationThreadID = threadID
                self.showInbox(scope: .sent)
                self.pendingActivityGeneration = self.inboxLoadGeneration
                if self.frame.width < 1000 { self.closeActivityPane() }
                self.makeFirstResponder(self.table)
                return nil
            }
        controller.onChanged = { [weak self] in self?.refreshInbox() }
        let panel = controller.view
        panel.translatesAutoresizingMaskIntoConstraints = false
        activityHost.addSubview(panel)
        NSLayoutConstraint.activate([
            panel.leadingAnchor.constraint(equalTo: activityHost.leadingAnchor), panel.trailingAnchor.constraint(equalTo: activityHost.trailingAnchor),
            panel.topAnchor.constraint(equalTo: activityHost.topAnchor), panel.bottomAnchor.constraint(equalTo: activityHost.bottomAnchor)
        ])
        activityHost.isHidden = false
        updateActivityLayout()
        controller.viewDidAppear(); controller.focusActivity()
    }
    private func closeActivityPane() {
        activityController?.viewWillDisappear(); activityController = nil
        activityHost.subviews.forEach { $0.removeFromSuperview() }; activityHost.isHidden = true
        inboxPanel?.isHidden = false; readerPanel?.isHidden = false
        updateContactPaneVisibility()
        makeFirstResponder(activityReturnResponder ?? table); activityReturnResponder = nil
    }
    private func updateActivityLayout() {
        guard activityController != nil else { return }
        contactPane.isHidden = true
        // Keep the activity inside the window when two content columns do not fit.
        inboxPanel?.isHidden = frame.width < 1360
        readerPanel?.isHidden = frame.width < 1000
    }
    /// Only tracked sent mail gets the activity button; it carries the open count.
    private func updateTrackingButton(refresh: Bool = false) {
        if !refresh {
            contactPane.showOpenActivity(nil)
            trackingActivityButton.isHidden = true
            trackingActivityButton.title = ""; trackingActivityButton.imagePosition = .imageOnly
        }
        guard let id = currentMessageID, let core, DistributionProfile.current?.hasScheduling == true else { return }
        Task { @MainActor [weak self] in
            guard let data = try? await core.callAsync(["op":"tracking_activity","message_id":id,"refresh":refresh]) as? [String: Any],
                  let self, self.currentMessageID == id,
                  let item = (data["items"] as? [[String: Any]])?.first else { return }
            let count = item["pixel_requests"] as? Int ?? 0
            self.contactPane.showOpenActivity(item)
            if !refresh && !self.contactPane.isHidden { self.updateTrackingButton(refresh: true) }
            self.trackingActivityButton.isHidden = false
            self.trackingActivityButton.title = count > 0 ? " \(count)" : ""
            self.trackingActivityButton.imagePosition = count > 0 ? .imageLeading : .imageOnly
            self.trackingActivityButton.toolTip = count == 0 ? ((item["reply_received"] as? Bool ?? false) ? "Reply received" : "No opens recorded") : (count == 1 ? "Opened once" : "Opened \(count) times")
        }
    }
    func refreshTrackingButton() { updateTrackingButton() }
    func showBookingPanel() {
        guard let core, DistributionProfile.current?.hasScheduling == true else { return }
        BookingPanel.present(core: core, emailGuest: { [weak self] address, hostEmail in
            guard let self else { return }
            self.closeCalendarWorkspace()
            let account = self.knownAccounts.first { $0.email.caseInsensitiveCompare(hostEmail ?? "") == .orderedSame }
            self.showComposePane(core: core, accountID: account?.id, recipient: address)
        }, findMail: { [weak self] address in
            guard let self else { return }; self.closeCalendarWorkspace(); self.showInbox(scope:.all)
            self.search.stringValue = "from:" + address; self.refreshInbox()
        })
    }
    func showCalendarWorkspace() {
        // G C and the palette follow the sidebar: no Calendar where none can be connected.
        guard calendarAvailable else { NSSound.beep(); return }
        guard calendarWorkspace == nil, let core, let container = contentView, let sidebarPanel else { return }
        if let composePane { composePane.requestClose(); if self.composePane != nil { return } }
        if activityController != nil { closeActivityPane() }
        workflowPopover?.close()
        clearGoTo()
        let calendar = cachedCalendarWorkspace ?? AgendaPanel.embedded(core: core, host: self) { [weak self] in self?.closeCalendarWorkspace() }
        guard let view = calendar.workspaceView else { return }
        calendarWorkspace = calendar
        if cachedCalendarWorkspace != nil { calendar.resume() }
        calendarWorkspaceView = view
        view.translatesAutoresizingMaskIntoConstraints = false
        container.addSubview(view)
        NSLayoutConstraint.activate([
            view.leadingAnchor.constraint(equalTo: sidebarPanel.trailingAnchor),
            view.trailingAnchor.constraint(equalTo: container.trailingAnchor),
            view.topAnchor.constraint(equalTo: container.topAnchor),
            view.bottomAnchor.constraint(equalTo: container.bottomAnchor)
        ])
        // Keep arranged column containers in layout; detaching them makes
        // AppKit shrink the outer window to the sidebar's intrinsic width.
        calendarHiddenMailViews = ([inboxPanel, readerPanel].compactMap { $0 } + [contactPane])
            .flatMap { $0.subviews }.map { ($0, $0.isHidden) }
        calendarHiddenMailViews.forEach { $0.0.isHidden = true }
        inboxPanel?.setAccessibilityHidden(true)
        readerPanel?.setAccessibilityHidden(true)
        contactPane.setAccessibilityHidden(true)
        makeFirstResponder(nil)
        updateSidebarSelection()
    }
    private func closeCalendarWorkspace() {
        guard calendarWorkspace != nil else { return }
        calendarWorkspaceView?.removeFromSuperview()
        calendarWorkspaceView = nil
        cachedCalendarWorkspace = calendarWorkspace
        calendarWorkspace = nil
        calendarHiddenMailViews.forEach { $0.0.isHidden = $0.1 }
        calendarHiddenMailViews = []
        inboxPanel?.setAccessibilityHidden(false)
        readerPanel?.setAccessibilityHidden(false)
        contactPane.setAccessibilityHidden(false)
        updateSidebarSelection()
        makeFirstResponder(table)
    }
    @objc private func showSnoozedClicked() { showInbox(scope: .snoozed) }
    @objc private func showAccountClicked(_ sender: NSButton) {
        guard knownAccounts.indices.contains(sender.tag) else { return }
        showInbox(scope: .account(knownAccounts[sender.tag].id))
    }
    @objc private func showSplitClicked(_ sender: NSButton) {
        guard splits.indices.contains(sender.tag) else { return }
        showInbox(scope: .split(splits[sender.tag].id))
    }
    private func showInbox(scope: InboxScope) {
        if scope != .sent {
            let targetID = pendingActivityThread?.id ?? pendingActivitySelection?.threadID
            if let targetID, pendingNotificationThreadID == targetID { pendingNotificationThreadID = nil }
            pendingActivityThread = nil; pendingActivitySelection = nil; pendingActivityGeneration = nil
        }
        closeCalendarWorkspace()
        clearGoTo()
        folder = .inbox
        inboxScope = scope
        inboxTitle?.stringValue = switch scope {
        case .all: "Inbox"
        case .unread: "Unread"
        case .pinned: "Favorites"
        case .snoozed: "Snoozed"
        case .followups: "Follow-ups"
        case .archived: "Archive"
        case .sent: "Sent"
        case .account(let id): knownAccounts.first(where: { $0.id == id })?.email ?? "Account"
        case .split(let id): splits.first(where: { $0.id == id })?.name ?? "Split"
        case .label(let name): "#\(name)"
        }
        search.placeholderString = "Search all mail  /"
        search.stringValue = ""
        canLoadOlder = true
        inboxLimit = 100
        updateSidebarSelection()
        dismissComposePane()
        table.deselectAll(nil)
        table.reloadData()
        updateEmptyInbox()
        if threads.isEmpty { clearReader() }
        else { select(min(selection, threads.count - 1)) }
        refreshInbox()
    }
    @objc private func manageAccountsClicked() { manageAccounts() }
    @objc private func agentAccessClicked() { openAgentAccess() }

    func openAgentAccess() {
        guard let core else { showNotice("Mail engine unavailable."); return }
        let controller = agentAccess ?? AgentAccessController(core: core)
        if agentAccess == nil {
            controller.onReviewRequests = { [weak self] in self?.openAgentRequest(id: nil) }
            controller.onChanged = { [weak self] in
                guard let self else { return }
                Task { @MainActor in await self.refreshAgentRequests() }
            }
            agentAccess = controller
        }
        controller.open()
    }

    /// From a notification or the settings window: the main window on that request.
    func openAgentRequest(id: String?) {
        makeKeyAndOrderFront(nil)
        showAgentRequests(select: id)
    }

    @objc private func showAgentRequestsClicked() { showAgentRequests() }

    private func showAgentRequests(select target: String? = nil) {
        closeCalendarWorkspace()
        clearGoTo()
        guard core != nil else { return }
        markedThreadIDs.removeAll()
        folder = .agentRequests
        selection = -1
        updateTriageBar()
        inboxTitle?.stringValue = "Agent requests"
        search.stringValue = ""
        syncStatus.stringValue = ""
        updateSidebarSelection()
        dismissComposePane()
        table.deselectAll(nil)
        table.reloadData()
        updateEmptyInbox()
        clearReader()
        agentRequestReader.show(nil)
        updateAgentRequestsChrome()
        makeFirstResponder(table)
        reloadAgentRequestList(select: target)
    }

    /// Re-read the requests, keeping the selected one, or the row in its place
    /// when it was just decided, so J and K pick up where you were.
    private func reloadAgentRequestList(select target: String? = nil) {
        guard let core else { return }
        Task { @MainActor in
            let rows = (try? await core.callAsync(["op": "agent_requests"]) as? [[String: Any]]) ?? []
            guard folder == .agentRequests else { return }
            let current = target ?? (agentRequestReader.request?["id"] as? String)
            let previousIndex = max(0, table.selectedRow)
            agentRequests = rows
            agentRequestTotal = rows.count
            updateAgentRequestsChrome()
            syncStatus.stringValue = rows.isEmpty ? "" : "\(rows.count) waiting"
            suppressSelectionChanges = true
            table.reloadData()
            updateEmptyInbox()
            let visible = agentRequests
            var index = current.flatMap { id in visible.firstIndex { $0["id"] as? String == id } }
            if index == nil, !visible.isEmpty { index = min(previousIndex, visible.count - 1) }
            if let index {
                selection = index
                table.selectRowIndexes(IndexSet(integer: index), byExtendingSelection: false)
                table.scrollRowToVisible(index)
            }
            suppressSelectionChanges = false
            agentRequestReader.show(index.map { visible[$0] })
        }
    }

    private func updateAgentRequestsChrome() {
        let total = agentRequestTotal
        agentRequestsPill.text = total > 99 ? "99+" : String(total)
        agentRequestsPill.isHidden = total == 0
        agentRequestsSidebarButton?.isHidden = hasNoAccounts || !(agentAccessOn || total > 0 || folder == .agentRequests)
        let attention = total > 0 ? "\(total) waiting" : "nothing waiting"
        agentRequestsSidebarButton?.setAccessibilityLabel("Agent requests, \(attention)")
    }

    private func approveAgentRequest(_ request: [String: Any]) {
        guard let core, !agentRequestBusy, let id = request["id"] as? String else { return }
        if request["type"] as? String == "access" { return allowAgentRequest(request) }
        agentRequestBusy = true
        let kind = request["kind"] as? String ?? ""
        Task { @MainActor in
            defer { agentRequestBusy = false }
            do {
                let action = try await core.callAsync(["op": "approve_agent_action", "id": id, "undo_seconds": AgentRequestReader.undoSeconds]) as? [String: Any] ?? [:]
                let result = action["result"] as? [String: Any] ?? [:]
                switch kind {
                case "send_draft", "queue_send_draft", "schedule_draft":
                    let messageID = result["message_id"] as? String ?? ""
                    let sendAt = Date(timeIntervalSince1970: ((result["send_at_ms"] as? NSNumber)?.doubleValue ?? 0) / 1000)
                    let scheduled = result["status"] as? String == "scheduled"
                    // The composer's undo affordance, here and in the reader.
                    lastQueuedMessageID = messageID
                    cancelSendButton.title = scheduled ? "Cancel send" : "Undo send"
                    cancelSendButton.isHidden = false
                    if scheduled {
                        agentRequestReader.showUndo("Scheduled for \(sendAt.formatted(date: .abbreviated, time: .shortened))", action: "Cancel", until: nil) { [weak self] in self?.cancelAgentSend(messageID) }
                    } else {
                        let seconds = max(1, Int(sendAt.timeIntervalSinceNow.rounded()))
                        agentRequestReader.showUndo("Sending in \(seconds) seconds", action: "Undo", until: sendAt) { [weak self] in self?.cancelAgentSend(messageID) }
                    }
                case "batch_triage":
                    if result["undoable"] as? Bool == true {
                        let operation = result["operation_id"] as? String ?? ""
                        lastUndoBatch = (operation, request["account_ids"] as? [String] ?? [])
                        agentRequestReader.showUndo("Done", action: "Undo", until: Date().addingTimeInterval(30)) { [weak self] in self?.undoAgentBatch(operation) }
                    }
                case "undo_triage":
                    if lastUndoBatch?.id == result["operation_id"] as? String { lastUndoBatch = nil }
                default:
                    break
                }
                updateTriageBar()
            } catch {
                showNotice(error.localizedDescription)
            }
            await refreshAgentRequests()
            reloadAgentRequestList()
        }
    }

    private func rejectAgentRequest(_ request: [String: Any]) {
        guard let core, !agentRequestBusy, let id = request["id"] as? String else { return }
        agentRequestBusy = true
        Task { @MainActor in
            defer { agentRequestBusy = false }
            do {
                if request["type"] as? String == "access" {
                    _ = try await core.callAsync(["op": "dismiss_agent_access_request", "account_id": request["account_id"] as? String ?? ""])
                } else {
                    _ = try await core.callAsync(["op": "reject_agent_action", "id": id])
                }
            } catch { showNotice(error.localizedDescription) }
            await refreshAgentRequests()
            reloadAgentRequestList()
        }
    }

    /// Edit closes the request without sending and opens its draft in the
    /// composer; only the user's own Send sends it.
    private func editAgentRequest(_ request: [String: Any]) {
        guard let core, !agentRequestBusy, let id = request["id"] as? String else { return }
        agentRequestBusy = true
        Task { @MainActor in
            defer { agentRequestBusy = false }
            do {
                let action = try await core.callAsync(["op": "edit_agent_action", "id": id]) as? [String: Any] ?? [:]
                let messageID = ((action["details"] as? [String: Any])?["request"] as? [String: Any])?["message_id"] as? String
                await refreshAgentRequests()
                showDrafts(thenEdit: messageID)
            } catch {
                showNotice(error.localizedDescription)
                reloadAgentRequestList()
            }
        }
    }

    /// Allow… on a refused agent: the turn-on sheet with the level it needs.
    private func allowAgentRequest(_ request: [String: Any]) {
        guard let core, let accountID = request["account_id"] as? String else { return }
        let level = request["level"] as? String ?? "read"
        AgentAccessFlow.allow(core: core, window: self, accountID: accountID, level: level,
                              grants: request["grants"] as? [String] ?? ["sync"], custom: level == "custom",
                              changing: request["changing"] as? Bool == true) { [weak self] _ in
            guard let self else { return }
            Task { @MainActor in
                await self.refreshAgentRequests()
                self.reloadAgentRequestList()
                if let agentAccess = self.agentAccess, agentAccess.isOpen { await agentAccess.reload() }
            }
        }
    }

    private func cancelAgentSend(_ messageID: String) {
        guard let core else { return }
        Task { @MainActor in
            do {
                _ = try await core.callAsync(["op": "cancel_scheduled_draft", "message_id": messageID])
                if lastQueuedMessageID == messageID {
                    lastQueuedMessageID = nil
                    cancelSendButton.isHidden = true
                }
                agentRequestReader.hideUndo()
                syncStatus.stringValue = "Send cancelled · back in Drafts"
            } catch { showNotice(error.localizedDescription) }
        }
    }

    private func undoAgentBatch(_ operationID: String) {
        guard let core else { return }
        Task { @MainActor in
            do {
                _ = try await core.callAsync(["op": "undo_triage", "operation_id": operationID])
                if lastUndoBatch?.id == operationID { lastUndoBatch = nil }
                agentRequestReader.hideUndo()
                updateTriageBar()
            } catch { showNotice(error.localizedDescription) }
        }
    }

    private func saveAgentAttachment(requestID: String, attachmentID: String, filename: String) {
        guard let core else { return }
        Task { @MainActor in
            do {
                let file = try await core.callAsync(["op": "agent_request_attachment", "id": requestID, "attachment_id": attachmentID]) as? [String: Any] ?? [:]
                guard let encoded = file["data_base64"] as? String, let data = Data(base64Encoded: encoded) else { return }
                let panel = NSSavePanel()
                panel.nameFieldStringValue = filename
                guard panel.runModal() == .OK, let url = panel.url else { return }
                try data.write(to: url, options: .atomic)
                // The agent chose these bytes: mark the copy as downloaded so
                // Gatekeeper checks it before anything can run it.
                var values = URLResourceValues()
                values.quarantineProperties = [kLSQuarantineTypeKey as String: kLSQuarantineTypeOtherDownload, kLSQuarantineAgentNameKey as String: "Email"]
                var marked = url
                try? marked.setResourceValues(values)
            } catch { showNotice(error.localizedDescription) }
        }
    }

    /// Cheap local poll: the Agent requests item and its count, notifications,
    /// and an open requests list or settings window.
    private func refreshAgentRequests() async {
        guard let core, !agentPollInFlight else { return }
        agentPollInFlight = true
        defer { agentPollInFlight = false }
        guard let summary = try? await core.callAsync(["op": "agent_pending_summary"]) as? [String: Any] else { return }
        agentRequestTotal = (summary["total"] as? NSNumber)?.intValue ?? 0
        agentAccessOn = summary["access_on"] as? Bool == true
        updateAgentRequestsChrome()
        await MailNotifications.shared.recordAgentRequests(summary)
        // Re-read the list only when a request, decision or consent changed.
        let revision = summary["revision"] as? String ?? ""
        if revision != agentRevision {
            if folder == .agentRequests { reloadAgentRequestList() }
            if let agentAccess, agentAccess.isOpen { await agentAccess.reload() }
        }
        agentRevision = revision
    }
    @objc private func syncClicked() { syncAll() }
    @objc private func showDraftsClicked() { showDrafts() }

    @objc private func inboxDidScroll(_ notification: Notification) {
        guard folder == .inbox, inboxScope != .snoozed, inboxScope != .followups, search.stringValue.isEmpty, !isLoadingOlder, canLoadOlder, !knownAccounts.isEmpty, !threads.isEmpty else { return }
        let visible = inboxScroll.documentVisibleRect
        guard visible.maxY >= table.bounds.height - table.rowHeight * 4 else { return }
        loadOlderMail()
    }

    private func loadOlderMail() {
        guard folder == .inbox, let core, !knownAccounts.isEmpty, !isLoadingOlder else { return }
        isLoadingOlder = true
        syncStatus.stringValue = "Loading older mail…"
        let previousCount = threads.count
        inboxLimit += 100
        let requestedLimit = inboxLimit
        Task { @MainActor in
            let scope = inboxScope
            let cached: [MailThread]
            do {
                cached = try await Task.detached {
                    let accountID: String? = if case .account(let id) = scope { id } else { nil }
                    let splitID: String? = if case .split(let id) = scope { id } else { nil }
                    let label: String? = if case .label(let name) = scope { name } else { nil }
                    return try core.threadsChecked(limit: requestedLimit, accountID: accountID, splitID: splitID, unreadOnly: scope == .unread, pinned: scope == .pinned, followups: scope == .followups, label: label, archived: scope == .archived, sent: scope == .sent)
                }.value
            } catch {
                isLoadingOlder = false
                syncStatus.stringValue = "Older mail could not load"
                syncStatus.toolTip = error.localizedDescription
                return
            }
            if cached.count > previousCount {
                refreshInbox()
            } else {
                if scope == .pinned || { if case .label = scope { return true }; return false }() {
                    canLoadOlder = false
                    isLoadingOlder = false
                    syncStatus.stringValue = "Updated just now"
                    return
                }
                guard Date() >= nextOlderNetworkSyncAt else {
                    isLoadingOlder = false
                    syncStatus.stringValue = "Older mail will retry shortly"
                    return
                }
                nextOlderNetworkSyncAt = Date().addingTimeInterval(60)
                let result = try? await core.callAsync(["op": scope == .archived ? "sync_archives" : "sync_all"])
                refreshInbox()
                if let rows = result as? [[String: Any]] {
                    let failures = rows.compactMap { $0["error"] as? String }
                    let added = rows.compactMap { (($0["data"] as? [String: Any])?["messages_added"] as? NSNumber)?.intValue }.reduce(0, +)
                    if failures.isEmpty && added == 0 { canLoadOlder = false }
                    if !failures.isEmpty {
                        nextOlderNetworkSyncAt = Date().addingTimeInterval(300)
                        syncStatus.stringValue = "Older mail could not sync"
                    }
                } else {
                    nextOlderNetworkSyncAt = Date().addingTimeInterval(300)
                    syncStatus.stringValue = "Older mail could not sync"
                }
            }
            if canLoadOlder && syncStatus.stringValue == "Loading older mail…" { syncStatus.stringValue = "Older mail loaded" }
        }
    }

    private func showDrafts(thenEdit editID: String? = nil) {
        closeCalendarWorkspace()
        clearGoTo()
        guard let core else { showNotice("Mail engine unavailable."); return }
        markedThreadIDs.removeAll()
        folder = .drafts
        selection = -1
        updateTriageBar()
        inboxTitle?.stringValue = "Drafts"
        search.placeholderString = "Search drafts  /"
        search.stringValue = ""
        updateSidebarSelection()
        dismissComposePane()
        inboxLoadGeneration += 1
        table.deselectAll(nil)
        table.reloadData()
        updateEmptyInbox()
        clearReader()
        makeFirstResponder(table)
        let generation = inboxLoadGeneration
        Task { @MainActor in
            let loaded = await Task.detached(priority: .userInitiated) { core.drafts() }.value
            guard folder == .drafts, generation == inboxLoadGeneration else { return }
            drafts = loaded
            table.deselectAll(nil)
            table.reloadData()
            updateEmptyInbox()
            clearReader()
            if let editID, let row = filteredDrafts.firstIndex(where: { $0.id == editID }) {
                selection = row
                table.selectRowIndexes(IndexSet(integer: row), byExtendingSelection: false)
                table.scrollRowToVisible(row)
                showComposePane(core: core, draft: filteredDrafts[row])
            }
        }
    }

    @objc private func showSentClicked() { showInbox(scope: .sent) }

    @objc private func showOutboxClicked() { showOutbox() }

    private func showOutbox() {
        closeCalendarWorkspace()
        clearGoTo()
        guard let core else { return }
        markedThreadIDs.removeAll()
        folder = .outbox
        updateTriageBar()
        inboxTitle?.stringValue = "Outbox"
        search.placeholderString = "Search outbox  /"
        search.stringValue = ""
        updateSidebarSelection()
        dismissComposePane()
        table.deselectAll(nil)
        table.reloadData()
        updateEmptyInbox()
        clearReader()
        refreshOutbox(using: core)
    }

    private func refreshOutbox(using core: CoreBridge) {
        inboxLoadGeneration += 1
        let generation = inboxLoadGeneration
        Task { @MainActor in
            let loaded = await Task.detached(priority: .userInitiated) { core.outbox() }.value
            guard folder == .outbox, generation == inboxLoadGeneration else { return }
            outboxItems = loaded
            table.reloadData()
            updateEmptyInbox()
            if let selected = table.selectedRowIndexes.first, selected < filteredOutbox.count {
                showOutboxItem(filteredOutbox[selected])
            } else if !filteredOutbox.isEmpty {
                table.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false)
            } else { clearReader() }
        }
    }

    private func showOutboxItem(_ item: OutboxItem) {
        dismissComposePane()
        messageLoadGeneration += 1
        recipientSummaryMessage = nil
        recipientPopover?.close()
        currentMessageID = item.messageID
        currentAttachments = []
        currentHTML = nil
        readerMessageCount = 0
        subject.stringValue = item.subject
        sender.stringValue = item.to.isEmpty ? "No recipient" : "To \(item.to)"
        account.stringValue = item.status == "undoable" ? "Queued with undo" : "Scheduled for \(DateFormatter.localizedString(from: item.sendAt, dateStyle: .medium, timeStyle: .short))"
        subject.isHidden = false
        sender.isHidden = false
        account.isHidden = false
        readerActions.isHidden = false
        archiveButton.isHidden = true
        replyButton.isHidden = true
        replyAllButton.isHidden = true
        contactButton.isHidden = true
        labelsButton.isHidden = true
        pinButton.isHidden = true
        snoozeButton.isHidden = true
        cancelSendButton.isHidden = false
        cancelSendButton.title = item.status == "undoable" ? "Undo send" : "Cancel send"
        attachmentsButton.alphaValue = 0
        attachmentsButton.isEnabled = false
        remoteImagesButton.isHidden = true
        setRemoteImagesNoticeVisible(false)
        emptyState.isHidden = true
        readerBody.isHidden = false
        bodyScroll.isHidden = false
        webView?.isHidden = true
        bodyText.string = item.bodyPreview
        guard let core else { return }
        let generation = messageLoadGeneration
        Task { @MainActor in
            if let message = try? await core.callAsync(["op": "get_message", "id": item.messageID]) as? [String: Any],
               generation == messageLoadGeneration {
                bodyText.string = message["body_plain"] as? String ?? item.bodyPreview
            }
        }
    }

    @objc private func cancelScheduledClicked(_ sender: NSButton) {
        guard let id = (folder == .outbox ? filteredOutbox.first(where: { $0.messageID == currentMessageID })?.messageID : lastQueuedMessageID), let core else { return }
        Task { @MainActor in
            do {
                _ = try await core.callAsync(["op": "cancel_scheduled_draft", "message_id": id])
                lastQueuedMessageID = nil
                cancelSendButton.isHidden = true
                if folder == .outbox { refreshOutbox(using: core) }
                showNotice("Message returned to Drafts.")
            } catch { showNotice(error.localizedDescription) }
        }
    }

    private func dispatchScheduledMail() async {
        guard let core else { return }
        guard let results = try? await core.callAsync(["op": "dispatch_due"]) as? [[String: Any]], !results.isEmpty else { return }
        if let queued = lastQueuedMessageID, results.contains(where: { $0["message_id"] as? String == queued && $0["status"] as? String == "sent" }) {
            lastQueuedMessageID = nil
            cancelSendButton.isHidden = true
            syncStatus.stringValue = "Sent · Available in Sent"
        }
        if folder == .outbox { refreshOutbox(using: core) }
        else if folder == .inbox { refreshInbox() }
    }

    private func addAccount() {
        guard let core else { showNotice("Build mail-service and relaunch the app before adding accounts."); return }
        let window = AccountSetupWindow(core: core) { [weak self] in
            Task { @MainActor [weak self] in
                let accounts = await Task.detached { core.accountRecords() }.value
                self?.knownAccounts = accounts
                self?.updateAccountChrome()
                self?.refreshInbox()
                self?.syncAll()
            }
        }
        accountSetupWindow = window
        window.makeKeyAndOrderFront(nil)
    }

    private func manageAccounts() {
        guard core != nil else { showNotice("Mail engine unavailable."); return }
        let accounts = knownAccounts
        guard !accounts.isEmpty else { addAccount(); return }
        let alert = NSAlert()
        alert.messageText = "Connected accounts"
        alert.informativeText = accounts.map { "\($0.email) (\($0.provider))" }.joined(separator: "\n")
        alert.addButton(withTitle: "Close")
        alert.addButton(withTitle: "Remove an account…")
        alert.addButton(withTitle: "Agent Access…")
        switch alert.runModal() {
        case .alertSecondButtonReturn: removeAccount()
        case .alertThirdButtonReturn: openAgentAccess()
        default: break
        }
    }

    private func removeAccount() {
        guard let core else { return }
        let accounts = knownAccounts
        guard !accounts.isEmpty else { return }
        let alert = NSAlert()
        alert.messageText = "Remove a local account"
        alert.informativeText = "Choose the account to remove from this app. This removes its local cache and stored credential; mail at the provider stays there."
        let popup = NSPopUpButton()
        popup.addItems(withTitles: accounts.map(\.email))
        alert.accessoryView = popup
        alert.addButton(withTitle: "Cancel")
        alert.addButton(withTitle: "Remove")
        guard alert.runModal() == .alertSecondButtonReturn else { return }
        let chosen = accounts[popup.indexOfSelectedItem]
        syncStatus.stringValue = "Removing \(chosen.email)…"
        Task { @MainActor in
            do {
                _ = try await core.callAsync(["op": "remove_account", "account_id": chosen.id])
                syncStatus.stringValue = "Account removed"
                refreshInbox()
            } catch { showNotice(error.localizedDescription) }
        }
    }

    func isReadingNotificationThread(accountID: String, threadID: String) -> Bool {
        guard NSApp.isActive, isVisible, calendarWorkspace == nil, composePane == nil,
              threads.indices.contains(selection) else { return false }
        return threads[selection].id == threadID && threads[selection].accountID == accountID
    }

    func openTrackedNotification(accountID: String, threadID: String) {
        guard composePane == nil else { makeKeyAndOrderFront(nil); return }
        pendingNotificationThreadID = threadID.isEmpty ? nil : threadID
        showInbox(scope: .sent)
        makeKeyAndOrderFront(nil)
    }

    func openFollowupNotification(accountID: String, threadID: String) {
        guard composePane == nil else { makeKeyAndOrderFront(nil); return }
        pendingNotificationThreadID = threadID
        showInbox(scope: .followups)
        makeKeyAndOrderFront(nil)
    }

    func openNotification(accountID: String, threadID: String) {
        if composePane != nil {
            // Preserve an active draft. The user can close it when ready.
            makeKeyAndOrderFront(nil)
            return
        }
        pendingNotificationThreadID = threadID.isEmpty ? nil : threadID
        showInbox(scope: accountID.isEmpty ? .all : .account(accountID))
        makeKeyAndOrderFront(nil)
    }

    /// Mark as Read / Archive from a notification button, without opening the window.
    func triageFromNotification(kind: String, accountID: String, threadID: String) {
        guard let core, !threadID.isEmpty else { return }
        let operation = UUID().uuidString
        Task { @MainActor in
            do {
                let result = try await core.callAsync(["op":"batch_triage", "operation_id":operation, "kind":kind, "thread_ids":[threadID]]) as? [String:Any]
                if result?["undoable"] as? Bool == true { lastUndoBatch = (operation, [accountID]) }
                syncStatus.stringValue = (kind == "archive" ? "Archived" : "Marked read") + " from notification · saved locally"
                refreshInbox(); updateTriageBar()
                if !accountID.isEmpty && !accountID.hasPrefix("demo:"),
                   let result = try? await core.callAsync(["op":"sync_account", "account_id":accountID]) {
                    scheduleQueuedActionRetry(result)
                }
            } catch {
                syncStatus.stringValue = "Notification action failed"
                syncStatus.toolTip = error.localizedDescription
            }
        }
    }

    /// Continue bounded action passes without waiting for the idle minute refresh.
    private func scheduleQueuedActionRetry(_ value: Any) {
        func delay(_ value: Any) -> Double? {
            if let rows = value as? [Any] { return rows.compactMap(delay).min() }
            guard let row = value as? [String: Any] else { return nil }
            if let data = row["data"] { return delay(data) }
            guard (row["pending_actions"] as? NSNumber)?.intValue ?? 0 > 0 else { return nil }
            return max(1, min(3600, ((row["actions_retry_after_ms"] as? NSNumber)?.doubleValue ?? 5000) / 1000))
        }
        guard let seconds = delay(value) else { return }
        queuedActionRetryTimer?.invalidate()
        let timer = Timer(timeInterval: seconds, repeats: false) { [weak self] _ in
            Task { @MainActor [weak self] in self?.syncAll(showErrors: false) }
        }
        RunLoop.main.add(timer, forMode: .common)
        queuedActionRetryTimer = timer
    }

    private func syncAll(showErrors: Bool = true) {
        guard let core else { showNotice("Mail engine unavailable."); return }
        guard !knownAccounts.isEmpty else { if showErrors { addAccount() }; return }
        guard !syncInProgress else { return }
        syncInProgress = true
        syncStatus.stringValue = "Syncing…"
        Task { @MainActor in
            defer { syncInProgress = false }
            do {
                await MailNotifications.shared.primeMailBaseline(core: core, accountIDs: knownAccounts.map { $0.id })
                let result = try await core.callAsync(["op": "sync_all"])
                scheduleQueuedActionRetry(result)
                refreshInbox()
                if let rows = result as? [[String: Any]] {
                    let successful = rows.filter { $0["error"] == nil && ($0["data"] as? [String: Any])?["sync_error"] == nil }.compactMap { $0["account_id"] as? String }
                    await MailNotifications.shared.recordSync(core: core, successfulAccountIDs: successful)
                    let names = Dictionary(uniqueKeysWithValues: knownAccounts.map { ($0.id, $0.email) })
                    let failures = rows.compactMap { row -> String? in
                        guard let error = row["error"] as? String ?? (row["data"] as? [String: Any])?["sync_error"] as? String else { return nil }
                        let id = row["account_id"] as? String ?? "Account"
                        return "\(names[id] ?? id): \(error)"
                    }
                    let warnings = rows.flatMap { row in
                        ((row["data"] as? [String: Any])?["warnings"] as? [String]) ?? []
                    }
                    if !failures.isEmpty {
                        let throttled = failures.allSatisfy { $0.localizedCaseInsensitiveContains("rate limit") }
                        syncStatus.stringValue = throttled ? "Gmail is catching up; retrying shortly" : "Some accounts failed to sync"
                        syncStatus.toolTip = failures.joined(separator: "\n")
                        if showErrors && !throttled { showNotice(failures.joined(separator: "\n")) }
                    } else if !warnings.isEmpty {
                        syncStatus.stringValue = "Mail updated; a change is still pending"
                        syncStatus.toolTip = warnings.joined(separator: "\n")
                    } else {
                        syncStatus.stringValue = "Updated just now"
                        syncStatus.toolTip = nil
                    }
                } else {
                    syncStatus.stringValue = "Updated just now"
                    syncStatus.toolTip = nil
                }
            } catch {
                syncStatus.stringValue = "Sync failed"
                syncStatus.toolTip = error.localizedDescription
                if showErrors { showNotice(error.localizedDescription) }
            }
        }
    }

    private func compose() {
        closeCalendarWorkspace()
        guard let core else { showNotice("Mail engine unavailable."); return }
        guard !knownAccounts.isEmpty else { addAccount(); return }
        showComposePane(core: core)
    }

    @objc private func composeClicked(_ sender: NSButton) { compose() }

    @objc private func supportClicked() { showSupport() }

    private func showSupport(feedback: Bool = false) {
        // Destinations come only from the distribution profile. Builds without
        // one explain that instead of falling back to anyone's address.
        guard let profile = DistributionProfile.current, let supportAddress = profile.supportAddress,
              let feedbackAddress = profile.feedbackAddress else {
            let alert = NSAlert()
            alert.messageText = "Support & feedback"
            alert.informativeText = DistributionProfile.supportUnavailableMessage
            alert.addButton(withTitle: "OK")
            // A single-button alert ignores Escape; dismiss on it as Linux does.
            let escape = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { event in
                guard event.keyCode == 53 else { return event }
                NSApp.stopModal(withCode: .alertFirstButtonReturn)
                return nil
            }
            alert.runModal()
            if let escape { NSEvent.removeMonitor(escape) }
            return
        }
        guard composePane == nil else { showNotice("Save and close your current draft before opening support or feedback."); return }
        let alert = NSAlert()
        alert.messageText = "Support & feedback"
        alert.informativeText = "Ask for help at \(supportAddress) or share an idea at \(feedbackAddress). Review and send the email yourself.\n\nUse Attach file in the composer to add screenshots or logs. Attachments are optional and can be removed. Check them for private information before sending."
        alert.addButton(withTitle: feedback ? "Give feedback" : "Ask for support")
        alert.addButton(withTitle: feedback ? "Ask for support" : "Give feedback")
        alert.addButton(withTitle: "Cancel")
        let info = "Email 0.1.0\nBuild: \(EmailBuildIdentity.value)\nmacOS \(ProcessInfo.processInfo.operatingSystemVersionString)"
        let options = NSStackView()
        options.orientation = .vertical; options.alignment = .leading; options.spacing = 8
        let include = NSButton(checkboxWithTitle: "Include basic app information", target: nil, action: nil)
        include.state = .off
        options.addArrangedSubview(include)
        let preview = NSTextField(wrappingLabelWithString: info + "\n\nOnly the information above is included. No mail, accounts or logs are collected.")
        preview.font = .systemFont(ofSize: 11); preview.textColor = .secondaryLabelColor
        preview.isSelectable = true
        options.addArrangedSubview(preview)
        options.frame = NSRect(x: 0, y: 0, width: 360, height: 120)
        preview.preferredMaxLayoutWidth = 360
        alert.accessoryView = options
        let result = alert.runModal()
        guard result == .alertFirstButtonReturn || result == .alertSecondButtonReturn else { return }
        let isFeedback = result == .alertFirstButtonReturn ? feedback : !feedback
        let recipient = isFeedback ? feedbackAddress : supportAddress
        let subject = isFeedback ? "Email feedback" : "Email support request"
        var body = isFeedback ? "My idea or feedback:\n\n" : "What happened?\n\nWhat did you expect?\n\nSteps to reproduce (if relevant):\n\n"
        if include.state == .on { body += "\nApp information (optional):\n" + info + "\n" }
        guard let core, !knownAccounts.isEmpty else {
            var url = URLComponents(); url.scheme = "mailto"; url.path = recipient
            url.queryItems = [URLQueryItem(name: "subject", value: subject), URLQueryItem(name: "body", value: body)]
            if let link = url.url, NSWorkspace.shared.open(link) { return }
            showNotice("Could not open a mail app. Email \(recipient), or add an account to Email and try again.")
            return
        }
        closeCalendarWorkspace()
        showComposePane(core: core, recipient: recipient, supportMessage: (subject, body))
    }


    private func openCommands() {
        clearGoTo()
        paletteReturnResponder = firstResponder
        palettePage = .root
        paletteRootQuery = ""
        paletteResults = []
        paletteLoadGeneration += 1
        let generation = paletteLoadGeneration
        paletteSearch.stringValue = ""
        refreshPalette()
        checkSavedOpenActivity() // tracked mail can arrive after launch
        let visible = screen?.visibleFrame ?? NSScreen.main?.visibleFrame ?? frame
        let origin = NSPoint(x: min(max(frame.midX - palette.frame.width / 2, visible.minX + 8), visible.maxX - palette.frame.width - 8),
                             y: min(max(frame.maxY - palette.frame.height - 72, visible.minY + 8), visible.maxY - palette.frame.height - 8))
        palette.setFrameOrigin(origin)
        palette.makeKeyAndOrderFront(nil)
        paletteSearch.becomeFirstResponder()
        if let core {
            Task { @MainActor in
                guard let labels = try? await core.callAsync(["op": "list_labels"]) as? [[String: Any]],
                      generation == paletteLoadGeneration else { return }
                paletteLabels = labels.compactMap { $0["name"] as? String }
                if palette.isVisible { refreshPalette() }
            }
        }
    }

    @objc private func openCommandsClicked(_ sender: NSButton) { openCommands() }

    @objc private func archiveClicked(_ sender: NSButton) { archiveSelection() }

    @objc private func replyClicked(_ sender: NSButton) { reply() }
    @objc private func replyAllClicked(_ sender: NSButton) { reply(all: true) }

    @objc private func toggleContactPaneClicked(_ sender: NSButton) { toggleContactPane() }

    private func toggleContactPane() {
        if activityController != nil { closeActivityPane() }
        contactPanePreference = contactPane.isHidden
        updateContactPaneVisibility()
        if !contactPane.isHidden, folder == .inbox, threads.indices.contains(selection) {
            refreshContactPane(for: threads[selection]); updateTrackingButton()
        }
    }

    private func updateContactPaneVisibility() {
        let selected = activityController == nil && folder == .inbox && !sender.stringValue.isEmpty && composePane == nil
        var hasRelatedMail = false
        if threads.indices.contains(selection) {
            let selected = threads[selection]
            let address = ContactPane.address(from: selected.sender).lowercased()
            hasRelatedMail = threads.contains { $0.id != selected.id && ContactPane.address(from: $0.sender).lowercased() == address }
        }
        let preferred = contactPanePreference ?? (frame.width >= 1180 && hasRelatedMail)
        contactPane.isHidden = !selected || !preferred
        contactButton.contentTintColor = contactPane.isHidden ? EmailTheme.muted : EmailTheme.accent
    }

    private func refreshContactPane(for thread: MailThread) {
        let address = ContactPane.address(from: thread.sender).lowercased()
        let recent = threads.filter {
            $0.id != nil && $0.id != thread.id && ContactPane.address(from: $0.sender).lowercased() == address
        }.prefix(4)
        let cached = contactHistory[address]?.filter { $0.id != thread.id }.prefix(4)
        let googleAccountID = knownAccounts.first(where: { $0.id == thread.accountID && $0.provider == "gmail" })?.id
        contactPane.show(sender: thread.sender, recent: cached.map(Array.init) ?? Array(recent), googleAccountID: googleAccountID, core: core)
        updateContactPaneVisibility()
        guard !contactPane.isHidden, contactHistory[address] == nil else { return }
        guard let core, let selectedID = thread.id else { return }
        Task { @MainActor in
            let matches = (try? await Task.detached(priority: .utility) {
                try core.threadsChecked(query: address, limit: 25)
            }.value) ?? []
            guard displayedThreadID == selectedID else { return }
            let history = matches.filter { $0.id != nil && ContactPane.address(from: $0.sender).lowercased() == address }
            if contactHistory.count > 100 { contactHistory.removeAll(keepingCapacity: true) }
            contactHistory[address] = history
            contactPane.show(sender: thread.sender, recent: Array(history.filter { $0.id != selectedID }.prefix(4)), googleAccountID: googleAccountID, core: core)
        }
    }

    private func reply(all: Bool = false) {
        guard let core else { showNotice("Mail engine unavailable."); return }
        guard folder == .inbox, selection < threads.count else { return }
        let thread = threads[selection]
        let generation = messageLoadGeneration
        let fallbackID = currentMessageID
        guard let webView, !webView.isHidden else {
            reply(to: fallbackID, in: thread, using: core, all: all)
            return
        }
        let selectedID = "document.activeElement?.closest('details[data-message-id]')?.getAttribute('data-message-id') || document.querySelector('details[open][data-message-id]')?.getAttribute('data-message-id')"
        webView.evaluateJavaScript(selectedID) { [weak self] result, _ in
            Task { @MainActor [weak self] in
                guard let self, generation == self.messageLoadGeneration else { return }
                self.reply(to: (result as? String) ?? fallbackID, in: thread, using: core, all: all)
            }
        }
    }

    private func reply(to messageID: String?, in thread: MailThread, using core: CoreBridge, all: Bool) {
        let message = messageID.flatMap { core.call(["op": "get_message", "id": $0]) as? [String: Any] }
        let sameThread = message?["thread_id"] as? String == thread.id
        let headers = message?["recipient_headers"] as? [String: Any]
        let sender = sameThread ? (message?["sender"] as? String ?? thread.sender) : thread.sender
        let own = ComposePane.ownAddresses(for: knownAccounts)
        let originalTo = sameThread ? (message?["to"] as? [String] ?? []) : []
        let originalCc = sameThread ? (headers?["cc"] as? [String] ?? []) : []
        let recipients = ComposerAddressRules.replyRecipients(sender: sender, replyTo: sameThread ? (headers?["reply_to"] as? [String] ?? []) : [], to: originalTo, cc: originalCc, own: own, replyAll: all)
        let replySubject = sameThread ? (message?["subject"] as? String ?? thread.subject) : thread.subject
        let senderIsOwn = own.contains { ContactPane.address(from: $0).caseInsensitiveCompare(ContactPane.address(from: sender)) == .orderedSame }
        let receivingAddresses = (senderIsOwn ? [sender] : originalTo + originalCc).map { ContactPane.address(from: $0) }
        showComposePane(core: core, accountID: thread.accountID, preferredFromAddresses: receivingAddresses, replyTo: recipients.to.joined(separator: ", "), replySubject: replySubject, replyCc: recipients.cc, replyAll: all, replyThreadID: thread.id)
    }

    private func showComposePane(core: CoreBridge, accountID: String? = nil, preferredFromAddresses: [String] = [], recipient: String? = nil, replyTo: String? = nil, replySubject: String? = nil, replyCc: [String] = [], replyAll: Bool = false, replyThreadID: String? = nil, draft: MailDraft? = nil, supportMessage: (subject: String, body: String)? = nil) {
        guard let readerView else { return }
        dismissComposePane()
        let pane = ComposePane(core: core, accounts: knownAccounts, accountID: accountID, preferredFromAddresses: preferredFromAddresses, recipient: recipient, replyTo: replyTo, replySubject: replySubject, replyCc: replyCc, replyAll: replyAll, replyThreadID: replyThreadID, draft: draft, supportMessage: supportMessage, onChanged: { [weak self] in
            guard let self else { return }
            if self.folder == .drafts { self.showDrafts() } else { self.refreshInbox() }
        }, onQueued: { [weak self] id, scheduled, feedback in
            guard let self else { return }
            if self.folder == .drafts { self.showInbox(scope: .all) }
            self.lastQueuedMessageID = id
            self.syncStatus.stringValue = scheduled ? "Scheduled · View in Outbox" : "Sending in 10 seconds · Undo send"
            self.cancelSendButton.title = scheduled ? "Cancel send" : "Undo send"
            self.cancelSendButton.isHidden = false
            if feedback, let host = self.contentView { FeedbackThanks.show(in: host) }
        }, onDiscarded: { [weak self] id, subject in
            guard let self else { return }
            self.dismissComposePane()
            self.draftDiscarded(id: id, subject: subject, selectRow: nil)
            self.updateTriageBar()
            if self.folder != .drafts { self.refreshInbox() }
        }, onClose: { [weak self] in self?.dismissComposePane() })
        composePane = pane
        updateReaderShortcuts()
        updateContactPaneVisibility()
        readerView.addSubview(pane)
        if replyTo != nil {
            readerBodyBottomConstraint?.isActive = false
            pane.wantsLayer = true
            pane.layer?.borderColor = EmailTheme.cgColor(EmailTheme.line, in: effectiveAppearance)
            pane.layer?.borderWidth = 1
            pane.layer?.cornerRadius = 12
            let constraint = readerBody.bottomAnchor.constraint(equalTo: pane.topAnchor, constant: -8)
            replyBodyBottomConstraint = constraint
            NSLayoutConstraint.activate([
                pane.trailingAnchor.constraint(lessThanOrEqualTo: readerView.trailingAnchor, constant: -22),
                pane.widthAnchor.constraint(lessThanOrEqualToConstant: 780),
                pane.centerXAnchor.constraint(equalTo: readerView.centerXAnchor),
                pane.bottomAnchor.constraint(equalTo: readerKeyboardHint.topAnchor, constant: -10),
                constraint
            ])
            let preferredReplyHeight = pane.heightAnchor.constraint(equalToConstant: 220)
            // Below AppKit's windowSizeStayPut priority: fit the editor to
            // the user's window rather than resizing the window to this target.
            let editorPreference = NSLayoutConstraint.Priority(rawValue: NSLayoutConstraint.Priority.windowSizeStayPut.rawValue - 1)
            preferredReplyHeight.priority = editorPreference
            preferredReplyHeight.isActive = true
            pane.onDetailsVisibilityChanged = { expanded in preferredReplyHeight.constant = expanded ? 380 : 220 }
            // Message context and composer compete inside the existing window,
            // never by increasing the window's content fitting height.
            pane.topAnchor.constraint(greaterThanOrEqualTo: readerBody.topAnchor).isActive = true
            let preferredContextHeight = pane.topAnchor.constraint(greaterThanOrEqualTo: readerBody.topAnchor, constant: 80)
            preferredContextHeight.priority = .defaultLow; preferredContextHeight.isActive = true
            pane.heightAnchor.constraint(greaterThanOrEqualToConstant: 170).isActive = true
            let fillAvailableWidth = pane.leadingAnchor.constraint(equalTo: readerView.leadingAnchor, constant: 22)
            // The 780-point reply cap must leave margins in a wide reader,
            // not shrink the entire window to satisfy this preferred edge.
            fillAvailableWidth.priority = editorPreference
            fillAvailableWidth.isActive = true
            pane.focusBody(in: self)
        } else {
            setRemoteImagesNoticeVisible(false)
            NSLayoutConstraint.activate([
                pane.leadingAnchor.constraint(equalTo: readerView.leadingAnchor),
                pane.trailingAnchor.constraint(equalTo: readerView.trailingAnchor),
                pane.topAnchor.constraint(equalTo: readerView.topAnchor),
                pane.bottomAnchor.constraint(equalTo: readerView.bottomAnchor)
            ])
            if supportMessage != nil { pane.focusBody(in: self) } else { pane.focusInitialField(in: self) }
        }
    }

    // Runs with a disposable demo store; never sends or saves mail.
    func smokeComposerWindowLayout() -> Bool {
        guard let core else { return false }
        makeKeyAndOrderFront(nil)
        func settle() {
            contentView?.layoutSubtreeIfNeeded()
            RunLoop.main.run(until: Date(timeIntervalSinceNow: 0.15))
        }
        var passed = true
        for size in [NSSize(width: 1500, height: 900), NSSize(width: 1100, height: 760), NSSize(width: 760, height: 600)] {
            for contacts in [false, true] {
                dismissComposePane()
                contactPanePreference = contacts
                updateContactPaneVisibility()
                setContentSize(size)
                settle()
                let original = frame
                print("Checking requested \(size), actual \(frame.size), contacts=\(contacts)")
                func check(_ state: String) {
                    settle()
                    if frame != original {
                        print("FAIL \(state): \(original) -> \(frame)")
                        passed = false
                    }
                }
                showComposePane(core: core)
                check("compose")
                if !(firstResponder is NSTextView) { passed = false; print("FAIL compose focus") }
                dismissComposePane()
                check("close compose")
                showComposePane(core: core, replyTo: "fixture@example.test", replySubject: "Layout check")
                check("reply")
                if !(firstResponder is NSTextView) { passed = false; print("FAIL reply focus") }
                composePane?.focusCopyRecipient(in: self, blind: false)
                check("expanded reply")
                dismissComposePane()
                check("close reply")
                if firstResponder !== table { passed = false; print("FAIL inbox focus") }
            }
        }
        print(passed ? "Composer window geometry passed" : "Composer window geometry failed")
        return passed
    }

    private func dismissComposePane() {
        let wasComposing = composePane != nil
        replyBodyBottomConstraint?.isActive = false
        replyBodyBottomConstraint = nil
        readerBodyBottomConstraint?.isActive = true
        composePane?.removeFromSuperview()
        composePane = nil
        if let currentHTML, !readerBody.isHidden {
            setRemoteImagesNoticeVisible(RemoteImagePreference.containsRemoteImages(currentHTML) && !allowRemoteImagesForMessage)
        }
        if wasComposing { makeFirstResponder(table) }
        updateReaderShortcuts()
        updateContactPaneVisibility()
    }

    private func archiveSelection(allowRestore: Bool = true) {
        if !markedThreadIDs.isEmpty {
            if inboxScope == .archived && !allowRestore { syncStatus.stringValue = "Already archived · ⇧E to move to Inbox"; return }
            performTriage(inboxScope == .archived ? "restore" : "archive"); return
        }
        if inboxScope == .archived && !allowRestore {
            syncStatus.stringValue = "Already archived · ⇧E to move to Inbox"
            return
        }
        guard folder == .inbox, selection < threads.count else { return }
        if usesCore, threads[selection].id != nil, core != nil {
            performTriage(inboxScope == .archived ? "restore" : "archive")
        } else { removeSelectedThreadFromInbox() }
    }

    private func restoreTriageUndo() {
        guard let saved = core?.call(["op":"latest_triage_undo"]) as? [String:Any],
              let id = saved["operation_id"] as? String,
              let accounts = saved["account_ids"] as? [String] else { lastUndoBatch = nil; return }
        lastUndoBatch = (id, accounts)
    }

    private func updateTriageBar() {
        guard triageButtons.count == 5 else { return }
        let selecting = !markedThreadIDs.isEmpty
        if folder == .drafts {
            triageBar.isHidden = lastDiscardedDraft == nil
            triageCount.stringValue = "Draft discarded"
            for (index, button) in triageButtons.enumerated() { button.isHidden = index != 3 }
            triageButtons[3].toolTip = "Restore the discarded draft (Z)"
            triageButtons.forEach { $0.isEnabled = !draftDiscardBusy }
            updateReaderShortcuts()
            return
        }
        triageButtons[3].toolTip = nil
        triageButtons[4].isHidden = false
        let show = folder == .inbox && (selecting || lastUndoBatch != nil)
        triageBar.isHidden = !show
        triageCount.stringValue = selecting ? "\(markedThreadIDs.count) selected" : "Change saved"
        triageButtons[0].isHidden = !selecting
        triageButtons[0].toolTip = "Select all loaded conversations (⌘A)"
        triageButtons[1].isHidden = !selecting
        triageButtons[2].isHidden = !selecting || inboxScope == .archived
        triageButtons[3].isHidden = lastUndoBatch == nil
        triageButtons.forEach { $0.isEnabled = !triageBusy }
        updateReaderShortcuts()
    }
    @objc private func toggleMarkedRow(_ button: NSButton) {
        guard !triageBusy, threads.indices.contains(button.tag), let id = threads[button.tag].id else { return }
        if markedThreadIDs.contains(id) { markedThreadIDs.remove(id) } else { markedThreadIDs.insert(id) }
        updateTriageBar()
    }
    private func toggleMarkedSelection() {
        guard !triageBusy, folder == .inbox, threads.indices.contains(selection), let id = threads[selection].id else { return }
        if markedThreadIDs.contains(id) { markedThreadIDs.remove(id) } else { markedThreadIDs.insert(id) }
        table.reloadData(forRowIndexes: IndexSet(integer: selection), columnIndexes: IndexSet(integer: 0))
        updateTriageBar()
    }
    private func extendMarkedSelection(_ delta: Int) {
        guard folder == .inbox else { return }
        if threads.indices.contains(selection), let id = threads[selection].id { markedThreadIDs.insert(id) }
        select(selection + delta)
        if threads.indices.contains(selection), let id = threads[selection].id { markedThreadIDs.insert(id) }
        table.reloadData()
        updateTriageBar()
    }
    @objc private func selectLoadedClicked() {
        guard folder == .inbox else { return }
        markedThreadIDs = Set(threads.compactMap { $0.id })
        table.reloadData(); updateTriageBar()
        syncStatus.stringValue = "\(markedThreadIDs.count) loaded conversations selected"
    }
    @objc private func clearMarkedClicked() {
        if markedThreadIDs.isEmpty { lastUndoBatch = nil }
        markedThreadIDs.removeAll()
        table.reloadData(); updateTriageBar()
        makeFirstResponder(table)
    }
    @objc private func readMarkedClicked() { performTriage("read") }
    @objc private func archiveMarkedClicked() { performTriage("archive") }
    private func performTriage(_ kind: String) {
        guard !triageBusy, folder == .inbox, let core else { return }
        let targets = markedThreadIDs.isEmpty ? (threads.indices.contains(selection) ? [threads[selection]] : []) : threads.filter { $0.id.map(markedThreadIDs.contains) == true }
        let ids = targets.compactMap { $0.id }
        guard !ids.isEmpty else { return }
        let accounts = Array(Set(targets.compactMap { $0.accountID }))
        let operation = UUID().uuidString
        triageBusy = true; updateTriageBar()
        Task { @MainActor in
            do {
                let result = try await core.callAsync(["op":"batch_triage", "operation_id":operation, "kind":kind, "thread_ids":ids]) as? [String:Any]
                if result?["undoable"] as? Bool == true { lastUndoBatch = (operation,accounts) }
                else { restoreTriageUndo() }
                markedThreadIDs.removeAll()
                syncStatus.stringValue = "\(ids.count) conversation\(ids.count == 1 ? "" : "s") updated · saved locally"
                triageBusy = false; refreshInbox(); updateTriageBar()
                for id in accounts where !id.hasPrefix("demo:") {
                    do { let result = try await core.callAsync(["op":"sync_account","account_id":id]); scheduleQueuedActionRetry(result) }
                    catch { syncStatus.stringValue = "Saved locally · sync will retry"; syncStatus.toolTip = error.localizedDescription }
                }
            } catch {
                triageBusy = false; updateTriageBar()
                syncStatus.stringValue = "Selection unchanged · action failed"
                syncStatus.toolTip = error.localizedDescription
            }
        }
    }
    @objc private func undoTriageClicked() {
        if folder == .drafts { undoDiscardDraft(); return }
        guard !triageBusy, let undo = lastUndoBatch, let core else { return }
        triageBusy = true; updateTriageBar()
        Task { @MainActor in
            do {
                _ = try await core.callAsync(["op":"undo_triage","operation_id":undo.id])
                restoreTriageUndo()
                triageBusy = false; refreshInbox(); updateTriageBar()
                syncStatus.stringValue = "Action undone · saved locally"
                for id in undo.accounts where !id.hasPrefix("demo:") {
                    if let result = try? await core.callAsync(["op":"sync_account","account_id":id]) { scheduleQueuedActionRetry(result) }
                }
            } catch {
                triageBusy = false; updateTriageBar()
                syncStatus.stringValue = "Could not undo · newer changes are preserved"
                syncStatus.toolTip = error.localizedDescription
            }
        }
    }

    private func removeSelectedThreadFromInbox() {
        guard threads.indices.contains(selection) else { return }
        threads.remove(at: selection)
        suppressSelectionChanges = true
        table.reloadData()
        updateEmptyInbox()
        if !threads.isEmpty { select(min(selection, threads.count - 1)) }
        suppressSelectionChanges = false
        if threads.isEmpty { clearReader() }
        else { show(threads[selection]) }
    }

    private func contextMenu(for row: Int) -> NSMenu? {
        if folder == .drafts {
            guard filteredDrafts.indices.contains(row) else { return nil }
            let menu = NSMenu(title: "Draft actions")
            for (title, action) in [("Edit draft   ↵", #selector(editDraftFromMenu(_:))), ("Discard draft   ⌫", #selector(discardDraftFromMenu(_:)))] {
                let item = NSMenuItem(title: title, action: action, keyEquivalent: "")
                item.target = self
                menu.addItem(item)
            }
            return menu
        }
        guard folder == .inbox, threads.indices.contains(row) else { return nil }
        // A click outside the marked set starts a single-conversation context.
        // Inside it, preserve the batch and name its scope on every action.
        if let id = threads[row].id, !markedThreadIDs.isEmpty, !markedThreadIDs.contains(id) {
            clearMarkedClicked()
        }
        if selection != row { select(row) }
        let menu = NSMenu(title: "Message actions")
        func add(_ title: String, _ action: Selector) {
            let item = NSMenuItem(title: title, action: action, keyEquivalent: "")
            item.target = self
            menu.addItem(item)
        }
        let count = markedThreadIDs.count
        if count > 0 {
            add(inboxScope == .archived ? "Move \(count) selected to Inbox" : "Archive \(count) selected", #selector(archiveFromMenu(_:)))
            add("Move \(count) selected to Spam", #selector(spamFromMenu(_:)))
            menu.addItem(.separator())
            add("Mark \(count) selected as read", #selector(readFromMenu(_:)))
            add("Mark \(count) selected as unread", #selector(unreadFromMenu(_:)))
            return menu
        }
        add(inboxScope == .archived ? "Move to Inbox" : "Archive", #selector(archiveFromMenu(_:)))
        add("Move to Spam", #selector(spamFromMenu(_:)))
        menu.addItem(.separator())
        add(threads[row].unread ? "Mark as read" : "Mark as unread", #selector(toggleReadFromMenu(_:)))
        add("Mark all from this sender as read…", #selector(bulkReadSenderClicked(_:)))
        add(threads[row].pinned ? "Remove from Favorites" : "Add to Favorites", #selector(togglePinFromMenu(_:)))
        add("Reply", #selector(replyFromMenu(_:)))
        add("Reply all   ⇧R", #selector(replyAllFromMenu(_:)))
        if unsubscribeURL != nil { add("Unsubscribe…", #selector(unsubscribeFromMenu(_:))) }
        return menu
    }

    @objc private func editDraftFromMenu(_ sender: NSMenuItem) { editSelectedDraft() }
    @objc private func discardDraftFromMenu(_ sender: NSMenuItem) { discardSelectedDraft() }
    @objc private func archiveFromMenu(_ sender: NSMenuItem) { archiveSelection() }
    @objc private func spamFromMenu(_ sender: NSMenuItem) { moveToSpamSelection() }
    @objc private func readFromMenu(_ sender: NSMenuItem) { setRead(true) }
    @objc private func unreadFromMenu(_ sender: NSMenuItem) { setRead(false) }
    @objc private func toggleReadFromMenu(_ sender: NSMenuItem) {
        guard selection < threads.count else { return }
        setRead(threads[selection].unread)
    }
    @objc private func replyFromMenu(_ sender: NSMenuItem) { reply() }
    @objc private func replyAllFromMenu(_ sender: NSMenuItem) { reply(all: true) }
    @objc private func togglePinFromMenu(_ sender: NSMenuItem) { togglePinSelection() }
    @objc private func unsubscribeFromMenu(_ sender: NSMenuItem) { unsubscribeSelection() }

    private func moveToSpamSelection() { performTriage("spam") }

    private func sidebarButton(_ title: String, symbol: String? = nil, action: Selector, active: Bool = false, primary: Bool = false) -> NSButton {
        let button = NSButton(title: title, target: self, action: action)
        button.isBordered = false
        button.alignment = .left
        button.wantsLayer = true
        button.layer?.cornerRadius = 7
        button.layer?.backgroundColor = active ? EmailTheme.cgColor(EmailTheme.accentSoft, in: effectiveAppearance) : NSColor.clear.cgColor
        let color = primary || active ? EmailTheme.accent : EmailTheme.ink
        button.attributedTitle = NSAttributedString(string: title, attributes: [.foregroundColor: color, .font: NSFont.systemFont(ofSize: 12.5, weight: active ? .semibold : .regular)])
        if let symbol {
            let image = NSImage(systemSymbolName: symbol, accessibilityDescription: nil)?.withSymbolConfiguration(.init(pointSize: 13, weight: .regular))
            image?.size = NSSize(width: 17, height: 17)
            button.image = image
            button.imagePosition = .imageLeading
            button.contentTintColor = color
        }
        button.widthAnchor.constraint(equalToConstant: 136).isActive = true
        button.heightAnchor.constraint(equalToConstant: 29).isActive = true
        return button
    }

    private func updateMailboxCounts() {
        let due = unreadCounts["__followup_due"] ?? 0
        let totalUnread = unreadCounts["__total"] ?? 0
        // Dock badge mirrors the sidebar's All Inboxes count, as in Mail.
        NSApp.dockTile.badgeLabel = totalUnread > 0 ? (totalUnread > 999 ? "999+" : String(totalUnread)) : nil
        if mailboxCountBadges.count == 2 {
            for (index, count) in [totalUnread, due].enumerated() {
                mailboxCountBadges[index].stringValue = count > 999 ? "999+" : (count > 0 ? String(count) : "")
                mailboxCountBadges[index].isHidden = awaitingGoTo || count == 0
            }
        }
        inboxSidebarButton?.toolTip = "\(totalUnread.formatted()) unread conversations in downloaded inboxes · G then I"
        inboxSidebarButton?.setAccessibilityLabel("All Inboxes, \(totalUnread) unread")
        followupsSidebarButton?.title = "Follow-ups"
        followupsSidebarButton?.toolTip = "\(due.formatted()) due · \((unreadCounts["__followup_active"] ?? 0).formatted()) waiting for follow-up · G then W"
        followupsSidebarButton?.setAccessibilityLabel("Follow-ups, \(due) due")
    }

    private func updateSidebarSelection() {
        archiveButton.image = NSImage(systemSymbolName: inboxScope == .archived ? "tray.and.arrow.up" : "archivebox", accessibilityDescription: inboxScope == .archived ? "Move to Inbox" : "Archive")
        archiveButton.toolTip = inboxScope == .archived ? "Move to Inbox (⇧E)" : "Archive (E)"
        archiveButton.setAccessibilityLabel(inboxScope == .archived ? "Move to Inbox" : "Archive")
        updateReaderShortcuts()
        let showScopeTabs = folder == .inbox
        scopeTabs.isHidden = !showScopeTabs
        scopeTabButtons.forEach { $0.isHidden = !showScopeTabs }
        for (index, button) in scopeTabButtons.enumerated() {
            let active = folder == .inbox && (index == 0 ? inboxScope == .all : (index == 1 ? inboxScope == .unread : inboxScope == .pinned))
            let color = active ? EmailTheme.accent : EmailTheme.muted
            button.attributedTitle = NSAttributedString(string: button.title, attributes: [.foregroundColor: color, .font: NSFont.systemFont(ofSize: 11.5, weight: active ? .semibold : .medium)])
            button.layer?.backgroundColor = active ? EmailTheme.cgColor(EmailTheme.accentSoft, in: effectiveAppearance) : NSColor.clear.cgColor
            button.subviews.compactMap { $0 as? ShortcutBadge }.forEach { $0.textColor = color }
        }
        updateMailboxCounts()
        let destinations: [(NSButton?, Bool)] = [
            (inboxSidebarButton, folder == .inbox && (inboxScope == .all || inboxScope == .unread || inboxScope == .pinned)),
            (unreadSidebarButton, folder == .inbox && inboxScope == .unread),
            (pinnedSidebarButton, folder == .inbox && inboxScope == .pinned),
            (snoozedSidebarButton, folder == .inbox && inboxScope == .snoozed),
            (followupsSidebarButton, folder == .inbox && inboxScope == .followups),
            (draftsSidebarButton, folder == .drafts),
            (outboxSidebarButton, folder == .outbox),
            (archiveSidebarButton, folder == .inbox && inboxScope == .archived),
            (calendarSidebarButton, calendarWorkspace != nil),
            (sentSidebarButton, folder == .inbox && inboxScope == .sent)
        ]
        for (index, destination) in destinations.enumerated() {
            let (button, requestedActive) = destination
            let active = button === calendarSidebarButton ? requestedActive : requestedActive && calendarWorkspace == nil
            guard let button else { continue }
            let color = active ? EmailTheme.accent : EmailTheme.ink
            button.layer?.backgroundColor = active ? EmailTheme.cgColor(EmailTheme.accentSoft, in: effectiveAppearance) : NSColor.clear.cgColor
            button.attributedTitle = NSAttributedString(string: button.title, attributes: [.foregroundColor: color, .font: NSFont.systemFont(ofSize: 12.5, weight: active ? .semibold : .regular)])
            button.contentTintColor = color
            if goToBadges.indices.contains(index) {
                goToBadges[index].textColor = active ? EmailTheme.accent : EmailTheme.muted
            }
        }
        let requestsActive = folder == .agentRequests && calendarWorkspace == nil
        if let button = agentRequestsSidebarButton {
            let color = requestsActive ? EmailTheme.accent : EmailTheme.ink
            button.layer?.backgroundColor = requestsActive ? EmailTheme.cgColor(EmailTheme.accentSoft, in: effectiveAppearance) : NSColor.clear.cgColor
            button.attributedTitle = NSAttributedString(string: button.title, attributes: [.foregroundColor: color, .font: NSFont.systemFont(ofSize: 12.5, weight: requestsActive ? .semibold : .regular)])
            button.contentTintColor = color
        }
        agentRequestReader.isHidden = !requestsActive
        // No search or filters in Agent requests; they come back on leaving.
        let searchHidden = folder == .agentRequests
        search.isHidden = searchHidden
        searchFiltersButton.isHidden = searchHidden
        controlsBelowStatus?.isActive = false
        controlsBelowSearch?.isActive = !searchHidden
        controlsBelowStatus?.isActive = searchHidden
        refreshSidebar()
    }

    @objc private func createSplitClicked() { editSplit(nil) }
    @objc private func editSplitClicked(_ sender: NSMenuItem) {
        guard let id = sender.representedObject as? String else { return }
        editSplit(splits.first(where: { $0.id == id }))
    }
    private func editSplit(_ existing: MailSplit?) {
        guard let core else { return }
        let sender = threads.indices.contains(selection) ? ContactPane.address(from: threads[selection].sender) : nil
        let editor = SplitEditorWindow(core: core, existing: existing, saved: core.splits(), sender: sender) { [weak self] id in
            self?.refreshSidebar()
            self?.showInbox(scope: .split(id))
        }
        splitEditor = editor
        editor.showWindow(nil)
        editor.window?.makeKeyAndOrderFront(nil)
    }
    @objc private func deleteSplitClicked(_ sender: NSMenuItem) {
        guard let core, let id = sender.representedObject as? String else { return }
        let alert = NSAlert()
        alert.messageText = "Delete this split?"
        alert.informativeText = "Messages stay in your account. Only the saved view is removed."
        alert.addButton(withTitle: "Delete split")
        alert.addButton(withTitle: "Cancel")
        guard alert.runModal() == .alertFirstButtonReturn else { return }
        Task { @MainActor in
            do {
                _ = try await core.callAsync(["op": "delete_split", "id": id])
                splits = core.splits()
                if inboxScope == .split(id) { showInbox(scope: .all) }
                else { refreshSidebar() }
            } catch { showNotice(error.localizedDescription) }
        }
    }

    private func updateEmptyInbox() {
        let empty = switch folder {
        case .drafts: filteredDrafts.isEmpty
        case .outbox: filteredOutbox.isEmpty
        case .agentRequests: agentRequests.isEmpty
        case .inbox: threads.isEmpty
        }
        inboxScroll.isHidden = empty
        inboxEmpty.isHidden = !empty || (folder == .outbox && search.stringValue.isEmpty) || hasNoAccounts
        inboxEmpty.stringValue = switch folder {
        case .drafts: search.stringValue.isEmpty ? "No drafts yet." : "No drafts match your search."
        case .outbox: search.stringValue.isEmpty ? "Nothing scheduled to send." : "No outgoing messages match."
        case .agentRequests: "Nothing waiting."
        case .inbox: search.stringValue.isEmpty ? (inboxScope == .pinned ? "No favorites yet." : "No messages in this view.") : "No messages match your search."
        }
    }

    private var filteredDrafts: [MailDraft] {
        let query = search.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        return query.isEmpty ? drafts : drafts.filter { $0.to.localizedCaseInsensitiveContains(query) || $0.subject.localizedCaseInsensitiveContains(query) || $0.bodyPlain.localizedCaseInsensitiveContains(query) }
    }

    private var filteredOutbox: [OutboxItem] {
        let query = search.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        return query.isEmpty ? outboxItems : outboxItems.filter { $0.to.localizedCaseInsensitiveContains(query) || $0.subject.localizedCaseInsensitiveContains(query) }
    }

    private func heading(_ string: String, size: CGFloat, weight: NSFont.Weight, color: NSColor = EmailTheme.ink) -> NSTextField {
        let label = NSTextField(labelWithString: string)
        label.font = .systemFont(ofSize: size, weight: weight)
        label.textColor = color
        return label
    }

    func numberOfRows(in tableView: NSTableView) -> Int {
        switch folder {
        case .drafts: filteredDrafts.count
        case .outbox: filteredOutbox.count
        case .agentRequests: agentRequests.count
        case .inbox: threads.count
        }
    }

    func tableView(_ tableView: NSTableView, rowViewForRow row: Int) -> NSTableRowView? {
        MailRowView()
    }

    private func listLabel(_ text: String, size: CGFloat, weight: NSFont.Weight, color: NSColor = EmailTheme.ink) -> NSTextField {
        let label = heading(text, size: size, weight: weight, color: color)
        label.maximumNumberOfLines = 1
        label.lineBreakMode = .byTruncatingTail
        label.usesSingleLineMode = true
        return label
    }

    private func displayName(_ address: String) -> String {
        guard let bracket = address.firstIndex(of: "<") else { return address }
        let name = address[..<bracket].trimmingCharacters(in: .whitespacesAndNewlines)
            .trimmingCharacters(in: CharacterSet(charactersIn: "\""))
        return name.isEmpty ? address : name
    }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        if folder == .agentRequests {
            return AgentRequestRows.cell(agentRequests[row])
        }
        if folder == .outbox {
            let item = filteredOutbox[row]
            let cell = NSTableCellView()
            let recipient = listLabel(item.to.isEmpty ? "No recipient" : "To: \(item.to)", size: 12, weight: .semibold)
            let subject = listLabel(item.subject.isEmpty ? "No subject" : item.subject, size: 12, weight: .regular)
            let when = DateFormatter.localizedString(from: item.sendAt, dateStyle: .short, timeStyle: .short)
            let detail = item.lastError ?? (item.status == "undoable" ? "Undo available · sends \(when)" : "Scheduled for \(when)")
            let status = listLabel(detail, size: 11, weight: .regular, color: item.lastError == nil ? EmailTheme.muted : .systemRed)
            for view in [recipient, subject, status] { view.translatesAutoresizingMaskIntoConstraints = false; cell.addSubview(view) }
            NSLayoutConstraint.activate([
                recipient.leadingAnchor.constraint(equalTo: cell.leadingAnchor, constant: 18), recipient.trailingAnchor.constraint(equalTo: cell.trailingAnchor, constant: -18), recipient.topAnchor.constraint(equalTo: cell.topAnchor, constant: 6),
                subject.leadingAnchor.constraint(equalTo: recipient.leadingAnchor), subject.trailingAnchor.constraint(equalTo: recipient.trailingAnchor), subject.topAnchor.constraint(equalTo: recipient.bottomAnchor, constant: 2),
                status.leadingAnchor.constraint(equalTo: recipient.leadingAnchor), status.trailingAnchor.constraint(equalTo: recipient.trailingAnchor), status.topAnchor.constraint(equalTo: subject.bottomAnchor, constant: 2)
            ])
            return cell
        }
        if folder == .drafts {
            let draft = filteredDrafts[row]
            let cell = NSTableCellView()
            let recipient = listLabel(ComposerAddressRules.draftRecipientSummary(to: draft.to, cc: draft.cc, bcc: draft.bcc), size: 12, weight: .semibold)
            let subject = listLabel(draft.subject.isEmpty ? "No subject" : draft.subject, size: 12, weight: .regular)
            let preview = listLabel(draft.bodyPlain.isEmpty ? "Empty draft" : draft.bodyPlain, size: 11, weight: .regular, color: EmailTheme.muted)
            for view in [recipient, subject, preview] { view.translatesAutoresizingMaskIntoConstraints = false; cell.addSubview(view) }
            NSLayoutConstraint.activate([
                recipient.leadingAnchor.constraint(equalTo: cell.leadingAnchor, constant: 18), recipient.trailingAnchor.constraint(equalTo: cell.trailingAnchor, constant: -18), recipient.topAnchor.constraint(equalTo: cell.topAnchor, constant: 6),
                subject.leadingAnchor.constraint(equalTo: recipient.leadingAnchor), subject.trailingAnchor.constraint(equalTo: recipient.trailingAnchor), subject.topAnchor.constraint(equalTo: recipient.bottomAnchor, constant: 2),
                preview.leadingAnchor.constraint(equalTo: recipient.leadingAnchor), preview.trailingAnchor.constraint(equalTo: recipient.trailingAnchor), preview.topAnchor.constraint(equalTo: subject.bottomAnchor, constant: 2)
            ])
            return cell
        }
        let thread = threads[row]
        let cell = NSTableCellView()
        let check = NSButton(title: "", target: self, action: #selector(toggleMarkedRow(_:)))
        check.setButtonType(.toggle)
        check.isBordered = false
        check.imagePosition = .imageOnly
        check.image = NSImage(systemSymbolName: "square", accessibilityDescription: nil)
        check.alternateImage = NSImage(systemSymbolName: "checkmark.square.fill", accessibilityDescription: nil)
        check.contentTintColor = EmailTheme.muted
        check.setAccessibilityRole(.checkBox)
        check.tag = row
        check.state = thread.id.map { markedThreadIDs.contains($0) } == true ? .on : .off
        check.setAccessibilityLabel("Select conversation: \(thread.subject)")
        check.toolTip = "Select conversation (X) · ⇧J/⇧K extend"
        check.translatesAutoresizingMaskIntoConstraints = false
        cell.addSubview(check)
        let unreadDot = NSView()
        unreadDot.wantsLayer = true
        unreadDot.layer?.backgroundColor = EmailTheme.cgColor(EmailTheme.accent, in: effectiveAppearance)
        unreadDot.layer?.cornerRadius = 3.5
        unreadDot.isHidden = !thread.unread
        unreadDot.translatesAutoresizingMaskIntoConstraints = false
        cell.addSubview(unreadDot)
        let sender = listLabel(displayName(thread.sender), size: 12, weight: thread.unread ? .semibold : .regular)
        let subject = listLabel(thread.subject, size: 12, weight: thread.unread ? .semibold : .regular)
        let tags = thread.labels.isEmpty ? "" : thread.labels.prefix(2).map { "#\($0)" }.joined(separator: " ") + " · "
        let preview = listLabel(tags + thread.preview, size: 11, weight: .regular, color: EmailTheme.muted)
        let time = listLabel(thread.time, size: 10, weight: .regular, color: EmailTheme.muted)
        let favorite = NSButton(title: "", target: self, action: #selector(toggleFavoriteFromRow(_:)))
        favorite.tag = row
        favorite.isBordered = false
        favorite.image = NSImage(systemSymbolName: thread.pinned ? "star.fill" : "star", accessibilityDescription: nil)
        favorite.imagePosition = .imageOnly
        favorite.contentTintColor = thread.pinned ? EmailTheme.accent : EmailTheme.muted.withAlphaComponent(0.55)
        favorite.toolTip = thread.pinned ? "Remove from favorites (F)" : "Add to favorites (F)"
        favorite.setAccessibilityLabel(thread.pinned ? "Remove from favorites" : "Add to favorites")
        sender.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        time.setContentCompressionResistancePriority(.required, for: .horizontal)
        for view in [sender, subject, preview, time, favorite] { view.translatesAutoresizingMaskIntoConstraints = false; cell.addSubview(view) }
        NSLayoutConstraint.activate([
            check.leadingAnchor.constraint(equalTo: cell.leadingAnchor, constant: 8), check.widthAnchor.constraint(equalToConstant: 16), check.centerYAnchor.constraint(equalTo: sender.centerYAnchor), unreadDot.leadingAnchor.constraint(equalTo: cell.leadingAnchor, constant: 29), unreadDot.centerYAnchor.constraint(equalTo: sender.centerYAnchor), unreadDot.widthAnchor.constraint(equalToConstant: 7), unreadDot.heightAnchor.constraint(equalToConstant: 7),
            sender.leadingAnchor.constraint(equalTo: cell.leadingAnchor, constant: 42), sender.topAnchor.constraint(equalTo: cell.topAnchor, constant: 6),
            time.trailingAnchor.constraint(equalTo: cell.trailingAnchor, constant: -18), time.centerYAnchor.constraint(equalTo: sender.centerYAnchor),
            favorite.trailingAnchor.constraint(equalTo: time.leadingAnchor, constant: -6), favorite.centerYAnchor.constraint(equalTo: sender.centerYAnchor), favorite.widthAnchor.constraint(equalToConstant: 20), favorite.heightAnchor.constraint(equalToConstant: 20),
            sender.trailingAnchor.constraint(equalTo: favorite.leadingAnchor, constant: -6),
            subject.leadingAnchor.constraint(equalTo: sender.leadingAnchor), subject.trailingAnchor.constraint(equalTo: time.trailingAnchor), subject.topAnchor.constraint(equalTo: sender.bottomAnchor, constant: 2),
            preview.leadingAnchor.constraint(equalTo: sender.leadingAnchor), preview.trailingAnchor.constraint(equalTo: subject.trailingAnchor), preview.topAnchor.constraint(equalTo: subject.bottomAnchor, constant: 2)
        ])
        return cell
    }

    @objc private func toggleFavoriteFromRow(_ sender: NSButton) {
        guard folder == .inbox, threads.indices.contains(sender.tag) else { return }
        select(sender.tag)
        togglePinSelection()
    }

    func tableViewSelectionDidChange(_ notification: Notification) {
        guard !suppressSelectionChanges else { return }
        if readerMode { setReaderMode(false) }
        if folder == .agentRequests {
            let visible = agentRequests
            selection = table.selectedRow
            agentRequestReader.show(visible.indices.contains(table.selectedRow) ? visible[table.selectedRow] : nil)
            return
        }
        if folder == .outbox {
            guard table.selectedRow >= 0, table.selectedRow < filteredOutbox.count else { return }
            showOutboxItem(filteredOutbox[table.selectedRow])
            return
        }
        if folder == .drafts {
            guard filteredDrafts.indices.contains(table.selectedRow) else { return }
            selection = table.selectedRow
            showDraftPreview(filteredDrafts[selection])
            return
        }
        guard table.selectedRow >= 0, table.selectedRow < threads.count else { return }
        selection = table.selectedRow
        if threads[selection].id == nil || threads[selection].id != displayedThreadID { show(threads[selection]) }
    }

    @objc private func editSelectedDraft() {
        guard folder == .drafts, filteredDrafts.indices.contains(table.selectedRow), let core else { return }
        showComposePane(core: core, draft: filteredDrafts[table.selectedRow])
    }

    private func discardSelectedDraft() {
        guard folder == .drafts, !draftDiscardBusy, composePane == nil, let core else { return }
        let row = table.selectedRow
        guard filteredDrafts.indices.contains(row) else { return }
        let draft = filteredDrafts[row]
        draftDiscardBusy = true
        Task { @MainActor in
            defer { draftDiscardBusy = false; updateTriageBar() }
            do {
                _ = try await core.callAsync(["op": "discard_draft", "message_id": draft.id])
                draftDiscarded(id: draft.id, subject: draft.subject, selectRow: row)
            } catch {
                syncStatus.stringValue = "Draft kept · could not discard"
                syncStatus.toolTip = error.localizedDescription
            }
        }
    }

    /// Shared by the list and the composer: remember the undo, then keep the
    /// selection on the draft that took the discarded one's place.
    func draftDiscarded(id: String, subject: String, selectRow row: Int?) {
        lastDiscardedDraft = (id, subject)
        drafts.removeAll { $0.id == id }
        syncStatus.stringValue = folder == .drafts ? "Draft discarded · Z to undo" : "Draft discarded · Undo from Drafts (G D, then Z)"
        syncStatus.toolTip = subject.isEmpty ? nil : subject
        guard folder == .drafts else { return }
        reloadDrafts(selectingID: nil, fallbackRow: row ?? max(table.selectedRow, 0))
    }

    private func undoDiscardDraft() {
        guard folder == .drafts, !draftDiscardBusy, let undo = lastDiscardedDraft, let core else { return }
        draftDiscardBusy = true
        updateTriageBar()
        Task { @MainActor in
            defer { draftDiscardBusy = false; updateTriageBar() }
            do {
                _ = try await core.callAsync(["op": "restore_discarded_draft", "message_id": undo.id])
                lastDiscardedDraft = nil
                syncStatus.stringValue = "Draft restored"
                syncStatus.toolTip = nil
                reloadDrafts(selectingID: undo.id, fallbackRow: 0)
            } catch {
                lastDiscardedDraft = nil
                syncStatus.stringValue = "Could not restore the draft"
                syncStatus.toolTip = error.localizedDescription
            }
        }
    }

    private func reloadDrafts(selectingID: String?, fallbackRow: Int) {
        guard let core else { return }
        if !search.stringValue.isEmpty, selectingID != nil { search.stringValue = "" }
        inboxLoadGeneration += 1
        let generation = inboxLoadGeneration
        Task { @MainActor in
            let loaded = await Task.detached(priority: .userInitiated) { core.drafts() }.value
            guard folder == .drafts, generation == inboxLoadGeneration else { return }
            drafts = loaded
            table.reloadData()
            updateEmptyInbox()
            let visible = filteredDrafts
            let target = selectingID.flatMap { id in visible.firstIndex { $0.id == id } } ?? min(fallbackRow, visible.count - 1)
            if visible.indices.contains(target) {
                selection = target
                table.selectRowIndexes(IndexSet(integer: target), byExtendingSelection: false)
                table.scrollRowToVisible(target)
                showDraftPreview(visible[target])
            } else {
                selection = -1
                table.deselectAll(nil)
                clearReader()
            }
            updateTriageBar()
            makeFirstResponder(table)
        }
    }

    private func showDraftPreview(_ draft: MailDraft) {
        clearReader()
        subject.stringValue = draft.subject.isEmpty ? "No subject" : draft.subject
        sender.stringValue = ComposerAddressRules.draftRecipientSummary(to: draft.to, cc: draft.cc, bcc: draft.bcc)
        subject.isHidden = false
        sender.isHidden = false
        emptyState.isHidden = true
        readerBody.isHidden = false
        bodyScroll.isHidden = false
        bodyText.string = draft.bodyPlain
        bodyScroll.contentView.scroll(to: .zero)
        bodyScroll.reflectScrolledClipView(bodyScroll.contentView)
        showHTML("<div class=\"plain-text\">\(Self.escapeHTML(draft.bodyPlain))</div>")
        updateReaderShortcuts()
    }

    private func clearReader() {
        setReaderMode(false)
        awaitingReaderTop = false
        displayedThreadID = nil
        currentWebNavigation = nil
        webView?.stopLoading()
        recipientSummaryMessage = nil
        recipientPopover?.close()
        currentMessageID = nil
        unsubscribeURL = nil
        unsubscribeButton.isHidden = true
        currentAttachments = []
        currentHTML = nil
        readerMessageCount = 0
        attachmentsButton.alphaValue = 0
        attachmentsButton.isEnabled = false
        attachmentsButton.toolTip = nil
        remoteImagesButton.isHidden = true
        setRemoteImagesNoticeVisible(false)
        dismissComposePane()
        messageLoadGeneration += 1
        subject.stringValue = ""
        sender.stringValue = ""
        updateContactPaneVisibility()
        account.stringValue = ""
        bodyText.string = ""
        subject.isHidden = true
        sender.isHidden = true
        account.isHidden = true
        readerActions.isHidden = true
        readerBody.isHidden = true
        emptyState.isHidden = false
        let searching = !search.stringValue.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        emptyTitle.stringValue = folder == .drafts ? "No draft selected" : (folder == .outbox ? "Nothing pending" : (searching ? "No results" : (inboxScope == .archived ? "No archived mail cached yet" : (inboxScope == .pinned ? "No favorites yet" : (knownAccounts.isEmpty ? "Welcome to Email" : "You're all caught up")))))
        emptyDetail.stringValue = folder == .drafts ? "Select a draft to continue writing, or press C to start a message." : (folder == .outbox ? "Scheduled messages and the undo window appear here." : (searching ? "Try another search or clear the field." : (inboxScope == .pinned ? "Press F on an email to save it here for later." : (knownAccounts.isEmpty ? (core?.buildFeatures().microsoftAccounts == true ? "Add your Gmail, iCloud, Outlook or other email account to get started." : "Add your Gmail, iCloud or other email account to get started.") : "New messages will appear after the next sync."))))
        if folder == .inbox && inboxScope == .sent && !searching {
            emptyTitle.stringValue = "No sent mail cached yet"
            emptyDetail.stringValue = "Messages sent from Email appear here after delivery. Only locally available sent mail is shown."
        }
        emptyAction.isHidden = folder != .inbox || !knownAccounts.isEmpty || searching
        bodyScroll.isHidden = true
        webView?.isHidden = true
        attachmentCard.isHidden = true
    }

    private func show(_ thread: MailThread) {
        setReaderMode(false)
        awaitingReaderTop = false
        let previousDisplayedThreadID = displayedThreadID
        displayedThreadID = thread.id
        let activityMessageID = pendingActivitySelection?.threadID == thread.id ? pendingActivitySelection?.messageID : (previousDisplayedThreadID == thread.id ? activityReaderMessageID : nil)
        if activityMessageID != nil { pendingActivitySelection = nil }
        activityReaderMessageID = activityMessageID
        readerMessageCount = 0
        messageLoadGeneration += 1
        currentWebNavigation = nil
        webView?.stopLoading()
        recipientSummaryMessage = nil
        recipientPopover?.close()
        currentMessageID = nil
        unsubscribeURL = nil
        unsubscribeButton.isHidden = true
        currentAttachments = []
        currentHTML = nil
        allowRemoteImagesForMessage = RemoteImagePreference.isEnabled(.standard)
        attachmentsButton.alphaValue = 0
        attachmentsButton.isEnabled = false
        attachmentsButton.toolTip = nil
        remoteImagesButton.isHidden = true
        setRemoteImagesNoticeVisible(false)
        attachmentCard.isHidden = true
        emptyState.isHidden = true
        subject.isHidden = false
        sender.isHidden = false
        account.isHidden = false
        readerActions.isHidden = false
        archiveButton.isHidden = false
        replyButton.isHidden = false
        replyAllButton.isHidden = false
        contactButton.isHidden = false
        labelsButton.isHidden = false
        pinButton.isHidden = false
        pinButton.image = NSImage(systemSymbolName: thread.pinned ? "star.fill" : "star", accessibilityDescription: nil)
        pinButton.contentTintColor = thread.pinned ? EmailTheme.accent : EmailTheme.muted
        pinButton.toolTip = thread.pinned ? "Remove from favorites (F)" : "Add to favorites (F)"
        snoozeButton.isHidden = false
        cancelSendButton.isHidden = lastQueuedMessageID == nil
        snoozeButton.toolTip = thread.snoozedUntil == nil ? "Snooze (S)" : "Unsnooze (S)"
        snoozeButton.setAccessibilityLabel(thread.snoozedUntil == nil ? "Snooze" : "Unsnooze")
        readerBody.isHidden = false
        recipientHeaderScript = nil
        let generation = messageLoadGeneration
        let subjectText = thread.subject.trimmingCharacters(in: .whitespacesAndNewlines)
        subject.stringValue = ["", "Re:", "Fwd:"].contains(subjectText) ? "Conversation with \(displayName(thread.sender))" : subjectText
        sender.stringValue = thread.sender
        recipientSummaryMessage = nil
        recipientPopover?.close()
        account.stringValue = "\(thread.time)"
        refreshContactPane(for: thread)
        if let id = thread.id, let core {
            bodyText.string = thread.preview.isEmpty ? "Loading message…" : thread.preview
            bodyScroll.contentView.scroll(to: .zero)
            bodyScroll.reflectScrolledClipView(bodyScroll.contentView)
            bodyScroll.isHidden = false
            webView?.isHidden = true
            Task { @MainActor in
                do {
                    guard let messages = try await core.callAsync(["op": "list_thread_messages", "thread_id": id]) as? [[String: Any]],
                          let latest = messages.last,
                          let messageID = latest["id"] as? String else {
                        if generation == messageLoadGeneration { bodyText.string = "This message is no longer in the local cache." }
                        return
                    }
                    self.updateRecipientSummary(latest, time: thread.time)
                    var cards: [String] = []
                    var attachmentOnly = false
                    for (index, message) in messages.enumerated() {
                        guard generation == messageLoadGeneration else { return }
                        guard let itemID = message["id"] as? String else { continue }
                        let rendered: [String: Any]?
                        if let cached = renderedBodyCache[itemID] {
                            rendered = cached
                        } else {
                            rendered = try? await core.callAsync(["op": "render_message", "message_id": itemID]) as? [String: Any]
                            if let rendered {
                                renderedBodyCache[itemID] = rendered
                                renderedBodyOrder.append(itemID)
                                while renderedBodyOrder.count > 12 {
                                    renderedBodyCache.removeValue(forKey: renderedBodyOrder.removeFirst())
                                }
                            }
                        }
                        let plain = rendered?["body_plain"] as? String ?? message["body_plain"] as? String ?? ""
                        let content: String
                        if let html = rendered?["body_html_safe"] as? String, !html.isEmpty {
                            content = html
                        } else if !plain.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                            content = "<div class=\"plain-text\">\(Self.escapeHTML(plain))</div>"
                        } else if (message["has_attachments"] as? Bool) == true {
                            attachmentOnly = messages.count == 1
                            content = "<section class=\"attachment-only\"><h2>Attachment-only message</h2><p>The attached file contains this email's content.</p></section>"
                        } else {
                            content = "<section class=\"attachment-only\"><h2>No message text</h2><p>This email does not contain a readable body.</p></section>"
                        }
                        cards.append(Self.messageCard(content, message: message, index: index, count: messages.count))
                    }
                    guard generation == messageLoadGeneration else { return }
                    if messages.contains(where: { $0["recipient_headers"] is NSNull || $0["recipient_headers"] == nil }) {
                        Task { @MainActor [weak self] in
                            try? await Task.sleep(nanoseconds: 250_000_000)
                            guard let self, generation == self.messageLoadGeneration else { return }
                            guard let updated = try? await core.callAsync(["op": "refresh_recipient_headers", "thread_id": id]) as? [[String: Any]],
                                  generation == self.messageLoadGeneration, let newest = updated.last else { return }
                            self.updateRecipientSummary(newest, time: thread.time)
                            // Replace only header nodes; preserve scroll, history expansion and body layout.
                            let data = updated.map { ["id": $0["id"] as? String ?? "", "html": Self.recipientDetails($0)] }
                            if let encoded = try? JSONSerialization.data(withJSONObject: data), let json = String(data: encoded, encoding: .utf8) {
                                self.recipientHeaderScript = "for (const h of \(json)) { const node = Array.from(document.querySelectorAll('[data-recipient-id]')).find(n => n.dataset.recipientId === h.id); if (node) node.innerHTML = h.html; }"
                                self.applyRecipientHeaders()
                            }
                        }
                    }
                    currentMessageID = activityMessageID.flatMap { target in messages.contains(where: { $0["id"] as? String == target }) ? target : nil } ?? messageID
                    if let focused = messages.first(where: { $0["id"] as? String == currentMessageID }) { self.updateRecipientSummary(focused, time: thread.time) }
                    readerMessageCount = messages.count
                    if let header = latest["unsubscribe_header"] as? String {
                        unsubscribeURL = Self.unsubscribeURL(from: header)
                        unsubscribeButton.isHidden = unsubscribeURL == nil
                    }
                    if attachmentOnly { showAttachmentOnlyLoading() }
                    else { showHTML(cards.joined()) }
                    if messages.contains(where: { ($0["body_html"] as? String)?.localizedCaseInsensitiveContains("cid:") == true }) {
                        Task { @MainActor [weak self] in
                            try? await Task.sleep(nanoseconds: 250_000_000)
                            guard let self, generation == self.messageLoadGeneration else { return }
                            var resolvedCards = cards
                            for (index, message) in messages.enumerated() {
                                guard generation == self.messageLoadGeneration else { return }
                                guard index < resolvedCards.count,
                                      let itemID = message["id"] as? String,
                                      let html = message["body_html"] as? String,
                                      html.localizedCaseInsensitiveContains("cid:") else { continue }
                                if let cached = self.renderedBodyCache[itemID]?["body_html_safe"] as? String,
                                   !cached.localizedCaseInsensitiveContains("cid:") { continue }
                                let rendered = try? await core.callAsync(["op": "render_message", "message_id": itemID, "include_inline_images": true]) as? [String: Any]
                                guard let content = rendered?["body_html_safe"] as? String else { continue }
                                if self.renderedBodyCache[itemID] != nil { self.renderedBodyCache[itemID] = rendered }
                                resolvedCards[index] = Self.messageCard(content, message: message, index: index, count: messages.count)
                            }
                            guard generation == self.messageLoadGeneration else { return }
                            self.currentHTML = resolvedCards.joined()
                            self.pendingInlineImageSources = Self.inlineImageSources(before: cards.joined(), after: resolvedCards.joined())
                            self.applyPendingInlineImages()
                        }
                    }
                    do {
                        var files: [[String: Any]] = []
                        // Waiting briefly avoids an attachment request for every message
                        // passed while navigating quickly with J and K.
                        try? await Task.sleep(nanoseconds: 250_000_000)
                        guard generation == messageLoadGeneration else { return }
                        for message in messages {
                            guard generation == messageLoadGeneration else { return }
                            guard let itemID = message["id"] as? String else { continue }
                            if (message["has_attachments"] as? Bool) == false { continue }
                            let found = try await core.callAsync(["op": "list_attachments", "message_id": itemID]) as? [[String: Any]] ?? []
                            files += found.map { var item = $0; item["message_id"] = itemID; return item }
                        }
                        guard generation == messageLoadGeneration else { return }
                        currentAttachments = files
                        if attachmentOnly { showAttachmentOnlyFiles(files) }
                        if !files.isEmpty {
                            let firstName = files[0]["filename"] as? String ?? "Attachment"
                            attachmentsButton.toolTip = files.count == 1 ? "\(firstName) — \(Self.canPreviewAttachment(files[0]) ? "preview or save" : "save")" : "\(files.count) attachments"
                            attachmentsButton.alphaValue = 1
                            attachmentsButton.isEnabled = true
                        }
                    } catch {
                        guard generation == messageLoadGeneration else { return }
                        attachmentsButton.alphaValue = 0
                        attachmentsButton.isEnabled = false
                        if attachmentOnly {
                            attachmentName.stringValue = "Attachment unavailable"
                            attachmentMeta.stringValue = "Could not load file details. Try selecting this email again."
                            attachmentSave.isEnabled = false
                        }
                    }
                } catch {
                    if generation == messageLoadGeneration { bodyText.string = error.localizedDescription }
                }
            }
            return
        }
        if !thread.html.isEmpty { showHTML(thread.html) }
        else { bodyText.string = ""; bodyScroll.isHidden = false; webView?.isHidden = true }
    }

    private func showAttachmentOnlyLoading() {
        bodyScroll.isHidden = true
        webView?.isHidden = true
        attachmentName.stringValue = "Loading attachment…"
        attachmentMeta.stringValue = "This message has no body text."
        attachmentSave.title = "Save file…"
        attachmentSave.isEnabled = false
        attachmentCard.isHidden = false
    }

    private func showAttachmentOnlyFiles(_ files: [[String: Any]]) {
        guard let first = files.first else {
            attachmentName.stringValue = "Attachment unavailable"
            attachmentMeta.stringValue = "The provider did not return the attached file."
            attachmentSave.isEnabled = false
            return
        }
        let name = first["filename"] as? String ?? "Attachment"
        let size = (first["size"] as? NSNumber)?.int64Value ?? 0
        let kind = URL(fileURLWithPath: name).pathExtension.uppercased()
        let sizeLabel = size > 0 ? ByteCountFormatter.string(fromByteCount: size, countStyle: .file) : "Unknown size"
        attachmentName.stringValue = files.count == 1 ? name : "\(files.count) attachments"
        attachmentName.toolTip = files.count == 1 ? name : nil
        attachmentMeta.stringValue = "\(kind.isEmpty ? "File" : kind) · \(sizeLabel)"
        attachmentSave.title = files.count == 1 ? "Save file…" : "Show files"
        attachmentSave.isEnabled = true
    }

    @objc private func saveAttachmentOnlyClicked() {
        if currentAttachments.count == 1, let file = currentAttachments.first {
            promptSaveAttachment(file)
        } else if !currentAttachments.isEmpty {
            showAttachments(attachmentSave)
        }
    }

    @objc private func showAttachments(_ sender: NSButton) {
        guard !currentAttachments.isEmpty else {
            if let message = attachmentsButton.toolTip { showNotice(message) }
            return
        }
        let menu = NSMenu(title: "Attachments")
        for (index, file) in currentAttachments.enumerated() {
            let name = file["filename"] as? String ?? "Attachment"
            let size = (file["size"] as? NSNumber)?.int64Value ?? 0
            let detail = size > 0 ? "  ·  \(ByteCountFormatter.string(fromByteCount: size, countStyle: .file))" : ""
            if Self.canPreviewAttachment(file) {
                let preview = NSMenuItem(title: "Preview \(name)\(detail)", action: #selector(previewAttachment(_:)), keyEquivalent: "")
                preview.target = self
                preview.representedObject = file
                menu.addItem(preview)
            }
            let save = NSMenuItem(title: "Save \(name)…", action: #selector(saveAttachment(_:)), keyEquivalent: "")
            save.target = self
            save.representedObject = file
            menu.addItem(save)
            if index < currentAttachments.count - 1 { menu.addItem(.separator()) }
        }
        menu.popUp(positioning: nil, at: NSPoint(x: 0, y: sender.bounds.height), in: sender)
    }

    private static func canPreviewAttachment(_ file: [String: Any]) -> Bool {
        let name = (file["filename"] as? String ?? "").lowercased()
        if [".zip", ".gz", ".gzip", ".7z", ".rar"].contains(where: { name.hasSuffix($0) }) { return false }
        let type = (file["mime_type"] as? String ?? file["content_type"] as? String ?? "").lowercased()
        return type.hasPrefix("image/") || type == "application/pdf" || type.hasPrefix("text/")
    }

    @objc private func saveAttachment(_ sender: NSMenuItem) {
        guard let file = sender.representedObject as? [String: Any] else { return }
        promptSaveAttachment(file)
    }

    private func promptSaveAttachment(_ file: [String: Any]) {
        guard let partID = file["part_id"] as? String,
              let messageID = file["message_id"] as? String ?? currentMessageID,
              let core else { return }
        let name = URL(fileURLWithPath: file["filename"] as? String ?? "Attachment").lastPathComponent
        let panel = NSSavePanel()
        panel.nameFieldStringValue = name
        panel.beginSheetModal(for: self) { [weak self] response in
            guard response == .OK, let destination = panel.url else { return }
            Task { @MainActor [weak self] in
                do {
                    let result = try await core.callAsync(["op": "fetch_attachment", "message_id": messageID, "part_id": partID]) as? [String: Any]
                    guard let encoded = result?["data_base64"] as? String,
                          let data = Data(base64Encoded: encoded) else {
                        throw MailServiceError(message: "The attachment could not be decoded.")
                    }
                    try data.write(to: destination, options: .atomic)
                    self?.showNotice("Saved \(destination.lastPathComponent)")
                } catch {
                    self?.showNotice("Attachment could not be saved: \(error.localizedDescription)")
                }
            }
        }
    }

    @objc private func previewAttachment(_ sender: NSMenuItem) {
        guard let file = sender.representedObject as? [String: Any],
              let partID = file["part_id"] as? String,
              let messageID = file["message_id"] as? String ?? currentMessageID,
              let core else { return }
        let contentType = (file["mime_type"] as? String ?? file["content_type"] as? String ?? "").lowercased()
        guard Self.canPreviewAttachment(file) else {
            showNotice("Preview is available for images, PDF, and text. Use Save for this file.")
            return
        }
        if let size = (file["size"] as? NSNumber)?.int64Value, size > 20 * 1024 * 1024 {
            showNotice("This attachment is too large to preview. Use Save instead.")
            return
        }
        let selectedMessageID = currentMessageID
        Task { @MainActor in
            do {
                let result = try await core.callAsync(["op": "fetch_attachment", "message_id": messageID, "part_id": partID]) as? [String: Any]
                guard selectedMessageID == currentMessageID,
                      let encoded = result?["data_base64"] as? String,
                      let data = Data(base64Encoded: encoded), data.count <= 20 * 1024 * 1024 else { return }
                let alert = NSAlert()
                alert.messageText = file["filename"] as? String ?? "Attachment"
                alert.addButton(withTitle: "Done")
                let frame = NSRect(x: 0, y: 0, width: 620, height: 440)
                if contentType == "application/pdf", let document = PDFDocument(data: data) {
                    let view = PDFView(frame: frame)
                    view.document = document
                    view.autoScales = true
                    alert.accessoryView = view
                } else if contentType.hasPrefix("image/"), let image = NSImage(data: data) {
                    let view = NSImageView(frame: frame)
                    view.image = image
                    view.imageScaling = .scaleProportionallyUpOrDown
                    alert.accessoryView = view
                } else if contentType.hasPrefix("text/"), let text = String(data: data, encoding: .utf8) {
                    let scroll = NSScrollView(frame: frame)
                    let view = NSTextView(frame: frame)
                    view.isEditable = false
                    view.string = String(text.prefix(200_000))
                    scroll.documentView = view
                    scroll.hasVerticalScroller = true
                    alert.accessoryView = scroll
                } else {
                    showNotice("This attachment could not be previewed. Use Save instead.")
                    return
                }
                alert.runModal()
            } catch { showNotice(error.localizedDescription) }
        }
    }

    private static func escapeHTML(_ value: String) -> String {
        value.replacingOccurrences(of: "&", with: "&amp;")
            .replacingOccurrences(of: "<", with: "&lt;")
            .replacingOccurrences(of: ">", with: "&gt;")
            .replacingOccurrences(of: "\"", with: "&quot;")
            .replacingOccurrences(of: "'", with: "&#39;")
    }

    private func updateRecipientSummary(_ message: [String: Any], time: String) {
        let to = (message["to"] as? [String] ?? []).joined(separator: ", ")
        let headers = message["recipient_headers"] as? [String: Any]
        let cc = (headers?["cc"] as? [String] ?? []).joined(separator: ", ")
        recipientSummaryMessage = message
        account.stringValue = "⌄  " + [to.isEmpty ? "To not specified" : "To \(to)", cc.isEmpty ? "" : "Cc \(cc)", time].filter { !$0.isEmpty }.joined(separator: "  ·  ")
        account.toolTip = "Click for full recipients, Cc, Reply-To, and date."
        account.setAccessibilityValue(account.stringValue)
    }

    private func showRecipientPopover() {
        guard let message = recipientSummaryMessage else { return }
        if recipientPopover?.isShown == true { recipientPopover?.close(); return }
        let headers = message["recipient_headers"] as? [String: Any]
        var rows: [(String, String)] = [("From", message["sender"] as? String ?? ""),
            ("To", (message["to"] as? [String] ?? []).joined(separator: ", "))]
        for (label, key) in [("Cc", "cc"), ("Bcc", "bcc"), ("Reply-To", "reply_to")] {
            let value = (headers?[key] as? [String] ?? []).joined(separator: ", ")
            if !value.isEmpty { rows.append((label, value)) }
        }
        let date = Date(timeIntervalSince1970: ((message["received_at"] as? NSNumber)?.doubleValue ?? 0) / 1000)
        rows.append(("Date", DateFormatter.localizedString(from: date, dateStyle: .full, timeStyle: .long)))
        let content = NSStackView()
        content.orientation = .vertical
        content.alignment = .leading
        content.spacing = 10
        content.edgeInsets = NSEdgeInsets(top: 16, left: 18, bottom: 16, right: 18)
        content.translatesAutoresizingMaskIntoConstraints = false
        for (label, value) in rows {
            let row = NSStackView()
            row.orientation = .horizontal
            row.alignment = .firstBaseline
            row.spacing = 12
            let name = NSTextField(labelWithString: label)
            name.font = .systemFont(ofSize: 11)
            name.textColor = .secondaryLabelColor
            name.widthAnchor.constraint(equalToConstant: 50).isActive = true
            let text = NSTextField(wrappingLabelWithString: value.isEmpty ? "Not specified" : value)
            text.font = .systemFont(ofSize: 12)
            text.isSelectable = true
            text.widthAnchor.constraint(equalToConstant: 330).isActive = true
            row.addArrangedSubview(name)
            row.addArrangedSubview(text)
            content.addArrangedSubview(row)
        }
        let size = NSSize(width: 428, height: content.fittingSize.height + 32)
        let container = NSView(frame: NSRect(origin: .zero, size: size))
        content.edgeInsets = NSEdgeInsetsZero
        container.addSubview(content)
        NSLayoutConstraint.activate([
            content.leadingAnchor.constraint(equalTo: container.leadingAnchor, constant: 18),
            content.trailingAnchor.constraint(equalTo: container.trailingAnchor, constant: -18),
            content.topAnchor.constraint(equalTo: container.topAnchor, constant: 16),
            content.bottomAnchor.constraint(equalTo: container.bottomAnchor, constant: -16),
        ])
        let controller = NSViewController()
        controller.view = container
        let popover = NSPopover()
        popover.behavior = .transient
        popover.contentViewController = controller
        popover.contentSize = size
        recipientPopover = popover
        popover.show(relativeTo: account.bounds, of: account, preferredEdge: .maxY)
    }

    private static func recipientDetails(_ message: [String: Any]) -> String {
        let headers = message["recipient_headers"] as? [String: Any]
        var rows: [(String, String)] = [("From", message["sender"] as? String ?? ""), ("To", (message["to"] as? [String] ?? []).joined(separator: ", "))]
        for (label, key) in [("Cc", "cc"), ("Bcc", "bcc"), ("Reply-To", "reply_to")] {
            let value = (headers?[key] as? [String] ?? []).joined(separator: ", ")
            if !value.isEmpty { rows.append((label, value)) }
        }
        let date = Date(timeIntervalSince1970: ((message["received_at"] as? NSNumber)?.doubleValue ?? 0) / 1000)
        rows.append(("Date", DateFormatter.localizedString(from: date, dateStyle: .full, timeStyle: .long)))
        return rows.map { "<div class=\"recipient-row\"><span>\(escapeHTML($0.0))</span><div>\(escapeHTML($0.1.isEmpty ? "Not specified" : $0.1))</div></div>" }.joined()
    }

    private static func messageCard(_ content: String, message: [String: Any], index: Int, count: Int) -> String {
        let itemID = escapeHTML(message["id"] as? String ?? "")
        let name = escapeHTML(message["sender"] as? String ?? "Unknown sender")
        let date = Date(timeIntervalSince1970: ((message["received_at"] as? NSNumber)?.doubleValue ?? 0) / 1000)
        let when = escapeHTML(DateFormatter.localizedString(from: date, dateStyle: .medium, timeStyle: .short))
        let details = "<details class=\"recipient-details\"><summary>Message details <span>⌄</span></summary><div data-recipient-id=\"\(itemID)\">\(recipientDetails(message))</div></details>"
        guard count > 1 else { return content }
        let messageDetails = index == count - 1 ? "" : details
        let expanded = index == count - 1 ? " open" : ""
        return "<details\(expanded) data-message-id=\"\(itemID)\" class=\"message-card\"><summary><span class=\"message-sender\">\(name)</span><span class=\"message-date\">\(when)</span></summary><div class=\"message-content\">\(messageDetails)\(content)</div></details>"
    }

    private static func inlineImageSources(before: String, after: String) -> [String: String] {
        // The sanitizer preserves image order while resolving cid: URLs to data: URLs.
        let pattern = #"<img\b[^>]*?\bsrc\s*=\s*(["'])(.*?)\1"#
        guard let regex = try? NSRegularExpression(pattern: pattern, options: [.caseInsensitive, .dotMatchesLineSeparators]) else { return [:] }
        func sources(_ html: String) -> [String] {
            let range = NSRange(html.startIndex..<html.endIndex, in: html)
            return regex.matches(in: html, range: range).compactMap { match in
                guard let sourceRange = Range(match.range(at: 2), in: html) else { return nil }
                return String(html[sourceRange])
            }
        }
        var mapping: [String: String] = [:]
        for (original, resolved) in zip(sources(before), sources(after)) {
            if original.lowercased().hasPrefix("cid:"), resolved.lowercased().hasPrefix("data:") {
                mapping[original] = resolved
            }
        }
        return mapping
    }

    private func applyPendingInlineImages() {
        guard currentWebDidCommit, !pendingInlineImageSources.isEmpty, let webView,
              let data = try? JSONSerialization.data(withJSONObject: pendingInlineImageSources),
              let json = String(data: data, encoding: .utf8) else { return }
        let script = "for (const image of document.images) { const replacement = (\(json))[image.getAttribute('src')]; if (replacement) image.src = replacement; }"
        webView.evaluateJavaScript(script) { [weak self] _, error in
            guard error == nil else { return }
            self?.pendingInlineImageSources = [:]
        }
    }

    @objc private func loadImagesClicked(_ sender: NSButton) {
        let menu = NSMenu()
        let once = menu.addItem(withTitle: allowRemoteImagesForMessage ? "Block images in this email" : "Load images in this email", action: #selector(toggleImagesForMessage(_:)), keyEquivalent: "")
        once.target = self
        let globallyEnabled = RemoteImagePreference.isEnabled(.standard)
        let always = menu.addItem(withTitle: globallyEnabled ? "Block remote images by default" : "Load remote images by default", action: #selector(toggleImagesGloballyClicked(_:)), keyEquivalent: "")
        always.target = self
        menu.popUp(positioning: nil, at: NSPoint(x: 0, y: sender.bounds.height), in: sender)
    }

    @objc private func toggleImagesForMessage(_ sender: NSMenuItem) {
        guard let html = currentHTML else { return }
        allowRemoteImagesForMessage.toggle()
        showHTML(html)
    }

    @objc private func loadImagesForMessageClicked(_ sender: NSButton) {
        guard let html = currentHTML, !allowRemoteImagesForMessage else { return }
        // With Full Keyboard Access the button can hold focus; it is about to
        // hide, so give focus back to an open reply, else the message list.
        let returnFocus = firstResponder === remoteImagesNoticeButton
        allowRemoteImagesForMessage = true
        showHTML(html)
        if returnFocus {
            if let composePane { composePane.focusBody(in: self) } else { makeFirstResponder(table) }
        }
    }

    private func buildRemoteImagesNotice() {
        remoteImagesNotice.wantsLayer = true
        remoteImagesNotice.layer?.cornerRadius = 8
        remoteImagesNotice.layer?.borderWidth = 1
        remoteImagesNotice.translatesAutoresizingMaskIntoConstraints = false
        remoteImagesNotice.isHidden = true
        remoteImagesNotice.setAccessibilityElement(true)
        remoteImagesNotice.setAccessibilityRole(.group)
        remoteImagesNotice.setAccessibilityLabel("Remote images blocked")
        remoteImagesNotice.toolTip = "Remote images can tell the sender when and where you opened this email."
        let icon = NSImageView(image: NSImage(systemSymbolName: "photo", accessibilityDescription: nil) ?? NSImage())
        icon.symbolConfiguration = NSImage.SymbolConfiguration(pointSize: 12, weight: .regular)
        icon.contentTintColor = EmailTheme.muted
        icon.setContentHuggingPriority(.required, for: .horizontal)
        let text = NSTextField(labelWithString: "Remote images are blocked.")
        text.font = .systemFont(ofSize: 12)
        text.textColor = EmailTheme.muted
        text.lineBreakMode = .byTruncatingTail
        text.maximumNumberOfLines = 1
        text.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        text.setContentHuggingPriority(.required, for: .horizontal)
        remoteImagesNoticeButton.isBordered = false
        remoteImagesNoticeButton.font = .systemFont(ofSize: 12, weight: .semibold)
        remoteImagesNoticeButton.contentTintColor = EmailTheme.accent
        remoteImagesNoticeButton.setAccessibilityLabel("Load images in this email")
        remoteImagesNoticeButton.target = self
        remoteImagesNoticeButton.action = #selector(loadImagesForMessageClicked(_:))
        remoteImagesNoticeButton.setContentCompressionResistancePriority(.required, for: .horizontal)
        remoteImagesNoticeButton.setContentHuggingPriority(.required, for: .horizontal)
        let spacer = NSView()
        spacer.setContentHuggingPriority(.defaultLow, for: .horizontal)
        spacer.setContentCompressionResistancePriority(.fittingSizeCompression, for: .horizontal)
        let row = NSStackView(views: [icon, text, remoteImagesNoticeButton, spacer])
        row.orientation = .horizontal
        row.alignment = .centerY
        row.spacing = 6
        row.setCustomSpacing(8, after: icon)
        row.translatesAutoresizingMaskIntoConstraints = false
        remoteImagesNotice.addSubview(row)
        NSLayoutConstraint.activate([
            row.leadingAnchor.constraint(equalTo: remoteImagesNotice.leadingAnchor, constant: 10),
            row.trailingAnchor.constraint(equalTo: remoteImagesNotice.trailingAnchor, constant: -8),
            row.centerYAnchor.constraint(equalTo: remoteImagesNotice.centerYAnchor)
        ])
    }

    /// A new-message composer fills the reader; a reply leaves the message visible.
    private var composeCoversReader: Bool { composePane != nil && replyBodyBottomConstraint == nil }

    private func setRemoteImagesNoticeVisible(_ visible: Bool) {
        guard remoteImagesNotice.isHidden == visible else { return }
        remoteImagesNotice.isHidden = !visible
        // Deactivate before activating so both never hold at once.
        if visible { readerBodyBelowHeader?.isActive = false; readerBodyBelowNotice?.isActive = true }
        else { readerBodyBelowNotice?.isActive = false; readerBodyBelowHeader?.isActive = true }
    }

    @objc private func toggleImagesGloballyClicked(_ sender: NSMenuItem) {
        setLoadRemoteImagesGlobally(!RemoteImagePreference.isEnabled(.standard))
    }

    func setLoadRemoteImagesGlobally(_ enabled: Bool) {
        RemoteImagePreference.setEnabled(enabled, in: .standard)
        NSApp.mainMenu?.item(withTitle: "View")?.submenu?.item(withTitle: "Always Load Remote Images")?.state = enabled ? .on : .off
        allowRemoteImagesForMessage = enabled
        if let html = currentHTML { showHTML(html) }
    }

    private func showHTML(_ body: String) {
        // Only the service's sanitized HTML or a static fixture may enter the webview.
        currentHTML = body
        pendingInlineImageSources = [:]
        let hasRemoteImages = RemoteImagePreference.containsRemoteImages(body)
        remoteImagesButton.isHidden = !hasRemoteImages
        // Hidden under a covering composer, so it is not in the key view loop.
        setRemoteImagesNoticeVisible(hasRemoteImages && !allowRemoteImagesForMessage && !composeCoversReader)
        remoteImagesNoticeButton.toolTip = RemoteImagePreference.loadButtonTooltip(loadByDefault: RemoteImagePreference.isEnabled(.standard))
        remoteImagesButton.contentTintColor = allowRemoteImagesForMessage ? EmailTheme.accent : EmailTheme.muted
        remoteImagesButton.setAccessibilityLabel(allowRemoteImagesForMessage ? "Remote images allowed" : "Remote images blocked")
        remoteImagesButton.toolTip = RemoteImagePreference.controlTooltip(allowedHere: allowRemoteImagesForMessage, loadByDefault: RemoteImagePreference.isEnabled(.standard))
        let imageSources = allowRemoteImagesForMessage ? "data: https:" : "data:"
        let csp = "default-src 'none'; img-src \(imageSources); style-src 'unsafe-inline'; font-src 'none'; form-action 'none'; frame-src 'none'; referrer 'no-referrer'"
        let dark = EmailTheme.isDark(effectiveAppearance)
        let ink = dark ? "#ecebe8" : "#262626"
        var background = dark ? "#1b1c1d" : "#ffffff"
        let muted = dark ? "#a3a39f" : "#777875"
        let line = dark ? "#353637" : "#e5e4e1"
        let accent = dark ? "#d9d9df" : "#45464b"
        let plainCount = body.components(separatedBy: "class=\"plain-text\"").count - 1
        let plainLayout = plainCount > 0 && plainCount == max(1, readerMessageCount)
        if plainLayout {
            effectiveAppearance.performAsCurrentDrawingAppearance {
                if let color = EmailTheme.reader.usingColorSpace(.sRGB) {
                    background = String(format: "rgb(%d,%d,%d)", Int((color.redComponent * 255).rounded()), Int((color.greenComponent * 255).rounded()), Int((color.blueComponent * 255).rounded()))
                }
            }
        }
        let bodyLayout = plainLayout ? "margin:0;padding:18px 6px 32px;max-width:none" : "margin:0 auto;padding:0 14px;max-width:760px"
        let document = """
        <!doctype html><html><head><meta name="viewport" content="width=device-width,initial-scale=1"><meta name="referrer" content="no-referrer"><meta http-equiv="Content-Security-Policy" content="\(csp)"><style>html{overflow-x:hidden;background:\(background)}body{font:15px/1.62 -apple-system,BlinkMacSystemFont,sans-serif;color:\(ink);background:\(background);\(bodyLayout);overflow-wrap:anywhere}body *{box-sizing:border-box}table,td,div{max-width:100%!important}img{max-width:100%!important;height:auto!important}pre,code{white-space:pre-wrap!important}p{margin:0 0 1.2em}.plain-text{white-space:pre-wrap;max-width:72ch;font:inherit;text-align:left;tab-size:4}blockquote{border-left:2px solid \(line);margin:1.5em 0;padding:0 0 0 1em;color:\(muted)}a{color:\(accent)}.attachment-only{padding:16px 0;color:\(muted)}.attachment-only h2{color:\(ink);font-size:17px;font-weight:600;margin:0 0 6px}.attachment-only p{margin:0}.message-card{border:0;border-bottom:1px solid \(line);border-radius:0;margin:0;padding:18px 0 24px}.message-card:first-child{padding-top:4px}.message-card:last-child{border-bottom:0}.message-card summary{cursor:pointer;list-style:none;display:flex;gap:12px;align-items:baseline;justify-content:space-between;color:\(muted);font-size:12px}.message-card[open] summary{color:\(ink)}.message-sender{overflow:hidden;text-overflow:ellipsis;white-space:nowrap;font-weight:600}.message-date{flex:none;color:\(muted);font-weight:400;font-size:11px}.message-content{padding-top:24px}.recipient-details{font-size:12px;color:\(muted);margin:0 0 16px}.recipient-details>summary{display:block;font-size:11px;cursor:pointer;padding:4px 0;list-style:none}.recipient-row{display:grid;grid-template-columns:62px minmax(0,1fr);gap:10px;padding:4px 0}.recipient-row>div{color:\(ink);user-select:text;overflow-wrap:anywhere}.recipient-details[open]{padding-bottom:8px}</style></head><body>\(body)</body></html>
        """
        let webView = ensureWebView()
        // Sender HTML often specifies dark text over a transparent background.
        // Repair it at document end, before remote images finish and before the
        // page is revealed. Page-authored JavaScript remains disabled.
        let scripts = webView.configuration.userContentController
        scripts.removeAllUserScripts()
        if dark {
            scripts.addUserScript(WKUserScript(source: Self.darkMessageContrastScript,
                                             injectionTime: .atDocumentEnd,
                                             forMainFrameOnly: true))
        }
        // Contact-pane visibility can change the reading width for a new sender.
        // Lay it out now so the first paint uses the final width and page zoom.
        contentView?.layoutSubtreeIfNeeded()
        fitMessageToReader()
        // Keep the selected message's cached text visible until WebKit commits
        // its new document. Remote images can hold didFinish open indefinitely.
        bodyScroll.isHidden = false
        webView.isHidden = true
        webView.stopLoading()
        currentWebDidCommit = false
        currentWebNavigation = webView.loadHTMLString(document, baseURL: nil)
    }

    func windowDidResize(_ notification: Notification) {
        updateActivityLayout()
        updateContactPaneVisibility()
        fitMessageToReader()
    }

    func windowDidEndLiveResize(_ notification: Notification) {
        UserDefaults.standard.set(Double(frame.width), forKey: preferredWindowWidthKey)
    }

    @objc func resetWindowSize(_ sender: Any?) {
        setContentSize(NSSize(width: 1100, height: 760))
        center()
        UserDefaults.standard.set(Double(frame.width), forKey: preferredWindowWidthKey)
    }

    @objc func fillAvailableWidth(_ sender: Any?) {
        guard let screen = screen ?? NSScreen.main else { return }
        var frame = self.frame
        frame.origin.x = screen.visibleFrame.minX
        frame.size.width = screen.visibleFrame.width
        setFrame(frame, display: true)
        UserDefaults.standard.set(Double(self.frame.width), forKey: preferredWindowWidthKey)
    }

    func adjustWindowWidth(by amount: CGFloat) {
        guard let screen = screen ?? NSScreen.main else { return }
        var next = frame
        next.size.width = min(screen.visibleFrame.width, max(minSize.width, next.width + amount))
        next.origin.x = min(max(next.minX, screen.visibleFrame.minX), screen.visibleFrame.maxX - next.width)
        setFrame(next, display: true)
        UserDefaults.standard.set(Double(frame.width), forKey: preferredWindowWidthKey)
    }

    private func applyRecipientHeaders() {
        guard let script = recipientHeaderScript else { return }
        webView?.evaluateJavaScript(script, completionHandler: { _, _ in })
    }

    func webView(_ webView: WKWebView, didCommit navigation: WKNavigation!) {
        guard let currentWebNavigation, navigation === currentWebNavigation else { return }
        currentWebDidCommit = true
        bodyScroll.isHidden = true
        webView.isHidden = false
        applyRecipientHeaders()
        applyPendingInlineImages()
        applyActivityMessageFocus()
    }

    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        guard let currentWebNavigation, navigation === currentWebNavigation else { return }
        applyRecipientHeaders()
        applyPendingInlineImages()
        applyActivityMessageFocus()
        bodyScroll.isHidden = true
        webView.isHidden = false
        let generation = messageLoadGeneration
        let script = "Array.from(document.querySelectorAll('a[href]')).map(a => ({url:a.href, text:(a.textContent || a.getAttribute('aria-label') || a.title || '').trim(), context:(a.parentElement?.textContent || '').trim().slice(0,180)})).filter(a => /unsubscribe|opt[ -]?out|stop receiving/i.test(a.text + ' ' + a.url + ' ' + a.context)).map(a => a.url)"
        webView.evaluateJavaScript(script) { [weak self] result, _ in
            Task { @MainActor [weak self] in
                guard let self, generation == self.messageLoadGeneration else { return }
                let candidates = result as? [String] ?? []
                if self.unsubscribeURL == nil, let link = candidates.compactMap(URL.init(string:)).first(where: { ["https", "http", "mailto"].contains($0.scheme?.lowercased() ?? "") && ReaderLinkPolicy.externalURL($0) != nil }) {
                    self.unsubscribeURL = link
                }
                self.unsubscribeButton.isHidden = self.unsubscribeURL == nil
            }
        }
    }

    private func applyActivityMessageFocus() {
        guard let target = activityReaderMessageID, !target.isEmpty, let webView,
              let data = try? JSONSerialization.data(withJSONObject: [target]),
              let encoded = String(data: data, encoding: .utf8) else { return }
        webView.evaluateJavaScript("(() => { const id = \(encoded)[0]; const card = Array.from(document.querySelectorAll('details[data-message-id]')).find(el => el.dataset.messageId === id); if (card) { card.open = true; card.scrollIntoView({block:'start'}); } })()", completionHandler: nil)
    }

    private static let darkMessageContrastScript = #"""
    (() => {
      const rgb = value => {
        const match = value.match(/rgba?\(([^)]+)\)/);
        if (!match) return null;
        const parts = match[1].split(',').map(Number);
        return parts.length >= 3 ? [parts[0], parts[1], parts[2], parts[3] ?? 1] : null;
      };
      const luminance = color => {
        const channels = color.slice(0, 3).map(value => {
          const scaled = value / 255;
          return scaled <= 0.04045 ? scaled / 12.92 : ((scaled + 0.055) / 1.055) ** 2.4;
        });
        return channels[0] * 0.2126 + channels[1] * 0.7152 + channels[2] * 0.0722;
      };
      const contrast = (a, b) => {
        const first = luminance(a), second = luminance(b);
        return (Math.max(first, second) + 0.05) / (Math.min(first, second) + 0.05);
      };
      let repaired = 0;
      for (const element of document.querySelectorAll('body, body *')) {
        if (element.children.length && !Array.from(element.childNodes).some(node => node.nodeType === 3 && node.textContent.trim())) continue;
        if (!element.textContent.trim()) continue;
        const color = rgb(getComputedStyle(element).color);
        if (!color) continue;
        const ancestry = [];
        for (let parent = element; parent; parent = parent.parentElement) ancestry.push(parent);
        let background = [27, 28, 29];
        let imageBackground = false;
        for (const parent of ancestry.reverse()) {
          const style = getComputedStyle(parent);
          if (style.backgroundImage !== 'none') imageBackground = true;
          const layer = rgb(style.backgroundColor);
          if (layer && layer[3] > 0) {
            const alpha = layer[3];
            background = layer.slice(0, 3).map((channel, index) => channel * alpha + background[index] * (1 - alpha));
          }
        }
        if (imageBackground || contrast(color, background) >= 4.5) continue;
        const light = [236, 235, 232], dark = [38, 38, 38];
        element.style.setProperty('color', contrast(light, background) > contrast(dark, background) ? '#ecebe8' : '#262626', 'important');
        repaired++;
      }
      return repaired;
    })()
    """#

    private func fitMessageToReader() {
        guard let webView, readerBody.bounds.width > 0 else { return }
        // Many newsletters use a fixed layout wider than their visible text. Scale it when
        // the reading pane is narrower, while leaving ordinary mail at 100%.
        let available = max(1, readerBody.bounds.width - 12)
        let plainCount = (currentHTML ?? "").components(separatedBy: "class=\"plain-text\"").count - 1
        let plainLayout = plainCount > 0 && plainCount == max(1, readerMessageCount)
        let zoom = plainLayout ? 1 : min(1, max(0.55, available / 720))
        // pageZoom scales layout like CSS zoom. Reapplying magnification at the
        // end of a navigation changed the visible page size after its first paint.
        if abs(webView.pageZoom - zoom) > 0.005 { webView.pageZoom = zoom }
    }

    private func ensureWebView() -> WKWebView {
        if let webView { return webView }
        let config = WKWebViewConfiguration()
        config.websiteDataStore = .nonPersistent()
        config.defaultWebpagePreferences.allowsContentJavaScript = false
        let view = WKWebView(frame: .zero, configuration: config)
        view.navigationDelegate = self
        view.translatesAutoresizingMaskIntoConstraints = false
        readerBody.addSubview(view)
        NSLayoutConstraint.activate([
            view.leadingAnchor.constraint(equalTo: readerBody.leadingAnchor),
            view.trailingAnchor.constraint(equalTo: readerBody.trailingAnchor),
            view.topAnchor.constraint(equalTo: readerBody.topAnchor),
            view.bottomAnchor.constraint(equalTo: readerBody.bottomAnchor)
        ])
        webView = view
        return view
    }

    func webView(_ webView: WKWebView, decidePolicyFor navigationAction: WKNavigationAction) async -> WKNavigationActionPolicy {
        // Links with target=_blank arrive here too (targetFrame == nil). Cancelling
        // them means WebKit never asks a WKUIDelegate for a new web view.
        switch ReaderLinkPolicy.decide(url: navigationAction.request.url,
                                       linkActivated: navigationAction.navigationType == .linkActivated,
                                       programmaticLoad: navigationAction.navigationType == .other) {
        case .allow: return .allow
        case .cancel: return .cancel
        case .openExternally(let url):
            NSWorkspace.shared.open(url)
            return .cancel
        }
    }

    func controlTextDidChange(_ obj: Notification) {
        guard let field = obj.object as? NSSearchField else { return }
        if field == paletteSearch { refreshPalette(); return }
        guard field == search else { return }
        clearGoTo()
        if folder == .agentRequests { reloadAgentRequestList(); return }
        if folder == .drafts || folder == .outbox { table.reloadData(); updateEmptyInbox(); clearReader() }
        else {
            searchRefreshTask?.cancel()
            inboxLoadGeneration += 1 // invalidate an older query before the debounce elapses
            syncStatus.stringValue = "Searching downloaded mail…"
            searchRefreshTask = Task { @MainActor [weak self] in
                try? await Task.sleep(nanoseconds: 120_000_000)
                guard !Task.isCancelled, let self else { return }
                self.refreshInbox()
            }
        }
    }

    private func openWorkflow(calendar: Bool) {
        guard let core else { return }
        let threadID = threads.indices.contains(selection) ? threads[selection].id : nil
        if !calendar && (folder != .inbox || threadID == nil) { showNotice("Select a conversation to set a follow-up."); return }
        workflowPopover?.close()
        let editor = MailWorkflowPanel(core: core, calendar: calendar, threadID: threadID, messageID: currentMessageID, width: min(520, max(320, frame.width - 40))) { [weak self] in self?.refreshInbox() }
        let popover = NSPopover(); popover.behavior = .transient; popover.contentViewController = editor
        editor.dismiss = { [weak self, weak popover] in popover?.close(); self?.makeFirstResponder(self?.table) }
        popover.contentSize = editor.preferredContentSize; workflowPopover = popover
        popover.show(relativeTo: search.bounds, of: search, preferredEdge: .maxY)
    }

    @objc private func openSearchFilters() {
        guard folder == .inbox else { showNotice("Open a mailbox to use structured search filters."); return }
        if searchFilterPopover?.isShown == true { searchFilterPopover?.close(); return }
        let popover = NSPopover(); popover.behavior = .transient
        let editor = SearchFilterPanel(base: search.stringValue) { [weak self] query in
            guard let self else { return }
            self.searchRefreshTask?.cancel(); self.search.stringValue = query
            self.setReaderMode(false); self.makeFirstResponder(self.table); self.refreshInbox()
        }
        editor.dismiss = { [weak popover, weak self] in popover?.close(); if let self { self.makeFirstResponder(self.table) } }
        popover.contentViewController = editor
        _ = editor.view; popover.contentSize = editor.preferredContentSize
        searchFilterPopover = popover
        popover.show(relativeTo: search.bounds, of: search, preferredEdge: .maxY)
    }

    private static let searchSyntaxHelp = "Search downloaded mail only; messages not synced yet are excluded. Current mailbox, account and filter stay active.\n\nfrom:person@example.com · to:team@example.com · cc:person@example.com\nsubject:\"project update\" · label:\"Customer Support\"\nis:unread · is:read · is:starred · has:attachment\nin:archive · before:2026-11-01 · after:2026-10-01\n\nUse Archive to search archived mail. Conflicting scopes return no matches. Esc clears search and returns to J/K navigation."

    func controlTextDidBeginEditing(_ notification: Notification) {
        if notification.object as? NSSearchField === search {
            search.layer?.borderColor = EmailTheme.cgColor(EmailTheme.accent, in: effectiveAppearance)
        }
    }

    func controlTextDidEndEditing(_ notification: Notification) {
        if notification.object as? NSSearchField === search {
            search.layer?.borderColor = EmailTheme.cgColor(EmailTheme.line, in: effectiveAppearance)
        }
    }

    private func handle(_ event: NSEvent) -> Bool {
        if palette.isKeyWindow {
            if event.keyCode == 53 { backOrClosePalette(); return true }
            if event.keyCode == 51, paletteSearch.stringValue.isEmpty, palettePage != .root { backOrClosePalette(); return true }
            if palettePage == .snooze, paletteSearch.stringValue.isEmpty,
               let digit = event.charactersIgnoringModifiers?.first?.wholeNumberValue,
               (1...3).contains(digit), paletteResults.indices.contains(digit - 1) {
                runPaletteItem(paletteResults[digit - 1]); return true
            }
            if event.keyCode == 125 { paletteSelection = min(paletteSelection + 1, max(0, paletteButtons.count - 1)); updatePaletteSelection(); return true }
            if event.keyCode == 126 { paletteSelection = max(0, paletteSelection - 1); updatePaletteSelection(); return true }
            if event.keyCode == 36, paletteResults.indices.contains(paletteSelection) {
                runPaletteItem(paletteResults[paletteSelection]); return true
            }
            return false
        }
        // A local event monitor also sees keys in modal alerts. Their text
        // fields belong to another window, while this window's responder may
        // still be the inbox table.
        guard event.window === self, NSApp.modalWindow == nil else { return false }
        if event.modifierFlags.contains(.command), event.charactersIgnoringModifiers?.lowercased() == "k" {
            openCommands(); return true
        }
        if let calendarWorkspace {
            if calendarWorkspace.hasActiveOverlay { return calendarWorkspace.handleKeyboard(event) }
            let editing = (firstResponder as? NSTextView)?.isEditable == true
            let plain = event.modifierFlags.intersection([.command,.control,.option]).isEmpty
            let key = event.charactersIgnoringModifiers?.lowercased()
            if !editing && plain {
                if awaitingGoTo {
                    clearGoTo()
                    switch key {
                    case "i": showInbox(scope: .all); return true
                    case "u": showInbox(scope: .unread); return true
                    case "f": showInbox(scope: .pinned); return true
                    case "s": showInbox(scope: .snoozed); return true
                    case "w": showInbox(scope: .followups); return true
                    case "a": showArchiveClicked(); return true
                    case "d": showDrafts(); return true
                    case "t": showInbox(scope: .sent); return true
                    case "o": showOutbox(); return true
                    case "c": return true
                    default: break
                    }
                }
                if key == "g" { awaitingGoTo = true; goToBadges.forEach { $0.isHidden = false }; mailboxCountBadges.forEach { $0.isHidden = true }; return true }
            }
            if event.keyCode == 53 && awaitingGoTo { clearGoTo(); return true }
            return calendarWorkspace.handleKeyboard(event)
        }
        if let activityController, let focused = firstResponder as? NSView,
           focused === activityController.view || focused.isDescendant(of: activityController.view) {
            if activityController.handleKeyboard(event) { return true }
            return false
        }
        if event.keyCode == 53, let focused = firstResponder as? NSView,
           focused === contactPane || focused.isDescendant(of: contactPane) {
            makeFirstResponder(table); return true
        }
        if event.keyCode == 53, let composePane {
            if composePane.dismissRecipientSuggestions() { return true }
            composePane.requestClose()
            return true
        }
        if composePane != nil { return false }
        if event.modifierFlags.contains([.command, .shift]), event.charactersIgnoringModifiers?.lowercased() == "f" { openSearchFilters(); return true }
        if event.keyCode == 53, let searchEditor = search.currentEditor(), searchEditor === firstResponder {
            searchRefreshTask?.cancel()
            search.stringValue = ""
            makeFirstResponder(table)
            refreshInbox()
            return true
        }
        if event.keyCode == 36, let searchEditor = search.currentEditor(), searchEditor === firstResponder {
            makeFirstResponder(table)
            return true
        }
        if event.keyCode == 53, readerMode {
            setReaderMode(false)
            makeFirstResponder(table)
            return true
        }
        if event.keyCode == 53, awaitingGoTo {
            clearGoTo()
            return true
        }
        if awaitingGoTo && !event.modifierFlags.intersection([.command, .control, .option]).isEmpty {
            clearGoTo()
        }
        if event.modifierFlags.contains(.command), event.charactersIgnoringModifiers?.lowercased() == "r" {
            syncAll(); return true
        }
        if event.modifierFlags.contains(.command), event.charactersIgnoringModifiers?.lowercased() == "n" {
            compose(); return true
        }
        if event.modifierFlags.contains(.command), event.charactersIgnoringModifiers?.lowercased() == "u" {
            unsubscribeSelection(); return true
        }
        if event.modifierFlags.contains(.command), event.charactersIgnoringModifiers == "1" {
            adjustWindowWidth(by: 160); return true
        }
        if event.modifierFlags.contains(.command), event.charactersIgnoringModifiers == "2" {
            adjustWindowWidth(by: -160); return true
        }
        if let editor = firstResponder as? NSTextView, editor.isEditable { return false }
        if let field = firstResponder as? NSTextField, field.isEditable { return false }
        if folder == .agentRequests, calendarWorkspace == nil, let request = agentRequestReader.request {
            let modifiers = event.modifierFlags.intersection([.command, .control, .option, .shift])
            if event.keyCode == 36 || event.keyCode == 76, modifiers == .command {
                if agentRequestReader.approveReady { approveAgentRequest(request) }
                return true
            }
            if modifiers.isEmpty {
                if event.charactersIgnoringModifiers?.lowercased() == "e", request["can_edit"] as? Bool == true { editAgentRequest(request); return true }
                if event.keyCode == 51 || event.keyCode == 117 { rejectAgentRequest(request); return true }
            }
        }
        if event.modifierFlags.contains(.command), event.charactersIgnoringModifiers?.lowercased() == "a", folder == .inbox {
            selectLoadedClicked(); return true
        }
        if event.keyCode == 53, !markedThreadIDs.isEmpty { clearMarkedClicked(); return true }
        if readerMode, event.modifierFlags.contains(.control), event.modifierFlags.intersection([.command, .option]).isEmpty {
            if event.keyCode == 2 { scrollReader(direction: 1); return true } // Ctrl-D
            if event.keyCode == 32 { scrollReader(direction: -1); return true } // Ctrl-U
        }
        if readerMode, event.modifierFlags.intersection([.command, .control, .option]).isEmpty {
            let readerKey = event.charactersIgnoringModifiers?.lowercased()
            if awaitingReaderTop {
                awaitingReaderTop = false
                if readerKey == "g", !event.modifierFlags.contains(.shift) { scrollReaderToBoundary(top: true); return true }
            }
            if event.keyCode == 49 { scrollReader(direction: event.modifierFlags.contains(.shift) ? -1 : 1); return true }
            if event.modifierFlags.contains(.shift), readerKey == "g" { scrollReaderToBoundary(top: false); return true }
            switch readerKey {
            case "j": scrollReaderLine(direction: 1); return true
            case "k": scrollReaderLine(direction: -1); return true
            case "h": navigateConversation(direction: -1); return true
            case "l": navigateConversation(direction: 1); return true
            case "g": awaitingReaderTop = true; return true
            case "/": setReaderMode(false); search.becomeFirstResponder(); return true
            default: break
            }
        }
        if event.modifierFlags.contains(.control), event.modifierFlags.intersection([.command, .option]).isEmpty {
            let halfPage = max(1, Int(inboxScroll.documentVisibleRect.height / max(table.rowHeight, 1) / 2))
            if event.keyCode == 2 { select(selection + halfPage); return true } // Ctrl-D
            if event.keyCode == 32 { select(selection - halfPage); return true } // Ctrl-U
        }
        guard event.modifierFlags.intersection([.command, .control, .option]).isEmpty else { return false }
        if event.keyCode == 36, folder == .drafts {
            editSelectedDraft()
            return true
        }
        // Delete (⌫), forward delete (⌦) and # discard the selected draft.
        if folder == .drafts, event.keyCode == 51 || event.keyCode == 117 || event.characters == "#" {
            discardSelectedDraft()
            return true
        }
        if event.keyCode == 36, folder == .inbox, !readerBody.isHidden {
            clearGoTo()
            setReaderMode(true)
            return true
        }
        let key = event.charactersIgnoringModifiers?.lowercased()
        if awaitingGoTo {
            clearGoTo()
            switch key {
            case "i": showInbox(scope: .all); return true
            case "c": showCalendarWorkspace(); return true
            case "u": showInbox(scope: .unread); return true
            case "f": showInbox(scope: .pinned); return true
            case "s": showInbox(scope: .snoozed); return true
            case "w": showInbox(scope: .followups); return true
            case "a": showArchiveClicked(); return true
            case "d": showDrafts(); return true
            case "t": showInbox(scope: .sent); return true
                    case "o": showOutbox(); return true
            default: break
            }
        }
        if event.characters == "!", folder == .inbox {
            moveToSpamSelection(); return true
        }
        switch key {
        case "g":
            awaitingGoTo = true
            goToBadges.forEach { $0.isHidden = false }; mailboxCountBadges.forEach { $0.isHidden = true }
            return true
        case "x" where folder == .inbox: toggleMarkedSelection(); return true
        case "z" where folder == .inbox: undoTriageClicked(); return true
        case "z" where folder == .drafts: undoDiscardDraft(); return true
        case "j":
            if event.modifierFlags.contains(.shift) { extendMarkedSelection(1) } else { select(selection + 1) }
            return true
        case "k":
            if event.modifierFlags.contains(.shift) { extendMarkedSelection(-1) } else { select(selection - 1) }
            return true
        case "/" where folder != .agentRequests: search.becomeFirstResponder(); return true
        case "e" where folder == .inbox: archiveSelection(allowRestore: event.modifierFlags.contains(.shift)); return true
        case "m" where folder == .inbox: setRead(true); return true
        case "u" where folder == .inbox: setRead(false); return true
        case "f" where folder == .inbox: togglePinSelection(); return true
        case "r" where folder == .inbox: reply(all: event.modifierFlags.contains(.shift)); return true
        case "l" where folder == .inbox: showLabelsClicked(labelsButton); return true
        case "s" where folder == .inbox: snoozeClicked(snoozeButton); return true
        case "i": setLoadRemoteImagesGlobally(!RemoteImagePreference.isEnabled(.standard)); return true
        case "c": compose(); return true
        case "0": toggleContactPane(); return true
        default: return false
        }
    }

    private func clearGoTo() {
        awaitingGoTo = false
        goToBadges.forEach { $0.isHidden = true }; mailboxCountBadges.forEach { $0.isHidden = $0.stringValue.isEmpty }
    }

    private func updateReaderShortcuts() {
        if !markedThreadIDs.isEmpty && composePane == nil {
            readerKeyboardHint.items = [("X", "Select"), ("⇧J ⇧K", "Extend"), ("M U", "Read / unread"), ("E", "Archive"), ("Esc", "Clear")]
        } else if composePane != nil {
            readerKeyboardHint.items = [("Tab", "Next field"), ("⇧Tab", "Previous"), ("⌘↵", "Send"), ("Esc", "Close reply")]
        } else if folder == .drafts {
            readerKeyboardHint.items = [("J K", "Next / previous"), ("↵", "Edit draft"), ("⌫", "Discard"), lastDiscardedDraft == nil ? ("C", "New message") : ("Z", "Undo discard"), ("⌘K", "Commands")]
        } else if folder == .outbox {
            readerKeyboardHint.items = [("J K", "Next / previous"), ("C", "New message"), ("⌘K", "Commands")]
        } else if readerMode {
            readerKeyboardHint.items = [("Esc", "Inbox"), ("J K", "Scroll"), ("⌃D ⌃U", "Page"), ("G", "Bottom")] + (readerMessageCount > 1 ? [("H L", "History")] : [])
        } else {
            readerKeyboardHint.items = [("J K", "Next / previous"), ("↵", "Read"), ("R", "Reply"), (inboxScope == .archived ? "⇧E" : "E", inboxScope == .archived ? "Move to Inbox" : "Archive"), ("⌘K", "Commands")]
        }
        // Mail shortcuts mean nothing before there is mail.
        readerKeyboardHint.isHidden = hasNoAccounts && composePane == nil
    }

    /// No account yet: the window is a welcome with one action.
    private var hasNoAccounts: Bool { usesCore && knownAccounts.isEmpty }

    /// A calendar connects only to an account that signed in with Google
    /// (`connect_calendar`), which this build may not offer.
    private var calendarAvailable: Bool {
        !knownAccounts.isEmpty && (core?.buildFeatures().googleOAuthPublic == true || knownAccounts.contains { $0.provider == "gmail" })
    }

    /// Show the mail chrome only once there is an account, and Calendar only
    /// where one can be connected.
    private func updateAccountChrome() {
        let empty = hasNoAccounts
        accountOnlySidebarViews.forEach { $0.isHidden = empty }
        calendarSidebarButton?.isHidden = !calendarAvailable
        updateAgentRequestsChrome()
        inboxPanel?.isHidden = empty && activityController == nil
        if scopeTabButtons.count > 1 && empty { scopeTabButtons[1].title = "Unread" }
        updateReaderShortcuts()
    }

    /// The app's primary button: accent fill, text in the canvas color on dark.
    private func paintEmptyAction() {
        emptyAction.layer?.backgroundColor = EmailTheme.cgColor(EmailTheme.accent, in: effectiveAppearance)
        let ink = EmailTheme.isDark(effectiveAppearance) ? EmailTheme.canvas : NSColor.white
        emptyAction.attributedTitle = NSAttributedString(string: "Add an account", attributes: [.foregroundColor: ink, .font: NSFont.systemFont(ofSize: 12, weight: .semibold)])
    }

    private func setReaderMode(_ enabled: Bool) {
        readerMode = enabled
        awaitingReaderTop = false
        updateReaderShortcuts()

    }

    private func scrollReaderLine(direction: Int) {
        if let webView, !webView.isHidden {
            webView.evaluateJavaScript("window.scrollBy({top: 96 * \(direction), behavior: 'smooth'})", completionHandler: nil)
        } else if !bodyScroll.isHidden {
            let clip = bodyScroll.contentView
            clip.scroll(to: NSPoint(x: 0, y: max(0, clip.bounds.origin.y + CGFloat(96 * direction))))
            bodyScroll.reflectScrolledClipView(clip)
        }
    }

    private func scrollReaderToBoundary(top: Bool) {
        if let webView, !webView.isHidden {
            webView.evaluateJavaScript("window.scrollTo({top: \(top ? "0" : "document.documentElement.scrollHeight"), behavior: 'smooth'})", completionHandler: nil)
        } else if !bodyScroll.isHidden {
            let clip = bodyScroll.contentView
            let end = max(0, (bodyScroll.documentView?.bounds.height ?? 0) - clip.bounds.height)
            clip.scroll(to: NSPoint(x: 0, y: top ? 0 : end))
            bodyScroll.reflectScrolledClipView(clip)
        }
    }

    private func scrollReader(direction: Int) {
        if let webView, !webView.isHidden {
            webView.evaluateJavaScript("window.scrollBy({top: window.innerHeight * \(direction) * 0.82, behavior: 'smooth'})", completionHandler: nil)
        } else if !bodyScroll.isHidden {
            let clip = bodyScroll.contentView
            let nextY = max(0, clip.bounds.origin.y + clip.bounds.height * CGFloat(direction) * 0.82)
            clip.scroll(to: NSPoint(x: 0, y: nextY))
            bodyScroll.reflectScrolledClipView(clip)
        }
    }

    private func navigateConversation(direction: Int) {
        guard readerMessageCount > 1, let webView, !webView.isHidden else { return }
        let script = """
        (() => {
          const cards = Array.from(document.querySelectorAll('details.message-card'));
          if (!cards.length) return;
          const active = cards.findIndex(card => card.open);
          const next = Math.max(0, Math.min(cards.length - 1, (active < 0 ? cards.length - 1 : active) + \(direction)));
          cards.forEach((card, index) => { card.open = index === next; });
          cards[next].scrollIntoView({block: 'start', behavior: 'smooth'});
        })()
        """
        webView.evaluateJavaScript(script, completionHandler: nil)
    }

    private func select(_ row: Int) {
        let count = switch folder {
        case .drafts: filteredDrafts.count
        case .outbox: filteredOutbox.count
        case .agentRequests: agentRequests.count
        case .inbox: threads.count
        }
        guard count > 0 else { return }
        selection = min(max(row, 0), count - 1)
        table.selectRowIndexes(IndexSet(integer: selection), byExtendingSelection: false)
        table.scrollRowToVisible(selection)
    }

    private func showNotice(_ message: String) {
        let alert = NSAlert()
        alert.messageText = "Email"
        alert.informativeText = message
        alert.runModal()
    }
}

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    private var window: MailWindow?
    private var appearanceItems: [NSMenuItem] = []
    private var notificationsItem: NSMenuItem?
    private var notificationPreviewsItem: NSMenuItem?

    func setAppearanceMode(_ mode: Int) {
        UserDefaults.standard.set(mode, forKey: appearanceModeKey)
        applyAppearance(mode)
    }

    private func applyAppearance(_ mode: Int) {
        NSApp.appearance = switch mode {
        case 1: NSAppearance(named: .aqua)
        case 2: NSAppearance(named: .darkAqua)
        default: nil
        }
        window?.appearance = NSApp.appearance
        window?.applyTheme()
        for item in appearanceItems { item.state = item.tag == mode ? .on : .off }
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        if let iconURL = Bundle.main.url(forResource: "Email", withExtension: "icns"),
           let icon = NSImage(contentsOf: iconURL) {
            NSApp.applicationIconImage = icon
        }
        RemoteImagePreference.prepare(.standard)
        applyAppearance(UserDefaults.standard.integer(forKey: appearanceModeKey))
        MailNotifications.shared.start()
        MailNotifications.shared.onSettingsChanged = { [weak self] in self?.updateNotificationMenu() }
        MailNotifications.shared.onOpen = { [weak self] accountID, threadID in
            self?.showMainWindow()
            self?.window?.openNotification(accountID: accountID, threadID: threadID)
        }
        MailNotifications.shared.onOpenTracked = { [weak self] accountID, threadID in
            self?.showMainWindow()
            self?.window?.openTrackedNotification(accountID: accountID, threadID: threadID)
        }
        MailNotifications.shared.onOpenFollowup = { [weak self] accountID, threadID in
            self?.showMainWindow()
            self?.window?.openFollowupNotification(accountID: accountID, threadID: threadID)
        }
        MailNotifications.shared.onTriage = { [weak self] kind, accountID, threadID in
            self?.window?.triageFromNotification(kind: kind, accountID: accountID, threadID: threadID)
        }
        MailNotifications.shared.onOpenActivity = { [weak self] in self?.window?.refreshTrackingButton() }
        MailNotifications.shared.onOpenAgent = { [weak self] requestID in
            self?.showMainWindow()
            self?.window?.openAgentRequest(id: requestID)
        }
        MailNotifications.shared.isReadingThread = { [weak self] accountID, threadID in
            self?.window?.isReadingNotificationThread(accountID: accountID, threadID: threadID) ?? false
        }
        EmailUpdaterLoader.hasActiveDraft = { [weak self] in self?.window?.hasOpenComposer ?? false }
        installMenu()
        showMainWindow()
    }

    private func installMenu() {
        let main = NSMenu()
        let appItem = NSMenuItem()
        let appMenu = NSMenu()
        let notifications = NSMenuItem(title: "Notifications", action: nil, keyEquivalent: "")
        let notificationMenu = NSMenu(title: "Notifications")
        notificationsItem = notificationMenu.addItem(withTitle: "Enable Notifications", action: #selector(toggleNotifications(_:)), keyEquivalent: "")
        notificationsItem?.target = self
        notificationPreviewsItem = notificationMenu.addItem(withTitle: "Show Sender and Subject", action: #selector(toggleNotificationPreviews(_:)), keyEquivalent: "")
        notificationPreviewsItem?.target = self
        let testNotification = notificationMenu.addItem(withTitle: "Send Test Notification", action: #selector(testNotification(_:)), keyEquivalent: "")
        testNotification.target = self
        notifications.submenu = notificationMenu
        appMenu.addItem(notifications)
        appMenu.addItem(.separator())
        updateNotificationMenu()
        appMenu.addItem(withTitle: "Quit Email", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
        // Release bundles with a configured feed add update controls here.
        EmailUpdaterLoader.install(into: appMenu, at: 0)
        appItem.submenu = appMenu
        main.addItem(appItem)

        // Cocoa text editors resolve standard editing shortcuts through the
        // responder-chain Edit menu, including field editors in alerts/palettes.
        let editItem = NSMenuItem(title: "Edit", action: nil, keyEquivalent: "")
        let editMenu = NSMenu(title: "Edit")
        editMenu.addItem(withTitle: "Undo", action: Selector(("undo:")), keyEquivalent: "z")
        let redo = editMenu.addItem(withTitle: "Redo", action: Selector(("redo:")), keyEquivalent: "z")
        redo.keyEquivalentModifierMask = [.command, .shift]
        editMenu.addItem(.separator())
        editMenu.addItem(withTitle: "Cut", action: #selector(NSText.cut(_:)), keyEquivalent: "x")
        editMenu.addItem(withTitle: "Copy", action: #selector(NSText.copy(_:)), keyEquivalent: "c")
        editMenu.addItem(withTitle: "Paste", action: #selector(NSText.paste(_:)), keyEquivalent: "v")
        editMenu.addItem(withTitle: "Select All", action: #selector(NSText.selectAll(_:)), keyEquivalent: "a")
        editItem.submenu = editMenu
        main.addItem(editItem)

        let viewItem = NSMenuItem(title: "View", action: nil, keyEquivalent: "")
        let viewMenu = NSMenu(title: "View")
        let appearanceItem = NSMenuItem(title: "Appearance", action: nil, keyEquivalent: "")
        let appearanceMenu = NSMenu(title: "Appearance")
        for (title, mode) in [("Follow System", 0), ("Light", 1), ("Dark", 2)] {
            let item = appearanceMenu.addItem(withTitle: title, action: #selector(selectAppearance(_:)), keyEquivalent: "")
            item.target = self
            item.tag = mode
            item.state = UserDefaults.standard.integer(forKey: appearanceModeKey) == mode ? .on : .off
            appearanceItems.append(item)
        }
        appearanceItem.submenu = appearanceMenu
        viewMenu.addItem(appearanceItem)
        let cycleAppearance = viewMenu.addItem(withTitle: "Cycle Appearance", action: #selector(cycleAppearance(_:)), keyEquivalent: "a")
        cycleAppearance.keyEquivalentModifierMask = [.command, .shift]
        cycleAppearance.target = self
        viewMenu.addItem(.separator())
        let reset = viewMenu.addItem(withTitle: "Reset Window Size", action: #selector(resetWindowSize(_:)), keyEquivalent: "")
        reset.target = self
        let fillWidth = viewMenu.addItem(withTitle: "Fill Available Width", action: #selector(fillAvailableWidth(_:)), keyEquivalent: "f")
        fillWidth.keyEquivalentModifierMask = [.command, .shift]
        fillWidth.target = self
        let wider = viewMenu.addItem(withTitle: "Wider Window", action: #selector(widenWindow(_:)), keyEquivalent: "1")
        wider.target = self
        let narrower = viewMenu.addItem(withTitle: "Narrower Window", action: #selector(narrowWindow(_:)), keyEquivalent: "2")
        narrower.target = self
        viewMenu.addItem(.separator())
        let remoteImages = viewMenu.addItem(withTitle: "Always Load Remote Images", action: #selector(toggleRemoteImages(_:)), keyEquivalent: "i")
        remoteImages.keyEquivalentModifierMask = [.command, .shift]
        remoteImages.target = self
        remoteImages.state = RemoteImagePreference.isEnabled(.standard) ? .on : .off
        let zoom = viewMenu.addItem(withTitle: "Zoom Window", action: #selector(zoomWindow(_:)), keyEquivalent: "")
        zoom.target = self
        let fullScreen = viewMenu.addItem(withTitle: "Enter Full Screen", action: #selector(toggleFullScreen(_:)), keyEquivalent: "f")
        fullScreen.keyEquivalentModifierMask = [.command, .control]
        fullScreen.target = self
        viewItem.submenu = viewMenu
        main.addItem(viewItem)
        NSApp.mainMenu = main
    }

    @objc private func fillAvailableWidth(_ sender: Any?) {
        window?.fillAvailableWidth(sender)
    }

    @objc private func widenWindow(_ sender: Any?) {
        window?.adjustWindowWidth(by: 160)
    }

    @objc private func narrowWindow(_ sender: Any?) {
        window?.adjustWindowWidth(by: -160)
    }

    @objc private func toggleRemoteImages(_ sender: NSMenuItem) {
        let enabled = sender.state != .on
        sender.state = enabled ? .on : .off
        window?.setLoadRemoteImagesGlobally(enabled)
    }

    @objc private func selectAppearance(_ sender: NSMenuItem) {
        UserDefaults.standard.set(sender.tag, forKey: appearanceModeKey)
        applyAppearance(sender.tag)
    }

    @objc private func cycleAppearance(_ sender: Any?) {
        let next = (UserDefaults.standard.integer(forKey: appearanceModeKey) + 1) % 3
        UserDefaults.standard.set(next, forKey: appearanceModeKey)
        applyAppearance(next)
    }

    @objc private func resetWindowSize(_ sender: Any?) {
        window?.resetWindowSize(sender)
    }

    @objc private func zoomWindow(_ sender: Any?) {
        window?.performZoom(sender)
    }

    @objc private func toggleFullScreen(_ sender: Any?) {
        window?.toggleFullScreen(sender)
    }

    @objc private func toggleNotifications(_ sender: Any?) { MailNotifications.shared.toggleEnabled() }
    @objc private func toggleNotificationPreviews(_ sender: Any?) { MailNotifications.shared.togglePreviews() }
    @objc private func testNotification(_ sender: Any?) { MailNotifications.shared.sendTest() }
    private func updateNotificationMenu() {
        notificationsItem?.state = MailNotifications.shared.enabled ? .on : .off
        notificationPreviewsItem?.state = MailNotifications.shared.previews ? .on : .off
    }
    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        showMainWindow()
        return true
    }
    func showMainWindow() {
        if let window {
            window.makeKeyAndOrderFront(nil)
            NSApp.activate(ignoringOtherApps: true)
            return
        }
        NSApp.setActivationPolicy(.regular)
        window = MailWindow()
        window?.isReleasedWhenClosed = false
        window?.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        window?.restorePreferredWidth()
    }
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { !MailNotifications.shared.enabled }
}

@MainActor
public struct EmailDesktop {
    private static let appDelegate = AppDelegate()

    public static func main() {
        if IsolatedStore.isEmpty {
            fputs("Email: \(IsolatedStore.emptyMessage)\n", stderr)
            exit(2)
        }
        // A fixture relaunch verifies the installed updater, then exits before
        // creating a mailbox, starting sync or accessing user credentials.
        if CommandLine.arguments.count == 1,
           Bundle.main.bundleIdentifier == "invalid.email.update-fixture" {
            print(EmailUpdaterLoader.statusJSON())
            return
        }
        if CommandLine.arguments.contains("--updater-status") {
            print(EmailUpdaterLoader.statusJSON())
            return
        }
        if CommandLine.arguments.contains("--updater-fixture-install") {
            exit(EmailUpdaterLoader.fixtureInstall(timeout: 120))
        }
        if CommandLine.arguments.contains("--dispatch-due") {
            guard let core = CoreBridge() else { fputs("mail-service unavailable\n", stderr); exit(2) }
            do {
                _ = try core.request(["op": "dispatch_due"])
            } catch {
                fputs("scheduled mail dispatch failed: \(error.localizedDescription)\n", stderr)
                exit(3)
            }
            return
        }
        if CommandLine.arguments.contains("--smoke-service") || CommandLine.arguments.contains("--smoke-core") {
            guard let core = CoreBridge() else { fputs("mail-service unavailable\n", stderr); exit(2) }
            do {
                if let demo = IsolatedStore.demoRequest {
                    _ = try core.request(demo)
                }
                guard let accounts = try core.request(["op": "list_accounts"]) as? [[String: Any]],
                      let threads = try core.request(["op": "list_threads", "limit": 100, "offset": 0]) as? [[String: Any]] else {
                    fputs("mail-service smoke returned an invalid data shape\n", stderr); exit(3)
                }
                if let first = threads.first, let id = first["id"] as? String,
                   let message = try core.request(["op": "get_latest_message_for_thread", "thread_id": id]) as? [String: Any],
                   let messageID = message["id"] as? String {
                    _ = try core.request(["op": "render_message", "message_id": messageID])
                }
                print("mail-service connected: \(accounts.count) accounts, \(threads.count) inbox threads")
            } catch {
                fputs("mail-service smoke failed: \(error.localizedDescription)\n", stderr); exit(3)
            }
            return
        }
        if CommandLine.arguments.contains("--smoke-composer-window") {
            // mail-service refuses demo data in the real store, so a successful
            // seed proves the store is disposable.
            func seededDisposableStore() -> Bool {
                guard let demo = IsolatedStore.demoRequest, let bridge = CoreBridge() else { return false }
                do { _ = try bridge.request(demo); return true } catch { return false }
            }
            guard seededDisposableStore() else {
                fputs("Use EMAIL_DEMO=1 and EMAIL_STORE_PATH pointing to a disposable store.\n", stderr)
                exit(2)
            }
            let app = NSApplication.shared
            app.setActivationPolicy(.regular)
            app.finishLaunching()
            let window = MailWindow()
            exit(window.smokeComposerWindowLayout() ? 0 : 4)
        }
        let app = NSApplication.shared
        app.delegate = appDelegate
        app.finishLaunching()
        appDelegate.showMainWindow()
        app.run()
    }
}

@_cdecl("email_ui_main")
public func emailUIMain() {
    MainActor.assumeIsolated { EmailDesktop.main() }
}
