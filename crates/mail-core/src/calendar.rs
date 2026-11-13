//! Bounded, read-only invitation extraction. This is not a provider calendar.
//! RFC 5545 dates retain their timezone semantics; unresolved TZIDs and floating
//! times never acquire a guessed local offset. Recurrences are not expanded.
use crate::{MailError, MailStore, Result};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

const MAX_BYTES: usize = 1024 * 1024;
const MAX_EVENTS: usize = 100;
const MAX_LINE: usize = 16 * 1024;
const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS calendar_invitations(message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,event_index INTEGER NOT NULL,event_json TEXT NOT NULL,start_value TEXT NOT NULL,PRIMARY KEY(message_id,event_index)); CREATE INDEX IF NOT EXISTS calendar_invitations_start ON calendar_invitations(start_value,message_id)";
pub(crate) fn initialize(db: &rusqlite::Connection) -> Result<()> {
    db.execute_batch(SCHEMA)?;
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CalendarDateTime {
    pub value: String,
    pub display: String,
    /// None is floating time, UTC is explicit UTC, otherwise an unresolved TZID.
    pub timezone: Option<String>,
    pub all_day: bool,
    pub utc_ms: Option<i64>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CalendarPerson {
    pub email: String,
    pub name: Option<String>,
    pub participation_status: Option<String>,
    pub response_requested: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CalendarEvent {
    pub uid: String,
    pub summary: String,
    pub description: String,
    pub location: String,
    pub organizer: Option<CalendarPerson>,
    pub attendees: Vec<CalendarPerson>,
    pub start: CalendarDateTime,
    pub end: Option<CalendarDateTime>,
    pub status: Option<String>,
    pub method: Option<String>,
    pub recurrence_rule: Option<String>,
    pub warnings: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CalendarInvitation {
    pub message_id: String,
    pub thread_id: String,
    pub account_id: String,
    pub event: CalendarEvent,
}

impl MailStore {
    /// Cache explicit downloaded MIME/ICS data. A successful refresh replaces
    /// this message's previous parsed events atomically; parse failure keeps it.
    pub fn cache_calendar_invitations(
        &self,
        message_id: &str,
        bytes: &[u8],
    ) -> Result<Vec<CalendarInvitation>> {
        let events = parse_calendar_invitations(bytes)?;
        self.cache_calendar_events(message_id, &events)
    }
    /// Replace all parsed calendar parts for a message in one transaction.
    /// Callers collect parsed attachments before this operation, so an error in
    /// a later part cannot erase an earlier successful cache.
    pub fn cache_calendar_events(
        &self,
        message_id: &str,
        events: &[CalendarEvent],
    ) -> Result<Vec<CalendarInvitation>> {
        if events.len() > MAX_EVENTS || serde_json::to_vec(events)?.len() > MAX_BYTES {
            return Err(invalid("Calendar cache limit exceeded"));
        }
        let mut db = self.lock()?;
        let source: Option<(String, String)> = db
            .query_row(
                "SELECT thread_id,account_id FROM messages WHERE id=?1",
                [message_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let (thread_id, account_id) =
            source.ok_or_else(|| MailError::NotFound(message_id.into()))?;
        db.execute_batch(SCHEMA)?;
        let tx = db.transaction()?;
        tx.execute(
            "DELETE FROM calendar_invitations WHERE message_id=?1",
            [message_id],
        )?;
        let mut output = Vec::with_capacity(events.len());
        for (index, event) in events.iter().enumerate() {
            tx.execute("INSERT INTO calendar_invitations(message_id,event_index,event_json,start_value) VALUES(?1,?2,?3,?4)", params![message_id,index as i64,serde_json::to_string(&event)?,event.start.value])?;
            output.push(CalendarInvitation {
                message_id: message_id.into(),
                thread_id: thread_id.clone(),
                account_id: account_id.clone(),
                event: event.clone(),
            });
        }
        tx.commit()?;
        Ok(output)
    }
    pub fn calendar_invitation(&self, message_id: &str) -> Result<Vec<CalendarInvitation>> {
        let db = self.lock()?;
        if db
            .query_row("SELECT 1 FROM messages WHERE id=?1", [message_id], |_| {
                Ok(())
            })
            .optional()?
            .is_none()
        {
            return Err(MailError::NotFound(message_id.into()));
        }
        db.execute_batch(SCHEMA)?;
        let mut q = db.prepare("SELECT c.message_id,m.thread_id,m.account_id,c.event_json FROM calendar_invitations c JOIN messages m ON m.id=c.message_id WHERE c.message_id=?1 ORDER BY c.event_index")?;
        read_rows(q.query([message_id])?)
    }
    /// Cached invitations only, not calendar-provider events. Maximum 500.
    /// Sort is source wall-date order; unresolved zones are not compared as instants.
    pub fn list_calendar_invitations(
        &self,
        account_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<CalendarInvitation>> {
        let db = self.lock()?;
        if let Some(account) = account_id {
            if db
                .query_row("SELECT 1 FROM accounts WHERE id=?1", [account], |_| Ok(()))
                .optional()?
                .is_none()
            {
                return Err(MailError::UnknownAccount(account.into()));
            }
        }
        db.execute_batch(SCHEMA)?;
        let mut q = db.prepare("SELECT c.message_id,m.thread_id,m.account_id,c.event_json FROM calendar_invitations c JOIN messages m ON m.id=c.message_id WHERE (?1 IS NULL OR m.account_id=?1) ORDER BY c.start_value,c.message_id,c.event_index LIMIT ?2")?;
        read_rows(q.query(params![account_id, limit.min(500) as i64])?)
    }
}
fn read_rows(mut rows: rusqlite::Rows<'_>) -> Result<Vec<CalendarInvitation>> {
    let mut output = Vec::new();
    while let Some(r) = rows.next()? {
        let raw: String = r.get(3)?;
        if raw.len() > MAX_BYTES {
            return Err(invalid("Cached invitation exceeds size limit"));
        }
        output.push(CalendarInvitation {
            message_id: r.get(0)?,
            thread_id: r.get(1)?,
            account_id: r.get(2)?,
            event: serde_json::from_str(&raw)?,
        });
    }
    Ok(output)
}
fn invalid(message: &str) -> MailError {
    MailError::InvalidAction(message.into())
}

/// Extract only explicit text/calendar or .ics MIME parts, or a complete ICS
/// document. Never downloads URLs or follows ATTACH/ALTREP links.
pub fn parse_calendar_invitations(bytes: &[u8]) -> Result<Vec<CalendarEvent>> {
    if bytes.len() > 40 * MAX_BYTES {
        return Err(invalid("Calendar MIME message exceeds 40 MiB"));
    }
    let text = std::str::from_utf8(bytes).ok();
    if text.is_some_and(|s| {
        s.trim_start_matches('\u{feff}')
            .trim_start()
            .get(..15)
            .is_some_and(|s| s.eq_ignore_ascii_case("BEGIN:VCALENDAR"))
    }) {
        return parse_ics(text.unwrap().trim_start_matches('\u{feff}').trim_start());
    }
    let mime = mailparse::parse_mail(bytes).map_err(|_| invalid("Invalid calendar MIME data"))?;
    let mut output = Vec::new();
    let mut count = 0;
    collect_parts(&mime, 0, &mut count, &mut output)?;
    Ok(output)
}
fn collect_parts(
    part: &mailparse::ParsedMail<'_>,
    depth: usize,
    count: &mut usize,
    output: &mut Vec<CalendarEvent>,
) -> Result<()> {
    *count += 1;
    if depth > 16 || *count > 64 {
        return Err(invalid("Calendar MIME nesting/part limit exceeded"));
    }
    let filename = part
        .get_content_disposition()
        .params
        .get("filename")
        .cloned()
        .or_else(|| part.ctype.params.get("name").cloned())
        .unwrap_or_default();
    if part.ctype.mimetype.eq_ignore_ascii_case("text/calendar")
        || filename.to_ascii_lowercase().ends_with(".ics")
    {
        let bytes = part
            .get_body_raw()
            .map_err(|_| invalid("Invalid encoded calendar attachment"))?;
        if bytes.len() > MAX_BYTES {
            return Err(invalid("Decoded calendar exceeds 1 MiB"));
        }
        let text = std::str::from_utf8(&bytes).map_err(|_| invalid("Calendar must be UTF-8"))?;
        output.extend(parse_ics(text.trim_start_matches('\u{feff}'))?);
        if output.len() > MAX_EVENTS {
            return Err(invalid("Calendar event limit exceeded"));
        }
    }
    for child in &part.subparts {
        collect_parts(child, depth + 1, count, output)?;
    }
    Ok(())
}

struct Property {
    name: String,
    params: HashMap<String, String>,
    value: String,
}
fn segments(raw: &str, delimiter: char) -> Result<Vec<&str>> {
    let mut quoted = false;
    let mut start = 0;
    let mut result = Vec::new();
    for (index, ch) in raw.char_indices() {
        if ch == '"' {
            quoted = !quoted;
        }
        if ch == delimiter && !quoted {
            result.push(&raw[start..index]);
            start = index + ch.len_utf8();
        }
    }
    if quoted {
        return Err(invalid("Unclosed quoted calendar parameter"));
    }
    result.push(&raw[start..]);
    Ok(result)
}
fn property(line: &str) -> Result<Property> {
    let mut quoted = false;
    let mut colon = None;
    for (i, c) in line.char_indices() {
        if c == '"' {
            quoted = !quoted;
        }
        if c == ':' && !quoted {
            colon = Some(i);
            break;
        }
    }
    let colon = colon.ok_or_else(|| invalid("Invalid calendar content line"))?;
    let head = segments(&line[..colon], ';')?;
    let name = head[0].to_ascii_uppercase();
    if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        return Err(invalid("Invalid calendar property name"));
    }
    let mut params = HashMap::new();
    for param in &head[1..] {
        let (key, value) = param
            .split_once('=')
            .ok_or_else(|| invalid("Invalid calendar parameter"))?;
        let key = key.to_ascii_uppercase();
        if params
            .insert(key, value.trim_matches('"').to_string())
            .is_some()
        {
            return Err(invalid("Duplicate calendar parameter"));
        }
    }
    Ok(Property {
        name,
        params,
        value: line[colon + 1..].into(),
    })
}
fn unescape(raw: &str) -> Result<String> {
    let mut chars = raw.chars();
    let mut output = String::new();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n' | 'N') => output.push('\n'),
                Some('\\') => output.push('\\'),
                Some(',') => output.push(','),
                Some(';') => output.push(';'),
                _ => return Err(invalid("Invalid calendar text escape")),
            }
        } else {
            output.push(c);
        }
    }
    Ok(output)
}
fn parse_ics(text: &str) -> Result<Vec<CalendarEvent>> {
    if text.len() > MAX_BYTES
        || text
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\r' | '\n' | '\t'))
    {
        return Err(invalid("Invalid or oversized calendar text"));
    }
    let normalized = text.replace("\r\n", "\n");
    if normalized.contains('\r') {
        return Err(invalid("Invalid calendar line endings"));
    }
    let mut lines: Vec<String> = Vec::new();
    for raw in normalized.lines() {
        if lines.len() > 10_000 {
            return Err(invalid("Calendar line limit exceeded"));
        }
        if raw.starts_with([' ', '\t']) {
            let line = lines
                .last_mut()
                .ok_or_else(|| invalid("Invalid initial folded line"))?;
            line.push_str(&raw[1..]);
            if line.len() > MAX_LINE {
                return Err(invalid("Calendar content line too long"));
            }
        } else if !raw.is_empty() {
            if raw.len() > MAX_LINE {
                return Err(invalid("Calendar content line too long"));
            }
            lines.push(raw.into());
        }
    }
    let mut stack: Vec<String> = Vec::new();
    let mut events = Vec::new();
    let mut current = Vec::new();
    let mut method = None;
    let mut seen_calendar = false;
    for line in lines {
        let p = property(&line)?;
        if p.name == "BEGIN" {
            let kind = p.value.to_ascii_uppercase();
            if stack.is_empty() {
                if kind != "VCALENDAR" || seen_calendar {
                    return Err(invalid("Expected one VCALENDAR document"));
                }
                seen_calendar = true;
            }
            if kind == "VEVENT" && stack.last().map(String::as_str) != Some("VCALENDAR") {
                return Err(invalid("Invalid nested calendar event"));
            }
            if stack.len() > 8 {
                return Err(invalid("Calendar component nesting too deep"));
            }
            stack.push(kind);
            continue;
        }
        if p.name == "END" {
            let kind = p.value.to_ascii_uppercase();
            if stack.pop().as_deref() != Some(kind.as_str()) {
                return Err(invalid("Unbalanced calendar component"));
            }
            if kind == "VEVENT" {
                events.push(build_event(std::mem::take(&mut current), method.clone())?);
                if events.len() > MAX_EVENTS {
                    return Err(invalid("Calendar event limit exceeded"));
                }
            }
            continue;
        }
        match stack.last().map(String::as_str) {
            Some("VCALENDAR") if p.name == "METHOD" => {
                if method.replace(p.value.to_ascii_uppercase()).is_some() {
                    return Err(invalid("Duplicate calendar METHOD"));
                }
            }
            Some("VEVENT") => current.push(p),
            None => return Err(invalid("Content outside calendar document")),
            _ => {}
        }
    }
    if !seen_calendar || !stack.is_empty() {
        return Err(invalid("Incomplete calendar document"));
    }
    // VCALENDAR property order is not significant; METHOD may follow VEVENT.
    for event in &mut events {
        event.method = method.clone();
    }
    Ok(events)
}
fn build_event(properties: Vec<Property>, method: Option<String>) -> Result<CalendarEvent> {
    let mut single = HashMap::new();
    let mut attendees = Vec::new();
    let mut warnings = Vec::new();
    for p in properties {
        if p.name == "ATTENDEE" {
            if attendees.len() >= 100 {
                return Err(invalid("Calendar attendee limit exceeded"));
            }
            attendees.push(person(&p)?);
        } else if [
            "UID",
            "SUMMARY",
            "DESCRIPTION",
            "LOCATION",
            "ORGANIZER",
            "DTSTART",
            "DTEND",
            "DURATION",
            "STATUS",
            "RRULE",
            "RECURRENCE-ID",
        ]
        .contains(&p.name.as_str())
        {
            if single.insert(p.name.clone(), p).is_some() {
                return Err(invalid("Duplicate singleton calendar property"));
            }
        }
    }
    let start = date(
        single
            .get("DTSTART")
            .ok_or_else(|| invalid("Calendar event has no start"))?,
    )?;
    let end = single.get("DTEND").map(date).transpose()?;
    if single.contains_key("DURATION") {
        if end.is_some() {
            return Err(invalid("Calendar cannot have both DTEND and DURATION"));
        }
        warnings.push("Event duration is not expanded; end time is unavailable.".into());
    }
    if start.timezone.as_deref() != Some("UTC") && !start.all_day {
        warnings.push(
            if start.timezone.is_some() {
                "Timezone retained as supplied; UTC conversion is unavailable."
            } else {
                "Floating time: no timezone was supplied."
            }
            .into(),
        );
    }
    if let Some(end) = &end {
        if start.all_day != end.all_day {
            return Err(invalid("Calendar end must use the same date type as start"));
        }
        if start.timezone == end.timezone && end.value <= start.value {
            return Err(invalid("Calendar end must follow start"));
        }
        if let (Some(a), Some(b)) = (start.utc_ms, end.utc_ms) {
            if b <= a {
                return Err(invalid("Calendar end must follow start"));
            }
        }
        if start.all_day {
            warnings.push("All-day end date is exclusive.".into());
        } else if start.timezone != end.timezone {
            warnings.push(
                "Start and end use different timezone forms; duration is not calculated.".into(),
            );
        }
    }
    let text = |key: &str| -> Result<String> {
        single
            .get(key)
            .map(|p| unescape(&p.value))
            .transpose()
            .map(|v| v.unwrap_or_default())
    };
    let uid = text("UID")?;
    if uid.is_empty() {
        return Err(invalid("Calendar event has no UID"));
    }
    let recurrence_rule = single.get("RRULE").map(|p| p.value.clone());
    if recurrence_rule.is_some() || single.contains_key("RECURRENCE-ID") {
        warnings.push("Recurring event: occurrences and exceptions are not expanded.".into());
    }
    Ok(CalendarEvent {
        uid,
        summary: text("SUMMARY")?,
        description: text("DESCRIPTION")?,
        location: text("LOCATION")?,
        organizer: single.get("ORGANIZER").map(person).transpose()?,
        attendees,
        start,
        end,
        status: single.get("STATUS").map(|p| p.value.to_ascii_uppercase()),
        method,
        recurrence_rule,
        warnings,
    })
}
fn person(p: &Property) -> Result<CalendarPerson> {
    let (scheme, value) = p
        .value
        .split_once(':')
        .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("mailto"))
        .ok_or_else(|| invalid("Only mailto calendar participants are supported"))?;
    let _ = scheme;
    if !email_address::EmailAddress::is_valid(value) || value.chars().any(char::is_control) {
        return Err(invalid("Invalid calendar participant email"));
    }
    Ok(CalendarPerson {
        email: value.into(),
        name: p.params.get("CN").cloned(),
        participation_status: p.params.get("PARTSTAT").map(|s| s.to_ascii_uppercase()),
        response_requested: p
            .params
            .get("RSVP")
            .is_some_and(|s| s.eq_ignore_ascii_case("TRUE")),
    })
}
fn date(p: &Property) -> Result<CalendarDateTime> {
    let value = p.value.to_ascii_uppercase();
    let all_day = p
        .params
        .get("VALUE")
        .is_some_and(|v| v.eq_ignore_ascii_case("DATE"));
    if let Some(kind) = p.params.get("VALUE") {
        if !kind.eq_ignore_ascii_case("DATE") && !kind.eq_ignore_ascii_case("DATE-TIME") {
            return Err(invalid("Unsupported calendar date type"));
        }
    }
    let utc = value.ends_with('Z');
    let raw = value.strip_suffix('Z').unwrap_or(&value);
    if (all_day && raw.len() != 8)
        || (!all_day && (raw.len() != 15 || raw.as_bytes().get(8) != Some(&b'T')))
        || (all_day && utc)
    {
        return Err(invalid("Invalid calendar date/time"));
    }
    if !raw
        .bytes()
        .enumerate()
        .all(|(i, b)| (!all_day && i == 8) || b.is_ascii_digit())
    {
        return Err(invalid("Invalid calendar date/time digits"));
    }
    let n = |a: usize, b: usize| raw[a..b].parse::<i64>().unwrap_or(-1);
    let (y, m, d) = (n(0, 4), n(4, 6), n(6, 8));
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let days = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if leap {
                29
            } else {
                28
            }
        }
        _ => 0,
    };
    if y < 1 || d < 1 || d > days {
        return Err(invalid("Invalid Gregorian calendar date"));
    }
    let (h, min, s) = if all_day {
        (0, 0, 0)
    } else {
        (n(9, 11), n(11, 13), n(13, 15))
    };
    // Leap seconds cannot be represented faithfully by Unix milliseconds.
    if h > 23 || min > 59 || s > 59 {
        return Err(invalid("Invalid or unsupported calendar time"));
    }
    let tz = p.params.get("TZID").cloned();
    if tz.as_ref().is_some_and(|v| v.is_empty() || v.len() > 255)
        || tz.is_some() && (utc || all_day)
    {
        return Err(invalid("Invalid calendar timezone combination"));
    }
    let timezone = if utc { Some("UTC".into()) } else { tz };
    let display = if all_day {
        format!("{y:04}-{m:02}-{d:02}")
    } else {
        format!("{y:04}-{m:02}-{d:02} {h:02}:{min:02}:{s:02}")
    };
    let utc_ms = utc.then(|| (days_from_civil(y, m, d) * 86_400 + h * 3600 + min * 60 + s) * 1000);
    Ok(CalendarDateTime {
        value: value.clone(),
        display,
        timezone,
        all_day,
        utc_ms,
    })
}
fn days_from_civil(mut y: i64, m: i64, d: i64) -> i64 {
    y -= i64::from(m <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}
