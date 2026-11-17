import AppKit

/// The connection to the build's Scheduling service, shared by every booking
/// surface. Nothing about Scheduling shows until someone connects it in Manage
/// accounts (or with Connect Scheduling… in the command palette). After that
/// the composer's Booking link, Calendar's Bookings and links and the booking
/// commands appear, and Disconnect hides them again. The profile's Scheduling
/// origin only decides whether the Manage accounts entry exists at all.
@MainActor
final class SchedulingConnection {
    static let shared = SchedulingConnection()
    /// Posted whenever `state` or `problem` changes.
    static let changed = Notification.Name("EmailSchedulingConnectionChanged")

    enum State: Equatable {
        case disconnected
        /// Waiting for approval in the browser, with the code the browser may
        /// ask for.
        case waiting(code: String)
        /// The Scheduling account's address or name, when the service gave one.
        case connected(account: String?)
    }

    private(set) var state: State = .disconnected
    /// The last failure, shown in Manage accounts until the next action.
    private(set) var problem: String?
    /// Called once when approval completes, so the main window can say so.
    var onConnected: (() -> Void)?

    /// The build names a Scheduling service.
    var available: Bool { DistributionProfile.current?.hasScheduling == true }
    var isConnected: Bool {
        guard available, case .connected = state else { return false }
        return true
    }
    var name: String { DistributionProfile.current?.schedulingName ?? "Scheduling" }

    private var core: CoreBridge?
    private var poll: Timer?
    private var deadline = Date.distantPast
    private var pollFailures = 0
    /// Bumped by every action, so a late answer to an older one is ignored.
    private var generation = 0

    /// Load the saved connection once the mail engine is open.
    func start(core: CoreBridge) {
        guard available, self.core == nil else { return }
        self.core = core
        Task { await refresh() }
    }

    /// Re-read the saved connection, for example after the service rejected it.
    func refresh() async {
        guard available, let core else { return }
        if case .waiting = state { return }
        let serial = generation
        guard let status = try? await core.callAsync(["op": "booking_status"]) as? [String: Any], serial == generation else { return }
        if case .waiting = state { return }
        set(status["connected"] as? Bool == true ? .connected(account: Self.account(status["host"])) : .disconnected)
    }

    /// Start a device connection: open the browser and wait for approval.
    func connect() {
        guard available, let core else { return }
        generation += 1
        let serial = generation
        stopPolling()
        problem = nil
        let device = "Email on \(Host.current().localizedName ?? "Mac")"
        Task { @MainActor in
            do {
                let data = try await core.callAsync(["op": "booking_connect_start", "name": device]) as? [String: Any] ?? [:]
                guard serial == generation else { return }
                guard let code = data["user_code"] as? String, let raw = data["verification_url"] as? String,
                      let url = URL(string: raw), DistributionProfile.current?.isSchedulingURL(url) == true else {
                    return fail("Scheduling returned an address Email doesn't trust.")
                }
                deadline = Date().addingTimeInterval((data["expires_in"] as? Double) ?? 600)
                set(.waiting(code: code))
                NSWorkspace.shared.open(url)
                startPolling(every: max(5, (data["interval"] as? Double) ?? 5))
            } catch {
                guard serial == generation else { return }
                fail(error.localizedDescription)
            }
        }
    }

    /// Stop waiting for the browser.
    func cancel() {
        generation += 1
        stopPolling()
        problem = nil
        set(.disconnected)
    }

    /// Revoke this device's connection. No confirmation: connecting again is
    /// one browser approval away.
    func disconnect() {
        guard let core else { return }
        generation += 1
        let serial = generation
        stopPolling()
        Task { @MainActor in
            do {
                _ = try await core.callAsync(["op": "booking_disconnect"])
                guard serial == generation else { return }
                problem = nil
                set(.disconnected)
            } catch {
                guard serial == generation else { return }
                fail(error.localizedDescription)
            }
        }
    }

