//! Discarding local drafts, with a bounded restore window.
//!
//! A discarded draft leaves every live surface (Drafts, search, its reply
//! conversation) immediately. A private snapshot keeps its content,
//! attachments and tracking choice so Undo can put it back exactly. Snapshots
//! expire after `RESTORE_WINDOW_MS` and are purged by later discards.
use crate::{MailError, MailStore, Result, now_ms, refresh_thread};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// How long a discarded draft can be restored.
pub const RESTORE_WINDOW_MS: i64 = 7 * 24 * 60 * 60 * 1000;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct DiscardedDraft {
    pub message_id: String,
    pub account_id: String,
    pub subject: String,
    pub discarded_at_ms: i64,
    pub restorable_until_ms: i64,
}

pub(crate) fn initialize(db: &Connection) -> Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS discarded_drafts(
            message_id TEXT PRIMARY KEY,
            thread_id TEXT NOT NULL,
            account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
            sender TEXT NOT NULL,
            to_json TEXT NOT NULL,
            subject TEXT NOT NULL,
            body_plain TEXT NOT NULL,
            body_html TEXT,
            recipient_headers_json TEXT,
            received_at INTEGER NOT NULL,
            thread_folder TEXT NOT NULL,
            tracking_id TEXT,
            tracking_enabled INTEGER,
            tracking_host_id TEXT,
            discarded_at_ms INTEGER NOT NULL);
         CREATE INDEX IF NOT EXISTS discarded_drafts_time ON discarded_drafts(discarded_at_ms);
         CREATE TABLE IF NOT EXISTS discarded_draft_attachments(
            id TEXT PRIMARY KEY,
            message_id TEXT NOT NULL REFERENCES discarded_drafts(message_id) ON DELETE CASCADE,
            filename TEXT NOT NULL,
            mime_type TEXT NOT NULL,
            bytes BLOB NOT NULL,
            size INTEGER NOT NULL);
         CREATE INDEX IF NOT EXISTS discarded_draft_attachments_message ON discarded_draft_attachments(message_id);",
    )?;
    Ok(())
}

