//! Gmail API and Microsoft Graph transports for the Email engine.
//!
//! [`CloudClient`] implements `mail_core::MailAdapter` for Gmail and Microsoft
//! 365/Outlook accounts: incremental sync with Gmail history IDs or Graph delta
//! links (and staged recovery when a cursor expires), sending, remote triage,
//! labels, attachments and calendar access.
//!
//! [`authorize_interactive`] runs a desktop OAuth flow with PKCE, the system
//! browser and a loopback redirect, using the [`OAuthConfig`] of an app
//! registration you supply. It returns a [`TokenSet`]; [`refresh`] renews it,
//! and [`revoke_google_token`] ends a Google grant.
//! The crate never persists credentials: after each API call, read
//! [`CloudClient::tokens`] and store refreshed tokens in an OS credential store.
//!
//! Calls block and can wait on the network or the browser, so run them on a
//! worker thread. Provider HTML is returned as received; sanitize it before
//! display. See the crate README for app-registration requirements.
mod calendar;
pub use calendar::{CalendarEventChanges, validate_event_version};
mod cleanup;
mod gmail;
pub use cleanup::GmailCleanupPage;
mod graph;
mod oauth;
mod quota;
pub mod release;

use mail_core::{
    Account, MailAction, MailAdapter, MailError, OutgoingAttachment, Provider, SyncBatch,
};
use reqwest::blocking::{Client, RequestBuilder};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{io::Read, sync::Mutex, time::Duration};

pub use oauth::{
    GOOGLE_REVOKE_ENDPOINT, authorize_google_calendar, authorize_google_photos,
    authorize_interactive, refresh, revoke_google_token,
};

pub use gmail::{GmailAttachment, GmailSendAs};

