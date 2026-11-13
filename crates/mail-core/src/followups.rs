//! Durable local follow-ups. Evaluation never sends mail or changes provider flags.
use crate::{MailError, MailStore, Result, now_ms};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

const MAX_ACTIVE: i64 = 1_000;
const MAX_LIST: usize = 2_000;
const MAX_CANDIDATES: usize = 512;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct FollowupReminder {
    pub thread_id: String,
    pub account_id: String,
    pub subject: String,
    pub sender: String,
    pub due_at_ms: i64,
    pub created_at_ms: i64,
    pub only_if_no_reply: bool,
    /// waiting, due, replied or cancelled. Due does not mean a notification was delivered.
    pub status: String,
    pub reply_message_id: Option<String>,
}

pub(crate) fn initialize(db: &Connection) -> Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS followups(
            thread_id TEXT PRIMARY KEY REFERENCES threads(id) ON DELETE CASCADE,
            account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
            due_at_ms INTEGER NOT NULL,created_at_ms INTEGER NOT NULL,
            only_if_no_reply INTEGER NOT NULL,status TEXT NOT NULL,
            own_addresses_json TEXT NOT NULL,reply_message_id TEXT);
         CREATE INDEX IF NOT EXISTS followups_account_due ON followups(account_id,status,due_at_ms);
         CREATE INDEX IF NOT EXISTS followups_due ON followups(status,due_at_ms);
         CREATE INDEX IF NOT EXISTS followups_terminal_recent ON followups(created_at_ms DESC,thread_id) WHERE status IN ('replied','cancelled');
         CREATE INDEX IF NOT EXISTS followups_account_terminal_recent ON followups(account_id,created_at_ms DESC,thread_id) WHERE status IN ('replied','cancelled');
         CREATE TABLE IF NOT EXISTS followup_baseline_messages(
            thread_id TEXT NOT NULL REFERENCES followups(thread_id) ON DELETE CASCADE,
            message_id TEXT NOT NULL,PRIMARY KEY(thread_id,message_id));",
    )?;
    Ok(())
}

impl MailStore {
    pub fn set_followup(
        &self,
        thread_id: &str,
        due_at_ms: i64,
        only_if_no_reply: bool,
    ) -> Result<FollowupReminder> {
        self.set_followup_with_own_addresses(thread_id, due_at_ms, only_if_no_reply, &[])
    }

    /// The service may supply its provider-verified sending aliases. Stored sender
    /// strings alone are not evidence that an address belongs to the account.
    pub fn set_followup_with_own_addresses(
        &self,
        thread_id: &str,
        due_at_ms: i64,
        only_if_no_reply: bool,
        own_addresses: &[String],
    ) -> Result<FollowupReminder> {
        self.set_followup_checked(thread_id, due_at_ms, only_if_no_reply, own_addresses, None)
    }

    /// Atomically rejects a stale activity-panel action if a reply is now cached.
    pub fn set_followup_for_sent_message(
        &self,
        thread_id: &str,
        due_at_ms: i64,
        only_if_no_reply: bool,
        sent_message_id: &str,
    ) -> Result<FollowupReminder> {
        if !only_if_no_reply {
            return Err(MailError::InvalidAction(
                "Tracked-message follow-ups require no-reply reminders".into(),
            ));
        }
        self.set_followup_checked(
            thread_id,
            due_at_ms,
            only_if_no_reply,
            &[],
            Some(sent_message_id),
        )
    }

