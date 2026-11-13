//! Desktop-app operations for agent access: the consent disclosure, per-account
//! consent, and decisions on pending agent requests. Both native shells render
//! the disclosure from this one source so their wording cannot drift.
//!
//! These operations are deliberately absent from mail-mcp's access policy, so
//! an agent can neither grant itself access nor approve its own requests.
use mail_core::{AGENT_DISCLOSURE_VERSION, AGENT_GRANTS, MailStore, agent_grant_label as grant_label};
use serde_json::{Value, json};

/// "this Mac" in the macOS engine, "this computer" elsewhere.
const THIS_COMPUTER: &str = if cfg!(target_os = "macos") { "this Mac" } else { "this computer" };

/// The checkbox for a grant in the turn-on sheet: its label, a short note shown
/// after it, whether the note is a warning, and the full explanation (tooltip
/// and accessibility description).
fn grant_checkbox(grant: &str) -> (&'static str, Option<&'static str>, bool, &'static str) {
    match grant {
        "draft" => (
            "Write drafts",
            None,
            false,
            "Create and edit local drafts, draft attachments, snippets and signatures. Snippets are shared by all accounts.",
        ),
        "triage" => (
            "Archive, mark read and snooze",
            Some("you approve bulk changes"),
            false,
            "Archive, mark read or unread, snooze and set reminders one conversation at a time. Changes to many conversations wait for your approval.",
        ),
        "send" => (
            "Ask to send mail",
            Some("you approve each one"),
            false,
            "Ask to send or schedule drafts; every send waits for your approval. Agents can also cancel a scheduled send, including one you scheduled, which returns it to Drafts.",
        ),
        "sync" => (
            "Fetch new mail",
            None,
            false,
            "Fetch new mail from your provider and upload changes already queued.",
        ),
        "calendar_write" => (
            "Change calendar events",
            Some("no approval; guests get notified"),
            true,
            "Create and edit events and reply to invitations. Guests are notified, and these changes don't wait for your approval.",
        ),
        _ => ("", None, false, ""),
    }
}

/// The turn-on sheet shown before agent access is turned on for one account,
/// and again when its permissions are edited. Every optional grant starts
/// unchecked: reading is the only default. The sheet names what agents can
/// read (the lead, and the full list under `details`), says the AI provider's
/// terms apply, and says how to turn access off.
pub(crate) fn disclosure(email: &str) -> Value {
    json!({
        "version": AGENT_DISCLOSURE_VERSION,
        "title": format!("Let agents use {email}?"),
        "lead": format!("Agents can read this account's mail, drafts and calendar on {THIS_COMPUTER}."),
        "read": {"label": "Read mail and calendar", "note": "required"},
        "grants": AGENT_GRANTS.iter().map(|grant| {
            let (label, note, warning, detail) = grant_checkbox(grant);
            json!({
                "id": grant,
                "label": label,
                "note": note,
                "warning": warning,
                "detail": detail,
                "short_label": grant_label(grant),
            })
        }).collect::<Vec<_>>(),
        "provider_terms": "What an agent reads goes to its AI provider, such as Anthropic or OpenAI, under their terms.",
        "revoke": "You can turn this off anytime in Agent access.",
        "details_title": "What exactly can agents see?",
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
        "confirm": "Turn on",
        "change_confirm": "Save",
    })
}

/// "Read · Drafts · Ask to send": the account row's summary of a consent.
fn grant_summary(grants: &[String]) -> String {
    std::iter::once("Read")
        .chain(grants.iter().map(|g| grant_label(g)))
        .collect::<Vec<_>>()
        .join(" · ")
}

/// Everything the Agent access window shows, in one read.
pub(crate) fn overview(store: &MailStore, history_limit: u32) -> Result<Value, String> {
    let mut value = status(store)?;
    value["requests"] = serde_json::to_value(
        store.list_agent_actions(Some("pending"), 50).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    value["history"] = serde_json::to_value(
        store
            .list_agent_actions(Some("history"), history_limit.clamp(1, 200))
            .map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    Ok(value)
}

/// Accounts with their consent state, for the Agent access panel.
pub(crate) fn status(store: &MailStore) -> Result<Value, String> {
    let consents = store.agent_consents().map_err(|e| e.to_string())?;
    let accounts = store
        .list_accounts()
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|account| {
            let consent = consents.iter().find(|c| c.account_id == account.id);
            let labels = consent
                .map(|c| {
                    std::iter::once("Read".to_string())
                        .chain(c.grants.iter().map(|g| grant_label(g).to_string()))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            json!({
                "account_id": account.id,
                "email": account.email,
                "display_name": account.display_name,
                "provider": account.provider,
                "consent": consent,
                "current": consent.is_some_and(|c| c.is_current()),
                "grant_labels": labels,
                "grant_summary": consent.map(|c| grant_summary(&c.grants)),
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "disclosure_version": AGENT_DISCLOSURE_VERSION,
        "accounts": accounts,
        "pending": store.agent_pending_summary().map_err(|e| e.to_string())?,
    }))
}
