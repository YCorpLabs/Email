import Foundation

/// Navigation policy for the message reader's web view.
///
/// Mirrors `new_html_reader` and `reader_links.rs` in apps/linux/src: the reader
/// document never navigates away from the message, and only a link the user
/// activated is handed to the system default handler, and only for http, https
/// or a sanitized mailto. Redirects, meta refresh and any other navigation the
/// user did not start never open anything.
enum ReaderLinkPolicy {
    enum Decision: Equatable {
        /// Let the reader load it (the app's own `loadHTMLString` document).
        case allow
        /// Drop the navigation without opening anything.
        case cancel
        /// Drop the navigation in the reader and open the URL in the default app.
        case openExternally(URL)
    }

    static let externalSchemes: Set<String> = ["http", "https", "mailto"]
    /// The only mailto fields passed to the mail app. Anything else, such as
    /// `attach`, is dropped: some mail clients attached local files from links.
    static let mailtoFields: Set<String> = ["to", "cc", "bcc", "subject", "body"]

    /// - Parameters:
    ///   - url: The navigation request's URL.
    ///   - linkActivated: The user clicked a link (`WKNavigationType.linkActivated`),
    ///     including links that target a new window.
    ///   - programmaticLoad: The navigation was not user-initiated
    ///     (`WKNavigationType.other`), as for the app's own document load.
    static func decide(url: URL?, linkActivated: Bool, programmaticLoad: Bool) -> Decision {
        if programmaticLoad && url?.scheme == "about" { return .allow }
        guard linkActivated, let url, let external = externalURL(url) else { return .cancel }
        return .openExternally(external)
    }

    /// The URL to hand to the system for a message link, or nil to open nothing:
    /// http and https with a host unchanged, mailto rebuilt by `sanitizedMailto`.
    static func externalURL(_ url: URL) -> URL? {
        guard let scheme = url.scheme?.lowercased(), externalSchemes.contains(scheme) else { return nil }
        if scheme == "mailto" { return sanitizedMailto(url.absoluteString) }
        return (url.host ?? "").isEmpty ? nil : url
    }

    /// Rebuilds a mailto link from its recipient and the `to`, `cc`, `bcc`,
    /// `subject` and `body` fields only. Values are decoded and re-encoded so a
    /// literal `?`, `&` or `=` inside one can't be read as another field.
    /// Returns nil, so nothing opens, when the link has no recipient, invalid
    /// percent encoding or UTF-8, or a control character (CR, LF and tab are
    /// allowed in `body`). Same rules and test table as `sanitized_mailto` in
    /// apps/linux/src/reader_links.rs.
    static func sanitizedMailto(_ value: String) -> URL? {
        // Split on ASCII bytes, as the Rust copy does, so both cut at the same places.
        let bytes = Array(value.utf8)
        guard bytes.count >= 7, String(decoding: bytes[0..<7], as: UTF8.self).lowercased() == "mailto:" else { return nil }
        var link = bytes[7...]
        if let hash = link.firstIndex(of: UInt8(ascii: "#")) { link = link[..<hash] }
        let question = link.firstIndex(of: UInt8(ascii: "?"))
        let rawQuery = question.map { link[link.index(after: $0)...] } ?? []
        guard let recipient = String(decoding: link[..<(question ?? link.endIndex)], as: UTF8.self).removingPercentEncoding,
              !hasControl(recipient, allowingLineBreaks: false) else { return nil }
        var hasRecipient = !isBlank(recipient)
        var kept: [String] = []
        for pair in rawQuery.split(separator: UInt8(ascii: "&")) {
            let equals = pair.firstIndex(of: UInt8(ascii: "="))
            let rawValue = equals.map { String(decoding: pair[pair.index(after: $0)...], as: UTF8.self) } ?? ""
            guard let name = String(decoding: pair[..<(equals ?? pair.endIndex)], as: UTF8.self).removingPercentEncoding?.lowercased(),
                  let field = rawValue.removingPercentEncoding else { return nil }
            guard mailtoFields.contains(name), !field.isEmpty else { continue }
            if hasControl(field, allowingLineBreaks: name == "body") { return nil }
            if name != "subject" && name != "body" && !isBlank(field) { hasRecipient = true }
            kept.append(name + "=" + mailtoEncoded(field))
        }
        guard hasRecipient else { return nil }
        return URL(string: "mailto:" + mailtoEncoded(recipient) + (kept.isEmpty ? "" : "?" + kept.joined(separator: "&")))
    }

    /// Percent-encodes every UTF-8 byte except ASCII letters, digits and `-._~@,`.
    private static func mailtoEncoded(_ value: String) -> String {
        var result = ""
        for byte in value.utf8 {
            switch byte {
            case UInt8(ascii: "A")...UInt8(ascii: "Z"), UInt8(ascii: "a")...UInt8(ascii: "z"), UInt8(ascii: "0")...UInt8(ascii: "9"),
                 UInt8(ascii: "-"), UInt8(ascii: "."), UInt8(ascii: "_"), UInt8(ascii: "~"), UInt8(ascii: "@"), UInt8(ascii: ","):
                result.unicodeScalars.append(Unicode.Scalar(byte))
            default:
                result += String(format: "%%%02X", byte)
            }
        }
        return result
    }

    private static func hasControl(_ value: String, allowingLineBreaks: Bool) -> Bool {
        value.unicodeScalars.contains {
            $0.properties.generalCategory == .control && !(allowingLineBreaks && ["\r", "\n", "\t"].contains($0))
        }
    }

    /// Empty or only Unicode White_Space, the same test as Rust's `char::is_whitespace`.
    private static func isBlank(_ value: String) -> Bool {
        value.unicodeScalars.allSatisfy { $0.properties.isWhitespace }
    }
}
