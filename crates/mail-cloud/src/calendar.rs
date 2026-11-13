//! Google Calendar transport. Invitation MIME data never acts as provider calendar state.
//! Calendar tokens are granted and persisted separately by the service.
use crate::{CloudClient, CloudError, CloudProvider, Result};
use chrono::{DateTime, Datelike, NaiveDate, SecondsFormat, TimeZone, Utc};
use chrono_tz::Tz;
use mail_core::{AgendaAttendee, AgendaEvent, CalendarCollection};
use rand::{RngCore, rngs::OsRng};
use serde_json::{Value, json};
use std::collections::HashSet;
use url::Url;

const BASE: &str = "https://www.googleapis.com/calendar/v3";
const MAX_WINDOW_MS: i64 = 93 * 86_400_000;
const MAX_LISTING_WINDOW_MS: i64 = 400 * 86_400_000;
const MAX_EVENTS: usize = 10_000;
const MAX_CALENDARS: usize = 256;
const MAX_ATTENDEES: usize = 500;

/// Only these fields may be changed. Omitted fields, guests and provider metadata survive PATCH.
#[derive(Debug, Clone, Default)]
pub struct CalendarEventChanges {
    pub title: Option<String>,
    pub start_ms: Option<i64>,
    pub end_ms: Option<i64>,
    pub description: Option<String>,
    pub location: Option<String>,
}
impl CalendarEventChanges {
    pub fn validate(&self, expected_etag: &str, edit_scope: &str) -> Result<Value> {
        if expected_etag.trim().is_empty()
            || expected_etag.len() > 2048
            || expected_etag.chars().any(char::is_control)
        {
            return Err(invalid(
                "refresh the event before editing: version missing or invalid",
            ));
        }
        if !matches!(edit_scope, "single" | "instance") {
            return Err(invalid("choose single or instance edit scope"));
        }
        let mut patch = json!({});
        if let Some(title) = &self.title {
            if title.trim().is_empty()
                || title.len() > 16_384
                || title.chars().any(char::is_control)
            {
                return Err(invalid("invalid calendar title"));
            }
            patch["summary"] = json!(title.trim());
        }
        for (field, value) in [
            ("description", &self.description),
            ("location", &self.location),
        ] {
            if let Some(value) = value {
                if value.len() > 16_384
                    || value
                        .chars()
                        .any(|c| c.is_control() && c != '\n' && c != '\t')
                {
                    return Err(invalid("invalid calendar description or location"));
                }
                patch[field] = json!(value);
            }
        }
        match (self.start_ms, self.end_ms) {
            (Some(start), Some(end)) => {
                window(start, end)?;
                patch["start"] =
                    json!({"dateTime":instant(start)?.to_rfc3339_opts(SecondsFormat::Millis,true)});
                patch["end"] =
                    json!({"dateTime":instant(end)?.to_rfc3339_opts(SecondsFormat::Millis,true)});
            }
            (None, None) => {}
            _ => return Err(invalid("supply both calendar start and end")),
        }
        if patch.as_object().unwrap().is_empty() {
            return Err(invalid("no calendar changes supplied"));
        }
        Ok(patch)
    }
}
fn editable_event(value: &Value) -> bool {
    value["status"] != "cancelled"
        && value["organizer"]["self"] == true
        && value["locked"] != true
        && value.get("recurrence").is_none()
        && value["eventType"].as_str().unwrap_or("default") == "default"
        && value["start"]["dateTime"].is_string()
        && value["end"]["dateTime"].is_string()
}
fn validate_edit_target(value: &Value, expected_etag: &str, edit_scope: &str) -> Result<()> {
    if value["etag"].as_str() != Some(expected_etag) {
        return Err(CloudError::CalendarConflict);
    }
    if !editable_event(value) {
        return Err(invalid(
            "this event cannot be edited here; use its organizer calendar",
        ));
    }
    let recurring = value["recurringEventId"]
        .as_str()
        .is_some_and(|s| !s.is_empty());
    if recurring && (edit_scope != "instance" || !value["originalStartTime"].is_object()) {
        return Err(invalid("recurring events require explicit instance scope"));
    }
    if !recurring && edit_scope != "single" {
        return Err(invalid("instance scope requires a recurring instance"));
    }
    Ok(())
}