fn table_exists(tx: &Transaction<'_>, name: &str) -> Result<bool> {
    Ok(tx
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1",
            [name],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

impl MailStore {
    /// Account routing for a restorable draft, without reading its content.
    pub fn discarded_draft_account_id(&self, message_id: &str) -> Result<Option<String>> {
        Ok(self
            .lock()?
            .query_row(
                "SELECT account_id FROM discarded_drafts WHERE message_id=?1",
                [message_id],
                |r| r.get(0),
            )
            .optional()?)
    }
    /// Remove an unsent local draft. Drafts queued or scheduled for sending
    /// must be cancelled first, so a discard can never race delivery.
    pub fn discard_draft(&self, message_id: &str) -> Result<DiscardedDraft> {
        let mut db = self.lock()?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = now_ms();
        tx.execute(
            "DELETE FROM discarded_drafts WHERE discarded_at_ms<?1",
            [now - RESTORE_WINDOW_MS],
        )?;
        let row: Option<(i64, String, String, String, bool)> = tx
            .query_row(
                "SELECT rowid,thread_id,account_id,subject,is_draft FROM messages WHERE id=?1",
                [message_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .optional()?;
        let Some((rowid, thread_id, account_id, subject, is_draft)) = row else {
            return Err(MailError::NotFound(message_id.into()));
        };
        if !is_draft {
            return Err(MailError::InvalidAction(
                "Only unsent drafts can be discarded".into(),
            ));
        }
        if !message_id.starts_with("local:") {
            return Err(MailError::InvalidAction(
                "This draft is stored by your provider. Delete it there.".into(),
            ));
        }
        let queued: Option<String> = tx
            .query_row(
                "SELECT status FROM outbox WHERE message_id=?1",
                [message_id],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(status) = queued {
            return Err(MailError::InvalidAction(if status == "sending" {
                "This draft is being sent and can no longer be discarded".into()
            } else {
                "Cancel the scheduled send before discarding this draft".into()
            }));
        }
        let folder: String = tx
            .query_row("SELECT folder FROM threads WHERE id=?1", [&thread_id], |r| {
                r.get(0)
            })
            .optional()?
            .unwrap_or_else(|| "Drafts".into());
        let tracking: Option<(String, bool, Option<String>)> = if table_exists(&tx, "email_tracking")? {
            tx.query_row(
                "SELECT tracking_id,enabled,host_id FROM email_tracking WHERE message_id=?1",
                [message_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?
        } else {
            None
        };
        tx.execute("DELETE FROM discarded_drafts WHERE message_id=?1", [message_id])?;
        tx.execute(
            "INSERT INTO discarded_drafts(message_id,thread_id,account_id,sender,to_json,subject,body_plain,body_html,recipient_headers_json,received_at,thread_folder,tracking_id,tracking_enabled,tracking_host_id,discarded_at_ms) \
             SELECT id,thread_id,account_id,sender,to_json,subject,body_plain,body_html,recipient_headers_json,received_at,?2,?3,?4,?5,?6 FROM messages WHERE id=?1",
            params![
                message_id,
                folder,
                tracking.as_ref().map(|t| t.0.clone()),
                tracking.as_ref().map(|t| t.1),
                tracking.as_ref().and_then(|t| t.2.clone()),
                now
            ],
        )?;
        tx.execute(
            "INSERT INTO discarded_draft_attachments(id,message_id,filename,mime_type,bytes,size) \
             SELECT id,message_id,filename,mime_type,bytes,size FROM draft_attachments WHERE message_id=?1",
            [message_id],
        )?;
        tx.execute("DELETE FROM draft_attachments WHERE message_id=?1", [message_id])?;
        tx.execute("DELETE FROM message_search WHERE rowid=?1", [rowid])?;
        tx.execute("DELETE FROM messages WHERE rowid=?1", [rowid])?;
        let remaining: i64 = tx.query_row(
            "SELECT count(*) FROM messages WHERE thread_id=?1",
            [&thread_id],
            |r| r.get(0),
        )?;
        if remaining == 0 {
            tx.execute("DELETE FROM snoozes WHERE thread_id=?1", [&thread_id])?;
            tx.execute("DELETE FROM thread_labels WHERE thread_id=?1", [&thread_id])?;
            tx.execute("DELETE FROM pinned_threads WHERE thread_id=?1", [&thread_id])?;
            tx.execute("DELETE FROM threads WHERE id=?1", [&thread_id])?;
        } else {
            refresh_thread(&tx, &thread_id)?;
        }
        tx.commit()?;
        Ok(DiscardedDraft {
            message_id: message_id.into(),
            account_id,
            subject,
            discarded_at_ms: now,
            restorable_until_ms: now + RESTORE_WINDOW_MS,
        })
    }

    /// Put a discarded draft back, with its attachments and tracking choice.
    /// A reply whose conversation has since disappeared returns as a new draft.
    pub fn restore_discarded_draft(&self, message_id: &str) -> Result<crate::Message> {
        {
            let mut db = self.lock()?;
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let snapshot: Option<(String, String, String, i64, i64)> = tx
                .query_row(
                    "SELECT thread_id,account_id,thread_folder,received_at,discarded_at_ms FROM discarded_drafts WHERE message_id=?1",
                    [message_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
                )
                .optional()?;
            let Some((mut thread_id, account_id, folder, received_at, discarded_at)) = snapshot
            else {
                return Err(MailError::InvalidAction(
                    "This draft can no longer be restored".into(),
                ));
            };
            if discarded_at < now_ms() - RESTORE_WINDOW_MS {
                tx.execute("DELETE FROM discarded_drafts WHERE message_id=?1", [message_id])?;
                tx.commit()?;
                return Err(MailError::InvalidAction(
                    "This draft can no longer be restored".into(),
                ));
            }
            if tx
                .query_row("SELECT 1 FROM messages WHERE id=?1", [message_id], |_| Ok(()))
                .optional()?
                .is_some()
            {
                return Err(MailError::InvalidAction("This draft already exists".into()));
            }
            let owner: Option<String> = tx
                .query_row("SELECT account_id FROM threads WHERE id=?1", [&thread_id], |r| {
                    r.get(0)
                })
                .optional()?;
            match owner {
                Some(owner) if owner == account_id => {}
                _ if thread_id != message_id => {
                    // The reply conversation was removed (or never synced back).
                    // Keep the draft as a standalone message rather than lose it.
                    thread_id = message_id.to_string();
                }
                _ => {}
            }
            let thread_exists = tx
                .query_row("SELECT 1 FROM threads WHERE id=?1", [&thread_id], |_| Ok(()))
                .optional()?
                .is_some();
            if !thread_exists {
                let folder = if thread_id == message_id { folder } else { "Drafts".into() };
                tx.execute(
                    "INSERT INTO threads(id,account_id,subject,sender,snippet,last_message_at,unread_count,is_inbox,folder) \
                     SELECT ?2,account_id,subject,sender,'',?3,0,0,?4 FROM discarded_drafts WHERE message_id=?1",
                    params![message_id, thread_id, received_at, folder],
                )?;
            }
            let attachments: i64 = tx.query_row(
                "SELECT count(*) FROM discarded_draft_attachments WHERE message_id=?1",
                [message_id],
                |r| r.get(0),
            )?;
            tx.execute(
                "INSERT INTO messages(id,thread_id,account_id,sender,to_json,subject,body_plain,body_html,unsubscribe_header,has_attachments,received_at,is_read,is_draft,is_sent,recipient_headers_json) \
                 SELECT message_id,?2,account_id,sender,to_json,subject,body_plain,body_html,NULL,?3,received_at,1,1,0,recipient_headers_json FROM discarded_drafts WHERE message_id=?1",
                params![message_id, thread_id, attachments > 0],
            )?;
            tx.execute(
                "INSERT INTO message_search(rowid,thread_id,subject,sender,body_plain) \
                 SELECT rowid,thread_id,subject,sender,body_plain FROM messages WHERE id=?1",
                [message_id],
            )?;
            tx.execute(
                "INSERT INTO draft_attachments(id,message_id,filename,mime_type,bytes,size) \
                 SELECT id,message_id,filename,mime_type,bytes,size FROM discarded_draft_attachments WHERE message_id=?1",
                [message_id],
            )?;
            let tracking: Option<(Option<String>, Option<bool>, Option<String>)> = tx
                .query_row(
                    "SELECT tracking_id,tracking_enabled,tracking_host_id FROM discarded_drafts WHERE message_id=?1",
                    [message_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?;
            if let Some((Some(tracking_id), enabled, host)) = tracking {
                crate::tracking::schema(&tx)?;
                tx.execute(
                    "INSERT OR IGNORE INTO email_tracking(message_id,tracking_id,enabled,host_id) VALUES(?1,?2,?3,?4)",
                    params![message_id, tracking_id, enabled.unwrap_or(false), host],
                )?;
            }
            tx.execute("DELETE FROM discarded_drafts WHERE message_id=?1", [message_id])?;
            refresh_thread(&tx, &thread_id)?;
            tx.commit()?;
        }
        self.get_message(message_id)?
            .ok_or_else(|| MailError::NotFound(message_id.into()))
    }
}
