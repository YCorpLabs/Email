//! Filters: sort incoming mail into named piles, each with an action.
//!
//! A filter is a name, a plain-language description and an action. A local
//! model (see `mail-ai`) reads first mail from strangers and picks the filter
//! whose description fits, or none. This module owns everything except the
//! model: storage, the contact gate, which threads still need a decision,
//! and applying a decision through the ordinary action queue.
//!
//! Safety rules, in order:
//! - Mail from anyone the account has written to, from the account's own
//!   company domain, or in a conversation the person has replied in, never
//!   reaches the model and is never filtered.
//! - A sender the person moved by hand follows that choice from then on.
//! - A model decision below [`APPLY_CONFIDENCE`] ([`IGNORE_CONFIDENCE`] for
//!   Ignore) leaves the mail in the inbox, untagged.
//! - Filtering archives (and for Ignore marks read). Nothing is deleted, and
//!   every filtered conversation can be moved back.

use crate::{MailAction, MailError, MailStore, Result, ThreadSummary, now_ms, thread_row};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

/// Model decisions below this probability are not acted on.
pub const APPLY_CONFIDENCE: f32 = 0.8;
/// Ignore also marks mail read, so it needs a surer model.
pub const IGNORE_CONFIDENCE: f32 = 0.95;
/// How far back sorting reaches when filters are first turned on.
pub const BACKFILL_DAYS: i64 = 14;
pub const MAX_FILTERS: usize = 8;
/// Corrections shown to the model as examples.
pub const MAX_EXAMPLES: usize = 8;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FilterAction {
    /// Stays in the inbox, tagged with the filter.
    Inbox,
    /// Archived; read from the filter's view.
    SkipInbox,
    /// Archived and marked read.
    Ignore,
}

impl FilterAction {
    fn as_str(self) -> &'static str {
        match self {
            Self::Inbox => "inbox",
            Self::SkipInbox => "skip_inbox",
            Self::Ignore => "ignore",
        }
    }
    fn parse(s: &str) -> Self {
        match s {
            "skip_inbox" => Self::SkipInbox,
            "ignore" => Self::Ignore,
            _ => Self::Inbox,
        }
    }
    fn leaves_inbox(self) -> bool {
        !matches!(self, Self::Inbox)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct MailFilter {
    pub id: String,
    pub name: String,
    pub description: String,
    pub action: FilterAction,
    pub position: i64,
}

/// A filter as shown on a thread row.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ThreadFilterTag {
    pub id: String,
    pub name: String,
    pub action: FilterAction,
    /// Why the model put it here, in a few words; empty for hand-sorted mail.
    pub reason: String,
    /// `model` or `user` (moved by hand, or a sender the person sorted before).
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct FilterStarter {
    pub key: String,
    pub name: String,
    pub description: String,
    pub action: FilterAction,
}

/// What a person sees for each filter in the sidebar.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct FilterCount {
    pub filter_id: String,
    pub unread: u32,
    pub last_7_days: u32,
}

/// The catch-all the model may choose: mail a person should see.
pub const EVERYTHING_ELSE_NAME: &str = "People";
pub const EVERYTHING_ELSE_DESCRIPTION: &str = "Everything a person should see: people writing personally, including strangers who are customers, users, investors, press, students or contributors; friends, family, colleagues, questions, invitations and introductions.";

pub fn starters() -> Vec<FilterStarter> {
    let s = |key: &str, name: &str, description: &str, action| FilterStarter {
        key: key.into(),
        name: name.into(),
        description: description.into(),
        action,
    };
    vec![
        s(
            "cold_outreach",
            "Cold outreach",
            "Unsolicited sales pitches: a stranger offering their own service or product, even when written personally or as a question. Agencies, dev shops, outsourcing, SEO, lead generation, growth services, recruiters offering candidates, sales demos, follow-ups to their own earlier pitch.",
            FilterAction::Ignore,
        ),
        s(
            "promotions",
            "Promotions",
            "Marketing from companies trying to sell: sales, discounts, offers, product launches, events selling tickets. Not receipts, tickets you bought, bookings, bills or account messages.",
            FilterAction::SkipInbox,
        ),
        s(
            "newsletters",
            "Newsletters",
            "Newsletters, digests and blog posts I subscribed to.",
            FilterAction::SkipInbox,
        ),
        s(
            "notifications",
            "Notifications",
            "Automated messages from services: alerts, GitHub and CI, receipts, invoices and bills, bookings and tickets, shipping, reminders, security and account messages.",
            FilterAction::Inbox,
        ),
    ]
}

