use super::{MailError, MailStore, Result};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use uuid::Uuid;

const MAX_IMAGE_EVENTS: usize = 100;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TrackingImageEvent {
    pub at_ms: i64,
    pub count: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TrackingDetail {
    pub expires_at: Option<i64>,
    pub refreshed_at_ms: Option<i64>,
    pub remote_count: u64,
    pub events: Vec<TrackingImageEvent>,
    pub timeline_complete: bool,
    pub sent_at_ms: Option<i64>,
}

/// Local header context only; never loads message bodies for the activity feed.
#[derive(Clone, Debug)]
pub struct TrackingMessage {
    pub id: String,
    pub thread_id: String,
    pub account_id: String,
    pub subject: String,
    pub to: Vec<String>,
    pub received_at: i64,
    pub is_sent: bool,
    pub is_draft: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EmailTracking {
    pub message_id: String,
    pub tracking_id: String,
    pub enabled: bool,
    pub host_id: Option<String>,
    pub pixel_requests: u64,
    pub first_requested_at: Option<i64>,
    pub last_requested_at: Option<i64>,
}
/// Tracked sent mail: enabled records on sent, non-draft messages.
const TRACKED_SENT: &str = "email_tracking t JOIN messages m ON m.id=t.message_id WHERE t.enabled=1 AND m.is_sent=1 AND m.is_draft=0";

pub(crate) fn schema(db: &Connection) -> Result<()> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS email_tracking(message_id TEXT PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE ON UPDATE CASCADE,tracking_id TEXT NOT NULL UNIQUE,enabled INTEGER NOT NULL DEFAULT 0,host_id TEXT,pixel_requests INTEGER NOT NULL DEFAULT 0,first_requested_at INTEGER,last_requested_at INTEGER);")?;
    db.execute_batch("CREATE TABLE IF NOT EXISTS email_tracking_detail(tracking_id TEXT PRIMARY KEY REFERENCES email_tracking(tracking_id) ON DELETE CASCADE,expires_at INTEGER,refreshed_at_ms INTEGER,remote_count INTEGER NOT NULL DEFAULT 0,events_json TEXT NOT NULL DEFAULT '[]',timeline_complete INTEGER NOT NULL DEFAULT 0,sent_at_ms INTEGER); CREATE INDEX IF NOT EXISTS email_tracking_detail_expiry ON email_tracking_detail(expires_at);")?;
    db.execute_batch("CREATE TABLE IF NOT EXISTS email_tracking_send_baseline(tracking_id TEXT NOT NULL REFERENCES email_tracking(tracking_id) ON DELETE CASCADE,message_id TEXT NOT NULL,PRIMARY KEY(tracking_id,message_id));")?;
    Ok(())
}
pub(crate) fn record_sent(db: &Connection, message_id: &str, at: i64) -> Result<()> {
    schema(db)?;
    db.execute("INSERT INTO email_tracking_detail(tracking_id,sent_at_ms) SELECT tracking_id,?2 FROM email_tracking WHERE message_id=?1 AND enabled=1 ON CONFLICT(tracking_id) DO UPDATE SET sent_at_ms=coalesce(email_tracking_detail.sent_at_ms,excluded.sent_at_ms)",params![message_id,at])?;
    // Snapshot every message already known at provider acceptance, irrespective of
    // its Date timestamp. Future-dated cached mail is not a new reply. The sent
    // anchor itself guarantees a nonempty baseline, even for an empty thread.
    let tracking: Option<String> = db
        .query_row(
            "SELECT tracking_id FROM email_tracking WHERE message_id=?1 AND enabled=1",
            [message_id],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(tracking) = tracking {
        let exists: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM email_tracking_send_baseline WHERE tracking_id=?1)",
            [&tracking],
            |r| r.get(0),
        )?;
        if !exists {
            db.execute("WITH anchor AS (SELECT account_id,coalesce((SELECT target_thread FROM local_conversation_links WHERE source_thread=m.thread_id),m.thread_id) AS canonical FROM messages m WHERE id=?1) INSERT INTO email_tracking_send_baseline(tracking_id,message_id) SELECT ?2,m.id FROM messages m,anchor a WHERE m.account_id=a.account_id AND m.thread_id IN (SELECT a.canonical UNION SELECT source_thread FROM local_conversation_links WHERE target_thread=a.canonical)",params![message_id,tracking])?;
        }
    }
    Ok(())
}
fn detail(db: &Connection, id: &str) -> Result<TrackingDetail> {
    let raw: Option<(Option<i64>,Option<i64>,u64,String,bool,Option<i64>)> = db.query_row("SELECT expires_at,refreshed_at_ms,remote_count,events_json,timeline_complete,sent_at_ms FROM email_tracking_detail WHERE tracking_id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).optional()?;
    match raw {
        Some((
            expires_at,
            refreshed_at_ms,
            remote_count,
            events,
            timeline_complete,
            sent_at_ms,
        )) => Ok(TrackingDetail {
            expires_at,
            refreshed_at_ms,
            remote_count,
            events: serde_json::from_str(&events)?,
            timeline_complete,
            sent_at_ms,
        }),
        None => Ok(TrackingDetail::default()),
    }
}
fn row(row: &rusqlite::Row<'_>) -> rusqlite::Result<EmailTracking> {
    Ok(EmailTracking {
        message_id: row.get(0)?,
        tracking_id: row.get(1)?,
        enabled: row.get(2)?,
        host_id: row.get(3)?,
        pixel_requests: row.get(4)?,
        first_requested_at: row.get(5)?,
        last_requested_at: row.get(6)?,
    })
}
fn editable(db: &Connection, id: &str) -> Result<()> {
    let valid: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM messages WHERE id=?1 AND is_draft=1 AND NOT EXISTS(SELECT 1 FROM outbox WHERE message_id=?1))",[id],|r|r.get(0))?;
    if !valid {
        return Err(MailError::InvalidAction(
            "Tracking can be changed only on an unscheduled draft".into(),
        ));
    }
    Ok(())
}
impl MailStore {
    pub fn email_tracking(&self, id: &str) -> Result<Option<EmailTracking>> {
        let db = self.lock()?;
        schema(&db)?;
        Ok(db.query_row("SELECT message_id,tracking_id,enabled,host_id,pixel_requests,first_requested_at,last_requested_at FROM email_tracking WHERE message_id=?1",[id],row).optional()?)
    }
    /// Persist an opaque identifier before any network request, with tracking off.
    pub fn prepare_email_tracking(&self, id: &str) -> Result<EmailTracking> {
        let mut db = self.lock()?;
        schema(&db)?;
        let tx = db.transaction()?;
        editable(&tx, id)?;
        tx.execute(
            "INSERT OR IGNORE INTO email_tracking(message_id,tracking_id) VALUES(?1,?2)",
            params![id, Uuid::new_v4().to_string()],
        )?;
        let record=tx.query_row("SELECT message_id,tracking_id,enabled,host_id,pixel_requests,first_requested_at,last_requested_at FROM email_tracking WHERE message_id=?1",[id],row)?;
        tx.commit()?;
        Ok(record)
    }
    pub fn set_email_tracking(&self, id: &str, enabled: bool, host_id: Option<&str>) -> Result<()> {
        let mut db = self.lock()?;
        schema(&db)?;
        let tx = db.transaction()?;
        editable(&tx, id)?;
        if enabled {
            let host = host_id.ok_or_else(|| {
                MailError::InvalidAction("Connect tracking before enabling it".into())
            })?;
            let changed=tx.execute("UPDATE email_tracking SET enabled=1,host_id=?2 WHERE message_id=?1 AND (host_id IS NULL OR host_id=?2)",params![id,host])?;
            if changed != 1 {
                return Err(MailError::InvalidAction(
                    "Reconnect the original tracking account or create a new draft".into(),
                ));
            }
        } else {
            tx.execute(
                "UPDATE email_tracking SET enabled=0 WHERE message_id=?1",
                [id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
    /// Whether any sent (not draft) message has an enabled tracking record:
    /// the same rows `email_tracking_history` lists.
    pub fn has_tracked_sent_mail(&self) -> Result<bool> {
        let db = self.lock()?;
        schema(&db)?;
        Ok(db.query_row(&format!("SELECT EXISTS(SELECT 1 FROM {TRACKED_SENT})"), [], |r| r.get(0))?)
    }
    pub fn email_tracking_history(&self) -> Result<Vec<EmailTracking>> {
        let db = self.lock()?;
        schema(&db)?;
        let mut query=db.prepare(&format!("SELECT t.message_id,t.tracking_id,t.enabled,t.host_id,t.pixel_requests,t.first_requested_at,t.last_requested_at FROM {TRACKED_SENT} ORDER BY m.received_at DESC LIMIT 500"))?;
        query
            .query_map([], row)?
            .map(|v| v.map_err(Into::into))
            .collect()
    }
    pub fn tracking_message(&self, id: &str) -> Result<Option<TrackingMessage>> {
        let db = self.lock()?;
        let raw:Option<(String,String,String,String,String,i64,bool,bool)>=db.query_row("SELECT id,thread_id,account_id,subject,to_json,received_at,is_sent,is_draft FROM messages WHERE id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?))).optional()?;
        raw.map(
            |(id, thread_id, account_id, subject, to, received_at, is_sent, is_draft)| {
                Ok(TrackingMessage {
                    id,
                    thread_id,
                    account_id,
                    subject,
                    to: serde_json::from_str(&to)?,
                    received_at,
                    is_sent,
                    is_draft,
                })
            },
        )
        .transpose()
    }

    /// Preserve only a bounded, authenticated snapshot. Stale replies cannot replace
    /// newer history; a summary refresh never discards individually fetched detail.
    pub fn cache_tracking_snapshot(
        &self,
        id: &str,
        host: &str,
        count: u64,
        expires: Option<i64>,
        events: Option<&[TrackingImageEvent]>,
        complete: bool,
        checked: Option<i64>,
    ) -> Result<()> {
        if count > i64::MAX as u64
            || events.is_some_and(|events| {
                events.len() > MAX_IMAGE_EVENTS
                    || events.iter().any(|e| {
                        e.at_ms < 0
                            || e.at_ms > super::now_ms().saturating_add(60_000)
                            || e.count == 0
                            || e.count > count
                    })
                    || events.windows(2).any(|p| p[0].at_ms >= p[1].at_ms)
                    || events
                        .iter()
                        .try_fold(0u64, |sum, e| sum.checked_add(e.count))
                        .is_none_or(|sum| sum > count)
            })
        {
            return Err(MailError::InvalidAction(
                "Invalid tracking history snapshot".into(),
            ));
        }
        let mut db = self.lock()?;
        schema(&db)?;
        let tx = db.transaction()?;
        if !tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM email_tracking WHERE tracking_id=?1 AND host_id=?2)",
            params![id, host],
            |r| r.get::<_, bool>(0),
        )? {
            return Ok(());
        }
        let old = detail(&tx, id)?;
        let (events_json, history_complete) = if count >= old.remote_count {
            if let Some(events) = events {
                (
                    serde_json::to_string(events)?,
                    complete && events.iter().map(|e| e.count).sum::<u64>() == count,
                )
            } else {
                (
                    serde_json::to_string(&old.events)?,
                    (old.timeline_complete || count == 0) && count == old.remote_count,
                )
            }
        } else {
            (serde_json::to_string(&old.events)?, old.timeline_complete)
        };
        tx.execute("INSERT INTO email_tracking_detail(tracking_id,expires_at,refreshed_at_ms,remote_count,events_json,timeline_complete,sent_at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(tracking_id) DO UPDATE SET expires_at=coalesce(excluded.expires_at,email_tracking_detail.expires_at),refreshed_at_ms=CASE WHEN excluded.refreshed_at_ms IS NULL THEN email_tracking_detail.refreshed_at_ms WHEN email_tracking_detail.refreshed_at_ms IS NULL THEN excluded.refreshed_at_ms ELSE max(email_tracking_detail.refreshed_at_ms,excluded.refreshed_at_ms) END,remote_count=max(email_tracking_detail.remote_count,excluded.remote_count),events_json=excluded.events_json,timeline_complete=excluded.timeline_complete",params![id,expires,checked,count,events_json,history_complete,old.sent_at_ms])?;
        tx.commit()?;
        Ok(())
    }

    pub fn tracking_detail(&self, id: &str) -> Result<TrackingDetail> {
        let mut db = self.lock()?;
        schema(&db)?;
        let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let now = super::now_ms();
        // Prune all expired hosted snapshots whenever activity is accessed. Counts
        // and send/refresh metadata survive; app-closed cleanup is not implied.
        tx.execute("UPDATE email_tracking_detail SET events_json='[]',timeline_complete=0 WHERE expires_at<=?1 AND events_json<>'[]'",[now])?;
        let mut result = detail(&tx, id)?;
        let before = result.events.len();
        result
            .events
            .retain(|event| event.at_ms >= now.saturating_sub(90 * 86_400_000));
        if before != result.events.len() {
            result.timeline_complete = false;
            tx.execute("UPDATE email_tracking_detail SET events_json=?2,timeline_complete=0 WHERE tracking_id=?1",params![id,serde_json::to_string(&result.events)?])?;
        }
        tx.commit()?;
        Ok(result)
    }

    /// Cached incoming messages in this account/thread only. Sending identities
    /// mirror follow-up exclusion rules: connected accounts plus confirmed sent aliases.
    pub fn tracking_reply(
        &self,
        message_id: &str,
        after: i64,
    ) -> Result<(Option<(String, i64)>, bool)> {
        let db = self.lock()?;
        schema(&db)?;
        let account: Option<String> = db
            .query_row(
                "SELECT account_id FROM messages WHERE id=?1 AND is_sent=1 AND is_draft=0",
                [message_id],
                |r| r.get(0),
            )
            .optional()?;
        let Some(account) = account else {
            return Ok((None, false));
        };
        let own = known_own_addresses(&db, &account, &[])?;
        tracking_reply_on(&db, message_id, after, &own)
    }

    pub fn tracking_reply_baseline_available(&self, message_id: &str) -> Result<bool> {
        let db = self.lock()?;
        schema(&db)?;
        Ok(db.query_row("SELECT EXISTS(SELECT 1 FROM email_tracking_send_baseline b JOIN email_tracking t ON t.tracking_id=b.tracking_id WHERE t.message_id=?1)",[message_id],|r|r.get(0))?)
    }

    /// Consume open-count changes once per device, independently of activity-panel refreshes.
    pub fn consume_tracking_notifications(
        &self,
        enabled: bool,
        session_since: i64,
        now: i64,
    ) -> Result<Vec<serde_json::Value>> {
        let mut db = self.lock()?;
        schema(&db)?;
        let tx = db.transaction()?;
        tx.execute_batch("CREATE TABLE IF NOT EXISTS tracking_notification_counts(tracking_id TEXT PRIMARY KEY, count INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS tracking_notification_state(id INTEGER PRIMARY KEY CHECK(id=1), enabled INTEGER NOT NULL, since INTEGER NOT NULL);")?;
        let state: Option<(bool, i64)> = tx
            .query_row(
                "SELECT enabled,since FROM tracking_notification_state WHERE id=1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let since = match state {
            Some((true, since)) if enabled => since.max(session_since),
            _ => now,
        };
        let mut items = Vec::new();
        {
            let mut query = tx.prepare("SELECT t.tracking_id,t.pixel_requests,t.last_requested_at,m.account_id,m.thread_id,m.subject FROM email_tracking t JOIN messages m ON m.id=t.message_id WHERE t.enabled=1 AND m.is_sent=1 AND m.is_draft=0 ORDER BY t.last_requested_at DESC LIMIT 500")?;
            let rows = query.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, Option<i64>>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                ))
            })?;
            for row in rows {
                let (id, count, last, account, thread, subject) = row?;
                let previous: i64 = tx
                    .query_row(
                        "SELECT count FROM tracking_notification_counts WHERE tracking_id=?1",
                        [&id],
                        |r| r.get(0),
                    )
                    .optional()?
                    .unwrap_or(0);
                if enabled && count > previous && last.is_some_and(|time| time >= since) {
                    items.push(serde_json::json!({"id":format!("open-{id}-{count}"),"account_id":account,"thread_id":thread,"subject":subject,"pixel_requests":count}));
                }
                tx.execute("INSERT INTO tracking_notification_counts(tracking_id,count) VALUES(?1,?2) ON CONFLICT(tracking_id) DO UPDATE SET count=max(count,excluded.count)", params![id,count])?;
            }
        }
        tx.execute("INSERT INTO tracking_notification_state(id,enabled,since) VALUES(1,?1,?2) ON CONFLICT(id) DO UPDATE SET enabled=excluded.enabled,since=excluded.since",params![enabled,since])?;
        tx.execute("DELETE FROM tracking_notification_counts WHERE tracking_id NOT IN (SELECT tracking_id FROM email_tracking)", [])?;
        tx.commit()?;
        Ok(items)
    }

    pub fn update_email_tracking_activity(
        &self,
        id: &str,
        host: &str,
        count: u64,
        first: Option<i64>,
        last: Option<i64>,
    ) -> Result<()> {
        let db = self.lock()?;
        schema(&db)?;
        db.execute("UPDATE email_tracking SET pixel_requests=max(pixel_requests,?3),first_requested_at=coalesce(first_requested_at,?4),last_requested_at=CASE WHEN ?5 IS NULL THEN last_requested_at ELSE max(coalesce(last_requested_at,?5),?5) END WHERE tracking_id=?1 AND host_id=?2",params![id,host,count.min(i64::MAX as u64),first,last])?;
        Ok(())
    }
}
/// One identity implementation shared by the feed and atomic follow-up creation.
pub(crate) fn known_own_addresses(
    db: &Connection,
    account: &str,
    extra: &[String],
) -> Result<BTreeSet<String>> {
    let mut own = BTreeSet::new();
    let mut q=db.prepare("SELECT email FROM accounts UNION SELECT sender FROM (SELECT sender FROM messages WHERE account_id=?1 AND is_sent=1 AND is_draft=0 ORDER BY received_at DESC LIMIT 512)")?;
    for address in q.query_map([account], |r| r.get::<_, String>(0))? {
        if let Some(address) = super::followups::mailbox(&address?) {
            own.insert(address);
        }
    }
    for address in extra {
        own.insert(super::followups::mailbox(address).ok_or_else(|| {
            MailError::InvalidAction("Invalid sending identity for follow-up".into())
        })?);
    }
    Ok(own)
}

