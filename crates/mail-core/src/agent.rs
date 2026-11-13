//! Per-account consent for agents connected through the local MCP server,
//! pending approvals for agent-requested sends and bulk changes, and a bounded
//! local history of agent activity.
//!
//! Only the desktop app grants consent and decides approvals. `mail-mcp` records
//! requests and activity, but it has no code path that approves, and its access
//! policy fails closed for the decision operations. Approval applies the request
//! in the same SQLite transaction that records the decision, so a crash leaves
//! either a pending request or an applied one, never a half-applied change.
use crate::{
    MailError, MailStore, RecipientHeaders, Result, batch_triage_tx, cleanup,
    normalize_outgoing_recipients, now_ms, schedule_draft_tx, undo_triage_tx,
    validate_triage_batch,
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;

/// The disclosure the app shows now. A new consent records this version, so
/// each grant says which text the user saw. Bump it whenever that text changes.
///
/// 2: the Agent access redesign. Shorter sheet, the full list one click away,
/// same data, grants and safeguards as version 1.
pub const AGENT_DISCLOSURE_VERSION: i64 = 2;
/// The oldest disclosure whose consents still authorize MCP access. Raise it
/// (to the current version) only when a change widens what agents can read or
/// do, or weakens a safeguard: older consents then stop working until the user
/// reviews the new text. Rewording alone keeps it, so version 1 consents stay
/// valid under version 2.
pub const AGENT_DISCLOSURE_MIN_VERSION: i64 = 1;
/// Optional grants. Consent itself authorizes reading the account's cached data.
pub const AGENT_GRANTS: [&str; 5] = ["draft", "triage", "send", "sync", "calendar_write"];
/// Agent requests that never take effect until the user approves them in the app.
pub const AGENT_APPROVAL_KINDS: [&str; 6] = [
    "send_draft",
    "queue_send_draft",
    "schedule_draft",
    "batch_triage",
    "undo_triage",
    "confirm_sender_cleanup",
];
/// Undo window used when the app does not supply one; matches the composer.
pub const DEFAULT_APPROVAL_UNDO_SECONDS: u32 = 10;
const MAX_PENDING: i64 = 50;
const HISTORY_LIMIT: i64 = 500;
const PREVIEW_CHARS: usize = 600;
const SAMPLE_THREADS: usize = 25;
const MAX_CLIENT_CHARS: usize = 64;
const BLOCK_COALESCE_MS: i64 = 10 * 60 * 1000;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct AgentConsent {
    pub account_id: String,
    pub granted_at_ms: i64,
    /// Optional grants the user allowed, a subset of `AGENT_GRANTS`.
    pub grants: Vec<String>,
    pub disclosure_version: i64,
}
impl AgentConsent {
    /// Consent recorded for a disclosure that still authorizes access.
    pub fn is_current(&self) -> bool {
        self.disclosure_version >= AGENT_DISCLOSURE_MIN_VERSION
    }
    pub fn allows(&self, grant: &str) -> bool {
        self.is_current() && self.grants.iter().any(|g| g == grant)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct AgentAction {
    pub id: String,
    pub created_at_ms: i64,
    pub kind: String,
    pub account_ids: Vec<String>,
    /// `pending`, `approved`, `rejected`, `failed`, `blocked` (refused for
    /// missing consent), or `done` for activity that needed no approval and for
    /// consent changes made in the app.
    pub status: String,
    pub summary: String,
    pub details: Value,
    /// Client name reported by the MCP host. Self-reported and informational.
    pub client: Option<String>,
    pub decided_at_ms: Option<i64>,
    pub result: Option<Value>,
    pub error: Option<String>,
}

/// A request that only takes effect after approval in the desktop app.
#[derive(Debug, Clone)]
pub enum AgentApprovalRequest {
    SendDraft {
        message_id: String,
    },
    QueueSendDraft {
        message_id: String,
        undo_seconds: Option<u32>,
    },
    ScheduleDraft {
        message_id: String,
        send_at_ms: i64,
    },
    BatchTriage {
        operation_id: String,
        kind: String,
        thread_ids: Vec<String>,
    },
    /// Reversing a batch is itself a change to many conversations.
    UndoTriage {
        operation_id: String,
    },
    ConfirmSenderCleanup {
        job_id: String,
    },
}
impl AgentApprovalRequest {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::SendDraft { .. } => "send_draft",
            Self::QueueSendDraft { .. } => "queue_send_draft",
            Self::ScheduleDraft { .. } => "schedule_draft",
            Self::BatchTriage { .. } => "batch_triage",
            Self::UndoTriage { .. } => "undo_triage",
            Self::ConfirmSenderCleanup { .. } => "confirm_sender_cleanup",
        }
    }
    /// One pending request per draft, batch operation or cleanup job.
    fn target(&self) -> String {
        match self {
            Self::SendDraft { message_id }
            | Self::QueueSendDraft { message_id, .. }
            | Self::ScheduleDraft { message_id, .. } => format!("draft:{message_id}"),
            Self::BatchTriage { operation_id, .. } => format!("triage:{operation_id}"),
            Self::UndoTriage { operation_id } => format!("undo:{operation_id}"),
            Self::ConfirmSenderCleanup { job_id } => format!("cleanup:{job_id}"),
        }
    }
    fn request_json(&self) -> Value {
        match self {
            Self::SendDraft { message_id } => json!({"message_id":message_id}),
            Self::QueueSendDraft {
                message_id,
                undo_seconds,
            } => json!({"message_id":message_id,"undo_seconds":undo_seconds}),
            Self::ScheduleDraft {
                message_id,
                send_at_ms,
            } => json!({"message_id":message_id,"send_at_ms":send_at_ms}),
            Self::BatchTriage {
                operation_id,
                kind,
                thread_ids,
            } => {
                let mut ids = thread_ids.clone();
                ids.sort();
                ids.dedup();
                json!({"operation_id":operation_id,"kind":kind,"thread_ids":ids})
            }
            Self::UndoTriage { operation_id } => json!({"operation_id":operation_id}),
            Self::ConfirmSenderCleanup { job_id } => json!({"job_id":job_id}),
        }
    }
}

/// The consent grant an approval kind requires.
pub fn agent_grant_for(kind: &str) -> &'static str {
    match kind {
        "send_draft" | "queue_send_draft" | "schedule_draft" => "send",
        _ => "triage",
    }
}

pub(crate) fn initialize(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS agent_consents(
            account_id TEXT PRIMARY KEY REFERENCES accounts(id) ON DELETE CASCADE,
            granted_at_ms INTEGER NOT NULL,
            grants_json TEXT NOT NULL,
            disclosure_version INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS agent_actions(
            id TEXT PRIMARY KEY,
            created_at_ms INTEGER NOT NULL,
            kind TEXT NOT NULL,
            target TEXT,
            account_ids_json TEXT NOT NULL,
            status TEXT NOT NULL,
            summary TEXT NOT NULL,
            details_json TEXT NOT NULL,
            client TEXT,
            decided_at_ms INTEGER,
            result_json TEXT,
            error TEXT
        );
        CREATE INDEX IF NOT EXISTS agent_actions_recent ON agent_actions(status,created_at_ms);
        CREATE UNIQUE INDEX IF NOT EXISTS agent_actions_one_pending ON agent_actions(target) WHERE status='pending';",
    )?;
    Ok(())
}

fn invalid(message: impl Into<String>) -> MailError {
    MailError::InvalidAction(message.into())
}

fn read_consent(db: &Connection, account_id: &str) -> Result<Option<AgentConsent>> {
    db.query_row(
        "SELECT account_id,granted_at_ms,grants_json,disclosure_version FROM agent_consents WHERE account_id=?1",
        [account_id],
        |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
            ))
        },
    )
    .optional()?
    .map(|(account_id, granted_at_ms, grants, disclosure_version)| {
        Ok(AgentConsent {
            account_id,
            granted_at_ms,
            grants: serde_json::from_str(&grants)?,
            disclosure_version,
        })
    })
    .transpose()
}

const ACTION_COLUMNS: &str = "id,created_at_ms,kind,account_ids_json,status,summary,details_json,client,decided_at_ms,result_json,error";

fn action_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<RawAction> {
    Ok(RawAction {
        id: r.get(0)?,
        created_at_ms: r.get(1)?,
        kind: r.get(2)?,
        account_ids: r.get(3)?,
        status: r.get(4)?,
        summary: r.get(5)?,
        details: r.get(6)?,
        client: r.get(7)?,
        decided_at_ms: r.get(8)?,
        result: r.get(9)?,
        error: r.get(10)?,
    })
}
struct RawAction {
    id: String,
    created_at_ms: i64,
    kind: String,
    account_ids: String,
    status: String,
    summary: String,
    details: String,
    client: Option<String>,
    decided_at_ms: Option<i64>,
    result: Option<String>,
    error: Option<String>,
}
impl RawAction {
    fn decode(self) -> Result<AgentAction> {
        Ok(AgentAction {
            id: self.id,
            created_at_ms: self.created_at_ms,
            kind: self.kind,
            account_ids: serde_json::from_str(&self.account_ids)?,
            status: self.status,
            summary: self.summary,
            details: serde_json::from_str(&self.details)?,
            client: self.client,
            decided_at_ms: self.decided_at_ms,
            result: self.result.as_deref().map(serde_json::from_str).transpose()?,
            error: self.error,
        })
    }
}

fn read_action(db: &Connection, id: &str) -> Result<Option<AgentAction>> {
    db.query_row(
        &format!("SELECT {ACTION_COLUMNS} FROM agent_actions WHERE id=?1"),
        [id],
        action_row,
    )
    .optional()?
    .map(RawAction::decode)
    .transpose()
}

fn latest_for_target(db: &Connection, target: &str) -> Result<Option<AgentAction>> {
    db.query_row(
        &format!(
            "SELECT {ACTION_COLUMNS} FROM agent_actions WHERE target=?1 ORDER BY (status='pending') DESC,created_at_ms DESC,rowid DESC LIMIT 1"
        ),
        [target],
        action_row,
    )
    .optional()?
    .map(RawAction::decode)
    .transpose()
}

fn account_email(db: &Connection, account_id: &str) -> Result<String> {
    db.query_row(
        "SELECT email FROM accounts WHERE id=?1",
        [account_id],
        |r| r.get(0),
    )
    .optional()?
    .ok_or_else(|| MailError::UnknownAccount(account_id.into()))
}

fn sanitize_client(client: Option<&str>) -> Option<String> {
    let clean: String = client?
        .chars()
        .filter(|c| !deceptive(*c))
        .take(MAX_CLIENT_CHARS)
        .collect();
    let clean = clean.trim().to_string();
    (!clean.is_empty()).then_some(clean)
}

/// Characters that can fake a line break or reverse text on the card: control
/// characters, Unicode line and paragraph separators, and bidi controls.
fn deceptive(c: char) -> bool {
    c.is_control()
        || matches!(c, '\u{2028}' | '\u{2029}' | '\u{200e}' | '\u{200f}' | '\u{061c}')
        || ('\u{202a}'..='\u{202e}').contains(&c)
        || ('\u{2066}'..='\u{2069}').contains(&c)
}

/// Unicode Default_Ignorable_Code_Point (DerivedCoreProperties.txt) plus the
/// other format characters (Cf) that render as nothing.
fn invisible(c: char) -> bool {
    matches!(c as u32,
        0x00AD | 0x034F | 0x061C | 0x115F..=0x1160 | 0x17B4..=0x17B5 | 0x180B..=0x180F
        | 0x200B..=0x200F | 0x202A..=0x202E | 0x2060..=0x206F | 0x3164 | 0xFE00..=0xFE0F
        | 0xFEFF | 0xFFA0 | 0xFFF0..=0xFFFB | 0x1BCA0..=0x1BCA3 | 0x1D173..=0x1D17A
        | 0xE0000..=0xE0FFF
        // Other Cf characters.
        | 0x0600..=0x0605 | 0x06DD | 0x070F | 0x0890..=0x0891 | 0x08E2 | 0x110BD | 0x110CD
        | 0x13430..=0x1345F)
}

/// Characters with an emoji presentation, including those that take U+FE0F.
fn pictographic(c: char) -> bool {
    matches!(c as u32,
        0x00A9 | 0x00AE | 0x203C | 0x2049 | 0x2122 | 0x2139 | 0x2194..=0x2199 | 0x21A9..=0x21AA
        | 0x231A..=0x231B | 0x2328 | 0x23CF | 0x23E9..=0x23F3 | 0x23F8..=0x23FA | 0x24C2
        | 0x25AA..=0x25AB | 0x25B6 | 0x25C0 | 0x25FB..=0x25FE | 0x2600..=0x27BF | 0x2934..=0x2935
        | 0x2B05..=0x2B07 | 0x2B1B..=0x2B1C | 0x2B50 | 0x2B55 | 0x3030 | 0x303D | 0x3297 | 0x3299
        | 0x1F000..=0x1FAFF)
}

