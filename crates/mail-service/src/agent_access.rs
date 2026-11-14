//! Desktop-app operations for agent access: the turn-on sheet, per-account
//! consent, and the requests agents make (sends, bulk changes, sender cleanup,
//! and refused agents that want an account). Both native shells render these
//! from this one source, so their wording and behavior cannot drift.
//!
//! These operations are deliberately absent from mail-mcp's access policy, so
//! an agent can neither grant itself access nor approve its own requests.
use mail_core::{AGENT_DISCLOSURE_VERSION, AgentAction, MailStore, agent_grant_label as grant_label};
use serde_json::{Value, json};

/// "this Mac" in the macOS engine, "this computer" elsewhere.
const THIS_COMPUTER: &str = if cfg!(target_os = "macos") { "this Mac" } else { "this computer" };

/// The three choices in the turn-on sheet and the settings popup. Reading is
/// implied by consent; each level adds grants to the one before.
pub(crate) const LEVELS: [(&str, &str, &str, &[&str]); 3] = [
    ("read", "Read only", "Read mail and calendar.", &["sync"]),
    (
        "organize",
        "Read and organize",
        "Also archive, mark read and snooze. You OK bulk changes.",
        &["sync", "triage"],
    ),
    (
        "write",
        "Read, organize and write",
        "Also write drafts and ask to send. You OK every email.",
        &["sync", "triage", "draft", "send"],
    ),
];

/// The individual switches under Custom…, in the order of the levels.
const CUSTOM: [&str; 5] = ["sync", "triage", "draft", "send", "calendar_write"];

/// A grant's switch under Custom…: its label, a short note, whether the note is
/// a warning, and the full explanation (tooltip and accessibility description).
fn grant_switch(grant: &str) -> (&'static str, Option<&'static str>, bool, &'static str) {
    match grant {
        "draft" => (
            "Write drafts",
            None,
            false,
            "Create and edit local drafts, draft attachments, snippets and signatures. Snippets are shared by all accounts.",
        ),
        "triage" => (
            "Archive, mark read and snooze",
            Some("you OK bulk changes"),
            false,
            "Archive, mark read or unread, snooze and set reminders one conversation at a time. Changes to many conversations wait for you.",
        ),
        "send" => (
            "Ask to send mail",
            Some("you OK every email"),
            false,
            "Ask to send or schedule drafts; every send waits for you. Agents can also cancel a scheduled send, including one you scheduled, which returns it to Drafts.",
        ),
        "sync" => ("Fetch new mail", None, false, "Fetch new mail from your provider and upload changes already queued."),
        "calendar_write" => (
            "Change calendar events",
            Some("guests get notified"),
            true,
            "Create and edit events and reply to invitations. Guests are notified, and these changes don't wait for you.",
        ),
        _ => ("", None, false, ""),
    }
}

fn sorted(grants: &[String]) -> Vec<String> {
    let mut grants = grants.to_vec();
    grants.sort();
    grants.dedup();
    grants
}

/// The level whose grants are exactly these, or "custom".
pub(crate) fn level_of(grants: &[String]) -> &'static str {
    let grants = sorted(grants);
    LEVELS
        .iter()
        .find(|(_, _, _, level)| sorted(&level.iter().map(|g| g.to_string()).collect::<Vec<_>>()) == grants)
        .map_or("custom", |(id, ..)| id)
}

/// The smallest level that covers `current` plus `needed`, and those grants.
fn level_covering(current: &[String], needed: &[String]) -> (&'static str, Vec<String>) {
    let mut wanted = current.to_vec();
    wanted.extend(needed.iter().cloned());
    let wanted = sorted(&wanted);
    for (id, _, _, level) in LEVELS {
        if wanted.iter().all(|g| level.contains(&g.as_str())) {
            return (id, level.iter().map(|g| g.to_string()).collect());
        }
    }
    ("custom", wanted)
}

