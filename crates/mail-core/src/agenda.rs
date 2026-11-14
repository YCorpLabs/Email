//! Cached connected-calendar data. Invitation documents are deliberately separate.
//! Availability is derived only from this cache, never a promise of live free/busy.
use crate::{MailError, MailStore, Result};
use rusqlite::{Connection, OptionalExtension, params};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

const DAY: i64 = 86_400_000;
const MAX_WINDOW: i64 = 93 * DAY;
const MAX_LISTING_WINDOW: i64 = 400 * DAY;
const MIN_TIME: i64 = -62_135_596_800_000;
const MAX_TIME: i64 = 253_402_300_800_000;
const MAX_EVENTS: usize = 10_000;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct CalendarCollection {
    pub id: String,
    pub account_id: String,
    pub name: String,
    pub primary: bool,
    pub can_write: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct AgendaAttendee {
    pub email: String,
    pub name: Option<String>,
    pub response: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct AgendaEvent {
    pub id: String,
    pub calendar_id: String,
    pub account_id: String,
    pub title: String,
    pub description: Option<String>,
    pub location: Option<String>,
    pub start_ms: i64,
    pub end_ms: i64,
    pub all_day: bool,
    pub status: String,
    pub organizer_email: Option<String>,
    pub attendees: Vec<AgendaAttendee>,
    pub html_url: Option<String>,
    pub etag: Option<String>,
    #[serde(default)]
    pub transparent: bool,
    #[serde(default)]
    pub can_respond: bool,
    #[serde(default)]
    pub can_edit: bool,
    #[serde(default)]
    pub recurring_event_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct AvailabilitySlot {
    pub start_ms: i64,
    pub end_ms: i64,
}

pub(crate) fn initialize(db: &Connection) -> Result<()> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS calendar_collections(
        account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
        id TEXT NOT NULL,name TEXT NOT NULL,primary_calendar INTEGER NOT NULL,
        can_write INTEGER NOT NULL,PRIMARY KEY(account_id,id));
      CREATE TABLE IF NOT EXISTS agenda_events(
        account_id TEXT NOT NULL,calendar_id TEXT NOT NULL,id TEXT NOT NULL,
        start_ms INTEGER NOT NULL,end_ms INTEGER NOT NULL,event_json TEXT NOT NULL,
        PRIMARY KEY(account_id,calendar_id,id),
        FOREIGN KEY(account_id,calendar_id) REFERENCES calendar_collections(account_id,id) ON DELETE CASCADE);
      CREATE INDEX IF NOT EXISTS agenda_events_window ON agenda_events(start_ms,end_ms,account_id,calendar_id);
      CREATE TABLE IF NOT EXISTS agenda_windows(
        account_id TEXT NOT NULL,calendar_id TEXT NOT NULL,start_ms INTEGER NOT NULL,end_ms INTEGER NOT NULL,refreshed_ms INTEGER NOT NULL DEFAULT 0,
        PRIMARY KEY(account_id,calendar_id),
        FOREIGN KEY(account_id,calendar_id) REFERENCES calendar_collections(account_id,id) ON DELETE CASCADE);")?;
    let columns = db
        .prepare("PRAGMA table_info(agenda_windows)")?
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if !columns.iter().any(|c| c == "refreshed_ms") {
        db.execute_batch(
            "ALTER TABLE agenda_windows ADD COLUMN refreshed_ms INTEGER NOT NULL DEFAULT 0",
        )?;
    }
    Ok(())
}

fn invalid(reason: &str) -> MailError {
    MailError::InvalidAction(reason.into())
}
fn text(value: &str, max: usize, empty: bool) -> Result<()> {
    if (!empty && value.trim().is_empty())
        || value.len() > max
        || value
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\r' && c != '\t')
    {
        return Err(invalid("Invalid or oversized calendar text"));
    }
    Ok(())
}
fn id(value: &str) -> Result<()> {
    text(value, 2048, false)?;
    if value.chars().any(char::is_control) {
        return Err(invalid("Invalid calendar identifier"));
    }
    Ok(())
}
fn listing_window(start: i64, end: i64) -> Result<()> {
    if start < MIN_TIME || end > MAX_TIME || end <= start || end - start > MAX_LISTING_WINDOW {
        return Err(invalid(
            "Calendar listing window must be positive and no longer than 400 days",
        ));
    }
    Ok(())
}
fn window(start: i64, end: i64) -> Result<()> {
    if start < MIN_TIME || end > MAX_TIME || end <= start || end - start > MAX_WINDOW {
        return Err(invalid(
            "Calendar window must be positive and no longer than 93 days",
        ));
    }
    Ok(())
}
fn account(db: &Connection, account_id: &str) -> Result<()> {
    id(account_id)?;
    if !db.query_row(
        "SELECT EXISTS(SELECT 1 FROM accounts WHERE id=?1)",
        [account_id],
        |r| r.get::<_, bool>(0),
    )? {
        return Err(MailError::UnknownAccount(account_id.into()));
    }
    Ok(())
}
fn collection(db: &Connection, account_id: &str, calendar_id: &str) -> Result<()> {
    account(db, account_id)?;
    id(calendar_id)?;
    if !db.query_row(
        "SELECT EXISTS(SELECT 1 FROM calendar_collections WHERE account_id=?1 AND id=?2)",
        params![account_id, calendar_id],
        |r| r.get::<_, bool>(0),
    )? {
        return Err(MailError::NotFound(
            "Calendar not found for this account".into(),
        ));
    }
    Ok(())
}
fn validate_event(event: &AgendaEvent) -> Result<()> {
    id(&event.id)?;
    id(&event.calendar_id)?;
    id(&event.account_id)?;
    if event.start_ms < MIN_TIME || event.end_ms > MAX_TIME || event.end_ms <= event.start_ms {
        return Err(invalid("Calendar event needs valid start and end instants"));
    }
    text(&event.title, 16 * 1024, true)?;
    text(&event.status, 64, false)?;
    for value in [
        &event.description,
        &event.location,
        &event.organizer_email,
        &event.html_url,
        &event.etag,
    ]
    .into_iter()
    .flatten()
    {
        text(value, 16 * 1024, true)?;
    }
    if event.attendees.len() > 500 {
        return Err(invalid("Too many calendar attendees"));
    }
    for attendee in &event.attendees {
        text(&attendee.email, 254, false)?;
        if attendee.email.chars().any(char::is_control) {
            return Err(invalid("Invalid calendar attendee"));
        }
        if let Some(name) = &attendee.name {
            text(name, 1024, true)?;
        }
        if let Some(response) = &attendee.response {
            text(response, 64, true)?;
        }
    }
    if serde_json::to_vec(event)?.len() > 128 * 1024 {
        return Err(invalid("Calendar event exceeds cache limit"));
    }
    Ok(())
}

fn validate_calendars<'a>(
    account_id: &str,
    calendars: &'a [CalendarCollection],
) -> Result<HashSet<&'a str>> {
    if calendars.len() > 256 {
        return Err(invalid("Too many connected calendars"));
    }
    let mut ids = HashSet::new();
    for calendar in calendars {
        id(&calendar.id)?;
        text(&calendar.name, 1024, true)?;
        if calendar.account_id != account_id || !ids.insert(calendar.id.as_str()) {
            return Err(invalid("Calendar account mismatch or duplicate identifier"));
        }
    }
    Ok(ids)
}

