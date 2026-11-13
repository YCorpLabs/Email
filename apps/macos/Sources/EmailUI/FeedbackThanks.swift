import AppKit

/// A small, click-through thank-you. It never takes focus or covers Undo send.
@MainActor
final class FeedbackThanks: NSView {
    private let started = Date.timeIntervalSinceReferenceDate
    private let reducedMotion = NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
    private let card = FeedbackThanksCard()
    private var bottom: NSLayoutConstraint?

    static func show(in host: NSView) {
        host.subviews.compactMap { $0 as? FeedbackThanks }.forEach { $0.removeFromSuperview() }
        let view = FeedbackThanks(frame: host.bounds)
        view.autoresizingMask = [.width, .height]
        host.addSubview(view)
        view.card.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(view.card)
        let bottom = view.card.bottomAnchor.constraint(equalTo: view.bottomAnchor, constant: -40)
        view.bottom = bottom
        let preferredWidth = view.card.widthAnchor.constraint(equalToConstant: 320)
        preferredWidth.priority = .defaultHigh
        NSLayoutConstraint.activate([
            view.card.centerXAnchor.constraint(equalTo: view.centerXAnchor), bottom, preferredWidth,
            view.card.widthAnchor.constraint(lessThanOrEqualTo: view.widthAnchor, constant: -32)
        ])
        view.update()
        Task { @MainActor [weak view] in
            for _ in 0..<100 {
                try? await Task.sleep(for: .milliseconds(32))
                guard let view, view.superview != nil else { return }
                view.update()
            }
            view?.removeFromSuperview()
        }
    }

    private func update() {
        let t = Date.timeIntervalSinceReferenceDate - started
        if !reducedMotion {
            let entrance = min(1, t / 0.22)
            let eased = 1 - pow(1 - entrance, 3)
            card.alphaValue = min(eased, max(0, (3.2 - t) / 0.35))
            bottom?.constant = -40 + 8 * (1 - eased)
            needsDisplay = true
        }
    }

    override func hitTest(_ point: NSPoint) -> NSView? { nil }
    override var isFlipped: Bool { true }

    override func draw(_ dirtyRect: NSRect) {
        guard !reducedMotion else { return }
        let t = Date.timeIntervalSinceReferenceDate - started
        guard t < 2.1 else { return }
        let colors: [NSColor] = [.systemPurple, .systemPink, .systemYellow, .systemTeal, .systemBlue]
        let spread = min(180, bounds.width * 0.28)
        for index in 0..<36 {
            let fraction = Double(index) / 35
            let x = bounds.midX + (fraction - 0.5) * spread * 2 * t
            let y = bounds.height - 72 - (160 + Double((index * 23) % 110)) * t + 115 * t * t
            colors[index % colors.count].withAlphaComponent(min(1, t / 0.08) * max(0, 1 - t / 2.1)).setFill()
            let path = NSBezierPath(rect: NSRect(x: -2, y: -3.5, width: 4, height: 7))
            let translation = AffineTransform(translationByX: x, byY: y)
            var spin = AffineTransform(rotationByDegrees: Double(index * 31) + t * 130)
            spin.append(translation)
            path.transform(using: spin)
            path.fill()
        }
    }
}

@MainActor
private final class FeedbackThanksCard: NSView {
    override init(frame: NSRect) {
        super.init(frame: frame)
        let heart = NSTextField(labelWithString: "♥")
        heart.font = .systemFont(ofSize: 20, weight: .medium)
        heart.textColor = .systemPurple
        let title = NSTextField(wrappingLabelWithString: "Thanks for the feedback!")
        title.font = .systemFont(ofSize: 13, weight: .semibold)
        title.textColor = EmailTheme.ink
        let row = NSStackView(views: [heart, title])
        row.alignment = .centerY; row.spacing = 12
        row.translatesAutoresizingMaskIntoConstraints = false
        addSubview(row)
        NSLayoutConstraint.activate([
            row.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 18),
            row.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -18),
            row.topAnchor.constraint(equalTo: topAnchor, constant: 14),
            row.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -14),
            heart.widthAnchor.constraint(equalToConstant: 22)
        ])
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    override func viewDidChangeEffectiveAppearance() { super.viewDidChangeEffectiveAppearance(); needsDisplay = true }
    override func draw(_ dirtyRect: NSRect) {
        let shape = NSBezierPath(roundedRect: bounds.insetBy(dx: 0.5, dy: 0.5), xRadius: 12, yRadius: 12)
        EmailTheme.inbox.setFill(); shape.fill()
        EmailTheme.line.setStroke(); shape.lineWidth = 1; shape.stroke()
    }
}