pub type Result<T> = std::result::Result<T, CloudError>;
const MAX_ATTACHMENT_BYTES: usize = 20 * 1024 * 1024;
const MAX_RESPONSE_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum CloudError {
    #[error("HTTP transport failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("JSON response failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("URL error: {0}")]
    Url(#[from] url::ParseError),
    #[error("provider returned HTTP {0}")]
    ApiStatus(u16),
    #[error("Calendar event changed. Refresh and review it before saving again.")]
    CalendarConflict,
    #[error("Gmail rate limit; retrying after {0} seconds. Cached mail remains available.")]
    RateLimited(u64),
    #[error("provider returned HTTP 403: {0}")]
    Forbidden(String),
    #[error("OAuth token exchange failed (HTTP {status}): {reason}")]
    OAuthToken { status: u16, reason: String },
    #[error("invalid provider response: {0}")]
    Response(&'static str),
    #[error("OAuth callback timed out")]
    AuthTimeout,
    #[error("OAuth callback state did not match")]
    AuthState,
    #[error("OAuth authorization was denied")]
    AuthDenied,
    #[error("could not open the system browser")]
    Browser,
    #[error("unsupported operation: {0}")]
    Unsupported(&'static str),
    #[error("account provider mismatch")]
    ProviderMismatch,
    #[error("invalid or unrecognized remote ID")]
    RemoteId,
    #[error("invalid message header")]
    InvalidHeader,
    #[error("invalid outgoing attachment: {0}")]
    InvalidAttachment(&'static str),
    #[error("MIME message error: {0}")]
    Message(#[from] lettre::error::Error),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CloudProvider {
    Gmail,
    Microsoft,
}

impl CloudProvider {
    pub fn core_provider(self) -> Provider {
        match self {
            Self::Gmail => Provider::Gmail,
            Self::Microsoft => Provider::Microsoft,
        }
    }
    pub fn scope(self) -> &'static str {
        match self {
            Self::Gmail => "https://www.googleapis.com/auth/gmail.modify",
            Self::Microsoft => "offline_access Mail.ReadWrite Mail.Send User.Read",
        }
    }
    pub fn auth_endpoint(self) -> &'static str {
        match self {
            Self::Gmail => "https://accounts.google.com/o/oauth2/v2/auth",
            Self::Microsoft => "https://login.microsoftonline.com/common/oauth2/v2.0/authorize",
        }
    }
    pub fn token_endpoint(self) -> &'static str {
        match self {
            Self::Gmail => "https://oauth2.googleapis.com/token",
            Self::Microsoft => "https://login.microsoftonline.com/common/oauth2/v2.0/token",
        }
    }
}

/// A desktop OAuth public client ID supplied by the application owner.
/// Google desktop OAuth may additionally provide a client secret; it is not treated as a secret.
#[derive(Clone, Serialize, Deserialize)]
pub struct OAuthConfig {
    pub provider: CloudProvider,
    pub client_id: String,
    pub client_secret: Option<String>,
}

/// Persist this only in the operating system credential vault, not the mail SQLite database.
#[derive(Clone, Serialize, Deserialize)]
pub struct TokenSet {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at_ms: i64,
}

pub struct CloudClient {
    config: OAuthConfig,
    tokens: Mutex<TokenSet>,
    http: Client,
    gmail_budget: std::sync::Arc<Mutex<quota::Budget>>,
}

impl CloudClient {
    pub fn new(config: OAuthConfig, tokens: TokenSet) -> Result<Self> {
        let http = Client::builder().timeout(Duration::from_secs(30)).build()?;
        let gmail_budget = quota::Budget::shared(
            &config.client_id,
            tokens
                .refresh_token
                .as_deref()
                .unwrap_or(&tokens.access_token),
        );
        Ok(Self {
            gmail_budget,
            config,
            tokens: Mutex::new(tokens),
            http,
        })
    }
    pub fn gmail_recipient_headers(
        &self,
        account: &Account,
        message_id: &str,
    ) -> Result<(Vec<String>, mail_core::RecipientHeaders)> {
        if self.provider() != CloudProvider::Gmail {
            return Err(CloudError::ProviderMismatch);
        }
        gmail::recipient_headers(self, account, message_id)
    }
    pub fn gmail_archive_page(
        &self,
        account: &Account,
        cursor: Option<&str>,
    ) -> Result<(SyncBatch, Option<String>)> {
        if self.provider() != CloudProvider::Gmail {
            return Err(CloudError::ProviderMismatch);
        }
        gmail::archive_page(self, account, cursor)
    }
    pub fn provider(&self) -> CloudProvider {
        self.config.provider
    }
    pub fn tokens(&self) -> TokenSet {
        self.tokens.lock().expect("token mutex poisoned").clone()
    }
    fn access_token(&self) -> Result<String> {
        let mut tokens = self.tokens.lock().expect("token mutex poisoned");
        if tokens.expires_at_ms <= now_ms() + 60_000 {
            *tokens = refresh(&self.config, &tokens)?;
        }
        Ok(tokens.access_token.clone())
    }
    fn request(&self, request: RequestBuilder) -> Result<Value> {
        let is_gmail = self.config.provider == CloudProvider::Gmail
            && request
                .try_clone()
                .and_then(|r| r.build().ok())
                .is_some_and(|r| r.url().host_str() == Some("gmail.googleapis.com"));
        if is_gmail {
            let built = request
                .try_clone()
                .ok_or(CloudError::Response("request cannot be paced"))?
                .build()?;
            let delay = self
                .gmail_budget
                .lock()
                .expect("quota budget")
                .reserve(quota::cost(built.method(), built.url()))?;
            std::thread::sleep(delay);
            self.gmail_budget
                .lock()
                .expect("quota budget")
                .check_after_wait()?;
        }
        let token = self.access_token()?;
        let mut request = request.bearer_auth(token);
        if self.config.provider == CloudProvider::Microsoft {
            request = request.header("Prefer", "IdType=\"ImmutableId\"");
        }
        let response = request.send()?;
        let status = response.status();
        if !status.is_success() {
            let retry_after = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|h| h.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(60)
                .clamp(1, 3600);
            if is_gmail && status.as_u16() == 429 {
                self.gmail_budget
                    .lock()
                    .expect("quota budget")
                    .cool_down(retry_after);
                return Err(CloudError::RateLimited(retry_after));
            }
            if status.as_u16() == 403 {
                let mut body = Vec::new();
                response.take(8193).read_to_end(&mut body)?;
                let reason = serde_json::from_slice::<Value>(&body)
                    .ok()
                    .and_then(|value| value["error"]["message"].as_str().map(str::to_owned))
                    .unwrap_or_else(|| "access denied by provider".to_string());
                let lower = String::from_utf8_lossy(&body).to_ascii_lowercase();
                if is_gmail
                    && (lower.contains("ratelimitexceeded")
                        || lower.contains("quota exceeded")
                        || lower.contains("userratelimitexceeded"))
                {
                    self.gmail_budget
                        .lock()
                        .expect("quota budget")
                        .cool_down(retry_after);
                    return Err(CloudError::RateLimited(retry_after));
                }
                return Err(CloudError::Forbidden(reason.chars().take(240).collect()));
            }
            return Err(CloudError::ApiStatus(status.as_u16()));
        }
        let mut body = Vec::new();
        response
            .take(MAX_RESPONSE_BYTES + 1)
            .read_to_end(&mut body)?;
        if body.len() as u64 > MAX_RESPONSE_BYTES {
            return Err(CloudError::Response(
                "provider response exceeds 32 MiB limit",
            ));
        }
        if body.is_empty() {
            return Ok(Value::Null);
        }
        Ok(serde_json::from_slice(&body)?)
    }
    fn get(&self, url: &str) -> Result<Value> {
        self.request(self.http.get(url))
    }

    /// Google's contact search indexes require an empty-query request before use.
    pub fn warm_google_contact_search(&self) -> Result<()> {
        for (endpoint, mask) in [
            (
                "https://people.googleapis.com/v1/people:searchContacts",
                "emailAddresses,photos",
            ),
            (
                "https://people.googleapis.com/v1/otherContacts:search",
                "emailAddresses",
            ),
        ] {
            let mut url = url::Url::parse(endpoint)?;
            url.query_pairs_mut()
                .append_pair("query", "")
                .append_pair("readMask", mask);
            match self.get(url.as_str()) {
                Ok(_) | Err(CloudError::Forbidden(_)) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    fn other_contact_photo_url(&self, resource: &str, address: &str) -> Result<Option<String>> {
        // otherContacts.search cannot return photos, and people.get does not
        // accept otherContacts/* resource names. List only after an exact hit.
        let mut page_token: Option<String> = None;
        for _ in 0..20 {
            let mut url = url::Url::parse("https://people.googleapis.com/v1/otherContacts")?;
            url.query_pairs_mut()
                .append_pair("readMask", "emailAddresses,photos")
                .append_pair("sources", "READ_SOURCE_TYPE_CONTACT")
                .append_pair("sources", "READ_SOURCE_TYPE_PROFILE")
                .append_pair("pageSize", "1000");
            if let Some(token) = &page_token {
                url.query_pairs_mut().append_pair("pageToken", token);
            }
            let page = self.get(url.as_str())?;
            for person in page["otherContacts"].as_array().into_iter().flatten() {
                if person["resourceName"].as_str() != Some(resource) {
                    continue;
                }
                let exact = person["emailAddresses"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|email| {
                        email["value"]
                            .as_str()
                            .is_some_and(|value| value.eq_ignore_ascii_case(address))
                    });
                if !exact {
                    return Ok(None);
                }
                return Ok(person["photos"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|photo| photo["default"] != true)
                    .and_then(|photo| photo["url"].as_str())
                    .map(str::to_owned));
            }
            page_token = page["nextPageToken"].as_str().map(str::to_owned);
            if page_token.is_none() {
                break;
            }
        }
        Ok(None)
    }

    /// Find a sender photo in this Google account's saved or auto-saved contacts.
    /// Results are matched by the complete email address, never a name prefix.
    pub fn google_contact_photo(&self, address: &str) -> Result<Option<Vec<u8>>> {
        if self.config.provider != CloudProvider::Gmail
            || address.len() > 254
            || !address.is_ascii()
            || address.contains(['\r', '\n'])
            || address.split('@').count() != 2
        {
            return Err(CloudError::Response("invalid Google contact address"));
        }
        let mut photo_url = None;
        for (endpoint, field) in [
            (
                "https://people.googleapis.com/v1/people:searchContacts",
                "results",
            ),
            (
                "https://people.googleapis.com/v1/otherContacts:search",
                "results",
            ),
        ] {
            let mut url = url::Url::parse(endpoint)?;
            let mask = if endpoint.contains("otherContacts") {
                "emailAddresses"
            } else {
                "emailAddresses,photos"
            };
            url.query_pairs_mut()
                .append_pair("query", address)
                .append_pair("readMask", mask)
                .append_pair("pageSize", "30");
            let result = match self.get(url.as_str()) {
                Ok(result) => result,
                // A user may grant only one of the two contact permissions.
                Err(CloudError::Forbidden(_)) => continue,
                Err(error) => return Err(error),
            };
            for hit in result[field].as_array().into_iter().flatten() {
                let person = hit.get("person").unwrap_or(hit);
                let exact = person["emailAddresses"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|email| {
                        email["value"]
                            .as_str()
                            .is_some_and(|value| value.eq_ignore_ascii_case(address))
                    });
                if !exact {
                    continue;
                }
                if let Some(url) = person["photos"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|photo| photo["default"] != true)
                    .and_then(|photo| photo["url"].as_str())
                {
                    photo_url = Some(url.to_owned());
                    break;
                }
                // Other-contact search excludes photos; list only after an exact match.
                if let Some(resource) = person["resourceName"]
                    .as_str()
                    .filter(|r| r.starts_with("otherContacts/"))
                {
                    photo_url = self.other_contact_photo_url(resource, address)?;
                    if photo_url.is_some() {
                        break;
                    }
                }
            }
            if photo_url.is_some() {
                break;
            }
        }
        let Some(raw_url) = photo_url else {
            return Ok(None);
        };
        let url = url::Url::parse(&raw_url)?;
        let host = url.host_str().unwrap_or("");
        if url.scheme() != "https"
            || !(host == "googleusercontent.com" || host.ends_with(".googleusercontent.com"))
            || url.username() != ""
            || url.password().is_some()
        {
            return Ok(None);
        }
        let http = Client::builder()
            .timeout(Duration::from_secs(8))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        let response = http.get(url).send()?;
        if !response.status().is_success()
            || !response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| v.starts_with("image/"))
        {
            return Ok(None);
        }
        let mut data = Vec::new();
        response.take(1_048_577).read_to_end(&mut data)?;
        Ok((!data.is_empty() && data.len() <= 1_048_576).then_some(data))
    }
    fn post(&self, url: &str, body: &Value) -> Result<Value> {
        self.request(self.http.post(url).json(body))
    }
    fn patch(&self, url: &str, body: &Value) -> Result<Value> {
        self.request(self.http.patch(url).json(body))
    }
    fn delete(&self, url: &str) -> Result<Value> {
        self.request(self.http.delete(url))
    }

    /// Resolve the signed-in mailbox after OAuth to create a local Account.
    pub fn identity(&self) -> Result<(String, String)> {
        match self.config.provider {
            CloudProvider::Gmail => gmail::identity(self),
            CloudProvider::Microsoft => graph::identity(self),
        }
    }
    pub fn sync_batch(&self, account: &Account, cursor: Option<&str>) -> Result<SyncBatch> {
        if account.provider != self.config.provider.core_provider() {
            return Err(CloudError::ProviderMismatch);
        }
        match self.config.provider {
            CloudProvider::Gmail => gmail::sync(self, account, cursor),
            CloudProvider::Microsoft => graph::sync(self, account, cursor),
        }
    }
    pub fn send_plain(&self, account: &Account, to: &str, subject: &str, body: &str) -> Result<()> {
        self.send_with_attachments(account, to, subject, body, &[])
    }
    pub fn send_with_attachments(
        &self,
        account: &Account,
        to: &str,
        subject: &str,
        body: &str,
        attachments: &[OutgoingAttachment],
    ) -> Result<()> {
        self.send_with_attachments_from(account, &account.email, to, subject, body, attachments)
    }
    pub fn send_as_aliases(&self) -> Result<Vec<GmailSendAs>> {
        if self.config.provider != CloudProvider::Gmail {
            return Err(CloudError::ProviderMismatch);
        }
        gmail::send_as_aliases(self)
    }
    pub fn send_with_attachments_from(
        &self,
        account: &Account,
        from: &str,
        to: &str,
        subject: &str,
        body: &str,
        attachments: &[OutgoingAttachment],
    ) -> Result<()> {
        self.send_with_recipients_from(account, from, to, "", "", subject, body, attachments)
    }
    pub fn send_with_recipients_from(
        &self,
        account: &Account,
        from: &str,
        to: &str,
        cc: &str,
        bcc: &str,
        subject: &str,
        body: &str,
        attachments: &[OutgoingAttachment],
    ) -> Result<()> {
        self.send_in_thread(account, from, to, cc, bcc, subject, body, attachments, None).map(|_| ())
    }
    pub fn send_in_thread(
        &self, account: &Account, from: &str, to: &str, cc: &str, bcc: &str,
        subject: &str, body: &str, attachments: &[OutgoingAttachment], thread_id: Option<&str>,
    ) -> Result<Option<String>> {
        self.send_in_thread_html(account,from,to,cc,bcc,subject,body,None,attachments,thread_id)
    }
    pub fn send_in_thread_html(
        &self, account: &Account, from: &str, to: &str, cc: &str, bcc: &str,
        subject: &str, body: &str, body_html: Option<&str>, attachments: &[OutgoingAttachment], thread_id: Option<&str>,
    ) -> Result<Option<String>> {
        if account.provider != self.config.provider.core_provider() {
            return Err(CloudError::ProviderMismatch);
        }
        validate_outgoing_attachments(attachments)?;
        match self.config.provider {
            CloudProvider::Gmail => {
                gmail::send_in_thread_html(self, account, from, to, cc, bcc, subject, body, body_html, attachments, thread_id)
            }
            CloudProvider::Microsoft => {
                if !from.eq_ignore_ascii_case(&account.email) {
                    return Err(CloudError::InvalidHeader);
                }
                graph::send_with_recipients_html(self, to, cc, bcc, subject, body, body_html, attachments).map(|_| None)
            }
        }
    }
    pub fn attachments(&self, account: &Account, message_id: &str) -> Result<Vec<GmailAttachment>> {
        if account.provider != self.config.provider.core_provider() {
            return Err(CloudError::ProviderMismatch);
        }
        match self.config.provider {
            CloudProvider::Gmail => gmail::attachments(self, account, message_id),
            CloudProvider::Microsoft => graph::attachments(self, account, message_id),
        }
    }
    pub fn attachment_data(
        &self,
        account: &Account,
        message_id: &str,
        part_id: &str,
    ) -> Result<Vec<u8>> {
        if account.provider != self.config.provider.core_provider() {
            return Err(CloudError::ProviderMismatch);
        }
        match self.config.provider {
            CloudProvider::Gmail => gmail::attachment_data(self, account, message_id, part_id),
            CloudProvider::Microsoft => graph::attachment_data(self, account, message_id, part_id),
        }
    }
    /// Gmail bulk cleanup operates on cached message IDs, in bounded requests.
    pub fn gmail_bulk_mark_read(&self, account: &Account, message_ids: &[String]) -> Result<()> {
        if self.config.provider != CloudProvider::Gmail
            || account.provider != mail_core::Provider::Gmail
        {
            return Err(CloudError::ProviderMismatch);
        }
        gmail::bulk_mark_read(self, account, message_ids)
    }
    /// `message_ids` are the account-namespaced local IDs in the target thread.
    pub fn apply_action(
        &self,
        account: &Account,
        action: &MailAction,
        message_ids: &[String],
    ) -> Result<()> {
        if account.provider != self.config.provider.core_provider() {
            return Err(CloudError::ProviderMismatch);
        }
        match self.config.provider {
            CloudProvider::Gmail => gmail::apply_action(self, account, action),
            CloudProvider::Microsoft => graph::apply_action(self, account, action, message_ids),
        }
    }
}

pub(crate) fn validate_outgoing_attachments(items: &[OutgoingAttachment]) -> Result<()> {
    const MAX_FILE: usize = 20 * 1024 * 1024;
    const MAX_TOTAL: usize = 25 * 1024 * 1024;
    if items.len() > 10 {
        return Err(CloudError::InvalidAttachment("too many files"));
    }
    let mut total = 0usize;
    for item in items {
        if item.filename.is_empty()
            || item.filename.len() > 255
            || item
                .filename
                .chars()
                .any(|c| c.is_control() || matches!(c, '/' | '\\'))
        {
            return Err(CloudError::InvalidAttachment("invalid filename"));
        }
        if item.bytes.len() > MAX_FILE {
            return Err(CloudError::InvalidAttachment("file exceeds 20 MiB"));
        }
        total = total.saturating_add(item.bytes.len());
        if total > MAX_TOTAL {
            return Err(CloudError::InvalidAttachment("attachments exceed 25 MiB"));
        }
        let mime = if item.mime_type.is_empty() {
            "application/octet-stream"
        } else {
            item.mime_type.as_str()
        };
        let Some((kind, subtype)) = mime.split_once('/') else {
            return Err(CloudError::InvalidAttachment("invalid MIME type"));
        };
        if kind.is_empty()
            || subtype.is_empty()
            || mime.len() > 127
            || !mime
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'+' | b'.' | b'/'))
        {
            return Err(CloudError::InvalidAttachment("invalid MIME type"));
        }
    }
    Ok(())
}

pub(crate) fn attachment_mime(item: &OutgoingAttachment) -> &str {
    if item.mime_type.is_empty() {
        "application/octet-stream"
    } else {
        &item.mime_type
    }
}

impl MailAdapter for CloudClient {
    fn sync_with_cached_messages(
        &self,
        account: &Account,
        cursor: Option<&str>,
        cached: &std::collections::HashMap<String, String>,
    ) -> mail_core::Result<SyncBatch> {
        if self.provider() == CloudProvider::Gmail {
            gmail::sync_with_cache(self, account, cursor, cached)
                .map_err(|e| MailError::Provider(e.to_string()))
        } else {
            self.sync(account, cursor)
        }
    }
    fn provider(&self) -> Provider {
        self.config.provider.core_provider()
    }
    fn sync(&self, account: &Account, cursor: Option<&str>) -> mail_core::Result<SyncBatch> {
        self.sync_batch(account, cursor)
            .map_err(|e| MailError::Provider(e.to_string()))
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
fn remote_id<'a>(account: &Account, kind: &str, local: &'a str) -> Result<&'a str> {
    let prefix = format!("{}:{kind}:", account.id);
    local.strip_prefix(&prefix).ok_or(CloudError::RemoteId)
}
fn local_id(account: &Account, kind: &str, remote: &str) -> String {
    format!("{}:{kind}:{remote}", account.id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
    };
    #[test]
    fn accepts_empty_success_body_without_reporting_failed_send() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/send", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 1024];
            let _ = stream.read(&mut request).unwrap();
            stream
                .write_all(
                    b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .unwrap();
        });
        let client = CloudClient::new(
            OAuthConfig {
                provider: CloudProvider::Microsoft,
                client_id: "test".into(),
                client_secret: None,
            },
            TokenSet {
                access_token: "test".into(),
                refresh_token: None,
                expires_at_ms: now_ms() + 600_000,
            },
        )
        .unwrap();
        assert_eq!(
            client.post(&url, &serde_json::json!({})).unwrap(),
            Value::Null
        );
        server.join().unwrap();
    }
}
