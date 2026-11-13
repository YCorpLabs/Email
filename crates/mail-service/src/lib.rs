//! Account service and JSON API that the Email desktop shells and MCP server use.
//!
//! [`MailService`] combines `mail-core`'s cache with the `mail-imap` and
//! `mail-cloud` adapters. It owns account configuration, OS credential storage
//! (macOS Keychain or the Linux Secret Service), sync coordination, safe HTML
//! rendering, attachments, drafts, scheduled sending and the other operations a
//! front end needs. Open it with [`MailService::open`] on a database path and
//! send requests with [`MailService::request_json`]; the request and response
//! shapes are documented in `docs/service-api.md`.
//!
//! The same API is exported as a C ABI (`include/mail_service.h`:
//! `mail_service_open`, `mail_service_request_json`, `mail_service_free_string`,
//! `mail_service_close` and `mail_service_distribution_profile_json`), which the
//! Swift app loads as a dynamic library.
//! A new front end in any language can drive the engine through it.
//!
//! Network operations block and belong on a worker thread. Account passwords
//! and OAuth tokens go only to the OS credential store, never to SQLite or logs.
mod agent_access;
mod agenda;
#[cfg(any(target_os = "macos", test))]
mod credential_access;
mod lifecycle;
use lifecycle::{AccountCloudClient, CachedTokens, CredentialStore, OsCredentials};
mod booking;
mod store_location;
pub use store_location::{canonical_store_path, default_store_directory, is_default_store};
mod calendar_view;
mod cleanup;
#[cfg(feature = "test-fixture")]
mod fixture_credentials;
#[cfg(test)]
mod gmail_imap_tests;
pub mod profile;
mod tracking;
use base64::{Engine, engine::general_purpose::STANDARD};
use mail_cloud::{
    CloudClient, CloudProvider, OAuthConfig, TokenSet, authorize_google_photos,
    authorize_interactive,
};
use mail_core::{Account, MailAction, MailStore, Provider};
use mail_cloud::release;
use mail_imap::{
    GMAIL_IMAP_HOST, ImapConfig, ImapConnector, ServerHint, SetupProblem, TransportSecurity,
    attachment_bytes, attachment_metadata,
};
use percent_encoding::percent_decode_str;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    ffi::{CStr, CString, c_char},
    fs,
    panic::{AssertUnwindSafe, catch_unwind},
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use uuid::Uuid;

// v4 copies authorized historical values into items created by the current app.
// A matching designated requirement alone does not migrate historical XARA
// partitions. Preserve the development launcher and verify silent read-back.
// Every service name derives from the distribution profile's namespace. The
// development profile keeps its historical namespace, so its items are unchanged;
// other profiles can never read or overwrite those items.
#[cfg(target_os = "macos")]
const KEYRING_SERVICE: &str = concat!(env!("EMAIL_PROFILE_KEYCHAIN_NAMESPACE"), ".v4");
#[cfg(target_os = "macos")]
const PREVIOUS_KEYRING_SERVICE: Option<&str> =
    Some(concat!(env!("EMAIL_PROFILE_KEYCHAIN_NAMESPACE"), ".v3"));
#[cfg(target_os = "macos")]
const SECOND_KEYRING_SERVICE: &str = concat!(env!("EMAIL_PROFILE_KEYCHAIN_NAMESPACE"), ".v2");
#[cfg(not(target_os = "macos"))]
const KEYRING_SERVICE: &str = concat!(env!("EMAIL_PROFILE_KEYCHAIN_NAMESPACE"), ".v2");
#[cfg(not(target_os = "macos"))]
const PREVIOUS_KEYRING_SERVICE: Option<&str> = None;
const LEGACY_KEYRING_SERVICE: &str = env!("EMAIL_PROFILE_KEYCHAIN_NAMESPACE");

#[derive(Clone, Serialize, Deserialize)]
struct SavedConfig {
    account_id: String,
    #[serde(default)]
    generation: String,
    provider: Provider,
    username: Option<String>,
    smtp_username: Option<String>,
    imap_host: Option<String>,
    imap_port: Option<u16>,
    imap_security: Option<TransportSecurity>,
    smtp_host: Option<String>,
    smtp_port: Option<u16>,
    smtp_security: Option<TransportSecurity>,
    archive_mailbox: Option<String>,
    client_id: Option<String>,
    client_secret: Option<String>,
    #[serde(default)]
    google_photos_enabled: bool,
    #[serde(default)]
    google_calendar_enabled: bool,
}

/// Account-setup presets that still use the generic IMAP path.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ImapPreset {
    /// Gmail with a Google app password (no OAuth app involved).
    Gmail,
}