fn emoji_modifier(c: char) -> bool {
    matches!(c as u32, 0x1F3FB..=0x1F3FF)
}

/// Indic viramas (and Sinhala al-lakuna) that form conjuncts with a joiner.
fn virama(c: char) -> bool {
    matches!(c as u32,
        0x094D | 0x09CD | 0x0A4D | 0x0ACD | 0x0B4D | 0x0BCD | 0x0C4D | 0x0CCD | 0x0D4D | 0x0DCA
        | 0x1039 | 0x17D2 | 0x1B44 | 0xA8C4)
}

fn arabic_letter(c: char) -> bool {
    matches!(c as u32, 0x0620..=0x064A | 0x066E..=0x06D3 | 0x06FA..=0x06FF | 0x0750..=0x077F | 0x08A0..=0x08C9 | 0xFB50..=0xFDFF | 0xFE70..=0xFEFC)
}

/// Checks text that is sent after approval. Returns the first character the
/// card could not show, and otherwise the number of invisible characters
/// allowed in context, which the card reports. Allowed in context: variation
/// selectors after characters that have emoji presentation, the keycap
/// sequence, joiners inside emoji sequences or next to an Indic virama,
/// non-joiners between Arabic-script letters or next to a virama, and tags in
/// a subdivision flag. Line breaks and tabs are allowed where `multiline`.
pub(crate) fn hidden_characters(text: &str, multiline: bool) -> std::result::Result<usize, char> {
    let chars: Vec<char> = text.chars().collect();
    let mut allowed = 0;
    let mut index = 0;
    while index < chars.len() {
        let c = chars[index];
        let before = index.checked_sub(1).map(|i| chars[i]);
        let after = chars.get(index + 1).copied();
        // The visible character a joiner or selector attaches to, skipping a
        // selector or skin-tone modifier.
        let base = chars[..index]
            .iter()
            .rev()
            .find(|b| !matches!(**b as u32, 0xFE0E | 0xFE0F) && !emoji_modifier(**b))
            .copied();
        let in_context = match c {
            '\u{FE0E}' | '\u{FE0F}' => {
                before.is_some_and(pictographic)
                    || (before.is_some_and(|b| b.is_ascii_digit() || b == '#' || b == '*')
                        && after == Some('\u{20E3}'))
            }
            '\u{200D}' => {
                (base.is_some_and(pictographic) && after.is_some_and(pictographic))
                    || before.is_some_and(virama)
                    || after.is_some_and(virama)
            }
            '\u{200C}' => {
                (before.is_some_and(arabic_letter) && after.is_some_and(arabic_letter))
                    || before.is_some_and(virama)
                    || after.is_some_and(virama)
            }
            '\u{E0020}'..='\u{E007F}' => {
                // A subdivision flag: U+1F3F4, tag letters, then CANCEL TAG.
                let start = chars[..index].iter().rposition(|b| !('\u{E0020}'..='\u{E007E}').contains(b));
                let opened = start.is_some_and(|i| chars[i] == '\u{1F3F4}');
                let closed = chars[index..].iter().find(|b| !('\u{E0020}'..='\u{E007E}').contains(*b)) == Some(&'\u{E007F}');
                opened && (c == '\u{E007F}' || closed)
            }
            _ => false,
        };
        if in_context {
            allowed += 1;
        } else if !(multiline && matches!(c, '\n' | '\r' | '\t')) && (invisible(c) || deceptive(c)) {
            return Err(c);
        }
        index += 1;
    }
    Ok(allowed)
}

fn hidden_error(field: &str, c: char) -> MailError {
    invalid(format!(
        "The draft {field} contains a character Email cannot show for review (U+{:04X}); remove it first",
        c as u32
    ))
}

fn truncate_chars(value: &str, limit: usize) -> String {
    let mut chars = value.chars();
    let head: String = chars.by_ref().take(limit).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

/// A quoted, bounded subject for history text, without characters that can
/// fake a line break or reverse the text.
pub fn agent_quoted_subject(subject: &str) -> String {
    let subject: String = subject.chars().filter(|c| !deceptive(*c)).collect();
    let subject = subject.trim();
    if subject.is_empty() {
        "(no subject)".into()
    } else {
        format!("“{}”", truncate_chars(subject, 80))
    }
}


fn recipient_label(addresses: &[String]) -> String {
    match addresses.len() {
        0 => "no recipients".into(),
        1..=3 => addresses.join(", "),
        n => format!("{} and {} more", addresses[..2].join(", "), n - 2),
    }
}

/// The short name both shells, the account summary ("Read · Drafts · Ask to
/// send"), the history and MCP refusals use for a grant. Each is the start of
/// the matching checkbox in the turn-on sheet, so an agent's "ask the user to
/// allow Ask to send" points at a box the user can find.
pub fn agent_grant_label(grant: &str) -> &'static str {
    match grant {
        "draft" => "Drafts",
        "triage" => "Archive and snooze",
        "send" => "Ask to send",
        "sync" => "Fetch mail",
        "calendar_write" => "Calendar changes",
        _ => "this permission",
    }
}

/// Stable 64-bit FNV-1a digest of a request, so a decided request can drop
/// its large fields and still recognise an identical retry.
fn digest(value: &Value) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in value.to_string().bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// Snapshot of exactly what would be sent. Approval compares it again, so the
/// user approves the content they saw and nothing edited afterwards.
fn draft_snapshot(db: &Connection, message_id: &str) -> Result<Value> {
    type DraftRow = (
        String,
        String,
        String,
        Option<String>,
        String,
        String,
        Option<String>,
        bool,
    );
    let row: Option<DraftRow> = db
        .query_row(
            "SELECT account_id,sender,to_json,recipient_headers_json,subject,body_plain,body_html,is_draft FROM messages WHERE id=?1",
            [message_id],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                ))
            },
        )
        .optional()?;
    let Some((account_id, sender, to, headers, subject, body_plain, body_html, is_draft)) = row
    else {
        return Err(invalid("message is not an unsent draft"));
    };
    if !is_draft {
        return Err(invalid("message is not an unsent draft"));
    }
    if db.query_row(
        "SELECT EXISTS(SELECT 1 FROM outbox WHERE message_id=?1)",
        [message_id],
        |r| r.get::<_, bool>(0),
    )? {
        return Err(invalid("draft is already queued for sending"));
    }
    let to: Vec<String> = serde_json::from_str(&to)?;
    let headers: RecipientHeaders = headers
        .as_deref()
        .map(serde_json::from_str)
        .transpose()?
        .unwrap_or_default();
    let mut query = db.prepare(
        "SELECT id,filename,mime_type,size FROM draft_attachments WHERE message_id=?1 ORDER BY rowid",
    )?;
    let attachments = query
        .query_map([message_id], |r| {
            Ok(json!({"id":r.get::<_,String>(0)?,"filename":r.get::<_,String>(1)?,"mime_type":r.get::<_,String>(2)?,"size":r.get::<_,i64>(3)?}))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(json!({
        "message_id": message_id,
        "account_id": account_id,
        "from": sender,
        "to": to,
        "cc": headers.cc,
        "bcc": headers.bcc,
        "reply_to": headers.reply_to,
        "subject": subject,
        "body_plain": body_plain,
        "body_html": body_html,
        "attachments": attachments,
    }))
}

/// Bare address and display name of each formatted recipient, for display.
fn address_parts(list: &[String]) -> Vec<Value> {
    let part = |address: &str, name: Option<String>| {
        let name = name
            .map(|n| n.chars().filter(|c| !deceptive(*c)).collect::<String>())
            .filter(|n| !n.trim().is_empty());
        json!({"address": address.trim(), "name": name})
    };
    let mut parts = Vec::new();
    for value in list.iter().filter(|v| !v.trim().is_empty()) {
        match mailparse::addrparse(value) {
            Ok(parsed) => {
                for single in parsed.iter().flat_map(|item| match item {
                    mailparse::MailAddr::Single(single) => vec![single.clone()],
                    mailparse::MailAddr::Group(group) => group.addrs.clone(),
                }) {
                    parts.push(part(&single.addr, single.display_name));
                }
            }
            // Never show a formatted string as if it were a bare address.
            Err(_) => match (value.rfind('<'), value.rfind('>')) {
                (Some(open), Some(close)) if open < close => parts.push(part(
                    &value[open + 1..close],
                    Some(value[..open].trim().trim_matches('"').to_string()),
                )),
                _ => parts.push(part(value, None)),
            },
        }
    }
    parts
}

fn triage_label(kind: &str, count: usize) -> String {
    let noun = if count == 1 {
        "conversation"
    } else {
        "conversations"
    };
    match kind {
        "archive" => format!("Archive {count} {noun}"),
        "restore" => format!("Move {count} {noun} back to the inbox"),
        "spam" => format!("Move {count} {noun} to Spam"),
        "read" => format!("Mark {count} {noun} read"),
        _ => format!("Mark {count} {noun} unread"),
    }
}

fn undo_label(kind: &str, count: usize) -> String {
    let noun = if count == 1 { "conversation" } else { "conversations" };
    match kind {
        "archive" => format!("Undo archiving {count} {noun}"),
        "restore" => format!("Undo moving {count} {noun} back to the inbox"),
        _ => format!("Undo moving {count} {noun} to Spam"),
    }
}

/// Accounts, per-account counts and a sample of a frozen conversation selection.
fn selection_details(db: &Connection, ids: &[String]) -> Result<(Vec<String>, Value)> {
    let mut counts = std::collections::BTreeMap::<String, usize>::new();
    let mut sample = Vec::new();
    let mut statement = db.prepare("SELECT account_id,subject,sender FROM threads WHERE id=?1")?;
    for id in ids {
        let (account, subject, sender): (String, String, String) = statement
            .query_row([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .optional()?
            .ok_or_else(|| invalid("A selected conversation is not in the local cache"))?;
        if sample.len() < SAMPLE_THREADS {
            sample.push(json!({"id":id,"account_id":account,"subject":subject,"sender":sender}));
        }
        *counts.entry(account).or_default() += 1;
    }
    let account_ids: Vec<String> = counts.keys().cloned().collect();
    let emails = account_ids
        .iter()
        .map(|id| account_email(db, id))
        .collect::<Result<Vec<_>>>()?;
    Ok((
        account_ids,
        json!({"count": ids.len(), "account_counts": counts, "threads": sample, "account_emails": emails}),
    ))
}

/// Fail as an invalid request, not a database error, when a selected
/// conversation disappeared (for example after a sync) before approval.
fn require_threads(db: &Connection, ids: &[String]) -> Result<()> {
    let mut statement = db.prepare("SELECT EXISTS(SELECT 1 FROM threads WHERE id=?1)")?;
    for id in ids {
        if !statement.query_row([id], |r| r.get::<_, bool>(0))? {
            return Err(invalid("a selected conversation is no longer in the local cache"));
        }
    }
    Ok(())
}

/// After a decision, keep only a preview of the content for the history, and
/// drop a batch's full selection (its sample and count remain).
fn minimize(kind: &str, mut details: Value) -> Value {
    if kind == "batch_triage"
        && let Some(request) = details.get_mut("request").and_then(Value::as_object_mut)
    {
        request.remove("thread_ids");
    }
    if matches!(kind, "send_draft" | "queue_send_draft" | "schedule_draft")
        && let Some(draft) = details.get_mut("draft").and_then(Value::as_object_mut)
    {
        if let Some(body) = draft.get("body_plain").and_then(Value::as_str) {
            let preview = truncate_chars(body, PREVIEW_CHARS);
            draft.insert("body_plain".into(), Value::String(preview));
        }
        draft.insert("body_html".into(), Value::Null);
    }
    details
}

/// Decisions and consent changes are kept separately from routine activity
/// and refusals, so a busy or refused agent cannot push them out.
fn trim_history(db: &Connection) -> Result<()> {
    // Protected: the user's own decisions and consent changes. Requests the
    // app closed by itself (marked closed_by "system") are routine, so an
    // agent cannot push decisions out by making its requests go stale.
    const DECISION: &str = "(status IN ('approved','rejected') OR (status='failed' AND coalesce(json_extract(details_json,'$.closed_by'),'')!='system') OR kind LIKE 'consent_%')";
    // Scan only once the history clearly exceeds both limits; each group
    // still keeps at least its newest HISTORY_LIMIT entries.
    let decided: i64 = db.query_row("SELECT count(*) FROM agent_actions WHERE status!='pending'", [], |r| r.get(0))?;
    if decided <= 2 * HISTORY_LIMIT + 50 {
        return Ok(());
    }
    for class in [
        DECISION.to_string(),
        format!("(status!='pending' AND NOT {DECISION})"),
    ] {
        db.execute(
            &format!(
                // Newest by decision time, so a request decided now is never
                // the one trimmed, however long it waited.
                "DELETE FROM agent_actions WHERE {class} AND id NOT IN (SELECT id FROM agent_actions WHERE {class} ORDER BY coalesce(decided_at_ms,created_at_ms) DESC,rowid DESC LIMIT ?1)"
            ),
            [HISTORY_LIMIT],
        )?;
    }
    Ok(())
}

fn insert_action(
    db: &Connection,
    kind: &str,
    target: Option<&str>,
    account_ids: &[String],
    status: &str,
    summary: &str,
    details: &Value,
    client: Option<&str>,
) -> Result<String> {
    let id = uuid::Uuid::new_v4().to_string();
    let decided = (status != "pending").then(now_ms);
    db.execute(
        "INSERT INTO agent_actions(id,created_at_ms,kind,target,account_ids_json,status,summary,details_json,client,decided_at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
        params![
            id,
            now_ms(),
            kind,
            target,
            serde_json::to_string(account_ids)?,
            status,
            truncate_chars(summary, 300),
            serde_json::to_string(details)?,
            sanitize_client(client),
            decided,
        ],
    )?;
    trim_history(db)?;
    Ok(id)
}

/// Close a still-pending request. Returns false when it was already decided.
fn close_pending(db: &Connection, id: &str, status: &str, error: Option<&str>) -> Result<bool> {
    close_pending_by(db, id, status, error, false)
}

/// As `close_pending`; `system` marks a closure the app made without the
/// user, which counts as routine history.
fn close_pending_by(db: &Connection, id: &str, status: &str, error: Option<&str>, system: bool) -> Result<bool> {
    let Some(action) = read_action(db, id)? else {
        return Ok(false);
    };
    if action.status != "pending" {
        return Ok(false);
    }
    let mut details = minimize(&action.kind, action.details);
    if system {
        details["closed_by"] = json!("system");
    }
    let changed = db.execute(
        "UPDATE agent_actions SET status=?2,error=?3,decided_at_ms=?4,details_json=?5 WHERE id=?1 AND status='pending'",
        params![id, status, error, now_ms(), serde_json::to_string(&details)?],
    )?;
    trim_history(db)?;
    Ok(changed == 1)
}

/// Close waiting requests whose target changed outside the request: a draft
/// sent, scheduled or discarded; a cleanup no longer ready; a batch whose
/// conversations left the cache; an undo already done or no longer possible.
fn close_stale_send_requests(db: &Connection) -> Result<()> {
    for (id, reason) in stale_requests(db)? {
        close_pending_by(db, &id, "failed", Some(reason), true)?;
    }
    Ok(())
}

/// Waiting requests whose target changed outside the request, read-only.
fn stale_requests(db: &Connection) -> Result<Vec<(String, &'static str)>> {
    let mut stale = Vec::new();
    let mut query = db.prepare(
        "SELECT id,kind,json_extract(details_json,'$.request') FROM agent_actions WHERE status='pending'",
    )?;
    let rows = query
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?)))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    drop(query);
    for (id, kind, request) in rows {
        let request: Value = request.as_deref().map(serde_json::from_str).transpose()?.unwrap_or(Value::Null);
        let text = |key: &str| request[key].as_str().unwrap_or_default().to_string();
        let reason = match kind.as_str() {
            "send_draft" | "queue_send_draft" | "schedule_draft" => (!db.query_row(
                "SELECT EXISTS(SELECT 1 FROM messages WHERE id=?1 AND is_draft=1) AND NOT EXISTS(SELECT 1 FROM outbox WHERE message_id=?1)",
                [text("message_id")],
                |r| r.get::<_, bool>(0),
            )?)
            .then_some("The draft was sent, scheduled or discarded outside this request"),
            "confirm_sender_cleanup" => {
                let ready = cleanup::read_job(db, &text("job_id"))
                    .map(|job| job.state == "ready" && job.phase == "apply")
                    .unwrap_or(false);
                (!ready).then_some("The cleanup changed outside this request")
            }
            "batch_triage" => (db.query_row(
                "SELECT count(*) FROM json_each(?1) j WHERE NOT EXISTS(SELECT 1 FROM threads WHERE id=j.value)",
                [request["thread_ids"].to_string()],
                |r| r.get::<_, i64>(0),
            )? > 0)
            .then_some("A selected conversation is no longer in the local cache"),
            "undo_triage" => (!db.query_row(
                "SELECT EXISTS(SELECT 1 FROM triage_batches WHERE operation_id=?1 AND undone=0)",
                [text("operation_id")],
                |r| r.get::<_, bool>(0),
            )?)
            .then_some("The change was already undone or is no longer available"),
            _ => None,
        };
        if let Some(reason) = reason {
            stale.push((id, reason));
        }
    }
    Ok(stale)
}