/// The turn-on sheet: shown when an account is turned on (from the settings
/// popup or Allow… on a request) and for Custom…. Read only is the default.
/// It names every kind of data agents can read (`details`), says the AI
/// provider's terms apply and how to turn access off (`note`).
pub(crate) fn disclosure(email: &str) -> Value {
    json!({
        "version": AGENT_DISCLOSURE_VERSION,
        "title": format!("Let agents use {email}?"),
        "levels": LEVELS.iter().map(|(id, label, sub, grants)| json!({
            "id": id, "label": label, "sub": sub, "grants": grants,
        })).collect::<Vec<_>>(),
        "default_level": "read",
        "custom": CUSTOM.iter().map(|grant| {
            let (label, note, warning, detail) = grant_switch(grant);
            json!({"id": grant, "label": label, "note": note, "warning": warning, "detail": detail})
        }).collect::<Vec<_>>(),
        "note": "What an agent reads goes to its AI provider, like Anthropic or OpenAI. You can turn this off anytime.",
        "custom_link": "Custom…",
        "details_link": "What can agents see?",
        "details": [
            format!("Messages: senders, recipients, subjects, bodies and attachments. An attachment that isn't on {THIS_COMPUTER} yet is downloaded from your mail provider when an agent asks for it."),
            "Drafts and labels".to_string(),
            "Snoozed, scheduled and queued mail".to_string(),
            "Calendar invitations and connected calendar events".to_string(),
            "Recipient suggestions, follow-up reminders and signatures".to_string(),
            "Open-tracking activity for messages you chose to track".to_string(),
            "While any account is on: your snippets and saved splits, and your bookings if Scheduling is connected".to_string(),
            "Email marks mail content as untrusted when it hands it to an agent, but an agent can still be misled by what it reads.".to_string(),
            "Email's developer never receives any of it. Once an agent reads something, its provider's terms and privacy policy apply, not Email's.".to_string(),
            "An agent's own MCP registration can limit it further. Agents connected from Agent access can do what you allow here.".to_string(),
        ],
        "cancel": "Cancel",
        "confirm": "Allow",
        "change_confirm": "Save",
    })
}