/// One conversation the model should read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilterCandidate {
    pub thread_id: String,
    pub account_id: String,
    pub message_id: String,
    pub sender: String,
    pub subject: String,
    pub body: String,
    pub bulk: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct FilterDecision {
    pub thread_id: String,
    pub filter_id: Option<String>,
    pub moved: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilterExample {
    pub from: String,
    pub subject: String,
    /// Filter name, or the catch-all name for "keep in inbox".
    pub category: String,
}

pub(crate) fn initialize(db: &Connection) -> Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS mail_filters(id TEXT PRIMARY KEY,name TEXT NOT NULL COLLATE NOCASE UNIQUE,description TEXT NOT NULL,action TEXT NOT NULL,position INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS filter_settings(key TEXT PRIMARY KEY,value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS thread_filters(thread_id TEXT PRIMARY KEY,filter_id TEXT,message_id TEXT NOT NULL,confidence REAL NOT NULL,reason TEXT NOT NULL,source TEXT NOT NULL,decided_at_ms INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS thread_filters_filter ON thread_filters(filter_id,decided_at_ms);
CREATE TABLE IF NOT EXISTS filter_senders(account_id TEXT NOT NULL,address TEXT NOT NULL,filter_id TEXT,from_header TEXT NOT NULL,subject TEXT NOT NULL,created_at_ms INTEGER NOT NULL,PRIMARY KEY(account_id,address));",
    )?;
    Ok(())
}

fn setting(db: &Connection, key: &str) -> Result<Option<String>> {
    Ok(db
        .query_row("SELECT value FROM filter_settings WHERE key=?1", [key], |r| r.get(0))
        .optional()?)
}

fn set_setting(db: &Connection, key: &str, value: &str) -> Result<()> {
    db.execute(
        "INSERT INTO filter_settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        params![key, value],
    )?;
    Ok(())
}

/// Lowercased address from `Name <a@b>` or a bare address.
pub fn address_of(header: &str) -> String {
    let h = header.trim();
    let inner = match (h.rfind('<'), h.rfind('>')) {
        (Some(a), Some(b)) if b > a => &h[a + 1..b],
        _ => h,
    };
    inner.trim().trim_matches('"').to_lowercase()
}

fn domain_of(address: &str) -> Option<&str> {
    address.rsplit_once('@').map(|(_, d)| d).filter(|d| !d.is_empty())
}

const PUBLIC_DOMAINS: &[&str] = &[
    "gmail.com", "googlemail.com", "outlook.com", "hotmail.com", "live.com", "msn.com",
    "icloud.com", "me.com", "mac.com", "yahoo.com", "aol.com", "proton.me", "protonmail.com",
    "pm.me", "gmx.com", "gmx.net", "gmx.de", "fastmail.com", "hey.com", "online.no",
];

fn validate(name: &str, description: &str) -> Result<()> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 40 || name.chars().any(char::is_control) {
        return Err(MailError::InvalidAction("Give the filter a name of up to 40 characters".into()));
    }
    if name.eq_ignore_ascii_case(EVERYTHING_ELSE_NAME) || name.eq_ignore_ascii_case("inbox") {
        return Err(MailError::InvalidAction(format!("\"{name}\" is already used for your inbox")));
    }
    let description = description.trim();
    if description.is_empty() {
        return Err(MailError::InvalidAction("Describe what goes in this filter".into()));
    }
    if description.chars().count() > 400 || description.contains('\0') {
        return Err(MailError::InvalidAction("Keep the description under 400 characters".into()));
    }
    Ok(())
}

impl MailStore {
    pub fn list_filters(&self) -> Result<Vec<MailFilter>> {
        let db = self.lock()?;
        list_filters(&db)
    }

    pub fn filters_enabled(&self) -> Result<bool> {
        let db = self.lock()?;
        Ok(setting(&db, "enabled")?.as_deref() == Some("1"))
    }

    /// Turn sorting on or off. Turning it on the first time with no filters
    /// creates the given starters (all of them when `starter_keys` is None).
    pub fn set_filters_enabled(&self, enabled: bool, starter_keys: Option<&[String]>) -> Result<()> {
        let mut db = self.lock()?;
        let tx = db.transaction()?;
        if enabled {
            let count: i64 = tx.query_row("SELECT COUNT(*) FROM mail_filters", [], |r| r.get(0))?;
            if count == 0 {
                for (position, starter) in starters()
                    .into_iter()
                    .filter(|s| starter_keys.is_none_or(|keys| keys.iter().any(|k| *k == s.key)))
                    .enumerate()
                {
                    tx.execute(
                        "INSERT INTO mail_filters(id,name,description,action,position) VALUES(?1,?2,?3,?4,?5)",
                        params![Uuid::new_v4().to_string(), starter.name, starter.description, starter.action.as_str(), position as i64],
                    )?;
                }
            }
            if setting(&tx, "enabled_at_ms")?.is_none() {
                set_setting(&tx, "enabled_at_ms", &now_ms().to_string())?;
            }
        }
        set_setting(&tx, "enabled", if enabled { "1" } else { "0" })?;
        tx.commit()?;
        Ok(())
    }

    pub fn upsert_filter(
        &self,
        id: Option<&str>,
        name: &str,
        description: &str,
        action: FilterAction,
    ) -> Result<MailFilter> {
        validate(name, description)?;
        let mut db = self.lock()?;
        let tx = db.transaction()?;
        let name = name.trim();
        let description = description.trim();
        let clash: Option<String> = tx
            .query_row(
                "SELECT id FROM mail_filters WHERE name=?1 COLLATE NOCASE AND id IS NOT ?2",
                params![name, id],
                |r| r.get(0),
            )
            .optional()?;
        if clash.is_some() {
            return Err(MailError::InvalidAction(format!("You already have a filter called \"{name}\"")));
        }
        let id = match id {
            Some(id) => {
                let changed = tx.execute(
                    "UPDATE mail_filters SET name=?2,description=?3,action=?4 WHERE id=?1",
                    params![id, name, description, action.as_str()],
                )?;
                if changed == 0 {
                    return Err(MailError::InvalidAction("That filter no longer exists".into()));
                }
                id.to_owned()
            }
            None => {
                let count: i64 = tx.query_row("SELECT COUNT(*) FROM mail_filters", [], |r| r.get(0))?;
                if count as usize >= MAX_FILTERS {
                    return Err(MailError::InvalidAction(format!("You can have up to {MAX_FILTERS} filters")));
                }
                let id = Uuid::new_v4().to_string();
                let position: i64 = tx.query_row("SELECT COALESCE(MAX(position)+1,0) FROM mail_filters", [], |r| r.get(0))?;
                tx.execute(
                    "INSERT INTO mail_filters(id,name,description,action,position) VALUES(?1,?2,?3,?4,?5)",
                    params![id, name, description, action.as_str(), position],
                )?;
                id
            }
        };
        let filter = filter_by_id(&tx, &id)?.expect("just written");
        tx.commit()?;
        Ok(filter)
    }

    /// Delete a filter. Its conversations stay where they are (archived mail
    /// stays in Archive); they just lose the tag.
    pub fn delete_filter(&self, id: &str) -> Result<()> {
        let mut db = self.lock()?;
        let tx = db.transaction()?;
        if tx.execute("DELETE FROM mail_filters WHERE id=?1", [id])? == 0 {
            return Err(MailError::InvalidAction("That filter no longer exists".into()));
        }
        tx.execute("DELETE FROM thread_filters WHERE filter_id=?1", [id])?;
        tx.execute("DELETE FROM filter_senders WHERE filter_id=?1", [id])?;
        tx.commit()?;
        Ok(())
    }

    pub fn reorder_filters(&self, ids: &[String]) -> Result<()> {
        let mut db = self.lock()?;
        let tx = db.transaction()?;
        for (position, id) in ids.iter().enumerate() {
            tx.execute("UPDATE mail_filters SET position=?2 WHERE id=?1", params![id, position as i64])?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn filter_counts(&self) -> Result<Vec<FilterCount>> {
        let db = self.lock()?;
        let since = now_ms() - 7 * 86_400_000;
        let mut q = db.prepare(
            "SELECT f.id,
                COALESCE(SUM(CASE WHEN t.unread_count>0 THEN 1 ELSE 0 END),0),
                COALESCE(SUM(CASE WHEN t.last_message_at>=?1 THEN 1 ELSE 0 END),0)
             FROM mail_filters f
             LEFT JOIN thread_filters tf ON tf.filter_id=f.id
             LEFT JOIN threads t ON t.id=tf.thread_id
             GROUP BY f.id ORDER BY f.position",
        )?;
        let rows = q.query_map([since], |r| {
            Ok(FilterCount {
                filter_id: r.get(0)?,
                unread: r.get::<_, i64>(1)? as u32,
                last_7_days: r.get::<_, i64>(2)? as u32,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Conversations in a filter, newest first, wherever they are now.
    pub fn list_filter_threads(&self, filter_id: &str, limit: u32, offset: u32) -> Result<Vec<ThreadSummary>> {
        let db = self.lock()?;
        let mut q = db.prepare(
            "SELECT t.id,t.account_id,t.subject,t.sender,t.snippet,t.last_message_at,t.unread_count,t.is_inbox,t.folder
             FROM thread_filters tf JOIN threads t ON t.id=tf.thread_id
             WHERE tf.filter_id=?1 AND t.folder NOT IN ('Trash','Spam')
             ORDER BY t.last_message_at DESC,t.id LIMIT ?2 OFFSET ?3",
        )?;
        let mut rows = q.query(params![filter_id, limit.min(500), offset])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(thread_row(&db, row)?);
        }
        Ok(out)
    }

    /// Up to `limit` conversations the model should read, newest first.
    /// Conversations that need no model (contacts, senders sorted by hand, a
    /// follow-up in a conversation already filtered) are settled here and
    /// returned in the decisions list.
    pub fn filter_candidates(&self, limit: usize) -> Result<(Vec<FilterCandidate>, Vec<FilterDecision>)> {
        let mut db = self.lock()?;
        let tx = db.transaction()?;
        if setting(&tx, "enabled")?.as_deref() != Some("1") {
            return Ok((Vec::new(), Vec::new()));
        }
        let filters: HashMap<String, MailFilter> =
            list_filters(&tx)?.into_iter().map(|f| (f.id.clone(), f)).collect();
        let enabled_at: i64 = setting(&tx, "enabled_at_ms")?
            .and_then(|v| v.parse().ok())
            .unwrap_or_else(now_ms);
        let since = enabled_at - BACKFILL_DAYS * 86_400_000;
        let accounts: HashMap<String, String> = {
            let mut q = tx.prepare("SELECT id,email FROM accounts")?;
            q.query_map([], |r| Ok((r.get(0)?, address_of(&r.get::<_, String>(1)?))))?
                .collect::<std::result::Result<_, _>>()?
        };
        let mut contacts: HashMap<String, HashSet<String>> = HashMap::new();
        {
            let mut q = tx.prepare(
                "SELECT m.account_id,j.value FROM messages m,json_each(m.to_json) j WHERE m.is_sent=1 AND m.is_draft=0",
            )?;
            let rows = q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
            for row in rows {
                let (account, to) = row?;
                contacts.entry(account).or_default().insert(address_of(&to));
            }
        }
        let mut sorted_senders: HashMap<(String, String), Option<String>> = HashMap::new();
        {
            let mut q = tx.prepare("SELECT account_id,address,filter_id FROM filter_senders")?;
            let rows = q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?)))?;
            for row in rows {
                let (account, address, filter) = row?;
                sorted_senders.insert((account, address), filter);
            }
        }
        // Inbox conversations in the window whose newest message is incoming.
        let rows: Vec<(String, String, String, String, String, String, bool, bool)> = {
            let mut q = tx.prepare(
                "SELECT t.id,t.account_id,m.id,m.sender,m.subject,
                        CASE WHEN length(trim(m.body_plain))>0 THEN m.body_plain ELSE t.snippet END,
                        m.unsubscribe_header IS NOT NULL AND length(m.unsubscribe_header)>0,
                        EXISTS(SELECT 1 FROM messages s WHERE s.thread_id=t.id AND s.is_sent=1 AND s.is_draft=0)
                 FROM threads t
                 JOIN messages m ON m.id=(SELECT id FROM messages x WHERE x.thread_id=t.id AND x.is_draft=0 ORDER BY x.received_at DESC,x.id DESC LIMIT 1)
                 WHERE t.is_inbox=1 AND t.last_message_at>=?1 AND m.is_sent=0
                   AND NOT EXISTS(SELECT 1 FROM snoozes s WHERE s.thread_id=t.id AND s.wake_at_ms>?2)
                 ORDER BY t.last_message_at DESC,t.id",
            )?;
            q.query_map(params![since, now_ms()], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?))
            })?
            .collect::<std::result::Result<_, _>>()?
        };
        let mut candidates = Vec::new();
        let mut decisions = Vec::new();
        for (thread_id, account_id, message_id, sender, subject, body, bulk, replied) in rows {
            let existing: Option<(Option<String>, String, String)> = tx
                .query_row(
                    "SELECT filter_id,message_id,source FROM thread_filters WHERE thread_id=?1",
                    [&thread_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?;
            if let Some((filter_id, decided_message, source)) = existing {
                if decided_message == message_id {
                    continue;
                }
                // A new message arrived in a conversation already decided.
                let follow = !replied && source != "contact";
                let target = filter_id.filter(|_| follow).and_then(|id| filters.get(&id).cloned());
                let moved = match &target {
                    Some(filter) if filter.action.leaves_inbox() => {
                        apply_filter_action(&tx, &thread_id, &message_id, filter.action)?;
                        true
                    }
                    _ => false,
                };
                tx.execute(
                    "UPDATE thread_filters SET message_id=?2 WHERE thread_id=?1",
                    params![thread_id, message_id],
                )?;
                if moved {
                    decisions.push(FilterDecision { thread_id, filter_id: target.map(|f| f.id), moved });
                }
                continue;
            }
            let address = address_of(&sender);
            let own = accounts.get(&account_id);
            let same_company = match (domain_of(&address), own.and_then(|a| domain_of(a))) {
                (Some(a), Some(b)) => a == b && !PUBLIC_DOMAINS.contains(&a),
                _ => false,
            };
            let known = replied
                || same_company
                || own.is_some_and(|o| *o == address)
                || contacts.get(&account_id).is_some_and(|c| c.contains(&address));
            if known {
                record(&tx, &thread_id, &message_id, None, 1.0, "", "contact")?;
                continue;
            }
            if let Some(filter_id) = sorted_senders.get(&(account_id.clone(), address.clone())) {
                let filter = filter_id.as_ref().and_then(|id| filters.get(id)).cloned();
                record(&tx, &thread_id, &message_id, filter.as_ref().map(|f| f.id.as_str()), 1.0, "", "user")?;
                let moved = match &filter {
                    Some(f) if f.action.leaves_inbox() => {
                        apply_filter_action(&tx, &thread_id, &message_id, f.action)?;
                        true
                    }
                    _ => false,
                };
                decisions.push(FilterDecision { thread_id, filter_id: filter.map(|f| f.id), moved });
                continue;
            }
            if candidates.len() < limit {
                let body = with_earlier_messages(&tx, &thread_id, &message_id, &body)?;
                candidates.push(FilterCandidate {
                    thread_id,
                    account_id,
                    message_id,
                    sender,
                    subject,
                    body,
                    bulk,
                });
            }
        }
        tx.commit()?;
        Ok((candidates, decisions))
    }

    /// Conversations still waiting for the model.
    pub fn filter_backlog(&self) -> Result<usize> {
        // Settling contacts here is cheap and keeps the count honest.
        Ok(self.filter_candidates(usize::MAX)?.0.len())
    }

    /// Store the model's choice and apply the filter's action when it is
    /// confident. `filter_id` None is the catch-all.
    pub fn record_model_decision(
        &self,
        candidate: &FilterCandidate,
        filter_id: Option<&str>,
        confidence: f32,
        reason: &str,
    ) -> Result<FilterDecision> {
        let mut db = self.lock()?;
        let tx = db.transaction()?;
        // The conversation may have changed while the model was reading.
        let current: Option<String> = tx
            .query_row(
                "SELECT id FROM messages WHERE thread_id=?1 AND is_draft=0 ORDER BY received_at DESC,id DESC LIMIT 1",
                [&candidate.thread_id],
                |r| r.get(0),
            )
            .optional()?;
        let in_inbox: bool = tx
            .query_row("SELECT is_inbox FROM threads WHERE id=?1", [&candidate.thread_id], |r| r.get(0))
            .optional()?
            .unwrap_or(false);
        let filter = match filter_id {
            Some(id) => filter_by_id(&tx, id)?,
            None => None,
        };
        let chosen = filter.filter(|f| {
            confidence >= if f.action == FilterAction::Ignore { IGNORE_CONFIDENCE } else { APPLY_CONFIDENCE }
        });
        record(
            &tx,
            &candidate.thread_id,
            &candidate.message_id,
            chosen.as_ref().map(|f| f.id.as_str()),
            confidence,
            reason,
            "model",
        )?;
        let moved = match &chosen {
            Some(f) if f.action.leaves_inbox() && in_inbox && current.as_deref() == Some(candidate.message_id.as_str()) => {
                apply_filter_action(&tx, &candidate.thread_id, &candidate.message_id, f.action)?;
                true
            }
            _ => false,
        };
        tx.commit()?;
        Ok(FilterDecision { thread_id: candidate.thread_id.clone(), filter_id: chosen.map(|f| f.id), moved })
    }

    /// The person sorts a conversation by hand. Later mail from the same
    /// sender follows, and the model sees it as an example. `filter_id`
    /// None puts it back in the inbox.
    pub fn move_thread_to_filter(&self, thread_id: &str, filter_id: Option<&str>) -> Result<FilterDecision> {
        let mut db = self.lock()?;
        let tx = db.transaction()?;
        let (account_id, is_inbox): (String, bool) = tx
            .query_row("SELECT account_id,is_inbox FROM threads WHERE id=?1", [thread_id], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?
            .ok_or_else(|| MailError::InvalidAction(format!("unknown thread: {thread_id}")))?;
        let filter = match filter_id {
            Some(id) => Some(filter_by_id(&tx, id)?.ok_or_else(|| MailError::InvalidAction("That filter no longer exists".into()))?),
            None => None,
        };
        let (message_id, sender, subject): (String, String, String) = tx
            .query_row(
                "SELECT id,sender,subject FROM messages WHERE thread_id=?1 AND is_draft=0 AND is_sent=0 ORDER BY received_at DESC,id DESC LIMIT 1",
                [thread_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?
            .ok_or_else(|| MailError::InvalidAction("Only received mail can be sorted".into()))?;
        record(&tx, thread_id, &message_id, filter.as_ref().map(|f| f.id.as_str()), 1.0, "", "user")?;
        tx.execute(
            "INSERT INTO filter_senders(account_id,address,filter_id,from_header,subject,created_at_ms) VALUES(?1,?2,?3,?4,?5,?6)
             ON CONFLICT(account_id,address) DO UPDATE SET filter_id=excluded.filter_id,from_header=excluded.from_header,subject=excluded.subject,created_at_ms=excluded.created_at_ms",
            params![account_id, address_of(&sender), filter.as_ref().map(|f| f.id.clone()), sender, subject, now_ms()],
        )?;
        let moved = match &filter {
            Some(f) if f.action.leaves_inbox() => {
                if is_inbox {
                    apply_filter_action(&tx, thread_id, &format!("{message_id}:user"), f.action)?;
                }
                true
            }
            _ => {
                if !is_inbox {
                    queue(&tx, &format!("filter:{thread_id}:{message_id}:restore:{}", Uuid::new_v4()), MailAction::Move {
                        thread_id: thread_id.into(),
                        folder: "Inbox".into(),
                    })?;
                }
                !is_inbox
            }
        };
        tx.commit()?;
        Ok(FilterDecision { thread_id: thread_id.into(), filter_id: filter.map(|f| f.id), moved })
    }

    /// Recent hand-sorted senders, for the model's prompt.
    pub fn filter_examples(&self) -> Result<Vec<FilterExample>> {
        let db = self.lock()?;
        let mut q = db.prepare(
            "SELECT s.from_header,s.subject,f.name FROM filter_senders s LEFT JOIN mail_filters f ON f.id=s.filter_id
             ORDER BY s.created_at_ms DESC LIMIT ?1",
        )?;
        let rows = q.query_map([MAX_EXAMPLES as i64], |r| {
            Ok(FilterExample {
                from: r.get(0)?,
                subject: r.get(1)?,
                category: r.get::<_, Option<String>>(2)?.unwrap_or_else(|| EVERYTHING_ELSE_NAME.into()),
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }
}

/// A follow-up ("just bumping this") means little alone; show the model the
/// earlier messages of the conversation too, oldest first.
fn with_earlier_messages(db: &Connection, thread_id: &str, latest_id: &str, latest: &str) -> Result<String> {
    let mut q = db.prepare(
        "SELECT sender,CASE WHEN length(trim(body_plain))>0 THEN body_plain ELSE subject END FROM messages
         WHERE thread_id=?1 AND id<>?2 AND is_draft=0 ORDER BY received_at DESC,id DESC LIMIT 2",
    )?;
    let mut earlier: Vec<(String, String)> = q
        .query_map(params![thread_id, latest_id], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<std::result::Result<_, _>>()?;
    let latest: String = latest.chars().take(2400).collect();
    if earlier.is_empty() {
        return Ok(latest);
    }
    earlier.reverse();
    let mut out = String::from("Earlier in this conversation:\n");
    for (sender, body) in earlier {
        out.push_str(&format!("From {sender}: {}\n", body.chars().take(600).collect::<String>()));
    }
    out.push_str("\nLatest message:\n");
    out.push_str(&latest);
    Ok(out)
}

fn list_filters(db: &Connection) -> Result<Vec<MailFilter>> {
    let mut q = db.prepare("SELECT id,name,description,action,position FROM mail_filters ORDER BY position,name")?;
    let rows = q.query_map([], |r| {
        Ok(MailFilter {
            id: r.get(0)?,
            name: r.get(1)?,
            description: r.get(2)?,
            action: FilterAction::parse(&r.get::<_, String>(3)?),
            position: r.get(4)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

fn filter_by_id(db: &Connection, id: &str) -> Result<Option<MailFilter>> {
    Ok(list_filters(db)?.into_iter().find(|f| f.id == id))
}

fn record(
    tx: &Transaction<'_>,
    thread_id: &str,
    message_id: &str,
    filter_id: Option<&str>,
    confidence: f32,
    reason: &str,
    source: &str,
) -> Result<()> {
    tx.execute(
        "INSERT INTO thread_filters(thread_id,filter_id,message_id,confidence,reason,source,decided_at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7)
         ON CONFLICT(thread_id) DO UPDATE SET filter_id=excluded.filter_id,message_id=excluded.message_id,confidence=excluded.confidence,reason=excluded.reason,source=excluded.source,decided_at_ms=excluded.decided_at_ms",
        params![thread_id, filter_id, message_id, confidence as f64, reason, source, now_ms()],
    )?;
    Ok(())
}

fn apply_filter_action(tx: &Transaction<'_>, thread_id: &str, key: &str, action: FilterAction) -> Result<()> {
    if action.leaves_inbox() {
        queue(tx, &format!("filter:{thread_id}:{key}:archive"), MailAction::Archive { thread_id: thread_id.into() })?;
    }
    if action == FilterAction::Ignore {
        queue(tx, &format!("filter:{thread_id}:{key}:read"), MailAction::MarkRead { thread_id: thread_id.into(), read: true })?;
    }
    Ok(())
}

fn queue(tx: &Transaction<'_>, operation_id: &str, action: MailAction) -> Result<()> {
    crate::apply_action_tx(tx, operation_id, action)?;
    Ok(())
}

/// The filter tag for a thread row, if the thread is in a filter.
pub(crate) fn thread_tag(db: &Connection, thread_id: &str) -> Result<Option<ThreadFilterTag>> {
    Ok(db
        .query_row(
            "SELECT f.id,f.name,f.action,tf.reason,tf.source FROM thread_filters tf JOIN mail_filters f ON f.id=tf.filter_id WHERE tf.thread_id=?1",
            [thread_id],
            |r| {
                Ok(ThreadFilterTag {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    action: FilterAction::parse(&r.get::<_, String>(2)?),
                    reason: r.get(3)?,
                    source: r.get(4)?,
                })
            },
        )
        .optional()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_normalized() {
        assert_eq!(address_of("Ravi <Ravi@Brightforge.io>"), "ravi@brightforge.io");
        assert_eq!(address_of(" sara@acme.no "), "sara@acme.no");
    }
}