    /// Only the AppKit check harness sets a state directly.
    func setForChecks(_ state: State, problem: String? = nil) {
        generation += 1
        stopPolling()
        self.problem = problem
        self.state = state
        NotificationCenter.default.post(name: Self.changed, object: nil)
    }

    private func startPolling(every interval: TimeInterval) {
        pollFailures = 0
        let timer = Timer(timeInterval: interval, repeats: true) { _ in
            MainActor.assumeIsolated { SchedulingConnection.shared.checkApproval() }
        }
        // Common modes: approval also arrives while Manage accounts is open.
        RunLoop.main.add(timer, forMode: .common)
        poll = timer
    }

    private func stopPolling() {
        poll?.invalidate()
        poll = nil
    }

    private func checkApproval() {
        guard case .waiting = state, let core else { return stopPolling() }
        guard Date() < deadline else {
            stopPolling()
            set(.disconnected)
            return fail("The connection expired. Connect to try again.")
        }
        let serial = generation
        Task { @MainActor in
            do {
                let result = try await core.callAsync(["op": "booking_connect_poll"]) as? [String: Any] ?? [:]
                guard serial == generation, case .waiting = state else { return }
                pollFailures = 0
                guard result["connected"] as? Bool == true else { return }
                stopPolling()
                problem = nil
                set(.connected(account: Self.account(result["host"])))
                onConnected?()
            } catch {
                guard serial == generation, case .waiting = state else { return }
                // A brief network failure shouldn't end the wait; repeated ones do.
                pollFailures += 1
                guard pollFailures >= 3 else { return }
                stopPolling()
                set(.disconnected)
                fail(error.localizedDescription)
            }
        }
    }

    private func set(_ new: State) {
        guard new != state else { return }
        state = new
        NotificationCenter.default.post(name: Self.changed, object: nil)
    }

    private func fail(_ message: String) {
        problem = message
        NotificationCenter.default.post(name: Self.changed, object: nil)
    }

    private static func account(_ host: Any?) -> String? {
        let host = host as? [String: Any]
        return [host?["email"], host?["name"]].lazy.compactMap { ($0 as? String)?.trimmingCharacters(in: .whitespaces) }.first { !$0.isEmpty }
    }
}

/// The Scheduling account in Manage accounts, under the mail accounts. Manage
/// accounts is an NSAlert, so this is its accessory view: the alert's own 13 pt
/// text in its 220 pt text column, status in the secondary color and actions
/// as text links. It follows the connection while the alert is open and asks
/// the alert to lay itself out again when its height changes.
@MainActor
final class SchedulingAccountEntry: NSView {
    static let width: CGFloat = 220
    weak var alert: NSAlert?
    private let lines = NSStackView()

