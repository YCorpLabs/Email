use crate::{
    CloudClient, CloudError, Result, attachment_mime, local_id, remote_id,
    validate_outgoing_attachments,
};
#[cfg(test)]
use base64::engine::general_purpose::STANDARD;
use base64::{
    Engine,
    engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD},
};
use lettre::{
    Message as OutgoingMessage,
    message::{Attachment, Mailbox, MultiPart, SinglePart, header::ContentType},
};
use mail_core::{Account, MailAction, Message, OutgoingAttachment, ResetPhase, SyncBatch};
use mailparse::MailAddr;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

const BASE: &str = "https://gmail.googleapis.com/gmail/v1/users/me";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GmailAttachment {
    pub part_id: String,
    pub filename: String,
    pub mime_type: String,
    pub size: u64,
    #[serde(default)]
    pub content_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GmailSendAs {
    pub email: String,
    pub display_name: String,
    pub is_default: bool,
}

pub(super) fn send_as_aliases(client: &CloudClient) -> Result<Vec<GmailSendAs>> {
    let response = client.get(&format!("{BASE}/settings/sendAs"))?;
    parse_send_as_aliases(&response)
}

fn parse_send_as_aliases(response: &Value) -> Result<Vec<GmailSendAs>> {
    let aliases = response["sendAs"]
        .as_array()
        .ok_or(CloudError::Response("sendAs"))?;
    let mut result = Vec::new();
    for alias in aliases {
        if alias["verificationStatus"].as_str() == Some("pending") {
            continue;
        }
        let Some(email) = alias["sendAsEmail"].as_str() else {
            continue;
        };
        result.push(GmailSendAs {
            email: email.to_string(),
            display_name: alias["displayName"].as_str().unwrap_or("").to_string(),
            is_default: alias["isDefault"].as_bool().unwrap_or(false),
        });
    }
    result.sort_by_key(|alias| !alias.is_default);
    Ok(result)
}

fn content_id(part: &Value) -> Option<String> {
    header(&part["headers"], "Content-ID")
        .map(|value| {
            value
                .trim()
                .trim_start_matches('<')
                .trim_end_matches('>')
                .to_string()
        })
        .filter(|value| !value.is_empty())
}

fn message_payload(client: &CloudClient, account: &Account, message_id: &str) -> Result<Value> {
    let remote = remote_id(account, "gmail-message", message_id)?;
    let mut url = url::Url::parse(&format!("{BASE}/messages/"))?;
    url.path_segments_mut()
        .map_err(|_| CloudError::Response("invalid Gmail message URL"))?
        .pop_if_empty()
        .push(remote);
    url.query_pairs_mut().append_pair("format", "full");
    client.get(url.as_str())
}

fn find_parts<'a>(
    part: &'a Value,
    path: &str,
    depth: usize,
    parts_left: &mut usize,
    output: &mut Vec<(String, &'a Value)>,
) {
    if depth > 24 || *parts_left == 0 {
        return;
    }
    *parts_left -= 1;
    if let Some(children) = part["parts"].as_array() {
        for (index, child) in children.iter().enumerate() {
            let child_path = if path.is_empty() {
                index.to_string()
            } else {
                format!("{path}.{index}")
            };
            find_parts(child, &child_path, depth + 1, parts_left, output);
        }
    }
    let named = part["filename"]
        .as_str()
        .is_some_and(|name| !name.is_empty());
    let inline_image = part["mimeType"]
        .as_str()
        .is_some_and(|mime| mime.starts_with("image/"))
        && content_id(part).is_some();
    // Calendar invitations commonly arrive as an unnamed alternative body,
    // rather than a named attachment. Keep their addressable part so explicit
    // invitation loading can obtain the ICS bytes without guessing from text.
    let calendar = part["mimeType"].as_str().is_some_and(|mime| {
        mime.split(';')
            .next()
            .unwrap_or("")
            .trim()
            .eq_ignore_ascii_case("text/calendar")
    });
    if named || inline_image || calendar {
        output.push((
            if path.is_empty() {
                "root".to_string()
            } else {
                path.to_string()
            },
            part,
        ));
    }
}

fn attachment_parts(payload: &Value) -> Vec<(String, &Value)> {
    let mut parts = Vec::new();
    let mut parts_left = 256;
    find_parts(payload, "", 0, &mut parts_left, &mut parts);
    parts
}

pub(super) fn attachments(
    client: &CloudClient,
    account: &Account,
    message_id: &str,
) -> Result<Vec<GmailAttachment>> {
    let value = message_payload(client, account, message_id)?;
    Ok(attachment_parts(&value["payload"])
        .into_iter()
        .map(|(part_id, part)| GmailAttachment {
            part_id: part["body"]["attachmentId"]
                .as_str()
                .map(|id| format!("gmail-attachment:{id}"))
                .unwrap_or(part_id),
            filename: part["filename"]
                .as_str()
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| {
                    if part["mimeType"].as_str().is_some_and(|mime| {
                        mime.split(';')
                            .next()
                            .unwrap_or("")
                            .trim()
                            .eq_ignore_ascii_case("text/calendar")
                    }) {
                        "Invitation.ics"
                    } else {
                        "Attachment"
                    }
                })
                .to_string(),
            mime_type: part["mimeType"]
                .as_str()
                .unwrap_or("application/octet-stream")
                .to_string(),
            size: part["body"]["size"].as_u64().unwrap_or(0),
            content_id: content_id(part),
        })
        .collect())
}