/// `add_imap_account`: a password-based account (iCloud, the Gmail app-password
/// preset, or any IMAP server). Server fields may be left out for `imap`
/// without a preset; the engine then guesses them from the address
/// (`mail_imap::guess_servers`) and reports `servers_not_found` with its guess
/// if they don't answer.
#[derive(Deserialize)]
struct AddImapAccount {
    provider: Provider,
    email: String,
    display_name: String,
    username: Option<String>,
    smtp_username: Option<String>,
    imap_host: Option<String>,
    imap_port: Option<u16>,
    imap_security: Option<TransportSecurity>,
    smtp_host: Option<String>,
    smtp_port: Option<u16>,
    smtp_security: Option<TransportSecurity>,
    archive_mailbox: Option<String>,
    password: String,
    #[serde(default)]
    preset: Option<ImapPreset>,
}

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum Request {
    /// Demo data for a disposable store: `set` is "small" (the default, three
    /// messages) or "launch" (the launch demo mailbox).
    SeedDemo {
        set: Option<String>,
        utc_offset_minutes: Option<i32>,
    },
    /// What this build offers; both native shells shape account setup with it.
    BuildFeatures,
    /// The provider an address belongs to, from its domain alone (no network):
    /// account setup opens that provider's steps after the person enters it.
    SetupProvider {
        email: String,
    },
    /// Desktop-only agent access operations. Never add these to mail-mcp's policy.
    AgentAccessStatus,
    AgentAccessOverview {
        history_limit: Option<u32>,
    },
    AgentAccessDisclosure {
        account_id: String,
    },
    GrantAgentAccess {
        account_id: String,
        grants: Vec<String>,
        disclosure_version: i64,
    },
    RevokeAgentAccess {
        account_id: String,
    },
    ListAgentActions {
        status: Option<String>,
        limit: Option<u32>,
    },
    AgentPendingSummary,
    ApproveAgentAction {
        id: String,
        undo_seconds: Option<u32>,
    },
    RejectAgentAction {
        id: String,
    },
    AgentRequestAttachment {
        id: String,
        attachment_id: String,
    },
    BookingStatus,
    BookingSetupStatus,
    BookingSetupRead,
    BookingSetupStart { name: Option<String> },
    BookingSetupPoll,
    BookingSetupPreview { patch: Value },
    BookingSetupApply { plan_id: String, confirm: bool },
    BookingConnectStart {
        name: Option<String>,
    },
    BookingConnectPoll,
    BookingDisconnect,
    BookingList {
        #[serde(default)]
        refresh: bool,
    },
    BookingLink {
        meeting_type_id: String,
    },
    BookingAvailability {
        meeting_type_id: String,
    },
    BookingAnalytics {
        #[serde(default)]
        refresh: bool,
    },
    TrackingGet { message_id: String },
    TrackingCapability,
    TrackingSet { message_id: String, enabled: bool },
    TrackingNotifications { enabled: bool, session_since: i64 },
    TrackingActivity { message_id: Option<String>, #[serde(default)] refresh: bool },
    CalendarStatus {
        account_id: String,
    },
    ConnectCalendar {
        account_id: String,
    },
    ListCalendars {
        account_id: Option<String>,
    },
    ListAgenda {
        account_id: Option<String>,
        start_ms: i64,
        end_ms: i64,
        limit: Option<usize>,
        #[serde(default)]
        deduplicate_shared: bool,
        #[serde(default)]
        hidden_sources: Vec<calendar_view::CalendarSource>,
    },
    SyncCalendar {
        account_id: String,
        start_ms: i64,
        end_ms: i64,
    },
    RespondCalendarInvitation {
        account_id: String,
        calendar_id: String,
        event_id: String,
        response: String,
    },
    CreateCalendarEvent {
        account_id: String,
        calendar_id: String,
        title: String,
        start_ms: i64,
        end_ms: i64,
        attendee_emails: Vec<String>,
        description: Option<String>,
        location: Option<String>,
    },
    UpdateCalendarEvent {
        account_id: String,
        calendar_id: String,
        event_id: String,
        expected_etag: String,
        edit_scope: Option<String>,
        title: Option<String>,
        start_ms: Option<i64>,
        end_ms: Option<i64>,
        description: Option<String>,
        location: Option<String>,
    },
    Availability {
        account_id: Option<String>,
        start_ms: i64,
        end_ms: i64,
        duration_minutes: u32,
    },
    ListCalendarInvitations {
        account_id: Option<String>,
        limit: Option<usize>,
    },
    CalendarInvitation {
        message_id: String,
    },
    RefreshCalendarInvitation {
        message_id: String,
    },
    SetFollowup {
        thread_id: String,
        due_at_ms: i64,
        only_if_no_reply: bool,
        #[serde(default)]
        sent_message_id: Option<String>,
    },
    ListFollowups {
        account_id: Option<String>,
    },
    ListFollowupThreads {
        account_id: Option<String>,
    },
    CancelFollowup {
        thread_id: String,
    },
    GetSignature {
        account_id: String,
        email: String,
    },
    UpsertSignature {
        account_id: String,
        email: String,
        body: String,
        for_new: bool,
        for_reply: bool,
    },
    DeleteSignature {
        account_id: String,
        email: String,
    },
    UnreadCounts,
    SuggestRecipients {
        account_id: String,
        query: String,
        #[serde(default = "default_recipient_limit")]
        limit: usize,
    },
    CreateSenderCleanup {
        account_id: String,
        sender: String,
    },
    ListSenderCleanups {
        account_id: Option<String>,
    },
    GetSenderCleanup {
        job_id: String,
    },
    StepSenderCleanup {
        job_id: String,
    },
    ConfirmSenderCleanup {
        job_id: String,
    },
    PauseSenderCleanup {
        job_id: String,
    },
    ResumeSenderCleanup {
        job_id: String,
    },
    CancelSenderCleanup {
        job_id: String,
    },
    UnreadFromSender {
        account_id: String,
        sender: String,
    },
    BulkMarkRead {
        operation_id: String,
        account_id: String,
        thread_ids: Vec<String>,
    },
    BatchTriage {
        operation_id: String,
        kind: String,
        thread_ids: Vec<String>,
    },
    LatestTriageUndo,
    UndoTriage {
        operation_id: String,
    },
    ListAccounts,
    ListSendAs {
        account_id: String,
    },
    ListThreads {
        account_id: Option<String>,
        split_id: Option<String>,
        unread_only: Option<bool>,
        limit: Option<u32>,
        offset: Option<u32>,
    },
    ListLabels,
    ListLabelThreads {
        label: String,
        query: Option<String>,
        limit: Option<u32>,
        offset: Option<u32>,
    },
    ListArchived {
        limit: Option<u32>,
        offset: Option<u32>,
    },
    SyncArchives {
        restart: Option<bool>,
    },
    ListSnippets,
    UpsertSnippet {
        id: Option<String>,
        name: String,
        body: String,
    },
    DeleteSnippet {
        id: String,
    },
    ListSplits,
    PreviewSplit {
        query: String,
    },
    UpsertSplit {
        id: Option<String>,
        name: String,
        query: String,
    },
    DeleteSplit {
        id: String,
    },
    SnoozeThread {
        thread_id: String,
        wake_at_ms: i64,
    },
    UnsnoozeThread {
        thread_id: String,
    },
    ListSnoozedThreads,
    ListPinnedThreads {
        limit: Option<u32>,
        offset: Option<u32>,
    },
    SetThreadPinned {
        thread_id: String,
        pinned: bool,
    },
    ScheduleDraft {
        message_id: String,
        send_at_ms: i64,
    },
    QueueSendDraft {
        message_id: String,
        undo_seconds: Option<u32>,
    },
    CancelScheduledDraft {
        message_id: String,
    },
    ListOutbox,
    DispatchDue,
    AddDraftAttachment {
        message_id: String,
        filename: String,
        mime_type: String,
        content_base64: String,
    },
    ListDraftAttachments {
        message_id: String,
    },
    RemoveDraftAttachment {
        message_id: String,
        attachment_id: String,
    },
    Search {
        query: String,
        limit: Option<u32>,
        offset: Option<u32>,
        is_inbox: Option<bool>,
        account_id: Option<String>,
        unread_only: Option<bool>,
        starred_only: Option<bool>,
        label: Option<String>,
        folder: Option<String>,
        split_id: Option<String>,
        snoozed_only: Option<bool>,
        followups_only: Option<bool>,
    },
    GetLatestMessageForThread {
        thread_id: String,
    },
    GetMessage {
        id: String,
    },
    RefreshRecipientHeaders {
        thread_id: String,
    },
    ListThreadMessages {
        thread_id: String,
    },
    ListDrafts {
        account_id: Option<String>,
    },
    RenderMessage {
        message_id: String,
        include_inline_images: Option<bool>,
    },
    ListAttachments {
        message_id: String,
    },
    FetchAttachment {
        message_id: String,
        part_id: String,
    },
    PendingActions,
    AddImapAccount(AddImapAccount),
    StartOauth {
        provider: Provider,
        client_id: String,
        client_secret: Option<String>,
    },
    EnableGooglePhotos {
        account_id: String,
    },
    GooglePhotoState {
        account_id: String,
    },
    GoogleContactPhoto {
        account_id: String,
        address: String,
    },
    ReloadAccountCredentials {
        account_id: String,
    },
    RemoveAccount {
        account_id: String,
    },
    SyncAccount {
        account_id: String,
    },
    SyncAll,
    ComposeDraft {
        account_id: String,
        reply_thread_id: Option<String>,
        from: Option<String>,
        to: String,
        cc: Option<String>,
        bcc: Option<String>,
        subject: String,
        body_plain: String,
    },
    UpdateDraft {
        message_id: String,
        from: Option<String>,
        to: String,
        cc: Option<String>,
        bcc: Option<String>,
        subject: String,
        body_plain: String,
    },
    SendDraft {
        message_id: String,
    },
    DiscardDraft {
        message_id: String,
    },
    RestoreDiscardedDraft {
        message_id: String,
    },
    ApplyAction {
        operation_id: String,
        action: MailAction,
    },
}

pub struct MailService {
    store: MailStore,
    database_path: PathBuf,
    config_path: PathBuf,
    configs: Mutex<Vec<SavedConfig>>,
    token_cache: Mutex<HashMap<String, CachedTokens>>,
    credentials: std::sync::Arc<dyn CredentialStore>,
    warmed_google_photo_accounts: Mutex<HashMap<String,String>>,
}
// Native shells use independent services for cached reads and network work.
// Coalesce overlapping syncs across those handles, rather than letting Linux's
// worker-per-action model race duplicate flushes and consume Gmail's budget.
struct SyncLease {
    key: (PathBuf, String),
}
fn active_syncs() -> &'static Mutex<HashSet<(PathBuf, String)>> {
    static ACTIVE: std::sync::OnceLock<Mutex<HashSet<(PathBuf, String)>>> =
        std::sync::OnceLock::new();
    ACTIVE.get_or_init(Default::default)
}
impl SyncLease {
    fn acquire(path: &Path, account: &str) -> Result<Option<Self>, String> {
        let path = path
            .parent()
            .and_then(|p| p.canonicalize().ok())
            .map(|p| p.join(path.file_name().unwrap_or_default()))
            .unwrap_or_else(|| path.to_path_buf());
        let key = (path, account.to_owned());
        if !active_syncs()
            .lock()
            .map_err(|_| "sync registry unavailable")?
            .insert(key.clone())
        {
            return Ok(None);
        }
        Ok(Some(Self { key }))
    }
}
impl Drop for SyncLease {
    fn drop(&mut self) {
        if let Ok(mut active) = active_syncs().lock() {
            active.remove(&self.key);
        }
    }
}
// Bound each outgoing pass so an old cleanup cannot monopolize a sync worker.
// The time bound is checked between requests; an individual provider timeout is not preempted.
struct ActionBudget {
    remaining: usize,
    max_elapsed: Duration,
}
impl ActionBudget {
    fn interactive() -> Self {
        Self {
            remaining: 40,
            max_elapsed: Duration::from_secs(2),
        }
    }
    fn start_request(&mut self, elapsed: Duration) -> bool {
        if self.remaining == 0 || elapsed >= self.max_elapsed {
            return false;
        }
        self.remaining -= 1;
        true
    }
}
#[derive(Default)]
struct ActionFlushReport {
    applied: usize,
    pending: usize,
    warnings: Vec<String>,
    retry_after_ms: u64,
}
fn is_bulk_read(entry: &mail_core::QueuedAction) -> bool {
    (entry.operation_id.starts_with("bulk:") || entry.operation_id.starts_with("triage:"))
        && matches!(entry.action, MailAction::MarkRead { read: true, .. })
}
/// The OS credential store. QA builds against the synthetic Gmail fixture can
/// use private files instead, so they never touch a real keychain.
fn credential_store() -> std::sync::Arc<dyn CredentialStore> {
    #[cfg(feature = "test-fixture")]
    match fixture_credentials::FileCredentials::from_environment() {
        Some(Ok(files)) => return std::sync::Arc::new(files),
        Some(Err(error)) => return std::sync::Arc::new(fixture_credentials::UnusableCredentials(error)),
        None => {}
    }
    std::sync::Arc::new(OsCredentials)
}
/// Feature switches for this build (see `mail_cloud::release`).
fn build_features() -> Value {
    json!({
        "channel": release::RELEASE_CHANNEL,
        "distribution": release::DISTRIBUTION,
        "google_contact_photos": release::GOOGLE_CONTACT_PHOTOS,
        "microsoft_accounts": release::MICROSOFT_ACCOUNTS,
        "google_oauth_public": release::GOOGLE_OAUTH_PUBLIC,
    })
}
/// Google app passwords are 16 letters, shown in groups of four. Checking the
/// shape first keeps a regular Google password from being sent to IMAP at all.
/// Google sign-in failures, in words that point to the way forward.
fn google_sign_in_failure(error: mail_cloud::CloudError) -> String {
    use mail_cloud::CloudError;
    match error {
        CloudError::AuthDenied => "Google didn't finish the sign-in. If Google said “Access blocked” or that Email hasn't \
            completed verification, Google sign-in isn't open to this account yet. Use Gmail (app password) instead."
            .into(),
        CloudError::AuthTimeout => "Browser sign-in timed out. If Google showed “Access blocked”, Google sign-in isn't \
            open to this account yet. Use Gmail (app password) instead."
            .into(),
        CloudError::Browser => "Email couldn't open your browser for Google sign-in. Use Gmail (app password) instead, \
            or set a default browser and try again."
            .into(),
        other => other.to_string(),
    }
}
/// Whether an IMAP host is Gmail's (`imap.gmail.com`, or the older
/// `imap.googlemail.com`).
fn is_gmail_imap_host(host: &str) -> bool {
    let host = host.trim().trim_end_matches('.');
    host.eq_ignore_ascii_case(GMAIL_IMAP_HOST) || host.eq_ignore_ascii_case("imap.googlemail.com")
}
fn action_retry_after_ms(error: &str) -> u64 {
    error
        .split("retrying after ")
        .nth(1)
        .and_then(|s| s.split_whitespace().next())
        .and_then(|s| s.parse::<u64>().ok())
        .map(|seconds| seconds.clamp(1, 3600) * 1000)
        .unwrap_or(60_000)
}
fn sync_account_lanes(
    accounts: &[Account],
    run: impl Fn(&str) -> Result<Value, String> + Sync,
) -> Vec<Value> {
    let next = AtomicUsize::new(0);
    let results = Mutex::new(vec![Value::Null; accounts.len()]);
    std::thread::scope(|scope| {
        for _ in 0..accounts.len().min(4) {
            let run = &run;
            let next = &next;
            let results = &results;
            scope.spawn(move || loop {
                let i=next.fetch_add(1,Ordering::Relaxed);
                let Some(account)=accounts.get(i) else {break;};
                let value=match run(&account.id) {
                    Ok(data)=>json!({"account_id":account.id,"ok":data.get("sync_error").is_none(),"data":data}),
                    Err(error)=>json!({"account_id":account.id,"ok":false,"error":error}),
                };
                results.lock().expect("sync results")[i]=value;
            });
        }
    });
    results.into_inner().expect("sync results")
}
impl MailService {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref();
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                fs::DirBuilder::new()
                    .recursive(true)
                    .mode(0o700)
                    .create(parent)
                    .map_err(|e| e.to_string())?;
            }
            #[cfg(not(unix))]
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let config_path = path.with_extension("accounts.json");
        ensure_private_config(&config_path)?;
        let configs = if config_path.exists() {
            serde_json::from_slice(&fs::read(&config_path).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?
        } else {
            Vec::new()
        };
        let store = MailStore::open(path).map_err(|e| e.to_string())?;
        store_location::note_opened_store(path);
        Ok(Self {
            store,
            database_path: path.to_path_buf(),
            config_path,
            configs: Mutex::new(configs),
            token_cache: Mutex::new(HashMap::new()),
            credentials: credential_store(),
            warmed_google_photo_accounts: Mutex::new(HashMap::new()),
        })
    }
    pub fn request_json(&self, raw: &str) -> Result<Value, String> {
        let request: Request = serde_json::from_str(raw).map_err(|e| e.to_string())?;
        self.respond(request)
    }
    /// The response envelope both shells read: `{"ok":true,"data":…}` or
    /// `{"ok":false,"error":"…"}`. A failed `add_imap_account` also carries
    /// `setup_error` (`code`, `message`, `action`, `detail`, `servers`; see
    /// docs/service-api.md), so account setup can show a one-line fix and keep
    /// the server's words behind a Details disclosure.
    pub fn request_envelope(&self, raw: &str) -> Value {
        let result = match serde_json::from_str::<Request>(raw) {
            Ok(Request::AddImapAccount(request)) => self.add_imap(request).map_err(|problem| {
                json!({"ok": false, "error": problem.message.clone(), "setup_error": problem})
            }),
            Ok(request) => self
                .respond(request)
                .map_err(|error| json!({"ok": false, "error": error})),
            Err(error) => Err(json!({"ok": false, "error": error.to_string()})),
        };
        result.map_or_else(|envelope| envelope, |data| json!({"ok": true, "data": data}))
    }
    fn value<T: Serialize>(&self, result: mail_core::Result<T>) -> Result<Value, String> {
        serde_json::to_value(result.map_err(|e| e.to_string())?).map_err(|e| e.to_string())
    }
    fn refresh_calendar_invitation(&self, message_id: &str) -> Result<Value, String> {
        let message = self
            .store
            .get_message(message_id)
            .map_err(|e| e.to_string())?
            .ok_or("message not found")?;
        let account = self.account(&message.account_id)?;
        let mut events = Vec::new();
        if let Some(raw) = self.demo_raw(&message)? {
            events.extend(mail_core::parse_calendar_invitations(&raw).map_err(|e| e.to_string())?);
        } else if matches!(account.provider, Provider::Imap | Provider::Icloud) {
            let saved = self.saved_config(&account.id)?;
            let raw = self
                .imap_connector(&saved)?
                .fetch_raw_message(&account, message_id)
                .map_err(|e| e.to_string())?;
            events.extend(mail_core::parse_calendar_invitations(&raw).map_err(|e| e.to_string())?);
        } else {
            let files = self.respond(Request::ListAttachments {
                message_id: message_id.into(),
            })?;
            let files = files.as_array().ok_or("invalid attachment response")?;
            let candidates = files
                .iter()
                .filter(|file| {
                    let mime = file["mime_type"]
                        .as_str()
                        .unwrap_or("")
                        .split(';')
                        .next()
                        .unwrap_or("")
                        .trim();
                    mime.eq_ignore_ascii_case("text/calendar")
                        || file["filename"]
                            .as_str()
                            .unwrap_or("")
                            .to_ascii_lowercase()
                            .ends_with(".ics")
                })
                .collect::<Vec<_>>();
            if candidates.len() > 16 {
                return Err("Too many calendar attachments in this message".into());
            }
            for file in candidates {
                if file["size"].as_u64().unwrap_or(0) > 1024 * 1024 {
                    return Err("Calendar invitation exceeds 1 MiB".into());
                }
                let part_id = file["part_id"].as_str().ok_or("calendar part missing")?;
                let attachment = self.respond(Request::FetchAttachment {
                    message_id: message_id.into(),
                    part_id: part_id.into(),
                })?;
                let bytes = STANDARD
                    .decode(
                        attachment["data_base64"]
                            .as_str()
                            .ok_or("missing invitation bytes")?,
                    )
                    .map_err(|_| "invalid invitation data")?;
                let parsed =
                    mail_core::parse_calendar_invitations(&bytes).map_err(|e| e.to_string())?;
                if parsed.is_empty() {
                    return Err(
                        "Calendar attachment has no supported event; saved invitations were kept."
                            .into(),
                    );
                }
                events.extend(parsed);
            }
        }
        self.value(self.store.cache_calendar_events(message_id, &events))
    }
    fn inline_images(
        &self,
        message: &mail_core::Message,
        safe_html: &str,
    ) -> Result<String, String> {
        if !safe_html.to_ascii_lowercase().contains("cid:") {
            return Ok(safe_html.into());
        }
        if let Some(raw) = self.demo_raw(message)? {
            return raw_inline_images(safe_html, &raw);
        }
        let account = self.account(&message.account_id)?;
        let saved = match self.saved_config(&account.id) {
            Ok(saved) => saved,
            Err(_) => return Ok(safe_html.into()),
        };
        let result = match account.provider {
            Provider::Gmail | Provider::Microsoft => {
                let client = self.cloud_client(&saved)?;
                let parts = client
                    .attachments(&account, &message.id)
                    .map_err(|e| e.to_string())?
                    .into_iter()
                    .filter_map(|item| {
                        item.content_id.map(|content_id| InlinePart {
                            part_id: item.part_id,
                            content_id,
                            mime_type: item.mime_type,
                            size: item.size,
                        })
                    })
                    .collect::<Vec<_>>();
                let html = replace_inline_images(safe_html, &parts, |id| {
                    client
                        .attachment_data(&account, &message.id, id)
                        .map_err(|e| e.to_string())
                });
                let _ = self.save_cloud_tokens(&saved, &client);
                html
            }
            Provider::Icloud | Provider::Imap => {
                let raw = self
                    .imap_connector(&saved)?
                    .fetch_raw_message(&account, &message.id)
                    .map_err(|e| e.to_string())?;
                raw_inline_images(safe_html, &raw)?
            }
        };
        Ok(result)
    }
    /// Seeded demo messages keep their MIME source locally, standing in for a
    /// provider. Mail in real accounts never reads the demo table.
    fn demo_raw(&self, message: &mail_core::Message) -> Result<Option<Vec<u8>>, String> {
        if !is_demo_account(&message.account_id) {
            return Ok(None);
        }
        self.store
            .demo_raw_message(&message.id)
            .map_err(|e| e.to_string())
    }
    fn load_configs(&self) -> Result<Vec<SavedConfig>, String> {
        ensure_private_config(&self.config_path)?;
        if self.config_path.exists() {
            serde_json::from_slice(&fs::read(&self.config_path).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())
        } else {
            Ok(Vec::new())
        }
    }
    fn saved_config(&self, id: &str) -> Result<SavedConfig, String> {
        let _lifecycle = self.lifecycle_lock()?;
        let latest = self.load_configs()?;
        *self
            .configs
            .lock()
            .map_err(|_| "config lock poisoned".to_string())? = latest.clone();
        latest
            .into_iter()
            .find(|c| c.account_id == id)
            .ok_or_else(|| format!("account config missing: {id}"))
    }
    fn save_configs(&self, configs: &[SavedConfig]) -> Result<(), String> {
        ensure_private_config(&self.config_path)?;
        let temp = self
            .config_path
            .with_extension(format!("accounts.json.tmp.{}", Uuid::new_v4()));
        let bytes = serde_json::to_vec_pretty(configs).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::{
                io::Write,
                os::unix::fs::{OpenOptionsExt, PermissionsExt},
            };
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&temp)
                .map_err(|e| e.to_string())?;
            file.set_permissions(fs::Permissions::from_mode(0o600))
                .map_err(|e| e.to_string())?;
            file.write_all(&bytes).map_err(|e| e.to_string())?;
            file.sync_all().map_err(|e| e.to_string())?;
        }
        #[cfg(not(unix))]
        fs::write(&temp, bytes).map_err(|e| e.to_string())?;
        fs::rename(&temp, &self.config_path).map_err(|e| {
            let _ = fs::remove_file(&temp);
            e.to_string()
        })
    }
    #[cfg(not(target_os = "macos"))]
    fn entry(id: &str) -> Result<keyring::Entry, String> {
        keyring::Entry::new(KEYRING_SERVICE, id).map_err(|e| e.to_string())
    }
    fn secret(&self, id: &str) -> Result<String, String> {
        // Callers hold the lifecycle lock and have checked the captured account
        // generation before any legacy adoption can recreate a current item.
        self.credentials.read_mail(id)?.ok_or_else(|| "Account credential unavailable".into())
    }
    fn account(&self, id: &str) -> Result<Account, String> {
        self.store
            .list_accounts()
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|a| a.id == id)
            .ok_or_else(|| format!("unknown account: {id}"))
    }
}
fn default_recipient_limit() -> usize {
    6
}

