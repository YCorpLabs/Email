//! Pure visual-search filter rules. Raw terms remain raw; no silent broadening.
use std::collections::HashMap;

const FIELDS: [&str; 6] = ["from", "to", "cc", "subject", "after", "before"];

#[derive(Debug, Default)]
pub struct FilterDraft {
    pub remainder: String,
    pub fields: HashMap<String, String>,
    pub read: usize,
    pub attachments: bool,
}
#[derive(Debug)]
struct Token {
    raw: String,
    value: String,
    literal: bool,
}

fn tokens(base: &str) -> Result<Vec<Token>, String> {
    if base.len() > 4096 {
        return Err("Search is limited to 4096 bytes".into());
    }
    let mut output = Vec::new();
    let mut raw = String::new();
    let mut value = String::new();
    let mut quoted = false;
    let mut escaped = false;
    let mut literal = false;
    for c in base.chars() {
        if c.is_control() && !c.is_whitespace() {
            return Err("Search contains an unsupported control character".into());
        }
        if escaped {
            raw.push(c);
            value.push(c);
            escaped = false;
            continue;
        }
        if quoted && c == '\\' {
            raw.push(c);
            escaped = true;
            continue;
        }
        if c == '"' {
            if raw.is_empty() {
                literal = true;
            }
            raw.push(c);
            quoted = !quoted;
            continue;
        }
        if c.is_whitespace() && !quoted {
            if !raw.is_empty() {
                output.push(Token {
                    raw: std::mem::take(&mut raw),
                    value: std::mem::take(&mut value),
                    literal,
                });
                literal = false;
            }
        } else {
            raw.push(c);
            value.push(c);
        }
    }
    if quoted || escaped {
        return Err("Close the quoted search value with a double quote".into());
    }
    if !raw.is_empty() {
        output.push(Token {
            raw,
            value,
            literal,
        });
    }
    if output.len() > 64 {
        return Err("Use at most 64 search terms".into());
    }
    Ok(output)
}
fn pair(token: &Token) -> Option<(String, &str)> {
    if token.literal {
        return None;
    }
    token
        .value
        .split_once(':')
        .map(|(key, value)| (key.to_ascii_lowercase(), value))
}
fn key(token: &Token) -> Option<String> {
    let (key, value) = pair(token)?;
    if FIELDS.contains(&key.as_str()) {
        Some(key)
    } else if key == "is"
        && (value.eq_ignore_ascii_case("read") || value.eq_ignore_ascii_case("unread"))
    {
        Some("read".into())
    } else if key == "has" && value.eq_ignore_ascii_case("attachment") {
        Some("attachments".into())
    } else {
        None
    }
}
fn valid_date(value: &str) -> bool {
    let b = value.as_bytes();
    if b.len() != 10
        || b[4] != b'-'
        || b[7] != b'-'
        || b.iter()
            .enumerate()
            .any(|(i, c)| i != 4 && i != 7 && !c.is_ascii_digit())
    {
        return false;
    }
    let year = value[..4].parse::<u32>().unwrap();
    let month = value[5..7].parse::<u32>().unwrap();
    let day = value[8..].parse::<u32>().unwrap();
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let max = match month {
        2 => {
            if leap {
                29
            } else {
                28
            }
        }
        4 | 6 | 9 | 11 => 30,
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        _ => 0,
    };
    year > 0 && day > 0 && day <= max
}

/// Decompose only unambiguous unique controls. Duplicate keys and quoted literal
/// operator-looking terms stay in remainder, preserving all original predicates.
pub fn decompose(base: &str) -> FilterDraft {
    let Ok(parsed) = tokens(base) else {
        return FilterDraft {
            remainder: base.into(),
            ..Default::default()
        };
    };
    let mut counts = HashMap::<String, usize>::new();
    for token in &parsed {
        if let Some(k) = key(token) {
            *counts.entry(k).or_default() += 1;
        }
    }
    let mut draft = FilterDraft::default();
    let mut remaining = Vec::new();
    for token in parsed {
        let supported = key(&token);
        if let (Some(k), Some((_, value))) = (supported, pair(&token)) {
            if counts[&k] == 1
                && !value.is_empty()
                && value.trim() == value
                && (!(k == "after" || k == "before") || valid_date(value))
            {
                match k.as_str() {
                    "read" => {
                        draft.read = if value.eq_ignore_ascii_case("unread") {
                            1
                        } else {
                            2
                        }
                    }
                    "attachments" => draft.attachments = true,
                    _ => {
                        draft.fields.insert(k, value.into());
                    }
                }
                continue;
            }
        }
        remaining.push(token.raw);
    }
    draft.remainder = remaining.join(" ");
    draft
}
fn quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