    init() {
        super.init(frame: NSRect(x: 0, y: 0, width: Self.width, height: 16))
        lines.orientation = .vertical
        lines.alignment = .leading
        lines.spacing = 0 // the alert's account list runs 16 pt per line
        lines.translatesAutoresizingMaskIntoConstraints = false
        addSubview(lines)
        NSLayoutConstraint.activate([
            // NSAlert insets its informative text 2 pt more than an accessory.
            lines.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 2),
            lines.trailingAnchor.constraint(lessThanOrEqualTo: trailingAnchor),
            lines.topAnchor.constraint(equalTo: topAnchor),
        ])
        render()
        NotificationCenter.default.addObserver(self, selector: #selector(connectionDidChange), name: SchedulingConnection.changed, object: nil)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    @objc private func connectionDidChange() { render() }

    /// The visible lines, for the check harness and accessibility tests.
    var texts: [String] { lines.arrangedSubviews.flatMap(Self.strings) }

    private func render() {
        lines.arrangedSubviews.forEach { lines.removeArrangedSubview($0); $0.removeFromSuperview() }
        let connection = SchedulingConnection.shared
        lines.addArrangedSubview(text(connection.name))
        switch connection.state {
        case .disconnected:
            lines.addArrangedSubview(row([text("Not connected", muted: true), dot(), link("Connect", label: "Connect \(connection.name)") { connection.connect() }]))
        case .waiting(let code):
            lines.addArrangedSubview(row([text("Waiting for your browser", muted: true), dot(), link("Cancel", label: "Stop connecting \(connection.name)") { connection.cancel() }]))
            lines.addArrangedSubview(text("Code if asked: \(code)", muted: true))
        case .connected(let account):
            if let account { lines.addArrangedSubview(text(account)) }
            lines.addArrangedSubview(text("Connected on this Mac", muted: true))
            var actions: [NSView] = [link("Disconnect", label: "Disconnect \(connection.name)") { connection.disconnect() }]
            if let studio = DistributionProfile.current?.schedulingURL(path: "/studio") {
                actions += [dot(), link("Manage online", label: "Manage \(connection.name) online") { NSWorkspace.shared.open(studio) }]
            }
            lines.addArrangedSubview(row(actions))
        }
        if let problem = connection.problem {
            let field = text(problem)
            field.textColor = .systemRed
            field.lineBreakMode = .byWordWrapping
            field.maximumNumberOfLines = 3
            field.preferredMaxLayoutWidth = Self.width - 2
            lines.addArrangedSubview(field)
        }
        lines.layoutSubtreeIfNeeded()
        let height = lines.fittingSize.height
        guard frame.height != height else { return }
        setFrameSize(NSSize(width: Self.width, height: height))
        alert?.layout()
    }

    private func text(_ value: String, muted: Bool = false) -> NSTextField {
        let field = NSTextField(labelWithString: value)
        field.font = .systemFont(ofSize: 13)
        field.textColor = muted ? .secondaryLabelColor : .labelColor
        field.lineBreakMode = .byTruncatingMiddle
        field.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        return field
    }

    private func dot() -> NSTextField { text("·", muted: true) }

    private func link(_ title: String, label: String, _ run: @escaping () -> Void) -> NSTextField {
        let field = SchedulingLink(title: title, run: run)
        field.font = .systemFont(ofSize: 13)
        field.setAccessibilityLabel(label)
        return field
    }

    private func row(_ views: [NSView]) -> NSStackView {
        let row = NSStackView(views: views)
        row.spacing = 4
        row.alignment = .firstBaseline
        return row
    }

    private static func strings(_ view: NSView) -> [String] {
        if let field = view as? NSTextField { return [field.stringValue] }
        return view.subviews.flatMap(strings)
    }
}

/// A text link that lines up with the label text around it (a borderless
/// button insets its title). Clickable, a link to VoiceOver, and a key view
/// with Full Keyboard Access like a button: Tab reaches it, Space runs it.
@MainActor
private final class SchedulingLink: NSTextField {
    private let run: () -> Void
    init(title: String, run: @escaping () -> Void) {
        self.run = run
        super.init(frame: .zero)
        stringValue = title
        textColor = .linkColor
        // Configured as NSTextField(labelWithString:) is: one line, no bezel.
        isEditable = false; isSelectable = false; isBordered = false; isBezeled = false; drawsBackground = false
        usesSingleLineMode = true; cell?.wraps = false; cell?.isScrollable = false; lineBreakMode = .byClipping
        setContentHuggingPriority(.required, for: .horizontal)
        setContentCompressionResistancePriority(.required, for: .horizontal)
        setAccessibilityRole(.link)
        focusRingType = .default
        addGestureRecognizer(NSClickGestureRecognizer(target: self, action: #selector(clicked)))
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    @objc private func clicked() { run() }
    override func accessibilityPerformPress() -> Bool { run(); return true }
    override func resetCursorRects() { addCursorRect(bounds, cursor: .pointingHand) }
    override var acceptsFirstResponder: Bool { true }
    override var canBecomeKeyView: Bool { NSApp.isFullKeyboardAccessEnabled }
    override var focusRingMaskBounds: NSRect { bounds }
    override func drawFocusRingMask() { NSBezierPath(roundedRect: bounds, xRadius: 3, yRadius: 3).fill() }
    override func keyDown(with event: NSEvent) {
        if event.charactersIgnoringModifiers == " " { run() } else { super.keyDown(with: event) }
    }
}