    fn set_followup_checked(
        &self,
        thread_id: &str,
        due_at_ms: i64,
        only_if_no_reply: bool,
        own_addresses: &[String],
        sent_message_id: Option<&str>,
    ) -> Result<FollowupReminder> {
        let created_at_ms = now_ms();
        if due_at_ms <= created_at_ms
            || due_at_ms > created_at_ms.saturating_add(5 * 366 * 86_400_000)
        {
            return Err(MailError::InvalidAction(
                "Choose a follow-up time in the future, within five years".into(),
            ));
        }
        if own_addresses.len() > 64 {
            return Err(MailError::InvalidAction(
                "At most 64 sending identities are supported".into(),
            ));
        }
        let mut db = self.lock()?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (account_id, subject, sender): (String, String, String) = tx
            .query_row(
                "SELECT account_id,subject,sender FROM threads WHERE id=?1",
                [thread_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?
            .ok_or_else(|| MailError::NotFound(thread_id.into()))?;
        let active: i64 = tx.query_row(
            "SELECT COUNT(*) FROM followups WHERE status IN ('waiting','due') AND thread_id<>?1",
            [thread_id],
            |r| r.get(0),
        )?;
        if active >= MAX_ACTIVE {
            return Err(MailError::InvalidAction(
                "Keep at most 1,000 active follow-ups".into(),
            ));
        }
        let own = super::tracking::known_own_addresses(&tx, &account_id, own_addresses)?;
        if let Some(anchor) = sent_message_id {
            super::tracking::guard_followup_anchor(&tx, thread_id, &account_id, anchor, &own)?;
        }
        tx.execute("INSERT INTO followups(thread_id,account_id,due_at_ms,created_at_ms,only_if_no_reply,status,own_addresses_json,reply_message_id) VALUES(?1,?2,?3,?4,?5,'waiting',?6,NULL) ON CONFLICT(thread_id) DO UPDATE SET account_id=excluded.account_id,due_at_ms=excluded.due_at_ms,created_at_ms=excluded.created_at_ms,only_if_no_reply=excluded.only_if_no_reply,status='waiting',own_addresses_json=excluded.own_addresses_json,reply_message_id=NULL",
            params![thread_id,account_id,due_at_ms,created_at_ms,only_if_no_reply,serde_json::to_string(&own)?])?;
        tx.execute(
            "DELETE FROM followup_baseline_messages WHERE thread_id=?1",
            [thread_id],
        )?;
        tx.execute("WITH canonical(id) AS (SELECT coalesce((SELECT target_thread FROM local_conversation_links WHERE source_thread=?1),?1)) INSERT INTO followup_baseline_messages(thread_id,message_id) SELECT ?1,m.id FROM messages m WHERE m.account_id=?2 AND m.thread_id IN (SELECT id FROM canonical UNION SELECT source_thread FROM local_conversation_links WHERE target_thread=(SELECT id FROM canonical))", params![thread_id,account_id])?;
        tx.commit()?;
        Ok(FollowupReminder {
            thread_id: thread_id.into(),
            account_id,
            subject,
            sender,
            due_at_ms,
            created_at_ms,
            only_if_no_reply,
            status: "waiting".into(),
            reply_message_id: None,
        })
    }

    /// Re-evaluates local cached replies and time. The app need not be running at
    /// the due instant: reopening evaluates it as due. No provider fetch occurs.
    /// Returns active reminders first, then recent terminal states, at most 2,000.
    pub fn list_followups(&self, account_id: Option<&str>) -> Result<Vec<FollowupReminder>> {
        let mut db = self.lock()?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(id) = account_id {
            if !tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM accounts WHERE id=?1)",
                [id],
                |r| r.get::<_, bool>(0),
            )? {
                return Err(MailError::UnknownAccount(id.into()));
            }
        }
        // Query the bounded active set separately from indexed recent history;
        // sorting all historical reminders on every inbox refresh is unnecessary.
        let mut rows = reminder_rows(&tx, account_id, true, MAX_LIST)?;
        rows.extend(reminder_rows(
            &tx,
            account_id,
            false,
            MAX_LIST - rows.len(),
        )?);
        let now = now_ms();
        let mut result = Vec::with_capacity(rows.len());
        for (mut item, own_json) in rows {
            if matches!(item.status.as_str(), "waiting" | "due") {
                if item.only_if_no_reply {
                    let own: BTreeSet<String> = serde_json::from_str(&own_json)?;
                    let mut replies = tx.prepare("WITH canonical(id) AS (SELECT coalesce((SELECT target_thread FROM local_conversation_links WHERE source_thread=?1),?1)) SELECT m.id,m.sender FROM messages m WHERE m.thread_id IN (SELECT id FROM canonical UNION SELECT source_thread FROM local_conversation_links WHERE target_thread=(SELECT id FROM canonical)) AND m.account_id=?2 AND m.is_sent=0 AND m.is_draft=0 AND m.received_at>?3 AND NOT EXISTS(SELECT 1 FROM followup_baseline_messages b WHERE b.thread_id=?1 AND b.message_id=m.id) ORDER BY m.received_at,m.id LIMIT ?4")?;
                    let candidates: Vec<(String, String)> = replies
                        .query_map(
                            params![
                                item.thread_id,
                                item.account_id,
                                item.created_at_ms,
                                MAX_CANDIDATES as i64
                            ],
                            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
                        )?
                        .collect::<std::result::Result<_, _>>()?;
                    drop(replies);
                    for candidate in candidates {
                        let (id, sender) = candidate;
                        if mailbox(&sender).is_some_and(|email| !own.contains(&email)) {
                            item.status = "replied".into();
                            item.reply_message_id = Some(id);
                            break;
                        }
                        // Persist inspected non-replies so a large run of own or
                        // malformed messages cannot permanently hide the reply
                        // beyond this evaluation's bounded batch. New imports
                        // with earlier timestamps remain independently eligible.
                        tx.execute("INSERT OR IGNORE INTO followup_baseline_messages(thread_id,message_id) VALUES(?1,?2)", params![item.thread_id,id])?;
                    }
                }
                if item.status != "replied" {
                    item.status = if item.due_at_ms <= now {
                        "due"
                    } else {
                        "waiting"
                    }
                    .into();
                }
                tx.execute(
                    "UPDATE followups SET status=?2,reply_message_id=?3 WHERE thread_id=?1",
                    params![item.thread_id, item.status, item.reply_message_id],
                )?;
            }
            result.push(item);
        }
        tx.commit()?;
        Ok(result)
    }