pub(crate) fn tracking_reply_on(
    db: &Connection,
    message_id: &str,
    after: i64,
    own: &BTreeSet<String>,
) -> Result<(Option<(String, i64)>, bool)> {
    schema(db)?;
    let owner: Option<(String, String)> = db
        .query_row(
            "SELECT account_id,thread_id FROM messages WHERE id=?1 AND is_sent=1 AND is_draft=0",
            [message_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((account, thread)) = owner else {
        return Ok((None, false));
    };
    let mut q=db.prepare("WITH canonical(id) AS (SELECT coalesce((SELECT target_thread FROM local_conversation_links WHERE source_thread=?2),?2)) SELECT id,received_at,sender FROM messages m WHERE account_id=?1 AND thread_id IN (SELECT id FROM canonical UNION SELECT source_thread FROM local_conversation_links WHERE target_thread=(SELECT id FROM canonical)) AND is_sent=0 AND is_draft=0 AND received_at>?3 AND NOT EXISTS(SELECT 1 FROM email_tracking_send_baseline b JOIN email_tracking t ON t.tracking_id=b.tracking_id WHERE t.message_id=?4 AND b.message_id=m.id) ORDER BY received_at,id LIMIT 512")?;
    let mut inspected = 0;
    for candidate in q.query_map(params![account, thread, after, message_id], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, String>(2)?,
        ))
    })? {
        let (id, at, sender) = candidate?;
        inspected += 1;
        if super::followups::mailbox(&sender).is_some_and(|email| !own.contains(&email)) {
            return Ok((Some((id, at)), false));
        }
    }
    Ok((None, inspected == 512))
}

