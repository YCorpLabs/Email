import AppKit

/// "Continue with Google", drawn to Google's sign-in button branding
/// guidelines: the standard four-colour "G" (unchanged, 18 pt), 40 pt tall,
/// 12 pt before the logo and 10 pt after it, a 4 pt corner radius, the light
/// theme (white fill, #747775 outline, #1F1F1F text) or the dark theme
/// (#131314 fill, #8E918F outline, #E3E3E3 text) following the appearance.
/// The text is Roboto Medium 14 as the guidelines ask when it is installed,
/// otherwise the system font at medium weight (macOS does not ship Roboto).
/// Linux draws the same button (`account_setup.rs`, `google_button`).
@MainActor
final class GoogleSignInButton: NSButton {
    static let title = "Continue with Google"
    static let height: CGFloat = 40

    private var hovering = false

    init() {
        super.init(frame: .zero)
        title = Self.title
        isBordered = false
        bezelStyle = .regularSquare
        setButtonType(.momentaryChange)
        focusRingType = .default
        setAccessibilityLabel(Self.title)
        heightAnchor.constraint(equalToConstant: Self.height).isActive = true
    }

    required init?(coder: NSCoder) { fatalError() }

    override var intrinsicContentSize: NSSize {
        let text = (Self.title as NSString).size(withAttributes: [.font: Self.font])
        return NSSize(width: ceil(12 + 18 + 10 + text.width + 12), height: Self.height)
    }

    private static let font = NSFont(name: "Roboto-Medium", size: 14) ?? .systemFont(ofSize: 14, weight: .medium)

    private static func color(_ hex: UInt32, alpha: CGFloat = 1) -> NSColor {
        NSColor(srgbRed: CGFloat((hex >> 16) & 0xff) / 255, green: CGFloat((hex >> 8) & 0xff) / 255,
                blue: CGFloat(hex & 0xff) / 255, alpha: alpha)
    }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        trackingAreas.forEach(removeTrackingArea)
        addTrackingArea(NSTrackingArea(rect: bounds, options: [.activeInKeyWindow, .mouseEnteredAndExited, .inVisibleRect], owner: self, userInfo: nil))
    }
    override func mouseEntered(with event: NSEvent) { hovering = true; needsDisplay = true }
    override func mouseExited(with event: NSEvent) { hovering = false; needsDisplay = true }
    override func viewDidChangeEffectiveAppearance() { super.viewDidChangeEffectiveAppearance(); needsDisplay = true }

    override func draw(_ dirtyRect: NSRect) {
        let dark = EmailTheme.isDark(effectiveAppearance)
        let fill = Self.color(dark ? 0x131314 : 0xFFFFFF)
        let stroke = Self.color(dark ? 0x8E918F : 0x747775)
        let ink = Self.color(dark ? 0xE3E3E3 : 0x1F1F1F)
        let shape = NSBezierPath(roundedRect: bounds.insetBy(dx: 0.5, dy: 0.5), xRadius: 4, yRadius: 4)
        fill.setFill()
        shape.fill()
        // Google's state layers: the text colour at 8 % on hover, 12 % pressed.
        if isHighlighted || hovering {
            ink.withAlphaComponent(isHighlighted ? 0.12 : 0.08).setFill()
            shape.fill()
        }
        stroke.setStroke()
        shape.lineWidth = 1
        shape.stroke()
        let logo = NSRect(x: 12, y: (bounds.height - 18) / 2, width: 18, height: 18)
        GoogleLogo.draw(in: logo, flipped: isFlipped)
        let attributes: [NSAttributedString.Key: Any] = [.font: Self.font, .foregroundColor: ink]
        let size = (Self.title as NSString).size(withAttributes: attributes)
        // Text centred in the button, never under the logo (as Google's own button).
        let x = max(logo.maxX + 10, (bounds.width - size.width) / 2)
        (Self.title as NSString).draw(at: NSPoint(x: x, y: (bounds.height - size.height) / 2), withAttributes: attributes)
    }
}

