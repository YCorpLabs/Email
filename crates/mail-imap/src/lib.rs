//! IMAP and SMTP adapter for the Email engine: iCloud Mail and standard servers.
//!
//! [`ImapConnector`] implements `mail_core::MailAdapter` over TLS. It pages new
//! and older INBOX messages, parses MIME into the normalized `mail_core::Message`
//! model, sends through authenticated SMTP and applies read, archive and move
//! actions when the server supports them safely (UID MOVE, never a
//! copy-and-expunge fallback). [`ImapConfig::icloud`] fills in Apple's server
//! settings; other providers supply their own hosts and ports.
//!
//! Secrets are passed in at runtime through [`ImapConfig`] and are never
//! persisted by this crate; the caller owns credential storage. All network
//! methods block, so call them from a worker thread. Message bodies are
//! returned exactly as received: sanitize HTML before rendering it.
//! [`attachment_metadata`] and [`attachment_bytes`] read attachments from a raw
//! message without keeping them in the cache.
#[cfg(feature = "test-fixture")]
pub mod fixture;
mod setup;
pub use setup::{ServerHint, SetupError, SetupStage};

use imap::types::{Flag, NameAttribute};
use lettre::{
    Message as OutgoingMessage, SmtpTransport, Transport,
    message::{Attachment, Mailbox, MultiPart, SinglePart, header::ContentType},
    transport::smtp::{
        authentication::Credentials,
        client::{Tls, TlsParameters},
    },
};
use mail_core::{
    Account, MailAction, MailAdapter, MailError, MailStore, Message, MessageState,
    OutgoingAttachment, Provider, SyncBatch,
};
use mailparse::{MailHeaderMap, ParsedMail, body::Body};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{fmt, net::TcpStream};

const MAX_RAW_MESSAGE_BYTES: usize = 40 * 1024 * 1024;
const MAX_SYNC_BATCH_BYTES: usize = 80 * 1024 * 1024;
const MAX_ATTACHMENT_BYTES: usize = 20 * 1024 * 1024;

/// Gmail's IMAP and SMTP endpoints for app-password accounts.
/// <https://developers.google.com/workspace/gmail/imap/imap-smtp>
pub const GMAIL_IMAP_HOST: &str = "imap.gmail.com";
pub const GMAIL_IMAP_PORT: u16 = 993;
pub const GMAIL_SMTP_HOST: &str = "smtp.gmail.com";
pub const GMAIL_SMTP_PORT: u16 = 465;
/// Gmail advertises this IMAP extension; it marks All Mail with `\All`.
const GMAIL_CAPABILITY: &str = "X-GM-EXT-1";

/// What account setup learned from the server.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SetupReport {
    /// Gmail's All Mail mailbox (special-use `\All`, RFC 6154). Its name is
    /// localized for some accounts, for example `[Google Mail]/All Mail`.
    /// Archiving moves a message there, which removes only the Inbox label.
    pub gmail_all_mail: Option<String>,
}

type ImapSession = imap::Session<native_tls::TlsStream<TcpStream>>;

/// The mailbox the server marks with a special-use attribute such as `\Junk`.
fn special_use(session: &mut ImapSession, attribute: &str) -> Result<Option<String>, ImapError> {
    Ok(special_use_in(&mailboxes(session)?, attribute))
}

/// Every mailbox name with its attributes, from one LIST.
fn mailboxes(session: &mut ImapSession) -> Result<Vec<(String, Vec<String>)>, ImapError> {
    Ok(session
        .list(None, Some("*"))?
        .iter()
        .map(|mailbox| {
            let attributes = mailbox
                .attributes()
                .iter()
                .filter_map(|value| match value {
                    NameAttribute::Custom(value) => Some(value.to_string()),
                    _ => None,
                })
                .collect();
            (mailbox.name().to_string(), attributes)
        })
        .collect())
}

/// The mailbox an address reaches, for telling whether two addresses are the
/// same account: case is ignored and, for gmail.com and googlemail.com, so are
/// dots and `+tags`, as Gmail does.
pub fn mailbox_identity(address: &str) -> String {
    let address = address.trim().to_ascii_lowercase();
    match address.rsplit_once('@') {
        Some((local, "gmail.com" | "googlemail.com")) => format!(
            "{}@gmail.com",
            local.split('+').next().unwrap_or(local).replace('.', "")
        ),
        _ => address,
    }
}

fn special_use_in(mailboxes: &[(String, Vec<String>)], attribute: &str) -> Option<String> {
    mailboxes
        .iter()
        .find(|(_, attributes)| {
            attributes
                .iter()
                .any(|value| value.eq_ignore_ascii_case(attribute))
        })
        .map(|(name, _)| name.clone())
}

const ALL_MAIL_HIDDEN: &str = "Gmail's All Mail isn't visible to IMAP. In Gmail settings › Labels, turn on “Show in IMAP” for All Mail, then try again.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportSecurity {
    ImplicitTls,
    StartTls,
}

