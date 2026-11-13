//! Local-first mail model and cache shared by every Email front end.
//!
//! `mail-core` is the provider-independent heart of the engine. It defines the
//! normalized model ([`Account`], [`Message`], [`ThreadSummary`], [`MailAction`])
//! and [`MailStore`], a SQLite cache with full-text search, local drafts, an
//! outbox for scheduled and undoable sends, labels, saved splits, snoozes and a
//! durable queue of actions to replay against the provider.
//!
//! Provider adapters implement [`MailAdapter`] and return a [`SyncBatch`];
//! [`MailStore::run_sync`] applies it, including staged cursor resets that keep
//! the existing cache until a full refetch completes. `mail-imap` and
//! `mail-cloud` are the adapters this repository ships.
//!
//! The crate does no network I/O and stores no credentials. A new front end
//! (another desktop shell, a TUI, a test harness) can read the cache directly
//! through [`MailStore`], or use `mail-service` for account setup, credentials
//! and sync. A small C ABI (`include/mail_core.h`) exposes a JSON request API
//! over the store for non-Rust callers.
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{fs, path::Path, sync::Mutex};
use uuid::Uuid;

mod access;
mod agent;
pub use agent::{
    AGENT_APPROVAL_KINDS, AGENT_DISCLOSURE_MIN_VERSION, AGENT_DISCLOSURE_VERSION, AGENT_GRANTS,
    AgentAction, AgentApprovalRequest, AgentConsent, DEFAULT_APPROVAL_UNDO_SECONDS,
    agent_grant_for, agent_grant_label, agent_quoted_subject,
};
mod agenda;
pub use agenda::{AgendaAttendee, AgendaEvent, AvailabilitySlot, CalendarCollection};
mod calendar;
mod discarded_drafts;
mod followups;
pub use discarded_drafts::DiscardedDraft;
mod signatures;
pub use calendar::{CalendarEvent, CalendarInvitation, parse_calendar_invitations};
pub use followups::FollowupReminder;
pub use signatures::IdentitySignature;
mod cleanup;
mod demo;
pub use demo::{
    DEMO_PERSONAL_ACCOUNT, DEMO_WORK_ACCOUNT, LaunchDemoSummary, SearchCorpusSummary,
};
mod recipient_suggestions;
mod search;
mod snippets;
mod notification_text;
pub use notification_text::notification_snippet;
mod tracking;
pub use tracking::{EmailTracking, TrackingDetail, TrackingImageEvent, TrackingMessage};
pub use cleanup::{CleanupLease, SenderCleanup};
pub use recipient_suggestions::RecipientSuggestion;
pub use search::SearchScope;
pub use snippets::Snippet;