/// Google's standard "G" mark from the sign-in branding kit (48×48 artboard).
enum GoogleLogo {
    private static let paths: [(String, UInt32)] = [
        ("M24 9.5c3.54 0 6.71 1.22 9.21 3.6l6.85-6.85C35.9 2.38 30.47 0 24 0 14.62 0 6.51 5.38 2.56 13.22l7.98 6.19C12.43 13.72 17.74 9.5 24 9.5z", 0xEA4335),
        ("M46.98 24.55c0-1.57-.15-3.09-.38-4.55H24v9.02h12.94c-.58 2.96-2.26 5.48-4.78 7.18l7.73 6c4.51-4.18 7.09-10.36 7.09-17.65z", 0x4285F4),
        ("M10.53 28.59c-.48-1.45-.76-2.99-.76-4.59s.27-3.14.76-4.59l-7.98-6.19C.92 16.46 0 20.12 0 24c0 3.88.92 7.54 2.56 10.78l7.97-6.19z", 0xFBBC05),
        ("M24 48c6.48 0 11.93-2.13 15.89-5.81l-7.73-6c-2.15 1.45-4.92 2.3-8.16 2.3-6.26 0-11.57-4.22-13.47-9.91l-7.98 6.19C6.51 42.62 14.62 48 24 48z", 0x34A853),
    ]

    static func draw(in rect: NSRect, flipped: Bool) {
        let scale = rect.width / 48
        // SVG is y-down: x' = s·x + minX; y' = ±s·y + (minY or maxY).
        let transform = AffineTransform(m11: scale, m12: 0, m21: 0, m22: flipped ? scale : -scale,
                                        tX: rect.minX, tY: flipped ? rect.minY : rect.maxY)
        for (data, hex) in paths {
            let path = bezier(data)
            path.transform(using: transform)
            NSColor(srgbRed: CGFloat((hex >> 16) & 0xff) / 255, green: CGFloat((hex >> 8) & 0xff) / 255,
                    blue: CGFloat(hex & 0xff) / 255, alpha: 1).setFill()
            path.fill()
        }
    }

    /// The SVG path commands the mark uses: M L H V C S Z, absolute and relative.
    static func bezier(_ data: String) -> NSBezierPath {
        let path = NSBezierPath()
        var scanner = Array(data.unicodeScalars)[...]
        var command: Character = "M"
        var current = NSPoint.zero, start = NSPoint.zero, control: NSPoint?
        func number() -> CGFloat? {
            while let first = scanner.first, first == " " || first == "," { scanner.removeFirst() }
            var text = ""
            if let first = scanner.first, first == "-" || first == "+" { text.append(Character(first)); scanner.removeFirst() }
            var dot = false
            while let first = scanner.first, first.properties.numericType != nil || (first == "." && !dot) {
                if first == "." { dot = true }
                text.append(Character(first)); scanner.removeFirst()
            }
            return Double(text).map { CGFloat($0) }
        }
        while !scanner.isEmpty {
            let remaining = scanner.count
            defer { if scanner.count == remaining { scanner.removeAll() } } // never loop on bad data
            while let first = scanner.first, first == " " || first == "," { scanner.removeFirst() }
            if let first = scanner.first, CharacterSet.letters.contains(first) {
                command = Character(first); scanner.removeFirst()
            }
            let relative = command.isLowercase
            let origin = relative ? current : .zero
            switch command.uppercased() {
            case "M":
                guard let x = number(), let y = number() else { return path }
                current = NSPoint(x: origin.x + x, y: origin.y + y); start = current
                path.move(to: current)
                command = relative ? "l" : "L"; control = nil
            case "L":
                guard let x = number(), let y = number() else { return path }
                current = NSPoint(x: origin.x + x, y: origin.y + y)
                path.line(to: current); control = nil
            case "H":
                guard let x = number() else { return path }
                current.x = origin.x + x
                path.line(to: current); control = nil
            case "V":
                guard let y = number() else { return path }
                current.y = origin.y + y
                path.line(to: current); control = nil
            case "C":
                guard let x1 = number(), let y1 = number(), let x2 = number(), let y2 = number(), let x = number(), let y = number() else { return path }
                let second = NSPoint(x: origin.x + x2, y: origin.y + y2)
                let end = NSPoint(x: origin.x + x, y: origin.y + y)
                path.curve(to: end, controlPoint1: NSPoint(x: origin.x + x1, y: origin.y + y1), controlPoint2: second)
                control = second; current = end
            case "S":
                guard let x2 = number(), let y2 = number(), let x = number(), let y = number() else { return path }
                let first = control.map { NSPoint(x: 2 * current.x - $0.x, y: 2 * current.y - $0.y) } ?? current
                let second = NSPoint(x: origin.x + x2, y: origin.y + y2)
                let end = NSPoint(x: origin.x + x, y: origin.y + y)
                path.curve(to: end, controlPoint1: first, controlPoint2: second)
                control = second; current = end
            case "Z":
                path.close(); current = start; control = nil
            default:
                return path
            }
        }
        return path
    }
}