/// `base` is the editable remainder from decompose. New controls intersect it;
/// invalid/conflicting raw filters are rejected, never removed or replaced.
pub fn query(
    base: &str,
    fields: &HashMap<String, String>,
    read: usize,
    attachments: bool,
) -> Result<String, String> {
    if read > 2 {
        return Err("Choose Any, Unread or Read".into());
    }
    for (key, value) in fields {
        if !FIELDS.contains(&key.as_str()) && !value.trim().is_empty() {
            return Err(format!("Unsupported visual filter: {key}"));
        }
    }
    let parsed = tokens(base)?;
    let mut existing = HashMap::<String, Vec<String>>::new();
    let mut read_filters = Vec::new();
    let mut after = Vec::new();
    let mut before = Vec::new();
    for token in &parsed {
        if token.value.is_empty() {
            return Err("Search terms cannot be empty".into());
        }
        let Some((name, value)) = pair(token) else {
            continue;
        };
        if [
            "from", "to", "cc", "subject", "after", "before", "is", "in", "has", "label",
        ]
        .contains(&name.as_str())
            && value.is_empty()
        {
            return Err(format!("{name}: needs a value"));
        }
        match name.as_str() {
            "is" => {
                if value.eq_ignore_ascii_case("unread") {
                    read_filters.push(1);
                } else if value.eq_ignore_ascii_case("read") {
                    read_filters.push(2);
                } else if !value.eq_ignore_ascii_case("starred") {
                    return Err("Use is:unread, is:read, or is:starred".into());
                }
            }
            "has" if !value.eq_ignore_ascii_case("attachment") => {
                return Err("Use has:attachment".into());
            }
            "in" if !["inbox", "archive", "spam", "junk", "trash"]
                .contains(&value.to_ascii_lowercase().as_str()) =>
            {
                return Err("Use in:inbox, in:archive, in:spam, or in:trash".into());
            }
            "after" | "before" => {
                if !valid_date(value) {
                    return Err("Use a valid date in YYYY-MM-DD form".into());
                }
                if name == "after" {
                    after.push(value.to_owned());
                } else {
                    before.push(value.to_owned());
                }
            }
            _ => {}
        }
        if FIELDS.contains(&name.as_str()) {
            existing.entry(name).or_default().push(value.into());
        }
    }
    let mut output = base.trim().to_owned();
    for key in FIELDS {
        let Some(value) = fields.get(key).map(|v| v.trim()).filter(|v| !v.is_empty()) else {
            continue;
        };
        if value.chars().any(char::is_control) {
            return Err(format!("{key}: cannot contain control characters"));
        }
        if existing
            .get(key)
            .is_some_and(|values| values.iter().any(|v| v != value))
        {
            return Err(format!(
                "{key}: also appears in additional terms. Edit those terms before changing this filter."
            ));
        }
        if key == "after" || key == "before" {
            if !valid_date(value) {
                return Err("Use a valid date in YYYY-MM-DD form".into());
            }
            if key == "after" {
                after.push(value.into());
            } else {
                before.push(value.into());
            }
        }
        if !output.is_empty() {
            output.push(' ');
        }
        output.push_str(key);
        output.push(':');
        output.push_str(&quote(value));
    }
    if read != 0 {
        read_filters.push(read);
    }
    if read_filters.contains(&1) && read_filters.contains(&2) {
        return Err("Read and Unread conflict. Edit additional terms or choose Any.".into());
    }
    if let (Some(lower), Some(upper)) = (after.iter().max(), before.iter().min()) {
        if lower >= upper {
            return Err("After must be earlier than Before".into());
        }
    }
    if read != 0 {
        if !output.is_empty() {
            output.push(' ');
        }
        output.push_str(if read == 1 { "is:unread" } else { "is:read" });
    }
    if attachments {
        if !output.is_empty() {
            output.push(' ');
        }
        output.push_str("has:attachment");
    }
    tokens(&output)?;
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrip_preserves_phrase_escaping_and_unknown_operators() {
        let raw = r#"from:"Doe, Jane" subject:"say \"hello\"" "literal from:person" label:"Customer Support" unknown:value is:starred is:unread has:attachment after:2024-02-29"#;
        let draft = decompose(raw);
        assert_eq!(draft.fields["from"], "Doe, Jane");
        assert_eq!(draft.fields["subject"], "say \"hello\"");
        assert_eq!(draft.read, 1);
        assert!(draft.attachments);
        let rebuilt = query(
            &draft.remainder,
            &draft.fields,
            draft.read,
            draft.attachments,
        )
        .unwrap();
        let again = decompose(&rebuilt);
        assert_eq!(again.fields, draft.fields);
        assert_eq!(again.remainder, draft.remainder);
        assert_eq!(again.read, 1);
        assert!(again.attachments);
    }
    #[test]
    fn duplicates_are_kept_and_conflicting_controls_rejected() {
        let raw = "from:a@example.org from:b@example.org is:read is:unread after:2026-01-01 after:2026-02-01";
        let draft = decompose(raw);
        assert!(!draft.fields.contains_key("from"));
        assert!(!draft.fields.contains_key("after"));
        assert_eq!(draft.read, 0);
        assert_eq!(draft.remainder, raw);
        assert!(
            query(&draft.remainder, &draft.fields, 0, false)
                .unwrap_err()
                .contains("conflict")
        );
        let mut fields = HashMap::new();
        fields.insert("from".into(), "c@example.org".into());
        assert!(
            query("from:a@example.org from:b@example.org", &fields, 0, false)
                .unwrap_err()
                .contains("additional terms")
        );
        assert!(query("is:read", &HashMap::new(), 1, false).is_err());
    }
    #[test]
    fn literal_operator_text_and_edge_spaces_are_not_extracted() {
        let raw = r#""from:alice" "from":alice from:" alice " "subject:a b""#;
        let draft = decompose(raw);
        assert!(draft.fields.is_empty());
        assert_eq!(draft.remainder, raw);
        assert_eq!(query(raw, &HashMap::new(), 0, false).unwrap(), raw);
    }
    #[test]
    fn invalid_or_contradictory_dates_fail_without_broadening() {
        for date in [
            "2023-02-29",
            "2026-04-31",
            "0000-01-01",
            "2026-1-01",
            "２０２６-01-01",
        ] {
            let mut fields = HashMap::new();
            fields.insert("after".into(), date.into());
            assert!(query("", &fields, 0, false).is_err());
        }
        assert!(
            query(
                "after:2026-02-01 before:2026-02-01",
                &HashMap::new(),
                0,
                false
            )
            .is_err()
        );
        let mut fields = HashMap::new();
        fields.insert("before".into(), "2026-02-01".into());
        assert!(query("after:2026-03-01", &fields, 0, false).is_err());
        assert!(
            query(
                "after:2024-02-29 before:2024-03-01",
                &HashMap::new(),
                0,
                false
            )
            .is_ok()
        );
    }
    #[test]
    fn malformed_queries_and_operator_values_remain_visible_errors() {
        for raw in [
            "subject:\"unfinished",
            "from:",
            "is:banana",
            "has:everything",
            "in:moon",
            "\"\"",
        ] {
            let draft = decompose(raw);
            assert!(
                query(
                    &draft.remainder,
                    &draft.fields,
                    draft.read,
                    draft.attachments
                )
                .is_err(),
                "{raw}"
            );
        }
        assert!(query(&"word ".repeat(65), &HashMap::new(), 0, false).is_err());
        assert!(query(&"x".repeat(4097), &HashMap::new(), 0, false).is_err());
        let mut fields = HashMap::new();
        fields.insert("subject".into(), "hello\nthere".into());
        assert!(query("", &fields, 0, false).is_err());
    }
    #[test]
    fn user_values_are_quoted_not_promoted_to_extra_operators() {
        let mut fields = HashMap::new();
        fields.insert("subject".into(), "hi is:read \\".into());
        let built = query("label:Work", &fields, 1, true).unwrap();
        let draft = decompose(&built);
        assert_eq!(draft.fields["subject"], "hi is:read \\");
        assert_eq!(draft.read, 1);
        assert!(draft.attachments);
        assert_eq!(draft.remainder, "label:Work");
    }
}
