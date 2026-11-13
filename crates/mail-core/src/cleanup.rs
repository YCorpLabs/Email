//! Durable, explicitly confirmed provider-side sender cleanup snapshots.
use crate::{MailError, MailStore, Result, now_ms, refresh_thread};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS sender_cleanups(id TEXT PRIMARY KEY, account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,sender TEXT NOT NULL,state TEXT NOT NULL,phase TEXT NOT NULL,page_token TEXT,created_at_ms INTEGER NOT NULL,confirmed_action_rowid INTEGER,last_error TEXT,retry_at_ms INTEGER NOT NULL DEFAULT 0,lease TEXT,lease_until_ms INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS sender_cleanup_items(job_id TEXT NOT NULL REFERENCES sender_cleanups(id) ON DELETE CASCADE,message_id TEXT NOT NULL,outcome TEXT NOT NULL DEFAULT 'pending',action_marker TEXT,read_action_marker TEXT,PRIMARY KEY(job_id,message_id));
CREATE INDEX IF NOT EXISTS cleanup_pending ON sender_cleanup_items(job_id,outcome);
"#;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SenderCleanup {
    pub id: String,
    pub account_id: String,
    pub sender: String,
    pub state: String,
    pub phase: String,
    pub page_token: Option<String>,
    pub created_at_ms: i64,
    pub last_error: Option<String>,
    pub retry_at_ms: i64,
    pub matched: usize,
    pub completed: usize,
    pub skipped: usize,
    /// Subjects only from locally cached members of the provider snapshot.
    pub sample_subjects: Vec<String>,
}
#[derive(Debug, Clone)]
pub struct CleanupLease {
    pub job: SenderCleanup,
    pub token: String,
    pub message_ids: Vec<String>,
}
fn ensure(db: &Connection) -> Result<()> {
    db.execute_batch(SCHEMA)?;
    Ok(())
}
fn invalid(s: &str) -> MailError {
    MailError::InvalidAction(s.into())
}
/// Confirm a ready snapshot inside the caller's transaction. Shared by the
/// user's own confirmation and by an approved agent request.
pub(crate) fn confirm_tx(db: &Connection, id: &str) -> Result<SenderCleanup> {
    ensure(db)?;
    let job = read(db, id)?;
    if job.state != "ready" || job.phase != "apply" {
        return Err(invalid("Wait for the cleanup preview before confirming"));
    }
    db.execute("UPDATE sender_cleanup_items SET read_action_marker=(SELECT a.operation_id FROM messages m JOIN actions a ON a.thread_id=m.thread_id WHERE m.id=sender_cleanup_items.message_id AND json_extract(a.action_json,'$.type')='mark_read' ORDER BY a.rowid DESC LIMIT 1),action_marker=(SELECT a.operation_id FROM messages m JOIN actions a ON a.thread_id=m.thread_id WHERE m.id=sender_cleanup_items.message_id ORDER BY a.rowid DESC LIMIT 1) WHERE job_id=?1",[id])?;
    let watermark: i64 = db.query_row("SELECT coalesce(max(rowid),0) FROM actions", [], |r| {
        r.get(0)
    })?;
    db.execute("UPDATE sender_cleanups SET state=?2,confirmed_action_rowid=?3,last_error=NULL WHERE id=?1",params![id,if job.matched==0{"completed"}else{"running"},watermark])?;
    read(db, id)
}
pub(crate) fn read_job(db: &Connection, id: &str) -> Result<SenderCleanup> {
    ensure(db)?;
    read(db, id)
}
fn read(db: &Connection, id: &str) -> Result<SenderCleanup> {
    let mut job=db.query_row("SELECT id,account_id,sender,state,phase,page_token,created_at_ms,last_error,retry_at_ms FROM sender_cleanups WHERE id=?1",[id],|r|Ok(SenderCleanup{id:r.get(0)?,account_id:r.get(1)?,sender:r.get(2)?,state:r.get(3)?,phase:r.get(4)?,page_token:r.get(5)?,created_at_ms:r.get(6)?,last_error:r.get(7)?,retry_at_ms:r.get(8)?,matched:0,completed:0,skipped:0,sample_subjects:vec![]})).optional()?.ok_or_else(||MailError::NotFound(id.into()))?;
    (job.matched,job.completed,job.skipped)=db.query_row("SELECT count(*),coalesce(sum(outcome='done'),0),coalesce(sum(outcome='skipped'),0) FROM sender_cleanup_items WHERE job_id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
    let mut q=db.prepare("SELECT DISTINCT m.subject FROM sender_cleanup_items i JOIN messages m ON m.id=i.message_id WHERE i.job_id=?1 ORDER BY m.received_at DESC LIMIT 5")?;
    job.sample_subjects = q
        .query_map([id], |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()?;
    Ok(job)
}
impl MailStore {
    pub fn create_sender_cleanup(&self, account_id: &str, sender: &str) -> Result<SenderCleanup> {
        let addresses = crate::parse_outgoing_recipients(sender)?;
        if addresses.len() != 1 {
            return Err(invalid("Choose exactly one sender"));
        }
        let parsed = mailparse::addrparse(&addresses[0]).map_err(|_| invalid("Invalid sender"))?;
        let mailbox = match &parsed[0] {
            mailparse::MailAddr::Single(v) => v.addr.to_lowercase(),
            _ => return Err(invalid("Choose one sender mailbox")),
        };
        // Strict literal mailbox prevents injecting Gmail query operators.
        if !mailbox
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || ".!#$%&'*+-/=?^_`{|}~@".contains(c))
        {
            return Err(invalid("Sender mailbox is not supported for cleanup"));
        }
        let mut db = self.lock()?;
        ensure(&db)?;
        let tx = db.transaction()?;
        let provider: String = tx.query_row(
            "SELECT provider FROM accounts WHERE id=?1",
            [account_id],
            |r| r.get(0),
        )?;
        if provider != "gmail" {
            return Err(invalid(
                "Provider-wide sender cleanup currently supports Gmail",
            ));
        }
        if tx.query_row(
            "SELECT count(*) FROM sender_cleanups WHERE state NOT IN ('completed','cancelled')",
            [],
            |r| r.get::<_, i64>(0),
        )? >= 50
        {
            return Err(invalid("Finish or cancel an existing cleanup first"));
        }
        let id = uuid::Uuid::new_v4().to_string();
        tx.execute("INSERT INTO sender_cleanups(id,account_id,sender,state,phase,created_at_ms) VALUES(?1,?2,?3,'discovering','scan',?4)",params![id,account_id,mailbox,now_ms()])?;
        tx.commit()?;
        read(&db, &id)
    }
    pub fn sender_cleanup(&self, id: &str) -> Result<SenderCleanup> {
        let db = self.lock()?;
        ensure(&db)?;
        read(&db, id)
    }
    pub fn list_sender_cleanups(&self, account_id: Option<&str>) -> Result<Vec<SenderCleanup>> {
        let db = self.lock()?;
        ensure(&db)?;
        let mut q=db.prepare("SELECT id FROM sender_cleanups WHERE (?1 IS NULL OR account_id=?1) ORDER BY (state NOT IN ('completed','cancelled')) DESC,created_at_ms DESC LIMIT 100")?;
        let ids = q
            .query_map([account_id], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        ids.iter().map(|id| read(&db, id)).collect()
    }
    /// Own at most one bounded network operation; expired leases recover after a crash.
    pub fn claim_cleanup_step(&self, id: &str) -> Result<Option<CleanupLease>> {
        let mut db = self.lock()?;
        ensure(&db)?;
        let tx = db.transaction()?;
        let job = read(&tx, id)?;
        if !matches!(job.state.as_str(), "discovering" | "running") || job.retry_at_ms > now_ms() {
            return Ok(None);
        }
        let token = uuid::Uuid::new_v4().to_string();
        if tx.execute("UPDATE sender_cleanups SET lease=?2,lease_until_ms=?3 WHERE id=?1 AND (lease IS NULL OR lease_until_ms<=?4) AND NOT EXISTS(SELECT 1 FROM sender_cleanups other WHERE other.account_id=sender_cleanups.account_id AND other.id<>?1 AND other.lease IS NOT NULL AND other.lease_until_ms>?4)",params![id,token,now_ms()+120_000,now_ms()])?==0{return Ok(None);}
        let message_ids = if job.phase == "apply" {
            tx.execute("UPDATE sender_cleanup_items SET outcome='skipped' WHERE job_id=?1 AND outcome='pending' AND EXISTS(SELECT 1 FROM messages m WHERE m.id=sender_cleanup_items.message_id AND coalesce((SELECT operation_id FROM actions WHERE thread_id=m.thread_id ORDER BY rowid DESC LIMIT 1),'')<>coalesce(sender_cleanup_items.action_marker,''))",[id])?;
            let mut q=tx.prepare("SELECT message_id FROM sender_cleanup_items WHERE job_id=?1 AND outcome='pending' ORDER BY message_id LIMIT 500")?;
            q.query_map([id], |r| r.get(0))?
                .collect::<std::result::Result<Vec<String>, _>>()?
        } else {
            vec![]
        };
        tx.commit()?;
        Ok(Some(CleanupLease {
            job,
            token,
            message_ids,
        }))
    }
    pub fn record_cleanup_page(
        &self,
        lease: &CleanupLease,
        ids: &[String],
        next: Option<&str>,
    ) -> Result<SenderCleanup> {
        if ids.len() > 500 || next.is_some_and(|s| s.len() > 8192 || s.is_empty()) {
            return Err(invalid("Invalid cleanup page"));
        }
        let prefix = format!("{}:gmail-message:", lease.job.account_id);
        if ids.iter().any(|id| {
            !id.strip_prefix(&prefix).is_some_and(|s| {
                !s.is_empty()
                    && s.len() <= 256
                    && s.chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            })
        }) {
            return Err(invalid("Cleanup page crosses account boundaries"));
        }
        let mut db = self.lock()?;
        ensure(&db)?;
        let tx = db.transaction()?;
        check_lease(&tx, lease)?;
        let current = read(&tx, &lease.job.id)?;
        if current.phase != "scan" || current.page_token != lease.job.page_token {
            return Err(invalid("Stale cleanup page"));
        }
        if next.is_some() && next == current.page_token.as_deref() {
            return Err(invalid("Provider repeated the cleanup page token"));
        }
        let mut distinct_new = std::collections::HashSet::new();
        for id in ids {
            let exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM sender_cleanup_items WHERE job_id=?1 AND message_id=?2)",params![current.id,id], |r| r.get(0))?;
            if !exists {
                distinct_new.insert(id);
            }
        }
        if current.matched + distinct_new.len() > 100_000 {
            return Err(invalid(
                "Cleanup snapshot exceeds 100,000 messages; narrow the sender scope",
            ));
        }
        for id in ids {
            tx.execute(
                "INSERT OR IGNORE INTO sender_cleanup_items(job_id,message_id) VALUES(?1,?2)",
                params![current.id, id],
            )?;
        }
        tx.execute("UPDATE sender_cleanups SET page_token=?2,lease=NULL,lease_until_ms=0,last_error=NULL,retry_at_ms=0,state=CASE WHEN state='discovering' AND ?2 IS NULL THEN 'ready' ELSE state END,phase=CASE WHEN ?2 IS NULL THEN 'apply' ELSE phase END WHERE id=?1",params![current.id,next])?;
        tx.commit()?;
        read(&db, &current.id)
    }
    /// Confirm only the enumerated snapshot; new incoming mail is excluded.
    pub fn confirm_sender_cleanup(&self, id: &str) -> Result<SenderCleanup> {
        let mut db = self.lock()?;
        ensure(&db)?;
        let tx = db.transaction()?;
        confirm_tx(&tx, id)?;
        tx.commit()?;
        read(&db, id)
    }
    /// Pause/cancel cannot recall an in-flight request; its result still commits.
    pub fn control_sender_cleanup(&self, id: &str, command: &str) -> Result<SenderCleanup> {
        let db = self.lock()?;
        ensure(&db)?;
        let job = read(&db, id)?;
        let state = match command {
            "pause" if matches!(job.state.as_str(), "discovering" | "running") => "paused",
            "resume" if job.state == "paused" => {
                if job.phase == "scan" {
                    "discovering"
                } else if cleanup_confirmed(&db, id)? {
                    "running"
                } else {
                    "ready"
                }
            }
            "cancel" if !matches!(job.state.as_str(), "completed" | "cancelled") => "cancelled",
            _ => return Err(invalid("This cleanup cannot perform that transition")),
        };
        if db.execute(
            "UPDATE sender_cleanups SET state=?2,last_error=NULL WHERE id=?1 AND state=?3",
            params![id, state, job.state],
        )? == 0
        {
            return Err(invalid("Cleanup changed; refresh and try again"));
        }
        read(&db, id)
    }
    pub fn finish_cleanup_batch(&self, lease: &CleanupLease) -> Result<SenderCleanup> {
        let mut db = self.lock()?;
        ensure(&db)?;
        let tx = db.transaction()?;
        check_lease(&tx, lease)?;
        if lease.job.phase != "apply" {
            return Err(invalid("Cleanup has not been confirmed"));
        }
        let mut compensated = std::collections::HashSet::new();
        for id in &lease.message_ids {
            if tx.execute("UPDATE sender_cleanup_items SET outcome='done' WHERE job_id=?1 AND message_id=?2 AND outcome='pending'",params![lease.job.id,id])?==0{continue;}
            let thread: Option<String> = tx
                .query_row(
                    "SELECT thread_id FROM messages WHERE id=?1 AND account_id=?2",
                    params![id, lease.job.account_id],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(thread) = thread {
                let newer:bool=tx.query_row("SELECT coalesce((SELECT operation_id FROM actions WHERE thread_id=?1 ORDER BY rowid DESC LIMIT 1),'')<>coalesce((SELECT action_marker FROM sender_cleanup_items WHERE job_id=?2 AND message_id=?3),'')",params![thread,lease.job.id,id],|r|r.get(0))?;
                if !newer {
                    tx.execute("UPDATE messages SET is_read=1 WHERE id=?1", [id])?;
                    refresh_thread(&tx, &thread)?;
                }
                // A concurrent worker may have confirmed unread before this slower
                // provider cleanup returned. Requeue it after cleanup, without replaying
                // unrelated archive/label changes or touching other thread members locally.
                let latest_read: Option<(String, String)> = tx.query_row(
                    "SELECT operation_id,action_json FROM actions WHERE thread_id=?1 AND json_extract(action_json,'$.type')='mark_read' ORDER BY rowid DESC LIMIT 1",
                    [&thread], |r| Ok((r.get(0)?, r.get(1)?)),
                ).optional()?;
                let marker: Option<String> = tx.query_row(
                    "SELECT read_action_marker FROM sender_cleanup_items WHERE job_id=?1 AND message_id=?2",
                    params![lease.job.id, id], |r| r.get(0),
                )?;
                if let Some((operation, raw)) = latest_read {
                    if marker.as_deref() != Some(operation.as_str())
                        && matches!(
                            serde_json::from_str::<crate::MailAction>(&raw)?,
                            crate::MailAction::MarkRead { read: false, .. }
                        )
                        && compensated.insert(thread.clone())
                    {
                        tx.execute(
                            "INSERT INTO actions(operation_id,action_json,status,thread_id) VALUES(?1,?2,'pending',?3)",
                            params![format!("cleanup:compensate:{}", uuid::Uuid::new_v4()),raw,thread],
                        )?;
                    }
                }
            }
        }
        tx.execute("UPDATE sender_cleanups SET lease=NULL,lease_until_ms=0,last_error=NULL,retry_at_ms=0,state=CASE WHEN state='running' AND NOT EXISTS(SELECT 1 FROM sender_cleanup_items WHERE job_id=?1 AND outcome='pending') THEN 'completed' ELSE state END WHERE id=?1",[&lease.job.id])?;
        tx.commit()?;
        read(&db, &lease.job.id)
    }
    pub fn cleanup_step_failed(
        &self,
        lease: &CleanupLease,
        error: &str,
        retry_ms: i64,
    ) -> Result<SenderCleanup> {
        let db = self.lock()?;
        ensure(&db)?;
        let message: String = error
            .chars()
            .filter(|c| !c.is_control())
            .take(600)
            .collect();
        if db.execute("UPDATE sender_cleanups SET lease=NULL,lease_until_ms=0,last_error=?2,retry_at_ms=?3 WHERE id=?1 AND lease=?4",params![lease.job.id,message,now_ms()+retry_ms.clamp(1000,3_600_000),lease.token])? == 0 {
            return Err(invalid("Cleanup step no longer owns its lease"));
        }
        read(&db, &lease.job.id)
    }
}
fn cleanup_confirmed(db: &Connection, id: &str) -> Result<bool> {
    Ok(db.query_row(
        "SELECT confirmed_action_rowid IS NOT NULL FROM sender_cleanups WHERE id=?1",
        [id],
        |r| r.get(0),
    )?)
}
fn check_lease(db: &Connection, lease: &CleanupLease) -> Result<()> {
    let valid: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sender_cleanups WHERE id=?1 AND lease=?2)",
        params![lease.job.id, lease.token],
        |r| r.get(0),
    )?;
    if !valid {
        return Err(invalid("Cleanup step no longer owns its lease"));
    }
    Ok(())
}