/// Must run in the same write transaction as reminder insertion.
pub(crate) fn guard_followup_anchor(
    db: &Connection,
    thread_id: &str,
    account: &str,
    message_id: &str,
    own: &BTreeSet<String>,
) -> Result<()> {
    schema(db)?;
    let anchor:Option<(String,String,i64)>=db.query_row("SELECT m.account_id,m.thread_id,coalesce(d.sent_at_ms,m.received_at) FROM messages m JOIN email_tracking t ON t.message_id=m.id LEFT JOIN email_tracking_detail d ON d.tracking_id=t.tracking_id WHERE m.id=?1 AND m.is_sent=1 AND m.is_draft=0 AND t.enabled=1",[message_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
    let Some((anchor_account, anchor_thread, at)) = anchor else {
        return Err(MailError::InvalidAction(
            "Choose a tracked sent message from this conversation".into(),
        ));
    };
    let same:bool=db.query_row("SELECT coalesce((SELECT target_thread FROM local_conversation_links WHERE source_thread=?1),?1)=coalesce((SELECT target_thread FROM local_conversation_links WHERE source_thread=?2),?2)",params![thread_id,anchor_thread],|r|r.get(0))?;
    if anchor_account != account || !same {
        return Err(MailError::InvalidAction(
            "Choose a tracked sent message from this conversation".into(),
        ));
    }
    let (reply, limited) = tracking_reply_on(db, message_id, at, own)?;
    if reply.is_some() {
        return Err(MailError::InvalidAction(
            "A reply has arrived. Refresh activity before setting a reminder.".into(),
        ));
    }
    if limited {
        return Err(MailError::InvalidAction("Reply history is too large to check safely. Open the conversation before setting a reminder.".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn send_baseline_excludes_cached_future_mail_and_survives_provider_id_adoption() {
        let store = MailStore::open_in_memory().unwrap();
        store.seed_demo().unwrap();
        let draft = store
            .compose_draft("demo:personal", "recipient@example.test", "Subject", "Body")
            .unwrap();
        let record = store.prepare_email_tracking(&draft.id).unwrap();
        store
            .set_email_tracking(&draft.id, true, Some("host"))
            .unwrap();
        let mut future = draft.clone();
        future.id = "known-future".into();
        future.thread_id = "linked-future".into();
        future.sender = "recipient@example.test".into();
        future.is_draft = false;
        future.is_sent = false;
        future.received_at = super::super::now_ms() + 60_000;
        store.upsert_message(&future).unwrap();
        store
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO local_conversation_links(source_thread,target_thread) VALUES(?1,?2)",
                params![future.thread_id, draft.thread_id],
            )
            .unwrap();
        store.mark_draft_sent(&draft.id).unwrap();
        let at = store
            .tracking_detail(&record.tracking_id)
            .unwrap()
            .sent_at_ms
            .unwrap();
        assert!(store.tracking_reply_baseline_available(&draft.id).unwrap());
        assert_eq!(store.tracking_reply(&draft.id, at).unwrap(), (None, false));
        store
            .adopt_sent_provider_id(&draft.id, "adopted-anchor")
            .unwrap();
        assert!(
            store
                .tracking_reply_baseline_available("adopted-anchor")
                .unwrap()
        );
        assert_eq!(
            store.tracking_reply("adopted-anchor", at).unwrap(),
            (None, false)
        );
        future.id = "actually-new-reply".into();
        future.received_at = at + 1;
        store.upsert_message(&future).unwrap();
        assert_eq!(
            store.tracking_reply("adopted-anchor", at).unwrap(),
            (Some((future.id.clone(), at + 1)), false)
        );
        // Baseline-less historical rows retain the timestamp-only fallback and
        // explicitly report unavailable baseline instead of claiming coverage.
        store
            .lock()
            .unwrap()
            .execute(
                "DELETE FROM email_tracking_send_baseline WHERE tracking_id=?1",
                [&record.tracking_id],
            )
            .unwrap();
        assert!(
            !store
                .tracking_reply_baseline_available("adopted-anchor")
                .unwrap()
        );
    }

    #[test]
    fn anchored_reminder_rechecks_reply_atomically_and_preserves_existing_reminder_on_refusal() {
        let store = MailStore::open_in_memory().unwrap();
        store.seed_demo().unwrap();
        let draft = store
            .compose_draft("demo:personal", "recipient@example.test", "Subject", "Body")
            .unwrap();
        let record = store.prepare_email_tracking(&draft.id).unwrap();
        store
            .set_email_tracking(&draft.id, true, Some("host"))
            .unwrap();
        store.mark_draft_sent(&draft.id).unwrap();
        let at = store
            .tracking_detail(&record.tracking_id)
            .unwrap()
            .sent_at_ms
            .unwrap();
        let mut message = draft.clone();
        message.id = "own-incoming".into();
        message.sender = "Alex <alex@example.com>".into();
        message.is_draft = false;
        message.is_sent = false;
        message.received_at = at + 10;
        store.upsert_message(&message).unwrap();
        let due = super::super::now_ms() + 86_400_000;
        assert!(!store.tracking_reply(&draft.id, at).unwrap().0.is_some()); // activity panel read
        let initial = store
            .set_followup_for_sent_message(&draft.thread_id, due, true, &draft.id)
            .unwrap();
        message.id = "reply-after-panel-read".into();
        message.sender = "recipient@example.test".into();
        message.received_at = at + 20;
        store.upsert_message(&message).unwrap();
        let error = store
            .set_followup_for_sent_message(&draft.thread_id, due + 60_000, true, &draft.id)
            .unwrap_err()
            .to_string();
        assert!(error.contains("A reply has arrived. Refresh activity before setting a reminder."));
        let persisted_due: i64 = store
            .lock()
            .unwrap()
            .query_row(
                "SELECT due_at_ms FROM followups WHERE thread_id=?1",
                [&draft.thread_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(persisted_due, initial.due_at_ms);
        assert!(
            store
                .set_followup_for_sent_message(&draft.thread_id, due, false, &draft.id)
                .is_err()
        );
        let other = store
            .compose_draft("demo:personal", "other@example.test", "Other", "Body")
            .unwrap();
        assert!(
            store
                .set_followup_for_sent_message(&other.thread_id, due, true, &draft.id)
                .is_err()
        );
        store
            .upsert_account(&super::super::Account {
                id: "foreign".into(),
                provider: super::super::Provider::Gmail,
                email: "foreign@example.test".into(),
                display_name: "Foreign".into(),
            })
            .unwrap();
        let foreign = store
            .compose_draft("foreign", "other@example.test", "Other", "Body")
            .unwrap();
        let foreign_tracking = store.prepare_email_tracking(&foreign.id).unwrap();
        store
            .set_email_tracking(&foreign.id, true, Some("host"))
            .unwrap();
        store.mark_draft_sent(&foreign.id).unwrap();
        assert!(
            store
                .set_followup_for_sent_message(&draft.thread_id, due, true, &foreign.id)
                .is_err()
        );
        assert_ne!(foreign_tracking.tracking_id, record.tracking_id);
        // Ordinary non-anchored reminders deliberately retain their original API.
        assert!(store.set_followup(&draft.thread_id, due, false).is_ok());
    }

    #[test]
    fn guarded_reminder_rejects_incomplete_reply_scan_and_linked_replies_resolve_reminders() {
        let store = MailStore::open_in_memory().unwrap();
        store.seed_demo().unwrap();
        let draft = store
            .compose_draft("demo:personal", "recipient@example.test", "Subject", "Body")
            .unwrap();
        let record = store.prepare_email_tracking(&draft.id).unwrap();
        store
            .set_email_tracking(&draft.id, true, Some("host"))
            .unwrap();
        store.mark_draft_sent(&draft.id).unwrap();
        let at = store
            .tracking_detail(&record.tracking_id)
            .unwrap()
            .sent_at_ms
            .unwrap();
        let mut message = draft.clone();
        message.thread_id = "linked-replies".into();
        message.id = "linked-own".into();
        message.sender = "alex@example.com".into();
        message.is_draft = false;
        message.is_sent = false;
        message.received_at = at + 1;
        store.upsert_message(&message).unwrap();
        store
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO local_conversation_links(source_thread,target_thread) VALUES(?1,?2)",
                params![message.thread_id, draft.thread_id],
            )
            .unwrap();
        let due = super::super::now_ms() + 86_400_000;
        let follow = store
            .set_followup_for_sent_message(&draft.thread_id, due, true, &draft.id)
            .unwrap();
        message.id = "linked-new-reply".into();
        message.sender = "recipient@example.test".into();
        message.received_at = super::super::now_ms().max(follow.created_at_ms) + 1;
        store.upsert_message(&message).unwrap();
        let status = store.list_followups(Some("demo:personal")).unwrap();
        assert_eq!(status[0].status, "replied");
        assert_eq!(status[0].reply_message_id, Some(message.id.clone()));
        // A second tracked conversation with only a full page of own messages
        // cannot establish absence of a hidden reply, so creation fails closed.
        let other = store
            .compose_draft("demo:personal", "recipient@example.test", "Other", "Body")
            .unwrap();
        let t = store.prepare_email_tracking(&other.id).unwrap();
        store
            .set_email_tracking(&other.id, true, Some("host"))
            .unwrap();
        store.mark_draft_sent(&other.id).unwrap();
        let after = store
            .tracking_detail(&t.tracking_id)
            .unwrap()
            .sent_at_ms
            .unwrap();
        message.thread_id = other.thread_id.clone();
        message.sender = "alex@example.com".into();
        message.received_at = after + 1;
        for n in 0..512 {
            message.id = format!("own-page-{n}");
            store.upsert_message(&message).unwrap();
        }
        let error = store
            .set_followup_for_sent_message(&other.thread_id, due, true, &other.id)
            .unwrap_err()
            .to_string();
        assert!(error.contains("too large to check safely"));
    }

    #[test]
    fn bounded_history_snapshots_are_monotonic_idempotent_private_and_survive_adoption() {
        let root = std::env::temp_dir().join(format!("tracking-history-{}", Uuid::new_v4()));
        let path = root.join("mail.sqlite");
        let store = MailStore::open(&path).unwrap();
        store.seed_demo().unwrap();
        let draft = store
            .compose_draft(
                "demo:personal",
                "recipient@example.test",
                "Private subject",
                "Private body",
            )
            .unwrap();
        let record = store.prepare_email_tracking(&draft.id).unwrap();
        store
            .set_email_tracking(&draft.id, true, Some("host"))
            .unwrap();
        store.mark_draft_sent(&draft.id).unwrap();
        let accepted = store
            .tracking_detail(&record.tracking_id)
            .unwrap()
            .sent_at_ms
            .unwrap();
        let events = vec![TrackingImageEvent {
            at_ms: accepted,
            count: 2,
        }];
        for _ in 0..2 {
            store
                .cache_tracking_snapshot(
                    &record.tracking_id,
                    "host",
                    2,
                    Some(accepted + 90 * 86_400_000),
                    Some(&events),
                    true,
                    Some(accepted),
                )
                .unwrap();
        }
        let stale = vec![TrackingImageEvent {
            at_ms: accepted - 1,
            count: 1,
        }];
        store
            .cache_tracking_snapshot(
                &record.tracking_id,
                "host",
                1,
                None,
                Some(&stale),
                false,
                Some(accepted - 1),
            )
            .unwrap();
        let detail = store.tracking_detail(&record.tracking_id).unwrap();
        assert_eq!(detail.events, events);
        assert!(detail.timeline_complete);
        assert_eq!(detail.refreshed_at_ms, Some(accepted));
        store
            .cache_tracking_snapshot(
                &record.tracking_id,
                "other-host",
                99,
                None,
                None,
                false,
                Some(accepted + 1),
            )
            .unwrap();
        assert_eq!(
            store
                .tracking_detail(&record.tracking_id)
                .unwrap()
                .remote_count,
            2
        );
        store
            .cache_tracking_snapshot(
                &record.tracking_id,
                "host",
                3,
                None,
                None,
                false,
                Some(accepted + 2),
            )
            .unwrap();
        let detail = store.tracking_detail(&record.tracking_id).unwrap();
        assert_eq!(detail.events, events);
        assert!(!detail.timeline_complete);
        assert!(
            store
                .cache_tracking_snapshot(
                    &record.tracking_id,
                    "host",
                    3,
                    None,
                    Some(&vec![
                        TrackingImageEvent {
                            at_ms: accepted,
                            count: 1
                        };
                        101
                    ]),
                    false,
                    None
                )
                .is_err()
        );
        assert!(
            store
                .cache_tracking_snapshot(
                    &record.tracking_id,
                    "host",
                    3,
                    None,
                    Some(&[TrackingImageEvent {
                        at_ms: accepted,
                        count: 4
                    }]),
                    true,
                    None
                )
                .is_err()
        );
        store
            .adopt_sent_provider_id(&draft.id, "provider-history-message")
            .unwrap();
        drop(store);
        let store = MailStore::open(&path).unwrap();
        let detail = store.tracking_detail(&record.tracking_id).unwrap();
        assert_eq!(detail.sent_at_ms, Some(accepted));
        assert_eq!(detail.events, events);
        assert_eq!(detail.remote_count, 3);
        assert_eq!(
            store
                .email_tracking("provider-history-message")
                .unwrap()
                .unwrap()
                .tracking_id,
            record.tracking_id
        );
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn accessing_activity_prunes_expired_image_history_but_keeps_counts() {
        let store = MailStore::open_in_memory().unwrap();
        store.seed_demo().unwrap();
        let draft = store
            .compose_draft("demo:personal", "recipient@example.test", "Subject", "Body")
            .unwrap();
        let record = store.prepare_email_tracking(&draft.id).unwrap();
        store
            .set_email_tracking(&draft.id, true, Some("host"))
            .unwrap();
        store.mark_draft_sent(&draft.id).unwrap();
        store
            .update_email_tracking_activity(&record.tracking_id, "host", 2, Some(100), Some(100))
            .unwrap();
        store
            .cache_tracking_snapshot(
                &record.tracking_id,
                "host",
                2,
                Some(1),
                Some(&[TrackingImageEvent {
                    at_ms: 100,
                    count: 2,
                }]),
                true,
                Some(200),
            )
            .unwrap();
        let detail = store.tracking_detail(&record.tracking_id).unwrap();
        assert!(detail.events.is_empty());
        assert!(!detail.timeline_complete);
        assert!(detail.sent_at_ms.is_some());
        assert_eq!(
            store
                .email_tracking(&draft.id)
                .unwrap()
                .unwrap()
                .pixel_requests,
            2
        );
    }

    #[test]
    fn cached_reply_detection_excludes_own_aliases_drafts_and_earlier_mail() {
        let store = MailStore::open_in_memory().unwrap();
        store.seed_demo().unwrap();
        let draft = store
            .compose_draft("demo:personal", "recipient@example.test", "Subject", "Body")
            .unwrap();
        let record = store.prepare_email_tracking(&draft.id).unwrap();
        store
            .set_email_tracking(&draft.id, true, Some("host"))
            .unwrap();
        store.mark_draft_sent(&draft.id).unwrap();
        let at = store
            .tracking_detail(&record.tracking_id)
            .unwrap()
            .sent_at_ms
            .unwrap();
        let mut message = draft.clone();
        message.id = "earlier-mail".into();
        message.sender = "recipient@example.test".into();
        message.received_at = at - 1;
        message.is_draft = false;
        message.is_sent = false;
        store.upsert_message(&message).unwrap();
        message.id = "unsent-external".into();
        message.received_at = at + 1;
        message.is_draft = true;
        store.upsert_message(&message).unwrap();
        message.id = "sent-alias".into();
        message.sender = "Alias <alias@example.test>".into();
        message.is_draft = false;
        message.is_sent = true;
        store.upsert_message(&message).unwrap();
        message.id = "incoming-from-alias".into();
        message.is_sent = false;
        message.received_at = at + 2;
        store.upsert_message(&message).unwrap();
        assert_eq!(store.tracking_reply(&draft.id, at).unwrap(), (None, false));
        message.id = "real-reply".into();
        message.sender = "Recipient <recipient@example.test>".into();
        message.received_at = at + 3;
        store.upsert_message(&message).unwrap();
        assert_eq!(
            store.tracking_reply(&draft.id, at).unwrap(),
            (Some(("real-reply".into(), at + 3)), false)
        );
        assert_eq!(store.tracking_reply("unknown", at).unwrap(), (None, false));
    }

    #[test]
    fn open_alerts_respect_tracking_disabled_periods_and_persisted_counts() {
        let root = std::env::temp_dir().join(format!("tracking-alerts-{}", Uuid::new_v4()));
        let path = root.join("mail.sqlite");
        let store = MailStore::open(&path).unwrap();
        store.seed_demo().unwrap();
        let draft = store
            .compose_draft(
                "demo:personal",
                "reader@example.test",
                "Project update",
                "Body",
            )
            .unwrap();
        let record = store.prepare_email_tracking(&draft.id).unwrap();
        store
            .set_email_tracking(&draft.id, true, Some("host"))
            .unwrap();
        // Draft activity never produces an alert.
        store
            .update_email_tracking_activity(&record.tracking_id, "host", 1, Some(50), Some(50))
            .unwrap();
        assert!(
            store
                .consume_tracking_notifications(true, 100, 100)
                .unwrap()
                .is_empty()
        );
        store.mark_draft_sent(&draft.id).unwrap();
        assert!(
            store
                .consume_tracking_notifications(true, 100, 110)
                .unwrap()
                .is_empty(),
            "Old activity is quiet"
        );
        store
            .update_email_tracking_activity(&record.tracking_id, "host", 3, Some(50), Some(120))
            .unwrap();
        let alerts = store
            .consume_tracking_notifications(true, 100, 130)
            .unwrap();
        assert_eq!(alerts.len(), 1, "Several image requests coalesce");
        assert_eq!(alerts[0]["subject"], "Project update");
        assert_eq!(alerts[0]["thread_id"], draft.thread_id);
        assert!(
            store
                .consume_tracking_notifications(true, 100, 140)
                .unwrap()
                .is_empty()
        );
        // Changing the provider ID must not reset the opaque notification identity.
        store
            .adopt_sent_provider_id(&draft.id, "provider-sent-id")
            .unwrap();
        drop(store);
        let store = MailStore::open(&path).unwrap();
        assert!(
            store
                .consume_tracking_notifications(true, 150, 150)
                .unwrap()
                .is_empty()
        );
        store
            .update_email_tracking_activity(&record.tracking_id, "host", 4, Some(50), Some(160))
            .unwrap();
        assert!(
            store
                .consume_tracking_notifications(false, 150, 170)
                .unwrap()
                .is_empty()
        );
        store
            .update_email_tracking_activity(&record.tracking_id, "host", 5, Some(50), Some(180))
            .unwrap();
        assert!(
            store
                .consume_tracking_notifications(true, 150, 200)
                .unwrap()
                .is_empty(),
            "No replay after reenable"
        );
        store
            .update_email_tracking_activity(&record.tracking_id, "host", 6, Some(50), Some(210))
            .unwrap();
        assert_eq!(
            store
                .consume_tracking_notifications(true, 150, 220)
                .unwrap()
                .len(),
            1
        );
        // A shell can move its cutoff even if toggled off and on between polls.
        store
            .update_email_tracking_activity(&record.tracking_id, "host", 7, Some(50), Some(230))
            .unwrap();
        assert!(
            store
                .consume_tracking_notifications(true, 240, 250)
                .unwrap()
                .is_empty()
        );
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn tracking_is_off_by_default_and_cannot_change_after_queueing() {
        let store = MailStore::open_in_memory().unwrap();
        store.seed_demo().unwrap();
        let draft = store
            .compose_draft(
                "demo:personal",
                "recipient@example.com",
                "Subject",
                "Private body",
            )
            .unwrap();
        assert!(store.email_tracking(&draft.id).unwrap().is_none());
        let first = store.prepare_email_tracking(&draft.id).unwrap();
        assert!(!first.enabled);
        assert_eq!(
            first.tracking_id,
            store.prepare_email_tracking(&draft.id).unwrap().tracking_id
        );
        store
            .set_email_tracking(&draft.id, true, Some("host-a"))
            .unwrap();
        assert!(
            store
                .set_email_tracking(&draft.id, true, Some("host-b"))
                .is_err()
        );
        store.set_email_tracking(&draft.id, false, None).unwrap();
        store
            .schedule_draft(&draft.id, super::super::now_ms() + 60000, "scheduled")
            .unwrap();
        assert!(
            store
                .set_email_tracking(&draft.id, true, Some("host-a"))
                .is_err()
        );
    }
    #[test]
    fn tracking_activity_belongs_to_original_host_and_counts_never_decrease() {
        let store = MailStore::open_in_memory().unwrap();
        store.seed_demo().unwrap();
        let draft = store
            .compose_draft(
                "demo:personal",
                "recipient@example.com",
                "Subject",
                "Private body",
            )
            .unwrap();
        let record = store.prepare_email_tracking(&draft.id).unwrap();
        store
            .set_email_tracking(&draft.id, true, Some("host-a"))
            .unwrap();
        store.mark_draft_sent(&draft.id).unwrap();
        store
            .adopt_sent_provider_id(&draft.id, "demo:personal:gmail-message:sent-123")
            .unwrap();
        assert!(store.email_tracking(&draft.id).unwrap().is_none());
        assert_eq!(
            store
                .email_tracking("demo:personal:gmail-message:sent-123")
                .unwrap()
                .unwrap()
                .tracking_id,
            record.tracking_id
        );
        store
            .update_email_tracking_activity(&record.tracking_id, "host-b", 99, Some(1), Some(2))
            .unwrap();
        assert_eq!(store.email_tracking_history().unwrap()[0].pixel_requests, 0);
        store
            .update_email_tracking_activity(&record.tracking_id, "host-a", 0, None, None)
            .unwrap();
        assert_eq!(
            store.email_tracking_history().unwrap()[0].last_requested_at,
            None
        );
        store
            .update_email_tracking_activity(&record.tracking_id, "host-a", 3, Some(10), Some(20))
            .unwrap();
        store
            .update_email_tracking_activity(&record.tracking_id, "host-a", 2, Some(9), Some(15))
            .unwrap();
        assert_eq!(store.email_tracking_history().unwrap()[0].pixel_requests, 3);
        assert_eq!(
            store.email_tracking_history().unwrap()[0].last_requested_at,
            Some(20)
        );
        assert!(store.set_email_tracking(&draft.id, false, None).is_err());
    }
}
