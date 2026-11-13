//! Synthetic demo mailboxes for screenshots, recordings and contributors.
//!
//! Everything here is invented: people, companies and domains (`example.*`
//! and `.test`), message text, pictures and documents. The content is fixed
//! and deterministic; only dates follow the moment of seeding, so the inbox
//! always reads "today", "yesterday" and "this week".
//!
//! Callers decide whether a store is disposable. `mail-service` refuses to
//! seed the default mail store, and the desktop apps only seed a store named
//! by `EMAIL_STORE_PATH`.
mod bulk;
mod clock;
mod content;
mod files;

use crate::{Account, MailStore, Message, Provider, RecipientHeaders, Result, upsert_message_tx};
use clock::Clock;
use files::Part;
use rusqlite::{OptionalExtension, params};
use serde::Serialize;

pub use bulk::SearchCorpusSummary;

/// The work account of the launch demo.
pub const DEMO_WORK_ACCOUNT: &str = "demo:work";
/// The personal account of the launch demo (also used by the small demo).
pub const DEMO_PERSONAL_ACCOUNT: &str = "demo:personal";

const RAW_SCHEMA: &str = "CREATE TABLE IF NOT EXISTS demo_raw_messages(message_id TEXT PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,raw BLOB NOT NULL)";

/// What `seed_launch_demo` wrote. `seeded` is false when the store already
/// holds the launch demo; nothing is changed then.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct LaunchDemoSummary {
    pub seeded: bool,
    pub accounts: usize,
    pub threads: usize,
    pub messages: usize,
    pub inbox_threads: usize,
    pub drafts: usize,
    pub scheduled: usize,
}

