use crate::{
    CloudClient, CloudError, GmailAttachment, Result, attachment_mime, local_id, remote_id,
    validate_outgoing_attachments,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use chrono::DateTime;
use mail_core::{Account, MailAction, Message, OutgoingAttachment, ResetPhase, SyncBatch};
use mailparse::MailAddr;
use reqwest::{blocking::Client, redirect::Policy};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::time::Duration;
use url::Url;

const BASE: &str = "https://graph.microsoft.com/v1.0";
const START: &str = "https://graph.microsoft.com/v1.0/me/mailFolders/inbox/messages/delta?$select=id,conversationId,subject,from,toRecipients,ccRecipients,bccRecipients,replyTo,bodyPreview,receivedDateTime,isRead&$top=50";

fn attachments_url(account: &Account, message_id: &str) -> Result<Url> {
    let remote = remote_id(account, "graph-message", message_id)?;
    let mut url = base_message_url(remote)?;
    url.path_segments_mut()
        .map_err(|_| CloudError::Response("invalid Graph attachment URL"))?
        .push("attachments");
    Ok(url)
}

pub(super) fn attachments(
    client: &CloudClient,
    account: &Account,
    message_id: &str,
) -> Result<Vec<GmailAttachment>> {
    let mut url = attachments_url(account, message_id)?;
    url.query_pairs_mut()
        .append_pair("$select", "id,name,contentType,size,isInline,contentId");
    let value = client.get(url.as_str())?;
    Ok(value["value"]
        .as_array()
        .ok_or(CloudError::Response("Graph attachments missing"))?
        .iter()
        .filter_map(map_attachment)
        .collect())
}

fn map_attachment(item: &Value) -> Option<GmailAttachment> {
    Some(GmailAttachment {
        part_id: item["id"].as_str()?.to_string(),
        filename: item["name"].as_str().unwrap_or("Attachment").to_string(),
        mime_type: item["contentType"]
            .as_str()
            .unwrap_or("application/octet-stream")
            .to_string(),
        size: item["size"].as_u64().unwrap_or(0),
        content_id: item["contentId"]
            .as_str()
            .map(|value| {
                value
                    .trim()
                    .trim_start_matches('<')
                    .trim_end_matches('>')
                    .to_string()
            })
            .filter(|value| !value.is_empty()),
    })
}

pub(super) fn attachment_data(
    client: &CloudClient,
    account: &Account,
    message_id: &str,
    part_id: &str,
) -> Result<Vec<u8>> {
    let mut url = attachments_url(account, message_id)?;
    url.path_segments_mut()
        .map_err(|_| CloudError::Response("invalid Graph attachment URL"))?
        .push(part_id);
    let value = client.get(url.as_str())?;
    let data = value["contentBytes"]
        .as_str()
        .ok_or(CloudError::Unsupported(
            "this Outlook attachment type cannot be downloaded",
        ))?;
    if data.len() > crate::MAX_ATTACHMENT_BYTES.saturating_mul(4) / 3 + 4 {
        return Err(CloudError::Response("attachment exceeds 20 MiB limit"));
    }
    let bytes = STANDARD
        .decode(data)
        .map_err(|_| CloudError::Response("invalid attachment encoding"))?;
    if bytes.len() > crate::MAX_ATTACHMENT_BYTES {
        return Err(CloudError::Response("attachment exceeds 20 MiB limit"));
    }
    Ok(bytes)
}

pub(super) fn identity(client: &CloudClient) -> Result<(String, String)> {
    let profile = client.get(&format!(
        "{BASE}/me?$select=mail,userPrincipalName,displayName"
    ))?;
    let email = profile["mail"]
        .as_str()
        .filter(|s| !s.is_empty())
        .or_else(|| profile["userPrincipalName"].as_str())
        .ok_or(CloudError::Response("mail"))?
        .to_string();
    let name = profile["displayName"]
        .as_str()
        .unwrap_or(&email)
        .to_string();
    Ok((email, name))
}

pub(super) fn sync(
    client: &CloudClient,
    account: &Account,
    cursor: Option<&str>,
) -> Result<SyncBatch> {
    let (url, reset) = match cursor {
        Some(cursor) if cursor.starts_with('{') => {
            let saved: ResetCursor = serde_json::from_str(cursor)?;
            (saved.next_url, Some(ResetPhase::Continue))
        }
        Some(cursor) => (cursor.to_string(), None),
        None => (START.to_string(), None),
    };
    match sync_page(client, account, &url, reset) {
        Ok(batch) => Ok(batch),
        Err(CloudError::ApiStatus(410)) if cursor.is_some() => start_reset(client, account),
        Err(error) => Err(error),
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct ResetCursor {
    next_url: String,
}

fn start_reset(client: &CloudClient, account: &Account) -> Result<SyncBatch> {
    sync_page(client, account, START, Some(ResetPhase::Start))
}

fn sync_page(
    client: &CloudClient,
    account: &Account,
    url: &str,
    reset: Option<ResetPhase>,
) -> Result<SyncBatch> {
    let parsed = Url::parse(url)?;
    if parsed.scheme() != "https" || parsed.host_str() != Some("graph.microsoft.com") {
        return Err(CloudError::Response("invalid Graph delta cursor URL"));
    }
    let page = client.get(url)?;
    let mut ids = HashSet::new();
    let mut removed_message_ids = Vec::new();
    for item in page["value"]
        .as_array()
        .ok_or(CloudError::Response("Graph delta value"))?
    {
        let id = item["id"]
            .as_str()
            .ok_or(CloudError::Response("Graph message ID"))?;
        if item.get("@removed").is_some() {
            removed_message_ids.push(local_id(account, "graph-message", id));
        } else {
            ids.insert(id.to_string());
        }
    }
    let (messages, mut vanished, thread_labels) =
        fetch_messages(client, account, &ids.into_iter().collect::<Vec<_>>())?;
    removed_message_ids.append(&mut vanished);
    let next = page["@odata.nextLink"]
        .as_str()
        .or_else(|| page["@odata.deltaLink"].as_str())
        .ok_or(CloudError::Response("Graph delta cursor"))?;
    let parsed = Url::parse(next)?;
    if parsed.scheme() != "https" || parsed.host_str() != Some("graph.microsoft.com") {
        return Err(CloudError::Response("invalid Graph delta cursor URL"));
    }
    let more = page["@odata.nextLink"].as_str().is_some();
    let next_cursor = if reset.is_some() && more {
        serde_json::to_string(&ResetCursor {
            next_url: next.into(),
        })?
    } else {
        next.into()
    };
    Ok(SyncBatch {
        message_states: Vec::new(),
        messages,
        thread_labels,
        removed_message_ids,
        next_cursor,
        full_reset: false,
        reset_phase: reset.map(|phase| {
            if more {
                phase
            } else if phase == ResetPhase::Start {
                ResetPhase::StartComplete
            } else {
                ResetPhase::Complete
            }
        }),
    })
}

fn fetch_messages(
    client: &CloudClient,
    account: &Account,
    ids: &[String],
) -> Result<(Vec<Message>, Vec<String>, Vec<(String, Vec<String>)>)> {
    let mut messages = Vec::new();
    let mut removed = Vec::new();
    let mut labels = Vec::new();
    let size = ids.len().div_ceil(8).max(1);
    std::thread::scope(|scope| -> Result<()> {
        let handles: Vec<_> = ids
            .chunks(size)
            .map(|chunk| {
                scope.spawn(
                    move || -> Result<(Vec<Message>, Vec<String>, Vec<(String, Vec<String>)>)> {
                        let mut local = Vec::new();
                        let mut gone = Vec::new();
                        let mut local_labels = Vec::new();
                        for id in chunk {
                            let url = message_url(id)?;
                            match client.get(url.as_str()) {
                                Ok(value) => {
                                    let message = map_message(account, &value)?;
                                    let categories = value["categories"]
                                        .as_array()
                                        .into_iter()
                                        .flatten()
                                        .filter_map(|category| {
                                            category.as_str().map(ToString::to_string)
                                        })
                                        .collect();
                                    local_labels.push((message.thread_id.clone(), categories));
                                    local.push(message);
                                }
                                Err(CloudError::ApiStatus(404)) => {
                                    gone.push(local_id(account, "graph-message", id))
                                }
                                Err(error) => return Err(error),
                            }
                        }
                        Ok((local, gone, local_labels))
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
            labels.append(&mut ls);
        }
        Ok(())
    })?;
    let mut by_thread = std::collections::BTreeMap::<String, Vec<String>>::new();
    for (thread, categories) in labels {
        let current = by_thread.entry(thread).or_default();
        for category in categories {
            if !current.contains(&category) {
                current.push(category);
            }
        }
    }
    Ok((messages, removed, by_thread.into_iter().collect()))
}

fn base_message_url(id: &str) -> Result<Url> {
    let mut url = Url::parse(&format!("{BASE}/me/messages/"))?;
    url.path_segments_mut()
        .map_err(|_| CloudError::Response("invalid Graph URL"))?
        .pop_if_empty()
        .push(id);
    Ok(url)
}
fn message_url(id: &str) -> Result<Url> {
    let mut url = base_message_url(id)?;
    url.query_pairs_mut().append_pair("$select","id,conversationId,subject,from,toRecipients,ccRecipients,bccRecipients,replyTo,body,bodyPreview,receivedDateTime,isRead,categories,internetMessageHeaders,hasAttachments");
    Ok(url)
}
fn action_url(id: &str, action: &str) -> Result<Url> {
    let mut url = Url::parse(&format!("{BASE}/me/messages/"))?;
    url.path_segments_mut()
        .map_err(|_| CloudError::Response("invalid Graph URL"))?
        .pop_if_empty()
        .push(id)
        .push(action);
    Ok(url)
}

pub(crate) fn map_message(account: &Account, value: &Value) -> Result<Message> {
    let id = value["id"]
        .as_str()
        .ok_or(CloudError::Response("Graph message ID"))?;
    let conversation = value["conversationId"].as_str().unwrap_or(id);
    let address = &value["from"]["emailAddress"];
    let sender = match (address["name"].as_str(), address["address"].as_str()) {
        (Some(name), Some(email)) if !name.is_empty() => format!("{name} <{email}>"),
        (_, Some(email)) => email.to_string(),
        _ => "(unknown sender)".into(),
    };
    let to = value["toRecipients"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| {
            r["emailAddress"]["address"]
                .as_str()
                .map(ToString::to_string)
        })
        .collect();
    let content = value["body"]["content"].as_str().unwrap_or("");
    let content_type = value["body"]["contentType"].as_str().unwrap_or("");
    let (body_plain, body_html) = if content_type.eq_ignore_ascii_case("html") {
        (
            value["bodyPreview"].as_str().unwrap_or("").to_string(),
            Some(content.to_string()),
        )
    } else {
        (content.to_string(), None)
    };
    let received_at = value["receivedDateTime"]
        .as_str()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.timestamp_millis())
        .unwrap_or(0);
    Ok(Message {
        recipient_headers: Some(mail_core::RecipientHeaders {
            cc: recipient_addresses(value, "ccRecipients"),
            bcc: recipient_addresses(value, "bccRecipients"),
            reply_to: recipient_addresses(value, "replyTo"),
        }),
        id: local_id(account, "graph-message", id),
        thread_id: local_id(account, "graph-thread", conversation),
        account_id: account.id.clone(),
        sender,
        to,
        subject: value["subject"].as_str().unwrap_or("(no subject)").into(),
        body_plain,
        body_html,
        unsubscribe_header: value["internetMessageHeaders"]
            .as_array()
            .and_then(|headers| {
                headers.iter().find(|header| {
                    header["name"]
                        .as_str()
                        .is_some_and(|name| name.eq_ignore_ascii_case("List-Unsubscribe"))
                })
            })
            .and_then(|header| header["value"].as_str())
            .map(str::to_string),
        has_attachments: value["hasAttachments"].as_bool(),
        received_at,
        is_read: value["isRead"].as_bool().unwrap_or(false),
        is_draft: false,
        is_sent: false,
    })
}

fn outgoing_payload(to: &str, cc: &str, bcc: &str, subject: &str, body: &str) -> Result<Value> {
    if subject.contains(['\r', '\n']) {
        return Err(CloudError::InvalidHeader);
    }
    let parse = |raw: &str| {
        if raw.trim().is_empty() {
            Ok(Vec::new())
        } else {
            parse_recipients(raw)
        }
    };
    let to = parse(to)?;
    let cc = parse(cc)?;
    let bcc = parse(bcc)?;
    if to.is_empty() && cc.is_empty() && bcc.is_empty() {
        return Err(CloudError::InvalidHeader);
    }
    Ok(
        json!({"subject":subject,"body":{"contentType":"Text","content":body},"toRecipients":to,"ccRecipients":cc,"bccRecipients":bcc}),
    )
}

fn outgoing_payload_html(to:&str,cc:&str,bcc:&str,subject:&str,body:&str,body_html:Option<&str>)->Result<Value> {
    let mut payload=outgoing_payload(to,cc,bcc,subject,body)?;
    if let Some(html)=body_html {payload["body"]=json!({"contentType":"HTML","content":html});}
    Ok(payload)
}
pub(super) fn send_with_recipients_html(
    client: &CloudClient, to: &str, cc: &str, bcc: &str, subject: &str,
    body: &str, body_html: Option<&str>, attachments: &[OutgoingAttachment],
) -> Result<()> {
    validate_outgoing_attachments(attachments)?;
    let payload = outgoing_payload_html(to, cc, bcc, subject, body, body_html)?;
    if attachments.is_empty() {
        client.post(
            &format!("{BASE}/me/sendMail"),
            &json!({"message":payload,"saveToSentItems":true}),
        )?;
        return Ok(());
    }
    // Graph's attachment endpoints require a server-side draft. A failed
    // upload leaves the local draft untouched; the temporary remote draft is
    // best-effort removed so retrying does not accumulate copies.
    let remote = client.post(&format!("{BASE}/me/messages"), &payload)?;
    let id = remote["id"]
        .as_str()
        .ok_or(CloudError::Response("Graph draft ID missing"))?;
    let result = (|| {
        for attachment in attachments {
            attach_to_draft(client, id, attachment)?;
        }
        client.request(client.http.post(action_url(id, "send")?.as_str()))?;
        Ok(())
    })();
    if result.is_err() {
        if let Ok(url) = base_message_url(id) {
            let _ = client.delete(url.as_str());
        }
    }
    result
}

fn attach_to_draft(
    client: &CloudClient,
    draft_id: &str,
    attachment: &OutgoingAttachment,
) -> Result<()> {
    const SMALL_LIMIT: usize = 3_000_000;
    if attachment.bytes.len() < SMALL_LIMIT {
        let url = action_url(draft_id, "attachments")?;
        client.post(
            url.as_str(),
            &json!({
                "@odata.type":"#microsoft.graph.fileAttachment",
                "name":attachment.filename,
                "contentType":attachment_mime(attachment),
                "contentBytes":STANDARD.encode(&attachment.bytes),
            }),
        )?;
        return Ok(());
    }
    let mut url = base_message_url(draft_id)?;
    url.path_segments_mut()
        .map_err(|_| CloudError::Response("invalid Graph upload session URL"))?
        .push("attachments")
        .push("createUploadSession");
    let session = client.post(url.as_str(), &json!({
        "AttachmentItem":{"attachmentType":"file","name":attachment.filename,"size":attachment.bytes.len()}
    }))?;
    let upload_url = session["uploadUrl"]
        .as_str()
        .ok_or(CloudError::Response("Graph upload URL missing"))?;
    let upload_url = validate_upload_url(upload_url)?;
    // The upload URL itself is a short-lived bearer secret. Do not send our
    // OAuth token to it, follow redirects, or include it in error messages.
    let upload_client = Client::builder()
        .redirect(Policy::none())
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|_| CloudError::Response("Graph upload client unavailable"))?;
    const CHUNK: usize = 3 * 1024 * 1024;
    for start in (0..attachment.bytes.len()).step_by(CHUNK) {
        let end = (start + CHUNK).min(attachment.bytes.len());
        let response = upload_client
            .put(upload_url.as_str())
            .header("Content-Type", "application/octet-stream")
            .header(
                "Content-Range",
                format!("bytes {}-{}/{}", start, end - 1, attachment.bytes.len()),
            )
            .body(attachment.bytes[start..end].to_vec())
            .send()
            .map_err(|_| CloudError::Response("Graph attachment upload failed"))?;
        if !response.status().is_success() {
            return Err(CloudError::ApiStatus(response.status().as_u16()));
        }
    }
    Ok(())
}

fn validate_upload_url(raw: &str) -> Result<Url> {
    let url = Url::parse(raw).map_err(|_| CloudError::Response("invalid Graph upload URL"))?;
    let host = url.host_str().unwrap_or_default();
    if url.scheme() != "https"
        || url.username() != ""
        || url.password().is_some()
        || !(host == "outlook.office.com" || host.ends_with(".outlook.office.com"))
    {
        return Err(CloudError::Response("untrusted Graph upload URL"));
    }
    Ok(url)
}

fn recipient_addresses(value: &Value, key: &str) -> Vec<String> {
    value[key]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| {
            let address = r["emailAddress"]["address"].as_str()?;
            let name = r["emailAddress"]["name"].as_str().unwrap_or("");
            Some(if name.is_empty() {
                address.to_string()
            } else {
                format!("{name} <{address}>")
            })
        })
        .collect()
}
fn parse_recipients(to: &str) -> Result<Vec<Value>> {
    if to.contains(['\r', '\n']) {
        return Err(CloudError::InvalidHeader);
    }
    let addresses = mailparse::addrparse(to).map_err(|_| CloudError::InvalidHeader)?;
    let recipients: Vec<Value> = addresses.iter().flat_map(|item| match item {
        MailAddr::Single(single) => vec![json!({"emailAddress":{"address":single.addr,"name":single.display_name}})],
        MailAddr::Group(group) => group.addrs.iter().map(|single| json!({"emailAddress":{"address":single.addr,"name":single.display_name}})).collect(),
    }).collect();
    if recipients.is_empty() {
        return Err(CloudError::InvalidHeader);
    }
    Ok(recipients)
}

pub(super) fn apply_action(
    client: &CloudClient,
    account: &Account,
    action: &MailAction,
    message_ids: &[String],
) -> Result<()> {
    match action {
        MailAction::SendDraft { .. } => Err(CloudError::Unsupported(
            "send a local draft with send_plain after reviewing its content",
        )),
        MailAction::Archive { .. } => move_messages(client, account, message_ids, "archive"),
        MailAction::Move { folder, .. } => {
            let destination =
                if folder.eq_ignore_ascii_case("spam") || folder.eq_ignore_ascii_case("junk") {
                    "junkemail"
                } else if folder.eq_ignore_ascii_case("archive") {
                    "archive"
                } else if folder.eq_ignore_ascii_case("inbox") {
                    "inbox"
                } else {
                    folder.as_str()
                };
            move_messages(client, account, message_ids, destination)
        }
        MailAction::MarkRead { read, .. } => for_messages(client, account, message_ids, |id| {
            client.patch(base_message_url(id)?.as_str(), &json!({"isRead":read}))?;
            Ok(())
        }),
        MailAction::AddLabel { label, .. } => {
            set_category(client, account, message_ids, label, true)
        }
        MailAction::RemoveLabel { label, .. } => {
            set_category(client, account, message_ids, label, false)
        }
    }
}
fn folder_url(destination: &str) -> Result<Url> {
    let mut url = Url::parse(&format!("{BASE}/me/mailFolders/"))?;
    url.path_segments_mut()
        .map_err(|_| CloudError::Response("invalid Graph folder URL"))?
        .pop_if_empty()
        .push(destination);
    url.query_pairs_mut().append_pair("$select", "id");
    Ok(url)
}

fn message_folder_url(id: &str) -> Result<Url> {
    let mut url = base_message_url(id)?;
    url.query_pairs_mut()
        .append_pair("$select", "parentFolderId");
    Ok(url)
}

fn is_in_destination(message: &Value, destination_id: &str) -> Result<bool> {
    let parent = message["parentFolderId"]
        .as_str()
        .ok_or(CloudError::Response("Graph message parent folder missing"))?;
    Ok(parent == destination_id)
}

fn move_messages(
    client: &CloudClient,
    account: &Account,
    message_ids: &[String],
    destination: &str,
) -> Result<()> {
    if message_ids.is_empty() {
        return Err(CloudError::Response("thread has no cached messages"));
    }
    // Resolve the opaque folder ID once. Graph returns immutable message IDs,
    // but a retry must still avoid moving messages already processed earlier.
    let folder = client.get(folder_url(destination)?.as_str())?;
    let destination_id = folder["id"]
        .as_str()
        .ok_or(CloudError::Response("Graph destination folder ID missing"))?;
    for_messages(client, account, message_ids, |id| {
        let state_url = message_folder_url(id)?;
        let message = client.get(state_url.as_str())?;
        if is_in_destination(&message, destination_id)? {
            return Ok(());
        }
        match client.post(
            action_url(id, "move")?.as_str(),
            &json!({"destinationId":destination_id}),
        ) {
            Ok(_) => Ok(()),
            Err(CloudError::ApiStatus(404)) => {
                // A concurrent move may have completed after our read. Never
                // treat a missing message as success without checking its home.
                let current = client.get(state_url.as_str())?;
                if is_in_destination(&current, destination_id)? {
                    Ok(())
                } else {
                    Err(CloudError::ApiStatus(404))
                }
            }
            Err(error) => Err(error),
        }
    })
}
fn for_messages(
    client: &CloudClient,
    account: &Account,
    ids: &[String],
    mut f: impl FnMut(&str) -> Result<()>,
) -> Result<()> {
    let _ = client;
    if ids.is_empty() {
        return Err(CloudError::Response("thread has no cached messages"));
    }
    for local in ids {
        f(remote_id(account, "graph-message", local)?)?;
    }
    Ok(())
}
fn set_category(
    client: &CloudClient,
    account: &Account,
    ids: &[String],
    label: &str,
    add: bool,
) -> Result<()> {
    for_messages(client, account, ids, |id| {
        let message = client.get(message_url(id)?.as_str())?;
        let mut categories: Vec<String> = message["categories"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str().map(ToString::to_string))
            .collect();
        if add && !categories.iter().any(|s| s == label) {
            categories.push(label.to_string());
        }
        if !add {
            categories.retain(|s| s != label);
        }
        client.patch(
            base_message_url(id)?.as_str(),
            &json!({"categories":categories}),
        )?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mail_core::Provider;
    fn account() -> Account {
        Account {
            id: "acct-2".into(),
            provider: Provider::Microsoft,
            email: "alex@outlook.example".into(),
            display_name: "Alex".into(),
        }
    }
    #[test]
    fn maps_html_message_and_stable_ids() {
        let input = json!({"id":"AbC/123=","conversationId":"conv/1","subject":"Plan","from":{"emailAddress":{"name":"Maya","address":"maya@example.com"}},"toRecipients":[{"emailAddress":{"address":"alex@outlook.example"}}],"body":{"contentType":"html","content":"<p>Hi</p>"},"bodyPreview":"Hi","internetMessageHeaders":[{"name":"List-Unsubscribe","value":"<https://example.com/leave>"}],"hasAttachments":false,"receivedDateTime":"2026-11-06T12:00:00Z","isRead":true});
        let mapped = map_message(&account(), &input).unwrap();
        assert_eq!(mapped.id, "acct-2:graph-message:AbC/123=");
        assert_eq!(mapped.thread_id, "acct-2:graph-thread:conv/1");
        assert_eq!(mapped.body_html.as_deref(), Some("<p>Hi</p>"));
        assert_eq!(
            mapped.unsubscribe_header.as_deref(),
            Some("<https://example.com/leave>")
        );
        assert_eq!(mapped.has_attachments, Some(false));
        assert_eq!(mapped.body_plain, "Hi");
        assert_eq!(mapped.sender, "Maya <maya@example.com>");
        assert!(mapped.is_read);
    }

    #[test]
    fn upload_url_must_stay_on_microsoft_https_origin() {
        assert!(validate_upload_url("https://outlook.office.com/upload?authtoken=x").is_ok());
        assert!(validate_upload_url("http://outlook.office.com/upload?authtoken=x").is_err());
        assert!(
            validate_upload_url("https://outlook.office.com.evil.test/upload?authtoken=x").is_err()
        );
        assert!(validate_upload_url("https://user@outlook.office.com/upload?authtoken=x").is_err());
    }
    #[test]
    fn encodes_opaque_message_id_in_path() {
        let url = base_message_url("AbC/123=").unwrap();
        assert!(url.as_str().contains("AbC%2F123="));
        let url = attachments_url(&account(), "acct-2:graph-message:AbC/123=").unwrap();
        assert!(url.as_str().ends_with("/AbC%2F123=/attachments"));
    }
    #[test]
    fn reset_cursor_round_trips_opaque_graph_page_url() {
        let original = ResetCursor {
            next_url: "https://graph.microsoft.com/v1.0/me/mailFolders/inbox/messages/delta?$skiptoken=opaque%2Fnext".into(),
        };
        let encoded = serde_json::to_string(&original).unwrap();
        let restored: ResetCursor = serde_json::from_str(&encoded).unwrap();
        assert_eq!(restored.next_url, original.next_url);
    }
    #[test]
    fn keeps_inline_image_metadata_for_cid_rendering() {
        let item = json!({"id":"attachment-1","name":"logo.png","contentType":"image/png","size":42,"isInline":true,"contentId":"<logo@example>"});
        let mapped = map_attachment(&item).unwrap();
        assert_eq!(mapped.content_id.as_deref(), Some("logo@example"));
        assert_eq!(mapped.part_id, "attachment-1");
    }
    #[test]
    fn splits_rfc_mailboxes_into_graph_recipients() {
        let parsed = parse_recipients("\"Doe, Jane\" <jane@example.com>, bob@example.com").unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0]["emailAddress"]["address"], "jane@example.com");
        assert_eq!(parsed[0]["emailAddress"]["name"], "Doe, Jane");
        assert_eq!(parsed[1]["emailAddress"]["address"], "bob@example.com");
        assert!(parse_recipients("jane@example.com\r\nBcc: thief@example.com").is_err());
    }
    #[test]
    fn move_retry_checks_opaque_destination_id_without_case_folding() {
        assert!(
            is_in_destination(&json!({"parentFolderId":"Folder/AbC="}), "Folder/AbC=").unwrap()
        );
        assert!(
            !is_in_destination(&json!({"parentFolderId":"folder/AbC="}), "Folder/AbC=").unwrap()
        );
        assert!(
            is_in_destination(
                &json!({"id":"same-immutable-id","parentFolderId":"archive-id"}),
                "archive-id"
            )
            .unwrap()
        );
        assert!(
            !is_in_destination(
                &json!({"id":"same-immutable-id","parentFolderId":"inbox-id"}),
                "archive-id"
            )
            .unwrap()
        );
        assert!(is_in_destination(&json!({"id":"same-immutable-id"}), "archive-id").is_err());
    }
    #[test]
    fn move_status_urls_encode_opaque_ids() {
        let folder = folder_url("archive/opaque=").unwrap();
        assert!(
            folder.as_str().contains("/mailFolders/archive%2Fopaque="),
            "{folder}"
        );
        let message = message_folder_url("AbC/123=").unwrap();
        assert!(message.as_str().contains("/messages/AbC%2F123="));
        assert!(message.as_str().contains("parentFolderId"));
        let action = action_url("AbC/123=", "move").unwrap();
        assert!(action.as_str().ends_with("/messages/AbC%2F123=/move"));
    }
    #[test]
    fn graph_payload_separates_recipient_classes_and_allows_bcc_only() {
        let payload = outgoing_payload(
            "to@example.com",
            "Copy <cc@example.com>",
            "hidden@example.com",
            "Subject",
            "Body",
        )
        .unwrap();
        assert_eq!(
            payload["toRecipients"][0]["emailAddress"]["address"],
            "to@example.com"
        );
        assert_eq!(
            payload["ccRecipients"][0]["emailAddress"]["address"],
            "cc@example.com"
        );
        assert_eq!(
            payload["bccRecipients"][0]["emailAddress"]["address"],
            "hidden@example.com"
        );
        let blind = outgoing_payload("", "", "hidden@example.com", "Subject", "Body").unwrap();
        assert_eq!(blind["toRecipients"].as_array().unwrap().len(), 0);
        assert!(outgoing_payload("", "", "", "Subject", "Body").is_err());
    }
    #[test]
    fn optional_html_tracking_preserves_graph_delivery_lists_and_default_text() {
        let html="<p>Body</p><img src=\"https://scheduling.example.test/api/tracking/pixel/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.gif\">";
        let plain=outgoing_payload_html("guest@example.com","copy@example.com","hidden@example.com","Subject","Body",None).unwrap();
        assert_eq!(plain["body"]["contentType"],"Text");
        let tracked=outgoing_payload_html("guest@example.com","copy@example.com","hidden@example.com","Subject","Body",Some(html)).unwrap();
        assert_eq!(tracked["body"],json!({"contentType":"HTML","content":html}));
        for key in ["toRecipients","ccRecipients","bccRecipients","subject"] {assert_eq!(tracked[key],plain[key]);}
    }
}
