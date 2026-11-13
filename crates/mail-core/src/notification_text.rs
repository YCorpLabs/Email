//! Short, readable message previews for system notifications.
//!
//! Newsletters often ship a plain-text part that is only dividers, a "view web
//! version" link and legal text, so the plain body is cleaned per paragraph and,
//! when it is a stub, the HTML preheader or visible text is used instead.

const BOILERPLATE: &[&str] = &[
    "web version", "view in browser", "view in your browser", "view it in your browser",
    "view this email", "view email in", "view online", "view as a web page", "open in browser",
    "read online", "trouble viewing", "not displaying correctly", "can't see this email",
    "can’t see this email", "cannot see this email", "unsubscribe", "manage preferences",
    "email preferences",
];

const HTML_STUB: &[&str] = &[
    "cannot display html", "can't display html", "can’t display html", "unable to display html",
    "not display html", "not support html", "doesn't support html", "doesn’t support html",
    "enable html", "html-capable", "html capable", "html version", "html format",
];

/// Notification text for a message: what was newly written, without quoted
/// history, signatures, links, dividers or "view in browser" chrome.
pub fn notification_snippet(body_plain: &str, body_html: &str, limit: usize) -> String {
    let plain = plain_snippet(body_plain);
    let joined = body_plain.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
    let stub = HTML_STUB.iter().any(|phrase| joined.contains(phrase));
    let text = if (plain.is_empty() || stub) && !body_html.trim().is_empty() {
        let html = html_snippet(body_html);
        if html.is_empty() && !stub { plain } else { html }
    } else {
        plain
    };
    truncate(&text, limit)
}

fn plain_snippet(body: &str) -> String {
    let mut paragraphs: Vec<Vec<String>> = vec![Vec::new()];
    let mut seen_text = false;
    for raw in body.lines() {
        let line = raw.trim();
        let lower = line.to_lowercase();
        if line == "--" || raw.starts_with("-- ") || lower.starts_with("-----original message")
            || lower.starts_with("sent from my ") || lower.starts_with("________________")
            || (lower.starts_with("on ") && lower.ends_with("wrote:"))
            || (lower.starts_with("from:") && seen_text) { break; }
        if line.starts_with('>') { continue; }
        let clean = clean_line(&decode_entities(line));
        if clean.chars().any(char::is_alphanumeric) {
            seen_text = true;
            paragraphs.last_mut().unwrap().push(clean);
        } else if !paragraphs.last().unwrap().is_empty() {
            // Blank lines and dividers end a paragraph.
            paragraphs.push(Vec::new());
        }
    }
    paragraphs.iter().map(|lines| lines.join(" ")).filter(|paragraph| {
        let lower = paragraph.to_lowercase();
        !paragraph.is_empty()
            && !(paragraph.chars().count() < 160 && BOILERPLATE.iter().any(|phrase| lower.contains(phrase)))
    }).collect::<Vec<_>>().join(" ")
}

/// Drops invisible preheader padding, links and `[image: …]` placeholders.
fn clean_line(raw: &str) -> String {
    let mut line: String = raw.chars().filter_map(|c| match c {
        '\u{200B}'..='\u{200D}' | '\u{2060}' | '\u{FEFF}' | '\u{034F}' | '\u{00AD}' => None,
        '\u{00A0}' | '\u{2007}' | '\u{202F}' => Some(' '),
        c => Some(c),
    }).collect();
    while let Some(start) = find_ascii_ci(&line, "[image:") {
        let Some(end) = line[start..].find(']') else { break };
        line.replace_range(start..start + end + 1, "");
    }
    line.split_whitespace().filter(|word| {
        let token = word.trim_matches(|c| "<>()[]\"'".contains(c)).to_lowercase();
        !(token.starts_with("http://") || token.starts_with("https://") || token.starts_with("www.")
            || token.starts_with("mailto:") || word.chars().all(|c| "()[]<>{}".contains(c)))
    }).collect::<Vec<_>>().join(" ")
}