impl MailService {
    fn respond(&self, request: Request) -> Result<Value, String> {
        match request {
            Request::AgentAccessStatus => agent_access::status(&self.store),
            Request::AgentAccessOverview { history_limit } => {
                agent_access::overview(&self.store, history_limit.unwrap_or(60))
            }
            Request::AgentAccessDisclosure { account_id } => {
                let account = self.account(&account_id)?;
                Ok(agent_access::disclosure(&account.email))
            }
            Request::GrantAgentAccess {
                account_id,
                grants,
                disclosure_version,
            } => self.value(self.store.grant_agent_access(
                &account_id,
                &grants,
                disclosure_version,
            )),
            Request::RevokeAgentAccess { account_id } => self
                .store
                .revoke_agent_access(&account_id)
                .map(|removed| json!({"account_id":account_id,"revoked":removed}))
                .map_err(|e| e.to_string()),
            Request::ListAgentActions { status, limit } => self.value(
                self.store
                    .list_agent_actions(status.as_deref(), limit.unwrap_or(200)),
            ),
            Request::AgentPendingSummary => self.value(self.store.agent_pending_summary()),
            Request::ApproveAgentAction { id, undo_seconds } => {
                self.value(self.store.approve_agent_action(
                    &id,
                    undo_seconds.unwrap_or(mail_core::DEFAULT_APPROVAL_UNDO_SECONDS),
                ))
            }
            Request::RejectAgentAction { id } => self.value(self.store.reject_agent_action(&id)),
            Request::AgentRequestAttachment { id, attachment_id } => {
                let (filename, mime_type, bytes) = self
                    .store
                    .agent_request_attachment(&id, &attachment_id)
                    .map_err(|e| e.to_string())?;
                Ok(json!({"filename":filename,"mime_type":mime_type,"data_base64":STANDARD.encode(bytes)}))
            }
            Request::TrackingGet { message_id } => self.tracking_get(&message_id),
            Request::TrackingCapability => self.tracking_capability(),
            Request::TrackingSet { message_id, enabled } => self.tracking_set(&message_id, enabled),
            Request::TrackingNotifications { enabled, session_since } => {
                if enabled {
                    self.tracking_activity(None, true)?;
                }
                self.store
                    .consume_tracking_notifications(enabled, session_since, timestamp_ms())
                    .map(|items| json!({"items":items}))
                    .map_err(|e| e.to_string())
            }
            Request::TrackingActivity { message_id, refresh } => self.tracking_activity(message_id.as_deref(), refresh),
            Request::BookingStatus => self.booking_status(),
            Request::BookingSetupStatus => self.booking_setup_status(),
            Request::BookingSetupRead => self.booking_setup_read(),
            Request::BookingSetupStart { name } => self.booking_setup_start(name),
            Request::BookingSetupPoll => self.booking_setup_poll(),
            Request::BookingSetupPreview { patch } => self.booking_setup_preview(patch),
            Request::BookingSetupApply { plan_id, confirm } => self.booking_setup_apply(&plan_id,confirm),
            Request::BookingConnectStart { name } => self.booking_connect_start(name),
            Request::BookingConnectPoll => self.booking_connect_poll(),
            Request::BookingDisconnect => self.booking_disconnect(),
            Request::BookingList { refresh } => self.booking_list(refresh),
            Request::BookingLink { meeting_type_id } => self.booking_link(&meeting_type_id),
            Request::BookingAvailability { meeting_type_id } => self.booking_availability(&meeting_type_id),
            Request::BookingAnalytics { refresh } => self.booking_analytics(refresh),
            Request::CalendarStatus { account_id } => self.calendar_status(&account_id),
            Request::ConnectCalendar { account_id } => self.connect_calendar(&account_id),
            Request::ListCalendars { account_id } => {
                self.value(self.store.list_calendar_collections(account_id.as_deref()))
            }
            Request::ListAgenda {
                account_id,
                start_ms,
                end_ms,
                limit,
                deduplicate_shared,
                hidden_sources,
            } => {
                calendar_view::validate_sources(&hidden_sources)?;
                let requested = limit.unwrap_or(500).min(10_000);
                let events = self.store.list_agenda_events(
                    account_id.as_deref(), start_ms, end_ms,
                    if deduplicate_shared || !hidden_sources.is_empty() { 10_000 } else { requested },
                ).map_err(|e| e.to_string())?;
                Ok(json!(calendar_view::visible_events(events, &hidden_sources, deduplicate_shared, requested)))
            },
            Request::SyncCalendar {
                account_id,
                start_ms,
                end_ms,
            } => self.sync_calendar(&account_id, start_ms, end_ms),
            Request::RespondCalendarInvitation {
                account_id,
                calendar_id,
                event_id,
                response,
            } => self.respond_calendar(&account_id, &calendar_id, &event_id, &response),
            Request::CreateCalendarEvent {
                account_id,
                calendar_id,
                title,
                start_ms,
                end_ms,
                attendee_emails,
                description,
                location,
            } => self.create_calendar_event(
                &account_id,
                &calendar_id,
                &title,
                start_ms,
                end_ms,
                &attendee_emails,
                description.as_deref(),
                location.as_deref(),
            ),
            Request::UpdateCalendarEvent {
                account_id,
                calendar_id,
                event_id,
                expected_etag,
                edit_scope,
                title,
                start_ms,
                end_ms,
                description,
                location,
            } => self.update_calendar_event(
                &account_id,
                &calendar_id,
                &event_id,
                &expected_etag,
                edit_scope.as_deref().unwrap_or("single"),
                mail_cloud::CalendarEventChanges {
                    title,
                    start_ms,
                    end_ms,
                    description,
                    location,
                },
            ),
            Request::Availability {
                account_id,
                start_ms,
                end_ms,
                duration_minutes,
            } => self.calendar_availability(
                account_id.as_deref(),
                start_ms,
                end_ms,
                duration_minutes,
            ),
            Request::ListCalendarInvitations { account_id, limit } => self.value(
                self.store
                    .list_calendar_invitations(account_id.as_deref(), limit.unwrap_or(100)),
            ),
            Request::CalendarInvitation { message_id } => {
                self.value(self.store.calendar_invitation(&message_id))
            }
            Request::RefreshCalendarInvitation { message_id } => {
                self.refresh_calendar_invitation(&message_id)
            }
            Request::SetFollowup {thread_id,due_at_ms,only_if_no_reply,sent_message_id} => {
                self.value(match sent_message_id {
                    Some(anchor)=>self.store.set_followup_for_sent_message(&thread_id,due_at_ms,only_if_no_reply,&anchor),
                    None=>self.store.set_followup(&thread_id,due_at_ms,only_if_no_reply),
                })
            },
            Request::ListFollowupThreads { account_id } => {
                self.value(self.store.list_followup_threads(account_id.as_deref()))
            }
            Request::ListFollowups { account_id } => {
                self.value(self.store.list_followups(account_id.as_deref()))
            }
            Request::CancelFollowup { thread_id } => {
                self.value(self.store.cancel_followup(&thread_id))
            }
            Request::GetSignature { account_id, email } => {
                self.value(self.store.get_signature(&account_id, &email))
            }
            Request::UpsertSignature {
                account_id,
                email,
                body,
                for_new,
                for_reply,
            } => self.value(self.store.upsert_signature(
                &account_id,
                &email,
                &body,
                for_new,
                for_reply,
            )),
            Request::DeleteSignature { account_id, email } => {
                self.value(self.store.delete_signature(&account_id, &email))
            }
            Request::SuggestRecipients {
                account_id,
                query,
                limit,
            } => self.value(self.store.suggest_recipients(&account_id, &query, limit)),
            Request::BuildFeatures => Ok(build_features()),
            Request::SetupProvider { email } => {
                Ok(json!({"provider": mail_imap::provider_for_address(&email)}))
            }
            Request::SeedDemo {
                set,
                utc_offset_minutes,
            } => {
                if is_default_store(&self.database_path) {
                    return Err("Demo messages are never added to the default mail store or its directory. Use a separate directory.".into());
                }
                match set.as_deref().unwrap_or("small") {
                    "small" => {
                        self.store.seed_demo().map_err(|e| e.to_string())?;
                        Ok(Value::Null)
                    }
                    "launch" => self.value(
                        self.store
                            .seed_launch_demo(timestamp_ms(), utc_offset_minutes.unwrap_or(0)),
                    ),
                    other => Err(format!("Unknown demo set: {other}. Use small or launch.")),
                }
            }
            Request::CreateSenderCleanup { account_id, sender } => {
                self.create_sender_cleanup(&account_id, &sender)
            }
            Request::ListSenderCleanups { account_id } => {
                self.value(self.store.list_sender_cleanups(account_id.as_deref()))
            }
            Request::GetSenderCleanup { job_id } => self.value(self.store.sender_cleanup(&job_id)),
            Request::StepSenderCleanup { job_id } => self.step_sender_cleanup(&job_id),
            Request::ConfirmSenderCleanup { job_id } => {
                self.value(self.store.confirm_sender_cleanup(&job_id))
            }
            Request::PauseSenderCleanup { job_id } => {
                self.value(self.store.control_sender_cleanup(&job_id, "pause"))
            }
            Request::ResumeSenderCleanup { job_id } => {
                self.value(self.store.control_sender_cleanup(&job_id, "resume"))
            }
            Request::CancelSenderCleanup { job_id } => {
                self.value(self.store.control_sender_cleanup(&job_id, "cancel"))
            }
            Request::UnreadCounts => self.value(self.store.unread_counts()),
            Request::UnreadFromSender { account_id, sender } => {
                self.value(self.store.unread_from_sender(&account_id, &sender))
            }
            Request::BulkMarkRead {
                operation_id,
                account_id,
                thread_ids,
            } => self.value(
                self.store
                    .bulk_mark_read(&operation_id, &account_id, &thread_ids),
            ),
            Request::BatchTriage {
                operation_id,
                kind,
                thread_ids,
            } => self.value(self.store.batch_triage(&operation_id, &kind, &thread_ids)),
            Request::LatestTriageUndo => self.value(self.store.latest_triage_undo()),
            Request::UndoTriage { operation_id } => {
                self.value(self.store.undo_triage(&operation_id))
            }
            Request::ListAccounts => self.value(self.store.list_accounts()),
            Request::ListSendAs { account_id } => {
                let account = self.account(&account_id)?;
                if account.provider == Provider::Gmail {
                    let saved = self.saved_config(&account_id)?;
                    let client = self.cloud_client(&saved)?;
                    let aliases = client.send_as_aliases().map_err(|e| e.to_string())?;
                    let _ = self.save_cloud_tokens(&saved, &client);
                    serde_json::to_value(aliases).map_err(|e| e.to_string())
                } else {
                    Ok(
                        json!([{"email":account.email,"display_name":account.display_name,"is_default":true}]),
                    )
                }
            }
            Request::ListThreads {
                account_id,
                split_id,
                unread_only,
                limit,
                offset,
            } => self.value(match split_id {
                Some(id) => self.store.list_split_threads(
                    &id,
                    account_id.as_deref(),
                    unread_only.unwrap_or(false),
                    limit.unwrap_or(100),
                    offset.unwrap_or(0),
                ),
                None => self.store.list_threads_filtered(
                    account_id.as_deref(),
                    unread_only.unwrap_or(false),
                    limit.unwrap_or(100),
                    offset.unwrap_or(0),
                ),
            }),
            Request::ListLabels => self.value(self.store.list_labels()),
            Request::ListLabelThreads {
                label,
                query,
                limit,
                offset,
            } => self.value(self.store.list_label_threads(
                &label,
                query.as_deref(),
                limit.unwrap_or(100),
                offset.unwrap_or(0),
            )),
            Request::ListArchived { limit, offset } => self.value(
                self.store
                    .list_archived_threads(limit.unwrap_or(100), offset.unwrap_or(0)),
            ),
            Request::SyncArchives { restart } => {
                let accounts = self.store.list_accounts().map_err(|e| e.to_string())?;
                let rows = accounts
                    .into_iter()
                    .filter(|a| a.provider == Provider::Gmail && !is_demo_account(&a.id))
                    .map(|account| {
                        let result = (|| -> Result<usize, String> {
                            if restart.unwrap_or(false) {
                                self.store
                                    .restart_archive_pages(&account.id)
                                    .map_err(|e| e.to_string())?;
                            }
                            let (cursor, complete) = self
                                .store
                                .archive_page_cursor(&account.id)
                                .map_err(|e| e.to_string())?;
                            if complete {
                                return Ok(0);
                            }
                            let saved = self.saved_config(&account.id)?;
                            let client = self.cloud_client(&saved)?;
                            let result = client.gmail_archive_page(&account, cursor.as_deref());
                            let _ = self.save_cloud_tokens(&saved, &client);
                            let (batch, next) = result.map_err(|e| e.to_string())?;
                            let count = batch.messages.len();
                            self.store
                                .cache_archive_page(&account.id, &batch, next.as_deref())
                                .map_err(|e| e.to_string())?;
                            Ok(count)
                        })();
                        match result {
                            Ok(count) => {
                                json!({"account_id":account.id,"data":{"messages_added":count}})
                            }
                            Err(error) => json!({"account_id":account.id,"error":error}),
                        }
                    })
                    .collect::<Vec<_>>();
                Ok(json!(rows))
            }
            Request::ListSnippets => self.value(self.store.list_snippets()),
            Request::UpsertSnippet { id, name, body } => {
                self.value(self.store.upsert_snippet(id.as_deref(), &name, &body))
            }
            Request::DeleteSnippet { id } => self.value(self.store.delete_snippet(&id)),
            Request::ListSplits => self.value(self.store.list_splits()),
            Request::PreviewSplit { query } => self.value(self.store.preview_split(&query, 500)),
            Request::UpsertSplit { id, name, query } => {
                self.value(self.store.upsert_split(id.as_deref(), &name, &query))
            }
            Request::DeleteSplit { id } => {
                self.store.delete_split(&id).map_err(|e| e.to_string())?;
                Ok(json!({"id":id}))
            }
            Request::SnoozeThread {
                thread_id,
                wake_at_ms,
            } => {
                self.store
                    .snooze_thread(&thread_id, wake_at_ms)
                    .map_err(|e| e.to_string())?;
                Ok(json!({"thread_id":thread_id,"wake_at_ms":wake_at_ms}))
            }
            Request::UnsnoozeThread { thread_id } => {
                self.store
                    .unsnooze_thread(&thread_id)
                    .map_err(|e| e.to_string())?;
                Ok(json!({"thread_id":thread_id}))
            }
            Request::ListSnoozedThreads => self.value(self.store.list_snoozed_threads()),
            Request::ListPinnedThreads { limit, offset } => self.value(
                self.store
                    .list_pinned_threads(limit.unwrap_or(100), offset.unwrap_or(0)),
            ),
            Request::SetThreadPinned { thread_id, pinned } => {
                self.store
                    .set_thread_pinned(&thread_id, pinned)
                    .map_err(|e| e.to_string())?;
                Ok(json!({"thread_id":thread_id,"pinned":pinned}))
            }
            Request::ScheduleDraft {
                message_id,
                send_at_ms,
            } => self.value(
                self.store
                    .schedule_draft(&message_id, send_at_ms, "scheduled"),
            ),
            Request::QueueSendDraft {
                message_id,
                undo_seconds,
            } => {
                let delay = undo_seconds.unwrap_or(10).clamp(1, 60) as i64;
                self.value(self.store.schedule_draft(
                    &message_id,
                    timestamp_ms() + delay * 1000,
                    "undoable",
                ))
            }
            Request::CancelScheduledDraft { message_id } => {
                self.store
                    .cancel_scheduled_draft(&message_id)
                    .map_err(|e| e.to_string())?;
                Ok(json!({"message_id":message_id,"status":"draft"}))
            }
            Request::ListOutbox => self.value(self.store.list_outbox()),
            Request::DispatchDue => self.dispatch_due(),
            Request::AddDraftAttachment {
                message_id,
                filename,
                mime_type,
                content_base64,
            } => {
                if content_base64.len() > 28 * 1024 * 1024 {
                    return Err("attachment is too large".into());
                }
                let bytes = STANDARD
                    .decode(content_base64)
                    .map_err(|e| format!("invalid attachment encoding: {e}"))?;
                self.value(self.store.add_draft_attachment(
                    &message_id,
                    &filename,
                    &mime_type,
                    &bytes,
                ))
            }
            Request::ListDraftAttachments { message_id } => {
                self.value(self.store.list_draft_attachments(&message_id))
            }
            Request::RemoveDraftAttachment {
                message_id,
                attachment_id,
            } => {
                self.store
                    .remove_draft_attachment(&message_id, &attachment_id)
                    .map_err(|e| e.to_string())?;
                Ok(json!({"message_id":message_id,"attachment_id":attachment_id}))
            }
            Request::Search {
                query,
                limit,
                offset,
                is_inbox,
                account_id,
                unread_only,
                starred_only,
                label,
                folder,
                split_id,
                snoozed_only,
                followups_only,
            } => self.value(self.store.search_scoped(
                &query,
                limit.unwrap_or(100),
                offset.unwrap_or(0),
                &mail_core::SearchScope {
                    account_ids: account_id.map(|id| vec![id]),
                    is_inbox,
                    unread_only: unread_only.unwrap_or(false),
                    starred_only: starred_only.unwrap_or(false),
                    label,
                    folder,
                    split_id,
                    snoozed_only: snoozed_only.unwrap_or(false),
                    followups_only: followups_only.unwrap_or(false),
                },
            )),
            Request::GetLatestMessageForThread { thread_id } => {
                let mut value = self.value(self.store.get_latest_message_for_thread(&thread_id))?;
                // Shared banner text, so macOS and Linux notifications read the same.
                if let Some(message) = value.as_object_mut() {
                    let plain = message.get("body_plain").and_then(Value::as_str).unwrap_or("");
                    let html = message.get("body_html").and_then(Value::as_str).unwrap_or("");
                    let snippet = mail_core::notification_snippet(plain, html, 180);
                    message.insert("notification_snippet".into(), Value::String(snippet));
                }
                Ok(value)
            }
            Request::GetMessage { id } => self.value(self.store.get_message(&id)),
            Request::RefreshRecipientHeaders { thread_id } => {
                let messages = self
                    .store
                    .list_thread_messages(&thread_id)
                    .map_err(|e| e.to_string())?;
                for message in messages
                    .iter()
                    .filter(|m| m.recipient_headers.is_none() && !m.is_draft)
                {
                    let account = self.account(&message.account_id)?;
                    if account.provider != Provider::Gmail {
                        continue;
                    }
                    let saved = self.saved_config(&account.id)?;
                    let client = self.cloud_client(&saved)?;
                    let result = client
                        .gmail_recipient_headers(&account, &message.id)
                        .map_err(|e| e.to_string());
                    let _ = self.save_cloud_tokens(&saved, &client);
                    let (to, headers) = result?;
                    self.store
                        .update_recipient_headers(&message.id, &to, &headers)
                        .map_err(|e| e.to_string())?;
                }
                self.value(self.store.list_thread_messages(&thread_id))
            }
            Request::ListThreadMessages { thread_id } => {
                self.value(self.store.list_thread_messages(&thread_id))
            }
            Request::ListDrafts { account_id } => {
                self.value(self.store.list_drafts(account_id.as_deref()))
            }
            Request::RenderMessage {
                message_id,
                include_inline_images,
            } => {
                let message = self
                    .store
                    .get_message(&message_id)
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| format!("message not found: {message_id}"))?;
                let safe_html = message.body_html.as_ref().map(|html| {
                    // Many legitimate newsletters still ship HTTP image URLs.
                    // The reader permits only HTTPS images; upgrade those URLs
                    // after sanitizing so branding does not render as broken alt text.
                    tracking::strip_tracking_pixels(&upgrade_image_urls(&message_html_sanitizer().clean(&html).to_string()))
                });
                let safe_html = if include_inline_images == Some(true) {
                    match safe_html {
                        Some(html) => Some(self.inline_images(&message, &html).unwrap_or(html)),
                        None => None,
                    }
                } else {
                    safe_html
                };
                Ok(json!({"body_plain":message.body_plain,"body_html_safe":safe_html}))
            }
            Request::ListAttachments { message_id } => {
                let message = self
                    .store
                    .get_message(&message_id)
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| format!("message not found: {message_id}"))?;
                if let Some(raw) = self.demo_raw(&message)? {
                    return raw_attachment_list(&raw);
                }
                let account = self.account(&message.account_id)?;
                match account.provider {
                    Provider::Gmail | Provider::Microsoft => {
                        let saved = self.saved_config(&account.id)?;
                        let client = self.cloud_client(&saved)?;
                        let result = client
                            .attachments(&account, &message_id)
                            .map_err(|e| e.to_string());
                        let _ = self.save_cloud_tokens(&saved, &client);
                        serde_json::to_value(result?).map_err(|e| e.to_string())
                    }
                    Provider::Icloud | Provider::Imap => {
                        let saved = self.saved_config(&account.id)?;
                        let raw = self
                            .imap_connector(&saved)?
                            .fetch_raw_message(&account, &message_id)
                            .map_err(|e| e.to_string())?;
                        raw_attachment_list(&raw)
                    }
                }
            }
            Request::FetchAttachment {
                message_id,
                part_id,
            } => {
                let message = self
                    .store
                    .get_message(&message_id)
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| format!("message not found: {message_id}"))?;
                if let Some(raw) = self.demo_raw(&message)? {
                    let bytes = attachment_bytes(&raw, &part_id).map_err(|e| e.to_string())?;
                    return Ok(json!({"data_base64": STANDARD.encode(bytes)}));
                }
                let account = self.account(&message.account_id)?;
                let saved = self.saved_config(&account.id)?;
                let bytes = match account.provider {
                    Provider::Gmail | Provider::Microsoft => {
                        let client = self.cloud_client(&saved)?;
                        let result = client
                            .attachment_data(&account, &message_id, &part_id)
                            .map_err(|e| e.to_string());
                        let _ = self.save_cloud_tokens(&saved, &client);
                        result?
                    }
                    Provider::Icloud | Provider::Imap => {
                        let raw = self
                            .imap_connector(&saved)?
                            .fetch_raw_message(&account, &message_id)
                            .map_err(|e| e.to_string())?;
                        attachment_bytes(&raw, &part_id).map_err(|e| e.to_string())?
                    }
                };
                if bytes.len() > 20 * 1024 * 1024 {
                    return Err("attachment exceeds the 20 MiB download limit".into());
                }
                Ok(json!({"data_base64": STANDARD.encode(bytes)}))
            }
            Request::PendingActions => self.value(self.store.pending_actions()),
            Request::AddImapAccount(request) => self.add_imap(request).map_err(|problem| problem.message),
            Request::StartOauth {
                provider,
                client_id,
                client_secret,
            } => self.start_oauth(provider, client_id, client_secret),
            Request::EnableGooglePhotos { account_id } => self.enable_google_photos(&account_id),
            Request::GooglePhotoState { account_id } => {
                let saved = self.saved_config(&account_id)?;
                Ok(json!({"enabled": release::GOOGLE_CONTACT_PHOTOS
                    && saved.provider == Provider::Gmail
                    && saved.google_photos_enabled}))
            }
            Request::GoogleContactPhoto {
                account_id,
                address,
            } => {
                let saved = self.saved_config(&account_id)?;
                if !release::GOOGLE_CONTACT_PHOTOS
                    || saved.provider != Provider::Gmail
                    || !saved.google_photos_enabled
                {
                    return Ok(Value::Null);
                }
                let client = self.cloud_client(&saved)?;
                let result = (|| {
                    let warmed=self.warmed_google_photo_accounts.lock().map_err(|_|"Google photo cache lock poisoned")?
                        .get(&account_id).is_some_and(|generation|generation==&saved.generation);
                    if !warmed {
                        client.warm_google_contact_search().map_err(|e|e.to_string())?;
                        let _lifecycle=self.lifecycle_lock()?;self.verify_generation(&saved)?;
                        self.warmed_google_photo_accounts.lock().map_err(|_|"Google photo cache lock poisoned")?
                            .insert(account_id.clone(),saved.generation.clone());
                    }
                    client
                        .google_contact_photo(&address)
                        .map_err(|e| e.to_string())
                })();
                let _ = self.save_cloud_tokens(&saved, &client);
                Ok(match result? {
                    Some(bytes) => json!({"data_base64": STANDARD.encode(bytes)}),
                    None => Value::Null,
                })
            }
            Request::ReloadAccountCredentials { account_id } => {
                self.saved_config(&account_id)?;
                self.token_cache
                    .lock()
                    .map_err(|_| "token cache lock poisoned".to_string())?
                    .remove(&account_id);
                Ok(Value::Null)
            }
            Request::RemoveAccount { account_id } => {
                self.remove_account(&account_id)?;
                Ok(Value::Null)
            }
            Request::SyncAccount { account_id } => self.sync_account(&account_id),
            Request::SyncAll => {
                let accounts = self.store.list_accounts().map_err(|e| e.to_string())?;
                Ok(Value::Array(sync_account_lanes(&accounts, |id| {
                    self.sync_account(id)
                })))
            }
            Request::ComposeDraft {
                account_id,
                reply_thread_id,
                from,
                to,
                cc,
                bcc,
                subject,
                body_plain,
            } => {
                let draft = self.store.compose_draft_with_recipients(
                &account_id,
                from.as_deref(),
                &to,
                cc.as_deref(),
                bcc.as_deref(),
                &subject,
                &body_plain,
                ).map_err(|e| e.to_string())?;
                if let Some(thread_id) = reply_thread_id {
                    self.value(self.store.attach_draft_to_thread(&draft.id, &thread_id))
                } else { self.value(Ok::<_, mail_core::MailError>(draft)) }
            },
            Request::UpdateDraft {
                message_id,
                from,
                to,
                cc,
                bcc,
                subject,
                body_plain,
            } => self.value(self.store.update_draft_with_recipients(
                &message_id,
                from.as_deref(),
                &to,
                cc.as_deref(),
                bcc.as_deref(),
                &subject,
                &body_plain,
            )),
            Request::SendDraft { message_id } => self.send_draft(&message_id),
            Request::DiscardDraft { message_id } => {
                self.value(self.store.discard_draft(&message_id))
            }
            Request::RestoreDiscardedDraft { message_id } => {
                self.value(self.store.restore_discarded_draft(&message_id))
            }
            Request::ApplyAction {
                operation_id,
                action,
            } => self.apply_action(&operation_id, action),
        }
    }
    fn imap_connector(&self, c: &SavedConfig) -> Result<ImapConnector, String> {
        let config = ImapConfig {
            account_id: c.account_id.clone(),
            imap_host: c.imap_host.clone().ok_or("IMAP host missing")?,
            imap_port: c.imap_port.ok_or("IMAP port missing")?,
            imap_security: c.imap_security.ok_or("IMAP security missing")?,
            smtp_host: c.smtp_host.clone().ok_or("SMTP host missing")?,
            smtp_port: c.smtp_port.ok_or("SMTP port missing")?,
            smtp_security: c.smtp_security.ok_or("SMTP security missing")?,
            username: c.username.clone().ok_or("IMAP username missing")?,
            smtp_username: c.smtp_username.clone().ok_or("SMTP username missing")?,
            secret: {let _lifecycle=self.lifecycle_lock()?;self.verify_generation(c)?;self.secret(&c.account_id)?},
            archive_mailbox: c.archive_mailbox.clone(),
        };
        ImapConnector::new(config, c.provider.clone()).map_err(|e| e.to_string())
    }
    fn cloud_client(&self, c: &SavedConfig) -> Result<AccountCloudClient, String> {
        self.client_for_generation(c, false)
    }
    fn save_cloud_tokens(
        &self,
        c: &SavedConfig,
        client: &AccountCloudClient,
    ) -> Result<(), String> {
        if c.account_id != client.account_id || c.generation != client.generation {
            return Err("Account authorization changed".into());
        }
        self.commit_client_tokens(client, false)
    }
    fn persist_account(
        &self,
        account: &Account,
        mut saved: SavedConfig,
        secret: &str,
        tokens: Option<TokenSet>,
    ) -> Result<(), String> {
        let _lifecycle = self.lifecycle_lock()?;
        saved.generation = Uuid::new_v4().to_string();
        let generation = saved.generation.clone();
        let mut configs = self
            .configs
            .lock()
            .map_err(|_| "config lock poisoned".to_string())?;
        *configs = self.load_configs()?;
        if configs
            .iter()
            .any(|current| current.account_id == account.id)
        {
            return Err("Account already exists".into());
        }
        // One connection per mailbox, whichever way it was added (for example a
        // Google sign-in finished in the browser after an app-password setup).
        self.mailbox_not_connected_in(&account.email, &configs)?;
        self.credentials.write(&account.id, secret)?;
        let mut updated = configs.clone();
        updated.push(saved);
        if let Err(error) = self.save_configs(&updated) {
            let _ = self.credentials.delete(&account.id, false);
            return Err(error);
        }
        if let Err(error) = self.store.upsert_account(account) {
            let _ = self.save_configs(&configs);
            let _ = self.credentials.delete(&account.id, false);
            return Err(error.to_string());
        }
        *configs = updated;
        self.clear_removed_account_caches(&account.id)?;
        if let Some(tokens) = tokens {
            self.token_cache
                .lock()
                .map_err(|_| "token cache lock poisoned")?
                .insert(account.id.clone(), CachedTokens { revision: None, generation, tokens });
        }
        Ok(())
    }
    fn ensure_mailbox_not_connected(&self, email: &str) -> Result<(), String> {
        self.mailbox_not_connected_in(email, &self.load_configs()?)
    }
    fn mailbox_not_connected_in(&self, email: &str, configs: &[SavedConfig]) -> Result<(), String> {
        let mailbox = mail_imap::mailbox_identity(email);
        let connected = self.store.list_accounts().map_err(|e| e.to_string())?;
        if connected.iter().any(|existing| {
            mail_imap::mailbox_identity(&existing.email) == mailbox
                && configs.iter().any(|saved| saved.account_id == existing.id)
        }) {
            return Err(format!(
                "{} is already connected in Email. Remove it first to connect it another way.",
                email.trim()
            ));
        }
        Ok(())
    }
    fn add_imap(&self, request: AddImapAccount) -> Result<Value, SetupProblem> {
        let AddImapAccount {
            provider,
            email,
            display_name,
            username,
            smtp_username,
            mut imap_host,
            mut imap_port,
            mut imap_security,
            mut smtp_host,
            mut smtp_port,
            mut smtp_security,
            archive_mailbox,
            password,
            preset,
        } = request;
        if !matches!(provider, Provider::Icloud | Provider::Imap) {
            return Err("provider must be icloud or imap".into());
        }
        if email.trim().is_empty() || !email.contains('@') {
            return Err("valid email required".into());
        }
        let gmail_preset = preset == Some(ImapPreset::Gmail);
        if gmail_preset && provider != Provider::Imap {
            return Err("the Gmail app password preset uses the imap provider".into());
        }
        // Before any network call; persist_account checks again under its lock.
        self.ensure_mailbox_not_connected(&email)?;
        if gmail_preset && !mail_imap::looks_like_google_app_password(&password) {
            return Err(SetupProblem::google_password());
        }
        // No servers given: guess them from the address (no lookup), try them
        // once, and hand the guess back for editing if nothing answers.
        let guessed = if provider == Provider::Imap
            && !gmail_preset
            && imap_host.is_none()
            && smtp_host.is_none()
        {
            let guess = mail_imap::guess_servers(&email).ok_or("Enter your mail server names.")?;
            imap_host = Some(guess.imap_host.clone());
            imap_port = Some(guess.imap_port);
            imap_security = Some(guess.imap_security);
            smtp_host = Some(guess.smtp_host.clone());
            smtp_port = Some(guess.smtp_port);
            smtp_security = Some(guess.smtp_security);
            Some(guess)
        } else {
            None
        };
        let account = Account {
            id: Uuid::new_v4().to_string(),
            provider: provider.clone(),
            email: email.trim().into(),
            display_name,
        };
        let mut config = if provider == Provider::Icloud {
            ImapConfig::icloud(&account.id, &account.email, password)
        } else if gmail_preset {
            ImapConfig::gmail(&account.id, &account.email, password)
        } else {
            ImapConfig {
                account_id: account.id.clone(),
                imap_host: imap_host.clone().ok_or("IMAP host required")?,
                imap_port: imap_port.unwrap_or(993),
                imap_security: imap_security.unwrap_or(TransportSecurity::ImplicitTls),
                smtp_host: smtp_host.clone().ok_or("SMTP host required")?,
                smtp_port: smtp_port.unwrap_or(587),
                smtp_security: smtp_security.unwrap_or(TransportSecurity::StartTls),
                username: username.clone().unwrap_or_else(|| account.email.clone()),
                smtp_username: smtp_username
                    .clone()
                    .unwrap_or_else(|| account.email.clone()),
                secret: password,
                archive_mailbox: archive_mailbox.clone(),
            }
        };
        // The Gmail preset owns its servers and usernames: an app password made
        // for Google goes only to Google, whatever else the request carries.
        // Tests and QA builds redirect those servers to a synthetic fixture.
        if !gmail_preset {
            if let Some(v) = username {
                config.username = v;
            }
            if let Some(v) = smtp_username {
                config.smtp_username = v;
            }
            if let Some(v) = imap_host {
                config.imap_host = v;
            }
            if let Some(v) = imap_port {
                config.imap_port = v;
            }
            if let Some(v) = imap_security {
                config.imap_security = v;
            }
            if let Some(v) = smtp_host {
                config.smtp_host = v;
            }
            if let Some(v) = smtp_port {
                config.smtp_port = v;
            }
            if let Some(v) = smtp_security {
                config.smtp_security = v;
            }
            if let Some(v) = archive_mailbox {
                config.archive_mailbox = Some(v);
            }
        }
        let hint = if gmail_preset || is_gmail_imap_host(&config.imap_host) {
            ServerHint::Gmail
        } else if provider == Provider::Icloud {
            ServerHint::Icloud
        } else {
            ServerHint::Generic
        };
        let connector =
            ImapConnector::new(config.clone(), provider.clone()).map_err(|e| e.to_string())?;
        let report = connector.verify_account().map_err(|failure| {
            let problem = failure.explain(&config, hint);
            match &guessed {
                Some(guess) if failure.servers_may_be_wrong() => SetupProblem {
                    code: "servers_not_found",
                    message: format!(
                        "Couldn't find the mail servers for {}. Check the server settings.",
                        email.trim().rsplit('@').next().unwrap_or_default()
                    ),
                    action: None,
                    servers: Some(guess.clone()),
                    ..problem
                },
                _ => problem,
            }
        })?;
        if config.archive_mailbox.is_none() {
            // Gmail: archive moves mail to All Mail, which drops only the Inbox
            // label. If All Mail was not listed, archive looks it up later.
            config.archive_mailbox = report.gmail_all_mail;
        }
        let saved = SavedConfig {
            account_id: account.id.clone(),
            generation: String::new(),
            provider,
            username: Some(config.username),
            smtp_username: Some(config.smtp_username),
            imap_host: Some(config.imap_host),
            imap_port: Some(config.imap_port),
            imap_security: Some(config.imap_security),
            smtp_host: Some(config.smtp_host),
            smtp_port: Some(config.smtp_port),
            smtp_security: Some(config.smtp_security),
            archive_mailbox: config.archive_mailbox,
            client_id: None,
            client_secret: None,
            google_photos_enabled: false,
            google_calendar_enabled: false,
        };
        self.persist_account(&account,saved,&config.secret,None)?;
        Ok(json!(account))
    }
    fn start_oauth(
        &self,
        provider: Provider,
        client_id: String,
        client_secret: Option<String>,
    ) -> Result<Value, String> {
        let cloud_provider = match provider {
            Provider::Gmail if !release::GOOGLE_OAUTH_PUBLIC => {
                return Err("One-click Google sign-in isn't available in this build yet. Add Gmail with an app password.".into());
            }
            Provider::Gmail => CloudProvider::Gmail,
            Provider::Microsoft if !release::MICROSOFT_ACCOUNTS => {
                return Err("Outlook and Hotmail accounts aren't available in this release yet.".into());
            }
            Provider::Microsoft => CloudProvider::Microsoft,
            _ => return Err("provider must be gmail or microsoft".into()),
        };
        let config = OAuthConfig {
            provider: cloud_provider,
            client_id: client_id.clone(),
            client_secret: client_secret.clone(),
        };
        let tokens = authorize_interactive(&config).map_err(|error| {
            if provider == Provider::Gmail {
                google_sign_in_failure(error)
            } else {
                error.to_string()
            }
        })?;
        let client = CloudClient::new(config, tokens).map_err(|e| e.to_string())?;
        let (email, display_name) = client.identity().map_err(|e| e.to_string())?;
        let account = Account {
            id: Uuid::new_v4().to_string(),
            provider: provider.clone(),
            email,
            display_name,
        };
        let saved = SavedConfig {
            account_id: account.id.clone(),
            generation: String::new(),
            provider,
            username: None,
            smtp_username: None,
            imap_host: None,
            imap_port: None,
            imap_security: None,
            smtp_host: None,
            smtp_port: None,
            smtp_security: None,
            archive_mailbox: None,
            client_id: Some(client_id),
            client_secret,
            google_photos_enabled: false,
            google_calendar_enabled: false,
        };
        let tokens=client.tokens();
        self.persist_account(&account,saved,&serde_json::to_string(&tokens).map_err(|e|e.to_string())?,Some(tokens))?;
        Ok(json!(account))
    }
    fn enable_google_photos(&self, account_id: &str) -> Result<Value, String> {
        if !release::GOOGLE_CONTACT_PHOTOS {
            return Err("Google contact photos aren't available in this release.".into());
        }
        let account = self.account(account_id)?;
        let saved = self.saved_config(account_id)?;
        if saved.provider != Provider::Gmail {
            return Err("Google photos require a Gmail account".into());
        }
        if saved.google_photos_enabled {
            return Ok(json!({"enabled": true}));
        }
        let config = OAuthConfig {
            provider: CloudProvider::Gmail,
            client_id: saved.client_id.clone().ok_or("Google client ID missing")?,
            client_secret: saved.client_secret.clone(),
        };
        let tokens = authorize_google_photos(&config, &account.email).map_err(|e| e.to_string())?;
        if tokens.refresh_token.is_none() {
            return Err("Google did not issue an offline token; please retry photo access".into());
        }
        let client = CloudClient::new(config, tokens).map_err(|e| e.to_string())?;
        let (authorized_email, _) = client.identity().map_err(|e| e.to_string())?;
        if !authorized_email.eq_ignore_ascii_case(&account.email) {
            return Err(
                "The Google account selected in the browser does not match this mailbox".into(),
            );
        }
        self.commit_scoped_grant(&saved,false,client.tokens())?;
        Ok(json!({"enabled": true}))
    }
    fn clear_removed_account_caches(&self, id: &str) -> Result<(), String> {
        let mut tokens = self
            .token_cache
            .lock()
            .map_err(|_| "token cache lock poisoned")?;
        tokens.remove(id);
        tokens.remove(&Self::calendar_vault_id(id));
        drop(tokens);
        self.warmed_google_photo_accounts
            .lock()
            .map_err(|_| "Google photo cache lock poisoned")?
            .remove(id);
        Ok(())
    }
    fn remove_account(&self, id: &str) -> Result<(), String> {
        let saved = self.saved_config(id)?;
        let _lifecycle = self.lifecycle_lock()?;
        let mut configs = self
            .configs
            .lock()
            .map_err(|_| "config lock poisoned".to_string())?;
        *configs = self.verify_generation(&saved)?;
        let updated: Vec<_> = configs
            .iter()
            .filter(|c| c.account_id != id)
            .cloned()
            .collect();
        self.save_configs(&updated)?;
        if let Err(error) = self.store.remove_account(id) {
            let _ = self.save_configs(&configs);
            return Err(error.to_string());
        }
        *configs = updated;
        // Clear process-local remnants even if the subsequent credential-store
        // deletion fails and requires the user to finish cleanup manually.
        self.clear_removed_account_caches(id)?;
        // Calendar authorization is separate and must not outlive a removed mailbox.
        let mut failure = None;
        for key in [Self::calendar_vault_id(id), id.to_owned()] {
            for legacy in [false, true] {
                if let Err(error) = self.credentials.delete(&key, legacy) {
                    failure.get_or_insert(error);
                }
            }
        }
        if let Some(error) = failure {
            return Err(format!(
                "Mailbox removed, but credentials require manual cleanup: {error}"
            ));
        }
        Ok(())
    }
    fn sync_account(&self, id: &str) -> Result<Value, String> {
        let Some(_lease) = SyncLease::acquire(&self.config_path, id)? else {
            return Ok(
                json!({"sync_in_progress":true,"messages_added":0,"actions_applied":0,
                "pending_actions":self.store.pending_action_count_for_account(id).map_err(|e|e.to_string())?,
                "actions_retry_after_ms":5000}),
            );
        };
        let account = self.account(id)?;
        if is_demo_account(&account.id) && self.saved_config(id).is_err() {
            return self.settle_demo_account(id);
        }
        let saved = self.saved_config(id)?;
        let mut warnings = Vec::new();
        // Read current mail before draining old changes. Pending intents remain
        // overlaid by mail-core during reconciliation, so this cannot undo local triage.
        let messages = match account.provider {
            Provider::Icloud | Provider::Imap => self
                .imap_connector(&saved)
                .and_then(|client| self.store.run_sync(&client, id).map_err(|e| e.to_string())),
            Provider::Gmail | Provider::Microsoft => self.cloud_client(&saved).and_then(|client| {
                let result = self.store.run_sync(&client.client, id).map_err(|e| e.to_string());
                if let Err(error) = self.save_cloud_tokens(&saved, &client) {
                    warnings.push(format!("refreshed credentials could not be saved: {error}"));
                }
                result
            }),
        };
        let incoming_error = messages.as_ref().err().cloned();
        let report = self.flush_actions(&account, &saved)?;
        warnings.extend(report.warnings);
        if let Some(error) = &incoming_error {
            warnings.push(format!("Incoming mail refresh failed: {error}"));
        }
        let retry_after = incoming_error
            .as_deref()
            .map(action_retry_after_ms)
            .unwrap_or(5000)
            .max(report.retry_after_ms);
        let mut result = json!({"messages_added":messages.unwrap_or(0),"actions_applied":report.applied,
            "pending_actions":report.pending,"actions_retry_after_ms":retry_after,"warnings":warnings});
        if let Some(error) = incoming_error {
            result["sync_error"] = Value::String(error);
        }
        Ok(result)
    }
    /// Seeded demo accounts have no provider: nothing to fetch, and local
    /// changes are final, so sync reports success instead of a failure banner.
    fn settle_demo_account(&self, id: &str) -> Result<Value, String> {
        let mut applied = 0;
        loop {
            let pending = self
                .store
                .pending_actions_for_account(id, 10_000)
                .map_err(|e| e.to_string())?;
            if pending.is_empty() {
                break;
            }
            for action in &pending {
                self.store
                    .confirm_action(&action.operation_id)
                    .map_err(|e| e.to_string())?;
            }
            applied += pending.len();
        }
        Ok(json!({"messages_added":0,"actions_applied":applied,"pending_actions":0,
            "actions_retry_after_ms":5000,"warnings":[]}))
    }
    fn action_account(&self, action: &MailAction) -> Result<String, String> {
        match action {
            MailAction::SendDraft { message_id } => self
                .store
                .message_account_id(message_id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("unknown message: {message_id}")),
            _ => {
                let thread_id = action_thread(action).ok_or("action has no thread")?;
                self.store
                    .thread_account_id(thread_id)
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| format!("unknown thread: {thread_id}"))
            }
        }
    }
    fn flush_actions(
        &self,
        account: &Account,
        saved: &SavedConfig,
    ) -> Result<ActionFlushReport, String> {
        self.flush_actions_with(account, ActionBudget::interactive(), |entries, message_ids| {
            if entries.len()>1 || (is_bulk_read(&entries[0]) && !message_ids.is_empty()) {
                let client=self.cloud_client(saved)?;
                client.gmail_bulk_mark_read(account,message_ids).map_err(|e|e.to_string())?;
                // Provider acceptance must not be replayed solely because credential persistence failed.
                return Ok(self.save_cloud_tokens(saved,&client).err().map(|e|format!("provider accepted action, but refreshed credentials could not be saved: {e}")));
            }
            self.remote_action(account,saved,&entries[0].action)
        })
    }
    fn flush_actions_with(
        &self,
        account: &Account,
        budget: ActionBudget,
        execute: impl FnMut(&[mail_core::QueuedAction], &[String]) -> Result<Option<String>, String>,
    ) -> Result<ActionFlushReport, String> {
        let started = Instant::now();
        self.flush_actions_with_clock(account, budget, execute, || started.elapsed())
    }
    fn flush_actions_with_clock(
        &self,
        account: &Account,
        mut budget: ActionBudget,
        mut execute: impl FnMut(&[mail_core::QueuedAction], &[String]) -> Result<Option<String>, String>,
        mut elapsed: impl FnMut() -> Duration,
    ) -> Result<ActionFlushReport, String> {
        let pending = self
            .store
            .pending_actions_for_account(&account.id, 2048)
            .map_err(|e| e.to_string())?;
        let mut report = ActionFlushReport::default();
        let mut index = 0;
        while index < pending.len() && budget.start_request(elapsed()) {
            let mut end = index + 1;
            let mut ids = Vec::new();
            if account.provider == Provider::Gmail && is_bulk_read(&pending[index]) {
                // Group only contiguous reads. A following unread/move/undo never overtakes them.
                // The API maximum is 1000 message IDs, not 1000 conversations.
                while end <= pending.len() {
                    let candidate = &pending[end - 1];
                    if !is_bulk_read(candidate) {
                        end -= 1;
                        break;
                    }
                    let thread =
                        action_thread(&candidate.action).ok_or("bulk read missing thread")?;
                    let message_ids = self
                        .store
                        .thread_message_ids(thread)
                        .map_err(|e| e.to_string())?;
                    if ids.len() + message_ids.len() > 1000 {
                        end -= 1;
                        break;
                    }
                    ids.extend(message_ids);
                    end += 1;
                }
                end = end.min(pending.len());
                // A single exceptionally large thread uses the thread-level endpoint.
                if end == index {
                    end = index + 1;
                    ids.clear();
                }
            }
            match execute(&pending[index..end], &ids) {
                Ok(warning) => {
                    for entry in &pending[index..end] {
                        self.store
                            .confirm_action(&entry.operation_id)
                            .map_err(|e| e.to_string())?;
                        report.applied += 1;
                    }
                    if let Some(warning) = warning {
                        report.warnings.push(warning);
                    }
                }
                Err(error) => {
                    report.retry_after_ms = action_retry_after_ms(&error);
                    report
                        .warnings
                        .push(format!("Mail changes remain queued: {error}"));
                    break; // Never overtake a failed action, including an in-flight inverse.
                }
            }
            index = end;
        }
        report.pending = self
            .store
            .pending_action_count_for_account(&account.id)
            .map_err(|e| e.to_string())?;
        Ok(report)
    }
    fn remote_action(
        &self,
        account: &Account,
        saved: &SavedConfig,
        action: &MailAction,
    ) -> Result<Option<String>, String> {
        match account.provider {
            Provider::Icloud | Provider::Imap => {
                self.imap_connector(saved)?
                    .apply_action(&self.store, action)
                    .map_err(|e| e.to_string())?;
                Ok(None)
            }
            Provider::Gmail | Provider::Microsoft => {
                let client = self.cloud_client(saved)?;
                let ids = match action_thread(action) {
                    Some(id) => self
                        .store
                        .thread_message_ids(id)
                        .map_err(|e| e.to_string())?,
                    None => Vec::new(),
                };
                client
                    .apply_action(account, action, &ids)
                    .map_err(|e| e.to_string())?;
                Ok(self.save_cloud_tokens(saved, &client).err().map(|e| format!("provider accepted action, but refreshed credentials could not be saved: {e}")))
            }
        }
    }
    fn apply_action(&self, operation_id: &str, action: MailAction) -> Result<Value, String> {
        if matches!(action, MailAction::SendDraft { .. }) {
            return Err("use send_draft for provider-confirmed sending".into());
        }
        let account = self.account(&self.action_account(&action)?)?;
        if matches!(account.provider, Provider::Icloud | Provider::Imap)
            && matches!(
                action,
                MailAction::AddLabel { .. } | MailAction::RemoveLabel { .. }
            )
        {
            return Err("labels are unavailable for IMAP accounts".into());
        }
        self.value(self.store.apply_action(operation_id, action))
    }
    fn send_draft(&self, id: &str) -> Result<Value, String> {
        if !self
            .store
            .claim_immediate_send(id)
            .map_err(|e| e.to_string())?
        {
            return Err("draft is missing, sent, or already queued for sending".into());
        }
        match self.send_draft_inner(id) {
            Ok(value) => Ok(value),
            Err(error) => {
                // A timeout may happen after provider acceptance. Keep the
                // claim for review rather than retrying automatically.
                self.store
                    .fail_outbox(id, &error)
                    .map_err(|e| e.to_string())?;
                Err(error)
            }
        }
    }
    fn dispatch_due(&self) -> Result<Value, String> {
        let mut results = Vec::new();
        for id in self.store.due_outbox(100).map_err(|e| e.to_string())? {
            if !self
                .store
                .claim_due_outbox(&id)
                .map_err(|e| e.to_string())?
            {
                continue;
            }
            match self.send_draft_inner(&id) {
                Ok(_) => results.push(json!({"message_id":id,"status":"sent"})),
                Err(error) => {
                    self.store
                        .fail_outbox(&id, &error)
                        .map_err(|e| e.to_string())?;
                    results.push(json!({"message_id":id,"status":"failed","error":error}));
                }
            }
        }
        Ok(Value::Array(results))
    }
    fn send_draft_inner(&self, id: &str) -> Result<Value, String> {
        let mut draft = self
            .store
            .get_message(id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("unknown message: {id}"))?;
        if !draft.is_draft {
            return Err("message is not an unsent draft".into());
        }
        let (to, headers) =
            mail_core::normalize_outgoing_recipients(&draft.to, draft.recipient_headers.as_ref())
                .map_err(|e| e.to_string())?;
        draft.to = to;
        draft.recipient_headers = Some(headers.clone());
        let attachments = self
            .store
            .outgoing_attachments(id)
            .map_err(|e| e.to_string())?;
        let account = self.account(&draft.account_id)?;
        let saved = self.saved_config(&account.id)?;
        let from = draft.sender.trim();
        if from.is_empty() || from.contains(['\r', '\n']) {
            return Err("invalid From address".into());
        }
        let outgoing_html = self.tracking_outgoing_html(&draft)?;
        match account.provider {
            Provider::Icloud | Provider::Imap => {
                if !from.eq_ignore_ascii_case(&account.email) {
                    return Err("this account has not configured that From address".into());
                }
                self.imap_connector(&saved)?
                    .send_with_recipients(
                        from,
                        &draft.to,
                        &headers.cc,
                        &headers.bcc,
                        &draft.subject,
                        &draft.body_plain,
                        outgoing_html.as_deref(),
                        &attachments,
                    )
                    .map_err(|e| e.to_string())?
            }
            Provider::Gmail | Provider::Microsoft => {
                let client = self.cloud_client(&saved)?;
                if account.provider == Provider::Gmail {
                    let aliases = client.send_as_aliases().map_err(|e| e.to_string())?;
                    if !aliases
                        .iter()
                        .any(|alias| alias.email.eq_ignore_ascii_case(from))
                    {
                        return Err(
                            "this From address is not a verified Gmail send-as address".into()
                        );
                    }
                } else if !from.eq_ignore_ascii_case(&account.email) {
                    return Err("this account has not configured that From address".into());
                }
                let provider_id = client
                    .send_in_thread_html(
                        &account,
                        from,
                        &draft.to.join(", "),
                        &headers.cc.join(", "),
                        &headers.bcc.join(", "),
                        &draft.subject,
                        &draft.body_plain,
                        outgoing_html.as_deref(),
                        &attachments,
                        (account.provider == Provider::Gmail && draft.thread_id.contains(":gmail-thread:")).then_some(draft.thread_id.as_str()),
                    )
                    .map_err(|e| e.to_string())?;
                let untracked = self.mark_sent(id).map_err(|e| format!("provider accepted the message, but local sent state could not be saved: {e}"))?;
                if let Some(provider_id) = provider_id {
                    self.store.adopt_sent_provider_id(id, &provider_id).map_err(|e| format!("message sent but provider identity could not be saved: {e}"))?;
                }
                let warning = self
                    .save_cloud_tokens(&saved, &client)
                    .err()
                    .map(|e| format!("refreshed credentials could not be saved: {e}"));
                let warning = match (untracked, warning) {
                    (Some(a), Some(b)) => Some(format!("{a} {b}")),
                    (a, b) => a.or(b),
                };
                return Ok(json!({"sent":true,"message_id":id,"warning":warning}));
            }
        }
        let warning = self.mark_sent(id).map_err(|e| {
            format!("provider accepted the message, but local sent state could not be saved: {e}")
        })?;
        Ok(json!({"sent":true,"message_id":id,"warning":warning}))
    }
}

