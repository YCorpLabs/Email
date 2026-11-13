//! Which message links may be handed to the desktop's default handler.
//!
//! Mirrors `ReaderLinkPolicy` in apps/macos/Sources/EmailUI/ReaderLinkPolicy.swift:
//! http and https pass unchanged, and mailto is rebuilt from its recipient and
//! the `to`, `cc`, `bcc`, `subject` and `body` fields only. Anything else, such
//! as `attach`, is dropped: some mail clients attached local files from links.

use url::Url;

const MAILTO_FIELDS: [&str; 5] = ["to", "cc", "bcc", "subject", "body"];

/// The URI to launch for a message link, or `None` to open nothing.
pub fn external_uri(uri: &str) -> Option<String> {
    match Url::parse(uri).ok()?.scheme() {
        "http" | "https" => Some(uri.to_string()),
        "mailto" => sanitized_mailto(uri),
        _ => None,
    }
}

/// Rebuilds a mailto link from its recipient and the allowed fields. Values are
/// decoded and re-encoded so a literal `?`, `&` or `=` inside one can't be read
/// as another field. Returns `None` when the link has no recipient, invalid
/// percent encoding or UTF-8, or a control character (CR, LF and tab are
/// allowed in `body`). Same rules and test table as
/// `ReaderLinkPolicy.sanitizedMailto` on macOS.
pub fn sanitized_mailto(value: &str) -> Option<String> {
    if !value.get(..7)?.eq_ignore_ascii_case("mailto:") {
        return None;
    }
    let link = value[7..].split('#').next().unwrap_or_default();
    let (raw_recipient, raw_query) = link.split_once('?').unwrap_or((link, ""));
    let recipient = percent_decoded(raw_recipient)?;
    if has_control(&recipient, false) {
        return None;
    }
    let mut has_recipient = !is_blank(&recipient);
    let mut kept = Vec::new();
    for pair in raw_query.split('&').filter(|pair| !pair.is_empty()) {
        let (raw_name, raw_value) = pair.split_once('=').unwrap_or((pair, ""));
        let name = percent_decoded(raw_name)?.to_lowercase();
        let field = percent_decoded(raw_value)?;
        if !MAILTO_FIELDS.contains(&name.as_str()) || field.is_empty() {
            continue;
        }
        if has_control(&field, name == "body") {
            return None;
        }
        if name != "subject" && name != "body" && !is_blank(&field) {
            has_recipient = true;
        }
        kept.push(format!("{name}={}", mailto_encoded(&field)));
    }
    if !has_recipient {
        return None;
    }
    let query = if kept.is_empty() {
        String::new()
    } else {
        format!("?{}", kept.join("&"))
    };
    Some(format!("mailto:{}{query}", mailto_encoded(&recipient)))
}

/// Strict percent-decoding: every `%` must start two hex digits and the bytes
/// must be UTF-8. A `+` stays a plus sign, as mailto has no form encoding.
fn percent_decoded(value: &str) -> Option<String> {
    let mut bytes = Vec::with_capacity(value.len());
    let mut input = value.bytes();
    while let Some(byte) = input.next() {
        if byte != b'%' {
            bytes.push(byte);
            continue;
        }
        let high = (input.next()? as char).to_digit(16)?;
        let low = (input.next()? as char).to_digit(16)?;
        bytes.push((high * 16 + low) as u8);
    }
    String::from_utf8(bytes).ok()
}

/// Percent-encodes every UTF-8 byte except ASCII letters, digits and `-._~@,`.
fn mailto_encoded(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~@,".contains(&byte) {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn has_control(value: &str, allowing_line_breaks: bool) -> bool {
    value
        .chars()
        .any(|c| c.is_control() && !(allowing_line_breaks && matches!(c, '\r' | '\n' | '\t')))
}