    /// Active follow-up conversations, including archived mail, in due order.
    /// This is a dedicated local view and does not change the inbox folder.
    pub fn list_followup_threads(
        &self,
        account_id: Option<&str>,
    ) -> Result<Vec<crate::ThreadSummary>> {
        let active = self
            .list_followups(account_id)?
            .into_iter()
            .filter(|item| matches!(item.status.as_str(), "waiting" | "due"));
        let db = self.lock()?;
        let mut result = Vec::new();
        let mut q = db.prepare("SELECT id,account_id,subject,sender,snippet,last_message_at,unread_count,is_inbox,folder FROM threads WHERE id=?1 AND account_id=?2")?;
        for item in active {
            let mut rows = q.query(params![item.thread_id, item.account_id])?;
            if let Some(row) = rows.next()? {
                result.push(crate::thread_row(&db, row)?);
            }
        }
        Ok(result)
    }

    pub fn cancel_followup(&self, thread_id: &str) -> Result<()> {
        let db = self.lock()?;
        if db.execute(
            "UPDATE followups SET status='cancelled' WHERE thread_id=?1",
            [thread_id],
        )? == 0
        {
            return Err(MailError::NotFound(thread_id.into()));
        }
        Ok(())
    }
}

fn reminder_rows(
    db: &Connection,
    account_id: Option<&str>,
    active: bool,
    limit: usize,
) -> Result<Vec<(FollowupReminder, String)>> {
    let predicate = if active {
        "f.status IN ('waiting','due')"
    } else {
        "f.status IN ('replied','cancelled')"
    };
    let order = if active {
        "f.due_at_ms,f.thread_id"
    } else {
        "f.created_at_ms DESC,f.thread_id"
    };
    let scope = if account_id.is_some() {
        "AND f.account_id=?1"
    } else {
        ""
    };
    let limit_bind = if account_id.is_some() { "?2" } else { "?1" };
    let sql = format!(
        "SELECT f.thread_id,f.account_id,t.subject,t.sender,f.due_at_ms,f.created_at_ms,f.only_if_no_reply,f.status,f.reply_message_id,f.own_addresses_json FROM followups f JOIN threads t ON t.id=f.thread_id AND t.account_id=f.account_id WHERE {predicate} {scope} ORDER BY {order} LIMIT {limit_bind}"
    );
    let values: Vec<rusqlite::types::Value> = account_id
        .into_iter()
        .map(|id| id.to_owned().into())
        .chain(std::iter::once((limit as i64).into()))
        .collect();
    let mut q = db.prepare(&sql)?;
    Ok(q.query_map(rusqlite::params_from_iter(values), |r| {
        Ok((
            FollowupReminder {
                thread_id: r.get(0)?,
                account_id: r.get(1)?,
                subject: r.get(2)?,
                sender: r.get(3)?,
                due_at_ms: r.get(4)?,
                created_at_ms: r.get(5)?,
                only_if_no_reply: r.get(6)?,
                status: r.get(7)?,
                reply_message_id: r.get(8)?,
            },
            r.get(9)?,
        ))
    })?
    .collect::<std::result::Result<_, _>>()?)
}

pub(crate) fn mailbox(value: &str) -> Option<String> {
    if value.len() > 16_384 || value.chars().any(char::is_control) {
        return None;
    }
    let parsed = mailparse::addrparse(value).ok()?;
    if parsed.len() != 1 {
        return None;
    }
    match &parsed[0] {
        mailparse::MailAddr::Single(single)
            if email_address::EmailAddress::is_valid(&single.addr) =>
        {
            Some(single.addr.to_ascii_lowercase())
        }
        _ => None,
    }
}