/// Runtime-only account configuration. The secret is an app password or mailbox password;
/// callers should obtain it from the OS credential store and discard it after use.
#[derive(Clone)]
pub struct ImapConfig {
    pub account_id: String,
    pub imap_host: String,
    pub imap_port: u16,
    pub imap_security: TransportSecurity,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_security: TransportSecurity,
    pub username: String,
    pub smtp_username: String,
    pub secret: String,
    pub archive_mailbox: Option<String>,
}
impl fmt::Debug for ImapConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ImapConfig")
            .field("account_id", &self.account_id)
            .field("imap_host", &self.imap_host)
            .field("imap_port", &self.imap_port)
            .field("imap_security", &self.imap_security)
            .field("smtp_host", &self.smtp_host)
            .field("smtp_port", &self.smtp_port)
            .field("smtp_security", &self.smtp_security)
            .field("username", &self.username)
            .field("smtp_username", &self.smtp_username)
            .field("secret", &"[redacted]")
            .field("archive_mailbox", &self.archive_mailbox)
            .finish()
    }
}
impl ImapConfig {
    pub fn icloud(
        account_id: impl Into<String>,
        address: impl Into<String>,
        app_password: impl Into<String>,
    ) -> Self {
        let address = address.into();
        let username = address.split('@').next().unwrap_or(&address).to_string();
        Self {
            account_id: account_id.into(),
            imap_host: "imap.mail.me.com".into(),
            imap_port: 993,
            imap_security: TransportSecurity::ImplicitTls,
            smtp_host: "smtp.mail.me.com".into(),
            smtp_port: 587,
            smtp_security: TransportSecurity::StartTls,
            username,
            smtp_username: address,
            secret: app_password.into(),
            archive_mailbox: Some("Archive".into()),
        }
    }
    /// Gmail through the generic IMAP path with a Google app password. Google
    /// shows app passwords in groups of four; whitespace is not part of them.
    /// The archive mailbox is Gmail's All Mail, found during setup
    /// (`verify_account`) and again whenever archive runs.
    pub fn gmail(
        account_id: impl Into<String>,
        address: impl Into<String>,
        app_password: impl Into<String>,
    ) -> Self {
        let address = address.into();
        let secret: String = app_password
            .into()
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        Self {
            account_id: account_id.into(),
            imap_host: GMAIL_IMAP_HOST.into(),
            imap_port: GMAIL_IMAP_PORT,
            imap_security: TransportSecurity::ImplicitTls,
            smtp_host: GMAIL_SMTP_HOST.into(),
            smtp_port: GMAIL_SMTP_PORT,
            smtp_security: TransportSecurity::ImplicitTls,
            username: address.clone(),
            smtp_username: address,
            secret,
            archive_mailbox: None,
        }
    }
    fn validate(&self) -> Result<(), ImapError> {
        if self.account_id.is_empty()
            || self.imap_host.is_empty()
            || self.smtp_host.is_empty()
            || self.username.is_empty()
            || self.smtp_username.is_empty()
            || self.secret.is_empty()
            || self.imap_port == 0
            || self.smtp_port == 0
        {
            return Err(ImapError::Configuration(
                "account, hosts, ports, username, and secret are required".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ImapError {
    #[error("invalid account configuration: {0}")]
    Configuration(String),
    #[error("IMAP error: {0}")]
    Imap(#[from] imap::Error),
    #[error("TLS error: {0}")]
    Tls(#[from] native_tls::Error),
    #[error("mail parse error: {0}")]
    Parse(#[from] mailparse::MailParseError),
    #[error("mail core error: {0}")]
    Core(#[from] MailError),
    #[error("SMTP message error: {0}")]
    Message(#[from] lettre::error::Error),
    #[error("SMTP transport error: {0}")]
    Smtp(#[from] lettre::transport::smtp::Error),
    #[error("unsupported action: {0}")]
    Unsupported(String),
    #[error("invalid provider identifier: {0}")]
    Identifier(String),
    #[error("invalid outgoing attachment: {0}")]
    InvalidAttachment(&'static str),
}

#[derive(Debug, Clone)]
pub struct ImapConnector {
    config: ImapConfig,
    provider: Provider,
    batch_size: usize,
    /// Bytes of message bodies one sync may download.
    sync_budget: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct Cursor {
    uid_validity: u32,
    high_uid: u32,
    backfill_before: Option<u32>,
}

impl ImapConnector {
    pub fn new(config: ImapConfig, provider: Provider) -> Result<Self, ImapError> {
        config.validate()?;
        if !matches!(provider, Provider::Icloud | Provider::Imap) {
            return Err(ImapError::Configuration(
                "IMAP connector supports iCloud and generic IMAP accounts".into(),
            ));
        }
        Ok(Self {
            config,
            provider,
            batch_size: 150,
            sync_budget: MAX_SYNC_BATCH_BYTES,
        })
    }
    pub fn with_batch_size(mut self, size: usize) -> Self {
        self.batch_size = size.clamp(1, 1000);
        self
    }
    /// Lower the per-sync download budget, for example in tests. The first
    /// download of a sync is always allowed, so every sync makes progress.
    pub fn with_sync_budget(mut self, bytes: usize) -> Self {
        self.sync_budget = bytes.clamp(1, MAX_SYNC_BATCH_BYTES);
        self
    }
    pub fn config(&self) -> &ImapConfig {
        &self.config
    }

    /// Authenticate, select INBOX and check SMTP before an account is persisted.
    /// Blocking I/O. Records which step failed so account setup can explain it
    /// (see `SetupError::message`). One IMAP session: on Gmail it also finds All
    /// Mail, so setup never logs in twice. All Mail is optional here.
    pub fn verify_account(&self) -> Result<SetupReport, SetupError> {
        let client = self
            .connect()
            .map_err(SetupError::at(SetupStage::Connect))?;
        let mut session = client
            .login(&self.config.username, &self.config.secret)
            .map_err(|(error, _)| SetupError::new(SetupStage::Login, ImapError::Imap(error)))?;
        let mailbox = |error: ImapError| SetupError::new(SetupStage::Mailbox, error);
        session
            .select("INBOX")
            .map_err(|error| mailbox(error.into()))?;
        // All Mail is optional at setup (archive looks it up later), so a
        // failed CAPABILITY or LIST does not fail account setup.
        let gmail = session
            .capabilities()
            .is_ok_and(|capabilities| capabilities.has_str(GMAIL_CAPABILITY));
        let gmail_all_mail = if gmail {
            special_use(&mut session, "\\All").ok().flatten()
        } else {
            None
        };
        let _ = session.logout();
        let smtp = self
            .smtp_transport()
            .map_err(SetupError::at(SetupStage::Smtp))?;
        match smtp.test_connection() {
            Ok(true) => Ok(SetupReport { gmail_all_mail }),
            Ok(false) => Err(SetupError::new(
                SetupStage::Smtp,
                ImapError::Configuration("SMTP server did not respond to NOOP".into()),
            )),
            Err(error) => Err(SetupError::new(SetupStage::Smtp, error.into())),
        }
    }

    /// Fetch one cached INBOX message again for attachment access without marking it read.
    pub fn fetch_raw_message(
        &self,
        account: &Account,
        message_id: &str,
    ) -> Result<Vec<u8>, ImapError> {
        if account.id != self.config.account_id || account.provider != self.provider {
            return Err(ImapError::Configuration(
                "account does not match connector".into(),
            ));
        }
        let prefix = format!("{}:imap:", account.id);
        let remote = message_id
            .strip_prefix(&prefix)
            .ok_or_else(|| ImapError::Identifier("message ID is not from this account".into()))?;
        let (validity, uid) = remote
            .split_once(':')
            .ok_or_else(|| ImapError::Identifier("invalid IMAP message ID".into()))?;
        let validity: u32 = validity
            .parse()
            .map_err(|_| ImapError::Identifier("invalid UIDVALIDITY".into()))?;
        let uid: u32 = uid
            .parse()
            .map_err(|_| ImapError::Identifier("invalid UID".into()))?;
        let mut session = self.login()?;
        let selected = session.select("INBOX")?;
        if selected.uid_validity != Some(validity) {
            return Err(ImapError::Identifier(
                "message UIDVALIDITY has changed".into(),
            ));
        }
        let metadata = session.uid_fetch(uid.to_string(), "UID RFC822.SIZE")?;
        let size = metadata
            .iter()
            .find(|item| item.uid == Some(uid))
            .and_then(|item| item.size);
        if !size.is_some_and(|size| size as usize <= MAX_RAW_MESSAGE_BYTES) {
            return Err(ImapError::Identifier(
                "message exceeds the attachment access limit".into(),
            ));
        }
        let fetched =
            session.uid_fetch(uid.to_string(), &bounded_body_query(MAX_RAW_MESSAGE_BYTES))?;
        let bytes = fetched
            .iter()
            .find(|item| item.uid == Some(uid))
            .and_then(|item| item.body())
            .filter(|raw| {
                raw.len() == size.unwrap_or(0) as usize && raw.len() <= MAX_RAW_MESSAGE_BYTES
            })
            .map(|raw| raw.to_vec())
            .ok_or_else(|| ImapError::Identifier("message is no longer in INBOX".into()))?;
        let _ = session.logout();
        Ok(bytes)
    }

    fn smtp_transport(&self) -> Result<SmtpTransport, ImapError> {
        let credentials = Credentials::new(
            self.config.smtp_username.clone(),
            self.config.secret.clone(),
        );
        let (host, port) = endpoint(&self.config.smtp_host, self.config.smtp_port);
        #[allow(unused_mut)]
        let mut parameters = TlsParameters::builder(host.clone());
        #[cfg(feature = "test-fixture")]
        for der in fixture::trusted_roots() {
            parameters = parameters
                .add_root_certificate(lettre::transport::smtp::client::Certificate::from_der(der)?);
        }
        let parameters = parameters.build()?;
        let tls = match self.config.smtp_security {
            TransportSecurity::ImplicitTls => Tls::Wrapper(parameters),
            TransportSecurity::StartTls => Tls::Required(parameters),
        };
        Ok(SmtpTransport::builder_dangerous(host)
            .port(port)
            .tls(tls)
            .credentials(credentials)
            .build())
    }

    fn connect(&self) -> Result<imap::Client<native_tls::TlsStream<TcpStream>>, ImapError> {
        #[allow(unused_mut)]
        let mut builder = native_tls::TlsConnector::builder();
        #[cfg(feature = "test-fixture")]
        for der in fixture::trusted_roots() {
            builder.add_root_certificate(native_tls::Certificate::from_der(&der)?);
        }
        let tls = builder.build()?;
        let (host, port) = endpoint(&self.config.imap_host, self.config.imap_port);
        let address = (host.as_str(), port);
        Ok(match self.config.imap_security {
            TransportSecurity::ImplicitTls => imap::connect(address, &host, &tls)?,
            TransportSecurity::StartTls => imap::connect_starttls(address, &host, &tls)?,
        })
    }

    fn login(&self) -> Result<ImapSession, ImapError> {
        self.connect()?
            .login(&self.config.username, &self.config.secret)
            .map_err(|(error, _)| ImapError::Imap(error))
    }

    /// `cached` holds message IDs already in the local store. Their bodies never
    /// change on the server, so the periodic refresh of recent mail reads only
    /// their flags. Re-downloading them every minute would approach Gmail's
    /// daily IMAP download limit for busy inboxes.
    ///
    /// Mail that does not fit this sync's download budget is deferred to the
    /// next sync, which then knows what this one stored. Without that index
    /// (`None`), deferring could repeat the same downloads forever, so such
    /// mail is stored as a placeholder instead.
    fn sync_impl(
        &self,
        account: &Account,
        cursor: Option<&str>,
        cached: Option<&std::collections::HashMap<String, String>>,
    ) -> Result<SyncBatch, ImapError> {
        if account.id != self.config.account_id || account.provider != self.provider {
            return Err(ImapError::Configuration(
                "account does not match connector".into(),
            ));
        }
        let mut session = self.login()?;
        let selected = session.select("INBOX")?;
        let validity = selected
            .uid_validity
            .ok_or_else(|| ImapError::Configuration("server did not return UIDVALIDITY".into()))?;
        let old = cursor
            .and_then(|c| serde_json::from_str::<Cursor>(c).ok())
            .filter(|c| c.uid_validity == validity);
        let mut all: Vec<u32> = session.uid_search("ALL")?.into_iter().collect();
        all.sort_unstable();
        let (selected, next) = select_uids(&all, old, validity, self.batch_size);
        let message_id = |uid: u32| format!("{}:imap:{validity}:{uid}", account.id);
        // IMAP messages never change, so anything already cached (including a
        // permanent placeholder) only needs its flags.
        let (known, wanted): (Vec<u32>, Vec<u32>) = selected
            .into_iter()
            .partition(|uid| cached.is_some_and(|ids| ids.contains_key(&message_id(*uid))));
        let mut message_states = Vec::with_capacity(known.len());
        for chunk in known.chunks(100) {
            let set = chunk
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(",");
            for fetched in session.uid_fetch(&set, "UID FLAGS")?.iter() {
                let Some(uid) = fetched.uid else { continue };
                message_states.push(MessageState {
                    id: message_id(uid),
                    is_read: fetched
                        .flags()
                        .iter()
                        .any(|flag| matches!(flag, Flag::Seen)),
                    is_draft: false,
                    is_sent: false,
                });
            }
        }
        let mut messages = Vec::with_capacity(wanted.len());
        let mut batch_bytes = 0usize;
        // Messages that did not fit this sync's download budget. They are not
        // stored as placeholders: the cursor stays put and the next sync, with a
        // fresh budget, downloads them (already cached ones cost only flags).
        let mut deferred = false;
        // Fetch sizes first, then fetch each bounded body separately. A single FETCH
        // response must never retain many large message bodies at once.
        for chunk in wanted.chunks(25) {
            let set = chunk
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(",");
            for fetched in session
                .uid_fetch(&set, "UID FLAGS INTERNALDATE RFC822.SIZE")?
                .iter()
            {
                let Some(uid) = fetched.uid else { continue };
                let read = fetched
                    .flags()
                    .iter()
                    .any(|flag| matches!(flag, Flag::Seen));
                let internal_ms = fetched.internal_date().map(|d| d.timestamp_millis());
                let remaining = self.sync_budget.saturating_sub(batch_bytes);
                let Some(size) = fetched.size.map(|size| size as usize) else {
                    messages.push(unavailable_message(
                        account,
                        validity,
                        uid,
                        read,
                        internal_ms,
                        "Message size unavailable",
                    ));
                    continue;
                };
                if size > MAX_RAW_MESSAGE_BYTES {
                    messages.push(unavailable_message(
                        account,
                        validity,
                        uid,
                        read,
                        internal_ms,
                        "Message exceeds local sync limit",
                    ));
                    continue;
                }
                // The first download of a sync is always allowed, so every sync
                // makes progress whatever the budget.
                let allowance = if batch_bytes == 0 {
                    MAX_RAW_MESSAGE_BYTES
                } else {
                    remaining.min(MAX_RAW_MESSAGE_BYTES)
                };
                if size > allowance {
                    if cached.is_some() {
                        deferred = true;
                    } else {
                        messages.push(unavailable_message(
                            account,
                            validity,
                            uid,
                            read,
                            internal_ms,
                            "Message exceeds local sync limit",
                        ));
                    }
                    continue;
                }
                let body = session.uid_fetch(uid.to_string(), bounded_body_query(allowance))?;
                let Some(raw) = body
                    .iter()
                    .find(|item| item.uid == Some(uid))
                    .and_then(|item| item.body())
                else {
                    messages.push(unavailable_message(
                        account,
                        validity,
                        uid,
                        read,
                        internal_ms,
                        "Message body unavailable",
                    ));
                    continue;
                };
                if raw.len() != size || raw.len() > allowance {
                    messages.push(unavailable_message(
                        account,
                        validity,
                        uid,
                        read,
                        internal_ms,
                        "Message exceeds local sync limit",
                    ));
                    continue;
                }
                batch_bytes += raw.len();
                messages.push(parse_or_placeholder(
                    raw,
                    account,
                    validity,
                    uid,
                    read,
                    internal_ms,
                ));
            }
        }
        let _ = session.logout();
        let next_cursor = if deferred {
            serde_json::to_string(&old)
        } else {
            serde_json::to_string(&next)
        }
        .map_err(|e| ImapError::Configuration(e.to_string()))?;
        Ok(SyncBatch {
            message_states,
            messages,
            thread_labels: Vec::new(),
            removed_message_ids: Vec::new(),
            full_reset: false,
            reset_phase: None,
            next_cursor,
        })
    }

    pub fn send(
        &self,
        from: &str,
        to: &[String],
        subject: &str,
        body_plain: &str,
        body_html: Option<&str>,
    ) -> Result<(), ImapError> {
        self.send_with_attachments(from, to, subject, body_plain, body_html, &[])
    }

    pub fn send_with_attachments(
        &self,
        from: &str,
        to: &[String],
        subject: &str,
        body_plain: &str,
        body_html: Option<&str>,
        attachments: &[OutgoingAttachment],
    ) -> Result<(), ImapError> {
        self.send_with_recipients(
            from,
            to,
            &[],
            &[],
            subject,
            body_plain,
            body_html,
            attachments,
        )
    }

    pub fn send_with_recipients(
        &self,
        from: &str,
        to: &[String],
        cc: &[String],
        bcc: &[String],
        subject: &str,
        body_plain: &str,
        body_html: Option<&str>,
        attachments: &[OutgoingAttachment],
    ) -> Result<(), ImapError> {
        let message = build_outgoing_recipients(
            from,
            to,
            cc,
            bcc,
            subject,
            body_plain,
            body_html,
            attachments,
        )?;
        self.smtp_transport()?.send(&message)?;
        Ok(())
    }

    pub fn apply_action(&self, store: &MailStore, action: &MailAction) -> Result<(), ImapError> {
        let thread_id = match action {
            MailAction::Archive { thread_id }
            | MailAction::MarkRead { thread_id, .. }
            | MailAction::Move { thread_id, .. } => thread_id,
            _ => {
                return Err(ImapError::Unsupported(
                    "only archive, move, and mark-read are supported by generic IMAP".into(),
                ));
            }
        };
        let mut session = self.login()?;
        let selected = session.select("INBOX")?;
        let validity = selected
            .uid_validity
            .ok_or_else(|| ImapError::Configuration("server did not return UIDVALIDITY".into()))?;
        let messages = store.list_thread_messages(thread_id)?;
        if messages.is_empty() {
            return Err(ImapError::Identifier("thread has no local messages".into()));
        }
        let uids = messages
            .iter()
            .map(|m| parse_uid(&m.id, &self.config.account_id, validity))
            .collect::<Result<Vec<_>, _>>()?;
        let uid_set = uids
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(",");
        match action {
            MailAction::MarkRead { read, .. } => {
                let op = if *read {
                    "+FLAGS.SILENT (\\Seen)"
                } else {
                    "-FLAGS.SILENT (\\Seen)"
                };
                session.uid_store(&uid_set, op)?;
            }
            MailAction::Archive { .. } | MailAction::Move { .. } => {
                let (has_move, gmail) = {
                    let capabilities = session.capabilities()?;
                    (
                        capabilities.has_str("MOVE"),
                        capabilities.has_str(GMAIL_CAPABILITY),
                    )
                };
                if !has_move {
                    return Err(ImapError::Unsupported("IMAP server does not advertise MOVE; archive is unavailable without risking unrelated deletions".into()));
                }
                let move_to =
                    |session: &mut ImapSession, destination: &str| -> Result<(), ImapError> {
                        let quoted = imap_quote(destination)?;
                        session.run_command_and_check_ok(format!("UID MOVE {uid_set} {quoted}"))?;
                        Ok(())
                    };
                if matches!(action, MailAction::Archive { .. }) && gmail {
                    // Gmail's All Mail name follows the account's display
                    // language, so it can change after setup: try the saved
                    // name, and look it up again only if Gmail rejects it.
                    let saved = self.config.archive_mailbox.clone();
                    let first = match &saved {
                        Some(name) => move_to(&mut session, name),
                        None => Err(ImapError::Unsupported(ALL_MAIL_HIDDEN.into())),
                    };
                    match first {
                        Ok(()) => {}
                        Err(
                            failure @ (ImapError::Imap(imap::Error::No(_))
                            | ImapError::Unsupported(_)),
                        ) => {
                            match special_use(&mut session, "\\All")? {
                                Some(current) if Some(&current) != saved.as_ref() => {
                                    move_to(&mut session, &current)?
                                }
                                // Same name: Gmail refused for another reason (for
                                // example throttling); report that, and retry later.
                                Some(_) => return Err(failure),
                                None => return Err(ImapError::Unsupported(ALL_MAIL_HIDDEN.into())),
                            }
                        }
                        Err(error) => return Err(error),
                    }
                    let _ = session.logout();
                    return Ok(());
                }
                let destination = match action {
                    MailAction::Archive { .. } => {
                        self.config.archive_mailbox.clone().ok_or_else(|| {
                            ImapError::Configuration("archive mailbox is not configured".into())
                        })?
                    }
                    MailAction::Move { folder, .. }
                        if folder.eq_ignore_ascii_case("spam")
                            || folder.eq_ignore_ascii_case("junk") =>
                    {
                        let names = mailboxes(&mut session)?;
                        special_use_in(&names, "\\Junk")
                            .or_else(|| {
                                names
                                    .iter()
                                    .map(|(name, _)| name)
                                    .find(|name| {
                                        let leaf = name.rsplit(['/', '.']).next().unwrap_or(name);
                                        leaf.eq_ignore_ascii_case("junk")
                                            || leaf.eq_ignore_ascii_case("spam")
                                    })
                                    .cloned()
                            })
                            .ok_or_else(|| {
                                ImapError::Unsupported(
                                    "this account does not advertise a Junk or Spam mailbox".into(),
                                )
                            })?
                    }
                    MailAction::Move { folder, .. } => folder.clone(),
                    _ => unreachable!(),
                };
                move_to(&mut session, &destination)?;
            }
            _ => unreachable!(),
        }
        let _ = session.logout();
        Ok(())
    }
}

/// The host and port to dial. Fixture builds can redirect a configured server
/// (for example `imap.gmail.com:993`) to a local synthetic server; ordinary
/// builds always use the configured values.
fn endpoint(host: &str, port: u16) -> (String, u16) {
    #[cfg(feature = "test-fixture")]
    if let Some(redirect) = fixture::redirect(host, port) {
        return redirect;
    }
    (host.to_string(), port)
}

fn bounded_body_query(limit: usize) -> String {
    format!("UID BODY.PEEK[]<0.{}>", limit.saturating_add(1))
}

fn parse_or_placeholder(
    raw: &[u8],
    account: &Account,
    validity: u32,
    uid: u32,
    read: bool,
    internal_ms: Option<i64>,
) -> Message {
    parse_message(raw, account, validity, uid, read, internal_ms).unwrap_or_else(|_| {
        unavailable_message(
            account,
            validity,
            uid,
            read,
            internal_ms,
            "Message MIME could not be decoded",
        )
    })
}

fn unavailable_message(
    account: &Account,
    validity: u32,
    uid: u32,
    read: bool,
    internal_ms: Option<i64>,
    reason: &str,
) -> Message {
    let id = format!("{}:imap:{validity}:{uid}", account.id);
    Message {
        recipient_headers: None,
        id: id.clone(),
        thread_id: id,
        account_id: account.id.clone(),
        sender: "(unavailable message)".into(),
        to: vec![account.email.clone()],
        subject: reason.into(),
        body_plain: "Open this message in your mail provider to view its contents.".into(),
        body_html: None,
        unsubscribe_header: None,
        has_attachments: Some(false),
        received_at: internal_ms.unwrap_or(0),
        is_read: read,
        is_draft: false,
        is_sent: false,
    }
}

#[cfg(test)]
fn build_outgoing_with_attachments(
    from: &str,
    to: &[String],
    subject: &str,
    body_plain: &str,
    body_html: Option<&str>,
    attachments: &[OutgoingAttachment],
) -> Result<OutgoingMessage, ImapError> {
    build_outgoing_recipients(
        from,
        to,
        &[],
        &[],
        subject,
        body_plain,
        body_html,
        attachments,
    )
}

fn build_outgoing_recipients(
    from: &str,
    to: &[String],
    cc: &[String],
    bcc: &[String],
    subject: &str,
    body_plain: &str,
    body_html: Option<&str>,
    attachments: &[OutgoingAttachment],
) -> Result<OutgoingMessage, ImapError> {
    validate_outgoing_attachments(attachments)?;
    if to.is_empty() && cc.is_empty() && bcc.is_empty() {
        return Err(ImapError::Configuration(
            "at least one recipient is required".into(),
        ));
    }
    let mut builder = OutgoingMessage::builder()
        .from(
            from.parse::<Mailbox>()
                .map_err(|e| ImapError::Configuration(e.to_string()))?,
        )
        .subject(subject);
    for (kind, recipients) in [("to", to), ("cc", cc), ("bcc", bcc)] {
        for recipient in recipients {
            let mailbox = recipient
                .parse::<Mailbox>()
                .map_err(|e| ImapError::Configuration(e.to_string()))?;
            builder = match kind {
                "to" => builder.to(mailbox),
                "cc" => builder.cc(mailbox),
                _ => builder.bcc(mailbox),
            };
        }
    }
    // Lettre derives the envelope from all three lists, then removes Bcc from
    // formatted SMTP content by default. Never call keep_bcc() on this path.
    if attachments.is_empty() {
        return Ok(match body_html {
            Some(html) => builder.multipart(
                MultiPart::alternative()
                    .singlepart(SinglePart::plain(body_plain.to_owned()))
                    .singlepart(SinglePart::html(html.to_owned())),
            )?,
            None => builder.body(body_plain.to_owned())?,
        });
    }
    let mut mixed = match body_html {
        Some(html) => MultiPart::mixed().multipart(
            MultiPart::alternative()
                .singlepart(SinglePart::plain(body_plain.to_owned()))
                .singlepart(SinglePart::html(html.to_owned())),
        ),
        None => MultiPart::mixed().singlepart(SinglePart::plain(body_plain.to_owned())),
    };
    for attachment in attachments {
        let mime = if attachment.mime_type.is_empty() {
            "application/octet-stream"
        } else {
            &attachment.mime_type
        };
        let content_type = ContentType::parse(mime)
            .map_err(|_| ImapError::InvalidAttachment("invalid MIME type"))?;
        mixed = mixed.singlepart(
            Attachment::new(attachment.filename.clone())
                .body(attachment.bytes.clone(), content_type),
        );
    }
    Ok(builder.multipart(mixed)?)
}

fn validate_outgoing_attachments(items: &[OutgoingAttachment]) -> Result<(), ImapError> {
    const MAX_FILE: usize = 20 * 1024 * 1024;
    const MAX_TOTAL: usize = 25 * 1024 * 1024;
    if items.len() > 10 {
        return Err(ImapError::InvalidAttachment("too many files"));
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
            return Err(ImapError::InvalidAttachment("invalid filename"));
        }
        if item.bytes.len() > MAX_FILE {
            return Err(ImapError::InvalidAttachment("file exceeds 20 MiB"));
        }
        total = total.saturating_add(item.bytes.len());
        if total > MAX_TOTAL {
            return Err(ImapError::InvalidAttachment("attachments exceed 25 MiB"));
        }
        let mime = if item.mime_type.is_empty() {
            "application/octet-stream"
        } else {
            &item.mime_type
        };
        let Some((kind, subtype)) = mime.split_once('/') else {
            return Err(ImapError::InvalidAttachment("invalid MIME type"));
        };
        if kind.is_empty()
            || subtype.is_empty()
            || mime.len() > 127
            || !mime
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'+' | b'.' | b'/'))
        {
            return Err(ImapError::InvalidAttachment("invalid MIME type"));
        }
    }
    Ok(())
}

impl MailAdapter for ImapConnector {
    fn provider(&self) -> Provider {
        self.provider.clone()
    }
    fn sync(&self, account: &Account, cursor: Option<&str>) -> mail_core::Result<SyncBatch> {
        self.sync_impl(account, cursor, None)
            .map_err(|e| MailError::Provider(e.to_string()))
    }
    fn sync_with_cached_messages(
        &self,
        account: &Account,
        cursor: Option<&str>,
        cached: &std::collections::HashMap<String, String>,
    ) -> mail_core::Result<SyncBatch> {
        self.sync_impl(account, cursor, Some(cached))
            .map_err(|e| MailError::Provider(e.to_string()))
    }
}

fn select_uids(
    all: &[u32],
    old: Option<Cursor>,
    validity: u32,
    batch_size: usize,
) -> (Vec<u32>, Cursor) {
    if all.is_empty() {
        return (
            Vec::new(),
            Cursor {
                uid_validity: validity,
                high_uid: old.map_or(0, |c| c.high_uid),
                backfill_before: None,
            },
        );
    }
    if let Some(old) = old {
        let new = all
            .iter()
            .copied()
            .filter(|uid| *uid > old.high_uid)
            .take(batch_size)
            .collect::<Vec<_>>();
        let high_uid = new.last().copied().unwrap_or(old.high_uid);
        let remaining = batch_size.saturating_sub(new.len());
        let older = old
            .backfill_before
            .map(|before| {
                all.iter()
                    .copied()
                    .filter(|uid| *uid < before)
                    .rev()
                    .take(remaining)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let next_before = if older.is_empty() {
            old.backfill_before
                .filter(|before| all.iter().any(|uid| *uid < *before))
        } else {
            older
                .last()
                .copied()
                .filter(|min| all.iter().any(|uid| *uid < *min))
        };
        let mut wanted = new;
        wanted.extend(older);
        // Refresh a few recent messages for read/unread changes made in other clients.
        let refresh_budget = batch_size.min(25);
        wanted.extend(all.iter().copied().rev().take(refresh_budget));
        wanted.sort_unstable();
        wanted.dedup();
        return (
            wanted,
            Cursor {
                uid_validity: validity,
                high_uid,
                backfill_before: next_before,
            },
        );
    }
    let newest = all
        .iter()
        .copied()
        .rev()
        .take(batch_size)
        .collect::<Vec<_>>();
    let min = newest.last().copied().unwrap_or(0);
    let backfill_before = all.iter().any(|uid| *uid < min).then_some(min);
    let mut wanted = newest;
    wanted.sort_unstable();
    (
        wanted,
        Cursor {
            uid_validity: validity,
            high_uid: *all.last().unwrap_or(&0),
            backfill_before,
        },
    )
}

fn parse_uid(id: &str, account_id: &str, validity: u32) -> Result<u32, ImapError> {
    let prefix = format!("{account_id}:imap:{validity}:");
    id.strip_prefix(&prefix)
        .and_then(|s| s.parse::<u32>().ok())
        .ok_or_else(|| ImapError::Identifier(id.into()))
}
fn imap_quote(s: &str) -> Result<String, ImapError> {
    if s.is_empty() || s.contains(['\r', '\n', '\0']) {
        return Err(ImapError::Configuration("invalid IMAP mailbox name".into()));
    }
    Ok(format!(
        "\"{}\"",
        s.replace('\\', "\\\\").replace('"', "\\\"")
    ))
}

pub fn parse_message(
    raw: &[u8],
    account: &Account,
    uid_validity: u32,
    uid: u32,
    is_read: bool,
    internal_ms: Option<i64>,
) -> Result<Message, ImapError> {
    let parsed = mailparse::parse_mail(raw)?;
    let sender = parsed.headers.get_first_value("From").unwrap_or_default();
    let subject = parsed
        .headers
        .get_first_value("Subject")
        .unwrap_or_default();
    let addresses = |name: &str| -> Vec<String> {
        parsed
            .headers
            .get_all_values(name)
            .iter()
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
                    .unwrap_or_else(|_| vec![raw.clone()])
            })
            .collect()
    };
    let to = addresses("To");
    let own_id = parsed.headers.get_first_value("Message-ID");
    let references = parsed.headers.get_first_value("References");
    let in_reply_to = parsed.headers.get_first_value("In-Reply-To");
    let root = references
        .as_deref()
        .and_then(|s| s.split_whitespace().next())
        .or(in_reply_to.as_deref())
        .or(own_id.as_deref());
    let message_id = format!("{}:imap:{uid_validity}:{uid}", account.id);
    let thread_id = match root {
        Some(header) => format!(
            "{}:thread:{:x}",
            account.id,
            Sha256::digest(header.trim().as_bytes())
        ),
        None => message_id.clone(),
    };
    let mut body = ParsedBody::default();
    parse_body(&parsed, &mut body)?;
    let received_at = parsed
        .headers
        .get_first_value("Date")
        .and_then(|s| mailparse::dateparse(&s).ok())
        .map(|sec| sec.saturating_mul(1000))
        .or(internal_ms)
        .unwrap_or(0);
    Ok(Message {
        recipient_headers: Some(mail_core::RecipientHeaders {
            cc: addresses("Cc"),
            bcc: addresses("Bcc"),
            reply_to: addresses("Reply-To"),
        }),
        id: message_id,
        thread_id,
        account_id: account.id.clone(),
        sender,
        to,
        subject,
        body_plain: body.plain.unwrap_or_default(),
        body_html: body.html,
        unsubscribe_header: parsed.headers.get_first_value("List-Unsubscribe"),
        has_attachments: Some(!attachment_metadata(raw)?.is_empty()),
        received_at,
        is_read,
        is_draft: false,
        is_sent: false,
    })
}

#[derive(Default)]
struct ParsedBody {
    plain: Option<String>,
    html: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachmentMetadata {
    pub part_id: String,
    pub filename: Option<String>,
    pub content_type: String,
    pub content_id: Option<String>,
    pub decoded_size: usize,
}

/// Lists MIME attachments and inline resources without retaining their bytes.
pub fn attachment_metadata(raw: &[u8]) -> Result<Vec<AttachmentMetadata>, ImapError> {
    if raw.len() > MAX_RAW_MESSAGE_BYTES {
        return Err(ImapError::Identifier(
            "message exceeds the attachment access limit".into(),
        ));
    }
    let parsed = mailparse::parse_mail(raw)?;
    let mut output = Vec::new();
    collect_attachments(&parsed, "", &mut output)?;
    Ok(output)
}

fn collect_attachments(
    part: &ParsedMail<'_>,
    path: &str,
    output: &mut Vec<AttachmentMetadata>,
) -> Result<(), ImapError> {
    if !part.subparts.is_empty() {
        for (index, child) in part.subparts.iter().enumerate() {
            let child_path = if path.is_empty() {
                index.to_string()
            } else {
                format!("{path}.{index}")
            };
            collect_attachments(child, &child_path, output)?;
        }
        return Ok(());
    }
    let disposition = part.get_content_disposition();
    let filename = disposition
        .params
        .get("filename")
        .or_else(|| part.ctype.params.get("name"))
        .cloned();
    let content_id = part.headers.get_first_value("Content-ID").map(|s| {
        s.trim()
            .trim_start_matches('<')
            .trim_end_matches('>')
            .to_string()
    });
    if disposition.disposition == mailparse::DispositionType::Attachment
        || filename.is_some()
        || content_id.is_some()
    {
        output.push(AttachmentMetadata {
            part_id: if path.is_empty() {
                "root".into()
            } else {
                path.into()
            },
            filename,
            content_type: part.ctype.mimetype.to_ascii_lowercase(),
            content_id,
            decoded_size: estimated_decoded_size(part),
        });
    }
    Ok(())
}

fn estimated_decoded_size(part: &ParsedMail<'_>) -> usize {
    match part.get_body_encoded() {
        Body::Base64(body) => {
            let raw = body.get_raw();
            let symbols = raw.iter().filter(|b| !b.is_ascii_whitespace()).count();
            let padding = raw
                .iter()
                .rev()
                .filter(|b| !b.is_ascii_whitespace())
                .take(2)
                .filter(|b| **b == b'=')
                .count();
            symbols
                .div_ceil(4)
                .saturating_mul(3)
                .saturating_sub(padding)
        }
        Body::QuotedPrintable(body) => body.get_raw().len(),
        Body::SevenBit(body) | Body::EightBit(body) => body.get_raw().len(),
        Body::Binary(body) => body.get_raw().len(),
    }
}

/// Returns the decoded bytes of a named MIME attachment selected by its part path.
pub fn attachment_bytes(raw: &[u8], part_id: &str) -> Result<Vec<u8>, ImapError> {
    if raw.len() > MAX_RAW_MESSAGE_BYTES {
        return Err(ImapError::Identifier(
            "message exceeds the attachment access limit".into(),
        ));
    }
    let parsed = mailparse::parse_mail(raw)?;
    let mut attachments = Vec::new();
    collect_attachments(&parsed, "", &mut attachments)?;
    if !attachments.iter().any(|item| item.part_id == part_id) {
        return Err(ImapError::Identifier("attachment not found".into()));
    }
    fn part_at_path<'a>(part: &'a ParsedMail<'a>, path: &str) -> Option<&'a ParsedMail<'a>> {
        if path == "root" {
            return Some(part);
        }
        let mut current = part;
        for segment in path.split('.') {
            current = current.subparts.get(segment.parse::<usize>().ok()?)?;
        }
        Some(current)
    }
    let part = part_at_path(&parsed, part_id)
        .ok_or_else(|| ImapError::Identifier("attachment not found".into()))?;
    if estimated_decoded_size(part) > MAX_ATTACHMENT_BYTES {
        return Err(ImapError::Identifier(
            "attachment exceeds the preview limit".into(),
        ));
    }
    let bytes = part.get_body_raw()?;
    if bytes.len() > MAX_ATTACHMENT_BYTES {
        return Err(ImapError::Identifier(
            "attachment exceeds the preview limit".into(),
        ));
    }
    Ok(bytes)
}
fn parse_body(part: &ParsedMail<'_>, output: &mut ParsedBody) -> Result<(), ImapError> {
    if part.subparts.is_empty() {
        let disposition = part.get_content_disposition();
        if disposition.disposition == mailparse::DispositionType::Attachment
            || disposition.params.contains_key("filename")
            || part.ctype.params.contains_key("name")
        {
            return Ok(());
        }
        let mime = part.ctype.mimetype.to_ascii_lowercase();
        match mime.as_str() {
            "text/plain" if output.plain.is_none() => output.plain = Some(part.get_body()?),
            "text/html" if output.html.is_none() => output.html = Some(part.get_body()?),
            _ => {}
        }
    } else {
        for subpart in &part.subparts {
            parse_body(subpart, output)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account() -> Account {
        Account {
            id: "account:one".into(),
            provider: Provider::Imap,
            email: "user@example.com".into(),
            display_name: "User".into(),
        }
    }

    #[test]
    fn parses_multipart_message_without_treating_attachment_as_body() {
        let raw = b"From: Sender <sender@example.net>\r\nTo: User <user@example.com>\r\nSubject: =?UTF-8?Q?Hello_=E2=9C=93?=\r\nDate: Fri, 06 Nov 2026 10:00:00 +0000\r\nMessage-ID: <message@example.net>\r\nReferences: <root@example.net> <middle@example.net>\r\nList-Unsubscribe: <https://example.net/leave>\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=mix\r\n\r\n--mix\r\nContent-Type: multipart/alternative; boundary=alt\r\n\r\n--alt\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nPlain body\r\n--alt\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<p>HTML body</p>\r\n--alt--\r\n--mix\r\nContent-Type: text/plain; name=attachment.txt\r\nContent-Disposition: attachment; filename=attachment.txt\r\n\r\nAttachment text\r\n--mix--\r\n";
        let mail = parse_message(raw, &account(), 77, 42, false, None).unwrap();
        assert_eq!(mail.id, "account:one:imap:77:42");
        assert_eq!(mail.subject, "Hello ✓");
        assert!(mail.body_plain.contains("Plain body"));
        assert_eq!(
            mail.unsubscribe_header.as_deref(),
            Some("<https://example.net/leave>")
        );
        assert!(!mail.body_plain.contains("Attachment text"));
        assert!(mail.body_html.unwrap().contains("<p>HTML body</p>"));
        assert!(mail.thread_id.starts_with("account:one:thread:"));
        assert_eq!(mail.received_at, 1_793_959_200_000_i64);
        let attachments = attachment_metadata(raw).unwrap();
        assert_eq!(attachments.len(), 1);
        assert_eq!(attachments[0].part_id, "1");
        assert_eq!(attachments[0].filename.as_deref(), Some("attachment.txt"));
        assert_eq!(attachment_bytes(raw, "1").unwrap(), b"Attachment text");
    }

    #[test]
    fn cursor_backfills_older_mail_and_advances_new_mail_without_gaps() {
        let all = (1..=10).collect::<Vec<_>>();
        let (first, cursor) = select_uids(&all, None, 9, 3);
        assert_eq!(first, vec![8, 9, 10]);
        assert_eq!(cursor.high_uid, 10);
        assert_eq!(cursor.backfill_before, Some(8));
        let (second, cursor) = select_uids(&all, Some(cursor), 9, 3);
        assert!(second.contains(&5) && second.contains(&6) && second.contains(&7));
        assert_eq!(cursor.backfill_before, Some(5));
        let with_new = (1..=13).collect::<Vec<_>>();
        let (third, cursor) = select_uids(&with_new, Some(cursor), 9, 3);
        assert!(third.contains(&11) && third.contains(&12) && third.contains(&13));
        assert_eq!(cursor.high_uid, 13);
        assert_eq!(cursor.backfill_before, Some(5));
    }

    #[test]
    fn malformed_message_becomes_placeholder_without_losing_its_uid() {
        let message = parse_or_placeholder(
            b"Content-Type: text/plain\r\nContent-Transfer-Encoding: base64\r\n\r\n%%%%",
            &account(),
            77,
            42,
            false,
            Some(1234),
        );
        assert_eq!(message.id, "account:one:imap:77:42");
        assert_eq!(message.subject, "Message MIME could not be decoded");
        assert_eq!(message.received_at, 1234);
        assert!(!message.is_draft);
    }

    #[test]
    fn attachment_metadata_estimates_base64_size_without_decoding() {
        let raw = b"Content-Type: application/octet-stream; name=file.bin\r\nContent-Disposition: attachment; filename=file.bin\r\nContent-Transfer-Encoding: base64\r\n\r\nYWJjZA==\r\n";
        let metadata = attachment_metadata(raw).unwrap();
        assert_eq!(metadata[0].decoded_size, 4);
        assert_eq!(attachment_bytes(raw, "root").unwrap(), b"abcd");
        assert_eq!(bounded_body_query(10), "UID BODY.PEEK[]<0.11>");
    }

    #[test]
    fn secret_is_redacted_and_mailbox_names_cannot_inject_commands() {
        let config = ImapConfig::icloud("icloud:1", "person@icloud.com", "secret-value");
        assert!(!format!("{config:?}").contains("secret-value"));
        assert_eq!(config.username, "person");
        assert!(imap_quote("Archive\r\nUID EXPUNGE").is_err());
        assert_eq!(
            imap_quote("Other \"Folder\"").unwrap(),
            "\"Other \\\"Folder\\\"\""
        );
        assert!(parse_uid("account:one:imap:6:5", "account:one", 7).is_err());
    }

    #[test]
    fn builds_plain_and_html_mime_for_smtp() {
        let message = build_outgoing_with_attachments(
            "user@example.com",
            &["friend@example.net".into()],
            "A subject",
            "Plain content",
            Some("<p>HTML content</p>"),
            &[],
        )
        .unwrap();
        let raw = String::from_utf8(message.formatted()).unwrap();
        assert!(raw.contains("multipart/alternative"));
        assert!(raw.contains("Plain content"));
        assert!(raw.contains("<p>HTML content</p>"));
        assert!(raw.contains("friend@example.net"));
    }

    #[test]
    fn builds_attachment_mime_for_smtp() {
        let file = OutgoingAttachment {
            filename: "report.txt".into(),
            mime_type: "text/plain".into(),
            bytes: b"attached content".to_vec(),
        };
        let message = build_outgoing_with_attachments(
            "user@example.com",
            &["friend@example.net".into()],
            "Report",
            "Cover note",
            None,
            &[file],
        )
        .unwrap();
        let formatted = message.formatted();
        let parsed = mailparse::parse_mail(&formatted).unwrap();
        assert_eq!(parsed.ctype.mimetype, "multipart/mixed");
        assert_eq!(parsed.subparts.len(), 2);
        assert_eq!(parsed.subparts[0].get_body().unwrap(), "Cover note");
        assert_eq!(
            parsed.subparts[1].get_body_raw().unwrap(),
            b"attached content"
        );
        assert_eq!(
            parsed.subparts[1].get_content_disposition().params["filename"],
            "report.txt"
        );
    }
    #[test]
    fn smtp_envelope_delivers_bcc_without_exposing_it_in_headers() {
        for attachment in [false, true] {
            let files = if attachment {
                vec![OutgoingAttachment {
                    filename: "report.txt".into(),
                    mime_type: "text/plain".into(),
                    bytes: b"file".to_vec(),
                }]
            } else {
                vec![]
            };
            let message = build_outgoing_recipients(
                "me@example.com",
                &["to@example.com".into()],
                &["cc@example.com".into()],
                &["hidden@example.com".into()],
                "Subject",
                "Body",
                None,
                &files,
            )
            .unwrap();
            assert_eq!(message.envelope().to().len(), 3);
            assert!(
                message
                    .envelope()
                    .to()
                    .iter()
                    .any(|a| a.to_string() == "hidden@example.com")
            );
            let raw = String::from_utf8(message.formatted()).unwrap();
            assert!(raw.contains("Cc: cc@example.com"));
            assert!(!raw.contains("Bcc:"));
            assert!(!raw.contains("hidden@example.com"));
        }
        let blind = build_outgoing_recipients(
            "me@example.com",
            &[],
            &[],
            &["hidden@example.com".into()],
            "Subject",
            "Body",
            None,
            &[],
        )
        .unwrap();
        assert_eq!(blind.envelope().to().len(), 1);
        assert!(
            !String::from_utf8(blind.formatted())
                .unwrap()
                .contains("hidden@example.com")
        );
    }
}