fn validate_events(
    account_id: &str,
    calendar_id: &str,
    start_ms: i64,
    end_ms: i64,
    events: &[AgendaEvent],
) -> Result<()> {
    listing_window(start_ms, end_ms)?;
    if events.len() > MAX_EVENTS {
        return Err(invalid("Calendar window exceeds 10000 events"));
    }
    let mut ids = HashSet::new();
    for event in events {
        validate_event(event)?;
        if event.account_id != account_id
            || event.calendar_id != calendar_id
            || !ids.insert(&event.id)
            || event.end_ms <= start_ms
            || event.start_ms >= end_ms
        {
            return Err(invalid(
                "Event does not belong to requested calendar window",
            ));
        }
    }
    Ok(())
}

// Apply helpers accept the existing transaction through Connection's deref;
// callers always own transaction boundaries and validate before any mutation.
fn apply_calendars(
    db: &Connection,
    account_id: &str,
    calendars: &[CalendarCollection],
    ids: &HashSet<&str>,
) -> Result<()> {
    let existing = {
        let mut q = db.prepare("SELECT id FROM calendar_collections WHERE account_id=?1")?;
        q.query_map([account_id], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?
    };
    for old in existing {
        if !ids.contains(old.as_str()) {
            db.execute(
                "DELETE FROM calendar_collections WHERE account_id=?1 AND id=?2",
                params![account_id, old],
            )?;
        }
    }
    for calendar in calendars {
        db.execute("INSERT INTO calendar_collections(account_id,id,name,primary_calendar,can_write) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(account_id,id) DO UPDATE SET name=excluded.name,primary_calendar=excluded.primary_calendar,can_write=excluded.can_write",params![account_id,calendar.id,calendar.name,calendar.primary,calendar.can_write])?;
    }
    Ok(())
}

fn apply_event_window(
    db: &Connection,
    account_id: &str,
    calendar_id: &str,
    start_ms: i64,
    end_ms: i64,
    events: &[AgendaEvent],
    refreshed_ms: i64,
) -> Result<()> {
    db.execute("DELETE FROM agenda_events WHERE account_id=?1 AND calendar_id=?2 AND start_ms<?4 AND end_ms>?3",params![account_id,calendar_id,start_ms,end_ms])?;
    for event in events {
        db.execute("INSERT INTO agenda_events(account_id,calendar_id,id,start_ms,end_ms,event_json) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(account_id,calendar_id,id) DO UPDATE SET start_ms=excluded.start_ms,end_ms=excluded.end_ms,event_json=excluded.event_json",params![account_id,calendar_id,event.id,event.start_ms,event.end_ms,serde_json::to_string(event)?])?;
    }
    // Only the latest full window is certified; preserve older readable events
    // while conservatively dropping older/disjoint freshness certificates.
    db.execute("INSERT INTO agenda_windows(account_id,calendar_id,start_ms,end_ms,refreshed_ms) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(account_id,calendar_id) DO UPDATE SET start_ms=excluded.start_ms,end_ms=excluded.end_ms,refreshed_ms=excluded.refreshed_ms",params![account_id,calendar_id,start_ms,end_ms,refreshed_ms])?;
    Ok(())
}

impl MailStore {
    /// An attempted remote mutation can have succeeded even if its response was
    /// lost. Drop freshness certification immediately without hiding saved mail
    /// or calendar data; the next complete provider refresh restores coverage.
    pub fn invalidate_agenda_coverage(&self, account_id: &str, calendar_id: &str) -> Result<()> {
        let db = self.lock()?;
        collection(&db, account_id, calendar_id)?;
        db.execute(
            "DELETE FROM agenda_windows WHERE account_id=?1 AND calendar_id=?2",
            params![account_id, calendar_id],
        )?;
        Ok(())
    }

    /// Cache one successfully returned provider mutation. An individual event
    /// does not certify a complete free/busy window and invalidates old coverage.
    pub fn cache_agenda_event(&self, event: &AgendaEvent) -> Result<()> {
        validate_event(event)?;
        let mut db = self.lock()?;
        collection(&db, &event.account_id, &event.calendar_id)?;
        let tx = db.transaction()?;
        tx.execute(
            "DELETE FROM agenda_windows WHERE account_id=?1 AND calendar_id=?2",
            params![event.account_id, event.calendar_id],
        )?;
        tx.execute("INSERT INTO agenda_events(account_id,calendar_id,id,start_ms,end_ms,event_json) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(account_id,calendar_id,id) DO UPDATE SET start_ms=excluded.start_ms,end_ms=excluded.end_ms,event_json=excluded.event_json",params![event.account_id,event.calendar_id,event.id,event.start_ms,event.end_ms,serde_json::to_string(event)?])?;
        tx.commit()?;
        Ok(())
    }

    /// Drop one event the provider confirmed deleted. Like a cached mutation,
    /// it invalidates the calendar's coverage; other events stay readable.
    pub fn forget_agenda_event(
        &self,
        account_id: &str,
        calendar_id: &str,
        event_id: &str,
    ) -> Result<()> {
        let mut db = self.lock()?;
        collection(&db, account_id, calendar_id)?;
        let tx = db.transaction()?;
        tx.execute(
            "DELETE FROM agenda_windows WHERE account_id=?1 AND calendar_id=?2",
            params![account_id, calendar_id],
        )?;
        tx.execute(
            "DELETE FROM agenda_events WHERE account_id=?1 AND calendar_id=?2 AND id=?3",
            params![account_id, calendar_id, event_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Atomically refresh the account's complete connected-calendar list. Removed
    /// collections and their cached events are removed through foreign keys.
    pub fn replace_calendar_collections(
        &self,
        account_id: &str,
        calendars: &[CalendarCollection],
    ) -> Result<()> {
        let ids = validate_calendars(account_id, calendars)?;
        let mut db = self.lock()?;
        account(&db, account_id)?;
        let tx = db.transaction()?;
        apply_calendars(&tx, account_id, calendars, &ids)?;
        tx.commit()?;
        Ok(())
    }

    pub fn list_calendar_collections(
        &self,
        account_id: Option<&str>,
    ) -> Result<Vec<CalendarCollection>> {
        let db = self.lock()?;
        if let Some(a) = account_id {
            account(&db, a)?;
        }
        let mut q=db.prepare("SELECT id,account_id,name,primary_calendar,can_write FROM calendar_collections WHERE (?1 IS NULL OR account_id=?1) ORDER BY account_id,primary_calendar DESC,name,id")?;
        Ok(q.query_map([account_id], |r| {
            Ok(CalendarCollection {
                id: r.get(0)?,
                account_id: r.get(1)?,
                name: r.get(2)?,
                primary: r.get(3)?,
                can_write: r.get(4)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Commit a *complete* successful provider listing for this overlap window.
    /// Call only after all pages are fetched. Failed/partial fetches keep old data.
    pub fn replace_agenda_window(
        &self,
        account_id: &str,
        calendar_id: &str,
        start_ms: i64,
        end_ms: i64,
        events: &[AgendaEvent],
    ) -> Result<()> {
        validate_events(account_id, calendar_id, start_ms, end_ms, events)?;
        let mut db = self.lock()?;
        collection(&db, account_id, calendar_id)?;
        let tx = db.transaction()?;
        apply_event_window(
            &tx,
            account_id,
            calendar_id,
            start_ms,
            end_ms,
            events,
            crate::now_ms(),
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Commit all calendars and their complete event windows as one account
    /// snapshot. Models and exact one-window-per-calendar coverage are checked
    /// before mutation; any validation, serialization or database failure leaves
    /// the entire previous snapshot intact, including removed calendar caches.
    pub fn replace_agenda_snapshot(
        &self,
        account_id: &str,
        calendars: &[CalendarCollection],
        start_ms: i64,
        end_ms: i64,
        windows: &[(String, Vec<AgendaEvent>)],
    ) -> Result<()> {
        listing_window(start_ms, end_ms)?;
        let ids = validate_calendars(account_id, calendars)?;
        if windows.len() != calendars.len() {
            return Err(invalid(
                "Every calendar needs exactly one complete event window",
            ));
        }
        let mut seen = HashSet::new();
        for (calendar_id, events) in windows {
            if !ids.contains(calendar_id.as_str()) || !seen.insert(calendar_id.as_str()) {
                return Err(invalid(
                    "Calendar snapshot contains duplicate or unknown event windows",
                ));
            }
            validate_events(account_id, calendar_id, start_ms, end_ms, events)?;
        }
        let mut db = self.lock()?;
        account(&db, account_id)?;
        let tx = db.transaction()?;
        apply_calendars(&tx, account_id, calendars, &ids)?;
        let refreshed_ms = crate::now_ms();
        for (calendar_id, events) in windows {
            apply_event_window(
                &tx,
                account_id,
                calendar_id,
                start_ms,
                end_ms,
                events,
                refreshed_ms,
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// True only for a full-window snapshot fetched within the last 5 minutes.
    /// Cached events remain readable after this freshness check expires.
    pub fn agenda_window_coverage(
        &self,
        account_id: &str,
        calendar_id: &str,
        start_ms: i64,
        end_ms: i64,
    ) -> Result<bool> {
        listing_window(start_ms, end_ms)?;
        let db = self.lock()?;
        collection(&db, account_id, calendar_id)?;
        Ok(db.query_row("SELECT start_ms<=?3 AND end_ms>=?4 AND refreshed_ms>=?5 AND refreshed_ms<=?6 FROM agenda_windows WHERE account_id=?1 AND calendar_id=?2",params![account_id,calendar_id,start_ms,end_ms,crate::now_ms()-300_000,crate::now_ms()],|r|r.get(0)).optional()?.unwrap_or(false))
    }

    pub fn list_agenda_events(
        &self,
        account_id: Option<&str>,
        start_ms: i64,
        end_ms: i64,
        limit: usize,
    ) -> Result<Vec<AgendaEvent>> {
        listing_window(start_ms, end_ms)?;
        let db = self.lock()?;
        if let Some(a) = account_id {
            account(&db, a)?;
        }
        let mut q=db.prepare("SELECT event_json FROM agenda_events WHERE (?1 IS NULL OR account_id=?1) AND start_ms<?3 AND end_ms>?2 ORDER BY start_ms,end_ms,account_id,calendar_id,id LIMIT ?4")?;
        let mut rows = q.query(params![
            account_id,
            start_ms,
            end_ms,
            if limit >= MAX_EVENTS {
                MAX_EVENTS + 1
            } else {
                limit
            } as i64
        ])?;
        let mut events = Vec::new();
        while let Some(row) = rows.next()? {
            if events.len() == MAX_EVENTS {
                return Err(invalid(
                    "Too many cached calendar events; choose a shorter date range",
                ));
            }
            let raw: String = row.get(0)?;
            if raw.len() > 128 * 1024 {
                return Err(invalid("Cached calendar event exceeds limit"));
            }
            events.push(serde_json::from_str(&raw)?);
        }
        Ok(events)
    }

    /// Fixed-duration, non-overlapping candidate slots based only on cached busy
    /// events. No timezone/working-hours assumptions; caller must disclose cache
    /// coverage and obtain current provider free/busy before booking.
    pub fn availability_slots(
        &self,
        account_id: Option<&str>,
        start_ms: i64,
        end_ms: i64,
        duration_minutes: u32,
    ) -> Result<Vec<AvailabilitySlot>> {
        window(start_ms, end_ms)?;
        if !(5..=1440).contains(&duration_minutes) {
            return Err(invalid("Availability duration must be 5 to 1440 minutes"));
        }
        // Never truncate busy events and invent free time. One extra detects the
        // cap without materializing an unbounded mailbox/calendar query.
        let db = self.lock()?;
        if let Some(a) = account_id {
            account(&db, a)?;
        }
        let mut q=db.prepare("SELECT event_json FROM agenda_events WHERE (?1 IS NULL OR account_id=?1) AND start_ms<?3 AND end_ms>?2 ORDER BY start_ms,end_ms,account_id,calendar_id,id LIMIT 10001")?;
        let mut rows = q.query(params![account_id, start_ms, end_ms])?;
        let mut events = Vec::<AgendaEvent>::new();
        while let Some(row) = rows.next()? {
            if events.len() == MAX_EVENTS {
                return Err(invalid(
                    "Too many cached events to calculate availability safely",
                ));
            }
            let raw: String = row.get(0)?;
            if raw.len() > 128 * 1024 {
                return Err(invalid("Cached calendar event exceeds limit"));
            }
            events.push(serde_json::from_str(&raw)?);
        }
        let own_emails = {
            let mut q = db.prepare("SELECT id,email FROM accounts WHERE (?1 IS NULL OR id=?1)")?;
            q.query_map([account_id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<std::result::Result<HashMap<_, _>, _>>()?
        };
        let duration = i64::from(duration_minutes) * 60_000;
        let mut cursor = start_ms;
        let mut result = Vec::new();
        for event in events.into_iter().filter(|e| {
            !e.transparent
                && !e.status.eq_ignore_ascii_case("cancelled")
                && !e.status.eq_ignore_ascii_case("canceled")
                && !own_emails.get(&e.account_id).is_some_and(|email| {
                    e.attendees.iter().any(|a| {
                        a.email.eq_ignore_ascii_case(email)
                            && a.response
                                .as_deref()
                                .is_some_and(|response| response.eq_ignore_ascii_case("declined"))
                    })
                })
        }) {
            let busy_start = event.start_ms.max(start_ms);
            while cursor + duration <= busy_start && result.len() < 500 {
                result.push(AvailabilitySlot {
                    start_ms: cursor,
                    end_ms: cursor + duration,
                });
                cursor += duration;
            }
            cursor = cursor.max(event.end_ms).min(end_ms);
        }
        while cursor + duration <= end_ms && result.len() < 500 {
            result.push(AvailabilitySlot {
                start_ms: cursor,
                end_ms: cursor + duration,
            });
            cursor += duration;
        }
        Ok(result)
    }
}