/// Empty or only Unicode White_Space, the same test as Swift's `isWhitespace`.
fn is_blank(value: &str) -> bool {
    value.chars().all(char::is_whitespace)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Keep this table identical to mailtoCases in apps/macos/Tests/ReaderLinkPolicyChecks.swift.
    const MAILTO_CASES: &[(&str, Option<&str>)] = &[
        ("mailto:a@example.com", Some("mailto:a@example.com")),
        ("MAILTO:a@example.com", Some("mailto:a@example.com")),
        (
            "mailto:a@example.com?subject=Hi&attach=%2Fetc%2Fpasswd&body=Line%0D%0Atwo&X-Foo=1&attachment=x&CC=c@example.com",
            Some("mailto:a@example.com?subject=Hi&body=Line%0D%0Atwo&cc=c@example.com"),
        ),
        (
            "mailto:a@example.com?Attach=/etc/passwd",
            Some("mailto:a@example.com"),
        ),
        (
            "mailto:a@example.com?%61ttach=x&%73ubject=Hi",
            Some("mailto:a@example.com?subject=Hi"),
        ),
        (
            "mailto:a@example.com,b@example.com?to=c@example.com&to=d@example.com&bcc=e@example.com",
            Some(
                "mailto:a@example.com,b@example.com?to=c@example.com&to=d@example.com&bcc=e@example.com",
            ),
        ),
        (
            "mailto:?to=a@example.com&subject=Hi",
            Some("mailto:?to=a@example.com&subject=Hi"),
        ),
        (
            "mailto:a@example.com?to=&subject=&body=",
            Some("mailto:a@example.com"),
        ),
        (
            "mailto:a@example.com?subject=a+b%20c#frag",
            Some("mailto:a@example.com?subject=a%2Bb%20c"),
        ),
        ("mailto:a@example.com?subject", Some("mailto:a@example.com")),
        (
            "mailto:a@example.com?&&subject=Hi&",
            Some("mailto:a@example.com?subject=Hi"),
        ),
        (
            "mailto:a@example.com?body=x%3Dy&subject=a=b",
            Some("mailto:a@example.com?body=x%3Dy&subject=a%3Db"),
        ),
        (
            "mailto:a@example.com?subject=x?attach=/etc/passwd",
            Some("mailto:a@example.com?subject=x%3Fattach%3D%2Fetc%2Fpasswd"),
        ),
        (
            "mailto:a@example.com&attach=/etc/passwd",
            Some("mailto:a@example.com%26attach%3D%2Fetc%2Fpasswd"),
        ),
        (
            "mailto:J%C3%B8rn@example.com?subject=Gr%C3%BC%C3%9Fe%20%26%20more",
            Some("mailto:J%C3%B8rn@example.com?subject=Gr%C3%BC%C3%9Fe%20%26%20more"),
        ),
        (
            "mailto:a@example.com?subject=two words",
            Some("mailto:a@example.com?subject=two%20words"),
        ),
        (
            "mailto:a@example.com?subject=%EF%BB%BFHi",
            Some("mailto:a@example.com?subject=%EF%BB%BFHi"),
        ),
        ("mailto:%E2%80%8B", Some("mailto:%E2%80%8B")),
        (
            "mailto:a@example.com?body=Tab%09ok",
            Some("mailto:a@example.com?body=Tab%09ok"),
        ),
        ("mailto:", None),
        ("mailto:?subject=Hi&body=x", None),
        ("mailto:%20?subject=Hi", None),
        ("mailto:?to=%20", None),
        ("mailto:%E2%80%83", None),
        ("mailto:%zz", None),
        ("mailto:a@example.com?subject=%E2%28", None),
        ("mailto:a@example.com?x=%zz", None),
        ("mailto:a@example.com%0D%0ABcc:x@evil.example", None),
        (
            "mailto:a@example.com?subject=Hi%0ABcc:%20x@evil.example",
            None,
        ),
        ("mailto:a@example.com?cc=x%00@evil.example", None),
        ("mailto:a@example.com?body=a%00b", None),
        ("https://example.com/", None),
        ("mailto", None),
        ("mail:a@example.com", None),
    ];

    #[test]
    fn mailto_keeps_only_recipients_subject_and_body() {
        for (input, expected) in MAILTO_CASES {
            assert_eq!(sanitized_mailto(input).as_deref(), *expected, "{input}");
        }
    }

    #[test]
    fn external_uri_passes_web_links_and_sanitizes_mailto() {
        assert_eq!(
            external_uri("https://example.com/a?b=c").as_deref(),
            Some("https://example.com/a?b=c")
        );
        assert_eq!(
            external_uri("http://example.com/").as_deref(),
            Some("http://example.com/")
        );
        assert_eq!(
            external_uri("mailto:a@example.com?attach=/etc/passwd&subject=Hi").as_deref(),
            Some("mailto:a@example.com?subject=Hi")
        );
        for blocked in [
            "mailto:",
            "mailto:?subject=Hi",
            "javascript:alert(1)",
            "file:///etc/passwd",
            "ftp://example.com/",
        ] {
            assert_eq!(external_uri(blocked), None, "{blocked}");
        }
    }
}
