import AppKit

/// Message activity embedded beneath the selected contact, without duplicating reader context.
@MainActor final class OpenActivitySummary: NSStackView {
    var onRefresh: (() -> Void)?
    private let entries = NSStackView()
    private let refresh = NSButton()

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        orientation = .vertical; alignment = .leading; spacing = 10
        let title = NSTextField(labelWithString: "Open activity")
        title.font = .systemFont(ofSize: 13, weight: .semibold); title.textColor = EmailTheme.ink
        let info = NSButton(image: NSImage(systemSymbolName: "info.circle", accessibilityDescription: "About open tracking")!, target: nil, action: nil)
        info.isBordered = false; info.toolTip = "Privacy protection and mail scanners can affect open tracking. Opens in the same minute are grouped. Recipients are not identified."
        info.setAccessibilityHelp(info.toolTip)
        refresh.image = NSImage(systemSymbolName: "arrow.clockwise", accessibilityDescription: "Refresh open activity")
        refresh.imagePosition = .imageOnly; refresh.isBordered = false
        refresh.toolTip = "Refresh open activity"; refresh.target = self; refresh.action = #selector(refreshClicked)
        let header = NSStackView(views: [title, NSView(), info, refresh])
        header.spacing = 8; addArrangedSubview(header)
        header.widthAnchor.constraint(equalTo: widthAnchor).isActive = true
        entries.orientation = .vertical; entries.alignment = .leading; entries.spacing = 8
        addArrangedSubview(entries); entries.widthAnchor.constraint(equalTo: widthAnchor).isActive = true
        isHidden = true
    }
    required init?(coder: NSCoder) { fatalError() }
    @objc private func refreshClicked() { onRefresh?() }
    func focusRefresh() { window?.makeFirstResponder(refresh) }
    func show(_ item: [String: Any]?) {
        entries.arrangedSubviews.forEach { $0.removeFromSuperview() }
        isHidden = item == nil
        guard let item else { return }
        let count = (item["pixel_requests"] as? NSNumber)?.intValue ?? 0
        addLine(count == 0 ? "No opens recorded" : count == 1 ? "Opened once" : "Opened \(count) times", prominent: true)
        if let timing = TrackingActivityView.timing(item) { addLine(timing) }
        addLine(item["reply_received"] as? Bool == true ? "Reply received" : "No reply in downloaded mail")
        if item["tracking_status"] as? String == "expired" { addLine("Tracking expired") }
        let events = (item["timeline"] as? [[String: Any]] ?? []).filter { $0["kind"] as? String == "image_request" }
            .sorted { (($0["at_ms"] as? NSNumber)?.doubleValue ?? 0) > (($1["at_ms"] as? NSNumber)?.doubleValue ?? 0) }
        for event in events {
            guard let time = (event["at_ms"] as? NSNumber)?.doubleValue else { continue }
            let times = (event["count"] as? NSNumber)?.intValue ?? 1
            addLine("\(times > 1 ? "Opened \(times) times" : "Opened")\n\(TrackingActivityView.exact(time))")
        }
        if item["timeline_complete"] as? Bool != true { addLine("Earlier opens may only have saved totals.") }
    }
    private func addLine(_ text: String, prominent: Bool = false) {
        let label = NSTextField(wrappingLabelWithString: text)
        label.font = .systemFont(ofSize: prominent ? 16 : 11, weight: prominent ? .semibold : .regular)
        label.textColor = prominent ? EmailTheme.ink : EmailTheme.muted
        label.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        entries.addArrangedSubview(label); label.widthAnchor.constraint(equalTo: entries.widthAnchor).isActive = true
    }
}