pub type Result<T> = std::result::Result<T, MailError>;
#[derive(Debug, thiserror::Error)]
pub enum MailError {
    #[error("file error: {0}")]
    Io(#[from] std::io::Error),
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("serialization error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("mail store lock poisoned")]
    Lock,
    #[error("unknown account: {0}")]
    UnknownAccount(String),
    #[error("unknown thread: {0}")]
    NotFound(String),
    #[error("invalid action: {0}")]
    InvalidAction(String),
    #[error("provider error: {0}")]
    Provider(String),
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Gmail,
    Microsoft,
    Icloud,
    Imap,
}
impl Provider {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Gmail => "gmail",
            Self::Microsoft => "microsoft",
            Self::Icloud => "icloud",
            Self::Imap => "imap",
        }
    }
    fn parse(s: &str) -> Self {
        match s {
            "gmail" => Self::Gmail,
            "microsoft" => Self::Microsoft,
            "icloud" => Self::Icloud,
            _ => Self::Imap,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct Account {
    pub id: String,
    pub provider: Provider,
    pub email: String,
    pub display_name: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ThreadSummary {
    pub id: String,
    pub account_id: String,
    pub subject: String,
    pub sender: String,
    pub snippet: String,
    pub last_message_at: i64,
    pub unread_count: u32,
    pub is_inbox: bool,
    pub folder: String,
    pub labels: Vec<String>,
    #[serde(default)]
    pub is_pinned: bool,
    #[serde(default)]
    pub snoozed_until_ms: Option<i64>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct LabelSummary {
    pub name: String,
    pub count: u32,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct SplitInbox {
    pub id: String,
    pub name: String,
    pub query: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct OutboxItem {
    pub message_id: String,
    pub account_id: String,
    pub to: Vec<String>,
    pub subject: String,
    pub body_preview: String,
    pub send_at_ms: i64,
    pub status: String,
    pub last_error: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct DraftAttachmentMeta {
    pub id: String,
    pub message_id: String,
    pub filename: String,
    pub mime_type: String,
    pub size: u64,
}
#[derive(Debug, Clone)]
pub struct OutgoingAttachment {
    pub filename: String,
    pub mime_type: String,
    pub bytes: Vec<u8>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct RecipientHeaders {
    pub cc: Vec<String>,
    pub bcc: Vec<String>,
    pub reply_to: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct Message {
    #[serde(default)]
    pub recipient_headers: Option<RecipientHeaders>,
    pub id: String,
    pub thread_id: String,
    pub account_id: String,
    pub sender: String,
    pub to: Vec<String>,
    pub subject: String,
    pub body_plain: String,
    pub body_html: Option<String>,
    #[serde(default)]
    pub unsubscribe_header: Option<String>,
    #[serde(default)]
    pub has_attachments: Option<bool>,
    pub received_at: i64,
    pub is_read: bool,
    pub is_draft: bool,
    pub is_sent: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MailAction {
    Archive { thread_id: String },
    MarkRead { thread_id: String, read: bool },
    Move { thread_id: String, folder: String },
    AddLabel { thread_id: String, label: String },
    RemoveLabel { thread_id: String, label: String },
    SendDraft { message_id: String },
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ActionResult {
    pub operation_id: String,
    pub applied: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct QueuedAction {
    pub operation_id: String,
    pub action: MailAction,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct SyncCursor {
    pub account_id: String,
    pub value: String,
    pub updated_at: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct SyncBatch {
    /// Provider flag changes for already-cached messages, without replacing bodies or headers.
    #[serde(default)]
    pub message_states: Vec<MessageState>,
    pub messages: Vec<Message>,
    /// Current provider labels for threads touched by this batch.
    #[serde(default)]
    pub thread_labels: Vec<(String, Vec<String>)>,
    pub removed_message_ids: Vec<String>,
    pub full_reset: bool,
    #[serde(default)]
    pub reset_phase: Option<ResetPhase>,
    pub next_cursor: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct MessageState {
    pub id: String,
    pub is_read: bool,
    pub is_draft: bool,
    pub is_sent: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResetPhase {
    Start,
    Continue,
    Complete,
    StartComplete,
}

/// Each provider adapter translates remote mail into the normalized model.
/// Message and thread IDs must include the account namespace.
pub trait MailAdapter: Send + Sync {
    fn provider(&self) -> Provider;
    fn sync(&self, account: &Account, cursor: Option<&str>) -> Result<SyncBatch>;
    /// Cache index only; adapters must not hold the store lock across network work.
    fn sync_with_cached_messages(
        &self,
        account: &Account,
        cursor: Option<&str>,
        _cached: &std::collections::HashMap<String, String>,
    ) -> Result<SyncBatch> {
        self.sync(account, cursor)
    }
}

/// The connection is serialized behind a mutex; no credentials are stored here.
pub struct MailStore {
    connection: Mutex<Connection>,
}
impl MailStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        // Creating a new private file requires a short-lived descriptor. Keep
        // creation/open serialized so another thread cannot open SQLite on that
        // new file before the creation descriptor has closed.
        static OPEN_LOCK: Mutex<()> = Mutex::new(());
        let _opening = OPEN_LOCK.lock().map_err(|_| MailError::Lock)?;
        let path = path.as_ref();
        if path == Path::new(":memory:") {
            return Self::open_in_memory();
        }
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                fs::DirBuilder::new()
                    .recursive(true)
                    .mode(0o700)
                    .create(parent)?;
            }
            #[cfg(not(unix))]
            fs::create_dir_all(parent)?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
            if fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
                return Err(MailError::InvalidAction(
                    "mail database path must not be a symlink".into(),
                ));
            }
            // Never open/close an existing database outside SQLite. On POSIX,
            // closing ANY descriptor for the inode releases this process's
            // advisory locks, including those held by other SQLite bridges.
            // A peer can then unlink the still-live WAL, causing IOERR/corruption.
            match fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(path)
            {
                Ok(file) => file.set_permissions(fs::Permissions::from_mode(0o600))?,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
                }
                Err(error) => return Err(error.into()),
            }
        }
        let store = Self::from_connection(Connection::open(path)?)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for suffix in ["-wal", "-shm"] {
                let aux = std::path::PathBuf::from(format!("{}{}", path.display(), suffix));
                if aux.exists() {
                    fs::set_permissions(aux, fs::Permissions::from_mode(0o600))?;
                }
            }
        }
        Ok(store)
    }
    pub fn open_in_memory() -> Result<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }
    fn from_connection(mut connection: Connection) -> Result<Self> {
        connection.execute_batch(
            "PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;",
        )?;
        // A failed upgrade must leave the previous schema and queued work intact.
        // IMMEDIATE also serializes upgrades from independent desktop/MCP bridges.
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        Self::initialize_schema(&transaction)?;
        transaction.commit()?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }
    fn initialize_schema(connection: &Connection) -> Result<()> {
        connection.execute_batch(SCHEMA)?;
        calendar::initialize(&connection)?;
        agenda::initialize(&connection)?;
        followups::initialize(&connection)?;
        discarded_drafts::initialize(&connection)?;
        signatures::initialize(&connection)?;
        agent::initialize(&connection)?;
        let has_unsubscribe_header = connection
            .prepare("PRAGMA table_info(messages)")?
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<std::result::Result<Vec<_>, _>>()?
            .iter()
            .any(|name| name == "unsubscribe_header");
        if !has_unsubscribe_header {
            connection.execute_batch("ALTER TABLE messages ADD COLUMN unsubscribe_header TEXT")?;
        }
        let has_attachment_flag = connection
            .prepare("PRAGMA table_info(messages)")?
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<std::result::Result<Vec<_>, _>>()?
            .iter()
            .any(|name| name == "has_attachments");
        if !has_attachment_flag {
            connection.execute_batch("ALTER TABLE messages ADD COLUMN has_attachments INTEGER")?;
        }
        let columns = connection
            .prepare("PRAGMA table_info(messages)")?
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        if !columns.iter().any(|name| name == "recipient_headers_json") {
            connection
                .execute_batch("ALTER TABLE messages ADD COLUMN recipient_headers_json TEXT")?;
        }
        let has_confirmed_at = connection
            .prepare("PRAGMA table_info(actions)")?
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<std::result::Result<Vec<_>, _>>()?
            .iter()
            .any(|name| name == "confirmed_at_ms");
        if !has_confirmed_at {
            connection.execute_batch("ALTER TABLE actions ADD COLUMN confirmed_at_ms INTEGER")?;
        }
        let has_action_thread = connection
            .prepare("PRAGMA table_info(actions)")?
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<std::result::Result<Vec<_>, _>>()?
            .iter()
            .any(|name| name == "thread_id");
        if !has_action_thread {
            connection.execute_batch("ALTER TABLE actions ADD COLUMN thread_id TEXT")?;
            let existing = {
                let mut query =
                    connection.prepare("SELECT operation_id,action_json FROM actions")?;
                query
                    .query_map([], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                    })?
                    .collect::<std::result::Result<Vec<_>, _>>()?
            };
            for (operation_id, json) in existing {
                let action: MailAction = serde_json::from_str(&json)?;
                connection.execute(
                    "UPDATE actions SET thread_id=?2 WHERE operation_id=?1",
                    params![operation_id, action_thread_id(&action)],
                )?;
            }
        }
        connection.execute_batch("CREATE INDEX IF NOT EXISTS actions_active_thread ON actions(thread_id,status,confirmed_at_ms)")?;
        Ok(())
    }
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>> {
        self.connection.lock().map_err(|_| MailError::Lock)
    }
    pub fn upsert_account(&self, a: &Account) -> Result<()> {
        self.lock()?.execute("INSERT INTO accounts(id,provider,email,display_name) VALUES(?1,?2,?3,?4) ON CONFLICT(id) DO UPDATE SET provider=excluded.provider,email=excluded.email,display_name=excluded.display_name",params![a.id,a.provider.as_str(),a.email,a.display_name])?;
        Ok(())
    }
    pub fn list_accounts(&self) -> Result<Vec<Account>> {
        let db = self.lock()?;
        let mut q =
            db.prepare("SELECT id,provider,email,display_name FROM accounts ORDER BY email")?;
        Ok(q.query_map([], |r| {
            Ok(Account {
                id: r.get(0)?,
                provider: Provider::parse(&r.get::<_, String>(1)?),
                email: r.get(2)?,
                display_name: r.get(3)?,
            })
        })?
        .collect::<std::result::Result<_, _>>()?)
    }
    pub fn remove_account(&self, account_id: &str) -> Result<()> {
        let mut db = self.lock()?;
        let tx = db.transaction()?;
        let exists = tx
            .query_row("SELECT 1 FROM accounts WHERE id=?1", [account_id], |_| {
                Ok(())
            })
            .optional()?
            .is_some();
        if !exists {
            return Err(MailError::UnknownAccount(account_id.into()));
        }
        let mut message_ids = std::collections::HashSet::new();
        let mut thread_ids = std::collections::HashSet::new();
        {
            let mut query = tx.prepare("SELECT id FROM threads WHERE account_id=?1")?;
            for id in query.query_map([account_id], |row| row.get::<_, String>(0))? {
                thread_ids.insert(id?);
            }
        }
        {
            let mut query = tx.prepare("SELECT id,rowid FROM messages WHERE account_id=?1")?;
            let rows = query.query_map([account_id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
            })?;
            for row in rows {
                let (id, rowid) = row?;
                message_ids.insert(id);
                tx.execute("DELETE FROM message_search WHERE rowid=?1", [rowid])?;
            }
        }
        let belongs_to_removed_account = |action: &MailAction| match action {
            MailAction::SendDraft { message_id } => message_ids.contains(message_id),
            action => action_thread_id(action).is_some_and(|id| thread_ids.contains(id)),
        };
        // Undo batches can span accounts. Remove only the deleted account's
        // selection and inverse actions, keeping the other account's undo valid.
        let batches = {
            let mut query =
                tx.prepare("SELECT operation_id,request_json,inverse_json FROM triage_batches")?;
            query
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        for (operation, request, inverse) in batches {
            let (kind, mut ids): (String, Vec<String>) = serde_json::from_str(&request)?;
            let mut actions: Vec<MailAction> = serde_json::from_str(&inverse)?;
            let previous = (ids.len(), actions.len());
            ids.retain(|id| !thread_ids.contains(id));
            actions.retain(|action| !belongs_to_removed_account(action));
            if previous == (ids.len(), actions.len()) {
                continue;
            }
            if ids.is_empty() {
                tx.execute(
                    "DELETE FROM triage_batches WHERE operation_id=?1",
                    [&operation],
                )?;
            } else {
                tx.execute("UPDATE triage_batches SET request_json=?2,inverse_json=?3 WHERE operation_id=?1",
                    params![operation,serde_json::to_string(&(kind,ids))?,serde_json::to_string(&actions)?])?;
            }
        }
        {
            let mut query = tx.prepare("SELECT operation_id,action_json FROM actions")?;
            let rows =
                query.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
            for row in rows {
                let (id, raw) = row?;
                let action: MailAction = serde_json::from_str(&raw)?;
                let belongs = belongs_to_removed_account(&action);
                if belongs {
                    tx.execute("DELETE FROM actions WHERE operation_id=?1", [&id])?;
                }
            }
        }
        tx.execute("DELETE FROM draft_attachments WHERE message_id IN (SELECT id FROM messages WHERE account_id=?1)", [account_id])?;
        tx.execute(
            "DELETE FROM outbox WHERE message_id IN (SELECT id FROM messages WHERE account_id=?1)",
            [account_id],
        )?;
        tx.execute(
            "DELETE FROM snoozes WHERE thread_id IN (SELECT id FROM threads WHERE account_id=?1)",
            [account_id],
        )?;
        tx.execute("DELETE FROM pinned_threads WHERE thread_id IN (SELECT id FROM threads WHERE account_id=?1)", [account_id])?;
        tx.execute("DELETE FROM local_conversation_links WHERE source_thread IN (SELECT id FROM threads WHERE account_id=?1) OR target_thread IN (SELECT id FROM threads WHERE account_id=?1)", [account_id])?;
        tx.execute("DELETE FROM messages WHERE account_id=?1", [account_id])?;
        tx.execute("DELETE FROM thread_labels WHERE thread_id IN (SELECT id FROM threads WHERE account_id=?1)", [account_id])?;
        tx.execute("DELETE FROM threads WHERE account_id=?1", [account_id])?;
        tx.execute("DELETE FROM sync_state WHERE account_id=?1", [account_id])?;
        tx.execute(
            "DELETE FROM sync_reset_messages WHERE account_id=?1",
            [account_id],
        )?;
        tx.execute(
            "DELETE FROM sync_reset_labels WHERE account_id=?1",
            [account_id],
        )?;
        tx.execute(
            "DELETE FROM sync_reset_state WHERE account_id=?1",
            [account_id],
        )?;
        agent::remove_account_rows(&tx, account_id, &thread_ids)?;
        tx.execute("DELETE FROM accounts WHERE id=?1", [account_id])?;
        tx.commit()?;
        Ok(())
    }
    pub fn upsert_message(&self, m: &Message) -> Result<()> {
        let mut db = self.lock()?;
        let tx = db.transaction()?;
        upsert_message_tx(&tx, m)?;
        tx.commit()?;
        Ok(())
    }
    pub fn list_threads(
        &self,
        account_id: Option<&str>,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<ThreadSummary>> {
        self.list_threads_filtered(account_id, false, limit, offset)
    }
    pub fn list_threads_filtered(
        &self,
        account_id: Option<&str>,
        unread_only: bool,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<ThreadSummary>> {
        let db = self.lock()?;
        let mut q=db.prepare("SELECT id,account_id,subject,sender,snippet,last_message_at,unread_count,is_inbox,folder FROM threads WHERE is_inbox=1 AND (?1 IS NULL OR account_id=?1) AND (?5=0 OR unread_count>0) AND NOT EXISTS (SELECT 1 FROM snoozes s WHERE s.thread_id=threads.id AND s.wake_at_ms>?4) ORDER BY last_message_at DESC,id LIMIT ?2 OFFSET ?3")?;
        let mut rows = q.query(params![
            account_id,
            limit.min(500),
            offset,
            now_ms(),
            unread_only
        ])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push(thread_row(&db, r)?);
        }
        Ok(out)
    }
    pub fn list_archived_threads(&self, limit: u32, offset: u32) -> Result<Vec<ThreadSummary>> {
        let db = self.lock()?;
        let mut q = db.prepare("SELECT id,account_id,subject,sender,snippet,last_message_at,unread_count,is_inbox,folder FROM threads WHERE is_inbox=0 AND lower(folder)='archive' AND NOT EXISTS (SELECT 1 FROM snoozes s WHERE s.thread_id=threads.id AND s.wake_at_ms>?3) ORDER BY last_message_at DESC,id LIMIT ?1 OFFSET ?2")?;
        let mut rows = q.query(params![limit.min(500), offset, now_ms()])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(thread_row(&db, row)?);
        }
        Ok(out)
    }
    pub fn restart_archive_pages(&self, account_id: &str) -> Result<()> {
        self.lock()?.execute(
            "DELETE FROM archive_pages WHERE account_id=?1",
            [account_id],
        )?;
        Ok(())
    }
    pub fn archive_page_cursor(&self, account_id: &str) -> Result<(Option<String>, bool)> {
        Ok(self
            .lock()?
            .query_row(
                "SELECT cursor,complete FROM archive_pages WHERE account_id=?1",
                [account_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?
            .unwrap_or((None, false)))
    }
    /// A separate archive page must not advance or reset the live inbox cursor.
    pub fn cache_archive_page(
        &self,
        account_id: &str,
        batch: &SyncBatch,
        next: Option<&str>,
    ) -> Result<()> {
        let mut db = self.lock()?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for message in &batch.messages {
            if message.account_id != account_id {
                return Err(MailError::Provider("archive page account mismatch".into()));
            }
            upsert_message_tx(&tx, message)?;
        }
        for (id, labels) in &batch.thread_labels {
            let owner: Option<String> = tx
                .query_row("SELECT account_id FROM threads WHERE id=?1", [id], |r| {
                    r.get(0)
                })
                .optional()?;
            if owner.as_deref() != Some(account_id) {
                return Err(MailError::Provider(
                    "archive labels account mismatch".into(),
                ));
            }
            tx.execute("DELETE FROM thread_labels WHERE thread_id=?1", [id])?;
            for label in labels
                .iter()
                .filter(|label| label.as_str() != "__EMAIL_ARCHIVE__")
            {
                tx.execute(
                    "INSERT OR IGNORE INTO thread_labels(thread_id,label) VALUES(?1,?2)",
                    params![id, label],
                )?;
            }
            provider_folder(&tx, account_id, id, labels)?;
            replay_actions_for_thread(&tx, id)?;
        }
        tx.execute("INSERT INTO archive_pages(account_id,cursor,complete) VALUES(?1,?2,?3) ON CONFLICT(account_id) DO UPDATE SET cursor=excluded.cursor,complete=excluded.complete",params![account_id,next,next.is_none()])?;
        tx.commit()?;
        Ok(())
    }
    pub fn list_labels(&self) -> Result<Vec<LabelSummary>> {
        let db = self.lock()?;
        let mut q = db.prepare(
            "SELECT label,count(*) FROM thread_labels GROUP BY label ORDER BY lower(label)",
        )?;
        let rows = q.query_map([], |r| {
            Ok(LabelSummary {
                name: r.get(0)?,
                count: r.get::<_, i64>(1)? as u32,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }
    pub fn list_label_threads(
        &self,
        label: &str,
        query: Option<&str>,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<ThreadSummary>> {
        let db = self.lock()?;
        let mut statement = db.prepare(
            "SELECT t.id,t.account_id,t.subject,t.sender,t.snippet,t.last_message_at,t.unread_count,t.is_inbox,t.folder \
             FROM threads t WHERE EXISTS (SELECT 1 FROM thread_labels l WHERE l.thread_id=t.id AND lower(l.label)=lower(?1)) \
             AND (?2 IS NULL OR instr(lower(t.sender),lower(?2))>0 OR instr(lower(t.subject),lower(?2))>0 OR instr(lower(t.snippet),lower(?2))>0) \
             ORDER BY t.last_message_at DESC,t.id LIMIT ?3 OFFSET ?4",
        )?;
        let query = query.filter(|value| !value.is_empty());
        let mut rows = statement.query(params![label, query, limit.min(500), offset])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(thread_row(&db, row)?);
        }
        Ok(out)
    }
    pub fn snooze_thread(&self, thread_id: &str, wake_at_ms: i64) -> Result<()> {
        if wake_at_ms <= now_ms() {
            return Err(MailError::InvalidAction(
                "snooze time must be in the future".into(),
            ));
        }
        let db = self.lock()?;
        check_thread_connection(&db, thread_id)?;
        db.execute("INSERT INTO snoozes(thread_id,wake_at_ms) VALUES(?1,?2) ON CONFLICT(thread_id) DO UPDATE SET wake_at_ms=excluded.wake_at_ms", params![thread_id,wake_at_ms])?;
        Ok(())
    }
    pub fn unsnooze_thread(&self, thread_id: &str) -> Result<()> {
        let db = self.lock()?;
        check_thread_connection(&db, thread_id)?;
        db.execute("DELETE FROM snoozes WHERE thread_id=?1", [thread_id])?;
        Ok(())
    }
    pub fn list_snoozed_threads(&self) -> Result<Vec<ThreadSummary>> {
        let db = self.lock()?;
        let mut q = db.prepare("SELECT t.id,t.account_id,t.subject,t.sender,t.snippet,t.last_message_at,t.unread_count,t.is_inbox,t.folder FROM threads t JOIN snoozes s ON s.thread_id=t.id WHERE s.wake_at_ms>?1 ORDER BY s.wake_at_ms,t.last_message_at DESC")?;
        let mut rows = q.query([now_ms()])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push(thread_row(&db, r)?);
        }
        Ok(out)
    }
    pub fn set_thread_pinned(&self, thread_id: &str, pinned: bool) -> Result<()> {
        let db = self.lock()?;
        check_thread_connection(&db, thread_id)?;
        if pinned {
            db.execute(
                "INSERT OR IGNORE INTO pinned_threads(thread_id) VALUES(?1)",
                [thread_id],
            )?;
        } else {
            db.execute("DELETE FROM pinned_threads WHERE thread_id=?1", [thread_id])?;
        }
        Ok(())
    }
    pub fn list_pinned_threads(&self, limit: u32, offset: u32) -> Result<Vec<ThreadSummary>> {
        let db = self.lock()?;
        let mut q = db.prepare("SELECT t.id,t.account_id,t.subject,t.sender,t.snippet,t.last_message_at,t.unread_count,t.is_inbox,t.folder FROM threads t JOIN pinned_threads p ON p.thread_id=t.id ORDER BY t.last_message_at DESC,t.id LIMIT ?1 OFFSET ?2")?;
        let mut rows = q.query(params![limit.min(500), offset])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push(thread_row(&db, r)?);
        }
        Ok(out)
    }
    pub fn list_splits(&self) -> Result<Vec<SplitInbox>> {
        let db = self.lock()?;
        let mut q =
            db.prepare("SELECT id,name,query FROM split_inboxes ORDER BY lower(name),id")?;
        let rows = q.query_map([], |r| {
            Ok(SplitInbox {
                id: r.get(0)?,
                name: r.get(1)?,
                query: r.get(2)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }
    pub fn upsert_split(&self, id: Option<&str>, name: &str, query: &str) -> Result<SplitInbox> {
        let name = name.trim();
        let query = query.trim();
        if name.is_empty() || name.len() > 80 || query.is_empty() || query.len() > 256 {
            return Err(MailError::InvalidAction(
                "split name and query are required and must be short".into(),
            ));
        }
        self.preview_split(query, 1)?;
        let id = id
            .map(ToString::to_string)
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let canonical = |value: &str| {
            value
                .split([',', '\n'])
                .map(|v| v.trim().to_lowercase())
                .filter(|v| !v.is_empty())
                .collect::<Vec<_>>()
                .join(",")
        };
        if let Some(other) = self
            .list_splits()?
            .iter()
            .find(|other| other.id != id && canonical(&other.query) == canonical(query))
        {
            return Err(MailError::InvalidAction(format!(
                "These rules are already used by {}",
                other.name
            )));
        }
        let db = self.lock()?;
        db.execute("INSERT INTO split_inboxes(id,name,query) VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET name=excluded.name,query=excluded.query", params![id,name,query])?;
        Ok(SplitInbox {
            id,
            name: name.into(),
            query: query.into(),
        })
    }
    pub fn delete_split(&self, id: &str) -> Result<()> {
        let db = self.lock()?;
        if db.execute("DELETE FROM split_inboxes WHERE id=?1", [id])? == 0 {
            return Err(MailError::NotFound(id.into()));
        }
        Ok(())
    }
    pub fn list_split_threads(
        &self,
        split_id: &str,
        account_id: Option<&str>,
        unread_only: bool,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<ThreadSummary>> {
        let db = self.lock()?;
        let query: String = db
            .query_row(
                "SELECT query FROM split_inboxes WHERE id=?1",
                [split_id],
                |r| r.get(0),
            )
            .optional()?
            .ok_or_else(|| MailError::NotFound(split_id.into()))?;
        split_threads(&db, &query, account_id, unread_only, limit, offset)
    }
    /// Preview uses the exact saved-view matcher and only the local cache.
    pub fn preview_split(&self, query: &str, limit: u32) -> Result<Vec<ThreadSummary>> {
        let db = self.lock()?;
        split_threads(&db, query, None, false, limit, 0)
    }
    pub fn search(&self, query: &str, limit: u32) -> Result<Vec<ThreadSummary>> {
        self.search_in_folder(query, limit, None)
    }

    /// Filter before LIMIT so archived matches cannot crowd out inbox results.
    /// None preserves the global search used by external clients.
    pub fn search_in_folder(
        &self,
        query: &str,
        limit: u32,
        is_inbox: Option<bool>,
    ) -> Result<Vec<ThreadSummary>> {
        if query.trim().is_empty() {
            return Ok(vec![]);
        }
        self.search_scoped(
            query,
            limit,
            0,
            &SearchScope {
                is_inbox,
                ..Default::default()
            },
        )
    }
    pub fn get_message(&self, id: &str) -> Result<Option<Message>> {
        let db = self.lock()?;
        let raw:Option<(String,String,String,String,String,String,String,Option<String>,Option<String>,Option<bool>,i64,bool,bool,bool,Option<String>)>=db.query_row("SELECT id,thread_id,account_id,sender,to_json,subject,body_plain,body_html,unsubscribe_header,has_attachments,received_at,is_read,is_draft,is_sent,recipient_headers_json FROM messages WHERE id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?,r.get(8)?,r.get(9)?,r.get(10)?,r.get(11)?,r.get(12)?,r.get(13)?,r.get(14)?))).optional()? ;
        raw.map(|r| {
            Ok(Message {
                recipient_headers: r.14.as_deref().map(serde_json::from_str).transpose()?,
                id: r.0,
                thread_id: r.1,
                account_id: r.2,
                sender: r.3,
                to: serde_json::from_str(&r.4)?,
                subject: r.5,
                body_plain: r.6,
                body_html: r.7,
                unsubscribe_header: r.8,
                has_attachments: r.9,
                received_at: r.10,
                is_read: r.11,
                is_draft: r.12,
                is_sent: r.13,
            })
        })
        .transpose()
    }
    /// Remove a message reported as deleted or moved out of the synced folder.
    /// A provider may replay the same tombstone; missing IDs are harmless.
    pub fn remove_message(&self, account_id: &str, id: &str) -> Result<bool> {
        let mut db = self.lock()?;
        let tx = db.transaction()?;
        let existing: Option<(i64, String)> = tx
            .query_row(
                "SELECT rowid,thread_id FROM messages WHERE id=?1 AND account_id=?2",
                params![id, account_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((rowid, thread_id)) = existing else {
            return Ok(false);
        };
        tx.execute("DELETE FROM message_search WHERE rowid=?1", [rowid])?;
        tx.execute("DELETE FROM draft_attachments WHERE message_id=?1", [id])?;
        tx.execute("DELETE FROM outbox WHERE message_id=?1", [id])?;
        tx.execute("DELETE FROM messages WHERE rowid=?1", [rowid])?;
        let remaining: i64 = tx.query_row(
            "SELECT count(*) FROM messages WHERE thread_id=?1",
            [&thread_id],
            |r| r.get(0),
        )?;
        if remaining == 0 {
            tx.execute("DELETE FROM snoozes WHERE thread_id=?1", [&thread_id])?;
            tx.execute("DELETE FROM thread_labels WHERE thread_id=?1", [&thread_id])?;
            tx.execute("DELETE FROM threads WHERE id=?1", [&thread_id])?;
        } else {
            refresh_thread(&tx, &thread_id)?;
        }
        tx.commit()?;
        Ok(true)
    }
    pub fn update_recipient_headers(
        &self,
        id: &str,
        to: &[String],
        headers: &RecipientHeaders,
    ) -> Result<()> {
        self.lock()?.execute(
            "UPDATE messages SET to_json=?2,recipient_headers_json=?3 WHERE id=?1",
            params![
                id,
                serde_json::to_string(to)?,
                serde_json::to_string(headers)?
            ],
        )?;
        Ok(())
    }
    pub fn list_thread_messages(&self, thread_id: &str) -> Result<Vec<Message>> {
        let ids = {
            let db = self.lock()?;
            let mut q =
                db.prepare("WITH canonical(id) AS (SELECT coalesce((SELECT target_thread FROM local_conversation_links WHERE source_thread=?1),?1)) SELECT id FROM messages WHERE is_draft=0 AND thread_id IN (SELECT id FROM canonical UNION SELECT source_thread FROM local_conversation_links WHERE target_thread=(SELECT id FROM canonical)) ORDER BY received_at,id")?;
            let rows = q.query_map([thread_id], |r| r.get::<_, String>(0))?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };
        ids.iter()
            .map(|id| {
                self.get_message(id)?
                    .ok_or_else(|| MailError::NotFound(id.clone()))
            })
            .collect()
    }
    pub fn get_latest_message_for_thread(&self, thread_id: &str) -> Result<Option<Message>> {
        let id: Option<String> = {
            let db = self.lock()?;
            db.query_row("SELECT id FROM messages WHERE thread_id=?1 ORDER BY received_at DESC,id DESC LIMIT 1",[thread_id],|r|r.get(0)).optional()?
        };
        id.map(|id| self.get_message(&id))
            .transpose()
            .map(|item| item.flatten())
    }
    pub fn sync_cursor(&self, account_id: &str) -> Result<Option<SyncCursor>> {
        let db = self.lock()?;
        Ok(db
            .query_row(
                "SELECT account_id,value,updated_at FROM sync_state WHERE account_id=?1",
                [account_id],
                |r| {
                    Ok(SyncCursor {
                        account_id: r.get(0)?,
                        value: r.get(1)?,
                        updated_at: r.get(2)?,
                    })
                },
            )
            .optional()?)
    }
    fn reset_page_cursor(&self, account_id: &str) -> Result<Option<String>> {
        let db = self.lock()?;
        Ok(db
            .query_row(
                "SELECT page_cursor FROM sync_reset_state WHERE account_id=?1",
                [account_id],
                |r| r.get(0),
            )
            .optional()?)
    }
    /// Stage one provider page on disk. The visible mailbox and live cursor
    /// change together only after the final page is safely available.
    fn stage_reset_page(
        &self,
        account_id: &str,
        batch: &SyncBatch,
        phase: ResetPhase,
    ) -> Result<()> {
        let mut db = self.lock()?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let has_stage: bool = tx
            .query_row(
                "SELECT 1 FROM sync_reset_state WHERE account_id=?1",
                [account_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if matches!(phase, ResetPhase::Start | ResetPhase::StartComplete)
            || (phase == ResetPhase::Complete && !has_stage)
        {
            tx.execute(
                "DELETE FROM sync_reset_messages WHERE account_id=?1",
                [account_id],
            )?;
            tx.execute(
                "DELETE FROM sync_reset_labels WHERE account_id=?1",
                [account_id],
            )?;
        } else if !has_stage {
            return Err(MailError::Provider(
                "reset continuation has no staged first page".into(),
            ));
        }
        for message in &batch.messages {
            if message.account_id != account_id || message.id.starts_with("local:") {
                return Err(MailError::Provider(
                    "reset page contains an invalid account or local message ID".into(),
                ));
            }
            tx.execute("INSERT INTO sync_reset_messages(account_id,message_id,message_json) VALUES(?1,?2,?3) ON CONFLICT(account_id,message_id) DO UPDATE SET message_json=excluded.message_json",
                params![account_id,message.id,serde_json::to_string(message)?])?;
        }
        for id in &batch.removed_message_ids {
            tx.execute(
                "DELETE FROM sync_reset_messages WHERE account_id=?1 AND message_id=?2",
                params![account_id, id],
            )?;
        }
        for (thread_id, labels) in &batch.thread_labels {
            tx.execute("INSERT INTO sync_reset_labels(account_id,thread_id,labels_json) VALUES(?1,?2,?3) ON CONFLICT(account_id,thread_id) DO UPDATE SET labels_json=excluded.labels_json",
                params![account_id,thread_id,serde_json::to_string(labels)?])?;
        }
        if !matches!(phase, ResetPhase::Complete | ResetPhase::StartComplete) {
            tx.execute("INSERT INTO sync_reset_state(account_id,page_cursor) VALUES(?1,?2) ON CONFLICT(account_id) DO UPDATE SET page_cursor=excluded.page_cursor", params![account_id,batch.next_cursor])?;
            tx.commit()?;
            return Ok(());
        }
        // Drop only remotely owned messages. Local drafts and sent messages
        // remain available while the provider snapshot is replaced.
        {
            let mut q = tx.prepare(
                "SELECT rowid FROM messages WHERE account_id=?1 AND id NOT LIKE 'local:%' AND thread_id IN (SELECT id FROM threads WHERE is_inbox=1)",
            )?;
            let rows = q.query_map([account_id], |r| r.get::<_, i64>(0))?;
            for row in rows {
                tx.execute("DELETE FROM message_search WHERE rowid=?1", [row?])?;
            }
        }
        tx.execute(
            "DELETE FROM messages WHERE account_id=?1 AND id NOT LIKE 'local:%' AND thread_id IN (SELECT id FROM threads WHERE is_inbox=1)",
            [account_id],
        )?;
        {
            let mut q = tx.prepare("SELECT message_json FROM sync_reset_messages WHERE account_id=?1 ORDER BY message_id")?;
            let rows = q.query_map([account_id], |r| r.get::<_, String>(0))?;
            for row in rows {
                let message: Message = serde_json::from_str(&row?)?;
                upsert_message_tx(&tx, &message)?;
            }
        }
        let orphan_threads = {
            let mut q = tx.prepare("SELECT id FROM threads WHERE account_id=?1 AND NOT EXISTS (SELECT 1 FROM messages WHERE thread_id=threads.id)")?;
            q.query_map([account_id], |r| r.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        for id in orphan_threads {
            tx.execute("DELETE FROM snoozes WHERE thread_id=?1", [&id])?;
            tx.execute("DELETE FROM thread_labels WHERE thread_id=?1", [&id])?;
            tx.execute("DELETE FROM threads WHERE id=?1", [&id])?;
        }
        {
            let mut q = tx.prepare(
                "SELECT thread_id,labels_json FROM sync_reset_labels WHERE account_id=?1",
            )?;
            let rows = q.query_map([account_id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (thread_id, json) = row?;
                let owner: Option<String> = tx
                    .query_row(
                        "SELECT account_id FROM threads WHERE id=?1",
                        [&thread_id],
                        |r| r.get(0),
                    )
                    .optional()?;
                if owner.is_none() {
                    continue; // A later page removed the thread's last message.
                }
                if owner.as_deref() != Some(account_id) {
                    return Err(MailError::Provider(
                        "reset labels reference another account".into(),
                    ));
                }
                tx.execute("DELETE FROM thread_labels WHERE thread_id=?1", [&thread_id])?;
                let labels = serde_json::from_str::<Vec<String>>(&json)?;
                provider_folder(&tx, account_id, &thread_id, &labels)?;
                for label in labels
                    .iter()
                    .filter(|label| label.as_str() != "__EMAIL_ARCHIVE__")
                {
                    tx.execute(
                        "INSERT OR IGNORE INTO thread_labels(thread_id,label) VALUES(?1,?2)",
                        params![thread_id, label],
                    )?;
                }
            }
        }
        replay_actions_for_account(&tx, account_id)?;
        tx.execute("INSERT INTO sync_state(account_id,value,updated_at) VALUES(?1,?2,?3) ON CONFLICT(account_id) DO UPDATE SET value=excluded.value,updated_at=excluded.updated_at", params![account_id,batch.next_cursor,now_ms()])?;
        tx.execute(
            "DELETE FROM sync_reset_messages WHERE account_id=?1",
            [account_id],
        )?;
        tx.execute(
            "DELETE FROM sync_reset_labels WHERE account_id=?1",
            [account_id],
        )?;
        tx.execute(
            "DELETE FROM sync_reset_state WHERE account_id=?1",
            [account_id],
        )?;
        tx.commit()?;
        Ok(())
    }
    /// Re-running a partially applied batch is safe: the cursor advances only
    /// after every message has been cached successfully.
    pub fn run_sync(&self, adapter: &dyn MailAdapter, account_id: &str) -> Result<usize> {
        let account = self
            .list_accounts()?
            .into_iter()
            .find(|a| a.id == account_id)
            .ok_or_else(|| MailError::UnknownAccount(account_id.into()))?;
        if account.provider != adapter.provider() {
            return Err(MailError::Provider(
                "adapter does not match account provider".into(),
            ));
        }
        let mut count = 0;
        // Limit one invocation's work; an incomplete reset remains staged and
        // the next invocation resumes from its persisted page cursor.
        for _ in 0..128 {
            let reset_cursor = self.reset_page_cursor(account_id)?;
            let live_cursor = self.sync_cursor(account_id)?;
            let cursor = reset_cursor
                .as_deref()
                .or_else(|| live_cursor.as_ref().map(|c| c.value.as_str()));
            let cached: std::collections::HashMap<String, String> = {
                let db = self.lock()?;
                let mut q = db.prepare("SELECT id,thread_id FROM messages WHERE account_id=?1")?;
                let rows = q.query_map([account_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
                rows.collect::<std::result::Result<_, _>>()?
            };
            let batch = adapter.sync_with_cached_messages(&account, cursor, &cached)?;
            count += batch.messages.len();
            if let Some(phase) = batch
                .reset_phase
                .clone()
                .or_else(|| batch.full_reset.then_some(ResetPhase::Complete))
            {
                self.stage_reset_page(account_id, &batch, phase.clone())?;
                if matches!(phase, ResetPhase::Complete | ResetPhase::StartComplete) {
                    return Ok(count);
                }
                continue;
            }
            if reset_cursor.is_some() {
                return Err(MailError::Provider(
                    "provider ended reset before completing the snapshot".into(),
                ));
            }
            for message in &batch.messages {
                if message.account_id != account_id {
                    return Err(MailError::Provider(
                        "sync batch contains another account's message".into(),
                    ));
                }
                self.upsert_message(message)?;
            }
            // Minimal provider snapshots update flags only. Existing body, recipients,
            // inline attachments and MIME metadata remain intact and readable.
            if !batch.message_states.is_empty() {
                let mut db = self.lock()?;
                let tx = db.transaction()?;
                let mut touched = std::collections::BTreeSet::new();
                for state in &batch.message_states {
                    let owner: Option<(String, String)> = tx
                        .query_row(
                            "SELECT account_id,thread_id FROM messages WHERE id=?1",
                            [&state.id],
                            |r| Ok((r.get(0)?, r.get(1)?)),
                        )
                        .optional()?;
                    let Some((owner, thread)) = owner else {
                        continue;
                    };
                    if owner != account_id {
                        return Err(MailError::Provider(
                            "metadata references another account".into(),
                        ));
                    }
                    tx.execute(
                        "UPDATE messages SET is_read=?2,is_draft=?3,is_sent=?4 WHERE id=?1",
                        params![state.id, state.is_read, state.is_draft, state.is_sent],
                    )?;
                    touched.insert(thread);
                }
                for thread in touched {
                    refresh_thread(&tx, &thread)?;
                    replay_actions_for_thread(&tx, &thread)?;
                }
                tx.commit()?;
            }
            for (thread_id, labels) in &batch.thread_labels {
                let mut db = self.lock()?;
                let tx = db.transaction()?;
                let owner: Option<String> = tx
                    .query_row(
                        "SELECT account_id FROM threads WHERE id=?1",
                        [thread_id],
                        |r| r.get(0),
                    )
                    .optional()?;
                if owner.as_deref() != Some(account_id) {
                    return Err(MailError::Provider(
                        "sync labels reference another account or missing thread".into(),
                    ));
                }
                tx.execute("DELETE FROM thread_labels WHERE thread_id=?1", [thread_id])?;
                for label in labels
                    .iter()
                    .filter(|label| label.as_str() != "__EMAIL_ARCHIVE__")
                {
                    tx.execute(
                        "INSERT OR IGNORE INTO thread_labels(thread_id,label) VALUES(?1,?2)",
                        params![thread_id, label],
                    )?;
                }
                provider_folder(&tx, account_id, thread_id, labels)?;
                replay_actions_for_thread(&tx, thread_id)?;
                tx.commit()?;
            }
            for id in &batch.removed_message_ids {
                self.remove_message(account_id, id)?;
            }
            self.lock()?.execute("INSERT INTO sync_state(account_id,value,updated_at) VALUES(?1,?2,?3) ON CONFLICT(account_id) DO UPDATE SET value=excluded.value,updated_at=excluded.updated_at",params![account_id,batch.next_cursor,now_ms()])?;
            return Ok(count);
        }
        Ok(count)
    }
    pub fn compose_draft(
        &self,
        account_id: &str,
        to: &str,
        subject: &str,
        body_plain: &str,
    ) -> Result<Message> {
        self.compose_draft_from(account_id, None, to, subject, body_plain)
    }
    pub fn compose_draft_from(
        &self,
        account_id: &str,
        from: Option<&str>,
        to: &str,
        subject: &str,
        body_plain: &str,
    ) -> Result<Message> {
        self.compose_draft_with_recipients(account_id, from, to, None, None, subject, body_plain)
    }
    pub fn compose_draft_with_recipients(
        &self,
        account_id: &str,
        from: Option<&str>,
        to: &str,
        cc: Option<&str>,
        bcc: Option<&str>,
        subject: &str,
        body_plain: &str,
    ) -> Result<Message> {
        let account = self
            .list_accounts()?
            .into_iter()
            .find(|a| a.id == account_id)
            .ok_or_else(|| MailError::UnknownAccount(account_id.into()))?;
        let id = format!("local:{}", Uuid::new_v4());
        let m = Message {
            recipient_headers: if cc.is_some() || bcc.is_some() {
                Some(RecipientHeaders {
                    cc: draft_recipients(cc.unwrap_or("")),
                    bcc: draft_recipients(bcc.unwrap_or("")),
                    reply_to: Vec::new(),
                })
            } else {
                None
            },
            id: id.clone(),
            thread_id: id,
            account_id: account_id.into(),
            sender: from.unwrap_or(&account.email).into(),
            to: draft_recipients(to),
            subject: subject.into(),
            body_plain: body_plain.into(),
            body_html: None,
            unsubscribe_header: None,
            has_attachments: Some(false),
            received_at: now_ms(),
            is_read: true,
            is_draft: true,
            is_sent: false,
        };
        self.upsert_message(&m)?;
        self.lock()?.execute(
            "UPDATE threads SET is_inbox=0,folder='Drafts' WHERE id=?1",
            [&m.thread_id],
        )?;
        Ok(m)
    }
    /// Attach an explicit reply to its conversation; never merge by subject.
    pub fn attach_draft_to_thread(&self, message_id: &str, thread_id: &str) -> Result<Message> {
        let mut db = self.lock()?;
        let tx = db.transaction()?;
        let canonical: String = tx.query_row("SELECT coalesce((SELECT target_thread FROM local_conversation_links WHERE source_thread=?1),?1)", [thread_id], |r| r.get(0))?;
        let thread_id = canonical.as_str();
        let source: (String, String) = tx.query_row(
            "SELECT thread_id,account_id FROM messages WHERE id=?1 AND is_draft=1",
            [message_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
        let target: Option<String> = tx.query_row("SELECT account_id FROM threads WHERE id=?1", [thread_id], |r| r.get(0)).optional()?;
        if target.as_deref() != Some(source.1.as_str()) {
            return Err(MailError::InvalidAction("reply conversation belongs to another account or is missing".into()));
        }
        tx.execute("UPDATE messages SET thread_id=?2 WHERE id=?1", params![message_id,thread_id])?;
        if source.0 != thread_id {
            tx.execute("DELETE FROM threads WHERE id=?1 AND NOT EXISTS(SELECT 1 FROM messages WHERE thread_id=?1)", [&source.0])?;
        }
        tx.commit()?;
        drop(db);
        self.get_message(message_id)?.ok_or_else(|| MailError::NotFound(message_id.into()))
    }

    pub fn list_drafts(&self, account_id: Option<&str>) -> Result<Vec<Message>> {
        let ids = {
            let db = self.lock()?;
            let mut query = db.prepare("SELECT id FROM messages WHERE is_draft=1 AND (?1 IS NULL OR account_id=?1) AND NOT EXISTS (SELECT 1 FROM outbox o WHERE o.message_id=messages.id) ORDER BY received_at DESC")?;
            query
                .query_map([account_id], |row| row.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        ids.iter()
            .map(|id| {
                self.get_message(id)?
                    .ok_or_else(|| MailError::NotFound(id.clone()))
            })
            .collect()
    }
    pub fn update_draft(
        &self,
        id: &str,
        to: &str,
        subject: &str,
        body_plain: &str,
    ) -> Result<Message> {
        self.update_draft_from(id, None, to, subject, body_plain)
    }
    pub fn update_draft_from(
        &self,
        id: &str,
        from: Option<&str>,
        to: &str,
        subject: &str,
        body_plain: &str,
    ) -> Result<Message> {
        self.update_draft_with_recipients(id, from, to, None, None, subject, body_plain)
    }
    pub fn update_draft_with_recipients(
        &self,
        id: &str,
        from: Option<&str>,
        to: &str,
        cc: Option<&str>,
        bcc: Option<&str>,
        subject: &str,
        body_plain: &str,
    ) -> Result<Message> {
        let mut db = self.lock()?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let recipient_headers = if cc.is_some() || bcc.is_some() {
            let current: Option<String> = tx
                .query_row(
                    "SELECT recipient_headers_json FROM messages WHERE id=?1",
                    [id],
                    |r| r.get(0),
                )
                .optional()?
                .flatten();
            let mut headers: RecipientHeaders = current
                .as_deref()
                .map(serde_json::from_str)
                .transpose()?
                .unwrap_or_default();
            if let Some(cc) = cc {
                headers.cc = draft_recipients(cc);
            }
            if let Some(bcc) = bcc {
                headers.bcc = draft_recipients(bcc);
            }
            Some(serde_json::to_string(&headers)?)
        } else {
            None
        };
        let changed = tx.execute(
            "UPDATE messages SET to_json=?2,subject=?3,body_plain=?4,body_html=NULL,received_at=?5,sender=coalesce(?6,sender),recipient_headers_json=coalesce(?7,recipient_headers_json) WHERE id=?1 AND is_draft=1 AND NOT EXISTS (SELECT 1 FROM outbox WHERE message_id=?1)",
            params![id, serde_json::to_string(&draft_recipients(to))?, subject, body_plain, now_ms(), from,recipient_headers],
        )?;
        if changed == 0 {
            return Err(MailError::InvalidAction(
                "draft is missing, sent, or already queued for sending".into(),
            ));
        }
        let (rowid, thread_id, sender): (i64, String, String) = tx.query_row(
            "SELECT rowid,thread_id,sender FROM messages WHERE id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        tx.execute("DELETE FROM message_search WHERE rowid=?1", [rowid])?;
        tx.execute(
            "INSERT INTO message_search(rowid,thread_id,subject,sender,body_plain) VALUES(?1,?2,?3,?4,?5)",
            params![rowid, thread_id, subject, sender, body_plain],
        )?;
        refresh_thread(&tx, &thread_id)?;
        tx.commit()?;
        drop(db);
        self.get_message(id)?
            .ok_or_else(|| MailError::NotFound(id.into()))
    }
    pub fn add_draft_attachment(
        &self,
        message_id: &str,
        filename: &str,
        mime_type: &str,
        bytes: &[u8],
    ) -> Result<DraftAttachmentMeta> {
        if bytes.is_empty() || bytes.len() > 20 * 1024 * 1024 {
            return Err(MailError::InvalidAction(
                "attachment must be between 1 byte and 20 MiB".into(),
            ));
        }
        let filename = filename.trim();
        if filename.is_empty()
            || filename.len() > 255
            || filename.contains(['/', '\\', '\r', '\n', '\0'])
        {
            return Err(MailError::InvalidAction(
                "invalid attachment filename".into(),
            ));
        }
        let mime_type = mime_type.trim();
        if mime_type.is_empty()
            || mime_type.len() > 127
            || !mime_type.is_ascii()
            || mime_type.contains(['\r', '\n', '\0'])
        {
            return Err(MailError::InvalidAction(
                "invalid attachment MIME type".into(),
            ));
        }
        let mut db = self.lock()?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let is_draft: Option<bool> = tx
            .query_row(
                "SELECT is_draft FROM messages WHERE id=?1",
                [message_id],
                |r| r.get(0),
            )
            .optional()?;
        if is_draft != Some(true) {
            return Err(MailError::InvalidAction(
                "attachments require an unsent draft".into(),
            ));
        }
        let scheduled: i64 = tx.query_row(
            "SELECT count(*) FROM outbox WHERE message_id=?1",
            [message_id],
            |r| r.get(0),
        )?;
        if scheduled > 0 {
            return Err(MailError::InvalidAction(
                "cancel scheduled sending before editing attachments".into(),
            ));
        }
        let (count, total): (i64, i64) = tx.query_row(
            "SELECT count(*),coalesce(sum(size),0) FROM draft_attachments WHERE message_id=?1",
            [message_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if count >= 10 || total + bytes.len() as i64 > 25 * 1024 * 1024 {
            return Err(MailError::InvalidAction(
                "draft attachments exceed the 10-file or 25 MiB limit".into(),
            ));
        }
        let item = DraftAttachmentMeta {
            id: Uuid::new_v4().to_string(),
            message_id: message_id.into(),
            filename: filename.into(),
            mime_type: mime_type.into(),
            size: bytes.len() as u64,
        };
        tx.execute("INSERT INTO draft_attachments(id,message_id,filename,mime_type,bytes,size) VALUES(?1,?2,?3,?4,?5,?6)", params![item.id,item.message_id,item.filename,item.mime_type,bytes,item.size as i64])?;
        tx.commit()?;
        Ok(item)
    }
    pub fn list_draft_attachments(&self, message_id: &str) -> Result<Vec<DraftAttachmentMeta>> {
        let db = self.lock()?;
        let mut q = db.prepare("SELECT id,message_id,filename,mime_type,size FROM draft_attachments WHERE message_id=?1 ORDER BY rowid")?;
        let rows = q.query_map([message_id], |r| {
            Ok(DraftAttachmentMeta {
                id: r.get(0)?,
                message_id: r.get(1)?,
                filename: r.get(2)?,
                mime_type: r.get(3)?,
                size: r.get::<_, i64>(4)? as u64,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }
    pub fn outgoing_attachments(&self, message_id: &str) -> Result<Vec<OutgoingAttachment>> {
        let db = self.lock()?;
        let mut q = db.prepare("SELECT filename,mime_type,bytes FROM draft_attachments WHERE message_id=?1 ORDER BY rowid")?;
        let rows = q.query_map([message_id], |r| {
            Ok(OutgoingAttachment {
                filename: r.get(0)?,
                mime_type: r.get(1)?,
                bytes: r.get(2)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }
    pub fn remove_draft_attachment(&self, message_id: &str, attachment_id: &str) -> Result<()> {
        let mut db = self.lock()?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let scheduled: i64 = tx.query_row(
            "SELECT count(*) FROM outbox WHERE message_id=?1",
            [message_id],
            |r| r.get(0),
        )?;
        if scheduled > 0 {
            return Err(MailError::InvalidAction(
                "cancel scheduled sending before editing attachments".into(),
            ));
        }
        if tx.execute(
            "DELETE FROM draft_attachments WHERE id=?1 AND message_id=?2",
            params![attachment_id, message_id],
        )? == 0
        {
            return Err(MailError::NotFound(attachment_id.into()));
        }
        tx.commit()?;
        Ok(())
    }
    pub fn schedule_draft(
        &self,
        message_id: &str,
        send_at_ms: i64,
        status: &str,
    ) -> Result<OutboxItem> {
        let mut db = self.lock()?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        schedule_draft_tx(&tx, message_id, send_at_ms, status)?;
        tx.commit()?;
        drop(db);
        self.list_outbox()?
            .into_iter()
            .find(|i| i.message_id == message_id)
            .ok_or_else(|| MailError::NotFound(message_id.into()))
    }
    pub fn cancel_scheduled_draft(&self, message_id: &str) -> Result<()> {
        let db = self.lock()?;
        if db.execute("DELETE FROM outbox WHERE message_id=?1 AND status IN ('scheduled','undoable','failed')", [message_id])? == 0 {
            return Err(MailError::InvalidAction("send can no longer be undone".into()));
        }
        Ok(())
    }
    pub fn list_outbox(&self) -> Result<Vec<OutboxItem>> {
        let db = self.lock()?;
        let mut q = db.prepare("SELECT o.message_id,m.account_id,m.to_json,m.subject,substr(m.body_plain,1,180),o.send_at_ms,o.status,o.last_error FROM outbox o JOIN messages m ON m.id=o.message_id ORDER BY o.send_at_ms,o.message_id")?;
        let rows = q.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, i64>(5)?,
                r.get::<_, String>(6)?,
                r.get::<_, Option<String>>(7)?,
            ))
        })?;
        rows.map(|row| {
            let (
                message_id,
                account_id,
                to_json,
                subject,
                body_preview,
                send_at_ms,
                status,
                last_error,
            ) = row?;
            Ok(OutboxItem {
                message_id,
                account_id,
                to: serde_json::from_str(&to_json)?,
                subject,
                body_preview,
                send_at_ms,
                status,
                last_error,
            })
        })
        .collect()
    }
    pub fn due_outbox(&self, limit: u32) -> Result<Vec<String>> {
        let db = self.lock()?;
        let mut q = db.prepare("SELECT message_id FROM outbox WHERE status IN ('scheduled','undoable') AND send_at_ms<=?1 ORDER BY send_at_ms,message_id LIMIT ?2")?;
        let rows = q.query_map(params![now_ms(), limit.min(100)], |r| r.get(0))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }
    pub fn claim_due_outbox(&self, message_id: &str) -> Result<bool> {
        let mut db = self.lock()?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = tx.execute("UPDATE outbox SET status='sending',updated_at_ms=?2 WHERE message_id=?1 AND status IN ('scheduled','undoable') AND send_at_ms<=?2", params![message_id,now_ms()])?;
        tx.commit()?;
        Ok(changed == 1)
    }
    /// Atomically reserves an unscheduled draft for immediate delivery. A
    /// second process cannot reserve the same message while this claim exists.
    pub fn claim_immediate_send(&self, message_id: &str) -> Result<bool> {
        let mut db = self.lock()?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some((to, headers)) = tx
            .query_row(
                "SELECT to_json,recipient_headers_json FROM messages WHERE id=?1 AND is_draft=1",
                [message_id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)),
            )
            .optional()?
        {
            let to: Vec<String> = serde_json::from_str(&to)?;
            let headers: Option<RecipientHeaders> =
                headers.as_deref().map(serde_json::from_str).transpose()?;
            normalize_outgoing_recipients(&to, headers.as_ref())?;
        }
        let timestamp = now_ms();
        let changed = tx.execute(
            "INSERT OR IGNORE INTO outbox(message_id,send_at_ms,status,last_error,updated_at_ms) SELECT id,?2,'sending',NULL,?2 FROM messages WHERE id=?1 AND is_draft=1",
            params![message_id, timestamp],
        )?;
        tx.commit()?;
        Ok(changed == 1)
    }
    pub fn fail_outbox(&self, message_id: &str, error: &str) -> Result<()> {
        let db = self.lock()?;
        db.execute("UPDATE outbox SET status='failed',last_error=?2,updated_at_ms=?3 WHERE message_id=?1 AND status='sending'", params![message_id,error,now_ms()])?;
        Ok(())
    }
    pub fn outbox_status(&self, message_id: &str) -> Result<Option<String>> {
        let db = self.lock()?;
        Ok(db
            .query_row(
                "SELECT status FROM outbox WHERE message_id=?1",
                [message_id],
                |r| r.get(0),
            )
            .optional()?)
    }
    /// Record provider acceptance only after a successful remote send.
    pub fn mark_draft_sent(&self, id: &str) -> Result<()> {
        self.mark_draft_sent_with(id, false).map(|_| ())
    }
    /// Like `mark_draft_sent`, for a build without open tracking that sent the
    /// draft without a pixel: its tracking is turned off in the same transaction.
    /// Returns whether the draft had tracking on.
    pub fn mark_draft_sent_untracked(&self, id: &str) -> Result<bool> {
        self.mark_draft_sent_with(id, true)
    }
    fn mark_draft_sent_with(&self, id: &str, untrack: bool) -> Result<bool> {
        let mut db = self.lock()?;
        let tx = db.transaction()?;
        let thread_id: String = tx
            .query_row(
                "SELECT thread_id FROM messages WHERE id=?1 AND is_draft=1",
                [id],
                |r| r.get(0),
            )
            .optional()?
            .ok_or_else(|| MailError::InvalidAction("message is not an unsent draft".into()))?;
        tx.execute("UPDATE messages SET is_draft=0,is_sent=1 WHERE id=?1", [id])?;
        let mut dropped = false;
        if untrack {
            tracking::schema(&tx)?;
            dropped = tx.execute("UPDATE email_tracking SET enabled=0 WHERE message_id=?1 AND enabled=1", [id])? > 0;
        }
        tracking::record_sent(&tx, id, now_ms())?;
        tx.execute("DELETE FROM outbox WHERE message_id=?1", [id])?;
        tx.execute("DELETE FROM draft_attachments WHERE message_id=?1", [id])?;
        tx.execute(
            "UPDATE threads SET is_inbox=0,folder='Sent' WHERE id=?1 AND NOT EXISTS(SELECT 1 FROM messages WHERE thread_id=?1 AND is_draft=0 AND is_sent=0)",
            [&thread_id],
        )?;
        refresh_thread(&tx, &thread_id)?;
        tx.commit()?;
        Ok(dropped)
    }
    /// Use the provider's identity so the next sync updates, rather than duplicates, sent mail.
    pub fn adopt_sent_provider_id(&self, local_id: &str, provider_id: &str) -> Result<()> {
        let db = self.lock()?;
        db.execute("UPDATE messages SET id=?2 WHERE id=?1 AND is_sent=1 AND is_draft=0 AND NOT EXISTS(SELECT 1 FROM messages WHERE id=?2)", params![local_id,provider_id])?;
        Ok(())
    }

    /// Optimistic mutation is queued for provider reconciliation. SendDraft does not send mail.
    pub fn apply_action(&self, operation_id: &str, action: MailAction) -> Result<ActionResult> {
        let mut db = self.lock()?;
        let tx = db.transaction()?;
        if let Some(previous) = tx
            .query_row(
                "SELECT action_json FROM actions WHERE operation_id=?1",
                [operation_id],
                |r| r.get::<_, String>(0),
            )
            .optional()?
        {
            if serde_json::from_str::<MailAction>(&previous)? != action {
                return Err(MailError::InvalidAction(
                    "operation ID reused for a different action".into(),
                ));
            }
            return Ok(ActionResult {
                operation_id: operation_id.into(),
                applied: false,
            });
        }
        match &action {
            MailAction::Archive { thread_id } => {
                check_thread(&tx, thread_id)?;
                tx.execute(
                    "UPDATE threads SET is_inbox=0,folder='Archive' WHERE id=?1",
                    [thread_id],
                )?;
            }
            MailAction::MarkRead { thread_id, read } => {
                check_thread(&tx, thread_id)?;
                tx.execute(
                    "UPDATE messages SET is_read=?2 WHERE thread_id=?1",
                    params![thread_id, read],
                )?;
                refresh_thread(&tx, thread_id)?;
            }
            MailAction::Move { thread_id, folder } => {
                check_thread(&tx, thread_id)?;
                if folder.trim().is_empty() {
                    return Err(MailError::InvalidAction("folder is empty".into()));
                }
                tx.execute(
                    "UPDATE threads SET folder=?2,is_inbox=?3 WHERE id=?1",
                    params![thread_id, folder, folder.eq_ignore_ascii_case("inbox")],
                )?;
            }
            MailAction::AddLabel { thread_id, label } => {
                check_thread(&tx, thread_id)?;
                if label.trim().is_empty()
                    || label.len() > 100
                    || label.chars().any(char::is_control)
                {
                    return Err(MailError::InvalidAction(
                        "label must be 1–100 printable characters".into(),
                    ));
                }
                tx.execute(
                    "INSERT OR IGNORE INTO thread_labels(thread_id,label) VALUES(?1,?2)",
                    params![thread_id, label],
                )?;
            }
            MailAction::RemoveLabel { thread_id, label } => {
                check_thread(&tx, thread_id)?;
                tx.execute(
                    "DELETE FROM thread_labels WHERE thread_id=?1 AND label=?2",
                    params![thread_id, label],
                )?;
            }
            MailAction::SendDraft { message_id } => {
                let draft: Option<bool> = tx
                    .query_row(
                        "SELECT is_draft FROM messages WHERE id=?1",
                        [message_id],
                        |r| r.get(0),
                    )
                    .optional()?;
                if draft != Some(true) {
                    return Err(MailError::InvalidAction(format!(
                        "not a draft: {message_id}"
                    )));
                }
            }
        }
        tx.execute(
            "INSERT INTO actions(operation_id,action_json,status,thread_id) VALUES(?1,?2,'pending',?3)",
            params![operation_id, serde_json::to_string(&action)?, action_thread_id(&action)],
        )?;
        tx.commit()?;
        Ok(ActionResult {
            operation_id: operation_id.into(),
            applied: true,
        })
    }
    /// Counts conversations across the downloaded inbox, independent of UI pagination.
    pub fn unread_counts(&self) -> Result<serde_json::Value> {
        // Counting must not advance bounded reminder reconciliation during a UI refresh.
        let db = self.lock()?;
        let mut q = db.prepare("SELECT account_id,COUNT(*) FROM threads WHERE is_inbox=1 AND unread_count>0 AND NOT EXISTS (SELECT 1 FROM snoozes s WHERE s.thread_id=threads.id AND s.wake_at_ms>?1) GROUP BY account_id")?;
        let rows = q.query_map([now_ms()], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, u64>(1)?))
        })?;
        let mut accounts = serde_json::Map::new();
        let mut total = 0u64;
        for row in rows {
            let (id, count) = row?;
            total += count;
            accounts.insert(id, count.into());
        }
        let (active_followups, due_followups): (u64, u64) = db.query_row(
            "SELECT COUNT(*),COALESCE(SUM(CASE WHEN due_at_ms<=?1 THEN 1 ELSE 0 END),0) FROM followups WHERE status IN ('waiting','due')",
            [now_ms()], |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        Ok(serde_json::json!({"total":total,"accounts":accounts,"scope":"downloaded_inbox_conversations",
            "followups":{"active":active_followups,"due":due_followups}}))
    }
    /// Exact mailbox comparison: display names and substring matches cannot broaden cleanup.
    pub fn unread_from_sender(&self, account_id: &str, sender: &str) -> Result<Vec<String>> {
        fn address(value: &str) -> String {
            value
                .rsplit_once('<')
                .and_then(|(_, v)| v.split_once('>').map(|(a, _)| a))
                .unwrap_or(value)
                .trim()
                .to_lowercase()
        }
        let target = address(sender);
        if !target.contains('@') {
            return Err(MailError::InvalidAction(
                "Choose a sender email address".into(),
            ));
        }
        let db = self.lock()?;
        let mut q=db.prepare("SELECT id,sender FROM threads WHERE account_id=?1 AND unread_count>0 AND lower(folder) NOT IN ('spam','junk','trash','drafts')")?;
        let rows = q.query_map([account_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        let mut ids = Vec::new();
        for row in rows {
            let (id, sender) = row?;
            if address(&sender) == target {
                ids.push(id);
            }
        }
        Ok(ids)
    }
    /// Snapshot IDs are validated before any write; queue and optimistic state commit together.
    pub fn bulk_mark_read(
        &self,
        operation_id: &str,
        account_id: &str,
        thread_ids: &[String],
    ) -> Result<usize> {
        if operation_id.is_empty() || thread_ids.len() > 20000 {
            return Err(MailError::InvalidAction("Invalid bulk operation".into()));
        }
        let mut db = self.lock()?;
        let tx = db.transaction()?;
        let ids: std::collections::HashSet<&String> = thread_ids.iter().collect();
        for id in &ids {
            let owner: String =
                tx.query_row("SELECT account_id FROM threads WHERE id=?1", [*id], |r| {
                    r.get(0)
                })?;
            if owner != account_id {
                return Err(MailError::InvalidAction(
                    "Bulk selection crosses accounts".into(),
                ));
            }
        }
        let mut changed = 0;
        for id in ids {
            let op = format!("{operation_id}:{id}");
            let action = MailAction::MarkRead {
                thread_id: id.clone(),
                read: true,
            };
            let inserted=tx.execute("INSERT OR IGNORE INTO actions(operation_id,action_json,status,thread_id) VALUES(?1,?2,'pending',?3)",params![op,serde_json::to_string(&action)?,id])?;
            if inserted == 0 {
                continue;
            }
            tx.execute("UPDATE messages SET is_read=1 WHERE thread_id=?1", [id])?;
            refresh_thread(&tx, id)?;
            changed += 1;
        }
        tx.commit()?;
        Ok(changed)
    }
    /// Commit a reviewed selection and its provider outbox together. IDs are
    /// frozen by the caller, never re-evaluated against a changing search.
    pub fn batch_triage(
        &self,
        operation_id: &str,
        kind: &str,
        thread_ids: &[String],
    ) -> Result<serde_json::Value> {
        let mut db = self.lock()?;
        let tx = db.transaction()?;
        let result = batch_triage_tx(&tx, operation_id, kind, thread_ids)?;
        tx.commit()?;
        Ok(result)
    }

    /// Most recent still-valid placement undo, including across process restarts.
    /// A later unrelated batch may be undone first; never offer an inverse that
    /// would overwrite a newer action on one of its own conversations.
    pub fn latest_triage_undo(&self) -> Result<Option<serde_json::Value>> {
        let db = self.lock()?;
        let mut query = db.prepare("SELECT operation_id,request_json,inverse_json FROM triage_batches WHERE undone=0 AND inverse_json!='[]' ORDER BY rowid DESC LIMIT 100")?;
        let candidates = query.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        for candidate in candidates {
            let (operation, request, inverse) = candidate?;
            let (kind, ids): (String, Vec<String>) = serde_json::from_str(&request)?;
            let actions: Vec<MailAction> = serde_json::from_str(&inverse)?;
            let mut accounts = std::collections::BTreeSet::new();
            let mut valid = true;
            for action in actions {
                let Some(id) = action_thread_id(&action) else {
                    valid = false;
                    break;
                };
                let current: Option<(String,String)> = db.query_row(
                    "SELECT t.account_id,a.operation_id FROM threads t JOIN actions a ON a.rowid=(SELECT MAX(rowid) FROM actions WHERE thread_id=t.id) WHERE t.id=?1",
                    [id], |r| Ok((r.get(0)?,r.get(1)?))).optional()?;
                if let Some((account, latest)) = current {
                    if latest != format!("triage:{operation}:{id}") {
                        valid = false;
                        break;
                    }
                    accounts.insert(account);
                } else {
                    valid = false;
                    break;
                }
            }
            if valid && !ids.is_empty() {
                return Ok(Some(
                    serde_json::json!({"operation_id":operation,"kind":kind,"count":ids.len(),"account_ids":accounts}),
                ));
            }
        }
        Ok(None)
    }

    /// Undo only if no newer local action touched these conversations.
    /// An inverse is queued after the original, even when it is already in flight.
    pub fn undo_triage(&self, operation_id: &str) -> Result<serde_json::Value> {
        let mut db = self.lock()?;
        let tx = db.transaction()?;
        let result = undo_triage_tx(&tx, operation_id)?;
        tx.commit()?;
        Ok(result)
    }

    /// Lightweight queue routing: never deserialize message bodies to find an account.
    pub fn thread_account_id(&self, thread_id: &str) -> Result<Option<String>> {
        self.lock()?
            .query_row(
                "SELECT account_id FROM threads WHERE id=?1",
                [thread_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(Into::into)
    }
    pub fn message_account_id(&self, message_id: &str) -> Result<Option<String>> {
        self.lock()?
            .query_row(
                "SELECT account_id FROM messages WHERE id=?1",
                [message_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(Into::into)
    }
    pub fn thread_message_ids(&self, thread_id: &str) -> Result<Vec<String>> {
        let db = self.lock()?;
        let mut query =
            db.prepare("WITH canonical(id) AS (SELECT coalesce((SELECT target_thread FROM local_conversation_links WHERE source_thread=?1),?1)) SELECT id FROM messages WHERE is_draft=0 AND thread_id IN (SELECT id FROM canonical UNION SELECT source_thread FROM local_conversation_links WHERE target_thread=(SELECT id FROM canonical)) ORDER BY received_at,id")?;
        query
            .query_map([thread_id], |row| row.get(0))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }
    /// A bounded, account-scoped prefix preserves queue order without reading other accounts.
    /// SendDraft actions have no thread_id and are handled by the separate outbox dispatcher.
    pub fn pending_actions_for_account(
        &self,
        account_id: &str,
        limit: u32,
    ) -> Result<Vec<QueuedAction>> {
        let db = self.lock()?;
        let mut query = db.prepare("SELECT a.operation_id,a.action_json FROM actions a JOIN threads t ON t.id=a.thread_id WHERE a.status='pending' AND t.account_id=?1 ORDER BY a.rowid LIMIT ?2")?;
        query
            .query_map(params![account_id, limit], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .map(|row| {
                let (operation_id, json) = row?;
                Ok(QueuedAction {
                    operation_id,
                    action: serde_json::from_str(&json)?,
                })
            })
            .collect()
    }
    pub fn pending_action_count_for_account(&self, account_id: &str) -> Result<usize> {
        self.lock()?.query_row("SELECT COUNT(*) FROM actions a JOIN threads t ON t.id=a.thread_id WHERE a.status='pending' AND t.account_id=?1",[account_id],|row|row.get(0)).map_err(Into::into)
    }
    pub fn pending_actions(&self) -> Result<Vec<QueuedAction>> {
        let db = self.lock()?;
        let mut q = db.prepare(
            "SELECT operation_id,action_json FROM actions WHERE status='pending' ORDER BY rowid",
        )?;
        let rows = q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        rows.map(|r| {
            let (id, json) = r?;
            Ok(QueuedAction {
                operation_id: id,
                action: serde_json::from_str(&json)?,
            })
        })
        .collect()
    }
    pub fn confirm_action(&self, operation_id: &str) -> Result<()> {
        self.lock()?.execute(
            "UPDATE actions SET status='confirmed',confirmed_at_ms=?2 WHERE operation_id=?1",
            params![operation_id, now_ms()],
        )?;
        Ok(())
    }
    pub fn seed_demo(&self) -> Result<()> {
        let account = Account {
            id: "demo:personal".into(),
            provider: Provider::Gmail,
            email: "alex@example.com".into(),
            display_name: "Alex Morgan".into(),
        };
        self.upsert_account(&account)?;
        for (id, sender, subject, body, at, read) in [
            (
                "demo:1",
                "Maya Chen <maya@example.org>",
                "Design review on Friday",
                "I have the latest mockups ready. Let's review the inbox and shortcut flows on Friday.",
                1_794_000_000_000_i64,
                false,
            ),
            (
                "demo:2",
                "Rowan Lee <rowan@example.net>",
                "Weekend plans",
                "Coffee and a walk this weekend? Let me know what works for you.",
                1_793_900_000_000_i64,
                true,
            ),
            (
                "demo:3",
                "Studio Notes <hello@studio.example>",
                "November dispatch",
                "A small collection of ideas about better software and calmer interfaces.",
                1_793_800_000_000_i64,
                false,
            ),
        ] {
            self.upsert_message(&Message {
                recipient_headers: None,
                id: id.into(),
                thread_id: id.into(),
                account_id: account.id.clone(),
                sender: sender.into(),
                to: vec![account.email.clone()],
                subject: subject.into(),
                body_plain: body.into(),
                body_html: None,
                unsubscribe_header: None,
                has_attachments: Some(false),
                received_at: at,
                is_read: read,
                is_draft: false,
                is_sent: false,
            })?;
        }
        Ok(())
    }
}
/// Shared by user scheduling and approved agent requests, so both apply the
/// same draft, recipient and in-progress checks inside the caller's transaction.
pub(crate) fn schedule_draft_tx(
    tx: &Transaction<'_>,
    message_id: &str,
    send_at_ms: i64,
    status: &str,
) -> Result<()> {
    if !matches!(status, "scheduled" | "undoable") || send_at_ms <= now_ms() {
        return Err(MailError::InvalidAction(
            "send time must be in the future".into(),
        ));
    }
    let is_draft: Option<bool> = tx
        .query_row(
            "SELECT is_draft FROM messages WHERE id=?1",
            [message_id],
            |r| r.get(0),
        )
        .optional()?;
    if is_draft != Some(true) {
        return Err(MailError::InvalidAction(
            "message is not an unsent draft".into(),
        ));
    }
    let (to, headers): (String, Option<String>) = tx.query_row(
        "SELECT to_json,recipient_headers_json FROM messages WHERE id=?1",
        [message_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let to: Vec<String> = serde_json::from_str(&to)?;
    let headers: Option<RecipientHeaders> =
        headers.as_deref().map(serde_json::from_str).transpose()?;
    normalize_outgoing_recipients(&to, headers.as_ref())?;
    let previous: Option<String> = tx
        .query_row(
            "SELECT status FROM outbox WHERE message_id=?1",
            [message_id],
            |r| r.get(0),
        )
        .optional()?;
    if previous
        .as_deref()
        .is_some_and(|s| !matches!(s, "scheduled" | "undoable"))
    {
        return Err(MailError::InvalidAction(
            "this send is already in progress or needs review".into(),
        ));
    }
    tx.execute("INSERT INTO outbox(message_id,send_at_ms,status,last_error,updated_at_ms) VALUES(?1,?2,?3,NULL,?4) ON CONFLICT(message_id) DO UPDATE SET send_at_ms=excluded.send_at_ms,status=excluded.status,last_error=NULL,updated_at_ms=excluded.updated_at_ms", params![message_id,send_at_ms,status,now_ms()])?;
    Ok(())
}
/// Commit a reviewed selection and its provider outbox together. IDs are
/// frozen by the caller, never re-evaluated against a changing search.
/// Shared by batch triage and agent requests for it, so a request that is
/// accepted for approval is never rejected later for its shape.
pub(crate) fn validate_triage_batch(
    operation_id: &str,
    kind: &str,
    thread_ids: &[String],
) -> Result<Vec<String>> {
    if operation_id.is_empty()
        || operation_id.len() > 200
        || thread_ids.is_empty()
        || thread_ids.len() > 20000
    {
        return Err(MailError::InvalidAction(
            "Choose 1–20,000 conversations".into(),
        ));
    }
    if !matches!(kind, "archive" | "restore" | "spam" | "read" | "unread") {
        return Err(MailError::InvalidAction("Unknown triage action".into()));
    }
    let mut ids = thread_ids.to_vec();
    ids.sort();
    ids.dedup();
    Ok(ids)
}
pub(crate) fn batch_triage_tx(
    tx: &Transaction<'_>,
    operation_id: &str,
    kind: &str,
    thread_ids: &[String],
) -> Result<serde_json::Value> {
    let ids = validate_triage_batch(operation_id, kind, thread_ids)?;
    let request = serde_json::to_string(&(kind, &ids))?;
    if let Some((previous, undone)) = tx
        .query_row(
            "SELECT request_json,undone FROM triage_batches WHERE operation_id=?1",
            [operation_id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?)),
        )
        .optional()?
    {
        if previous != request {
            return Err(MailError::InvalidAction(
                "Operation ID reused for another selection".into(),
            ));
        }
        return Ok(
            serde_json::json!({"changed":0,"undoable":!undone && matches!(kind,"archive"|"restore"|"spam")}),
        );
    }
    let mut inverse = Vec::new();
    let mut actions = Vec::new();
    for id in &ids {
        let folder: String =
            tx.query_row("SELECT folder FROM threads WHERE id=?1", [id], |r| r.get(0))?;
        let action = match kind {
            "archive" => MailAction::Archive {
                thread_id: id.clone(),
            },
            "restore" => MailAction::Move {
                thread_id: id.clone(),
                folder: "Inbox".into(),
            },
            "spam" => MailAction::Move {
                thread_id: id.clone(),
                folder: "Spam".into(),
            },
            _ => MailAction::MarkRead {
                thread_id: id.clone(),
                read: kind == "read",
            },
        };
        if matches!(kind, "archive" | "restore" | "spam") {
            inverse.push(MailAction::Move {
                thread_id: id.clone(),
                folder,
            });
        }
        actions.push(action);
    }
    for (id, action) in ids.iter().zip(&actions) {
        let op = format!("triage:{operation_id}:{id}");
        tx.execute("INSERT INTO actions(operation_id,action_json,status,thread_id) VALUES(?1,?2,'pending',?3)",params![op,serde_json::to_string(action)?,id])?;
        replay_action(&tx, action)?;
    }
    tx.execute("INSERT INTO triage_batches(operation_id,request_json,inverse_json,undone) VALUES(?1,?2,?3,0)",params![operation_id,request,serde_json::to_string(&inverse)?])?;
    Ok(serde_json::json!({"changed":ids.len(),"undoable":!inverse.is_empty()}))
}
/// Reverse a placement batch inside the caller's transaction, unless a newer
/// action changed one of its conversations.
pub(crate) fn undo_triage_tx(tx: &Transaction<'_>, operation_id: &str) -> Result<serde_json::Value> {
    let (json, undone): (String, bool) = tx.query_row(
        "SELECT inverse_json,undone FROM triage_batches WHERE operation_id=?1",
        [operation_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if undone {
        return Ok(serde_json::json!({"changed":0}));
    }
    let inverse: Vec<MailAction> = serde_json::from_str(&json)?;
    if inverse.is_empty() {
        return Err(MailError::InvalidAction("This action has no undo".into()));
    }
    for action in &inverse {
        let id = action_thread_id(action)
            .ok_or_else(|| MailError::InvalidAction("Invalid undo".into()))?;
        check_thread(&tx, id)?;
        let latest: String = tx.query_row(
            "SELECT operation_id FROM actions WHERE thread_id=?1 ORDER BY rowid DESC LIMIT 1",
            [id],
            |r| r.get(0),
        )?;
        if latest != format!("triage:{operation_id}:{id}") {
            return Err(MailError::InvalidAction(
                "A selected conversation changed again; undo would overwrite a newer action"
                    .into(),
            ));
        }
    }
    for action in &inverse {
        let id = action_thread_id(action).unwrap();
        tx.execute("INSERT INTO actions(operation_id,action_json,status,thread_id) VALUES(?1,?2,'pending',?3)",params![format!("triage:undo:{operation_id}:{id}"),serde_json::to_string(action)?,id])?;
        replay_action(&tx, action)?;
    }
    tx.execute(
        "UPDATE triage_batches SET undone=1 WHERE operation_id=?1",
        [operation_id],
    )?;
    Ok(serde_json::json!({"changed":inverse.len()}))
}
fn upsert_message_tx(tx: &Transaction<'_>, m: &Message) -> Result<()> {
    if tx
        .query_row(
            "SELECT 1 FROM accounts WHERE id=?1",
            [&m.account_id],
            |_| Ok(()),
        )
        .optional()?
        .is_none()
    {
        return Err(MailError::UnknownAccount(m.account_id.clone()));
    }
    // Provider IDs must be namespaced by the adapter. Reject collisions rather than
    // silently move a message into another account's thread.
    let thread_owner: Option<String> = tx
        .query_row(
            "SELECT account_id FROM threads WHERE id=?1",
            [&m.thread_id],
            |r| r.get(0),
        )
        .optional()?;
    if thread_owner
        .as_deref()
        .is_some_and(|owner| owner != m.account_id)
    {
        return Err(MailError::InvalidAction(
            "thread ID belongs to a different account".into(),
        ));
    }
    let prior: Option<(i64, String, String)> = tx
        .query_row(
            "SELECT rowid,account_id,thread_id FROM messages WHERE id=?1",
            [&m.id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    if prior
        .as_ref()
        .is_some_and(|(_, account, thread)| account != &m.account_id || thread != &m.thread_id)
    {
        return Err(MailError::InvalidAction(
            "message ID belongs to a different account or thread".into(),
        ));
    }
    tx.execute("INSERT INTO threads(id,account_id,subject,sender,snippet,last_message_at,unread_count,is_inbox,folder) VALUES(?1,?2,?3,?4,'',?5,0,1,'Inbox') ON CONFLICT(id) DO NOTHING",params![m.thread_id,m.account_id,m.subject,m.sender,m.received_at])?;
    if let Some((rowid, _, _)) = prior {
        tx.execute("DELETE FROM message_search WHERE rowid=?1", [rowid])?;
    }
    tx.execute("INSERT INTO messages(id,thread_id,account_id,sender,to_json,subject,body_plain,body_html,unsubscribe_header,has_attachments,received_at,is_read,is_draft,is_sent,recipient_headers_json) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15) ON CONFLICT(id) DO UPDATE SET thread_id=excluded.thread_id,account_id=excluded.account_id,sender=excluded.sender,to_json=excluded.to_json,subject=excluded.subject,body_plain=excluded.body_plain,body_html=excluded.body_html,unsubscribe_header=excluded.unsubscribe_header,has_attachments=excluded.has_attachments,received_at=excluded.received_at,is_read=excluded.is_read,is_draft=excluded.is_draft,is_sent=excluded.is_sent,recipient_headers_json=coalesce(excluded.recipient_headers_json,messages.recipient_headers_json)",params![m.id,m.thread_id,m.account_id,m.sender,serde_json::to_string(&m.to)?,m.subject,m.body_plain,m.body_html,m.unsubscribe_header,m.has_attachments,m.received_at,m.is_read,m.is_draft,m.is_sent,m.recipient_headers.as_ref().map(serde_json::to_string).transpose()?])?;
    let rowid: i64 = tx.query_row("SELECT rowid FROM messages WHERE id=?1", [&m.id], |r| {
        r.get(0)
    })?;
    tx.execute("INSERT INTO message_search(rowid,thread_id,subject,sender,body_plain) VALUES(?1,?2,?3,?4,?5)",params![rowid,m.thread_id,m.subject,m.sender,m.body_plain])?;
    refresh_thread(&tx, &m.thread_id)?;
    replay_actions_for_thread(tx, &m.thread_id)?;
    Ok(())
}
// Provider reads may briefly lag after an accepted change. Keep local actions
// authoritative during that window, then allow provider state to take over.
const CONFIRMED_ACTION_GRACE_MS: i64 = 5 * 60 * 1000;

fn active_actions_for_thread(tx: &Transaction<'_>, thread_id: &str) -> Result<Vec<MailAction>> {
    let mut query = tx.prepare(
        "SELECT action_json FROM actions WHERE thread_id=?1 AND (status='pending' OR \
         (status='confirmed' AND confirmed_at_ms>=?2)) ORDER BY rowid",
    )?;
    let cutoff = now_ms().saturating_sub(CONFIRMED_ACTION_GRACE_MS);
    let rows = query.query_map(params![thread_id, cutoff], |row| row.get::<_, String>(0))?;
    rows.map(|row| Ok(serde_json::from_str(&row?)?)).collect()
}

fn action_thread_id(action: &MailAction) -> Option<&str> {
    match action {
        MailAction::Archive { thread_id }
        | MailAction::MarkRead { thread_id, .. }
        | MailAction::Move { thread_id, .. }
        | MailAction::AddLabel { thread_id, .. }
        | MailAction::RemoveLabel { thread_id, .. } => Some(thread_id),
        MailAction::SendDraft { .. } => None,
    }
}

/// Parse an outgoing RFC mailbox list without treating commas in quoted names
/// as separators. Drafts may retain incomplete input, but delivery must use this
/// strict path so every transport sees the same individual recipients.
pub fn parse_outgoing_recipients(raw: &str) -> Result<Vec<String>> {
    if raw.chars().any(char::is_control) {
        return Err(MailError::InvalidAction(
            "Recipient addresses cannot contain control characters".into(),
        ));
    }
    let parsed = mailparse::addrparse(raw).map_err(|_| {
        MailError::InvalidAction(
            "Enter valid recipient email addresses, separated by commas".into(),
        )
    })?;
    let mut recipients = Vec::new();
    for item in parsed.iter() {
        let singles: Vec<&mailparse::SingleInfo> = match item {
            mailparse::MailAddr::Single(single) => vec![single],
            mailparse::MailAddr::Group(group) => group.addrs.iter().collect(),
        };
        for single in singles {
            if !email_address::EmailAddress::is_valid(&single.addr) {
                return Err(MailError::InvalidAction(
                    "Enter a complete recipient email address".into(),
                ));
            }
            recipients.push(match &single.display_name {
                Some(name) if !name.is_empty() => format!(
                    "\"{}\" <{}>",
                    name.replace('\\', "\\\\").replace('\"', "\\\""),
                    single.addr
                ),
                _ => single.addr.clone(),
            });
        }
    }
    if recipients.is_empty() {
        return Err(MailError::InvalidAction(
            "At least one recipient is required".into(),
        ));
    }
    Ok(recipients)
}

/// Normalize every delivery list before any outbox claim. Empty individual
/// lists are valid (for example Bcc-only mail), but the combined set is not.
pub fn normalize_outgoing_recipients(
    to: &[String],
    headers: Option<&RecipientHeaders>,
) -> Result<(Vec<String>, RecipientHeaders)> {
    fn parse(values: &[String]) -> Result<Vec<String>> {
        let raw = values.join(", ");
        if raw.trim().is_empty() {
            Ok(Vec::new())
        } else {
            parse_outgoing_recipients(&raw)
        }
    }
    let to = parse(to)?;
    let mut headers = headers.cloned().unwrap_or_default();
    headers.cc = parse(&headers.cc)?;
    headers.bcc = parse(&headers.bcc)?;
    if to.is_empty() && headers.cc.is_empty() && headers.bcc.is_empty() {
        return Err(MailError::InvalidAction(
            "At least one recipient is required".into(),
        ));
    }
    Ok((to, headers))
}

fn draft_recipients(raw: &str) -> Vec<String> {
    if raw.trim().is_empty() {
        Vec::new()
    } else {
        parse_outgoing_recipients(raw).unwrap_or_else(|_| vec![raw.trim().to_owned()])
    }
}

fn replay_action(tx: &Transaction<'_>, action: &MailAction) -> Result<()> {
    match action {
        MailAction::Archive { thread_id } => {
            tx.execute(
                "UPDATE threads SET is_inbox=0,folder='Archive' WHERE id=?1",
                [thread_id],
            )?;
        }
        MailAction::MarkRead { thread_id, read } => {
            tx.execute(
                "UPDATE messages SET is_read=?2 WHERE thread_id=?1",
                params![thread_id, read],
            )?;
            refresh_thread(tx, thread_id)?;
        }
        MailAction::Move { thread_id, folder } => {
            tx.execute(
                "UPDATE threads SET folder=?2,is_inbox=?3 WHERE id=?1",
                params![thread_id, folder, folder.eq_ignore_ascii_case("inbox")],
            )?;
        }
        MailAction::AddLabel { thread_id, label } => {
            tx.execute(
                "INSERT OR IGNORE INTO thread_labels(thread_id,label) VALUES(?1,?2)",
                params![thread_id, label],
            )?;
        }
        MailAction::RemoveLabel { thread_id, label } => {
            tx.execute(
                "DELETE FROM thread_labels WHERE thread_id=?1 AND label=?2",
                params![thread_id, label],
            )?;
        }
        MailAction::SendDraft { .. } => {}
    }
    Ok(())
}

fn replay_actions_for_thread(tx: &Transaction<'_>, thread_id: &str) -> Result<()> {
    for action in active_actions_for_thread(tx, thread_id)? {
        replay_action(tx, &action)?;
    }
    Ok(())
}

fn replay_actions_for_account(tx: &Transaction<'_>, account_id: &str) -> Result<()> {
    let mut query = tx.prepare(
        "SELECT a.action_json FROM actions a JOIN threads t ON t.id=a.thread_id \
         WHERE t.account_id=?1 AND (a.status='pending' OR \
         (a.status='confirmed' AND a.confirmed_at_ms>=?2)) ORDER BY a.rowid",
    )?;
    let cutoff = now_ms().saturating_sub(CONFIRMED_ACTION_GRACE_MS);
    let actions = query
        .query_map(params![account_id, cutoff], |row| row.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    for json in actions {
        replay_action(tx, &serde_json::from_str(&json)?)?;
    }
    Ok(())
}

fn check_thread(tx: &Transaction<'_>, id: &str) -> Result<()> {
    if tx
        .query_row("SELECT 1 FROM threads WHERE id=?1", [id], |_| Ok(()))
        .optional()?
        .is_none()
    {
        Err(MailError::NotFound(id.into()))
    } else {
        Ok(())
    }
}
fn check_thread_connection(db: &Connection, id: &str) -> Result<()> {
    if db
        .query_row("SELECT 1 FROM threads WHERE id=?1", [id], |_| Ok(()))
        .optional()?
        .is_none()
    {
        Err(MailError::NotFound(id.into()))
    } else {
        Ok(())
    }
}
fn refresh_thread(tx: &Transaction<'_>, id: &str) -> Result<()> {
    tx.execute("UPDATE threads SET subject=(SELECT subject FROM messages WHERE thread_id=?1 ORDER BY received_at DESC LIMIT 1),sender=(SELECT sender FROM messages WHERE thread_id=?1 ORDER BY received_at DESC LIMIT 1),snippet=(SELECT substr(body_plain,1,180) FROM messages WHERE thread_id=?1 ORDER BY received_at DESC LIMIT 1),last_message_at=(SELECT max(received_at) FROM messages WHERE thread_id=?1),unread_count=(SELECT count(*) FROM messages WHERE thread_id=?1 AND is_read=0 AND is_draft=0) WHERE id=?1",[id])?;
    Ok(())
}
fn provider_folder(
    tx: &Transaction<'_>,
    account_id: &str,
    thread_id: &str,
    labels: &[String],
) -> Result<()> {
    let gmail: bool = tx.query_row(
        "SELECT provider='gmail' FROM accounts WHERE id=?1",
        [account_id],
        |r| r.get(0),
    )?;
    if gmail
        && labels.iter().any(|v| {
            matches!(
                v.as_str(),
                "INBOX" | "SPAM" | "TRASH" | "DRAFT" | "__EMAIL_ARCHIVE__"
            )
        })
    {
        let contains = |label: &str| labels.iter().any(|v| v == label);
        let folder = if contains("TRASH") {
            "Trash"
        } else if contains("SPAM") {
            "Spam"
        } else if contains("INBOX") {
            "Inbox"
        } else if contains("DRAFT") {
            "Drafts"
        } else {
            "Archive"
        };
        tx.execute(
            "UPDATE threads SET is_inbox=?2,folder=?3 WHERE id=?1",
            params![thread_id, folder == "Inbox", folder],
        )?;
    }
    Ok(())
}

fn split_filter(query: &str, values: &mut Vec<rusqlite::types::Value>) -> Result<String> {
    if query.len() > 256 || query.trim().is_empty() {
        return Err(MailError::InvalidAction(
            "Enter a split rule of up to 256 characters".into(),
        ));
    }
    let mut clauses = Vec::new();
    // Comma/newline separated rules are alternatives (OR), not an implicit AND.
    for term in query
        .split([',', '\n'])
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .take(17)
    {
        if clauses.len() >= 16 {
            return Err(MailError::InvalidAction(
                "Use at most 16 alternative split rules".into(),
            ));
        }
        let (field, needle) = term
            .split_once(':')
            .map(|(f, v)| (f.to_ascii_lowercase(), v.trim()))
            .unwrap_or(("any".into(), term));
        let sender = "lower(trim(CASE WHEN instr(t.sender,'<')>0 THEN substr(t.sender,instr(t.sender,'<')+1,instr(t.sender,'>')-instr(t.sender,'<')-1) ELSE t.sender END))";
        let recipient = "lower(trim(CASE WHEN instr(j.value,'<')>0 THEN substr(j.value,instr(j.value,'<')+1,instr(j.value,'>')-instr(j.value,'<')-1) ELSE j.value END))";
        if term.eq_ignore_ascii_case("unread") {
            clauses.push("t.unread_count>0".into());
            continue;
        }
        if needle.is_empty() {
            return Err(MailError::InvalidAction(
                "A split rule is missing its value".into(),
            ));
        }
        let index = values.len() + 1;
        let binding = format!("?{index}");
        let is_address = needle.contains('@') || needle.contains('*');
        let predicate = match field.as_str() {
            "label" => format!(
                "EXISTS (SELECT 1 FROM thread_labels l WHERE l.thread_id=t.id AND lower(l.label)=lower({binding}))"
            ),
            "subject" => format!("instr(lower(t.subject),lower({binding}))>0"),
            "from" if !is_address => format!("instr(lower(t.sender),lower({binding}))>0"),
            "from" => format!("{sender} LIKE {binding} ESCAPE '\\'"),
            "to" => format!(
                "EXISTS (SELECT 1 FROM messages m,json_each(m.to_json) j WHERE m.thread_id=t.id AND {recipient} LIKE {binding} ESCAPE '\\')"
            ),
            "any" if is_address => format!(
                "({sender} LIKE {binding} ESCAPE '\\' OR EXISTS (SELECT 1 FROM messages m,json_each(m.to_json) j WHERE m.thread_id=t.id AND {recipient} LIKE {binding} ESCAPE '\\'))"
            ),
            "any" => format!(
                "(instr(lower(t.sender),lower({binding}))>0 OR instr(lower(t.subject),lower({binding}))>0 OR instr(lower(t.snippet),lower({binding}))>0 OR EXISTS (SELECT 1 FROM thread_labels l WHERE l.thread_id=t.id AND instr(lower(l.label),lower({binding}))>0))"
            ),
            _ => {
                return Err(MailError::InvalidAction(
                    "Use from:, to:, subject:, label:, or an address/domain rule".into(),
                ));
            }
        };
        let value = if matches!(field.as_str(), "from" | "to" | "any") && is_address {
            needle
                .to_lowercase()
                .chars()
                .fold(String::new(), |mut out, c| {
                    match c {
                        '*' => out.push('%'),
                        '%' | '_' | '\\' => {
                            out.push('\\');
                            out.push(c);
                        }
                        _ => out.push(c),
                    };
                    out
                })
        } else {
            needle.to_owned()
        };
        values.push(value.into());
        clauses.push(predicate);
    }
    if clauses.is_empty() {
        return Err(MailError::InvalidAction(
            "Enter at least one split rule".into(),
        ));
    }
    Ok(format!("({})", clauses.join(" OR ")))
}

fn split_threads(
    db: &Connection,
    query: &str,
    account_id: Option<&str>,
    unread_only: bool,
    limit: u32,
    offset: u32,
) -> Result<Vec<ThreadSummary>> {
    let mut values: Vec<rusqlite::types::Value> = vec![
        account_id
            .map(|v| v.to_string().into())
            .unwrap_or(rusqlite::types::Value::Null),
        now_ms().into(),
        (unread_only as i64).into(),
    ];
    let filter = split_filter(query, &mut values)?;
    let sql = format!(
        "SELECT t.id,t.account_id,t.subject,t.sender,t.snippet,t.last_message_at,t.unread_count,t.is_inbox,t.folder FROM threads t WHERE t.is_inbox=1 AND (?1 IS NULL OR t.account_id=?1) AND (?3=0 OR t.unread_count>0) AND NOT EXISTS (SELECT 1 FROM snoozes s WHERE s.thread_id=t.id AND s.wake_at_ms>?2) AND ({}) ORDER BY t.last_message_at DESC,t.id LIMIT {} OFFSET {}",
        filter,
        limit.min(500),
        offset
    );
    let mut statement = db.prepare(&sql)?;
    let mut rows = statement.query(rusqlite::params_from_iter(values))?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(thread_row(db, row)?);
    }
    Ok(out)
}

fn thread_row(db: &Connection, r: &rusqlite::Row<'_>) -> Result<ThreadSummary> {
    let id: String = r.get(0)?;
    let mut q = db.prepare("SELECT label FROM thread_labels WHERE thread_id=?1 ORDER BY label")?;
    let labels = q
        .query_map([&id], |r| r.get(0))?
        .collect::<std::result::Result<Vec<String>, _>>()?;
    let snoozed_until_ms = db
        .query_row(
            "SELECT wake_at_ms FROM snoozes WHERE thread_id=?1 AND wake_at_ms>?2",
            params![&id, now_ms()],
            |r| r.get(0),
        )
        .optional()?;
    let is_pinned = db
        .query_row(
            "SELECT 1 FROM pinned_threads WHERE thread_id=?1",
            [&id],
            |_| Ok(true),
        )
        .optional()?
        .unwrap_or(false);
    Ok(ThreadSummary {
        id,
        account_id: r.get(1)?,
        subject: r.get(2)?,
        sender: r.get(3)?,
        snippet: r.get(4)?,
        last_message_at: r.get(5)?,
        unread_count: r.get::<_, i64>(6)? as u32,
        is_inbox: r.get(7)?,
        folder: r.get(8)?,
        labels,
        is_pinned,
        snoozed_until_ms,
    })
}
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS accounts(id TEXT PRIMARY KEY,provider TEXT NOT NULL,email TEXT NOT NULL,display_name TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS threads(id TEXT PRIMARY KEY,account_id TEXT NOT NULL REFERENCES accounts(id),subject TEXT NOT NULL,sender TEXT NOT NULL,snippet TEXT NOT NULL,last_message_at INTEGER NOT NULL,unread_count INTEGER NOT NULL,is_inbox INTEGER NOT NULL,folder TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS threads_inbox_time ON threads(is_inbox,last_message_at DESC);
CREATE INDEX IF NOT EXISTS threads_search_time ON threads(last_message_at DESC,id);
CREATE INDEX IF NOT EXISTS threads_search_account_time ON threads(account_id,last_message_at DESC,id);
CREATE TABLE IF NOT EXISTS messages(id TEXT NOT NULL UNIQUE,thread_id TEXT NOT NULL REFERENCES threads(id),account_id TEXT NOT NULL REFERENCES accounts(id),sender TEXT NOT NULL,to_json TEXT NOT NULL,subject TEXT NOT NULL,body_plain TEXT NOT NULL,body_html TEXT,unsubscribe_header TEXT,has_attachments INTEGER,received_at INTEGER NOT NULL,is_read INTEGER NOT NULL,is_draft INTEGER NOT NULL,is_sent INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS messages_thread ON messages(thread_id,received_at DESC);
CREATE INDEX IF NOT EXISTS messages_account_recent_recipients ON messages(account_id,received_at DESC,id);
CREATE VIRTUAL TABLE IF NOT EXISTS message_search USING fts5(thread_id UNINDEXED,subject,sender,body_plain);
CREATE TABLE IF NOT EXISTS thread_labels(thread_id TEXT NOT NULL REFERENCES threads(id),label TEXT NOT NULL,PRIMARY KEY(thread_id,label));
CREATE TABLE IF NOT EXISTS snoozes(thread_id TEXT PRIMARY KEY REFERENCES threads(id),wake_at_ms INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS pinned_threads(thread_id TEXT PRIMARY KEY);
CREATE INDEX IF NOT EXISTS snoozes_wake ON snoozes(wake_at_ms);
CREATE TABLE IF NOT EXISTS snippets(id TEXT PRIMARY KEY,name TEXT NOT NULL COLLATE NOCASE UNIQUE,body TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS split_inboxes(id TEXT PRIMARY KEY,name TEXT NOT NULL,query TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS local_conversation_links(source_thread TEXT PRIMARY KEY,target_thread TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS outbox(message_id TEXT PRIMARY KEY REFERENCES messages(id),send_at_ms INTEGER NOT NULL,status TEXT NOT NULL,last_error TEXT,updated_at_ms INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS outbox_due ON outbox(status,send_at_ms);
CREATE TABLE IF NOT EXISTS draft_attachments(id TEXT PRIMARY KEY,message_id TEXT NOT NULL REFERENCES messages(id),filename TEXT NOT NULL,mime_type TEXT NOT NULL,bytes BLOB NOT NULL,size INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS draft_attachments_message ON draft_attachments(message_id);
CREATE TABLE IF NOT EXISTS actions(operation_id TEXT PRIMARY KEY,action_json TEXT NOT NULL,status TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS triage_batches(operation_id TEXT PRIMARY KEY,request_json TEXT NOT NULL,inverse_json TEXT NOT NULL,undone INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS archive_pages(account_id TEXT PRIMARY KEY REFERENCES accounts(id) ON DELETE CASCADE,cursor TEXT,complete INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS sync_state(account_id TEXT PRIMARY KEY REFERENCES accounts(id),value TEXT NOT NULL,updated_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS sync_reset_state(account_id TEXT PRIMARY KEY REFERENCES accounts(id),page_cursor TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS sync_reset_messages(account_id TEXT NOT NULL REFERENCES accounts(id),message_id TEXT NOT NULL,message_json TEXT NOT NULL,PRIMARY KEY(account_id,message_id));
CREATE TABLE IF NOT EXISTS sync_reset_labels(account_id TEXT NOT NULL REFERENCES accounts(id),thread_id TEXT NOT NULL,labels_json TEXT NOT NULL,PRIMARY KEY(account_id,thread_id));
"#;

pub mod ffi;
