import AppKit

/// Read-only desktop access to the hosted scheduler. Calendar grants remain on the server.
@MainActor final class BookingPanel: NSWindowController, NSWindowDelegate {
    private static var active: BookingPanel?
    static func present(core: CoreBridge, insert: ((String) -> Void)? = nil, emailGuest: ((String, String?) -> Void)? = nil, findMail: ((String) -> Void)? = nil) {
        active?.close()
        let panel = BookingPanel(core: core, insert: insert, emailGuest: emailGuest, findMail: findMail)
        active = panel
        panel.showWindow(nil)
        panel.window?.makeKeyAndOrderFront(nil)
    }
    private let core: CoreBridge
    private let insert: ((String) -> Void)?
    private let emailGuest: ((String, String?) -> Void)?
    private let findMail: ((String) -> Void)?
    private let rows = NSStackView()
    private let notice = NSTextField(wrappingLabelWithString: "")
    private var poll: Timer?
    private var generation = 0
    private var pending = false
    private var pollDeadline = Date.distantPast
    private let footer = NSStackView()
    private var refreshButton: NSButton?
    private var titleLabel: NSTextField?
    private var connectionLayout: Bool?
    init(core: CoreBridge, insert: ((String) -> Void)?, emailGuest: ((String, String?) -> Void)?, findMail: ((String) -> Void)?) {
        self.core = core; self.insert = insert; self.emailGuest = emailGuest; self.findMail = findMail
        let window = BookingWindow(contentRect: NSRect(x: 0,y: 0,width: 500,height: 320), styleMask: [.titled,.closable,.resizable], backing: .buffered, defer: false)
        window.title = insert == nil ? "Scheduling" : "Insert booking link"
        window.minSize = NSSize(width: 460,height: 280)
        super.init(window: window)
        window.delegate = self
        let root = NSStackView(); root.orientation = .vertical; root.alignment = .leading; root.spacing = 18; root.translatesAutoresizingMaskIntoConstraints = false
        let content = NSView(); window.contentView = content; content.addSubview(root)
        NSLayoutConstraint.activate([root.leadingAnchor.constraint(equalTo: content.leadingAnchor,constant:24),root.trailingAnchor.constraint(equalTo: content.trailingAnchor,constant:-24),root.topAnchor.constraint(equalTo: content.topAnchor,constant:20),root.bottomAnchor.constraint(equalTo: content.bottomAnchor,constant:-20)])
        let header = NSStackView(); header.spacing = 10
        let title = text(insert == nil ? "Scheduling" : "Booking links",size:22,strong:true); header.addArrangedSubview(title); titleLabel = title
        let space = NSView(); space.setContentHuggingPriority(.defaultLow,for:.horizontal); header.addArrangedSubview(space)
        let refresh = button("Refresh") { [weak self] in self?.load(refresh:true) }; refreshButton = refresh; header.addArrangedSubview(refresh)
        root.addArrangedSubview(header); header.widthAnchor.constraint(equalTo: root.widthAnchor).isActive = true
        notice.font = .systemFont(ofSize:12); notice.textColor = .secondaryLabelColor; root.addArrangedSubview(notice)
        let scroll = NSScrollView(); scroll.hasVerticalScroller = true; scroll.drawsBackground = false; scroll.translatesAutoresizingMaskIntoConstraints = false
        let doc = BookingDocumentView(); doc.translatesAutoresizingMaskIntoConstraints = false; scroll.documentView = doc
        rows.orientation = .vertical; rows.alignment = .leading; rows.spacing = 14; rows.translatesAutoresizingMaskIntoConstraints = false; doc.addSubview(rows)
        NSLayoutConstraint.activate([doc.widthAnchor.constraint(equalTo: scroll.contentView.widthAnchor),rows.leadingAnchor.constraint(equalTo:doc.leadingAnchor),rows.trailingAnchor.constraint(equalTo:doc.trailingAnchor),rows.topAnchor.constraint(equalTo:doc.topAnchor),rows.bottomAnchor.constraint(equalTo:doc.bottomAnchor,constant:-12)])
        root.addArrangedSubview(scroll); scroll.widthAnchor.constraint(equalTo:root.widthAnchor).isActive = true; scroll.heightAnchor.constraint(greaterThanOrEqualToConstant:100).isActive = true
        footer.spacing = 14
        if let studio = DistributionProfile.current?.schedulingURL(path: "/studio") { footer.addArrangedSubview(button("Manage online ↗") { NSWorkspace.shared.open(studio) }) }
        footer.addArrangedSubview(button("Disconnect this device") { [weak self] in self?.disconnect() })
        root.addArrangedSubview(footer)
        window.center(); load(refresh:false)
    }
    required init?(coder:NSCoder) { fatalError("init(coder:) has not been implemented") }
    override func close() { generation += 1; pending = false; poll?.invalidate(); poll = nil; super.close() }
    func windowWillClose(_ notification: Notification) { generation += 1; pending = false; poll?.invalidate(); poll = nil }
    private func text(_ value:String,size:CGFloat = 13,strong:Bool = false) -> NSTextField {
        let label = NSTextField(wrappingLabelWithString:value); label.font = .systemFont(ofSize:size,weight:strong ? .semibold : .regular); label.isSelectable = true; return label
    }
    private func button(_ title:String,_ run:@escaping () -> Void) -> NSButton { let b = BookingAction(title:title,run:run); b.bezelStyle = .rounded; return b }
    private func clear() { rows.arrangedSubviews.forEach { rows.removeArrangedSubview($0); $0.removeFromSuperview() } }
    private func request(_ payload:[String:Any], done:@escaping ([String:Any]) -> Void) {
        let serial = generation
        Task { [weak self] in
            guard let self else { return }
            do { let value = try await core.callAsync(payload) as? [String:Any] ?? [:]; guard serial == generation else { return }; done(value) }
            catch { guard serial == generation else { return }; notice.stringValue = error.localizedDescription }
        }
    }
    private func layoutConnection(_ connecting: Bool) {
        footer.isHidden = connecting; refreshButton?.isHidden = connecting
        guard connectionLayout != connecting else { return }; connectionLayout = connecting
        window?.minSize = NSSize(width:460,height:connecting ? 280 : 380)
        guard let window else { return }
        let contentSize = NSSize(width:connecting ? 500 : 570,height:connecting ? 320 : 450)
        var frame = window.frameRect(forContentRect:NSRect(origin:.zero,size:contentSize))
        frame.origin = NSPoint(x:window.frame.midX-frame.width/2,y:window.frame.maxY-frame.height)
        window.setFrame(frame,display:true,animate:false)
    }
    private func load(refresh:Bool) {
        notice.stringValue = refresh ? "Refreshing…" : "Loading saved scheduling…"
        request(["op":"booking_status"]) { [weak self] status in
            guard let self else { return }
            if status["connected"] as? Bool != true {
                clear(); layoutConnection(true); titleLabel?.stringValue = "Connect scheduling"
                notice.stringValue = "Your booking links and meetings, right here in Email."
                rows.addArrangedSubview(text("Continue in your browser to connect your account. You’ll return here automatically."))
                let connectButton = button("Continue in browser") { [weak self] in self?.connect() }
                connectButton.keyEquivalent = "\r"; rows.addArrangedSubview(connectButton); return
            }
            request(["op":"booking_list","refresh":refresh]) { [weak self] value in self?.render(value); if !refresh { self?.load(refresh:true) } }
        }
    }
    private func render(_ value:[String:Any]) {
        clear(); layoutConnection(false); titleLabel?.stringValue = insert == nil ? "Scheduling" : "Booking links"
        let host = value["host"] as? [String:Any] ?? [:]
        notice.stringValue = [host["email"] as? String ?? "Scheduling connected",value["cached"] as? Bool == true ? "Saved on this device" : "Up to date"].joined(separator:" · ")
        rows.addArrangedSubview(text("Meeting links",size:14,strong:true))
        let types = value["meeting_types"] as? [[String:Any]] ?? []
        for type in types where type["published"] as? Bool == true {
            let row = NSStackView(); row.spacing = 12
            row.addArrangedSubview(text(type["title"] as? String ?? "Meeting",strong:true))
            let id = type["id"] as? String ?? ""
            row.addArrangedSubview(button(insert == nil ? "Copy link" : "Insert link") { [weak self] in
                guard let self else { return }; request(["op":"booking_link","meeting_type_id":id]) { [weak self] data in
                    guard let self, let url = data["url"] as? String else { return }
                    if let insert { insert(url); close() } else { NSPasteboard.general.clearContents(); NSPasteboard.general.setString(url,forType:.string); notice.stringValue = "Booking link copied" }
                }
            }); if insert != nil { row.addArrangedSubview(button("Suggest times") { [weak self] in self?.suggestTimes(id) }) }; rows.addArrangedSubview(row)
        }
        if !types.contains(where: { $0["published"] as? Bool == true }) { rows.addArrangedSubview(text("No active meeting links. Create one in Manage online.")) }
        if insert != nil { return }
        rows.addArrangedSubview(text("Upcoming bookings",size:14,strong:true))
        let now = Date().timeIntervalSince1970 * 1000
        let bookings = (value["bookings"] as? [[String:Any]] ?? []).filter { ($0["status"] as? String) == "confirmed" && ($0["end"] as? Double ?? 0) > now }.sorted { ($0["start"] as? Double ?? 0) < ($1["start"] as? Double ?? 0) }
        if bookings.isEmpty { rows.addArrangedSubview(text("No upcoming bookings")) }
        let date = DateFormatter(); date.dateStyle = .medium; date.timeStyle = .short
        for booking in bookings.prefix(30) {
            rows.addArrangedSubview(text(booking["title"] as? String ?? "Meeting",strong:true))
            let when = date.string(from:Date(timeIntervalSince1970:(booking["start"] as? Double ?? 0)/1000))
            rows.addArrangedSubview(text(when + " · " + (booking["name"] as? String ?? "Guest")))
            if let email = booking["email"] as? String {
                let actions = NSStackView(); actions.spacing = 10
                if let id = booking["id"] as? String, let encoded = id.addingPercentEncoding(withAllowedCharacters:.urlFragmentAllowed), let url = DistributionProfile.current?.schedulingURL(path: "/studio", fragment: "booking=" + encoded) { actions.addArrangedSubview(button("Manage booking ↗") { NSWorkspace.shared.open(url) }) }
                if let emailGuest { actions.addArrangedSubview(button("Email guest") { [weak self] in emailGuest(email,host["email"] as? String); self?.close() }) }
                if let findMail { actions.addArrangedSubview(button("View conversation") { [weak self] in findMail(email); self?.close() }) }
                rows.addArrangedSubview(actions)
            }
        }
        if let analytics = value["analytics"] as? [String:Any], !analytics.isEmpty {
            rows.addArrangedSubview(text("Analytics · last 30 days",size:14,strong:true))
            let totals = analytics["totals"] as? [String:Any] ?? [:]
            rows.addArrangedSubview(text("\(totals["pageViews"] as? Int ?? 0) page visits · \(totals["confirmed"] as? Int ?? 0) bookings · \(totals["cancelled"] as? Int ?? 0) cancellations"))
            if let analytics = DistributionProfile.current?.schedulingURL(path: "/studio", fragment: "analytics") { rows.addArrangedSubview(button("Open analytics ↗") { NSWorkspace.shared.open(analytics) }) }
        }
    }
    private func suggestTimes(_ id:String) {
        generation += 1
        notice.stringValue = "Checking current availability…"
        request(["op":"booking_availability","meeting_type_id":id]) { [weak self] result in
            guard let self else { return }; clear()
            notice.stringValue = "Choose up to five times. Availability is checked again when someone books."
            let zone = result["time_zone"] as? String ?? TimeZone.current.identifier
            rows.addArrangedSubview(text("Suggested times · " + zone,size:14,strong:true))
            var choices:[(NSButton,String)] = []
            let date = DateFormatter(); date.dateFormat = "EEE, d MMM · HH:mm"; date.timeZone = TimeZone(identifier:zone)
            for slot in (result["slots"] as? [[String:Any]] ?? []).prefix(12) {
                guard let start = slot["start"] as? Double,let url = slot["url"] as? String else { continue }
                let title = date.string(from:Date(timeIntervalSince1970:start/1000))
                let check = NSButton(checkboxWithTitle:title,target:nil,action:nil); rows.addArrangedSubview(check); choices.append((check,title + " — " + url))
            }
            if choices.isEmpty { rows.addArrangedSubview(text("No available times. Change your availability in Manage online.")) }
            rows.addArrangedSubview(button("Insert selected times") { [weak self] in
                guard let self else { return }; let selected = choices.filter { $0.0.state == .on }.map { $0.1 }
                guard !selected.isEmpty && selected.count <= 5 else { notice.stringValue = "Select between one and five times."; return }
                insert?(selected.joined(separator:"\n") + "\n" + zone); close()
            })
            rows.addArrangedSubview(button("Back to meeting links") { [weak self] in self?.load(refresh:false) })
        }
    }
    private func connect() {
        generation += 1; pending = false; poll?.invalidate(); poll = nil
        notice.stringValue = "Opening your browser…"
        request(["op":"booking_connect_start","name":"Email on \(Host.current().localizedName ?? "Mac")"]) { [weak self] data in
            guard let self, let code = data["user_code"] as? String,let raw = data["verification_url"] as? String,let url = URL(string:raw),DistributionProfile.current?.isSchedulingURL(url) == true else { return }
            clear(); layoutConnection(true); titleLabel?.stringValue = "Finish connecting in your browser"
            notice.stringValue = "Waiting for approval…"
            let progress = NSProgressIndicator(); progress.style = .spinning; progress.controlSize = .small; progress.startAnimation(nil)
            let waiting = NSStackView(views:[progress,text("Email will connect automatically once you approve.")]); waiting.spacing = 10; rows.addArrangedSubview(waiting)
            rows.addArrangedSubview(button("Reopen browser") { NSWorkspace.shared.open(url) })
            let fallback = NSStackView(); fallback.orientation = .vertical; fallback.alignment = .leading; fallback.spacing = 6; fallback.isHidden = true
            fallback.addArrangedSubview(text("If the browser asks for a code",size:11)); fallback.addArrangedSubview(text(code,size:16,strong:true))
            rows.addArrangedSubview(button("Connection help") { fallback.isHidden.toggle() }); rows.addArrangedSubview(fallback)
            pending = true; pollDeadline = Date().addingTimeInterval(data["expires_in"] as? Double ?? 600); NSWorkspace.shared.open(url); poll?.invalidate()
            poll = Timer.scheduledTimer(withTimeInterval:5,repeats:true) { [weak self] _ in Task { @MainActor in self?.checkConnection() } }
        }
    }
    private func checkConnection() {
        guard pending else { return }
        guard Date() < pollDeadline else { pending = false; poll?.invalidate(); poll = nil; load(refresh:false); notice.stringValue = "Connection expired. Continue in browser to try again."; return }
        request(["op":"booking_connect_poll"]) { [weak self] result in
            guard let self else { return }; if result["connected"] as? Bool == true { pending = false; poll?.invalidate(); poll = nil; NSApp.activate(ignoringOtherApps:true); window?.makeKeyAndOrderFront(nil); load(refresh:true) }
        }
    }
    private func disconnect() { pending = false; poll?.invalidate(); request(["op":"booking_disconnect"]) { [weak self] _ in self?.load(refresh:false) } }
}

@MainActor private final class BookingAction: NSButton {
    private let run: () -> Void
    init(title:String,run:@escaping () -> Void) { self.run = run; super.init(frame:.zero); self.title = title; target = self; action = #selector(invoke) }
    required init?(coder:NSCoder) { fatalError() }
    @objc private func invoke() { run() }
}

private final class BookingDocumentView: NSView { override var isFlipped: Bool { true } }

@MainActor private final class BookingWindow: NSWindow { override func cancelOperation(_ sender: Any?) { performClose(sender) } }