fn conditional_event_patch(
    http: &reqwest::blocking::Client,
    mut url: Url,
    etag: &str,
    patch: &Value,
) -> reqwest::blocking::RequestBuilder {
    url.query_pairs_mut().append_pair("sendUpdates", "all");
    http.patch(url.as_str())
        .header(reqwest::header::IF_MATCH, etag)
        .json(patch)
}

fn invalid(message: &'static str) -> CloudError {
    CloudError::Response(message)
}
fn identifier(value: &str) -> Result<&str> {
    if value.is_empty() || value.len() > 2048 || value.chars().any(char::is_control) {
        return Err(invalid("invalid calendar identifier"));
    }
    Ok(value)
}
fn text(value: &Value, field: &str, max: usize) -> Result<Option<String>> {
    match value.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if s.len() <= max => Ok(Some(s.clone())),
        _ => Err(invalid("invalid or oversized calendar field")),
    }
}
fn mailbox(value: &str) -> bool {
    value.len() <= 254
        && value.parse::<lettre::Address>().is_ok()
        && value.is_ascii()
        && !value.chars().any(|c| c.is_control() || c.is_whitespace())
        && value.split('@').count() == 2
        && value.split('@').all(|part| !part.is_empty())
        && !value.contains(['<', '>', ',', ';', '"', '\\'])
}
fn instant(ms: i64) -> Result<DateTime<Utc>> {
    Utc.timestamp_millis_opt(ms)
        .single()
        .filter(|d| (1..=9999).contains(&d.year()))
        .ok_or_else(|| invalid("calendar date is out of range"))
}
fn listing_window(start: i64, end: i64) -> Result<()> {
    instant(start)?;
    instant(end)?;
    if end <= start
        || end
            .checked_sub(start)
            .is_none_or(|n| n > MAX_LISTING_WINDOW_MS)
    {
        return Err(invalid(
            "calendar listing window must be positive and no more than 400 days",
        ));
    }
    Ok(())
}
fn window(start: i64, end: i64) -> Result<()> {
    instant(start)?;
    instant(end)?;
    if end <= start || end.checked_sub(start).is_none_or(|n| n > MAX_WINDOW_MS) {
        return Err(invalid(
            "calendar window must be positive and no more than 93 days",
        ));
    }
    Ok(())
}
fn event_url(calendar_id: &str, event_id: Option<&str>) -> Result<Url> {
    let mut url = Url::parse(BASE)?;
    {
        let mut segments = url
            .path_segments_mut()
            .map_err(|_| invalid("invalid calendar URL"))?;
        segments
            .push("calendars")
            .push(identifier(calendar_id)?)
            .push("events");
        if let Some(id) = event_id {
            segments.push(identifier(id)?);
        }
    }
    Ok(url)
}
fn next_page(page: &Value, seen: &mut HashSet<String>) -> Result<Option<String>> {
    let token = text(page, "nextPageToken", 4096)?;
    if let Some(token) = &token {
        if token.is_empty() || !seen.insert(token.clone()) {
            return Err(invalid("invalid or repeated calendar page token"));
        }
    }
    Ok(token)
}
fn parse_identity(value: &Value) -> Result<(String, String)> {
    let sub = text(value, "sub", 256)?
        .filter(|s| !s.is_empty())
        .ok_or_else(|| invalid("Google identity missing subject"))?;
    let email = text(value, "email", 254)?
        .filter(|s| mailbox(s))
        .ok_or_else(|| invalid("Google identity missing email"))?;
    if value["email_verified"] != true {
        return Err(invalid("Google email is not verified"));
    }
    Ok((sub, email))
}
fn parse_collection(value: &Value, account_id: &str) -> Result<Option<CalendarCollection>> {
    if value["deleted"] == true {
        return Ok(None);
    }
    let role = value["accessRole"].as_str().unwrap_or("none");
    if !matches!(
        role,
        "reader" | "writer" | "writerWithoutPrivateAccess" | "owner"
    ) {
        return Ok(None);
    }
    let id = text(value, "id", 2048)?.ok_or_else(|| invalid("calendar ID missing"))?;
    identifier(&id)?;
    Ok(Some(CalendarCollection {
        id,
        account_id: account_id.to_owned(),
        name: text(value, "summaryOverride", 1024)?
            .or(text(value, "summary", 1024)?)
            .unwrap_or_else(|| "Calendar".into()),
        primary: value["primary"] == true,
        can_write: matches!(role, "writer" | "writerWithoutPrivateAccess" | "owner"),
    }))
}
fn parse_time(value: &Value, zone: &str) -> Result<(i64, bool)> {
    if let Some(date_time) = value["dateTime"].as_str() {
        let parsed = DateTime::parse_from_rfc3339(date_time)
            .map_err(|_| invalid("invalid calendar event timestamp"))?;
        let ms = parsed.timestamp_millis();
        instant(ms)?;
        return Ok((ms, false));
    }
    let date = value["date"]
        .as_str()
        .ok_or_else(|| invalid("calendar event date missing"))?;
    let date = NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .map_err(|_| invalid("invalid calendar all-day date"))?;
    let zone: Tz = zone
        .parse()
        .map_err(|_| invalid("unrecognized calendar timezone"))?;
    // Do not guess on rare midnight transitions: ambiguous/nonexistent dates fail atomically.
    let local = zone
        .from_local_datetime(&date.and_hms_opt(0, 0, 0).unwrap())
        .single()
        .ok_or_else(|| invalid("calendar all-day midnight cannot be resolved unambiguously"))?;
    let ms = local.timestamp_millis();
    instant(ms)?;
    Ok((ms, true))
}
fn parse_event(
    value: &Value,
    account_id: &str,
    calendar_id: &str,
    zone: &str,
) -> Result<Option<AgendaEvent>> {
    let status = value["status"].as_str().unwrap_or("confirmed");
    // Cancelled recurring instances may contain only an ID. A full window replacement
    // removes old copies, so do not invent timestamps for these tombstones.
    if status == "cancelled" {
        return Ok(None);
    }
    if !matches!(status, "confirmed" | "tentative") {
        return Err(invalid("unknown calendar event status"));
    }
    let id = text(value, "id", 2048)?.ok_or_else(|| invalid("event ID missing"))?;
    identifier(&id)?;
    let (start_ms, all_day) = parse_time(&value["start"], zone)?;
    let (end_ms, end_all_day) = parse_time(&value["end"], zone)?;
    if start_ms >= end_ms || all_day != end_all_day {
        return Err(invalid("invalid calendar event interval"));
    }
    let raw_attendees = value.get("attendees").and_then(Value::as_array);
    if raw_attendees.is_some_and(|a| a.len() > MAX_ATTENDEES) {
        return Err(invalid("calendar attendee limit exceeded"));
    }
    let mut attendees = Vec::new();
    let mut can_respond = false;
    for attendee in raw_attendees.into_iter().flatten() {
        // Legacy/deleted guests and resources can lack a representable mailbox.
        // Keep the valid event without inventing or repairing guest identities.
        let Some(email) = attendee["email"].as_str().filter(|email| mailbox(email)) else {
            continue;
        };
        can_respond |= attendee["self"] == true && attendee["organizer"] != true;
        attendees.push(AgendaAttendee {
            email: email.to_owned(),
            name: text(attendee, "displayName", 1024)?,
            response: text(attendee, "responseStatus", 64)?,
        });
    }
    let html_url = text(value, "htmlLink", 4096)?
        .filter(|s| Url::parse(s).is_ok_and(|u| u.scheme() == "https" && u.host_str().is_some()));
    Ok(Some(AgendaEvent {
        id,
        calendar_id: calendar_id.to_owned(),
        account_id: account_id.to_owned(),
        title: text(value, "summary", 16_384)?.unwrap_or_else(|| "Busy".into()),
        description: text(value, "description", 16_384)?,
        location: text(value, "location", 16_384)?,
        start_ms,
        end_ms,
        all_day,
        status: status.to_owned(),
        organizer_email: text(&value["organizer"], "email", 254)?,
        attendees,
        html_url,
        etag: text(value, "etag", 2048)?,
        transparent: value["transparency"] == "transparent",
        can_respond,
        can_edit: editable_event(value),
        recurring_event_id: text(value, "recurringEventId", 2048)?,
    }))
}
fn response_patch(value: &Value, signed_in_email: &str, response: &str) -> Result<(String, Value)> {
    if !matches!(response, "accepted" | "declined" | "tentative") {
        return Err(invalid("choose accepted, declined or tentative"));
    }
    if value["status"] == "cancelled" {
        return Err(invalid("cannot respond to a cancelled event"));
    }
    let attendees = value["attendees"]
        .as_array()
        .ok_or_else(|| invalid("this event has no invitation for your account"))?;
    if attendees.len() > MAX_ATTENDEES {
        return Err(invalid("calendar attendee limit exceeded"));
    }
    let mut matches = attendees.iter().filter(|a| {
        a["self"] == true
            && a["organizer"] != true
            && a["email"]
                .as_str()
                .is_some_and(|email| email.eq_ignore_ascii_case(signed_in_email))
    });
    let attendee = matches
        .next()
        .ok_or_else(|| invalid("this event has no invitation for your account"))?;
    if matches.next().is_some() {
        return Err(invalid("ambiguous calendar invitation"));
    }
    let etag = text(value, "etag", 2048)?
        .filter(|s| !s.is_empty() && !s.chars().any(char::is_control))
        .ok_or_else(|| invalid("event version missing; refresh before responding"))?;
    // Google explicitly supports updating only the participant response with
    // attendeesOmitted=true. A partial array without that flag would delete guests.
    Ok((
        etag,
        json!({"attendeesOmitted":true,"attendees":[{"email":attendee["email"],"responseStatus":response}]}),
    ))
}