/// Everything the Agent access window shows, in one read.
pub(crate) fn overview(store: &MailStore, history_limit: u32) -> Result<Value, String> {
    let mut value = status(store)?;
    value["history"] = serde_json::to_value(
        store
            .list_agent_actions(Some("history"), history_limit.clamp(1, 200))
            .map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    Ok(value)
}

/// Accounts with their consent state, for the Agent access window.
pub(crate) fn status(store: &MailStore) -> Result<Value, String> {
    let consents = store.agent_consents().map_err(|e| e.to_string())?;
    let accounts = store
        .list_accounts()
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|account| {
            let consent = consents.iter().find(|c| c.account_id == account.id).filter(|c| c.is_current());
            json!({
                "account_id": account.id,
                "email": account.email,
                "display_name": account.display_name,
                "provider": account.provider,
                "consent": consent,
                "current": consent.is_some(),
                "level": consent.map_or("off", |c| level_of(&c.grants)),
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "disclosure_version": AGENT_DISCLOSURE_VERSION,
        "levels": LEVELS.iter().map(|(id, label, _, grants)| json!({"id": id, "label": label, "grants": grants})).collect::<Vec<_>>(),
        "accounts": accounts,
        "pending": summary(store)?,
    }))
}

/// The agent's name as people know it: Claude Code, Codex, or what the MCP
/// client reported (self-reported, informational).
pub(crate) fn agent_name(client: Option<&str>) -> String {
    let Some(client) = client.map(str::trim).filter(|c| !c.is_empty()) else {
        return "An agent".into();
    };
    let lower = client.to_ascii_lowercase();
    if lower.starts_with("claude-code") || lower == "claude code" {
        "Claude Code".into()
    } else if lower.starts_with("codex") {
        "Codex".into()
    } else if lower == "claude-ai" {
        "Claude".into()
    } else {
        client.into()
    }
}

fn plural(count: u64, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// A display name for a formatted address, or the address.
fn person(details: &Value, key: &str) -> Option<String> {
    let first = details["addresses"][key].as_array().and_then(|parts| parts.first())?;
    first["name"]
        .as_str()
        .filter(|n| !n.trim().is_empty())
        .or_else(|| first["address"].as_str())
        .map(str::to_owned)
}

fn sender_name(sender: &str) -> String {
    match sender.find('<') {
        Some(open) => {
            let name = sender[..open].trim().trim_matches('"');
            if name.is_empty() { sender.trim_matches(|c| c == '<' || c == '>').to_string() } else { name.to_string() }
        }
        None => sender.to_string(),
    }
}

fn one_line(text: &str, limit: usize) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() > limit {
        format!("{}…", line.chars().take(limit).collect::<String>().trim_end())
    } else {
        line
    }
}

/// A waiting approval as a row and reader in the main window.
fn approval(store: &MailStore, action: &AgentAction) -> Result<Value, String> {
    let details = &action.details;
    let agent = agent_name(action.client.as_deref());
    let count = details["count"].as_u64().unwrap_or(0);
    let threads = || -> Result<Vec<Value>, String> {
        let sample = details["threads"].as_array().cloned().unwrap_or_default();
        let ids: Vec<String> = sample.iter().filter_map(|t| t["id"].as_str().map(str::to_owned)).collect();
        let rows = store.agent_thread_rows(&ids).map_err(|e| e.to_string())?;
        // Conversations that left the cache still show what the request saved.
        Ok(sample
            .iter()
            .map(|t| {
                let mut row = rows.iter().find(|r| r["id"] == t["id"]).cloned().unwrap_or_else(|| t.clone());
                if let Some(snippet) = row["snippet"].as_str() {
                    row["snippet"] = json!(one_line(snippet, 140));
                }
                row
            })
            .collect())
    };
    let senders = |rows: &[Value]| {
        let mut names: Vec<String> = Vec::new();
        for row in rows {
            let name = sender_name(row["sender"].as_str().unwrap_or_default());
            if !name.is_empty() && !names.contains(&name) {
                names.push(name);
            }
        }
        one_line(&names.join(", "), 90)
    };
    let mut row = json!({
        "id": action.id,
        "kind": action.kind,
        "agent": agent,
        "client": action.client,
        "created_at_ms": action.created_at_ms,
        "account_ids": action.account_ids,
        "details": details,
        "can_edit": false,
        "note": Value::Null,
    });
    let (kind, title, preview, ask, approve, reject) = match action.kind.as_str() {
        "send_draft" | "queue_send_draft" | "schedule_draft" => {
            let draft = &details["draft"];
            let subject = draft["subject"].as_str().filter(|s| !s.is_empty()).unwrap_or("(no subject)");
            let to = person(details, "to").unwrap_or_else(|| "no one".into());
            let body = one_line(draft["body_plain"].as_str().unwrap_or_default(), 80);
            row["can_edit"] = json!(true);
            let scheduled = action.kind == "schedule_draft";
            (
                "send",
                format!("Send: {subject}"),
                if body.is_empty() { format!("To {to}") } else { format!("To {to} · {body}") },
                if scheduled { "wants to schedule this email" } else { "wants to send this email" }.to_string(),
                if scheduled { "Schedule" } else { "Send" }.to_string(),
                "Don't send".to_string(),
            )
        }
        "batch_triage" => {
            let rows = threads()?;
            let preview = senders(&rows);
            row["threads"] = json!(rows);
            let n = plural(count, "conversation", "conversations");
            let (ask, approve, reject, undoable) = match details["kind"].as_str().unwrap_or_default() {
                "archive" => (format!("wants to archive {n}"), format!("Archive {count}"), "Don't archive", true),
                "restore" => (format!("wants to move {n} back to the inbox"), format!("Move {count}"), "Don't move", true),
                "spam" => (format!("wants to move {n} to Spam"), format!("Move {count} to Spam"), "Don't move", true),
                "read" => (format!("wants to mark {n} read"), format!("Mark {count} read"), "Don't mark read", false),
                _ => (format!("wants to mark {n} unread"), format!("Mark {count} unread"), "Don't mark unread", false),
            };
            if undoable {
                row["note"] = json!("You can undo this afterwards.");
            }
            ("bulk", action.summary.clone(), preview, ask, approve, reject.to_string())
        }
        "undo_triage" => {
            let rows = threads()?;
            let preview = senders(&rows);
            row["threads"] = json!(rows);
            let mut summary = action.summary.clone();
            if let Some(first) = summary.get(..1) {
                summary = first.to_lowercase() + &summary[1..];
            }
            ("bulk", action.summary.clone(), preview, format!("wants to {summary}"), "Undo".into(), "Don't undo".into())
        }
        "confirm_sender_cleanup" => {
            let matched = details["matched"].as_u64().unwrap_or(0);
            let sender = details["sender"].as_str().unwrap_or_default().to_string();
            row["threads"] = json!(details["sample_subjects"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(|subject| json!({"sender": sender, "subject": subject}))
                .collect::<Vec<_>>());
            row["note"] = json!("This can't be undone.");
            (
                "bulk",
                action.summary.clone(),
                format!("{} from {sender}", plural(matched, "unread message", "unread messages")),
                format!("wants to mark {} from {sender} read", plural(matched, "message", "messages")),
                format!("Mark {matched} read"),
                "Don't mark read".into(),
            )
        }
        _ => ("bulk", action.summary.clone(), String::new(), "wants your OK".into(), "Approve".into(), "Don't".into()),
    };
    row["type"] = json!(kind);
    row["title"] = json!(title);
    row["preview"] = json!(preview);
    row["ask"] = json!(ask);
    row["approve_label"] = json!(approve);
    row["reject_label"] = json!(reject);
    Ok(row)
}

/// A refused agent that still wants an account, as a request.
fn access(request: &Value) -> Value {
    let email = request["account_email"].as_str().unwrap_or_default();
    let missing: Vec<String> = request["missing_grants"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect();
    let current: Vec<String> = request["current_grants"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect();
    let preview = if request["needs_access"].as_bool() == Some(true) || missing.is_empty() {
        "Agent access is off for this account".to_string()
    } else {
        let labels: Vec<String> = missing.iter().map(|g| format!("“{}”", grant_label(g))).collect();
        format!("{} is off for this account", labels.join(" and "))
    };
    let (level, grants) = level_covering(&current, &missing);
    json!({
        "id": request["id"],
        "kind": "access",
        "type": "access",
        "agent": agent_name(request["client"].as_str()),
        "client": request["client"],
        "created_at_ms": request["last_at_ms"],
        "account_ids": [request["account_id"]],
        "account_id": request["account_id"],
        "account_email": email,
        "title": format!("Wants to use {email}"),
        "preview": preview,
        "ask": format!("wants to use {email}"),
        "approve_label": "Allow…",
        "reject_label": "Ignore",
        "can_edit": false,
        "note": Value::Null,
        "level": level,
        "grants": grants,
        "changing": request["needs_access"].as_bool() == Some(false),
    })
}

/// Everything waiting for the user, oldest first: approvals, then refused
/// agents that want an account.
pub(crate) fn requests(store: &MailStore) -> Result<Vec<Value>, String> {
    let mut rows = store
        .list_agent_actions(Some("pending"), 50)
        .map_err(|e| e.to_string())?
        .iter()
        .map(|action| approval(store, action))
        .collect::<Result<Vec<_>, _>>()?;
    rows.extend(store.agent_access_requests().map_err(|e| e.to_string())?.iter().map(access));
    Ok(rows)
}

/// The notification for a request: "Claude Code wants to send “Design review
/// time” to Maya Chen".
fn notice(row: &Value) -> String {
    let agent = row["agent"].as_str().unwrap_or("An agent");
    if row["type"] == "send" {
        let subject = row["details"]["draft"]["subject"].as_str().filter(|s| !s.is_empty()).unwrap_or("(no subject)");
        let to = person(&row["details"], "to").unwrap_or_else(|| "no one".into());
        let verb = if row["kind"] == "schedule_draft" { "schedule" } else { "send" };
        return format!("{agent} wants to {verb} “{subject}” to {to}");
    }
    format!("{agent} {}", row["ask"].as_str().unwrap_or("wants your OK"))
}

/// The cheap poll for badges and notifications: the engine's pending summary
/// plus refused agents, a total for the sidebar, whether any account is on,
/// and one notice per request.
pub(crate) fn summary(store: &MailStore) -> Result<Value, String> {
    let mut summary = store.agent_pending_summary().map_err(|e| e.to_string())?;
    let rows = requests(store)?;
    let access_on = store
        .agent_consents()
        .map_err(|e| e.to_string())?
        .iter()
        .any(|c| c.is_current());
    let access_ids = rows.iter().filter(|r| r["type"] == "access").map(|r| r["id"].as_str().unwrap_or_default()).collect::<Vec<_>>().join(",");
    let revision = format!("{}-{}", summary["revision"].as_str().unwrap_or_default(), access_ids);
    summary["revision"] = json!(revision);
    summary["total"] = json!(rows.len());
    summary["access_on"] = json!(access_on);
    summary["notices"] = json!(rows
        .iter()
        .map(|r| {
            let text = notice(r);
            let agent = r["agent"].as_str().unwrap_or("An agent");
            let body = text.strip_prefix(agent).map(str::trim_start).unwrap_or(&text).to_string();
            json!({"id": r["id"], "text": text, "title": agent, "body": body})
        })
        .collect::<Vec<_>>());
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_name_grant_sets_and_cover_what_an_agent_needs() {
        let strings = |list: &[&str]| list.iter().map(|g| g.to_string()).collect::<Vec<_>>();
        assert_eq!(level_of(&strings(&["sync"])), "read");
        assert_eq!(level_of(&strings(&["triage", "sync"])), "organize");
        assert_eq!(level_of(&strings(&["send", "draft", "sync", "triage"])), "write");
        assert_eq!(level_of(&[]), "custom");
        assert_eq!(level_of(&strings(&["sync", "calendar_write"])), "custom");
        assert_eq!(level_covering(&[], &[]).0, "read");
        assert_eq!(level_covering(&strings(&["sync"]), &strings(&["send"])).0, "write");
        assert_eq!(level_covering(&[], &strings(&["triage"])).0, "organize");
        let (level, grants) = level_covering(&strings(&["sync"]), &strings(&["calendar_write"]));
        assert_eq!((level, grants), ("custom", strings(&["calendar_write", "sync"])));
    }

    #[test]
    fn agent_names_are_friendly() {
        assert_eq!(agent_name(Some("claude-code")), "Claude Code");
        assert_eq!(agent_name(Some("codex-mcp-client")), "Codex");
        assert_eq!(agent_name(Some("agent-flow-check")), "agent-flow-check");
        assert_eq!(agent_name(None), "An agent");
    }
}
