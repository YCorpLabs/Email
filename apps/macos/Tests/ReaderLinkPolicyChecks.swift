import Foundation

@main enum ReaderLinkPolicyChecks {
    static func main() {
        func decide(_ value: String?, link: Bool = false, other: Bool = false) -> ReaderLinkPolicy.Decision {
            ReaderLinkPolicy.decide(url: value.flatMap(URL.init(string:)), linkActivated: link, programmaticLoad: other)
        }
        func opens(_ value: String) -> Bool { decide(value, link: true) == .openExternally(URL(string: value)!) }

        // The app's own loadHTMLString document (about:blank) still loads.
        precondition(decide("about:blank", other: true) == .allow)
        // Clicked links open in the default handler, and only these schemes.
        precondition(opens("https://example.com/path?q=1#frag"))
        precondition(opens("http://example.com/"))
        precondition(opens("HTTPS://EXAMPLE.COM/"))
        precondition(opens("mailto:someone@example.com?subject=Hi"))
        for blocked in ["javascript:alert(1)", "file:///etc/passwd", "data:text/html,hi", "ftp://example.com/",
                        "tel:+4712345678", "cid:logo@example", "x-apple-helpviewer://x", "https:", "http:///path"] {
            precondition(decide(blocked, link: true) == .cancel, blocked)
        }
        // Navigations the user did not start never open anything: redirects,
        // meta refresh, form or back/forward reloads, and anything WebKit reports as other.
        for value in ["https://example.com/", "mailto:someone@example.com"] {
            precondition(decide(value, other: true) == .cancel, value)
            precondition(decide(value) == .cancel, value)
        }
        // A clicked about: link (an in-page #anchor) stays cancelled, as before this change.
        precondition(decide("about:blank#section", link: true) == .cancel)
        precondition(decide(nil, link: true) == .cancel)
        precondition(decide(nil, other: true) == .cancel)

        // mailto keeps only the recipient and to, cc, bcc, subject and body.
        // Keep this table identical to MAILTO_CASES in apps/linux/src/reader_links.rs.
        let mailtoCases: [(String, String?)] = [
            ("mailto:a@example.com", "mailto:a@example.com"),
            ("MAILTO:a@example.com", "mailto:a@example.com"),
            ("mailto:a@example.com?subject=Hi&attach=%2Fetc%2Fpasswd&body=Line%0D%0Atwo&X-Foo=1&attachment=x&CC=c@example.com", "mailto:a@example.com?subject=Hi&body=Line%0D%0Atwo&cc=c@example.com"),
            ("mailto:a@example.com?Attach=/etc/passwd", "mailto:a@example.com"),
            ("mailto:a@example.com?%61ttach=x&%73ubject=Hi", "mailto:a@example.com?subject=Hi"),
            ("mailto:a@example.com,b@example.com?to=c@example.com&to=d@example.com&bcc=e@example.com", "mailto:a@example.com,b@example.com?to=c@example.com&to=d@example.com&bcc=e@example.com"),
            ("mailto:?to=a@example.com&subject=Hi", "mailto:?to=a@example.com&subject=Hi"),
            ("mailto:a@example.com?to=&subject=&body=", "mailto:a@example.com"),
            ("mailto:a@example.com?subject=a+b%20c#frag", "mailto:a@example.com?subject=a%2Bb%20c"),
            ("mailto:a@example.com?subject", "mailto:a@example.com"),
            ("mailto:a@example.com?&&subject=Hi&", "mailto:a@example.com?subject=Hi"),
            ("mailto:a@example.com?body=x%3Dy&subject=a=b", "mailto:a@example.com?body=x%3Dy&subject=a%3Db"),
            ("mailto:a@example.com?subject=x?attach=/etc/passwd", "mailto:a@example.com?subject=x%3Fattach%3D%2Fetc%2Fpasswd"),
            ("mailto:a@example.com&attach=/etc/passwd", "mailto:a@example.com%26attach%3D%2Fetc%2Fpasswd"),
            ("mailto:J%C3%B8rn@example.com?subject=Gr%C3%BC%C3%9Fe%20%26%20more", "mailto:J%C3%B8rn@example.com?subject=Gr%C3%BC%C3%9Fe%20%26%20more"),
            ("mailto:a@example.com?subject=two words", "mailto:a@example.com?subject=two%20words"),
            ("mailto:a@example.com?subject=%EF%BB%BFHi", "mailto:a@example.com?subject=%EF%BB%BFHi"),
            ("mailto:%E2%80%8B", "mailto:%E2%80%8B"),
            ("mailto:a@example.com?body=Tab%09ok", "mailto:a@example.com?body=Tab%09ok"),
            ("mailto:", nil),
            ("mailto:?subject=Hi&body=x", nil),
            ("mailto:%20?subject=Hi", nil),
            ("mailto:?to=%20", nil),
            ("mailto:%E2%80%83", nil),
            ("mailto:%zz", nil),
            ("mailto:a@example.com?subject=%E2%28", nil),
            ("mailto:a@example.com?x=%zz", nil),
            ("mailto:a@example.com%0D%0ABcc:x@evil.example", nil),
            ("mailto:a@example.com?subject=Hi%0ABcc:%20x@evil.example", nil),
            ("mailto:a@example.com?cc=x%00@evil.example", nil),
            ("mailto:a@example.com?body=a%00b", nil),
            ("https://example.com/", nil),
            ("mailto", nil),
            ("mail:a@example.com", nil),
        ]
        for (input, expected) in mailtoCases {
            precondition(ReaderLinkPolicy.sanitizedMailto(input)?.absoluteString == expected, input)
        }
        precondition(decide("mailto:a@example.com?attach=/etc/passwd&subject=Hi", link: true)
                     == .openExternally(URL(string: "mailto:a@example.com?subject=Hi")!))
        precondition(decide("mailto:?subject=Hi", link: true) == .cancel)
        precondition(decide("mailto:", link: true) == .cancel)
        print("ReaderLinkPolicyChecks passed (\(mailtoCases.count) mailto cases)")
    }
}
