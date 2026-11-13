//! Desktop-app operations for agent access: the consent disclosure, per-account
//! consent, and decisions on pending agent requests. Both native shells render
//! the disclosure from this one source so their wording cannot drift.
//!
//! These operations are deliberately absent from mail-mcp's access policy, so
//! an agent can neither grant itself access nor approve its own requests.
use mail_core::{AGENT_DISCLOSURE_VERSION, AGENT_GRANTS, MailStore, agent_grant_label as grant_label};
use serde_json::{Value, json};

fn grant_detail(grant: &str) -> &'static str {
    match grant {
        "draft" => "Create and edit local drafts, draft attachments, snippets and signatures. Snippets are shared by all accounts.",
        "triage" => {
            "Archive, mark read or unread, snooze and set reminders. Bulk changes wait for your approval."
        }
        "send" => "Ask to send or schedule drafts; every send waits for your approval. Also lets agents cancel a scheduled send, including one you scheduled, which returns it to Drafts.",
        "sync" => "Fetch new mail from the provider and upload changes already queued.",
        "calendar_write" => {
            "Create and edit events and reply to invitations. Guests are notified, and these changes do not wait for approval."
        }
        _ => "",
    }
}

/// The disclosure shown before agent access is turned on for one account.
pub(crate) fn disclosure(email: &str) -> Value {
    json!({
        "version": AGENT_DISCLOSURE_VERSION,
        "title": format!("Turn on agent access for {email}?"),
        "change_title": format!("Change agent access for {email}?"),
        "intro": "AI agents and other tools that you connect to Email's local MCP server, such as Codex or Claude Code, will be able to read this account's data stored on this computer:",
        "shared_note": "While any account has agent access, agents can also read your snippets and saved splits, and your bookings if Scheduling is connected.",
        "data_categories": [
            "Messages: senders, recipients, subjects, bodies and attachments. An attachment not yet on this computer is downloaded from your mail provider when an agent asks for it.",
            "Drafts, labels, and snoozed, scheduled and queued mail",
            "Calendar invitations and connected calendar events",
            "Recipient suggestions, follow-up reminders and signatures",
            "Open-tracking activity for messages you chose to track",
        ],
        "provider_terms": "Anything an agent reads is sent to that agent and to the AI provider behind it. From then on, that provider's terms and privacy policy apply to the data, not Email's privacy policy. Email's developer never receives it.",
        "safeguards": "Sending mail and bulk changes always wait for your approval in Agent access. Email marks message content as untrusted when it passes it to an agent, but an agent can still be misled by what it reads.",
        "grants_heading": "Also allow agents to:",
        "read_label": "Read this account's data (required)",
        "grants": AGENT_GRANTS.iter().map(|grant| json!({
            "id": grant,
            "label": grant_label(grant),
            "detail": grant_detail(grant),
        })).collect::<Vec<_>>(),
        "revoke": "You can turn agent access off at any time in Agent access. It applies to the agent's next request. The permissions in each MCP registration still apply as well.",
        "confirm": "Turn On Agent Access",
        "change_confirm": "Save",
    })
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
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "disclosure_version": AGENT_DISCLOSURE_VERSION,
        "accounts": accounts,
        "pending": store.agent_pending_summary().map_err(|e| e.to_string())?,
    }))
}