/// The hidden preheader marketers write for inbox previews, else visible text.
fn html_snippet(html: &str) -> String {
    let body = match find_ascii_ci(html, "<body") { Some(start) => &html[start..], None => html };
    let mut visible = String::new();
    let mut preheader: Option<String> = None;
    let mut in_preheader = false;
    let mut skip_until: Option<&str> = None;
    let mut rest = body;
    while !rest.is_empty() {
        if let Some(close) = skip_until {
            match find_ascii_ci(rest, close) {
                Some(end) => { rest = &rest[end + close.len()..]; skip_until = None; }
                None => break,
            }
            continue;
        }
        let Some(open) = rest.find('<') else { push_text(&mut visible, rest); break };
        let text = &rest[..open];
        if in_preheader {
            let candidate = clean_line(&decode_entities(text));
            if candidate.chars().any(char::is_alphanumeric) {
                preheader = Some(candidate);
                in_preheader = false;
            }
        }
        push_text(&mut visible, text);
        rest = &rest[open..];
        if rest.starts_with("<!--") {
            skip_until = Some("-->");
            rest = &rest[4..];
            continue;
        }
        let end = rest.find('>').map_or(rest.len(), |i| i + 1);
        let tag = &rest[..end];
        rest = &rest[end..];
        let name: String = tag.trim_start_matches(['<', '/']).chars()
            .take_while(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase();
        match name.as_str() {
            "style" | "script" | "head" | "title" if !tag.starts_with("</") => {
                skip_until = Some(match name.as_str() {
                    "style" => "</style", "script" => "</script", "head" => "</head", _ => "</title",
                });
            }
            "br" | "p" | "div" | "tr" | "td" | "th" | "li" | "table" | "h1" | "h2" | "h3" | "h4"
            | "h5" | "h6" | "blockquote" => visible.push_str("\n\n"),
            _ => {}
        }
        if tag.starts_with("</") {
            in_preheader = false;
        } else if preheader.is_none() && !in_preheader {
            let compact: String = tag.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_ascii_lowercase();
            in_preheader = compact.contains("display:none");
        }
    }
    if let Some(preheader) = preheader { return preheader; }
    plain_snippet(&visible)
}

fn push_text(out: &mut String, text: &str) {
    if !text.trim().is_empty() { out.push(' '); out.push_str(text.trim()); }
}

fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let Some(semi) = rest[..rest.len().min(12)].find(';') else { out.push('&'); rest = &rest[1..]; continue };
        let entity = &rest[1..semi];
        let decoded = match entity {
            "amp" => Some('&'), "lt" => Some('<'), "gt" => Some('>'), "quot" => Some('"'),
            "apos" => Some('\''), "nbsp" => Some(' '), "rsquo" => Some('’'), "lsquo" => Some('‘'),
            "ldquo" => Some('“'), "rdquo" => Some('”'), "ndash" => Some('–'), "mdash" => Some('—'),
            "hellip" => Some('…'), "copy" => Some('©'), "reg" => Some('®'), "trade" => Some('™'),
            "zwnj" | "zwj" | "shy" => Some('\u{200B}'),
            _ => entity.strip_prefix('#').and_then(|num| match num.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok(),
                None => num.parse().ok(),
            }).and_then(char::from_u32),
        };
        match decoded {
            Some(c) => { out.push(c); rest = &rest[semi + 1..]; }
            None => { out.push('&'); rest = &rest[1..]; }
        }
    }
    out.push_str(rest);
    out
}

/// Byte offset of an ASCII needle, ignoring ASCII case; always a char boundary.
fn find_ascii_ci(haystack: &str, needle: &str) -> Option<usize> {
    haystack.as_bytes().windows(needle.len()).position(|w| w.eq_ignore_ascii_case(needle.as_bytes()))
}

fn truncate(text: &str, limit: usize) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() > limit { format!("{}…", text.chars().take(limit.saturating_sub(1)).collect::<String>().trim_end()) } else { text }
}

#[cfg(test)]
mod tests {
    use super::notification_snippet as snippet;

    #[test]
    fn replies_show_only_the_new_text() {
        assert_eq!(snippet("Sounds good,\nsee you at 3.\n\nOn Tue, Ada wrote:\n> earlier", "", 180), "Sounds good, see you at 3.");
        assert_eq!(snippet("> quoted\nNew text\n-- \nSignature", "", 180), "New text");
        assert_eq!(snippet(&"a".repeat(300), "", 180).chars().count(), 180);
    }

    #[test]
    fn newsletter_chrome_is_removed() {
        let body = format!("\u{200C}\u{034F} \u{00A0}\n{}\n\nView web version:\nhttps://t3.mail.example.com/r/?id=t0\n\n[image: Brand]\nGive every glance a centre stage.\nRead more <https://example.com/a>\n\nUnsubscribe", "-".repeat(70));
        assert_eq!(snippet(&body, "", 180), "Give every glance a centre stage. Read more");
        assert_eq!(snippet("We&#39;re live &amp; ready ( https://x.test ) now", "", 180), "We're live & ready now");
        assert_eq!(snippet("Having trouble viewing? Click here\n=====\n* * *\nhttps://example.com", "", 180), "");
    }

    #[test]
    fn plain_text_stubs_fall_back_to_the_html_preheader() {
        let plain = "----\n\nView web version:\nhttps://x.test/r\n\n----\n\nUnfortunately, your email client cannot display HTML, or your settings\nare turned off. To view this email, please click the link above.";
        let html = "<html><head><title>Hi</title><style>a{color:red}</style></head><body bgcolor=\"#fff\">\n<div style=\"font-size:1px; display: none;\">Bring focus with AI&#8209;powered Lens Blur&nbsp;&zwnj;&nbsp;&zwnj;</div><table><tr><td>Amplify your <br>focal point.</td></tr></table></body></html>";
        assert_eq!(snippet(plain, html, 180), "Bring focus with AI‑powered Lens Blur");
        let no_preheader = "<body><!-- x --><p>Amplify your<br>focal point.</p><p>Lead the eyes &amp; more.</p><a href=\"https://x\">Unsubscribe</a></body>";
        assert_eq!(snippet("", no_preheader, 180), "Amplify your focal point. Lead the eyes & more.");
    }
}