/// The newest refusal that still needs action, with the accounts every
/// recent unresolved refusal affects. Ordered by the last attempt, so an agent
/// that keeps retrying keeps the prompt visible.
fn blocked_summary(db: &Connection) -> Result<Option<Value>> {
    let mut query = db.prepare(
        "SELECT id,summary,created_at_ms,account_ids_json,error,coalesce(json_extract(details_json,'$.last_at_ms'),created_at_ms) AS last_at FROM agent_actions WHERE status='blocked' AND coalesce(json_extract(details_json,'$.last_at_ms'),created_at_ms)>=?1 ORDER BY last_at DESC,rowid DESC LIMIT 50",
    )?;
    let rows = query
        .query_map([now_ms() - 86_400_000], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, i64>(5)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    drop(query);
    if rows.is_empty() {
        return Ok(None);
    }
    // Current consents, read once: account → granted_at.
    let consents: std::collections::HashMap<String, i64> = {
        let mut query = db.prepare(
            "SELECT account_id,granted_at_ms FROM agent_consents WHERE disclosure_version>=?1",
        )?;
        query
            .query_map([AGENT_DISCLOSURE_MIN_VERSION], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<std::result::Result<_, _>>()?
    };
    // Accounts still off take priority over missing grants, so the banner's
    // text always matches the action it offers.
    let mut off = (None, BTreeSet::new());
    let mut grant = (None, BTreeSet::new());
    for (id, summary, created_at_ms, accounts, error, last_at_ms) in rows {
        let ids: Vec<String> = serde_json::from_str(&accounts)?;
        let mut row_off = BTreeSet::new();
        let mut row_grant = BTreeSet::new();
        for account in &ids {
            match consents.get(account) {
                Some(granted_at) if *granted_at < last_at_ms => {
                    row_grant.insert(account.clone());
                }
                Some(_) => {}
                None => {
                    row_off.insert(account.clone());
                }
            }
        }
        if ids.is_empty() && consents.is_empty() {
            let mut all = db.prepare("SELECT id FROM accounts ORDER BY email")?;
            row_off = all
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<std::result::Result<_, _>>()?;
        }
        let entry = json!({"id":id,"summary":summary,"created_at_ms":created_at_ms,"last_at_ms":last_at_ms,"account_ids":ids,"reason":error});
        if !row_off.is_empty() {
            off.0.get_or_insert(entry.clone());
            off.1.extend(row_off);
        }
        if !row_grant.is_empty() {
            grant.0.get_or_insert(entry);
            grant.1.extend(row_grant);
        }
    }
    let (entry, affected, needs_access) = match (off, grant) {
        ((Some(entry), affected), _) => (entry, affected, true),
        (_, (Some(entry), affected)) => (entry, affected, false),
        _ => return Ok(None),
    };
    let mut entry = entry;
    entry["affected_account_ids"] = json!(affected.into_iter().collect::<Vec<_>>());
    entry["needs_access"] = json!(needs_access);
    Ok(Some(entry))
}

fn pending_ids_for_account(db: &Connection, account_id: &str) -> Result<Vec<(String, String)>> {
    let mut query = db.prepare(
        "SELECT id,kind FROM agent_actions WHERE status='pending' AND EXISTS(SELECT 1 FROM json_each(agent_actions.account_ids_json) WHERE value=?1)",
    )?;
    Ok(query
        .query_map([account_id], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<std::result::Result<_, _>>()?)
}

impl MailStore {
    /// All recorded consents, including ones for an older disclosure version.
    pub fn agent_consents(&self) -> Result<Vec<AgentConsent>> {
        let db = self.lock()?;
        let mut query = db.prepare(
            "SELECT account_id,granted_at_ms,grants_json,disclosure_version FROM agent_consents ORDER BY account_id",
        )?;
        let rows = query
            .query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(|(account_id, granted_at_ms, grants, disclosure_version)| {
                Ok(AgentConsent { account_id, granted_at_ms, grants: serde_json::from_str(&grants)?, disclosure_version })
            })
            .collect()
    }

    /// Record the user's consent after they reviewed the disclosure in the app.
    /// Narrowing the grants withdraws pending requests they no longer cover.
    pub fn grant_agent_access(
        &self,
        account_id: &str,
        grants: &[String],
        disclosure_version: i64,
    ) -> Result<AgentConsent> {
        if disclosure_version != AGENT_DISCLOSURE_VERSION {
            return Err(invalid(
                "Review the current agent access disclosure before turning access on",
            ));
        }
        let requested: BTreeSet<&str> = grants.iter().map(String::as_str).collect();
        if let Some(unknown) = requested.iter().find(|g| !AGENT_GRANTS.contains(g)) {
            return Err(invalid(format!("Unknown agent access grant: {unknown}")));
        }
        let grants: Vec<String> = AGENT_GRANTS
            .iter()
            .filter(|g| requested.contains(*g))
            .map(|g| g.to_string())
            .collect();
        let mut db = self.lock()?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let email = account_email(&tx, account_id)?;
        let previous = read_consent(&tx, account_id)?;
        let now = now_ms();
        tx.execute(
            "INSERT INTO agent_consents(account_id,granted_at_ms,grants_json,disclosure_version) VALUES(?1,?2,?3,?4) ON CONFLICT(account_id) DO UPDATE SET granted_at_ms=excluded.granted_at_ms,grants_json=excluded.grants_json,disclosure_version=excluded.disclosure_version",
            params![account_id, now, serde_json::to_string(&grants)?, disclosure_version],
        )?;
        for (id, kind) in pending_ids_for_account(&tx, account_id)? {
            if !grants.iter().any(|g| g == agent_grant_for(&kind)) {
                close_pending(
                    &tx,
                    &id,
                    "rejected",
                    Some("The agent access grant for this request was turned off"),
                )?;
            }
        }
        let labels = std::iter::once("Read")
            .chain(grants.iter().map(|g| agent_grant_label(g)))
            .collect::<Vec<_>>()
            .join(", ");
        let summary = if previous.is_some() {
            format!("Agent access changed for {email}: {labels}")
        } else {
            format!("Agent access turned on for {email}: {labels}")
        };
        insert_action(
            &tx,
            if previous.is_some() {
                "consent_changed"
            } else {
                "consent_granted"
            },
            None,
            &[account_id.to_string()],
            "done",
            &summary,
            &json!({"grants":grants,"disclosure_version":disclosure_version,"account_email":email}),
            None,
        )?;
        tx.commit()?;
        Ok(AgentConsent {
            account_id: account_id.into(),
            granted_at_ms: now,
            grants,
            disclosure_version,
        })
    }

    /// Turn agent access off. Pending requests for the account are rejected.
    /// Mail the user already approved stays in the Outbox, where it can be cancelled.
    pub fn revoke_agent_access(&self, account_id: &str) -> Result<bool> {
        let mut db = self.lock()?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let email = account_email(&tx, account_id)?;
        let removed = tx.execute(
            "DELETE FROM agent_consents WHERE account_id=?1",
            [account_id],
        )? > 0;
        let mut withdrawn = 0;
        for (id, _) in pending_ids_for_account(&tx, account_id)? {
            if close_pending(&tx, &id, "rejected", Some("Agent access was turned off"))? {
                withdrawn += 1;
            }
        }
        if removed {
            insert_action(
                &tx,
                "consent_revoked",
                None,
                &[account_id.to_string()],
                "done",
                &format!("Agent access turned off for {email}"),
                &json!({"withdrawn_requests":withdrawn,"account_email":email}),
                None,
            )?;
        }
        tx.commit()?;
        Ok(removed)
    }

    /// Record an agent request that waits for the user's decision in the app.
    /// Identical retries return the existing request instead of a duplicate.
    pub fn request_agent_approval(
        &self,
        request: &AgentApprovalRequest,
        client: Option<&str>,
    ) -> Result<AgentAction> {
        let mut db = self.lock()?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let kind = request.kind();
        let target = request.target();
        let request_json = request.request_json();
        let request_digest = digest(&request_json);
        if let Some(existing) = latest_for_target(&tx, &target)? {
            let same = existing.kind == kind
                && existing.details["request_digest"].as_str() == Some(request_digest.as_str());
            if existing.status == "pending" {
                // An agent that edited the draft and asks again replaces the
                // request, so the card never shows content that is not sent.
                let stale_draft = same
                    && matches!(kind, "send_draft" | "queue_send_draft" | "schedule_draft")
                    && request_json["message_id"]
                        .as_str()
                        .and_then(|id| draft_snapshot(&tx, id).ok())
                        .is_some_and(|draft| draft != existing.details["draft"]);
                if stale_draft {
                    // Kept as "replaced" (routine history, not a decision), so an
                    // open card or the agent's approval_id gets a clear status.
                    close_pending(
                        &tx,
                        &existing.id,
                        "replaced",
                        Some("Replaced by a newer request after the agent changed the draft; follow the newer approval_id"),
                    )?;
                } else if same {
                    return Ok(existing);
                } else {
                return Err(invalid(
                    "Another agent request for this item is already waiting for approval in Email",
                ));
                }
            }
            if same
                && existing.status == "approved"
                && matches!(kind, "batch_triage" | "undo_triage" | "confirm_sender_cleanup")
            {
                return Ok(existing);
            }
        }
        let pending: i64 = tx.query_row(
            "SELECT count(*) FROM agent_actions WHERE status='pending'",
            [],
            |r| r.get(0),
        )?;
        if pending >= MAX_PENDING {
            return Err(invalid(
                "Too many agent requests are waiting for approval in Email; ask the user to review them first",
            ));
        }
        let (account_ids, summary, mut details) = match request {
            AgentApprovalRequest::SendDraft { message_id }
            | AgentApprovalRequest::QueueSendDraft { message_id, .. }
            | AgentApprovalRequest::ScheduleDraft { message_id, .. } => {
                let draft = draft_snapshot(&tx, message_id)?;
                // The approval card shows plain text. Never ask the user to
                // approve formatted content they cannot see.
                if draft["body_html"].as_str().is_some_and(|html| !html.trim().is_empty()) {
                    return Err(invalid(
                        "This draft has formatted (HTML) content that Email cannot show for review. Ask the user to send it from Email",
                    ));
                }
                let to: Vec<String> = serde_json::from_value(draft["to"].clone())?;
                let headers = RecipientHeaders {
                    cc: serde_json::from_value(draft["cc"].clone())?,
                    bcc: serde_json::from_value(draft["bcc"].clone())?,
                    reply_to: serde_json::from_value(draft["reply_to"].clone())?,
                };
                let (to, headers) = normalize_outgoing_recipients(&to, Some(&headers))?;
                // Everything that is sent must be visible on the card, so text
                // that hides characters is refused before the user sees it.
                // Subjects allow tabs (header unfolding) but not line breaks.
                let raw_subject = draft["subject"].as_str().unwrap_or_default();
                let mut invisible_count = hidden_characters(&raw_subject.replace('\t', " "), false)
                    .map_err(|c| hidden_error("subject", c))?;
                invisible_count += hidden_characters(draft["body_plain"].as_str().unwrap_or_default(), true)
                    .map_err(|c| hidden_error("body", c))?;
                for file in draft["attachments"].as_array().into_iter().flatten() {
                    for key in ["filename", "mime_type"] {
                        invisible_count += hidden_characters(file[key].as_str().unwrap_or_default(), false)
                            .map_err(|c| hidden_error("attachment", c))?;
                    }
                }
                let account_id = draft["account_id"].as_str().unwrap_or_default().to_string();
                let reply_to: Vec<String> = serde_json::from_value(draft["reply_to"].clone())?;
                for value in to.iter().chain(&headers.cc).chain(&headers.bcc).chain(&reply_to) {
                    invisible_count +=
                        hidden_characters(value, false).map_err(|c| hidden_error("recipients", c))?;
                }
                let addresses = json!({
                    "to": address_parts(&to),
                    "cc": address_parts(&headers.cc),
                    "bcc": address_parts(&headers.bcc),
                    "reply_to": address_parts(&reply_to),
                });
                // Summaries name bare addresses; display names can imitate addresses.
                let everyone: Vec<String> = ["to", "cc", "bcc"]
                    .iter()
                    .flat_map(|key| addresses[key].as_array().cloned().unwrap_or_default())
                    .filter_map(|part| part["address"].as_str().map(str::to_owned))
                    .collect();
                let subject = agent_quoted_subject(raw_subject);
                let mut details = json!({
                    "draft": draft,
                    "invisible_count": invisible_count,
                    "addresses": addresses,
                    "account_email": account_email(&tx, &account_id)?,
                });
                let verb = match request {
                    AgentApprovalRequest::ScheduleDraft { send_at_ms, .. } => {
                        if *send_at_ms <= now_ms() {
                            return Err(invalid("send time must be in the future"));
                        }
                        details["send_at_ms"] = json!(send_at_ms);
                        "Schedule"
                    }
                    AgentApprovalRequest::QueueSendDraft { undo_seconds, .. } => {
                        details["undo_seconds"] = json!(undo_seconds
                            .unwrap_or(DEFAULT_APPROVAL_UNDO_SECONDS)
                            .clamp(1, 60));
                        "Send"
                    }
                    _ => "Send",
                };
                let summary = format!("{verb} {subject} to {}", recipient_label(&everyone));
                (vec![account_id], summary, details)
            }
            AgentApprovalRequest::BatchTriage {
                operation_id,
                kind: triage_kind,
                ..
            } => {
                let ids: Vec<String> =
                    serde_json::from_value(request_json["thread_ids"].clone())?;
                let ids = validate_triage_batch(operation_id, triage_kind, &ids)?;
                if tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM triage_batches WHERE operation_id=?1)",
                    [operation_id],
                    |r| r.get::<_, bool>(0),
                )? {
                    return Err(invalid(
                        "Operation ID was already used for another change; use a new operation_id",
                    ));
                }
                let (account_ids, mut details) = selection_details(&tx, &ids)?;
                details["operation_id"] = json!(operation_id);
                details["kind"] = json!(triage_kind);
                (account_ids, triage_label(triage_kind, ids.len()), details)
            }
            AgentApprovalRequest::UndoTriage { operation_id } => {
                let batch: Option<(String, String, bool)> = tx
                    .query_row(
                        "SELECT request_json,inverse_json,undone FROM triage_batches WHERE operation_id=?1",
                        [operation_id],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                    )
                    .optional()?;
                let Some((batch, inverse, undone)) = batch else {
                    return Err(invalid("The requested undo batch is unavailable"));
                };
                if undone {
                    return Err(invalid("This change was already undone"));
                }
                if inverse == "[]" {
                    return Err(invalid("This action has no undo"));
                }
                let (original, ids): (String, Vec<String>) = serde_json::from_str(&batch)?;
                let (account_ids, mut details) = selection_details(&tx, &ids)?;
                details["operation_id"] = json!(operation_id);
                details["kind"] = json!(original);
                (account_ids, undo_label(&original, ids.len()), details)
            }
            AgentApprovalRequest::ConfirmSenderCleanup { job_id } => {
                let job = cleanup::read_job(&tx, job_id)?;
                if job.state != "ready" || job.phase != "apply" {
                    return Err(invalid("Wait for the cleanup preview before confirming"));
                }
                let noun = if job.matched == 1 { "message" } else { "messages" };
                let summary = format!(
                    "Mark {} unread {noun} from {} read",
                    job.matched, job.sender
                );
                let details = json!({
                    "job_id": job.id,
                    "account_id": job.account_id,
                    "account_email": account_email(&tx, &job.account_id)?,
                    "sender": job.sender,
                    "matched": job.matched,
                    "sample_subjects": job.sample_subjects,
                });
                (vec![job.account_id], summary, details)
            }
        };
        details["request"] = request_json;
        details["request_digest"] = json!(request_digest);
        let id = insert_action(
            &tx,
            kind,
            Some(&target),
            &account_ids,
            "pending",
            &summary,
            &details,
            client,
        )?;
        let action = read_action(&tx, &id)?.ok_or_else(|| MailError::NotFound(id.clone()))?;
        tx.commit()?;
        Ok(action)
    }

    /// Record agent activity that needed no approval, for the in-app history.
    /// Frequent operations (sync, cleanup steps) share one entry per kind and
    /// accounts for ten minutes, with a count, instead of flooding the history.
    pub fn record_agent_activity(
        &self,
        kind: &str,
        account_ids: &[String],
        summary: &str,
        details: Value,
        client: Option<&str>,
    ) -> Result<()> {
        let db = self.lock()?;
        if matches!(
            kind,
            "sync_account" | "sync_all" | "sync_calendar" | "refresh_calendar_invitation" | "step_sender_cleanup"
        ) {
            let client = sanitize_client(client);
            let recent: Option<(String, i64)> = db
                .query_row(
                    "SELECT id,coalesce(json_extract(details_json,'$.count'),1) FROM agent_actions WHERE status='done' AND kind=?1 AND account_ids_json=?2 AND coalesce(client,'')=coalesce(?4,'') AND coalesce(decided_at_ms,created_at_ms)>=?3 ORDER BY created_at_ms DESC LIMIT 1",
                    params![kind, serde_json::to_string(account_ids)?, now_ms() - BLOCK_COALESCE_MS, client],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            if let Some((id, count)) = recent {
                // Keep the latest wording (for example cleanup progress) and count.
                db.execute(
                    "UPDATE agent_actions SET summary=?4,details_json=json_set(?5,'$.count',?2,'$.last_at_ms',?3),decided_at_ms=?3 WHERE id=?1",
                    params![id, count + 1, now_ms(), truncate_chars(summary, 300), serde_json::to_string(&details)?],
                )?;
                return Ok(());
            }
        }
        insert_action(&db, kind, None, account_ids, "done", summary, &details, client)?;
        Ok(())
    }

    /// One attachment of a waiting send request, so the user can save and
    /// inspect it before approving. Only files in the request's snapshot.
    pub fn agent_request_attachment(&self, action_id: &str, attachment_id: &str) -> Result<(String, String, Vec<u8>)> {
        let db = self.lock()?;
        let action = read_action(&db, action_id)?.ok_or_else(|| MailError::NotFound(action_id.into()))?;
        let listed = action.details["draft"]["attachments"]
            .as_array()
            .is_some_and(|files| files.iter().any(|f| f["id"].as_str() == Some(attachment_id)));
        let message_id = action.details["request"]["message_id"].as_str().unwrap_or_default();
        if action.status != "pending" || !listed {
            return Err(invalid("That attachment is not part of a waiting request"));
        }
        Ok(db.query_row(
            "SELECT filename,mime_type,bytes FROM draft_attachments WHERE id=?1 AND message_id=?2",
            params![attachment_id, message_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?)
    }

    /// Pending requests first (oldest first), then history (newest first). `status` is
    /// `pending`, `history` (everything decided), or one decided status.
    pub fn list_agent_actions(&self, status: Option<&str>, limit: u32) -> Result<Vec<AgentAction>> {
        let filter = match status {
            None => "",
            Some("pending") => "WHERE status='pending'",
            Some("history") => "WHERE status!='pending'",
            Some(other) if ["approved", "rejected", "failed", "done", "blocked", "replaced"].contains(&other) => {
                "WHERE status=?2"
            }
            Some(other) => return Err(invalid(format!("Unknown agent action status: {other}"))),
        };
        let db = self.lock()?;
        let mut query = db.prepare(&format!(
            // Waiting requests oldest first, so a new request never moves a card
            // the user is reading; decided entries newest first.
            "SELECT {ACTION_COLUMNS} FROM agent_actions {filter} ORDER BY (status='pending') DESC,CASE WHEN status='pending' THEN created_at_ms END ASC,coalesce(decided_at_ms,created_at_ms) DESC,rowid DESC LIMIT ?1"
        ))?;
        let limit = limit.clamp(1, 1000);
        let rows = if filter.contains("?2") {
            query.query_map(params![limit, status], action_row)?
                .collect::<std::result::Result<Vec<_>, _>>()?
        } else {
            query.query_map(params![limit], action_row)?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        rows.into_iter().map(RawAction::decode).collect()
    }

    /// Record that the MCP server refused an agent because consent was missing
    /// or too narrow, so the app can point the user at Agent access. Repeated
    /// refusals for the same accounts within ten minutes share one entry.
    pub fn record_agent_block(
        &self,
        account_ids: &[String],
        operation: &str,
        reason: &str,
        client: Option<&str>,
    ) -> Result<()> {
        let mut db = self.lock()?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let key = serde_json::to_string(account_ids)?;
        let now = now_ms();
        let recent: Option<(String, String)> = tx
            .query_row(
                "SELECT id,details_json FROM agent_actions WHERE status='blocked' AND account_ids_json=?1 AND coalesce(json_extract(details_json,'$.last_at_ms'),created_at_ms)>=?2 AND coalesce(error,'')=?3 ORDER BY created_at_ms DESC LIMIT 1",
                params![key, now - BLOCK_COALESCE_MS, truncate_chars(reason, 300)],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((id, details)) = recent {
            let mut details: Value = serde_json::from_str(&details)?;
            let attempts = details["attempts"].as_u64().unwrap_or(1) + 1;
            details["attempts"] = json!(attempts);
            details["last_at_ms"] = json!(now);
            details["last_operation"] = json!(operation);
            tx.execute(
                "UPDATE agent_actions SET details_json=?2,error=?3,decided_at_ms=?4 WHERE id=?1",
                params![id, serde_json::to_string(&details)?, truncate_chars(reason, 300), now],
            )?;
        } else {
            let emails = account_ids
                .iter()
                .filter_map(|id| account_email(&tx, id).ok())
                .collect::<Vec<_>>();
            let summary = if emails.is_empty() {
                "An agent request was refused".to_string()
            } else {
                format!("An agent was blocked for {}", emails.join(", "))
            };
            let id = insert_action(
                &tx,
                "access_blocked",
                None,
                account_ids,
                "blocked",
                &summary,
                &json!({"attempts":1,"last_at_ms":now,"last_operation":operation,"account_emails":emails}),
                client,
            )?;
            tx.execute(
                "UPDATE agent_actions SET error=?2 WHERE id=?1",
                params![id, truncate_chars(reason, 300)],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Entries whose every account is in `accounts`, filtered in SQL so an
    /// agent's listing never decodes other accounts' rows.
    pub fn list_agent_actions_for_accounts(
        &self,
        status: Option<&str>,
        accounts: &[String],
        limit: u32,
    ) -> Result<Vec<AgentAction>> {
        let filter = match status {
            None => "1",
            Some("pending") => "status='pending'",
            Some("history") => "status!='pending'",
            Some(other) if ["approved", "rejected", "failed", "done", "blocked", "replaced"].contains(&other) => {
                "status=?2"
            }
            Some(other) => return Err(invalid(format!("Unknown agent action status: {other}"))),
        };
        let db = self.lock()?;
        let mut query = db.prepare(&format!(
            "SELECT {ACTION_COLUMNS} FROM agent_actions WHERE {filter} AND json_array_length(account_ids_json)>0 AND NOT EXISTS(SELECT 1 FROM json_each(agent_actions.account_ids_json) WHERE value NOT IN (SELECT value FROM json_each(?3))) ORDER BY (status='pending') DESC,CASE WHEN status='pending' THEN created_at_ms END ASC,coalesce(decided_at_ms,created_at_ms) DESC,rowid DESC LIMIT ?1"
        ))?;
        let rows = query
            .query_map(
                params![limit.clamp(1, 1000), status.unwrap_or(""), serde_json::to_string(accounts)?],
                action_row,
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        rows.into_iter().map(RawAction::decode).collect()
    }

    pub fn agent_action(&self, id: &str) -> Result<Option<AgentAction>> {
        let mut db = self.lock()?;
        let action = read_action(&db, id)?;
        if action.as_ref().is_some_and(|a| a.status == "pending")
            && stale_requests(&db)?.iter().any(|(stale, _)| stale == id)
        {
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            close_stale_send_requests(&tx)?;
            tx.commit()?;
            return read_action(&db, id);
        }
        Ok(action)
    }

    /// Count and newest pending requests, for badges and notifications. Send
    /// requests whose draft was sent, scheduled or discarded meanwhile close
    /// here, so they never wait for an Approve that cannot apply.
    pub fn agent_pending_summary(&self) -> Result<Value> {
        let mut db = self.lock()?;
        // Write only when a pending send exists, so an idle poll never waits
        // for the write lock.
        // Detect read-only; take the write lock only when something must close.
        if !stale_requests(&db)?.is_empty() {
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            close_stale_send_requests(&tx)?;
            tx.commit()?;
        }
        let count: i64 = db.query_row(
            "SELECT count(*) FROM agent_actions WHERE status='pending'",
            [],
            |r| r.get(0),
        )?;
        let mut query = db.prepare(
            "SELECT id,kind,summary,created_at_ms,account_ids_json FROM agent_actions WHERE status='pending' ORDER BY created_at_ms DESC,rowid DESC LIMIT 20",
        )?;
        let items = query
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, String>(4)?,
                ))
            })?
            .map(|row| {
                let (id, kind, summary, created_at_ms, accounts) = row?;
                let accounts: Value = serde_json::from_str(&accounts)?;
                Ok(json!({"id":id,"kind":kind,"summary":summary,"created_at_ms":created_at_ms,"account_ids":accounts}))
            })
            .collect::<Result<Vec<_>>>()?;
        let blocked = blocked_summary(&db)?;
        // Changes whenever any request, decision or activity is recorded, so
        // the shells re-read the full list only when something changed.
        let revision: (i64, i64, i64) = db.query_row(
            "SELECT count(*),coalesce(max(created_at_ms),0),coalesce(max(decided_at_ms),0) FROM agent_actions",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        let consents: (i64, i64) = db.query_row(
            "SELECT count(*),coalesce(max(granted_at_ms),0) FROM agent_consents",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let blocked_at = blocked.as_ref().and_then(|b| b["last_at_ms"].as_i64()).unwrap_or(0);
        let revision = format!(
            "{}-{}-{}-{}-{}-{blocked_at}",
            revision.0, revision.1, revision.2, consents.0, consents.1
        );
        Ok(json!({"count":count,"items":items,"blocked":blocked,"revision":revision}))
    }

    /// Apply a pending request on the user's explicit approval. The request is
    /// rechecked against current consent and, for sends, the exact draft the
    /// user saw. Sends are queued with an undo window and leave through the
    /// normal dispatcher; nothing is sent from this call.
    pub fn approve_agent_action(&self, id: &str, undo_seconds: u32) -> Result<AgentAction> {
        match self.approve_inner(id, undo_seconds) {
            Ok(()) => {}
            Err(MailError::Database(rusqlite::Error::QueryReturnedNoRows)) => {
                return Err(self.close_failed(id, "an item in this request no longer exists".into())?);
            }
            Err(error @ (MailError::Database(_) | MailError::Lock | MailError::Io(_))) => {
                return Err(error);
            }
            Err(MailError::InvalidAction(reason)) => return Err(self.close_failed(id, reason)?),
            Err(error) => {
                // Close it, but report the original error.
                self.close_failed(id, error.to_string())?;
                return Err(error);
            }
        }
        self.agent_action(id)?
            .ok_or_else(|| MailError::NotFound(id.into()))
    }

    /// Close a request that cannot be applied, so it never waits forever, and
    /// return the error to show.
    fn close_failed(&self, id: &str, reason: String) -> Result<MailError> {
        let mut db = self.lock()?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let closed = close_pending(&tx, id, "failed", Some(&reason))?;
        tx.commit()?;
        Ok(if closed {
            invalid(format!(
                "Could not approve: {reason}. The request was closed; the agent can ask again"
            ))
        } else {
            invalid(reason)
        })
    }

    fn approve_inner(&self, id: &str, undo_seconds: u32) -> Result<()> {
        let mut db = self.lock()?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let action = read_action(&tx, id)?.ok_or_else(|| MailError::NotFound(id.into()))?;
        if action.status == "replaced" {
            return Err(invalid("The agent replaced this request with a newer one"));
        }
        if action.status != "pending" {
            return Err(invalid("This request was already decided"));
        }
        let grant = agent_grant_for(&action.kind);
        for account in &action.account_ids {
            if !read_consent(&tx, account)?.is_some_and(|c| c.allows(grant)) {
                return Err(invalid(format!(
                    "agent access for this account no longer includes {}",
                    agent_grant_label(grant)
                )));
            }
        }
        let undo_ms = i64::from(undo_seconds.clamp(1, 60)) * 1000;
        let request = &action.details["request"];
        let result = match action.kind.as_str() {
            "send_draft" | "queue_send_draft" | "schedule_draft" => {
                let message_id = request["message_id"]
                    .as_str()
                    .ok_or_else(|| invalid("request has no draft"))?;
                let current = draft_snapshot(&tx, message_id)?;
                if current != action.details["draft"] {
                    return Err(invalid("the draft changed after the agent's request"));
                }
                let now = now_ms();
                let (send_at, status) = match action.kind.as_str() {
                    "send_draft" => (now + undo_ms, "undoable"),
                    "queue_send_draft" => {
                        let requested = request["undo_seconds"]
                            .as_u64()
                            .unwrap_or(u64::from(DEFAULT_APPROVAL_UNDO_SECONDS))
                            .clamp(1, 60) as i64
                            * 1000;
                        (now + requested.max(undo_ms), "undoable")
                    }
                    _ => {
                        let at = request["send_at_ms"]
                            .as_i64()
                            .ok_or_else(|| invalid("request has no send time"))?;
                        // A time that passed while waiting still gets an undo window.
                        if at > now + undo_ms {
                            (at, "scheduled")
                        } else {
                            (now + undo_ms, "undoable")
                        }
                    }
                };
                schedule_draft_tx(&tx, message_id, send_at, status)?;
                json!({"message_id":message_id,"send_at_ms":send_at,"status":status})
            }
            "batch_triage" => {
                let operation_id = request["operation_id"]
                    .as_str()
                    .ok_or_else(|| invalid("request has no operation"))?;
                let kind = request["kind"]
                    .as_str()
                    .ok_or_else(|| invalid("request has no triage action"))?;
                let ids: Vec<String> = serde_json::from_value(request["thread_ids"].clone())?;
                require_threads(&tx, &ids)?;
                let mut result = batch_triage_tx(&tx, operation_id, kind, &ids)?;
                result["operation_id"] = json!(operation_id);
                result
            }
            "undo_triage" => {
                let operation_id = request["operation_id"]
                    .as_str()
                    .ok_or_else(|| invalid("request has no operation"))?;
                let mut result = undo_triage_tx(&tx, operation_id)?;
                result["operation_id"] = json!(operation_id);
                result
            }
            "confirm_sender_cleanup" => {
                let job_id = request["job_id"]
                    .as_str()
                    .ok_or_else(|| invalid("request has no cleanup job"))?;
                let job = cleanup::confirm_tx(&tx, job_id)?;
                json!({"job_id":job.id,"state":job.state,"matched":job.matched})
            }
            other => return Err(invalid(format!("unknown agent request {other}"))),
        };
        let details = minimize(&action.kind, action.details.clone());
        let changed = tx.execute(
            "UPDATE agent_actions SET status='approved',decided_at_ms=?2,result_json=?3,details_json=?4,error=NULL WHERE id=?1 AND status='pending'",
            params![id, now_ms(), serde_json::to_string(&result)?, serde_json::to_string(&details)?],
        )?;
        if changed != 1 {
            return Err(invalid("This request was already decided"));
        }
        trim_history(&tx)?;
        tx.commit()?;
        Ok(())
    }

    /// Reject a pending request. Nothing it asked for is applied.
    pub fn reject_agent_action(&self, id: &str) -> Result<AgentAction> {
        let mut db = self.lock()?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if !close_pending(&tx, id, "rejected", None)? {
            return Err(if read_action(&tx, id)?.is_some() {
                invalid("This request was already decided")
            } else {
                MailError::NotFound(id.into())
            });
        }
        let action = read_action(&tx, id)?.ok_or_else(|| MailError::NotFound(id.into()))?;
        tx.commit()?;
        Ok(action)
    }

    /// Subject of a cached conversation or message, for activity summaries.
    pub fn agent_subject(&self, thread_id: Option<&str>, message_id: Option<&str>) -> Result<Option<String>> {
        let db = self.lock()?;
        if let Some(id) = message_id {
            return Ok(db
                .query_row("SELECT subject FROM messages WHERE id=?1", [id], |r| r.get(0))
                .optional()?);
        }
        if let Some(id) = thread_id {
            return Ok(db
                .query_row("SELECT subject FROM threads WHERE id=?1", [id], |r| r.get(0))
                .optional()?);
        }
        Ok(None)
    }
}

fn list_strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|items| items.iter().filter_map(Value::as_str).map(str::to_owned).collect())
        .unwrap_or_default()
}

/// Removing an account deletes its consent and agent history. An entry that
/// also covers other accounts keeps their part, without the removed
/// account's conversations or address.
pub(crate) fn remove_account_rows(
    db: &Connection,
    account_id: &str,
    thread_ids: &std::collections::HashSet<String>,
) -> Result<()> {
    db.execute("DELETE FROM agent_consents WHERE account_id=?1", [account_id])?;
    let email = account_email(db, account_id).ok();
    let rows = {
        let mut query = db.prepare(
            "SELECT id,account_ids_json,kind,status FROM agent_actions WHERE EXISTS(SELECT 1 FROM json_each(agent_actions.account_ids_json) WHERE value=?1)",
        )?;
        query
            .query_map([account_id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?
    };
    for (id, accounts, kind, status) in rows {
        let mut accounts: Vec<String> = serde_json::from_str(&accounts)?;
        accounts.retain(|a| a != account_id);
        if accounts.is_empty() {
            db.execute("DELETE FROM agent_actions WHERE id=?1", [&id])?;
            continue;
        }
        // A waiting request that spanned the removed account no longer
        // describes what would happen; close it before trimming.
        if status == "pending" {
            close_pending(db, &id, "rejected", Some("An account in this request was removed"))?;
        }
        let details: String =
            db.query_row("SELECT details_json FROM agent_actions WHERE id=?1", [&id], |r| r.get(0))?;
        let mut details: Value = serde_json::from_str(&details)?;
        if let Some(threads) = details.get_mut("threads").and_then(Value::as_array_mut) {
            threads.retain(|t| t["account_id"].as_str() != Some(account_id));
        }
        if let Some(ids) = details
            .get_mut("request")
            .and_then(|r| r.get_mut("thread_ids"))
            .and_then(Value::as_array_mut)
        {
            ids.retain(|t| !t.as_str().is_some_and(|t| thread_ids.contains(t)));
        }
        if let (Some(emails), Some(email)) = (details.get_mut("account_emails").and_then(Value::as_array_mut), &email) {
            emails.retain(|e| e.as_str() != Some(email.as_str()));
        }
        let removed = details["account_counts"][account_id].as_u64().unwrap_or(0) as usize;
        if let Some(counts) = details.get_mut("account_counts").and_then(Value::as_object_mut) {
            counts.remove(account_id);
        }
        let count = (details["count"].as_u64().unwrap_or(0) as usize).saturating_sub(removed);
        if details.get("count").is_some() {
            details["count"] = json!(count);
        }
        let emails = list_strings(&details["account_emails"]);
        let summary: Option<String> = match kind.as_str() {
            "batch_triage" => Some(triage_label(details["kind"].as_str().unwrap_or_default(), count)),
            "undo_triage" => Some(undo_label(details["kind"].as_str().unwrap_or_default(), count)),
            "access_blocked" => Some(format!("An agent was blocked for {}", emails.join(", "))),
            _ => None,
        };
        db.execute(
            "UPDATE agent_actions SET account_ids_json=?2,details_json=?3,summary=coalesce(?4,summary) WHERE id=?1",
            params![id, serde_json::to_string(&accounts)?, serde_json::to_string(&details)?, summary],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> MailStore {
        let store = MailStore::open_in_memory().unwrap();
        store.seed_demo().unwrap();
        store
    }
    fn all_grants() -> Vec<String> {
        AGENT_GRANTS.iter().map(|g| g.to_string()).collect()
    }
    fn draft(store: &MailStore) -> String {
        store
            .compose_draft("demo:personal", "maya@example.org", "Agent draft", "Hello from an agent")
            .unwrap()
            .id
    }
    fn send(id: &str) -> AgentApprovalRequest {
        AgentApprovalRequest::SendDraft {
            message_id: id.into(),
        }
    }

    #[test]
    fn consent_is_recorded_validated_revocable_and_logged() {
        let store = store();
        assert!(store.agent_consents().unwrap().is_empty());
        assert!(store
            .grant_agent_access("demo:personal", &["read_everything".into()], AGENT_DISCLOSURE_VERSION)
            .is_err());
        assert!(store
            .grant_agent_access("demo:personal", &[], AGENT_DISCLOSURE_VERSION - 1)
            .is_err());
        assert!(store
            .grant_agent_access("missing", &[], AGENT_DISCLOSURE_VERSION)
            .is_err());
        let consent = store
            .grant_agent_access(
                "demo:personal",
                &["send".into(), "draft".into(), "draft".into()],
                AGENT_DISCLOSURE_VERSION,
            )
            .unwrap();
        assert_eq!(consent.grants, vec!["draft", "send"]);
        assert!(consent.granted_at_ms > 0 && consent.allows("send") && !consent.allows("triage"));
        assert_eq!(store.agent_consents().unwrap(), vec![consent]);
        assert!(store.revoke_agent_access("demo:personal").unwrap());
        assert!(!store.revoke_agent_access("demo:personal").unwrap());
        assert!(store.agent_consents().unwrap().is_empty());
        let history = store.list_agent_actions(Some("history"), 10).unwrap();
        assert_eq!(
            history.iter().map(|a| a.kind.as_str()).collect::<Vec<_>>(),
            vec!["consent_revoked", "consent_granted"]
        );
        assert!(history[1].summary.contains("alex@example.com"));
        assert!(history[1].summary.ends_with("Read, Drafts, Ask to send"), "{}", history[1].summary);
    }

    #[test]
    fn consent_for_an_older_reworded_disclosure_keeps_working() {
        let store = store();
        // A consent recorded before the redesign, under disclosure version 1.
        store
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO agent_consents(account_id,granted_at_ms,grants_json,disclosure_version) VALUES('demo:personal',1,'[\"draft\"]',?1)",
                [AGENT_DISCLOSURE_MIN_VERSION],
            )
            .unwrap();
        let consent = store.agent_consents().unwrap().remove(0);
        assert!(AGENT_DISCLOSURE_MIN_VERSION < AGENT_DISCLOSURE_VERSION);
        assert!(consent.is_current() && consent.allows("draft") && !consent.allows("send"));
        // New consents must still come from the text the app shows now.
        assert!(store
            .grant_agent_access("demo:personal", &[], AGENT_DISCLOSURE_MIN_VERSION)
            .is_err());
        // Below the minimum, a consent stops authorizing until reviewed again.
        let stale = AgentConsent { disclosure_version: AGENT_DISCLOSURE_MIN_VERSION - 1, ..consent };
        assert!(!stale.is_current() && !stale.allows("draft"));
    }

    #[test]
    fn send_request_waits_for_approval_and_is_queued_with_undo_window() {
        let store = store();
        store
            .grant_agent_access("demo:personal", &all_grants(), AGENT_DISCLOSURE_VERSION)
            .unwrap();
        let id = draft(&store);
        let request = store.request_agent_approval(&send(&id), Some("codex\u{7}")).unwrap();
        assert_eq!(request.status, "pending");
        assert_eq!(request.client.as_deref(), Some("codex"));
        assert_eq!(request.details["draft"]["to"], json!(["maya@example.org"]));
        assert_eq!(request.details["draft"]["body_plain"], "Hello from an agent");
        assert!(request.summary.starts_with("Send “Agent draft” to maya@example.org"));
        // Nothing reaches the outbox or the dispatcher's due list while pending.
        assert!(store.list_outbox().unwrap().is_empty());
        assert!(store.due_outbox(100).unwrap().is_empty());
        // An identical retry returns the same request rather than a duplicate.
        assert_eq!(store.request_agent_approval(&send(&id), None).unwrap().id, request.id);
        assert!(store
            .request_agent_approval(
                &AgentApprovalRequest::ScheduleDraft {
                    message_id: id.clone(),
                    send_at_ms: now_ms() + 60_000
                },
                None
            )
            .is_err());
        assert_eq!(store.agent_pending_summary().unwrap()["count"], 1);
        let before = now_ms();
        let approved = store.approve_agent_action(&request.id, 10).unwrap();
        assert_eq!(approved.status, "approved");
        let outbox = store.list_outbox().unwrap();
        assert_eq!(outbox.len(), 1);
        assert_eq!(outbox[0].status, "undoable");
        assert!(outbox[0].send_at_ms >= before + 10_000);
        assert_eq!(approved.result.unwrap()["send_at_ms"], outbox[0].send_at_ms);
        // The undo window still applies after approval.
        store.cancel_scheduled_draft(&id).unwrap();
        assert!(store.list_outbox().unwrap().is_empty());
        assert!(store.approve_agent_action(&request.id, 10).is_err());
        assert_eq!(store.agent_pending_summary().unwrap()["count"], 0);
    }

    #[test]
    fn rejection_and_changed_drafts_never_reach_the_outbox() {
        let store = store();
        store
            .grant_agent_access("demo:personal", &all_grants(), AGENT_DISCLOSURE_VERSION)
            .unwrap();
        let id = draft(&store);
        let rejected = store.request_agent_approval(&send(&id), None).unwrap();
        assert_eq!(store.reject_agent_action(&rejected.id).unwrap().status, "rejected");
        assert!(store.approve_agent_action(&rejected.id, 10).is_err());
        assert!(store.reject_agent_action(&rejected.id).is_err());
        assert!(store.list_outbox().unwrap().is_empty());

        let changed = store.request_agent_approval(&send(&id), None).unwrap();
        store
            .update_draft(&id, "attacker@example.net", "Agent draft", "Different text")
            .unwrap();
        let error = store.approve_agent_action(&changed.id, 10).unwrap_err().to_string();
        assert!(error.contains("draft changed"), "{error}");
        let closed = store.agent_action(&changed.id).unwrap().unwrap();
        assert_eq!(closed.status, "failed");
        assert!(store.list_outbox().unwrap().is_empty());
        // Decided history keeps only a short preview of the body.
        let long = "x".repeat(2000);
        store.update_draft(&id, "maya@example.org", "Long", &long).unwrap();
        let request = store.request_agent_approval(&send(&id), None).unwrap();
        assert_eq!(request.details["draft"]["body_plain"].as_str().unwrap().len(), 2000);
        let rejected = store.reject_agent_action(&request.id).unwrap();
        assert!(rejected.details["draft"]["body_plain"].as_str().unwrap().chars().count() <= PREVIEW_CHARS + 1);
    }

    #[test]
    fn pending_requests_survive_restart_and_approve_afterwards() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mail.sqlite");
        let (id, request) = {
            let store = MailStore::open(&path).unwrap();
            store.seed_demo().unwrap();
            store
                .grant_agent_access("demo:personal", &["send".into()], AGENT_DISCLOSURE_VERSION)
                .unwrap();
            let id = draft(&store);
            let request = store
                .request_agent_approval(
                    &AgentApprovalRequest::QueueSendDraft {
                        message_id: id.clone(),
                        undo_seconds: Some(30),
                    },
                    None,
                )
                .unwrap();
            (id, request.id)
        };
        let store = MailStore::open(&path).unwrap();
        assert!(store.list_outbox().unwrap().is_empty());
        let pending = store.list_agent_actions(Some("pending"), 10).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, request);
        let before = now_ms();
        store.approve_agent_action(&request, 10).unwrap();
        let outbox = store.list_outbox().unwrap();
        assert_eq!(outbox[0].message_id, id);
        // The agent's longer undo request is honoured.
        assert!(outbox[0].send_at_ms >= before + 30_000);
    }

    #[test]
    fn schedule_requests_keep_future_times_and_add_undo_when_time_passed() {
        let store = store();
        store
            .grant_agent_access("demo:personal", &["send".into()], AGENT_DISCLOSURE_VERSION)
            .unwrap();
        let future = now_ms() + 86_400_000;
        let id = draft(&store);
        let request = store
            .request_agent_approval(
                &AgentApprovalRequest::ScheduleDraft {
                    message_id: id.clone(),
                    send_at_ms: future,
                },
                None,
            )
            .unwrap();
        store.approve_agent_action(&request.id, 10).unwrap();
        let item = &store.list_outbox().unwrap()[0];
        assert_eq!((item.send_at_ms, item.status.as_str()), (future, "scheduled"));
        store.cancel_scheduled_draft(&id).unwrap();

        let soon = now_ms() + 1_000;
        let request = store
            .request_agent_approval(
                &AgentApprovalRequest::ScheduleDraft {
                    message_id: id.clone(),
                    send_at_ms: soon,
                },
                None,
            )
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1_100));
        let before = now_ms();
        store.approve_agent_action(&request.id, 10).unwrap();
        let item = &store.list_outbox().unwrap()[0];
        assert_eq!(item.status, "undoable");
        assert!(item.send_at_ms >= before + 10_000);
        assert!(store
            .request_agent_approval(
                &AgentApprovalRequest::ScheduleDraft {
                    message_id: draft(&store),
                    send_at_ms: now_ms() - 1
                },
                None
            )
            .is_err());
    }

    #[test]
    fn revocation_and_narrowed_grants_withdraw_pending_requests() {
        let store = store();
        store
            .grant_agent_access("demo:personal", &all_grants(), AGENT_DISCLOSURE_VERSION)
            .unwrap();
        let first = store.request_agent_approval(&send(&draft(&store)), None).unwrap();
        let batch = store
            .request_agent_approval(
                &AgentApprovalRequest::BatchTriage {
                    operation_id: "agent-op".into(),
                    kind: "archive".into(),
                    thread_ids: vec!["demo:1".into()],
                },
                None,
            )
            .unwrap();
        store
            .grant_agent_access("demo:personal", &["triage".into()], AGENT_DISCLOSURE_VERSION)
            .unwrap();
        let withdrawn = store.agent_action(&first.id).unwrap().unwrap();
        assert_eq!(withdrawn.status, "rejected");
        assert!(withdrawn.error.unwrap().contains("turned off"));
        assert_eq!(store.agent_action(&batch.id).unwrap().unwrap().status, "pending");
        store.revoke_agent_access("demo:personal").unwrap();
        assert_eq!(store.agent_action(&batch.id).unwrap().unwrap().status, "rejected");
        assert!(store.approve_agent_action(&batch.id, 10).is_err());
        assert!(store.list_threads(None, 50, 0).unwrap().iter().any(|t| t.id == "demo:1"));
        assert!(store.latest_triage_undo().unwrap().is_none());
    }

    #[test]
    fn approval_rechecks_consent_even_if_a_request_is_still_pending() {
        let store = store();
        store
            .grant_agent_access("demo:personal", &["send".into()], AGENT_DISCLOSURE_VERSION)
            .unwrap();
        let request = store.request_agent_approval(&send(&draft(&store)), None).unwrap();
        // Simulate an external consent change that skipped withdrawal.
        store
            .lock()
            .unwrap()
            .execute("UPDATE agent_consents SET grants_json='[]'", [])
            .unwrap();
        let error = store.approve_agent_action(&request.id, 10).unwrap_err().to_string();
        assert!(error.contains("no longer includes Ask to send"), "{error}");
        assert!(store.list_outbox().unwrap().is_empty());
    }

    #[test]
    fn batch_triage_is_applied_only_on_approval_and_keeps_undo() {
        let store = store();
        store
            .grant_agent_access("demo:personal", &["triage".into()], AGENT_DISCLOSURE_VERSION)
            .unwrap();
        let request = AgentApprovalRequest::BatchTriage {
            operation_id: "agent-archive".into(),
            kind: "archive".into(),
            thread_ids: vec!["demo:2".into(), "demo:1".into()],
        };
        let pending = store.request_agent_approval(&request, Some("claude-code")).unwrap();
        assert_eq!(pending.summary, "Archive 2 conversations");
        assert_eq!(pending.details["threads"].as_array().unwrap().len(), 2);
        assert!(pending.details.get("thread_ids").is_none());
        assert_eq!(store.pending_actions().unwrap().len(), 0);
        assert!(store.list_threads(None, 50, 0).unwrap().iter().any(|t| t.id == "demo:1"));
        assert!(store
            .request_agent_approval(
                &AgentApprovalRequest::BatchTriage {
                    operation_id: "agent-archive".into(),
                    kind: "spam".into(),
                    thread_ids: vec!["demo:1".into()],
                },
                None
            )
            .is_err());
        let approved = store.approve_agent_action(&pending.id, 10).unwrap();
        assert_eq!(approved.result.as_ref().unwrap()["changed"], 2);
        assert!(!store.list_threads(None, 50, 0).unwrap().iter().any(|t| t.id == "demo:1"));
        assert_eq!(store.pending_actions().unwrap().len(), 2);
        assert_eq!(store.latest_triage_undo().unwrap().unwrap()["operation_id"], "agent-archive");
        // The decided entry drops the full selection but keeps count and sample.
        assert!(approved.details["request"].get("thread_ids").is_none());
        assert_eq!(approved.details["count"], 2);
        // A retry after approval reports the outcome without applying it again.
        let retry = store.request_agent_approval(&request, None).unwrap();
        assert_eq!((retry.id, retry.status.as_str()), (pending.id, "approved"));
        assert!(store
            .request_agent_approval(
                &AgentApprovalRequest::BatchTriage {
                    operation_id: "unknown-thread".into(),
                    kind: "archive".into(),
                    thread_ids: vec!["missing".into()],
                },
                None
            )
            .is_err());
    }

    #[test]
    fn sender_cleanup_confirmation_waits_for_approval() {
        let store = store();
        store
            .grant_agent_access("demo:personal", &["triage".into()], AGENT_DISCLOSURE_VERSION)
            .unwrap();
        let job = store.create_sender_cleanup("demo:personal", "news@example.com").unwrap();
        let request = AgentApprovalRequest::ConfirmSenderCleanup { job_id: job.id.clone() };
        assert!(store.request_agent_approval(&request, None).is_err());
        {
            let db = store.lock().unwrap();
            db.execute("UPDATE sender_cleanups SET state='ready',phase='apply' WHERE id=?1", [&job.id]).unwrap();
            db.execute("INSERT INTO sender_cleanup_items(job_id,message_id) VALUES(?1,'demo:3')", [&job.id]).unwrap();
        }
        let pending = store.request_agent_approval(&request, None).unwrap();
        assert_eq!(pending.summary, "Mark 1 unread message from news@example.com read");
        assert_eq!(store.sender_cleanup(&job.id).unwrap().state, "ready");
        store.approve_agent_action(&pending.id, 10).unwrap();
        assert_eq!(store.sender_cleanup(&job.id).unwrap().state, "running");
    }

    #[test]
    fn removing_an_account_deletes_consent_and_agent_history() {
        let store = store();
        store
            .grant_agent_access("demo:personal", &["send".into()], AGENT_DISCLOSURE_VERSION)
            .unwrap();
        store.request_agent_approval(&send(&draft(&store)), None).unwrap();
        store
            .record_agent_activity("archive_thread", &["demo:personal".into()], "Archived", json!({}), None)
            .unwrap();
        store.remove_account("demo:personal").unwrap();
        assert!(store.agent_consents().unwrap().is_empty());
        assert!(store.list_agent_actions(None, 100).unwrap().is_empty());
    }

    #[test]
    fn blocked_attempts_are_coalesced_and_reported() {
        let store = store();
        assert!(store.agent_pending_summary().unwrap()["blocked"].is_null());
        store.record_agent_block(&[], "list_accounts", "No account has agent access", Some("codex")).unwrap();
        store.record_agent_block(&[], "search_mail", "No account has agent access", Some("codex")).unwrap();
        store
            .record_agent_block(&["demo:personal".into()], "get_message", "Agent access is off", None)
            .unwrap();
        let blocked = store.list_agent_actions(Some("blocked"), 10).unwrap();
        assert_eq!(blocked.len(), 2);
        let global = blocked.iter().find(|b| b.account_ids.is_empty()).unwrap();
        assert_eq!(global.details["attempts"], 2);
        assert_eq!(global.details["last_operation"], "search_mail");
        let summary = store.agent_pending_summary().unwrap();
        assert!(summary["blocked"]["summary"].as_str().unwrap().contains("alex@example.com"));
        assert_eq!(summary["blocked"]["needs_access"], true);
        assert_eq!(summary["blocked"]["affected_account_ids"], json!(["demo:personal"]));
        // Once the user turns access on, the refusal no longer needs attention.
        store.grant_agent_access("demo:personal", &[], AGENT_DISCLOSURE_VERSION).unwrap();
        assert!(store.agent_pending_summary().unwrap()["blocked"].is_null());
        // A later refusal for a missing grant is shown, but access is on.
        std::thread::sleep(std::time::Duration::from_millis(2));
        store
            .record_agent_block(&["demo:personal".into()], "send_draft", "Agent access for this account does not include “Ask to send”", None)
            .unwrap();
        let summary = store.agent_pending_summary().unwrap();
        assert_eq!(summary["blocked"]["needs_access"], false);
    }

    #[test]
    fn removing_one_account_keeps_the_other_accounts_part_of_a_shared_entry() {
        let store = store();
        let mut other = store.list_accounts().unwrap().remove(0);
        other.id = "other".into();
        other.email = "other@example.com".into();
        store.upsert_account(&other).unwrap();
        let mut message = store.get_message("demo:2").unwrap().unwrap();
        message.id = "other-message".into();
        message.thread_id = "other-thread".into();
        message.account_id = "other".into();
        store.upsert_message(&message).unwrap();
        for account in ["demo:personal", "other"] {
            store.grant_agent_access(account, &["triage".into()], AGENT_DISCLOSURE_VERSION).unwrap();
        }
        let shared = store
            .request_agent_approval(
                &AgentApprovalRequest::BatchTriage {
                    operation_id: "both".into(),
                    kind: "archive".into(),
                    thread_ids: vec!["demo:1".into(), "other-thread".into()],
                },
                None,
            )
            .unwrap();
        store
            .record_agent_block(&["demo:personal".into(), "other".into()], "list_labels", "Not all accounts", None)
            .unwrap();
        let revision = store.agent_pending_summary().unwrap()["revision"].clone();
        store.remove_account("other").unwrap();
        assert_ne!(store.agent_pending_summary().unwrap()["revision"], revision);
        let kept = store.agent_action(&shared.id).unwrap().unwrap();
        assert_eq!(kept.account_ids, vec!["demo:personal".to_string()]);
        // The waiting request no longer describes what would happen, so it closes.
        assert_eq!(kept.status, "rejected");
        assert_eq!(kept.summary, "Archive 1 conversation");
        assert_eq!(kept.details["count"], 1);
        assert_eq!(kept.details["threads"].as_array().unwrap().len(), 1);
        assert_eq!(kept.details["account_emails"], json!(["alex@example.com"]));
        assert!(!serde_json::to_string(&kept).unwrap().contains("other@example.com"));
        let blocked = store.list_agent_actions(Some("blocked"), 5).unwrap();
        assert_eq!(blocked[0].summary, "An agent was blocked for alex@example.com");
    }

    #[test]
    fn undo_of_a_batch_waits_for_approval() {
        let store = store();
        store
            .grant_agent_access("demo:personal", &["triage".into()], AGENT_DISCLOSURE_VERSION)
            .unwrap();
        store
            .batch_triage("user-spam", "spam", &["demo:1".into(), "demo:2".into()])
            .unwrap();
        let request = AgentApprovalRequest::UndoTriage { operation_id: "user-spam".into() };
        let pending = store.request_agent_approval(&request, None).unwrap();
        assert_eq!(pending.summary, "Undo moving 2 conversations to Spam");
        assert!(!store.list_threads(None, 50, 0).unwrap().iter().any(|t| t.id == "demo:1"));
        store.approve_agent_action(&pending.id, 10).unwrap();
        assert!(store.list_threads(None, 50, 0).unwrap().iter().any(|t| t.id == "demo:1"));
        assert!(store.request_agent_approval(&request, None).unwrap().status == "approved");
        assert!(store
            .request_agent_approval(&AgentApprovalRequest::UndoTriage { operation_id: "missing".into() }, None)
            .is_err());
    }

    #[test]
    fn requests_close_when_their_conversations_disappear_and_html_is_refused() {
        let store = store();
        store
            .grant_agent_access("demo:personal", &["triage".into(), "send".into()], AGENT_DISCLOSURE_VERSION)
            .unwrap();
        let pending = store
            .request_agent_approval(
                &AgentApprovalRequest::BatchTriage {
                    operation_id: "vanish".into(),
                    kind: "archive".into(),
                    thread_ids: vec!["demo:3".into()],
                },
                None,
            )
            .unwrap();
        store.remove_message("demo:personal", "demo:3").unwrap();
        let error = store.approve_agent_action(&pending.id, 10).unwrap_err().to_string();
        assert!(error.contains("no longer in the local cache"), "{error}");
        assert_eq!(store.agent_action(&pending.id).unwrap().unwrap().status, "failed");
        // Formatted drafts cannot be shown for review, so they are refused.
        let id = draft(&store);
        store
            .lock()
            .unwrap()
            .execute("UPDATE messages SET body_html='<p>hidden <a href=x>link</a></p>' WHERE id=?1", [&id])
            .unwrap();
        let error = store.request_agent_approval(&send(&id), None).unwrap_err().to_string();
        assert!(error.contains("formatted (HTML)"), "{error}");
    }

    #[test]
    fn waiting_requests_are_listed_oldest_first() {
        let store = store();
        store
            .grant_agent_access("demo:personal", &["send".into()], AGENT_DISCLOSURE_VERSION)
            .unwrap();
        let first = store.request_agent_approval(&send(&draft(&store)), None).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(3));
        let second = store.request_agent_approval(&send(&draft(&store)), None).unwrap();
        let pending = store.list_agent_actions(Some("pending"), 10).unwrap();
        assert_eq!(
            pending.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(),
            vec![first.id.as_str(), second.id.as_str()]
        );
    }

    #[test]
    fn an_edited_draft_asked_again_replaces_the_stale_request() {
        let store = store();
        store
            .grant_agent_access("demo:personal", &["send".into()], AGENT_DISCLOSURE_VERSION)
            .unwrap();
        let id = draft(&store);
        let first = store.request_agent_approval(&send(&id), None).unwrap();
        store.update_draft(&id, "maya@example.org", "Agent draft", "Fixed a typo").unwrap();
        let second = store.request_agent_approval(&send(&id), None).unwrap();
        assert_ne!(first.id, second.id);
        assert_eq!(second.details["draft"]["body_plain"], "Fixed a typo");
        // The superseded request is kept as routine history with a clear status.
        let replaced = store.agent_action(&first.id).unwrap().unwrap();
        assert_eq!(replaced.status, "replaced");
        assert!(store.approve_agent_action(&first.id, 10).unwrap_err().to_string().contains("replaced"));
        store.approve_agent_action(&second.id, 10).unwrap();
    }

    #[test]
    fn routine_activity_cannot_push_out_decisions() {
        let store = store();
        store
            .grant_agent_access("demo:personal", &["send".into()], AGENT_DISCLOSURE_VERSION)
            .unwrap();
        let decided = store.request_agent_approval(&send(&draft(&store)), None).unwrap();
        store.reject_agent_action(&decided.id).unwrap();
        for _ in 0..(HISTORY_LIMIT + 20) {
            store
                .record_agent_activity("archive_thread", &["demo:personal".into()], "Archived", json!({}), None)
                .unwrap();
        }
        assert!(store.agent_action(&decided.id).unwrap().is_some());
        assert!(store.list_agent_actions(Some("done"), 1000).unwrap().len() <= 2 * HISTORY_LIMIT as usize + 51);
        let scoped = store
            .list_agent_actions_for_accounts(Some("rejected"), &["demo:personal".into()], 10)
            .unwrap();
        assert_eq!(scoped.len(), 1);
        assert!(store.list_agent_actions_for_accounts(None, &["other".into()], 10).unwrap().is_empty());
    }

    #[test]
    fn a_request_decided_after_long_waiting_is_kept_and_reported() {
        let store = store();
        store
            .grant_agent_access("demo:personal", &["send".into()], AGENT_DISCLOSURE_VERSION)
            .unwrap();
        let old = store.request_agent_approval(&send(&draft(&store)), None).unwrap();
        for _ in 0..(HISTORY_LIMIT + 5) {
            let newer = store.request_agent_approval(&send(&draft(&store)), None).unwrap();
            store.reject_agent_action(&newer.id).unwrap();
        }
        assert_eq!(store.approve_agent_action(&old.id, 10).unwrap().status, "approved");
        assert!(store.agent_action(&old.id).unwrap().is_some());
    }

    #[test]
    fn stale_send_requests_close_and_frequent_activity_coalesces() {
        let store = store();
        store
            .grant_agent_access("demo:personal", &["send".into()], AGENT_DISCLOSURE_VERSION)
            .unwrap();
        let id = draft(&store);
        let pending = store.request_agent_approval(&send(&id), None).unwrap();
        store.schedule_draft(&id, now_ms() + 600_000, "scheduled").unwrap();
        assert_eq!(store.agent_pending_summary().unwrap()["count"], 0);
        assert_eq!(store.agent_action(&pending.id).unwrap().unwrap().status, "failed");
        for _ in 0..3 {
            store
                .record_agent_activity("sync_account", &["demo:personal".into()], "Synced", json!({}), None)
                .unwrap();
        }
        let synced = store.list_agent_actions(Some("done"), 10).unwrap();
        let synced: Vec<_> = synced.iter().filter(|a| a.kind == "sync_account").collect();
        assert_eq!(synced.len(), 1);
        assert_eq!(synced[0].details["count"], 3);
        // A step reports its latest progress, and decided entries sort by decision time.
        store
            .record_agent_activity("step_sender_cleanup", &["demo:personal".into()], "Advanced (0)", json!({}), None)
            .unwrap();
        store
            .record_agent_activity("step_sender_cleanup", &["demo:personal".into()], "Advanced (40)", json!({}), None)
            .unwrap();
        let history = store.list_agent_actions(Some("history"), 10).unwrap();
        assert_eq!(history[0].summary, "Advanced (40)");
    }

    #[test]
    fn cards_get_bare_addresses_and_subjects_without_control_characters() {
        let store = store();
        store
            .grant_agent_access("demo:personal", &["send".into()], AGENT_DISCLOSURE_VERSION)
            .unwrap();
        let spoof = store
            .compose_draft("demo:personal", "\"maya@example.org\" <attacker@evil.example>", "Hello", "Body")
            .unwrap()
            .id;
        let request = store.request_agent_approval(&send(&spoof), None).unwrap();
        assert_eq!(request.details["addresses"]["to"][0]["address"], "attacker@evil.example");
        assert_eq!(request.details["addresses"]["to"][0]["name"], "maya@example.org");
        assert!(request.summary.ends_with("to attacker@evil.example"), "{}", request.summary);
        for subject in ["Hi\nWhen  now", "Hi\u{2028}When  now", "Invoice \u{202e}fdp.exe"] {
            let id = store.compose_draft("demo:personal", "maya@example.org", subject, "Body").unwrap().id;
            assert!(store.request_agent_approval(&send(&id), None).unwrap_err().to_string().contains("cannot show for review"), "{subject:?}");
        }
        let id = draft(&store);
        store.add_draft_attachment(&id, "report\u{202e}fdp.command", "text/plain", b"x").unwrap();
        assert!(store.request_agent_approval(&send(&id), None).unwrap_err().to_string().contains("draft attachment contains"));
        assert_eq!(address_parts(&["\"Maya, Chen\" <maya@example.org>".into()])[0]["address"], "maya@example.org");
        assert_eq!(address_parts(&["Broken \u{202e} <x@example.org".into()])[0]["name"], Value::Null);
    }

    #[test]
    fn a_requested_attachment_can_be_saved_only_while_waiting() {
        let store = store();
        store
            .grant_agent_access("demo:personal", &["send".into()], AGENT_DISCLOSURE_VERSION)
            .unwrap();
        let id = draft(&store);
        let file = store.add_draft_attachment(&id, "notes.txt", "text/plain", b"hello").unwrap();
        let request = store.request_agent_approval(&send(&id), None).unwrap();
        let (name, mime, bytes) = store.agent_request_attachment(&request.id, &file.id).unwrap();
        assert_eq!((name.as_str(), mime.as_str(), bytes.as_slice()), ("notes.txt", "text/plain", b"hello".as_slice()));
        assert!(store.agent_request_attachment(&request.id, "other").is_err());
        store.reject_agent_action(&request.id).unwrap();
        assert!(store.agent_request_attachment(&request.id, &file.id).is_err());
    }

    #[test]
    fn requests_that_go_stale_by_themselves_cannot_push_out_decisions() {
        let store = store();
        store
            .grant_agent_access("demo:personal", &["send".into()], AGENT_DISCLOSURE_VERSION)
            .unwrap();
        let decided = store.request_agent_approval(&send(&draft(&store)), None).unwrap();
        store.reject_agent_action(&decided.id).unwrap();
        for _ in 0..(HISTORY_LIMIT + 10) {
            let id = draft(&store);
            store.request_agent_approval(&send(&id), None).unwrap();
            store.discard_draft(&id).unwrap();
            store.agent_pending_summary().unwrap();
        }
        assert_eq!(store.agent_action(&decided.id).unwrap().unwrap().status, "rejected");
    }

    #[test]
    fn other_stale_requests_close_and_new_refusal_reasons_get_new_entries() {
        let store = store();
        store
            .grant_agent_access("demo:personal", &["triage".into()], AGENT_DISCLOSURE_VERSION)
            .unwrap();
        store.batch_triage("user-op", "archive", &["demo:1".into()]).unwrap();
        let undo = store
            .request_agent_approval(&AgentApprovalRequest::UndoTriage { operation_id: "user-op".into() }, None)
            .unwrap();
        store.undo_triage("user-op").unwrap();
        assert_eq!(store.agent_pending_summary().unwrap()["count"], 0);
        assert_eq!(store.agent_action(&undo.id).unwrap().unwrap().details["closed_by"], "system");
        store.record_agent_block(&["demo:personal".into()], "get_message", "Agent access is off", None).unwrap();
        store
            .record_agent_block(&["demo:personal".into()], "send_draft", "does not include “Ask to send”", None)
            .unwrap();
        assert_eq!(store.list_agent_actions(Some("blocked"), 10).unwrap().len(), 2);
        assert_eq!(sanitize_client(Some("\u{202e}liamE")), Some("liamE".into()));
    }

    #[test]
    fn invisible_characters_cannot_hide_content_from_the_card() {
        let store = store();
        store
            .grant_agent_access("demo:personal", &["send".into()], AGENT_DISCLOSURE_VERSION)
            .unwrap();
        let tagged: String = "Thanks, I'll take a look."
            .chars()
            .chain("482913".chars().map(|c| char::from_u32(0xE0000 + c as u32).unwrap()))
            .collect();
        for (to, subject, body) in [
            ("maya@example.org", "Re: plans", tagged.as_str()),
            ("maya@example.org", "Re: plans", "Thanks\u{200b}\u{200b}"),
            ("maya@example.org", "Re: plans", "Thanks\u{0007}"),
            ("maya@example.org", "Re: plans", "Th\u{200d}a\u{200c}nks"),
            ("maya@example.org", "Re: plans", "Thanks\u{1D173}"),
            ("maya@example.org", "Re: plans", "Thanks\u{E0080}"),
            ("maya@example.org", "Re: plans", "see you \u{2014}\u{FE0F} \u{201C}Tuesday\u{201D}\u{FE0E}"),
            ("maya@example.org", "Re:\u{2060} plans", "Thanks"),
            ("maya@example.org", "Re:\n plans", "Thanks"),
            ("\"Maya\u{200b}\" <maya@example.org>", "Re: plans", "Thanks"),
        ] {
            let id = store.compose_draft("demo:personal", to, subject, body).unwrap().id;
            let error = store.request_agent_approval(&send(&id), None).unwrap_err().to_string();
            assert!(error.contains("cannot show for review (U+"), "{to:?} {subject:?} {body:?}: {error}");
        }
        // Ordinary text, emoji, keycaps, flags and scripts that need joiners pass,
        // and the card is told how many invisible characters they use.
        for (subject, body, count) in [
            ("Re:\tplans", "Hi Maya,\n\n\tThanks!\r\nAlex", 0),
            ("Plans", "Family 👨\u{200d}👩\u{200d}👧 and ❤\u{fe0f}", 3),
            ("Plans", "❤\u{FE0F}\u{200D}🔥 🏳\u{FE0F}\u{200D}🌈 1\u{FE0F}\u{20E3} ©\u{FE0F}", 6),
            ("Plans", "می\u{200c}خواهم", 1),
            ("Plans", "ශ්\u{200D}රී and র\u{200D}্য", 2),
            ("Plans", "🏴\u{E0067}\u{E0062}\u{E0073}\u{E0063}\u{E0074}\u{E007F}", 6),
        ] {
            let id = store.compose_draft("demo:personal", "maya@example.org", subject, body).unwrap().id;
            let request = store.request_agent_approval(&send(&id), None);
            assert!(request.is_ok(), "{body:?}: {request:?}");
            assert_eq!(request.unwrap().details["invisible_count"], count, "{body:?}");
        }
    }

    #[test]
    fn pending_requests_are_bounded() {
        let store = store();
        store
            .grant_agent_access("demo:personal", &["send".into()], AGENT_DISCLOSURE_VERSION)
            .unwrap();
        for _ in 0..MAX_PENDING {
            store.request_agent_approval(&send(&draft(&store)), None).unwrap();
        }
        let error = store.request_agent_approval(&send(&draft(&store)), None).unwrap_err();
        assert!(error.to_string().contains("Too many"));
    }
}