impl CloudClient {
    fn require_google_calendar(&self) -> Result<()> {
        if self.provider() == CloudProvider::Gmail {
            Ok(())
        } else {
            Err(CloudError::ProviderMismatch)
        }
    }
    /// The authenticated Google subject/email, without requiring Gmail permissions.
    pub fn google_calendar_identity(&self) -> Result<(String, String)> {
        self.require_google_calendar()?;
        parse_identity(&self.get("https://openidconnect.googleapis.com/v1/userinfo")?)
    }
    pub fn google_calendars(&self, account_id: &str) -> Result<Vec<CalendarCollection>> {
        self.require_google_calendar()?;
        identifier(account_id)?;
        let mut result = Vec::new();
        let mut token: Option<String> = None;
        let mut seen = HashSet::new();
        for _ in 0..8 {
            let mut url = Url::parse(&format!("{BASE}/users/me/calendarList"))?;
            url.query_pairs_mut()
                .append_pair("maxResults", "250")
                .append_pair("minAccessRole", "reader");
            if let Some(token) = &token {
                url.query_pairs_mut().append_pair("pageToken", token);
            }
            let page = self.get(url.as_str())?;
            let items = page["items"]
                .as_array()
                .ok_or_else(|| invalid("calendar list missing items"))?;
            for item in items {
                if let Some(calendar) = parse_collection(item, account_id)? {
                    result.push(calendar);
                }
                if result.len() > MAX_CALENDARS {
                    return Err(invalid("calendar list exceeds 256 calendars"));
                }
            }
            token = next_page(&page, &mut seen)?;
            if token.is_none() {
                return Ok(result);
            }
        }
        Err(invalid("calendar list pagination limit exceeded"))
    }
    pub fn google_calendar_events(
        &self,
        account_id: &str,
        calendar_id: &str,
        start_ms: i64,
        end_ms: i64,
    ) -> Result<Vec<AgendaEvent>> {
        self.require_google_calendar()?;
        identifier(account_id)?;
        listing_window(start_ms, end_ms)?;
        let start = instant(start_ms)?.to_rfc3339_opts(SecondsFormat::Secs, true);
        let end = instant(end_ms)?.to_rfc3339_opts(SecondsFormat::Secs, true);
        let mut result = Vec::new();
        let mut token: Option<String> = None;
        let mut seen = HashSet::new();
        for _ in 0..20 {
            let mut url = event_url(calendar_id, None)?;
            // Retain the response calendar timezone for correct all-day boundaries.
            // Timed instances carry RFC3339 offsets and are normalized to UTC millis.
            url.query_pairs_mut()
                .append_pair("timeMin", &start)
                .append_pair("timeMax", &end)
                .append_pair("singleEvents", "true")
                .append_pair("showDeleted", "false")
                .append_pair("orderBy", "startTime")
                .append_pair("maxResults", "1000");
            if let Some(token) = &token {
                url.query_pairs_mut().append_pair("pageToken", token);
            }
            let page = self.get(url.as_str())?;
            let zone = page["timeZone"]
                .as_str()
                .ok_or_else(|| invalid("calendar timezone missing"))?;
            let items = page["items"]
                .as_array()
                .ok_or_else(|| invalid("event list missing items"))?;
            for item in items {
                if let Some(event) = parse_event(item, account_id, calendar_id, zone)? {
                    result.push(event);
                }
                if result.len() > MAX_EVENTS {
                    return Err(invalid("calendar window exceeds 10000 events"));
                }
            }
            token = next_page(&page, &mut seen)?;
            if token.is_none() {
                return Ok(result);
            }
        }
        Err(invalid("calendar event pagination limit exceeded"))
    }
    /// Respond only as the signed-in attendee. This sends Google's update to guests.
    /// A concurrent event edit produces HTTP 412; never retry a write blindly.
    pub fn google_calendar_respond(
        &self,
        calendar_id: &str,
        event_id: &str,
        response: &str,
    ) -> Result<()> {
        self.require_google_calendar()?;
        if !matches!(response, "accepted" | "declined" | "tentative") {
            return Err(invalid("choose accepted, declined or tentative"));
        }
        let mut url = event_url(calendar_id, Some(event_id))?;
        let (_, email) = self.google_calendar_identity()?;
        let current = self.get(url.as_str())?;
        let (etag, patch) = response_patch(&current, &email, response)?;
        url.query_pairs_mut().append_pair("sendUpdates", "all");
        self.request(
            self.http
                .patch(url.as_str())
                .header(reqwest::header::IF_MATCH, etag)
                .json(&patch),
        )?;
        Ok(())
    }
    /// Modify one organizer-owned timed event. Never retry a calendar write automatically.
    pub fn google_calendar_update(
        &self,
        account_id: &str,
        calendar_id: &str,
        event_id: &str,
        expected_etag: &str,
        edit_scope: &str,
        changes: &CalendarEventChanges,
    ) -> Result<AgendaEvent> {
        self.require_google_calendar()?;
        identifier(account_id)?;
        let patch = changes.validate(expected_etag, edit_scope)?;
        let url = event_url(calendar_id, Some(event_id))?;
        let current = self.get(url.as_str())?;
        validate_edit_target(&current, expected_etag, edit_scope)?;
        let result = self
            .request(conditional_event_patch(
                &self.http,
                url,
                expected_etag,
                &patch,
            ))
            .map_err(|e| {
                if matches!(e, CloudError::ApiStatus(412)) {
                    CloudError::CalendarConflict
                } else {
                    e
                }
            })?;
        parse_event(&result, account_id, calendar_id, "UTC")?
            .ok_or_else(|| invalid("updated event missing from response"))
    }
    /// Create a single timed event and send invitations to explicit recipients.
    pub fn google_calendar_create(
        &self,
        account_id: &str,
        calendar_id: &str,
        title: &str,
        start_ms: i64,
        end_ms: i64,
        attendee_emails: &[String],
        description: Option<&str>,
    ) -> Result<AgendaEvent> {
        self.google_calendar_create_with_location(
            account_id,
            calendar_id,
            title,
            start_ms,
            end_ms,
            attendee_emails,
            description,
            None,
        )
    }
    #[allow(clippy::too_many_arguments)]
    pub fn google_calendar_create_with_location(
        &self,
        account_id: &str,
        calendar_id: &str,
        title: &str,
        start_ms: i64,
        end_ms: i64,
        attendee_emails: &[String],
        description: Option<&str>,
        location: Option<&str>,
    ) -> Result<AgendaEvent> {
        self.require_google_calendar()?;
        identifier(account_id)?;
        window(start_ms, end_ms)?;
        if title.trim().is_empty()
            || title.len() > 16_384
            || title.chars().any(char::is_control)
            || description.is_some_and(|s| {
                s.len() > 16_384 || s.chars().any(|c| c.is_control() && c != '\n' && c != '\t')
            })
            || location.is_some_and(|s| {
                s.len() > 16_384 || s.chars().any(|c| c.is_control() && c != '\n' && c != '\t')
            })
            || attendee_emails.len() > 100
            || attendee_emails.iter().any(|s| !mailbox(s))
        {
            return Err(invalid("invalid new calendar event"));
        }
        let mut random = [0u8; 16];
        OsRng.fill_bytes(&mut random);
        let id = random
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        let mut body = json!({"id":id,"summary":title.trim(),
            "start":{"dateTime":instant(start_ms)?.to_rfc3339_opts(SecondsFormat::Millis,true)},
            "end":{"dateTime":instant(end_ms)?.to_rfc3339_opts(SecondsFormat::Millis,true)},
            "attendees":attendee_emails.iter().map(|email| json!({"email":email})).collect::<Vec<_>>()});
        if let Some(description) = description {
            body["description"] = json!(description);
        }
        if let Some(location) = location {
            body["location"] = json!(location);
        }
        let mut url = event_url(calendar_id, None)?;
        url.query_pairs_mut().append_pair("sendUpdates", "all");
        let result = self.request(self.http.post(url.as_str()).json(&body))?;
        parse_event(&result, account_id, calendar_id, "UTC")?
            .ok_or_else(|| invalid("created event missing from response"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn event() -> Value {
        json!({"id":"abc","summary":"Planning","etag":"\"v1\"", "status":"confirmed",
        "start":{"dateTime":"2026-11-09T10:00:00+01:00"},"end":{"dateTime":"2026-11-09T11:00:00+01:00"},
        "attendees":[{"email":"me@example.com","self":true,"responseStatus":"needsAction"},{"email":"other@example.com"}]})
    }
    #[test]
    fn edit_transport_uses_patch_exact_version_and_guest_notifications() {
        let patch = json!({"summary":"Revised"});
        let request = conditional_event_patch(
            &reqwest::blocking::Client::new(),
            event_url("shared/calendar@example.com", Some("instance/id")).unwrap(),
            "\"version-2\"",
            &patch,
        )
        .build()
        .unwrap();
        assert_eq!(request.method(), reqwest::Method::PATCH);
        assert_eq!(
            request.headers()[reqwest::header::IF_MATCH],
            "\"version-2\""
        );
        assert_eq!(request.url().query(), Some("sendUpdates=all"));
        assert!(request.url().path().ends_with("/events/instance%2Fid"));
        let body: Value =
            serde_json::from_slice(request.body().unwrap().as_bytes().unwrap()).unwrap();
        assert_eq!(body, patch);
    }
    #[test]
    fn editing_patches_only_explicit_fields_and_preserves_guests() {
        let mut raw = event();
        raw["organizer"] = json!({"self":true,"email":"owner@example.com"});
        raw["conferenceData"] = json!({"entryPoints":[{"uri":"https://meet.google.com/test"}]});
        let original = raw.clone();
        let changes = CalendarEventChanges {
            title: Some("New title".into()),
            location: Some(String::new()),
            ..Default::default()
        };
        validate_edit_target(&raw, "\"v1\"", "single").unwrap();
        let patch = changes.validate("\"v1\"", "single").unwrap();
        assert_eq!(patch, json!({"summary":"New title","location":""}));
        assert_eq!(raw, original);
        assert!(patch.get("attendees").is_none());
        assert!(patch.get("conferenceData").is_none());
        assert!(
            parse_event(&raw, "a", "c", "UTC")
                .unwrap()
                .unwrap()
                .can_edit
        );
    }
    #[test]
    fn editing_requires_exact_version_and_rejects_unsafe_targets() {
        let mut raw = event();
        raw["organizer"] = json!({"self":true});
        assert!(matches!(
            validate_edit_target(&raw, "\"stale\"", "single"),
            Err(CloudError::CalendarConflict)
        ));
        for (field, value) in [
            ("locked", json!(true)),
            ("recurrence", json!(["RRULE:FREQ=WEEKLY"])),
            ("eventType", json!("outOfOffice")),
            ("status", json!("cancelled")),
        ] {
            let mut restricted = raw.clone();
            restricted[field] = value;
            assert!(
                validate_edit_target(&restricted, "\"v1\"", "single").is_err(),
                "{field}"
            );
            assert!(!editable_event(&restricted), "{field}");
        }
        raw["organizer"]["self"] = json!(false);
        assert!(validate_edit_target(&raw, "\"v1\"", "single").is_err());
        raw["organizer"]["self"] = json!(true);
        raw["start"] = json!({"date":"2026-11-09"});
        assert!(validate_edit_target(&raw, "\"v1\"", "single").is_err());
    }
    #[test]
    fn recurring_edits_require_explicit_instance_and_never_patch_series() {
        let mut raw = event();
        raw["organizer"] = json!({"self":true});
        raw["recurringEventId"] = json!("series");
        assert!(validate_edit_target(&raw, "\"v1\"", "instance").is_err());
        raw["originalStartTime"] = json!({"dateTime":"2026-11-09T10:00:00+01:00"});
        assert!(validate_edit_target(&raw, "\"v1\"", "single").is_err());
        validate_edit_target(&raw, "\"v1\"", "instance").unwrap();
        let parsed = parse_event(&raw, "a", "c", "UTC").unwrap().unwrap();
        assert_eq!(parsed.recurring_event_id.as_deref(), Some("series"));
        raw.as_object_mut().unwrap().remove("recurringEventId");
        assert!(validate_edit_target(&raw, "\"v1\"", "instance").is_err());
    }
    #[test]
    fn editing_rejects_missing_versions_partial_times_and_empty_changes() {
        let title = CalendarEventChanges {
            title: Some("Changed".into()),
            ..Default::default()
        };
        for version in ["", " ", "v1\r\nIf-Match: *"] {
            assert!(title.validate(version, "single").is_err());
        }
        assert!(title.validate("v1", "series").is_err());
        assert!(
            CalendarEventChanges::default()
                .validate("v1", "single")
                .is_err()
        );
        let start = 1_794_218_400_000;
        let mut changes = CalendarEventChanges {
            start_ms: Some(start),
            ..Default::default()
        };
        assert!(changes.validate("v1", "single").is_err());
        changes.end_ms = Some(start);
        assert!(changes.validate("v1", "single").is_err());
        changes.end_ms = Some(start + 3_600_000);
        let patch = changes.validate("v1", "single").unwrap();
        assert!(patch["start"]["dateTime"].as_str().unwrap().ends_with('Z'));
        assert!(patch.get("attendees").is_none());
        changes.description = Some("hello\0world".into());
        assert!(changes.validate("v1", "single").is_err());
    }
    #[test]
    fn timed_events_use_offsets_and_keep_identity() {
        let parsed = parse_event(&event(), "account", "calendar", "Europe/Oslo")
            .unwrap()
            .unwrap();
        assert_eq!(
            parsed.start_ms,
            DateTime::parse_from_rfc3339("2026-11-09T09:00:00Z")
                .unwrap()
                .timestamp_millis()
        );
        assert_eq!(parsed.account_id, "account");
        assert!(parsed.can_respond);
        assert!(!parsed.transparent);
    }
    #[test]
    fn unrepresentable_provider_guests_do_not_hide_valid_events() {
        let mut raw = event();
        raw["attendees"] = json!([
            {"email":"me@example.com","self":true,"responseStatus":"needsAction"},
            {"email":"meeting-room@resource.calendar.google.com","resource":true},
            {"id":"deleted-guest","displayName":"Former guest"},
            {"email":null},
            {"email":42},
            {"email":"legacy room identifier","resource":true},
            {"email":"guest\r\nBcc: injected@example.com"},
            {"email":"x".repeat(255)},
            {"email":"other@example.com"}
        ]);
        let parsed = parse_event(&raw, "account", "calendar", "UTC")
            .unwrap()
            .unwrap();
        assert_eq!(parsed.title, "Planning");
        assert_eq!(parsed.account_id, "account");
        assert_eq!(
            parsed
                .attendees
                .iter()
                .map(|a| a.email.as_str())
                .collect::<Vec<_>>(),
            vec![
                "me@example.com",
                "meeting-room@resource.calendar.google.com",
                "other@example.com"
            ]
        );
        assert!(parsed.can_respond);
        let (_, patch) = response_patch(&raw, "me@example.com", "accepted").unwrap();
        assert_eq!(
            patch,
            json!({"attendeesOmitted":true,"attendees":[{"email":"me@example.com","responseStatus":"accepted"}]})
        );
    }

    #[test]
    fn malformed_self_guest_never_enables_or_redirects_rsvp() {
        let mut raw = event();
        raw["attendees"] = json!([
            {"email":"unrepresentable-self","self":true},
            {"email":"other@example.com"}
        ]);
        let parsed = parse_event(&raw, "a", "c", "UTC").unwrap().unwrap();
        assert!(!parsed.can_respond);
        assert_eq!(parsed.attendees.len(), 1);
        assert!(response_patch(&raw, "me@example.com", "accepted").is_err());
        assert!(response_patch(&raw, "other@example.com", "accepted").is_err());
    }

    #[test]
    fn all_day_uses_calendar_timezone_and_exclusive_end() {
        let raw = json!({"id":"day","start":{"date":"2026-03-29"},"end":{"date":"2026-03-30"}});
        let parsed = parse_event(&raw, "a", "c", "Europe/Oslo").unwrap().unwrap();
        assert!(parsed.all_day);
        assert_eq!(parsed.end_ms - parsed.start_ms, 23 * 3_600_000);
        assert!(parse_event(&raw, "a", "c", "not/a-zone").is_err());
    }
    #[test]
    fn cancelled_tombstones_are_not_fake_events() {
        assert!(
            parse_event(&json!({"id":"gone","status":"cancelled"}), "a", "c", "UTC")
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn transparent_and_private_busy_event_parse() {
        let raw = json!({"id":"private","transparency":"transparent","start":{"dateTime":"2026-11-09T10:00:00Z"},"end":{"dateTime":"2026-11-09T11:00:00Z"}});
        let parsed = parse_event(&raw, "a", "c", "UTC").unwrap().unwrap();
        assert_eq!(parsed.title, "Busy");
        assert!(parsed.transparent);
    }
    #[test]
    fn rsvp_changes_only_verified_self_response() {
        let (etag, patch) = response_patch(&event(), "ME@example.com", "accepted").unwrap();
        assert_eq!(etag, "\"v1\"");
        assert_eq!(
            patch,
            json!({"attendeesOmitted":true,"attendees":[{"email":"me@example.com","responseStatus":"accepted"}]})
        );
        assert!(response_patch(&event(), "other@example.com", "accepted").is_err());
        assert!(response_patch(&event(), "me@example.com", "bogus").is_err());
        let mut missing = event();
        missing.as_object_mut().unwrap().remove("etag");
        assert!(response_patch(&missing, "me@example.com", "accepted").is_err());
    }
    #[test]
    fn identity_requires_verified_email() {
        assert!(
            parse_identity(&json!({"sub":"123","email":"me@example.com","email_verified":true}))
                .is_ok()
        );
        assert!(
            parse_identity(&json!({"sub":"123","email":"me@example.com","email_verified":false}))
                .is_err()
        );
    }
    #[test]
    fn oversized_collection_and_attendee_names_fail_before_cache_mutation() {
        let long = "x".repeat(1025);
        assert!(
            parse_collection(&json!({"id":"c","accessRole":"reader","summary":long}), "a").is_err()
        );
        let mut raw = event();
        raw["attendees"][0]["displayName"] = json!(long);
        assert!(parse_event(&raw, "a", "c", "UTC").is_err());
    }

    #[test]
    fn event_creation_rejects_invalid_input_before_network() {
        let client = CloudClient::new(
            crate::OAuthConfig {
                provider: CloudProvider::Gmail,
                client_id: "test-calendar-client".into(),
                client_secret: None,
            },
            crate::TokenSet {
                access_token: "test".into(),
                refresh_token: None,
                expires_at_ms: i64::MAX,
            },
        )
        .unwrap();
        let start = 1_794_218_400_000;
        assert!(
            client
                .google_calendar_create("a", "primary", "", start, start + 60_000, &[], None)
                .is_err()
        );
        assert!(
            client
                .google_calendar_create("a", "primary", "Meeting", start, start, &[], None)
                .is_err()
        );
        assert!(
            client
                .google_calendar_create(
                    "a",
                    "primary",
                    "Meeting",
                    start,
                    start + 60_000,
                    &["person@example.com\r\nBcc:other@example.com".into()],
                    None
                )
                .is_err()
        );
        assert!(
            client
                .google_calendar_create(
                    "a",
                    "primary",
                    "Meeting",
                    start,
                    start + 60_000,
                    &[".@example.com".into()],
                    None
                )
                .is_err()
        );
    }

    #[test]
    fn calendar_roles_and_pagination_are_bounded() {
        assert!(
            parse_collection(
                &json!({"id":"shared","accessRole":"reader","summary":"Shared"}),
                "a"
            )
            .unwrap()
            .is_some_and(|c| !c.can_write)
        );
        assert!(
            parse_collection(&json!({"id":"busy","accessRole":"freeBusyReader"}), "a")
                .unwrap()
                .is_none()
        );
        let mut seen = HashSet::new();
        assert_eq!(
            next_page(&json!({"nextPageToken":"next"}), &mut seen).unwrap(),
            Some("next".into())
        );
        assert!(next_page(&json!({"nextPageToken":"next"}), &mut seen).is_err());
        assert!(window(0, MAX_WINDOW_MS + 1).is_err());
        assert!(listing_window(0, 366 * 86_400_000).is_ok());
        assert!(listing_window(0, MAX_LISTING_WINDOW_MS).is_ok());
        assert!(listing_window(0, MAX_LISTING_WINDOW_MS + 1).is_err());
        assert!(listing_window(i64::MIN, i64::MAX).is_err());
        assert!(
            event_url("a/b@example.com", Some("x?y"))
                .unwrap()
                .as_str()
                .contains("a%2Fb@example.com")
        );
    }
}