/// Accounts created by `seed_demo` use `demo:` IDs; real accounts get UUIDs.
fn is_demo_account(id: &str) -> bool {
    id.starts_with("demo:")
}
/// Attachment listing from a MIME source (IMAP mail and seeded demo mail).
fn raw_attachment_list(raw: &[u8]) -> Result<Value, String> {
    let files = attachment_metadata(raw).map_err(|e| e.to_string())?;
    Ok(json!(files.into_iter().map(|file| json!({
        "part_id": file.part_id,
        "filename": file.filename.unwrap_or_else(|| "Attachment".into()),
        "mime_type": file.content_type,
        "size": file.decoded_size,
        "content_id": file.content_id,
    })).collect::<Vec<_>>()))
}
/// Resolve `cid:` pictures from a MIME source (IMAP mail and seeded demo mail).
fn raw_inline_images(safe_html: &str, raw: &[u8]) -> Result<String, String> {
    let parts = attachment_metadata(raw)
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter_map(|item| {
            item.content_id.map(|content_id| InlinePart {
                part_id: item.part_id,
                content_id,
                mime_type: item.content_type,
                size: item.decoded_size as u64,
            })
        })
        .collect::<Vec<_>>();
    Ok(replace_inline_images(safe_html, &parts, |id| {
        attachment_bytes(raw, id).map_err(|e| e.to_string())
    }))
}
/// The sanitizer for message HTML shown in the reader on both platforms:
/// ammonia's default tags and URL schemes, plus `cid:` images and a few
/// layout attributes.
fn message_html_sanitizer() -> ammonia::Builder<'static> {
    let mut sanitizer = ammonia::Builder::default();
    sanitizer.add_generic_attributes(&["style", "align", "width", "height", "bgcolor"]);
    sanitizer.add_url_schemes(&["cid"]);
    sanitizer
}
fn upgrade_image_urls(html: &str) -> String {
    // Ammonia serializes allowed image attributes with double quotes. Keep
    // links and other URL-bearing attributes untouched.
    html.replace("src=\"http://", "src=\"https://")
}
fn timestamp_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[derive(Clone)]
struct InlinePart {
    part_id: String,
    content_id: String,
    mime_type: String,
    size: u64,
}
fn normalize_cid(value: &str) -> Option<String> {
    let decoded = percent_decode_str(value).decode_utf8().ok()?;
    let normalized = decoded
        .trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .to_ascii_lowercase();
    (!normalized.is_empty()).then_some(normalized)
}
fn replace_inline_images(
    safe_html: &str,
    parts: &[InlinePart],
    mut fetch: impl FnMut(&str) -> Result<Vec<u8>, String>,
) -> String {
    const MAX_INLINE_COUNT: usize = 8;
    const MAX_INLINE_BYTES: usize = 2 * 1024 * 1024;
    const MAX_TOTAL_BYTES: usize = 8 * 1024 * 1024;
    let src = Regex::new(r#"(?i)\bsrc="cid:([^"]+)""#).expect("fixed CID regex");
    let mut urls = HashMap::new();
    let mut total = 0;
    for captures in src.captures_iter(safe_html) {
        let Some(cid) = normalize_cid(&captures[1]) else {
            continue;
        };
        if urls.contains_key(&cid) || urls.len() >= MAX_INLINE_COUNT {
            continue;
        }
        let Some(part) = parts
            .iter()
            .find(|part| normalize_cid(&part.content_id).as_deref() == Some(cid.as_str()))
        else {
            continue;
        };
        let mime = part
            .mime_type
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        if !matches!(
            mime.as_str(),
            "image/png" | "image/jpeg" | "image/gif" | "image/webp"
        ) || part.size as usize > MAX_INLINE_BYTES
            || total + part.size as usize > MAX_TOTAL_BYTES
        {
            continue;
        }
        let Ok(bytes) = fetch(&part.part_id) else {
            continue;
        };
        if bytes.is_empty()
            || bytes.len() > MAX_INLINE_BYTES
            || total + bytes.len() > MAX_TOTAL_BYTES
            || !valid_inline_image(&mime, &bytes)
        {
            continue;
        }
        total += bytes.len();
        urls.insert(
            cid,
            format!("data:{mime};base64,{}", STANDARD.encode(bytes)),
        );
    }
    src.replace_all(safe_html, |captures: &regex::Captures<'_>| {
        normalize_cid(&captures[1])
            .and_then(|cid| urls.get(&cid).map(|url| format!("src=\"{url}\"")))
            .unwrap_or_else(|| captures[0].to_string())
    })
    .into_owned()
}
fn valid_inline_image(mime: &str, bytes: &[u8]) -> bool {
    match mime {
        "image/png" => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        "image/jpeg" => bytes.starts_with(&[0xff, 0xd8, 0xff]),
        "image/gif" => bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"),
        "image/webp" => bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP",
        _ => false,
    }
}
fn ensure_private_config(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err("account config path must not be a symlink".into());
            }
            Ok(_) => fs::set_permissions(path, fs::Permissions::from_mode(0o600))
                .map_err(|e| e.to_string())?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(())
}
fn action_thread(action: &MailAction) -> Option<&str> {
    match action {
        MailAction::Archive { thread_id }
        | MailAction::MarkRead { thread_id, .. }
        | MailAction::Move { thread_id, .. }
        | MailAction::AddLabel { thread_id, .. }
        | MailAction::RemoveLabel { thread_id, .. } => Some(thread_id),
        MailAction::SendDraft { .. } => None,
    }
}