pub(super) fn attachment_data(
    client: &CloudClient,
    account: &Account,
    message_id: &str,
    part_id: &str,
) -> Result<Vec<u8>> {
    let attachment_id = if let Some(id) = part_id.strip_prefix("gmail-attachment:") {
        Some(id.to_string())
    } else {
        None
    };
    let data = if let Some(attachment_id) = attachment_id {
        let remote = remote_id(account, "gmail-message", message_id)?;
        let mut url = url::Url::parse(&format!("{BASE}/messages/"))?;
        url.path_segments_mut()
            .map_err(|_| CloudError::Response("invalid Gmail attachment URL"))?
            .pop_if_empty()
            .push(remote)
            .push("attachments")
            .push(&attachment_id);
        client.get(url.as_str())?["data"]
            .as_str()
            .ok_or(CloudError::Response("attachment has no data"))?
            .to_string()
    } else {
        let value = message_payload(client, account, message_id)?;
        let part = attachment_parts(&value["payload"])
            .into_iter()
            .find(|(id, _)| id == part_id)
            .map(|(_, part)| part)
            .ok_or(CloudError::Response("attachment not found"))?;
        part["body"]["data"]
            .as_str()
            .ok_or(CloudError::Response("attachment has no data"))?
            .to_string()
    };
    if data.len() > crate::MAX_ATTACHMENT_BYTES.saturating_mul(4) / 3 + 4 {
        return Err(CloudError::Response("attachment exceeds 20 MiB limit"));
    }
    let bytes = URL_SAFE
        .decode(&data)
        .or_else(|_| URL_SAFE_NO_PAD.decode(&data))
        .map_err(|_| CloudError::Response("invalid attachment encoding"))?;
    if bytes.len() > crate::MAX_ATTACHMENT_BYTES {
        return Err(CloudError::Response("attachment exceeds 20 MiB limit"));
    }
    Ok(bytes)
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
enum Cursor {
    Initial {
        page_token: Option<String>,
        history_id: String,
        #[serde(default)]
        history_page_token: Option<String>,
        #[serde(default)]
        last_fresh_check_ms: Option<u64>,
    },
    ResetInitial {
        page_token: String,
        history_id: String,
    },
    History {
        history_id: String,
        page_token: Option<String>,
    },
}

pub(super) fn identity(client: &CloudClient) -> Result<(String, String)> {
    let profile = client.get(&format!("{BASE}/profile"))?;
    let email = string(&profile, "emailAddress")?.to_string();
    Ok((email.clone(), email))
}

pub(super) fn sync(
    client: &CloudClient,
    account: &Account,
    cursor: Option<&str>,
) -> Result<SyncBatch> {
    sync_with_cache(client, account, cursor, &HashMap::new())
}

pub(super) fn sync_with_cache(
    client: &CloudClient,
    account: &Account,
    cursor: Option<&str>,
    cached: &HashMap<String, String>,
) -> Result<SyncBatch> {
    let cursor = match cursor {
        Some(value) => serde_json::from_str::<Cursor>(value)?,
        None => {
            let profile = client.get(&format!("{BASE}/profile"))?;
            Cursor::Initial {
                page_token: None,
                history_id: string(&profile, "historyId")?.into(),
                history_page_token: None,
                last_fresh_check_ms: None,
            }
        }
    };
    match cursor {
        Cursor::Initial {
            page_token,
            history_id,
            history_page_token,
            last_fresh_check_ms,
        } => {
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            if last_fresh_check_ms.is_none_or(|last| now_ms.saturating_sub(last) >= 300_000) {
                return fresh_page(
                    client,
                    account,
                    page_token,
                    history_id,
                    history_page_token,
                    now_ms,
                    cached,
                );
            }
            let changes = history_page(
                client,
                account,
                &history_id,
                history_page_token.as_deref(),
                cached,
            )?;
            if changes.reset_phase.is_some() {
                return Ok(changes);
            }
            let Cursor::History {
                history_id: next_history_id,
                page_token: next_history_page_token,
            } = serde_json::from_str::<Cursor>(&changes.next_cursor)?
            else {
                return Err(CloudError::Response("invalid Gmail history cursor"));
            };
            if !changes.messages.is_empty()
                || !changes.removed_message_ids.is_empty()
                || !changes.thread_labels.is_empty()
                || next_history_page_token.is_some()
            {
                return Ok(SyncBatch {
                    next_cursor: serde_json::to_string(&Cursor::Initial {
                        page_token,
                        history_id: next_history_id,
                        history_page_token: next_history_page_token,
                        last_fresh_check_ms,
                    })?,
                    ..changes
                });
            }
            initial_page(
                client,
                account,
                page_token.as_deref(),
                next_history_id,
                None,
            )
        }
        Cursor::ResetInitial {
            page_token,
            history_id,
        } => match initial_page(
            client,
            account,
            Some(&page_token),
            history_id,
            Some(ResetPhase::Continue),
        ) {
            Err(CloudError::ApiStatus(400 | 404)) => start_reset(client, account),
            result => result,
        },
        Cursor::History {
            history_id,
            page_token,
        } => history_page(client, account, &history_id, page_token.as_deref(), cached),
    }
}

fn fresh_page(
    client: &CloudClient,
    account: &Account,
    page_token: Option<String>,
    history_id: String,
    history_page_token: Option<String>,
    now_ms: u64,
    cached: &HashMap<String, String>,
) -> Result<SyncBatch> {
    let mut url = url::Url::parse(&format!("{BASE}/messages"))?;
    url.query_pairs_mut()
        .append_pair("labelIds", "INBOX")
        .append_pair("maxResults", "50");
    let page = client.get(url.as_str())?;
    let (ids, metadata_threads) = inbox_fetch_plan(account, &page, cached);
    let (messages, mut removed_message_ids, mut thread_labels) =
        fetch_messages(client, account, &ids)?;
    let (message_states, state_labels, removed) =
        fetch_thread_states(client, account, &metadata_threads, cached)?;
    // A current minimal thread replaces any partial union from a newly seen
    // message in the same thread. Cached MIME never needs downloading again.
    thread_labels.retain(|(thread, _)| {
        !metadata_threads
            .iter()
            .any(|remote| local_id(account, "gmail-thread", remote) == *thread)
    });
    thread_labels.extend(state_labels);
    removed_message_ids.extend(removed);
    removed_message_ids.sort();
    removed_message_ids.dedup();
    Ok(SyncBatch {
        message_states,
        messages,
        thread_labels,
        removed_message_ids,
        next_cursor: serde_json::to_string(&Cursor::Initial {
            page_token,
            history_id,
            history_page_token,
            last_fresh_check_ms: Some(now_ms),
        })?,
        full_reset: false,
        reset_phase: None,
    })
}

// The periodic newest-inbox check runs while historical import is unfinished.
// Fetch MIME only for unknown IDs; refresh cached conversations through one
// minimal thread request each, preserving readable cached bodies and quota.
fn inbox_fetch_plan(
    account: &Account,
    page: &Value,
    cached: &HashMap<String, String>,
) -> (Vec<String>, Vec<String>) {
    let known_threads: HashSet<&String> = cached.values().collect();
    let mut full = HashSet::new();
    let mut threads = HashSet::new();
    for message in page["messages"].as_array().into_iter().flatten() {
        let Some(id) = message["id"].as_str() else {
            continue;
        };
        let local = local_id(account, "gmail-message", id);
        if let Some(thread) = cached.get(&local) {
            if let Ok(remote) = remote_id(account, "gmail-thread", thread) {
                threads.insert(remote.to_string());
            } else {
                // A malformed or foreign cached thread reference must not make
                // an owned provider message disappear from the refresh plan.
                full.insert(id.to_owned());
            }
        } else {
            full.insert(id.to_owned());
        }
        if let Some(thread) = message["threadId"].as_str() {
            if known_threads.contains(&local_id(account, "gmail-thread", thread)) {
                threads.insert(thread.to_owned());
            }
        }
    }
    let mut full: Vec<_> = full.into_iter().collect();
    full.sort();
    let mut threads: Vec<_> = threads.into_iter().collect();
    threads.sort();
    (full, threads)
}

// Determine work from the cache index before reserving any provider quota.
// Message bodies are immutable: a labels/read change needs current flags, not MIME.
fn history_fetch_plan(
    account: &Account,
    page: &Value,
    cached: &HashMap<String, String>,
) -> (Vec<String>, Vec<String>, HashSet<String>) {
    let mut full = HashSet::new();
    let mut threads = HashSet::new();
    let mut deleted = HashSet::new();
    let known_threads: HashSet<&String> = cached.values().collect();
    for entry in page["history"].as_array().into_iter().flatten() {
        for key in ["messagesAdded", "labelsAdded", "labelsRemoved"] {
            for item in entry[key].as_array().into_iter().flatten() {
                let message = &item["message"];
                let Some(id) = message["id"].as_str() else {
                    continue;
                };
                let local = local_id(account, "gmail-message", id);
                if let Some(thread) = cached.get(&local) {
                    if let Ok(remote) = remote_id(account, "gmail-thread", thread) {
                        threads.insert(remote.to_string());
                    }
                } else {
                    full.insert(id.to_owned());
                }
                if let Some(thread) = message["threadId"].as_str() {
                    if known_threads.contains(&local_id(account, "gmail-thread", thread)) {
                        threads.insert(thread.to_owned());
                    }
                }
            }
        }
        for item in entry["messagesDeleted"].as_array().into_iter().flatten() {
            if let Some(id) = item["message"]["id"].as_str() {
                deleted.insert(id.to_owned());
            }
        }
    }
    full.retain(|id| !deleted.contains(id));
    let mut full: Vec<_> = full.into_iter().collect();
    full.sort();
    let mut threads: Vec<_> = threads.into_iter().collect();
    threads.sort();
    (full, threads, deleted)
}

fn thread_snapshot(
    account: &Account,
    value: &Value,
) -> Result<(Vec<mail_core::MessageState>, (String, Vec<String>))> {
    let thread = local_id(account, "gmail-thread", string(value, "id")?);
    let mut states = Vec::new();
    let mut labels = HashSet::new();
    for message in value["messages"]
        .as_array()
        .ok_or(CloudError::Response("Gmail minimal thread messages"))?
    {
        let current: HashSet<String> = message["labelIds"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|l| l.as_str().map(str::to_owned))
            .collect();
        states.push(mail_core::MessageState {
            id: local_id(account, "gmail-message", string(message, "id")?),
            is_read: !current.contains("UNREAD"),
            is_draft: current.contains("DRAFT"),
            is_sent: current.contains("SENT"),
        });
        labels.extend(current);
    }
    if !labels
        .iter()
        .any(|l| matches!(l.as_str(), "INBOX" | "SPAM" | "TRASH" | "DRAFT"))
    {
        labels.insert("__EMAIL_ARCHIVE__".into());
    }
    let mut labels: Vec<_> = labels.into_iter().collect();
    labels.sort();
    Ok((states, (thread, labels)))
}

fn fetch_thread_states(
    client: &CloudClient,
    account: &Account,
    threads: &[String],
    cached: &HashMap<String, String>,
) -> Result<(
    Vec<mail_core::MessageState>,
    Vec<(String, Vec<String>)>,
    Vec<String>,
)> {
    let mut states = Vec::new();
    let mut labels = Vec::new();
    let mut removed = Vec::new();
    for thread in threads {
        match client.get(&format!("{BASE}/threads/{thread}?format=minimal")) {
            Ok(value) => {
                let (flags, thread_labels) = thread_snapshot(account, &value)?;
                states.extend(flags);
                labels.push(thread_labels);
            }
            Err(CloudError::ApiStatus(404)) => {
                let local = local_id(account, "gmail-thread", thread);
                removed.extend(
                    cached
                        .iter()
                        .filter(|(_, t)| **t == local)
                        .map(|(id, _)| id.clone()),
                );
            }
            Err(error) => return Err(error),
        }
    }
    Ok((states, resolve_thread_labels(client, labels)?, removed))
}

fn history_page(
    client: &CloudClient,
    account: &Account,
    history_id: &str,
    page_token: Option<&str>,
    cached: &HashMap<String, String>,
) -> Result<SyncBatch> {
    let mut url = url::Url::parse(&format!("{BASE}/history"))?;
    url.query_pairs_mut()
        .append_pair("startHistoryId", history_id)
        .append_pair("maxResults", "25");
    if let Some(token) = page_token {
        url.query_pairs_mut().append_pair("pageToken", token);
    }
    let page = match client.get(url.as_str()) {
        Ok(page) => page,
        Err(CloudError::ApiStatus(404)) => return start_reset(client, account),
        Err(error) => return Err(error),
    };
    let (ids, metadata_threads, deleted) = history_fetch_plan(account, &page, cached);
    let (messages, mut removed_message_ids, mut thread_labels) =
        fetch_messages(client, account, &ids)?;
    let (message_states, state_labels, state_removed) =
        fetch_thread_states(client, account, &metadata_threads, cached)?;
    // Minimal thread snapshots include every remote message, so their label
    // union is authoritative even when this page touched only one message.
    thread_labels.retain(|(thread, _)| {
        !metadata_threads
            .iter()
            .any(|id| local_id(account, "gmail-thread", id) == *thread)
    });
    thread_labels.extend(state_labels);
    removed_message_ids.extend(state_removed);
    removed_message_ids.extend(
        deleted
            .into_iter()
            .map(|id| local_id(account, "gmail-message", &id)),
    );
    removed_message_ids.sort();
    removed_message_ids.dedup();
    let next = match page["nextPageToken"].as_str() {
        Some(token) => Cursor::History {
            history_id: history_id.into(),
            page_token: Some(token.into()),
        },
        None => Cursor::History {
            history_id: page["historyId"].as_str().unwrap_or(history_id).into(),
            page_token: None,
        },
    };
    Ok(SyncBatch {
        message_states,
        messages,
        thread_labels,
        removed_message_ids,
        next_cursor: serde_json::to_string(&next)?,
        full_reset: false,
        reset_phase: None,
    })
}

fn start_reset(client: &CloudClient, account: &Account) -> Result<SyncBatch> {
    let profile = client.get(&format!("{BASE}/profile"))?;
    initial_page(
        client,
        account,
        None,
        string(&profile, "historyId")?.into(),
        Some(ResetPhase::Start),
    )
}

fn initial_page(
    client: &CloudClient,
    account: &Account,
    page_token: Option<&str>,
    history_id: String,
    reset: Option<ResetPhase>,
) -> Result<SyncBatch> {
    let mut url = url::Url::parse(&format!("{BASE}/messages"))?;
    url.query_pairs_mut()
        .append_pair("labelIds", "INBOX")
        .append_pair("maxResults", "50");
    if let Some(token) = page_token {
        url.query_pairs_mut().append_pair("pageToken", token);
    }
    let page = client.get(url.as_str())?;
    let ids: Vec<String> = page["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| m["id"].as_str().map(ToString::to_string))
        .collect();
    let (messages, removed_message_ids, thread_labels) = fetch_messages(client, account, &ids)?;
    let more = page["nextPageToken"].as_str();
    let next = match more {
        Some(token) if reset.is_some() => Cursor::ResetInitial {
            page_token: token.into(),
            history_id,
        },
        Some(token) => Cursor::Initial {
            page_token: Some(token.into()),
            history_id,
            history_page_token: None,
            last_fresh_check_ms: Some(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64,
            ),
        },
        None => Cursor::History {
            history_id,
            page_token: None,
        },
    };
    Ok(SyncBatch {
        message_states: Vec::new(),
        messages,
        thread_labels,
        removed_message_ids,
        next_cursor: serde_json::to_string(&next)?,
        full_reset: false,
        reset_phase: reset.map(|phase| {
            if more.is_some() {
                phase
            } else if phase == ResetPhase::Start {
                ResetPhase::StartComplete
            } else {
                ResetPhase::Complete
            }
        }),
    })
}

pub(crate) fn archive_page(
    client: &CloudClient,
    account: &Account,
    cursor: Option<&str>,
) -> Result<(SyncBatch, Option<String>)> {
    let mut url = url::Url::parse(&format!("{BASE}/messages"))?;
    url.query_pairs_mut()
        .append_pair("q", "-in:inbox -in:spam -in:trash -in:drafts")
        .append_pair("maxResults", "25");
    if let Some(cursor) = cursor {
        url.query_pairs_mut().append_pair("pageToken", cursor);
    }
    let page = client.get(url.as_str())?;
    let ids = page["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v["id"].as_str().map(str::to_owned))
        .collect::<Vec<_>>();
    let (messages, removed_message_ids, mut thread_labels) = fetch_messages(client, account, &ids)?;
    // A sent/archived message can belong to a thread with another inbox message.
    // Confirm thread-wide placement before changing its local folder.
    for (thread, labels) in &mut thread_labels {
        let remote = remote_id(account, "gmail-thread", thread)?;
        let value = client.get(&format!(
            "{BASE}/threads/{remote}?format=metadata&metadataHeaders=Subject"
        ))?;
        labels.retain(|v| {
            !matches!(
                v.as_str(),
                "INBOX" | "SPAM" | "TRASH" | "DRAFT" | "__EMAIL_ARCHIVE__"
            )
        });
        let flags = archive_thread_placement(&value)?;
        labels.extend(flags);
    }
    let next = page["nextPageToken"].as_str().map(str::to_owned);
    Ok((
        SyncBatch {
            message_states: Vec::new(),
            messages,
            removed_message_ids,
            thread_labels,
            next_cursor: String::new(),
            full_reset: false,
            reset_phase: None,
        },
        next,
    ))
}

fn archive_thread_placement(value: &Value) -> Result<Vec<String>> {
    let messages = value["messages"]
        .as_array()
        .filter(|v| !v.is_empty())
        .ok_or(CloudError::Response("Gmail archive thread messages"))?;
    let mut flags = Vec::new();
    for message in messages {
        for label in message["labelIds"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            if matches!(label, "INBOX" | "SPAM" | "TRASH" | "DRAFT")
                && !flags.iter().any(|v| v == label)
            {
                flags.push(label.to_owned());
            }
        }
    }
    if flags.is_empty() {
        flags.push("__EMAIL_ARCHIVE__".into());
    }
    Ok(flags)
}

fn fetch_messages(
    client: &CloudClient,
    account: &Account,
    ids: &[String],
) -> Result<(Vec<Message>, Vec<String>, Vec<(String, Vec<String>)>)> {
    // Bound concurrent requests so initial sync is responsive without overrunning provider quotas.
    let mut messages = Vec::new();
    let mut removed = Vec::new();
    let mut raw_labels = Vec::new();
    let size = ids.len().div_ceil(8).max(1);
    std::thread::scope(|scope| -> Result<()> {
        let handles: Vec<_> = ids
            .chunks(size)
            .map(|chunk| {
                scope.spawn(
                    move || -> Result<(Vec<Message>, Vec<String>, Vec<(String, Vec<String>)>)> {
                        let mut local_messages = Vec::new();
                        let mut local_removed = Vec::new();
                        let mut local_labels = Vec::new();
                        for id in chunk {
                            let url = format!("{BASE}/messages/{id}?format=full");
                            match client.get(&url) {
                                Ok(value) => {
                                    let mut label_ids: Vec<String> = value["labelIds"]
                                        .as_array()
                                        .into_iter()
                                        .flatten()
                                        .filter_map(|label| label.as_str().map(ToString::to_string))
                                        .collect();
                                    if !label_ids.iter().any(|v| {
                                        matches!(v.as_str(), "INBOX" | "SPAM" | "TRASH" | "DRAFT")
                                    }) {
                                        label_ids.push("__EMAIL_ARCHIVE__".into());
                                    }
                                    let thread_id = value["threadId"]
                                        .as_str()
                                        .ok_or(CloudError::Response("Gmail thread ID"))?;
                                    local_labels.push((
                                        local_id(account, "gmail-thread", thread_id),
                                        label_ids,
                                    ));
                                    local_messages.push(map_message(account, &value)?);
                                }
                                Err(CloudError::ApiStatus(404)) => {
                                    local_removed.push(local_id(account, "gmail-message", id))
                                }
                                Err(error) => return Err(error),
                            }
                        }
                        Ok((local_messages, local_removed, local_labels))
                    },
                )
            })
            .collect();
        for handle in handles {
            let (mut ms, mut rs, mut ls) = handle
                .join()
                .map_err(|_| CloudError::Response("message fetch panicked"))??;
            messages.append(&mut ms);
            removed.append(&mut rs);
            raw_labels.append(&mut ls);
        }
        Ok(())
    })?;
    Ok((
        messages,
        removed,
        resolve_thread_labels(client, raw_labels)?,
    ))
}

fn resolve_thread_labels(
    client: &CloudClient,
    raw_labels: Vec<(String, Vec<String>)>,
) -> Result<Vec<(String, Vec<String>)>> {
    let mut thread_labels = std::collections::BTreeMap::<String, Vec<String>>::new();
    if !raw_labels.is_empty() {
        let catalog = client.get(&format!("{BASE}/labels"))?;
        let names: std::collections::HashMap<&str, &str> = catalog["labels"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|label| Some((label["id"].as_str()?, label["name"].as_str()?)))
            .collect();
        for (thread, ids) in raw_labels {
            let labels = thread_labels.entry(thread).or_default();
            for id in ids {
                if matches!(
                    id.as_str(),
                    "INBOX"
                        | "SPAM"
                        | "TRASH"
                        | "DRAFT"
                        | "STARRED"
                        | "IMPORTANT"
                        | "__EMAIL_ARCHIVE__"
                ) {
                    if !labels.contains(&id) {
                        labels.push(id);
                    }
                    continue;
                }
                if let Some(name) = names.get(id.as_str()) {
                    if !id.starts_with("CATEGORY_")
                        && !matches!(
                            id.as_str(),
                            "INBOX"
                                | "UNREAD"
                                | "SENT"
                                | "DRAFT"
                                | "TRASH"
                                | "SPAM"
                                | "STARRED"
                                | "IMPORTANT"
                        )
                        && !labels.iter().any(|existing| existing == name)
                    {
                        labels.push((*name).to_string());
                    }
                }
            }
        }
    }
    Ok(thread_labels.into_iter().collect())
}

pub(crate) fn map_message(account: &Account, value: &Value) -> Result<Message> {
    let id = string(value, "id")?;
    let thread_id = string(value, "threadId")?;
    let labels = value["labelIds"].as_array();
    let read = !labels.is_some_and(|list| list.iter().any(|item| item == "UNREAD"));
    let payload = &value["payload"];
    let headers = &payload["headers"];
    let (plain, html) = bodies(payload);
    let subject = header(headers, "Subject")
        .unwrap_or("(no subject)")
        .to_string();
    let sender = header(headers, "From")
        .unwrap_or("(unknown sender)")
        .to_string();
    let to = header_addresses(headers, "To");
    Ok(Message {
        recipient_headers: Some(mail_core::RecipientHeaders {
            cc: header_addresses(headers, "Cc"),
            bcc: header_addresses(headers, "Bcc"),
            reply_to: header_addresses(headers, "Reply-To"),
        }),
        id: local_id(account, "gmail-message", id),
        thread_id: local_id(account, "gmail-thread", thread_id),
        account_id: account.id.clone(),
        sender,
        to,
        subject,
        body_plain: if plain.is_empty() {
            value["snippet"].as_str().unwrap_or("").into()
        } else {
            plain
        },
        body_html: html,
        unsubscribe_header: header(headers, "List-Unsubscribe").map(str::to_string),
        has_attachments: Some(!attachment_parts(payload).is_empty()),
        received_at: value["internalDate"]
            .as_str()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0),
        is_read: read,
        is_draft: false,
        is_sent: false,
    })
}
fn header_addresses(headers: &Value, name: &str) -> Vec<String> {
    headers
        .as_array()
        .into_iter()
        .flatten()
        .filter(|h| {
            h["name"]
                .as_str()
                .is_some_and(|n| n.eq_ignore_ascii_case(name))
        })
        .filter_map(|h| h["value"].as_str())
        .flat_map(|raw| {
            mailparse::addrparse(raw)
                .map(|items| {
                    items
                        .iter()
                        .flat_map(|item| match item {
                            mailparse::MailAddr::Single(a) => vec![a],
                            mailparse::MailAddr::Group(g) => g.addrs.iter().collect(),
                        })
                        .map(|a| match &a.display_name {
                            Some(name) => format!("{:?} <{}>", name, a.addr),
                            None => a.addr.clone(),
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_else(|_| {
                    if raw.trim().is_empty() {
                        vec![]
                    } else {
                        vec![raw.to_string()]
                    }
                })
        })
        .collect()
}
pub(crate) fn recipient_headers(
    client: &CloudClient,
    account: &Account,
    message_id: &str,
) -> Result<(Vec<String>, mail_core::RecipientHeaders)> {
    let remote = remote_id(account, "gmail-message", message_id)?;
    let value = client.get(&format!("{BASE}/messages/{remote}?format=metadata&metadataHeaders=To&metadataHeaders=Cc&metadataHeaders=Bcc&metadataHeaders=Reply-To"))?;
    let h = &value["payload"]["headers"];
    Ok((
        header_addresses(h, "To"),
        mail_core::RecipientHeaders {
            cc: header_addresses(h, "Cc"),
            bcc: header_addresses(h, "Bcc"),
            reply_to: header_addresses(h, "Reply-To"),
        },
    ))
}
fn bodies(part: &Value) -> (String, Option<String>) {
    const MAX_DEPTH: usize = 24;
    const MAX_PARTS: usize = 256;
    const MAX_TEXT_BYTES: usize = 4 * 1024 * 1024;
    let mut plain = String::new();
    let mut html = None;
    fn walk(
        part: &Value,
        depth: usize,
        parts_left: &mut usize,
        bytes_left: &mut usize,
        plain: &mut String,
        html: &mut Option<String>,
    ) {
        if depth > MAX_DEPTH || *parts_left == 0 {
            return;
        }
        *parts_left -= 1;
        if let Some(parts) = part["parts"].as_array() {
            for child in parts {
                walk(child, depth + 1, parts_left, bytes_left, plain, html);
            }
        }
        // Binary parts and text attachments are never body content. Check MIME
        // before decoding so a large attachment cannot allocate decoder output.
        if part["filename"]
            .as_str()
            .is_some_and(|filename| !filename.is_empty())
        {
            return;
        }
        let mime = part["mimeType"].as_str().unwrap_or("");
        if mime != "text/plain" && mime != "text/html" {
            return;
        }
        if mime == "text/html" && html.is_some() {
            return;
        }
        let Some(data) = part["body"]["data"].as_str() else {
            return;
        };
        // Base64 expands by at most 3/4. Bound encoded input before decoding.
        if *bytes_left == 0 || data.len() > bytes_left.saturating_mul(4) / 3 + 4 {
            return;
        }
        let bytes = URL_SAFE
            .decode(data)
            .or_else(|_| URL_SAFE_NO_PAD.decode(data));
        let Ok(bytes) = bytes else { return };
        if bytes.len() > *bytes_left {
            return;
        }
        *bytes_left -= bytes.len();
        let text = String::from_utf8_lossy(&bytes).into_owned();
        match mime {
            "text/plain" => {
                if !plain.is_empty() {
                    plain.push('\n');
                }
                plain.push_str(&text);
            }
            "text/html" => {
                if html.is_none() {
                    *html = Some(text);
                }
            }
            _ => {}
        }
    }
    let mut parts_left = MAX_PARTS;
    let mut bytes_left = MAX_TEXT_BYTES;
    walk(
        part,
        0,
        &mut parts_left,
        &mut bytes_left,
        &mut plain,
        &mut html,
    );
    (plain, html)
}
fn header<'a>(headers: &'a Value, name: &str) -> Option<&'a str> {
    headers.as_array()?.iter().find(|h| {
        h["name"]
            .as_str()
            .is_some_and(|n| n.eq_ignore_ascii_case(name))
    })?["value"]
        .as_str()
}
fn string<'a>(value: &'a Value, key: &'static str) -> Result<&'a str> {
    value[key].as_str().ok_or(CloudError::Response(key))
}

#[cfg(test)]
pub(super) fn send_plain(
    client: &CloudClient,
    from: &str,
    to: &str,
    subject: &str,
    body: &str,
) -> Result<()> {
    fn check(s: &str) -> Result<()> {
        if s.contains(['\r', '\n']) {
            Err(CloudError::InvalidHeader)
        } else {
            Ok(())
        }
    }
    check(from)?;
    check(to)?;
    check(subject)?;
    let subject = if subject.is_ascii() {
        subject.to_string()
    } else {
        format!("=?UTF-8?B?{}?=", STANDARD.encode(subject.as_bytes()))
    };
    let encoded = STANDARD.encode(body.as_bytes());
    let mime = format!(
        "From: {}\r\nTo: {}\r\nSubject: {}\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=UTF-8\r\nContent-Transfer-Encoding: base64\r\n\r\n{}\r\n",
        from, to, subject, encoded
    );
    client.post(
        &format!("{BASE}/messages/send"),
        &json!({"raw":URL_SAFE_NO_PAD.encode(mime.as_bytes())}),
    )?;
    Ok(())
}

pub(super) fn send_in_thread_html(
    client: &CloudClient, account: &Account, from: &str, to: &str, cc: &str, bcc: &str,
    subject: &str, body: &str, body_html: Option<&str>, attachments: &[OutgoingAttachment], thread_id: Option<&str>,
) -> Result<Option<String>> {
    let mime = build_message_recipients_html(from, to, cc, bcc, subject, body, body_html, attachments)?;
    let mut raw = mime.formatted();
    let mut payload = json!({});
    if let Some(thread_id) = thread_id {
        let remote = remote_id(account, "gmail-thread", thread_id)?;
        let thread = client.get(&format!("{BASE}/threads/{remote}?format=metadata&metadataHeaders=Message-ID&metadataHeaders=References"))?;
        if let Some(parent) = thread["messages"].as_array().and_then(|m| m.last()) {
            let headers = parent["payload"]["headers"].as_array();
            let header = |name: &str| headers.into_iter().flatten().find(|h| h["name"].as_str().is_some_and(|n| n.eq_ignore_ascii_case(name))).and_then(|h| h["value"].as_str());
            if let Some(id) = header("Message-ID") {
                let references = header("References").unwrap_or("");
                if id.chars().any(char::is_control) || references.chars().any(char::is_control) {
                    return Err(CloudError::InvalidHeader);
                }
                let mut prefix = format!("In-Reply-To: {id}\r\nReferences: {references} {id}\r\n").into_bytes();
                prefix.extend(raw);
                raw = prefix;
            } else { return Err(CloudError::InvalidHeader); }
        } else { return Err(CloudError::InvalidHeader); }
        payload["threadId"] = json!(remote);
    }
    payload["raw"] = json!(URL_SAFE_NO_PAD.encode(raw));
    let response = client.post(&format!("{BASE}/messages/send"), &payload)?;
    Ok(response["id"].as_str().map(|id| local_id(account, "gmail-message", id)))
}

#[cfg(test)]
fn build_attachment_message(
    from: &str,
    to: &str,
    subject: &str,
    body: &str,
    attachments: &[OutgoingAttachment],
) -> Result<OutgoingMessage> {
    build_message_recipients(from, to, "", "", subject, body, attachments)
}

#[cfg(test)]
fn build_message_recipients(
    from: &str,
    to: &str,
    cc: &str,
    bcc: &str,
    subject: &str,
    body: &str,
    attachments: &[OutgoingAttachment],
) -> Result<OutgoingMessage> {
    build_message_recipients_html(from,to,cc,bcc,subject,body,None,attachments)
}
fn build_message_recipients_html(
    from: &str, to: &str, cc: &str, bcc: &str, subject: &str, body: &str,
    body_html: Option<&str>, attachments: &[OutgoingAttachment],
) -> Result<OutgoingMessage> {
    validate_outgoing_attachments(attachments)?;
    if subject.contains(['\r', '\n'])
        || [to, cc, bcc]
            .iter()
            .any(|s| s.chars().any(char::is_control))
    {
        return Err(CloudError::InvalidHeader);
    }
    let mut builder = OutgoingMessage::builder()
        .from(
            from.parse::<Mailbox>()
                .map_err(|_| CloudError::InvalidHeader)?,
        )
        .subject(subject);
    builder = builder.keep_bcc();
    let mut count = 0;
    for (kind, raw) in [("to", to), ("cc", cc), ("bcc", bcc)] {
        if raw.trim().is_empty() {
            continue;
        }
        let parsed = mailparse::addrparse(raw).map_err(|_| CloudError::InvalidHeader)?;
        for recipient in parsed.iter().flat_map(|addr| match addr {
            MailAddr::Single(single) => vec![single],
            MailAddr::Group(group) => group.addrs.iter().collect(),
        }) {
            let mailbox = Mailbox::new(
                recipient.display_name.clone(),
                recipient
                    .addr
                    .parse()
                    .map_err(|_| CloudError::InvalidHeader)?,
            );
            builder = match kind {
                "to" => builder.to(mailbox),
                "cc" => builder.cc(mailbox),
                _ => builder.bcc(mailbox),
            };
            count += 1;
        }
    }
    if count == 0 {
        return Err(CloudError::InvalidHeader);
    }
    if attachments.is_empty() {
        return Ok(match body_html {
            Some(html) => builder.multipart(MultiPart::alternative().singlepart(SinglePart::plain(body.to_owned())).singlepart(SinglePart::html(html.to_owned())))?,
            None => builder.body(body.to_owned())?,
        });
    }
    let mut multipart = match body_html {
        Some(html) => MultiPart::mixed().multipart(MultiPart::alternative().singlepart(SinglePart::plain(body.to_owned())).singlepart(SinglePart::html(html.to_owned()))),
        None => MultiPart::mixed().singlepart(SinglePart::plain(body.to_owned())),
    };
    for attachment in attachments {
        let content_type = ContentType::parse(attachment_mime(attachment))
            .map_err(|_| CloudError::InvalidAttachment("invalid MIME type"))?;
        multipart = multipart.singlepart(
            Attachment::new(attachment.filename.clone())
                .body(attachment.bytes.clone(), content_type),
        );
    }
    Ok(builder.multipart(multipart)?)
}

pub(super) fn apply_action(
    client: &CloudClient,
    account: &Account,
    action: &MailAction,
) -> Result<()> {
    match action {
        MailAction::Archive { thread_id } => modify_thread(
            client,
            remote_id(account, "gmail-thread", thread_id)?,
            &[],
            &["INBOX"],
        ),
        MailAction::MarkRead { thread_id, read } => {
            if *read {
                modify_thread(
                    client,
                    remote_id(account, "gmail-thread", thread_id)?,
                    &[],
                    &["UNREAD"],
                )
            } else {
                modify_thread(
                    client,
                    remote_id(account, "gmail-thread", thread_id)?,
                    &["UNREAD"],
                    &[],
                )
            }
        }
        MailAction::Move { thread_id, folder } => {
            let remote = remote_id(account, "gmail-thread", thread_id)?;
            if let Some((add, remove)) = destination_labels(folder) {
                modify_thread(client, remote, add, remove)
            } else {
                let label = label_id(client, folder)?;
                modify_thread(client, remote, &[&label], &["INBOX", "SPAM", "TRASH"])
            }
        }
        MailAction::AddLabel { thread_id, label } => {
            let remote = remote_id(account, "gmail-thread", thread_id)?;
            let id = label_id(client, label)?;
            modify_thread(client, remote, &[&id], &[])
        }
        MailAction::RemoveLabel { thread_id, label } => {
            let remote = remote_id(account, "gmail-thread", thread_id)?;
            let id = label_id(client, label)?;
            modify_thread(client, remote, &[], &[&id])
        }
        MailAction::SendDraft { .. } => Err(CloudError::Unsupported(
            "send a local draft with send_plain after reviewing its content",
        )),
    }
}
// A move changes placement; adding INBOX alone leaves Spam/Trash authoritative
// on the next sync and makes a successful local restore or Undo bounce back.
// Keep read/starred/importance and user labels untouched.
fn destination_labels(folder: &str) -> Option<(&'static [&'static str], &'static [&'static str])> {
    if folder.eq_ignore_ascii_case("inbox") {
        Some((&["INBOX"], &["SPAM", "TRASH"]))
    } else if folder.eq_ignore_ascii_case("archive") {
        Some((&[], &["INBOX", "SPAM", "TRASH"]))
    } else if folder.eq_ignore_ascii_case("spam") || folder.eq_ignore_ascii_case("junk") {
        Some((&["SPAM"], &["INBOX", "TRASH"]))
    } else if folder.eq_ignore_ascii_case("trash") {
        Some((&["TRASH"], &["INBOX", "SPAM"]))
    } else {
        None
    }
}

pub(super) fn bulk_mark_read(
    client: &CloudClient,
    account: &Account,
    ids: &[String],
) -> Result<()> {
    let remote = ids
        .iter()
        .map(|id| remote_id(account, "gmail-message", id))
        .collect::<Result<Vec<_>>>()?;
    for chunk in remote.chunks(1000) {
        client.post(
            &format!("{BASE}/messages/batchModify"),
            &json!({"ids":chunk,"removeLabelIds":["UNREAD"]}),
        )?;
    }
    Ok(())
}
fn modify_thread(client: &CloudClient, id: &str, add: &[&str], remove: &[&str]) -> Result<()> {
    client.post(
        &format!("{BASE}/threads/{id}/modify"),
        &json!({"addLabelIds":add,"removeLabelIds":remove}),
    )?;
    Ok(())
}
fn label_id(client: &CloudClient, name: &str) -> Result<String> {
    let labels = client.get(&format!("{BASE}/labels"))?;
    if let Some(found) = labels["labels"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|l| l["name"] == name)
    {
        return Ok(string(found, "id")?.into());
    }
    let created = client.post(
        &format!("{BASE}/labels"),
        &json!({"name":name,"labelListVisibility":"labelShow","messageListVisibility":"show"}),
    )?;
    Ok(string(&created, "id")?.into())
}

#[cfg(test)]
mod tests {
    #[test]
    fn undo_spam_and_restore_replace_placement_without_changing_read_or_labels() {
        for destination in ["Inbox", "INBOX", "Archive", "Spam", "Junk", "Trash"] {
            let (add, remove) = destination_labels(destination).unwrap();
            let mut labels: HashSet<String> =
                ["INBOX", "SPAM", "TRASH", "UNREAD", "STARRED", "Label_Work"]
                    .into_iter()
                    .map(str::to_owned)
                    .collect();
            for label in remove {
                labels.remove(*label);
            }
            labels.extend(add.iter().map(|label| (*label).to_string()));
            let expected = match destination.to_ascii_lowercase().as_str() {
                "inbox" => Some("INBOX"),
                "archive" => None,
                "spam" | "junk" => Some("SPAM"),
                "trash" => Some("TRASH"),
                _ => unreachable!(),
            };
            for placement in ["INBOX", "SPAM", "TRASH"] {
                assert_eq!(
                    labels.contains(placement),
                    expected == Some(placement),
                    "destination {destination}"
                );
            }
            for kept in ["UNREAD", "STARRED", "Label_Work"] {
                assert!(labels.contains(kept));
            }
        }
        assert!(destination_labels("Customer Support").is_none());
    }

    #[test]
    fn newest_inbox_refresh_deduplicates_cached_threads_and_only_downloads_new_mime() {
        let account = account();
        let cached = HashMap::from([
            (
                local_id(&account, "gmail-message", "old-1"),
                local_id(&account, "gmail-thread", "conversation"),
            ),
            (
                local_id(&account, "gmail-message", "old-2"),
                local_id(&account, "gmail-thread", "conversation"),
            ),
        ]);
        let page = json!({"messages":[
            {"id":"old-1", "threadId":"conversation"},
            {"id":"old-2", "threadId":"conversation"},
            {"id":"new-reply", "threadId":"conversation"},
            {"id":"new-thread", "threadId":"other"},
            {"id":"new-reply", "threadId":"conversation"},
            {"threadId":"missing-id"}
        ]});
        let (full, threads) = inbox_fetch_plan(&account, &page, &cached);
        assert_eq!(full, vec!["new-reply", "new-thread"]);
        assert_eq!(threads, vec!["conversation"]);
        let (full, threads) = inbox_fetch_plan(&account, &page, &HashMap::new());
        assert_eq!(full, vec!["new-reply", "new-thread", "old-1", "old-2"]);
        assert!(threads.is_empty());
        let foreign = HashMap::from([(
            local_id(&account, "gmail-message", "old-1"),
            "other-account:gmail-thread:conversation".to_string(),
        )]);
        let (full, threads) = inbox_fetch_plan(&account, &page, &foreign);
        assert_eq!(full, vec!["new-reply", "new-thread", "old-1", "old-2"]);
        assert!(threads.is_empty());
    }

    #[test]
    fn cached_history_uses_one_minimal_thread_without_redownloading_bodies() {
        let account = account();
        let mut cached = HashMap::new();
        let mut changes = Vec::new();
        for i in 0..4000 {
            let id = format!("m{i}");
            cached.insert(
                local_id(&account, "gmail-message", &id),
                local_id(&account, "gmail-thread", "same-thread"),
            );
            changes
                .push(json!({"message":{"id":id,"threadId":"same-thread"},"labelIds":["UNREAD"]}));
        }
        changes.push(json!({"message":{"id":"new-message","threadId":"same-thread"}}));
        let page = json!({"history":[{"labelsRemoved":changes,"messagesDeleted":[{"message":{"id":"gone"}}]}]});
        let (full, threads, deleted) = history_fetch_plan(&account, &page, &cached);
        assert_eq!(full, vec!["new-message"]);
        assert_eq!(threads, vec!["same-thread"]);
        assert!(deleted.contains("gone"));
    }

    #[test]
    fn minimal_snapshot_preserves_labels_held_by_other_thread_messages() {
        let value = json!({"id":"t1","messages":[
            {"id":"one","labelIds":["INBOX","UNREAD","Label_Billing"]},
            {"id":"two","labelIds":["SENT","Label_Work"]}
        ]});
        let (states, (thread, labels)) = thread_snapshot(&account(), &value).unwrap();
        assert_eq!(thread, local_id(&account(), "gmail-thread", "t1"));
        assert!(!states[0].is_read);
        assert!(states[1].is_read && states[1].is_sent);
        assert!(labels.contains(&"Label_Billing".into()) && labels.contains(&"Label_Work".into()));
        assert!(!labels.contains(&"__EMAIL_ARCHIVE__".into()));
        let (_, (_, labels)) = thread_snapshot(
            &account(),
            &json!({"id":"t1","messages":[{"id":"one","labelIds":[]}]}),
        )
        .unwrap();
        assert!(labels.contains(&"__EMAIL_ARCHIVE__".into()));
        assert!(thread_snapshot(&account(), &json!({"id":"t1"})).is_err());
    }

    #[test]
    fn archived_message_does_not_hide_a_thread_with_an_inbox_reply() {
        let flags = super::archive_thread_placement(&serde_json::json!({"messages":[{"labelIds":["SENT"]},{"labelIds":["INBOX","UNREAD"]}]})).unwrap();
        assert_eq!(flags, vec!["INBOX"]);
        assert_eq!(
            super::archive_thread_placement(
                &serde_json::json!({"messages":[{"labelIds":["SENT"]}]})
            )
            .unwrap(),
            vec!["__EMAIL_ARCHIVE__"]
        );
        assert!(super::archive_thread_placement(&serde_json::json!({"messages":[]})).is_err());
    }
    use super::*;

    #[test]
    fn verified_send_as_aliases_keep_gmail_default_first() {
        let response = json!({"sendAs": [
            {"sendAsEmail":"alias@example.com","displayName":"Alias","verificationStatus":"accepted","isDefault":false},
            {"sendAsEmail":"pending@example.com","verificationStatus":"pending","isDefault":false},
            {"sendAsEmail":"primary@example.com","isPrimary":true,"isDefault":true}
        ]});
        let aliases = parse_send_as_aliases(&response).unwrap();
        assert_eq!(
            aliases
                .iter()
                .map(|alias| alias.email.as_str())
                .collect::<Vec<_>>(),
            vec!["primary@example.com", "alias@example.com"]
        );
        assert!(aliases[0].is_default);
    }

    #[test]
    fn attachment_message_uses_selected_from_address() {
        let message = build_attachment_message(
            "alias@example.com",
            "friend@example.com",
            "Hello",
            "Body",
            &[OutgoingAttachment {
                filename: "note.txt".into(),
                mime_type: "text/plain".into(),
                bytes: b"note".to_vec(),
            }],
        )
        .unwrap();
        let raw = String::from_utf8(message.formatted()).unwrap();
        assert!(raw.contains("From: alias@example.com\r\n"));
        assert!(!raw.contains("From: primary@example.com"));
    }
    use mail_core::Provider;
    fn account() -> Account {
        Account {
            id: "acct-1".into(),
            provider: Provider::Gmail,
            email: "alex@example.com".into(),
            display_name: "Alex".into(),
        }
    }
    #[test]
    fn preserves_recipient_aliases_quoted_names_and_cc() {
        let input = json!({"id":"m1","threadId":"t1","payload":{"headers":[
            {"name":"To","value":"\"Doe, Jane\" <jane@alias.example>, team@example.com"},
            {"name":"Cc","value":"other@example.com"},
            {"name":"cc","value":"second@example.com"},
            {"name":"Reply-To","value":"help@example.com"}
        ]}});
        let message = map_message(&account(), &input).unwrap();
        assert_eq!(
            message.to,
            vec!["\"Doe, Jane\" <jane@alias.example>", "team@example.com"]
        );
        let headers = message.recipient_headers.unwrap();
        assert_eq!(headers.cc, vec!["other@example.com", "second@example.com"]);
        assert_eq!(headers.reply_to, vec!["help@example.com"]);
        assert!(headers.bcc.is_empty());
    }
    #[test]
    fn maps_multipart_message_and_namespaces_ids() {
        let plain = URL_SAFE_NO_PAD.encode("Hello from Gmail".as_bytes());
        let html = URL_SAFE_NO_PAD.encode("<p>Hello from Gmail</p>".as_bytes());
        let input = json!({"id":"m1","threadId":"t1","internalDate":"1794000000000","labelIds":["INBOX","UNREAD"],"payload":{"headers":[{"name":"From","value":"Pat <pat@example.com>"},{"name":"Subject","value":"Hi"},{"name":"To","value":"alex@example.com"},{"name":"List-Unsubscribe","value":"<https://example.com/leave>"}],"parts":[{"mimeType":"text/plain","body":{"data":plain}},{"mimeType":"text/html","body":{"data":html}}]}});
        let mapped = map_message(&account(), &input).unwrap();
        assert_eq!(mapped.id, "acct-1:gmail-message:m1");
        assert_eq!(mapped.thread_id, "acct-1:gmail-thread:t1");
        assert_eq!(mapped.body_plain, "Hello from Gmail");
        assert_eq!(mapped.body_html.as_deref(), Some("<p>Hello from Gmail</p>"));
        assert_eq!(
            mapped.unsubscribe_header.as_deref(),
            Some("<https://example.com/leave>")
        );
        assert_eq!(mapped.has_attachments, Some(false));
        assert!(!mapped.is_read);
    }
    #[test]
    fn body_walker_skips_binary_and_named_text_attachments() {
        let payload = json!({"parts":[
            {"mimeType":"image/png","body":{"data":"A".repeat(7_000_000)}},
            {"mimeType":"text/plain","filename":"notes.txt","body":{"data":URL_SAFE_NO_PAD.encode("attachment secret")}},
            {"mimeType":"text/plain","body":{"data":URL_SAFE_NO_PAD.encode("actual body")}}
        ]});
        let (plain, html) = bodies(&payload);
        assert_eq!(plain, "actual body");
        assert!(html.is_none());
    }
    #[test]
    fn body_walker_bounds_depth_and_text_size() {
        let deep = (0..26).fold(
            json!({"mimeType":"text/plain","body":{"data":URL_SAFE_NO_PAD.encode("too deep")}}),
            |child, _| json!({"parts":[child]}),
        );
        assert_eq!(bodies(&deep).0, "");
        let oversized = json!({"mimeType":"text/plain","body":{"data":URL_SAFE_NO_PAD.encode(vec![b'a'; 4 * 1024 * 1024 + 1])}});
        assert_eq!(bodies(&oversized).0, "");
    }
    #[test]
    fn finds_named_attachments_in_nested_mime_parts() {
        let payload = json!({"parts":[
            {"mimeType":"text/plain","body":{"data":"SGVsbG8="}},
            {"mimeType":"multipart/mixed","parts":[
                {"filename":"report.xml.gz","mimeType":"application/gzip","body":{"attachmentId":"remote-1","size":123}},
                {"filename":"inline.png","mimeType":"image/png","body":{"data":"aGVsbG8=","size":5}}
            ]}
        ]});
        let found = attachment_parts(&payload);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].0, "1.0");
        assert_eq!(found[0].1["filename"], "report.xml.gz");
        assert_eq!(found[1].0, "1.1");
    }
    #[test]
    fn finds_attachment_as_root_mime_part() {
        let payload = json!({"filename":"report.zip","mimeType":"application/zip","body":{"attachmentId":"remote","size":683}});
        let found = attachment_parts(&payload);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, "root");
    }
    #[test]
    fn finds_inline_image_by_content_id_without_filename() {
        let payload = json!({"parts":[{"mimeType":"image/png","headers":[{"name":"Content-ID","value":"<logo@example>"}],"body":{"attachmentId":"image-1","size":42}}]});
        let found = attachment_parts(&payload);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, "0");
        assert_eq!(content_id(found[0].1).as_deref(), Some("logo@example"));
    }
    #[test]
    fn finds_unnamed_inline_calendar_parts_and_keeps_plain_body_separate() {
        let payload = json!({"parts":[
            {"mimeType":"text/plain","body":{"data":URL_SAFE_NO_PAD.encode("Meeting details")}},
            {"mimeType":"text/calendar","body":{"data":URL_SAFE_NO_PAD.encode("BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n"),"size":32}},
            {"mimeType":"multipart/alternative","parts":[
                {"mimeType":"text/html","body":{"data":URL_SAFE_NO_PAD.encode("<p>Meeting</p>")}},
                {"mimeType":"text/calendar; method=REQUEST","body":{"attachmentId":"invite-1","size":100}}
            ]}
        ]});
        let found = attachment_parts(&payload);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].0, "1");
        assert_eq!(found[1].0, "2.1");
        assert_eq!(found[1].1["body"]["attachmentId"], "invite-1");
        assert_eq!(bodies(&payload).0, "Meeting details");
        assert_eq!(
            attachment_parts(&json!({"mimeType":"text/calendar","body":{"data":"x"}}))[0].0,
            "root"
        );
    }
    #[test]
    fn cursor_round_trip_keeps_history_anchor_across_initial_pages() {
        let value = Cursor::Initial {
            page_token: Some("next".into()),
            history_id: "123".into(),
            history_page_token: Some("history-next".into()),
            last_fresh_check_ms: Some(42),
        };
        let bytes = serde_json::to_string(&value).unwrap();
        assert!(bytes.contains("\"history_id\":\"123\""));
        assert!(
            matches!(serde_json::from_str::<Cursor>(&bytes).unwrap(),Cursor::Initial{page_token:Some(_),history_id,history_page_token:Some(_),last_fresh_check_ms:Some(42)} if history_id=="123")
        );
        let legacy = r#"{"phase":"initial","page_token":"next","history_id":"123"}"#;
        assert!(matches!(
            serde_json::from_str::<Cursor>(legacy).unwrap(),
            Cursor::Initial {
                history_page_token: None,
                ..
            }
        ));
    }
    #[test]
    fn reset_cursor_keeps_paging_state_separate_from_normal_history() {
        let cursor = Cursor::ResetInitial {
            page_token: "page-2".into(),
            history_id: "history-7".into(),
        };
        let encoded = serde_json::to_string(&cursor).unwrap();
        assert!(
            matches!(serde_json::from_str::<Cursor>(&encoded).unwrap(), Cursor::ResetInitial { page_token, history_id } if page_token == "page-2" && history_id == "history-7")
        );
    }
    #[test]
    fn send_rejects_header_injection_before_network() {
        let client = CloudClient::new(
            crate::OAuthConfig {
                provider: crate::CloudProvider::Gmail,
                client_id: "unused".into(),
                client_secret: None,
            },
            crate::TokenSet {
                access_token: "unused".into(),
                refresh_token: None,
                expires_at_ms: i64::MAX,
            },
        )
        .unwrap();
        assert!(matches!(
            send_plain(
                &client,
                &account().email,
                "safe@example.com\r\nBcc: thief@example.com",
                "Hi",
                "body"
            ),
            Err(CloudError::InvalidHeader)
        ));
    }

    #[test]
    fn builds_mime_with_attachment_without_treating_file_as_body() {
        let file = OutgoingAttachment {
            filename: "notes.txt".into(),
            mime_type: "text/plain".into(),
            bytes: b"file payload".to_vec(),
        };
        let message = build_attachment_message(
            &account().email,
            "Pat <pat@example.net>",
            "Meeting notes",
            "Message body",
            &[file],
        )
        .unwrap();
        let formatted = message.formatted();
        let parsed = mailparse::parse_mail(&formatted).unwrap();
        assert_eq!(parsed.ctype.mimetype, "multipart/mixed");
        assert_eq!(parsed.subparts.len(), 2);
        assert_eq!(parsed.subparts[0].get_body().unwrap(), "Message body");
        assert_eq!(parsed.subparts[1].get_body_raw().unwrap(), b"file payload");
        assert_eq!(
            parsed.subparts[1].get_content_disposition().params["filename"],
            "notes.txt"
        );
    }
    #[test]
    fn gmail_private_upload_preserves_cc_and_bcc_for_delivery() {
        use mailparse::MailHeaderMap;
        let message = build_message_recipients(
            "me@example.com",
            "to@example.com",
            "Copy <cc@example.com>",
            "hidden@example.com",
            "Subject",
            "Body",
            &[],
        )
        .unwrap();
        let raw = message.formatted();
        let parsed = mailparse::parse_mail(&raw).unwrap();
        assert_eq!(
            parsed.headers.get_first_value("To").unwrap(),
            "to@example.com"
        );
        assert!(
            parsed
                .headers
                .get_first_value("Cc")
                .unwrap()
                .contains("cc@example.com")
        );
        assert_eq!(
            parsed.headers.get_first_value("Bcc").unwrap(),
            "hidden@example.com"
        );
        assert!(
            build_message_recipients(
                "me@example.com",
                "",
                "",
                "hidden@example.com",
                "Subject",
                "Body",
                &[]
            )
            .is_ok()
        );
        assert!(
            build_message_recipients("me@example.com", "", "", "", "Subject", "Body", &[]).is_err()
        );
    }
    #[test]
    fn tracked_mime_keeps_plain_alternative_recipients_and_attachment_bytes() {
        let html="<p>Private &lt;body&gt;</p><img src=\"https://scheduling.example.test/api/tracking/pixel/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.gif\" width=\"1\" height=\"1\" alt=\"\">";
        for with_attachment in [false,true] {
            let attachments=if with_attachment {vec![OutgoingAttachment{filename:"note.txt".into(),mime_type:"text/plain".into(),bytes:b"attachment bytes".to_vec()}]} else {vec![]};
            let message=build_message_recipients_html("me@example.com","guest@example.com","copy@example.com","hidden@example.com","Subject","Private <body>",Some(html),&attachments).unwrap();
            let raw=message.formatted();let parsed=mailparse::parse_mail(&raw).unwrap();
            let alternative=if with_attachment {assert_eq!(parsed.ctype.mimetype,"multipart/mixed");assert_eq!(parsed.subparts[1].get_body_raw().unwrap(),b"attachment bytes");&parsed.subparts[0]} else {&parsed};
            assert_eq!(alternative.ctype.mimetype,"multipart/alternative");assert_eq!(alternative.subparts[0].get_body().unwrap(),"Private <body>");assert_eq!(alternative.subparts[1].get_body().unwrap(),html);
        }
    }
}