/// A person or sender: display name and address.
#[derive(Debug, Clone, Copy)]
pub(crate) struct P(pub &'static str, pub &'static str);
impl P {
    pub fn addr(&self) -> String {
        format!("{} <{}>", self.0, self.1)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Place {
    Inbox,
    Archive,
    Sent,
}

#[derive(Debug, Clone)]
pub(crate) struct Msg {
    /// None means the account owner, which also marks the message as sent.
    pub from: Option<String>,
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub subject: Option<String>,
    pub at: i64,
    pub body: String,
    pub html: Option<String>,
    pub parts: Vec<Part>,
    pub read: bool,
    pub unsubscribe: Option<String>,
}
impl Msg {
    pub fn to(mut self, people: &[P]) -> Self {
        self.to = people.iter().map(P::addr).collect();
        self
    }
    pub fn cc(mut self, people: &[P]) -> Self {
        self.cc = people.iter().map(P::addr).collect();
        self
    }
    pub fn unread(mut self) -> Self {
        self.read = false;
        self
    }
    pub fn read_if(mut self, read: bool) -> Self {
        self.read = read;
        self
    }
    pub fn html(mut self, html: impl Into<String>) -> Self {
        self.html = Some(html.into());
        self
    }
    pub fn part(mut self, part: Part) -> Self {
        self.parts.push(part);
        self
    }
    pub fn unsubscribe(mut self, header: impl Into<String>) -> Self {
        self.unsubscribe = Some(header.into());
        self
    }
}

/// A message from someone else, read unless marked unread.
pub(crate) fn from(person: P, at: i64, body: impl Into<String>) -> Msg {
    Msg {
        from: Some(person.addr()),
        to: Vec::new(),
        cc: Vec::new(),
        subject: None,
        at,
        body: body.into(),
        html: None,
        parts: Vec::new(),
        read: true,
        unsubscribe: None,
    }
}

/// A message the account owner sent.
pub(crate) fn me(at: i64, body: impl Into<String>) -> Msg {
    Msg {
        from: None,
        ..from(P("", ""), at, body)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Thread {
    pub key: String,
    pub account: &'static str,
    pub subject: String,
    pub labels: Vec<&'static str>,
    pub place: Place,
    pub pinned: bool,
    pub snooze_until: Option<i64>,
    pub follow_up_at: Option<i64>,
    pub messages: Vec<Msg>,
}
impl Thread {
    pub fn work(key: impl Into<String>, subject: impl Into<String>) -> Self {
        Self::new(DEMO_WORK_ACCOUNT, key, subject)
    }
    pub fn home(key: impl Into<String>, subject: impl Into<String>) -> Self {
        Self::new(DEMO_PERSONAL_ACCOUNT, key, subject)
    }
    fn new(account: &'static str, key: impl Into<String>, subject: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            account,
            subject: subject.into(),
            labels: Vec::new(),
            place: Place::Inbox,
            pinned: false,
            snooze_until: None,
            follow_up_at: None,
            messages: Vec::new(),
        }
    }
    pub fn label(mut self, label: &'static str) -> Self {
        self.labels.push(label);
        self
    }
    pub fn archived(mut self) -> Self {
        self.place = Place::Archive;
        self
    }
    pub fn archived_if(mut self, archived: bool) -> Self {
        if archived {
            self.place = Place::Archive;
        }
        self
    }
    pub fn sent(mut self) -> Self {
        self.place = Place::Sent;
        self
    }
    pub fn pinned(mut self) -> Self {
        self.pinned = true;
        self
    }
    pub fn snoozed(mut self, until: i64) -> Self {
        self.snooze_until = Some(until);
        self
    }
    pub fn follow_up(mut self, at: i64) -> Self {
        self.follow_up_at = Some(at);
        self
    }
    pub fn msg(mut self, message: Msg) -> Self {
        self.messages.push(message);
        self
    }
    pub fn id(&self) -> String {
        format!("demo:launch:{}", self.key)
    }
}

/// An unsent draft, optionally a reply in an existing thread or scheduled.
#[derive(Debug, Clone)]
pub(crate) struct Draft {
    pub key: &'static str,
    pub account: &'static str,
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub subject: String,
    pub body: String,
    pub reply_to: Option<&'static str>,
    pub send_at: Option<i64>,
    pub saved_at: i64,
}

#[derive(Debug, Default)]
pub(crate) struct Demo {
    pub threads: Vec<Thread>,
    pub drafts: Vec<Draft>,
    pub splits: Vec<(&'static str, &'static str, &'static str)>,
    pub snippets: Vec<(&'static str, &'static str)>,
    pub signatures: Vec<(&'static str, &'static str, &'static str)>,
}

fn owner(account: &str) -> P {
    if account == DEMO_WORK_ACCOUNT {
        content::ALEX
    } else {
        content::ALEX_HOME
    }
}

impl MailStore {
    /// Seed the launch demo: two accounts and about 300 conversations with
    /// replies, attachments, invitations, newsletters, labels, splits, stars,
    /// snoozes, follow-ups, drafts and scheduled mail. Dates are relative to
    /// `now_ms` in the viewer's `utc_offset_minutes`.
    ///
    /// Callers must only pass a disposable store. Running it again on a store
    /// that already holds the launch demo changes nothing.
    pub fn seed_launch_demo(
        &self,
        now_ms: i64,
        utc_offset_minutes: i32,
    ) -> Result<LaunchDemoSummary> {
        let existing = self
            .list_accounts()?
            .iter()
            .any(|account| account.id == DEMO_WORK_ACCOUNT);
        if existing {
            // The work signature is written last: without it an earlier run
            // stopped part-way, and a new store is the only clean start.
            if self
                .get_signature(DEMO_WORK_ACCOUNT, content::ALEX.1)?
                .is_none()
            {
                return Err(crate::MailError::InvalidAction(
                    "This store holds an incomplete launch demo. Start from a new store.".into(),
                ));
            }
            return Ok(LaunchDemoSummary {
                seeded: false,
                accounts: 0,
                threads: 0,
                messages: 0,
                inbox_threads: 0,
                drafts: 0,
                scheduled: 0,
            });
        }
        let clock = Clock::new(now_ms, utc_offset_minutes);
        let demo = content::launch(&clock);
        for (id, person) in [
            (DEMO_WORK_ACCOUNT, content::ALEX),
            (DEMO_PERSONAL_ACCOUNT, content::ALEX_HOME),
        ] {
            self.upsert_account(&Account {
                id: id.into(),
                provider: Provider::Gmail,
                email: person.1.into(),
                display_name: person.0.into(),
            })?;
        }
        let mut invitations = Vec::new();
        let mut message_count = 0;
        {
            let mut db = self.lock()?;
            let tx = db.transaction()?;
            tx.execute_batch(RAW_SCHEMA)?;
            for thread in &demo.threads {
                let thread_id = thread.id();
                let me = owner(thread.account);
                for (index, message) in thread.messages.iter().enumerate() {
                    let id = format!("{thread_id}:{index}");
                    let subject = message.subject.clone().unwrap_or_else(|| {
                        if index == 0 {
                            thread.subject.clone()
                        } else {
                            format!("Re: {}", thread.subject)
                        }
                    });
                    let sent = message.from.is_none();
                    let to = if message.to.is_empty() {
                        vec![me.addr()]
                    } else {
                        message.to.clone()
                    };
                    upsert_message_tx(
                        &tx,
                        &Message {
                            recipient_headers: Some(RecipientHeaders {
                                cc: message.cc.clone(),
                                bcc: Vec::new(),
                                reply_to: Vec::new(),
                            }),
                            id: id.clone(),
                            thread_id: thread_id.clone(),
                            account_id: thread.account.into(),
                            sender: message.from.clone().unwrap_or_else(|| me.addr()),
                            to,
                            subject,
                            body_plain: message.body.clone(),
                            body_html: message.html.clone(),
                            unsubscribe_header: message.unsubscribe.clone(),
                            has_attachments: Some(message.parts.iter().any(|part| part.listed)),
                            received_at: message.at,
                            is_read: message.read || sent,
                            is_draft: false,
                            is_sent: sent,
                        },
                    )?;
                    if !message.parts.is_empty() {
                        let raw = files::mime_message(
                            &message.body,
                            message.html.as_deref(),
                            &message.parts,
                        );
                        tx.execute(
                            "INSERT OR REPLACE INTO demo_raw_messages(message_id,raw) VALUES(?1,?2)",
                            params![id, raw],
                        )?;
                    }
                    for part in &message.parts {
                        if part.mime_type.starts_with("text/calendar") {
                            invitations.push((id.clone(), part.bytes.clone()));
                        }
                    }
                    message_count += 1;
                }
                match thread.place {
                    Place::Inbox => {}
                    Place::Archive => {
                        tx.execute(
                            "UPDATE threads SET is_inbox=0,folder='Archive' WHERE id=?1",
                            [&thread_id],
                        )?;
                    }
                    Place::Sent => {
                        tx.execute(
                            "UPDATE threads SET is_inbox=0,folder='Sent' WHERE id=?1",
                            [&thread_id],
                        )?;
                    }
                }
                for label in &thread.labels {
                    tx.execute(
                        "INSERT OR IGNORE INTO thread_labels(thread_id,label) VALUES(?1,?2)",
                        params![thread_id, label],
                    )?;
                }
            }
            tx.commit()?;
        }
        for (message_id, bytes) in invitations {
            let events = crate::parse_calendar_invitations(&bytes)?;
            self.cache_calendar_events(&message_id, &events)?;
        }
        for thread in &demo.threads {
            let id = thread.id();
            if thread.pinned {
                self.set_thread_pinned(&id, true)?;
            }
            if let Some(until) = thread.snooze_until {
                self.snooze_thread(&id, until)?;
            }
            if let Some(at) = thread.follow_up_at {
                self.set_followup(&id, at, true)?;
            }
        }
        let mut scheduled = 0;
        for draft in &demo.drafts {
            let me = owner(draft.account);
            let id = format!("local:demo-{}", draft.key);
            let thread_id = draft
                .reply_to
                .map(|key| format!("demo:launch:{key}"))
                .unwrap_or_else(|| id.clone());
            self.upsert_message(&Message {
                recipient_headers: Some(RecipientHeaders {
                    cc: draft.cc.clone(),
                    bcc: Vec::new(),
                    reply_to: Vec::new(),
                }),
                id: id.clone(),
                thread_id: thread_id.clone(),
                account_id: draft.account.into(),
                sender: me.addr(),
                to: draft.to.clone(),
                subject: draft.subject.clone(),
                body_plain: draft.body.clone(),
                body_html: None,
                unsubscribe_header: None,
                has_attachments: Some(false),
                received_at: draft.saved_at,
                is_read: true,
                is_draft: true,
                is_sent: false,
            })?;
            if draft.reply_to.is_none() {
                self.lock()?.execute(
                    "UPDATE threads SET is_inbox=0,folder='Drafts' WHERE id=?1",
                    [&thread_id],
                )?;
            }
            if let Some(at) = draft.send_at {
                self.schedule_draft(&id, at, "scheduled")?;
                scheduled += 1;
            }
        }
        for (id, name, query) in &demo.splits {
            self.upsert_split(Some(id), name, query)?;
        }
        for (name, body) in &demo.snippets {
            self.upsert_snippet(None, name, body)?;
        }
        for (account, email, body) in &demo.signatures {
            self.upsert_signature(account, email, body, true, true)?;
        }
        let inbox_threads = demo
            .threads
            .iter()
            .filter(|thread| thread.place == Place::Inbox)
            .count();
        Ok(LaunchDemoSummary {
            seeded: true,
            accounts: 2,
            threads: demo.threads.len(),
            messages: message_count,
            inbox_threads,
            drafts: demo.drafts.len() - scheduled,
            scheduled,
        })
    }

    /// The stored MIME source of a demo message, which stands in for the
    /// provider when listing or opening its attachments, inline pictures and
    /// invitations. Always None for mail that was not seeded as a demo.
    pub fn demo_raw_message(&self, message_id: &str) -> Result<Option<Vec<u8>>> {
        let db = self.lock()?;
        let exists = db
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name='demo_raw_messages'",
                [],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !exists {
            return Ok(None);
        }
        db.query_row(
            "SELECT raw FROM demo_raw_messages WHERE message_id=?1",
            [message_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SearchScope;

    /// Thursday 12 November 2026, 09:30 UTC, the moment the content was
    /// written for, moved forward by whole weeks so it is never in the past:
    /// seeding sets snoozes and follow-ups after it, and the store rejects
    /// those unless they are in the future. Fixed for the whole test run.
    pub(super) fn now() -> i64 {
        static NOW: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
        *NOW.get_or_init(|| {
            const WRITTEN_FOR: i64 = 1_794_475_800_000;
            const WEEK: i64 = 7 * clock::DAY;
            let weeks = (crate::now_ms() - WRITTEN_FOR).div_euclid(WEEK) + 1;
            WRITTEN_FOR + weeks.max(0) * WEEK
        })
    }

    fn seeded() -> MailStore {
        let store = MailStore::open_in_memory().unwrap();
        let summary = store.seed_launch_demo(now(), 60).unwrap();
        assert!(summary.seeded);
        store
    }

    #[test]
    fn launch_demo_has_a_busy_believable_shape() {
        let store = seeded();
        let summary = store.seed_launch_demo(now(), 60).unwrap();
        assert!(!summary.seeded, "a second run must not duplicate the demo");
        let threads: i64 = store
            .lock()
            .unwrap()
            .query_row(
                "SELECT count(*) FROM threads WHERE id LIKE 'demo:launch:%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!((250..=400).contains(&threads), "{threads} threads");
        let inbox = store.list_threads(None, 500, 0).unwrap();
        assert!(
            (60..=140).contains(&inbox.len()),
            "{} inbox threads",
            inbox.len()
        );
        assert!(inbox.iter().all(|t| t.last_message_at < now()));
        // The newest conversations are from the last hour, unread.
        assert!(inbox[0].last_message_at > now() - 60 * 60_000);
        assert!(inbox[0].unread_count > 0);
        assert!(store.list_snoozed_threads().unwrap().len() >= 4);
        assert!(store.list_pinned_threads(100, 0).unwrap().len() >= 5);
        assert!(store.list_drafts(None).unwrap().len() >= 3);
        assert!(
            store
                .list_outbox()
                .unwrap()
                .iter()
                .filter(|o| o.status == "scheduled")
                .count()
                >= 2
        );
        assert!(store.list_splits().unwrap().len() >= 4);
        assert!(store.list_calendar_invitations(None, 100).unwrap().len() >= 4);
        let sent = store
            .search_scoped(
                "",
                500,
                0,
                &SearchScope {
                    folder: Some("sent".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(sent.len() >= 15, "{} sent", sent.len());
        let norwegian = store.search("hytta", 10).unwrap();
        assert!(!norwegian.is_empty());
        let labels = store.list_labels().unwrap();
        for name in ["Launch", "Newsletters", "Promotions", "Receipts", "Travel"] {
            assert!(
                labels.iter().any(|l| l.name == name),
                "missing label {name}"
            );
        }
    }

    #[test]
    fn an_interrupted_seed_is_reported_not_kept() {
        let store = seeded();
        store
            .delete_signature(DEMO_WORK_ACCOUNT, content::ALEX.1)
            .unwrap();
        let error = store.seed_launch_demo(now(), 60).unwrap_err();
        assert!(error.to_string().contains("incomplete launch demo"));
    }

    #[test]
    fn launch_demo_is_deterministic_apart_from_dates() {
        let a = seeded();
        let b = MailStore::open_in_memory().unwrap();
        b.seed_launch_demo(now() + 3 * clock::DAY, 0).unwrap();
        let rows = |store: &MailStore| -> Vec<(String, String, String)> {
            let db = store.lock().unwrap();
            let mut q = db
                .prepare("SELECT id,subject,body_plain FROM messages ORDER BY id")
                .unwrap();
            q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get(1)?, r.get(2)?)))
                .unwrap()
                .map(|r| r.unwrap())
                .filter(|(id, _, _)| !id.starts_with("local:"))
                .collect()
        };
        let (left, right) = (rows(&a), rows(&b));
        assert_eq!(left.len(), right.len());
        assert_eq!(
            left.iter().map(|r| &r.0).collect::<Vec<_>>(),
            right.iter().map(|r| &r.0).collect::<Vec<_>>()
        );
    }

    #[test]
    fn launch_demo_uses_fictional_domains_only() {
        let store = seeded();
        let db = store.lock().unwrap();
        let mut q = db
            .prepare("SELECT sender,to_json,body_plain,coalesce(body_html,'') FROM messages")
            .unwrap();
        let rows = q
            .query_map([], |r| {
                Ok(format!(
                    "{} {} {} {}",
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?
                ))
            })
            .unwrap();
        let address = regex_lite_domains();
        for text in rows {
            let text = text.unwrap();
            for domain in address(&text) {
                assert!(
                    domain.ends_with(".example")
                        || domain.ends_with(".test")
                        || matches!(
                            domain.as_str(),
                            "example.com" | "example.org" | "example.net"
                        )
                        || domain.ends_with(".example.com")
                        || domain.ends_with(".example.org")
                        || domain.ends_with(".example.net"),
                    "non-fictional domain {domain} in {text}"
                );
            }
        }
    }

    /// Hostnames after `@` or `://`, without a regex dependency.
    fn regex_lite_domains() -> impl Fn(&str) -> Vec<String> {
        |text: &str| {
            let mut out = Vec::new();
            for marker in ["@", "://"] {
                for piece in text.split(marker).skip(1) {
                    let host: String = piece
                        .chars()
                        .take_while(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '-')
                        .collect::<String>()
                        .trim_end_matches('.')
                        .to_ascii_lowercase();
                    if host.contains('.') {
                        out.push(host);
                    }
                }
            }
            out
        }
    }

    #[test]
    fn demo_files_are_served_from_the_stored_source() {
        let store = seeded();
        let db = store.lock().unwrap();
        let ids: Vec<String> = db
            .prepare("SELECT message_id FROM demo_raw_messages")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        drop(db);
        assert!(ids.len() >= 12);
        for id in ids {
            let raw = store.demo_raw_message(&id).unwrap().unwrap();
            let parsed = mailparse::parse_mail(&raw).unwrap();
            assert_eq!(parsed.ctype.mimetype, "multipart/mixed");
        }
        assert_eq!(store.demo_raw_message("demo:launch:missing").unwrap(), None);
        let plain = MailStore::open_in_memory().unwrap();
        plain.seed_demo().unwrap();
        assert_eq!(plain.demo_raw_message("demo:1").unwrap(), None);
    }
}