/// # Safety
/// path must be a NUL-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mail_service_open(path: *const c_char) -> *mut MailService {
    if path.is_null() {
        return std::ptr::null_mut();
    }
    catch_unwind(AssertUnwindSafe(|| {
        let path = unsafe { CStr::from_ptr(path) }.to_str().ok()?;
        MailService::open(path)
            .ok()
            .map(Box::new)
            .map(Box::into_raw)
    }))
    .ok()
    .flatten()
    .unwrap_or(std::ptr::null_mut())
}
/// # Safety
/// service must be a live pointer returned by mail_service_open.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mail_service_close(service: *mut MailService) {
    if !service.is_null() {
        let _ = catch_unwind(AssertUnwindSafe(|| drop(unsafe { Box::from_raw(service) })));
    }
}
/// # Safety
/// service must be live and raw must be a NUL-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mail_service_request_json(
    service: *mut MailService,
    raw: *const c_char,
) -> *mut c_char {
    let value = catch_unwind(AssertUnwindSafe(|| {
        if service.is_null() || raw.is_null() {
            return json!({"ok":false,"error":"null pointer"});
        }
        match unsafe { CStr::from_ptr(raw) }.to_str() {
            Ok(request) => unsafe { &*service }.request_envelope(request),
            Err(_) => json!({"ok":false,"error":"request must be UTF-8"}),
        }
    }))
    .unwrap_or_else(|_| json!({"ok":false,"error":"panic in mail service"}));
    CString::new(value.to_string())
        .expect("JSON NUL-free")
        .into_raw()
}
/// The compiled distribution profile as JSON, for shells that load the engine
/// dynamically. It needs no open store. Free the result with
/// mail_service_free_string.
#[unsafe(no_mangle)]
pub extern "C" fn mail_service_distribution_profile_json() -> *mut c_char {
    CString::new(profile::json().to_string())
        .expect("JSON NUL-free")
        .into_raw()
}
/// # Safety
/// value must be returned by mail_service_request_json or
/// mail_service_distribution_profile_json and not previously freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mail_service_free_string(value: *mut c_char) {
    if !value.is_null() {
        let _ = unsafe { CString::from_raw(value) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn agent_access_ops_render_one_disclosure_and_gate_the_dispatcher() {
        let (service, dir, _) = fairness_fixture();
        let status = service.request_json(r#"{"op":"agent_access_status"}"#).unwrap();
        assert_eq!(status["accounts"][0]["consent"], Value::Null);
        assert_eq!(status["pending"]["count"], 0);
        let disclosure = service
            .request_json(r#"{"op":"agent_access_disclosure","account_id":"demo:personal"}"#)
            .unwrap();
        assert_eq!(disclosure["version"], mail_core::AGENT_DISCLOSURE_VERSION);
        assert!(disclosure["title"].as_str().unwrap().contains("alex@example.com"));
        assert_eq!(disclosure["grants"].as_array().unwrap().len(), 5);
        assert!(disclosure["provider_terms"].as_str().unwrap().contains("provider's terms"));
        assert!(service
            .request_json(r#"{"op":"grant_agent_access","account_id":"demo:personal","grants":["send"],"disclosure_version":0}"#)
            .is_err());
        service
            .request_json(r#"{"op":"grant_agent_access","account_id":"demo:personal","grants":["send"],"disclosure_version":1}"#)
            .unwrap();
        let status = service.request_json(r#"{"op":"agent_access_status"}"#).unwrap();
        assert_eq!(status["accounts"][0]["grant_labels"], json!(["Read", "Send requests"]));
        let overview = service.request_json(r#"{"op":"agent_access_overview"}"#).unwrap();
        assert_eq!(overview["requests"], json!([]));
        assert_eq!(overview["history"][0]["kind"], "consent_granted");
        let request = |message_id: &str| {
            service
                .store
                .request_agent_approval(
                    &mail_core::AgentApprovalRequest::SendDraft {
                        message_id: message_id.into(),
                    },
                    Some("test-agent"),
                )
                .unwrap()
                .id
        };
        let approved_draft = service
            .store
            .compose_draft("demo:personal", "maya@example.org", "Approved", "Body")
            .unwrap()
            .id;
        let rejected_draft = service
            .store
            .compose_draft("demo:personal", "rowan@example.net", "Rejected", "Body")
            .unwrap()
            .id;
        let approve_id = request(&approved_draft);
        let reject_id = request(&rejected_draft);
        let summary = service.request_json(r#"{"op":"agent_pending_summary"}"#).unwrap();
        assert_eq!(summary["count"], 2);
        // The scheduler path finds nothing while requests wait for a decision.
        assert_eq!(service.request_json(r#"{"op":"dispatch_due"}"#).unwrap(), json!([]));
        let rejected = service
            .request_json(&json!({"op":"reject_agent_action","id":reject_id}).to_string())
            .unwrap();
        assert_eq!(rejected["status"], "rejected");
        let approved = service
            .request_json(
                &json!({"op":"approve_agent_action","id":approve_id,"undo_seconds":1}).to_string(),
            )
            .unwrap();
        assert_eq!(approved["status"], "approved");
        assert_eq!(service.request_json(r#"{"op":"dispatch_due"}"#).unwrap(), json!([]));
        std::thread::sleep(Duration::from_millis(1_100));
        // After the undo window only the approved draft is claimed. The demo
        // account has no provider configuration, so nothing is actually sent.
        let dispatched = service.request_json(r#"{"op":"dispatch_due"}"#).unwrap();
        let dispatched = dispatched.as_array().unwrap();
        assert_eq!(dispatched.len(), 1);
        assert_eq!(dispatched[0]["message_id"], approved_draft);
        assert_eq!(dispatched[0]["status"], "failed");
        assert!(service.store.outbox_status(&rejected_draft).unwrap().is_none());
        let history = service
            .request_json(r#"{"op":"list_agent_actions","status":"history"}"#)
            .unwrap();
        assert!(history.as_array().unwrap().iter().any(|a| a["id"] == reject_id));
        service
            .request_json(r#"{"op":"revoke_agent_access","account_id":"demo:personal"}"#)
            .unwrap();
        let status = service.request_json(r#"{"op":"agent_access_status"}"#).unwrap();
        assert_eq!(status["accounts"][0]["consent"], Value::Null);
        let _ = fs::remove_dir_all(dir);
    }
    fn fairness_fixture() -> (MailService, PathBuf, Account) {
        let dir = std::env::temp_dir().join(format!("email-fairness-{}", Uuid::new_v4()));
        let service = MailService::open(dir.join("mail.sqlite")).unwrap();
        service.store.seed_demo().unwrap();
        let account = service.account("demo:personal").unwrap();
        (service, dir, account)
    }
    #[test]
    fn scoped_search_contract_filters_before_limit_and_pages_local_results() {
        let (service, dir, _) = fairness_fixture();
        service
            .store
            .upsert_account(&Account {
                id: "other".into(),
                provider: Provider::Gmail,
                email: "other@example.com".into(),
                display_name: "Other".into(),
            })
            .unwrap();
        let mut message = service.store.get_message("demo:1").unwrap().unwrap();
        message.id = "foreign-newer".into();
        message.thread_id = message.id.clone();
        message.account_id = "other".into();
        message.received_at += 1000;
        service.store.upsert_message(&message).unwrap();
        let rows = service.request_json(r#"{"op":"search","query":"Friday","account_id":"demo:personal","unread_only":true,"is_inbox":true,"limit":1}"#).unwrap();
        assert_eq!(rows.as_array().unwrap().len(), 1);
        assert_eq!(rows[0]["id"], "demo:1");
        assert_eq!(
            service
                .request_json(r#"{"op":"search","query":"Friday","account_id":"other","limit":1}"#)
                .unwrap()[0]["id"],
            "foreign-newer"
        );
        assert!(
            service
                .request_json(
                    r#"{"op":"search","query":"Friday","account_id":"missing","limit":1}"#
                )
                .unwrap()
                .as_array()
                .unwrap()
                .is_empty()
        );
        // Empty text is a scoped cached listing; paging does not require a network request.
        assert!(
            service
                .request_json(r#"{"op":"search","query":"","limit":1}"#)
                .unwrap()
                .as_array()
                .unwrap()
                .len()
                == 1
        );
        let first = service
            .request_json(r#"{"op":"search","query":"","account_id":"demo:personal","limit":1}"#)
            .unwrap();
        let second = service
            .request_json(
                r#"{"op":"search","query":"","account_id":"demo:personal","limit":1,"offset":1}"#,
            )
            .unwrap();
        assert_ne!(first[0]["id"], second[0]["id"]);
        // Existing minimal requests still deserialize without any new required fields.
        assert!(
            service
                .request_json(r#"{"op":"search","query":"Friday","limit":10}"#)
                .unwrap()
                .as_array()
                .unwrap()
                .len()
                >= 2
        );
        drop(service);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn scoped_search_contract_combines_label_split_favorites_archive_and_snoozes() {
        let (service, dir, _) = fairness_fixture();
        for (op, label) in [("search-star", "STARRED"), ("search-label", "Work")] {
            service
                .store
                .apply_action(
                    op,
                    MailAction::AddLabel {
                        thread_id: "demo:1".into(),
                        label: label.into(),
                    },
                )
                .unwrap();
        }
        let split = service
            .store
            .upsert_split(None, "Maya", "maya@example.org")
            .unwrap();
        let request = json!({"op":"search","query":"Friday","account_id":"demo:personal","unread_only":true,"starred_only":true,"label":"Work","split_id":split.id,"limit":1});
        assert_eq!(
            service.request_json(&request.to_string()).unwrap()[0]["id"],
            "demo:1"
        );
        let mut absent = request.clone();
        absent["label"] = json!("Missing");
        assert!(
            service
                .request_json(&absent.to_string())
                .unwrap()
                .as_array()
                .unwrap()
                .is_empty()
        );
        service
            .store
            .apply_action(
                "search-archive",
                MailAction::Archive {
                    thread_id: "demo:1".into(),
                },
            )
            .unwrap();
        let archive = service
            .request_json(r#"{"op":"search","query":"Friday","folder":"archive","limit":1}"#)
            .unwrap();
        assert_eq!(archive[0]["id"], "demo:1");
        assert!(
            service
                .request_json(r#"{"op":"search","query":"Friday","folder":"inbox","limit":1}"#)
                .unwrap()
                .as_array()
                .unwrap()
                .is_empty()
        );
        service
            .store
            .snooze_thread("demo:1", timestamp_ms() + 60_000)
            .unwrap();
        assert_eq!(
            service
                .request_json(r#"{"op":"search","query":"","snoozed_only":true,"limit":1}"#)
                .unwrap()[0]["id"],
            "demo:1"
        );
        drop(service);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn outgoing_pass_is_bounded_and_failure_does_not_overtake() {
        let (service, dir, account) = fairness_fixture();
        for i in 0..40 {
            service
                .store
                .apply_action(
                    &format!("queue-{i}"),
                    MailAction::Archive {
                        thread_id: "demo:1".into(),
                    },
                )
                .unwrap();
        }
        let mut seen = Vec::new();
        let report = service
            .flush_actions_with(
                &account,
                ActionBudget {
                    remaining: 3,
                    max_elapsed: Duration::from_secs(1),
                },
                |batch, _| {
                    seen.extend(batch.iter().map(|x| x.operation_id.clone()));
                    Ok(None)
                },
            )
            .unwrap();
        assert_eq!(seen, vec!["queue-0", "queue-1", "queue-2"]);
        assert_eq!((report.applied, report.pending), (3, 37));
        let failed = service
            .flush_actions_with(&account, ActionBudget::interactive(), |batch, _| {
                assert_eq!(batch[0].operation_id, "queue-3");
                Err(
                    "Gmail rate limit; retrying after 7 seconds. Cached mail remains available."
                        .into(),
                )
            })
            .unwrap();
        assert_eq!(
            (failed.applied, failed.pending, failed.retry_after_ms),
            (0, 37, 7000)
        );
        assert_eq!(
            service
                .store
                .pending_actions_for_account(&account.id, 1)
                .unwrap()[0]
                .operation_id,
            "queue-3"
        );
        drop(service);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn outgoing_elapsed_budget_is_checked_between_requests() {
        let (service, dir, account) = fairness_fixture();
        for i in 0..3 {
            service
                .store
                .apply_action(
                    &format!("timed-{i}"),
                    MailAction::Archive {
                        thread_id: "demo:1".into(),
                    },
                )
                .unwrap();
        }
        let mut calls = 0;
        let elapsed = std::cell::Cell::new(Duration::ZERO);
        let report = service
            .flush_actions_with_clock(
                &account,
                ActionBudget {
                    remaining: 40,
                    max_elapsed: Duration::from_millis(10),
                },
                |_, _| {
                    calls += 1;
                    elapsed.set(Duration::from_millis(20));
                    Ok(None)
                },
                || elapsed.get(),
            )
            .unwrap();
        assert_eq!(calls, 1);
        assert_eq!((report.applied, report.pending), (1, 2));
        drop(service);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn grouped_reads_preserve_a_following_unread_and_undo_order() {
        let (service, dir, account) = fairness_fixture();
        for i in 1..=3 {
            service
                .store
                .apply_action(
                    &format!("bulk:first-{i}"),
                    MailAction::MarkRead {
                        thread_id: format!("demo:{i}"),
                        read: true,
                    },
                )
                .unwrap();
        }
        service
            .store
            .apply_action(
                "later-unread",
                MailAction::MarkRead {
                    thread_id: "demo:1".into(),
                    read: false,
                },
            )
            .unwrap();
        service
            .store
            .apply_action(
                "triage:later-archive",
                MailAction::Archive {
                    thread_id: "demo:1".into(),
                },
            )
            .unwrap();
        let mut batches = Vec::new();
        let report = service
            .flush_actions_with(
                &account,
                ActionBudget {
                    remaining: 2,
                    max_elapsed: Duration::from_secs(1),
                },
                |batch, ids| {
                    batches.push((
                        batch
                            .iter()
                            .map(|x| x.operation_id.clone())
                            .collect::<Vec<_>>(),
                        ids.len(),
                    ));
                    Ok(None)
                },
            )
            .unwrap();
        assert_eq!(batches[0].0.len(), 3);
        assert_eq!(batches[0].1, 3);
        assert_eq!(batches[1].0, vec!["later-unread"]);
        assert_eq!((report.applied, report.pending), (4, 1));
        assert_eq!(
            service
                .store
                .pending_actions_for_account(&account.id, 1)
                .unwrap()[0]
                .operation_id,
            "triage:later-archive"
        );
        drop(service);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn account_sync_lanes_are_bounded_and_failures_are_isolated() {
        let accounts: Vec<_> = (0..10)
            .map(|i| Account {
                id: i.to_string(),
                provider: Provider::Gmail,
                email: format!("{i}@example.org"),
                display_name: i.to_string(),
            })
            .collect();
        let active = AtomicUsize::new(0);
        let max = AtomicUsize::new(0);
        let results = sync_account_lanes(&accounts, |id| {
            let count = active.fetch_add(1, Ordering::SeqCst) + 1;
            max.fetch_max(count, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(10));
            active.fetch_sub(1, Ordering::SeqCst);
            if id == "0" {
                Err("one account failed".into())
            } else {
                Ok(json!({"messages_added":1}))
            }
        });
        assert!(max.load(Ordering::SeqCst) > 1);
        assert!(max.load(Ordering::SeqCst) <= 4);
        assert_eq!(results.len(), 10);
        assert_eq!(results[0]["ok"], false);
        for (i, row) in results.iter().enumerate().skip(1) {
            assert_eq!(row["account_id"], i.to_string());
            assert_eq!(row["ok"], true);
        }
    }

    #[test]
    fn sync_leases_coalesce_handles_but_not_other_accounts_or_stores() {
        let root = std::env::temp_dir().join(Uuid::new_v4().to_string());
        let path = root.join("mail.accounts.json");
        let first = SyncLease::acquire(&path, "a").unwrap().unwrap();
        assert!(SyncLease::acquire(&path, "a").unwrap().is_none());
        let other_account = SyncLease::acquire(&path, "b").unwrap().unwrap();
        let other_store = SyncLease::acquire(&root.join("other.accounts.json"), "a")
            .unwrap()
            .unwrap();
        drop(first);
        assert!(SyncLease::acquire(&path, "a").unwrap().is_some());
        drop((other_account, other_store));
    }

    #[test]
    fn message_sanitizer_strips_svg_and_mathml_mutation_payloads() {
        // The first two are the RUSTSEC-2026-0533 payloads fixed in ammonia
        // 4.2.5. They need svg/math tags allowed and comments kept, which the
        // reader's sanitizer never does; this pins that it stays that way.
        let payloads = [
            "<svg><foreignObject><math><iframe><!--</iframe><img src onerror=alert(1)>-->",
            "<math><mi><svg><iframe><!--</iframe><img src onerror=alert(1)>-->",
            r#"<math><mtext><table><mglyph><style><!--</style><img title="--&gt;&lt;/mglyph&gt;&lt;img&Tab;src=1&Tab;onerror=alert(1)&gt;">"#,
            "<svg><style><img src=x onerror=alert(1)></style></svg>",
            "<svg></p><style><a id=\"</style><img src=1 onerror=alert(1)>\">",
            r#"<math><annotation-xml encoding="text/html"><img src=x onerror=alert(1)></annotation-xml></math>"#,
            r#"<svg><a xlink:href="javascript:alert(1)"><text>x</text></a></svg>"#,
        ];
        for payload in payloads {
            let safe = message_html_sanitizer().clean(payload).to_string();
            let lower = safe.to_ascii_lowercase();
            for needle in ["onerror", "<svg", "<math", "<iframe", "<style", "<script", "javascript:"] {
                assert!(!lower.contains(needle), "{payload} -> {safe}");
            }
            // Mutation XSS changes meaning when sanitized output is parsed
            // again; sanitizing it again must be a no-op.
            assert_eq!(message_html_sanitizer().clean(&safe).to_string(), safe, "{payload}");
        }
    }

    #[test]
    fn image_upgrade_only_changes_sanitized_image_sources() {
        let safe = ammonia::Builder::default()
            .clean(r#"<a href="http://example.test/page"><img src="http://cdn.example.test/logo.png"></a>"#)
            .to_string();
        let upgraded = upgrade_image_urls(&safe);
        assert!(upgraded.contains("src=\"https://cdn.example.test/logo.png\""));
        assert!(upgraded.contains("href=\"http://example.test/page\""));
    }
    #[test]
    fn sanitized_cid_images_are_bounded_and_type_checked() {
        let mut sanitizer = ammonia::Builder::default();
        sanitizer.add_url_schemes(&["cid"]);
        let safe = sanitizer
            .clean(r#"<img src="cid:Logo%40example"><img src="https://images.example.test/tracker.png"><script>alert(1)</script>"#)
            .to_string();
        assert!(safe.contains("cid:Logo%40example"));
        assert!(!safe.contains("<script"));
        let parts = vec![InlinePart {
            part_id: "one".into(),
            content_id: "<Logo@example>".into(),
            mime_type: "image/gif".into(),
            size: 9,
        }];
        let mut fetched = 0;
        let rendered = replace_inline_images(&safe, &parts, |_| {
            fetched += 1;
            Ok(b"GIF89a123".to_vec())
        });
        assert_eq!(fetched, 1); // Never fetch a remote URL through CID resolution.
        assert!(rendered.contains("https://images.example.test/tracker.png"));
        assert!(rendered.contains("src=\"data:image/gif;base64,"));
        assert!(!rendered.contains("cid:"));
        let spoofed = replace_inline_images(&safe, &parts, |_| Ok(b"<svg/>".to_vec()));
        assert!(spoofed.contains("cid:Logo%40example"));
        let unsupported = vec![InlinePart {
            mime_type: "image/svg+xml".into(),
            ..parts[0].clone()
        }];
        let mut fetched_unsupported = false;
        let skipped_type = replace_inline_images(&safe, &unsupported, |_| {
            fetched_unsupported = true;
            Ok(b"<svg/>".to_vec())
        });
        assert!(!fetched_unsupported);
        assert!(skipped_type.contains("cid:Logo%40example"));
        let oversized = vec![InlinePart {
            size: 3 * 1024 * 1024,
            ..parts[0].clone()
        }];
        let mut fetched = false;
        let skipped = replace_inline_images(&safe, &oversized, |_| {
            fetched = true;
            Ok(vec![])
        });
        assert!(!fetched);
        assert!(skipped.contains("cid:Logo%40example"));
    }
    #[test]
    fn launch_demo_serves_files_pictures_and_invitations_without_a_provider() {
        let dir = std::env::temp_dir().join(format!("email-launch-demo-{}", Uuid::new_v4()));
        let service = MailService::open(dir.join("mail.sqlite")).unwrap();
        assert!(service
            .request_json(r#"{"op":"seed_demo","set":"party"}"#)
            .unwrap_err()
            .contains("Unknown demo set"));
        let summary = service
            .request_json(r#"{"op":"seed_demo","set":"launch","utc_offset_minutes":60}"#)
            .unwrap();
        assert_eq!(summary["seeded"], true);
        assert!(summary["threads"].as_u64().unwrap() >= 250);
        // A second request leaves the seeded store alone.
        let again = service
            .request_json(r#"{"op":"seed_demo","set":"launch"}"#)
            .unwrap();
        assert_eq!(again["seeded"], false);

        // A PDF attachment lists and downloads from the stored MIME source.
        let files = service
            .request_json(r#"{"op":"list_attachments","message_id":"demo:launch:brightwater-pilot:0"}"#)
            .unwrap();
        let pdf = files
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["mime_type"] == "application/pdf")
            .unwrap();
        assert_eq!(pdf["filename"], "Brightwater-pilot-summary.pdf");
        let data = service
            .request_json(&json!({"op":"fetch_attachment","message_id":"demo:launch:brightwater-pilot:0","part_id":pdf["part_id"]}).to_string())
            .unwrap();
        let bytes = STANDARD.decode(data["data_base64"].as_str().unwrap()).unwrap();
        assert!(bytes.starts_with(b"%PDF-1.4"));

        // Inline cid: pictures resolve to data URLs; remote images stay remote.
        let rendered = service
            .request_json(r#"{"op":"render_message","message_id":"demo:launch:onboarding-v3:2","include_inline_images":true}"#)
            .unwrap();
        let html = rendered["body_html_safe"].as_str().unwrap();
        assert!(html.contains("data:image/png;base64,"), "{html}");
        assert!(!html.contains("cid:"));
        let newsletter = service
            .request_json(r#"{"op":"render_message","message_id":"demo:launch:quiet-0:0"}"#)
            .unwrap();
        assert!(newsletter["body_html_safe"]
            .as_str()
            .unwrap()
            .contains("src=\"https://img.quietinterface.example/"));

        // Invitations are cached at seeding and refresh from the same source.
        let cached = service
            .request_json(r#"{"op":"calendar_invitation","message_id":"demo:launch:invite-go-no-go:0"}"#)
            .unwrap();
        assert_eq!(cached[0]["event"]["summary"], "Launch go/no-go");
        let refreshed = service
            .request_json(r#"{"op":"refresh_calendar_invitation","message_id":"demo:launch:invite-go-no-go:0"}"#)
            .unwrap();
        assert_eq!(refreshed[0]["event"]["method"], "REQUEST");

        // Demo accounts have no provider: sync succeeds and local triage is final.
        service
            .request_json(r#"{"op":"apply_action","operation_id":"demo-archive","action":{"type":"archive","thread_id":"demo:launch:parcel"}}"#)
            .unwrap();
        let lanes = service.request_json(r#"{"op":"sync_all"}"#).unwrap();
        assert!(lanes.as_array().unwrap().iter().all(|lane| lane["ok"] == true), "{lanes}");
        assert!(service.store.pending_actions().unwrap().is_empty());
        // Search over the seeded inbox, as the apps request it.
        let rows = service
            .request_json(r#"{"op":"search","query":"invoice","limit":100,"offset":0,"is_inbox":true}"#)
            .unwrap();
        assert!(!rows.as_array().unwrap().is_empty());
        drop(service);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn compose_cc_bcc_contract_preserves_omitted_fields_and_rejects_invalid_queue() {
        let dir = std::env::temp_dir().join(format!("email-cc-contract-{}", Uuid::new_v4()));
        let service = MailService::open(dir.join("mail.sqlite")).unwrap();
        service.request_json(r#"{"op":"seed_demo"}"#).unwrap();
        let draft = service.request_json(r#"{"op":"compose_draft","account_id":"demo:personal","to":"","cc":"copy@example.com","bcc":"hidden@example.com","subject":"Subject","body_plain":"Body"}"#).unwrap();
        assert_eq!(draft["recipient_headers"]["cc"][0], "copy@example.com");
        assert_eq!(draft["recipient_headers"]["bcc"][0], "hidden@example.com");
        let id = draft["id"].as_str().unwrap();
        let updated = service.request_json(&json!({"op":"update_draft","message_id":id,"to":"","subject":"Changed","body_plain":"Body","cc":""}).to_string()).unwrap();
        assert!(
            updated["recipient_headers"]["cc"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert_eq!(updated["recipient_headers"]["bcc"][0], "hidden@example.com");
        service.request_json(&json!({"op":"update_draft","message_id":id,"to":"valid@example.com","subject":"Changed","body_plain":"Body","bcc":"invalid@@example.com"}).to_string()).unwrap();
        assert!(service.request_json(&json!({"op":"schedule_draft","message_id":id,"send_at_ms":timestamp_ms()+60_000}).to_string()).is_err());
        assert!(
            service
                .request_json(r#"{"op":"list_outbox"}"#)
                .unwrap()
                .as_array()
                .unwrap()
                .is_empty()
        );
        drop(service);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn scheduled_drafts_and_attachments_survive_restart() {
        let dir = std::env::temp_dir().join(format!("email-service-workflows-{}", Uuid::new_v4()));
        let path = dir.join("mail.sqlite");
        let service = MailService::open(&path).unwrap();
        service.request_json(r#"{"op":"seed_demo"}"#).unwrap();
        let draft = service.request_json(r#"{"op":"compose_draft","account_id":"demo:personal","to":"friend@example.net","subject":"Later","body_plain":"Body"}"#).unwrap();
        let id = draft["id"].as_str().unwrap().to_string();
        let added = service.request_json(&json!({"op":"add_draft_attachment","message_id":id,"filename":"note.txt","mime_type":"text/plain","content_base64":STANDARD.encode(b"hello")}).to_string()).unwrap();
        assert_eq!(added["size"], 5);
        let scheduled = service
            .request_json(
                &json!({"op":"schedule_draft","message_id":id,"send_at_ms":timestamp_ms()+60_000})
                    .to_string(),
            )
            .unwrap();
        assert_eq!(scheduled["status"], "scheduled");
        assert!(
            service
                .request_json(r#"{"op":"list_drafts"}"#)
                .unwrap()
                .as_array()
                .unwrap()
                .is_empty()
        );
        drop(service);
        let service = MailService::open(&path).unwrap();
        assert_eq!(
            service
                .request_json(r#"{"op":"list_outbox"}"#)
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            service
                .request_json(&json!({"op":"list_draft_attachments","message_id":id}).to_string())
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            1
        );
        service
            .request_json(&json!({"op":"cancel_scheduled_draft","message_id":id}).to_string())
            .unwrap();
        assert_eq!(
            service
                .request_json(r#"{"op":"list_drafts"}"#)
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            1
        );
        drop(service);
        fs::remove_dir_all(dir).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn existing_account_config_is_migrated_to_private_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("email-service-perms-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("mail.sqlite");
        let config = path.with_extension("accounts.json");
        fs::write(&config, b"[]").unwrap();
        fs::set_permissions(&config, fs::Permissions::from_mode(0o644)).unwrap();
        drop(MailService::open(&path).unwrap());
        assert_eq!(
            fs::metadata(&config).unwrap().permissions().mode() & 0o777,
            0o600
        );
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn json_contract_keeps_unsent_drafts_and_sanitizes_html() {
        let dir = std::env::temp_dir().join(format!("email-service-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("mail.sqlite");
        let service = MailService::open(&path).unwrap();
        service.request_json(r#"{"op":"seed_demo"}"#).unwrap();
        assert!(
            !service
                .request_json(r#"{"op":"list_threads"}"#)
                .unwrap()
                .as_array()
                .unwrap()
                .is_empty()
        );
        let draft = service.request_json(r#"{"op":"compose_draft","account_id":"demo:personal","to":"friend@example.net","subject":"Hello","body_plain":"Draft text"}"#).unwrap();
        let id = draft["id"].as_str().unwrap();
        assert_eq!(
            service
                .request_json(r#"{"op":"list_drafts"}"#)
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let updated = service.request_json(&json!({"op":"update_draft","message_id":id,"to":"new@example.net","subject":"Updated","body_plain":"New text"}).to_string()).unwrap();
        assert_eq!(updated["subject"], "Updated");
        assert!(
            service
                .request_json(&json!({"op":"send_draft","message_id":id}).to_string())
                .is_err()
        );
        assert!(service.store.get_message(id).unwrap().unwrap().is_draft);
        let mut message = service.store.get_message(id).unwrap().unwrap();
        message.body_html = Some("<p style=\"color:red\">Safe</p><script>alert(1)</script>".into());
        service.store.upsert_message(&message).unwrap();
        let rendered = service
            .request_json(&json!({"op":"render_message","message_id":id}).to_string())
            .unwrap();
        assert!(
            !rendered["body_html_safe"]
                .as_str()
                .unwrap()
                .contains("<script>")
        );
        assert!(
            rendered["body_html_safe"]
                .as_str()
                .unwrap()
                .contains("style=")
        );
        let c_path = CString::new(path.to_str().unwrap()).unwrap();
        let c_request = CString::new(r#"{"op":"list_accounts"}"#).unwrap();
        let handle = unsafe { mail_service_open(c_path.as_ptr()) };
        assert!(!handle.is_null());
        let raw = unsafe { mail_service_request_json(handle, c_request.as_ptr()) };
        let envelope: Value =
            serde_json::from_str(unsafe { CStr::from_ptr(raw) }.to_str().unwrap()).unwrap();
        assert_eq!(envelope["ok"], true);
        assert!(envelope["data"].as_array().unwrap().len() > 0);
        unsafe {
            mail_service_free_string(raw);
            mail_service_close(handle);
        }
        drop(service);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn recipient_suggestions_require_account_and_return_safe_local_mailboxes() {
        let dir =
            std::env::temp_dir().join(format!("email-recipient-suggestions-{}", Uuid::new_v4()));
        let service = MailService::open(dir.join("mail.sqlite")).unwrap();
        service.request_json(r#"{"op":"seed_demo"}"#).unwrap();
        service.request_json(r#"{"op":"compose_draft","account_id":"demo:personal","to":"Jane Doe <jane@example.org>","subject":"Draft","body_plain":"Body"}"#).unwrap();
        let suggestions = service
            .request_json(
                r#"{"op":"suggest_recipients","account_id":"demo:personal","query":"ja"}"#,
            )
            .unwrap();
        assert_eq!(suggestions[0]["email"], "jane@example.org");
        assert_eq!(suggestions[0]["display_name"], "Jane Doe");
        assert!(
            suggestions[0]["formatted"]
                .as_str()
                .unwrap()
                .contains("<jane@example.org>")
        );
        assert!(
            service
                .request_json(r#"{"op":"suggest_recipients","query":"ja"}"#)
                .is_err()
        );
        assert!(
            service
                .request_json(r#"{"op":"suggest_recipients","account_id":"missing","query":"ja"}"#)
                .is_err()
        );
        assert!(
            service
                .request_json(
                    r#"{"op":"suggest_recipients","account_id":"demo:personal","query":""}"#
                )
                .unwrap()
                .as_array()
                .unwrap()
                .is_empty()
        );
        drop(service);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn removed_account_process_caches_clear_without_touching_other_accounts_or_vault() {
        let dir = std::env::temp_dir().join(format!("email-removed-cache-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let service = MailService::open(dir.join("mail.sqlite")).unwrap();
        for account in ["removed-private-account", "kept-account"] {
            for id in [account.to_owned(), MailService::calendar_vault_id(account)] {
                service.token_cache.lock().unwrap().insert(
                    id,
                    CachedTokens {revision:None,generation:"synthetic-generation".into(),tokens:TokenSet {
                        access_token: format!("synthetic-token-{account}"),
                        refresh_token: Some("synthetic-refresh".into()),
                        expires_at_ms: 0,
                    }},
                );
            }
            service
                .warmed_google_photo_accounts
                .lock()
                .unwrap()
                .insert(account.into(),"synthetic-generation".into());
        }
        service
            .clear_removed_account_caches("removed-private-account")
            .unwrap();
        let cache = service.token_cache.lock().unwrap();
        assert!(!cache.contains_key("removed-private-account"));
        assert!(!cache.contains_key(&MailService::calendar_vault_id("removed-private-account")));
        assert!(cache.contains_key("kept-account"));
        assert!(cache.contains_key(&MailService::calendar_vault_id("kept-account")));
        drop(cache);
        let warmed = service.warmed_google_photo_accounts.lock().unwrap();
        assert!(!warmed.contains_key("removed-private-account"));
        assert!(warmed.contains_key("kept-account"));
        drop(warmed);
        drop(service);
        fs::remove_dir_all(dir).unwrap();
    }

}
