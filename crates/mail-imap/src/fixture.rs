//! A synthetic Gmail-like IMAP and SMTP server for tests and QA builds.
//!
//! Compiled only with the `test-fixture` feature. It never contacts Google: it
//! listens on loopback with a certificate authority generated in memory, answers
//! with Gmail's documented response texts, and records what clients did. Use
//! the scenario addresses below to reproduce the common setup failures.
//!
//! Two ways to point a connector at it:
//! - in-process tests call [`GmailFixture::start`]; until it is dropped, this
//!   process trusts the fixture CA and dials it for `imap.gmail.com:993` and
//!   `smtp.gmail.com:465`, so tests exercise the unchanged Gmail preset
//!   (run such tests one at a time; the redirect is process-wide);
//! - QA builds of the native shells set `EMAIL_TEST_FIXTURE_CA` (a PEM file) and
//!   `EMAIL_TEST_FIXTURE_ENDPOINTS` (`imap.gmail.com:993=localhost:PORT,...`),
//!   which `examples/gmail_fixture.rs` prints, so the unchanged UI preset
//!   reaches the fixture.
use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose, SanType,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader, Read, Write},
    net::{IpAddr, Ipv4Addr, TcpListener, TcpStream},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

// Only FIXTURE_USER is a gmail.com address: tests use it to check that Gmail's
// dot and +tag variants count as the same mailbox. The other accounts behave
// alike on any Google domain, so they use a reserved one, as a Workspace
// account would.
/// Signs in. Use [`FIXTURE_APP_PASSWORD`], with or without Google's spacing.
pub const FIXTURE_USER: &str = "fixture.user@gmail.com";
/// A Google-style 16-letter app password.
pub const FIXTURE_APP_PASSWORD: &str = "abcdefghijklmnop";
/// Simulates entering the regular Google password on an account with 2-Step Verification.
pub const FIXTURE_APP_PASSWORD_REQUIRED: &str = "fixture.2sv@workspace.example";
/// Simulates a Google Workspace account whose administrator turned IMAP off.
pub const FIXTURE_IMAP_DISABLED: &str = "fixture.admin@workspace.example";
/// Simulates Google blocking an unusual sign-in until it is confirmed in a browser.
pub const FIXTURE_WEB_LOGIN: &str = "fixture.weblogin@workspace.example";
/// Answers LOGIN after [`SLOW_LOGIN`] and then rejects it, so a UI can be
/// exercised while a setup request is still in flight.
pub const FIXTURE_SLOW: &str = "fixture.slow@workspace.example";
/// Signs in with [`FIXTURE_APP_PASSWORD`] after [`SLOW_LOGIN`], so a UI can be
/// exercised when a request succeeds while the user is doing something else.
pub const FIXTURE_LATER: &str = "fixture.later@workspace.example";
pub const SLOW_LOGIN: Duration = Duration::from_secs(5);

const WRONG_PASSWORD: &str = "NO [AUTHENTICATIONFAILED] Invalid credentials (Failure)";
const APP_PASSWORD_REQUIRED: &str = "NO [ALERT] Application-specific password required: https://support.google.com/accounts/answer/185833 (Failure)";
const IMAP_DISABLED: &str = "NO [ALERT] IMAP access is disabled for your domain. Please contact your domain administrator for questions about this feature. (Failure)";
const WEB_LOGIN: &str =
    "NO [WEBALERT https://accounts.google.com/signin/continue?sarp=1&scc=1] Web login required.";
const UID_VALIDITY: u32 = 11;

struct FixtureTls {
    ca_der: Vec<u8>,
    ca_pem: String,
    chain: Vec<CertificateDer<'static>>,
    key_der: Vec<u8>,
}

/// One authority and `localhost` certificate per process. Valid for a week, so
/// Apple's TLS policy (validity limits, SAN and server-auth usage) accepts it.
fn tls() -> &'static FixtureTls {
    static TLS: OnceLock<FixtureTls> = OnceLock::new();
    TLS.get_or_init(|| {
        let now = time::OffsetDateTime::now_utc();
        let ca_key = KeyPair::generate().expect("fixture CA key");
        let mut ca = CertificateParams::new(Vec::<String>::new()).expect("fixture CA params");
        ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        ca.key_usages = vec![
            KeyUsagePurpose::KeyCertSign,
            KeyUsagePurpose::CrlSign,
            KeyUsagePurpose::DigitalSignature,
        ];
        ca.distinguished_name.push(
            DnType::CommonName,
            "Email test fixture CA (not trusted outside tests)",
        );
        ca.not_before = now - time::Duration::days(1);
        ca.not_after = now + time::Duration::days(7);
        let ca_cert = ca.self_signed(&ca_key).expect("fixture CA");
        let issuer = Issuer::new(ca, ca_key);
        let leaf_key = KeyPair::generate().expect("fixture leaf key");
        let mut leaf = CertificateParams::new(vec!["localhost".to_string()]).expect("fixture leaf");
        leaf.subject_alt_names
            .push(SanType::IpAddress(IpAddr::V4(Ipv4Addr::LOCALHOST)));
        leaf.distinguished_name
            .push(DnType::CommonName, "localhost");
        leaf.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        leaf.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        leaf.not_before = now - time::Duration::days(1);
        leaf.not_after = now + time::Duration::days(7);
        let leaf_cert = leaf
            .signed_by(&leaf_key, &issuer)
            .expect("fixture leaf certificate");
        FixtureTls {
            ca_der: ca_cert.der().to_vec(),
            ca_pem: ca_cert.pem(),
            chain: vec![leaf_cert.der().clone(), ca_cert.der().clone()],
            key_der: leaf_key.serialize_der(),
        }
    })
}

fn in_process_trust() -> &'static Mutex<bool> {
    static TRUST: OnceLock<Mutex<bool>> = OnceLock::new();
    TRUST.get_or_init(|| Mutex::new(false))
}

/// DER roots the connector adds to its trust store: this process's fixture CA
/// once a fixture started, and the PEM file named by `EMAIL_TEST_FIXTURE_CA`.
pub(crate) fn trusted_roots() -> Vec<Vec<u8>> {
    let mut roots = Vec::new();
    if in_process_trust().lock().is_ok_and(|trusted| *trusted) {
        roots.push(tls().ca_der.clone());
    }
    if let Some(pem) = std::env::var("EMAIL_TEST_FIXTURE_CA")
        .ok()
        .and_then(|path| std::fs::read_to_string(path).ok())
    {
        roots.extend(pem_certificates(&pem));
    }
    roots
}

fn pem_certificates(pem: &str) -> Vec<Vec<u8>> {
    use base64_decode as decode;
    pem.split("-----BEGIN CERTIFICATE-----")
        .skip(1)
        .filter_map(|block| block.split("-----END CERTIFICATE-----").next())
        .filter_map(|body| {
            decode(
                &body
                    .chars()
                    .filter(|c| !c.is_whitespace())
                    .collect::<String>(),
            )
        })
        .collect()
}

/// Minimal standard base64 decoder for the fixture CA file.
fn base64_decode(input: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut buffer = 0u32;
    let mut bits = 0;
    for byte in input.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            _ => return None,
        };
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    Some(out)
}

/// `EMAIL_TEST_FIXTURE_ENDPOINTS=imap.gmail.com:993=localhost:40001,smtp.gmail.com:465=localhost:40002`
type Redirects = Vec<(String, u16, String, u16)>;

fn in_process_redirects() -> &'static Mutex<Redirects> {
    static REDIRECTS: OnceLock<Mutex<Redirects>> = OnceLock::new();
    REDIRECTS.get_or_init(|| Mutex::new(Vec::new()))
}

/// Restores the previous in-process redirect when dropped.
pub struct RedirectGuard(Redirects);

impl Drop for RedirectGuard {
    fn drop(&mut self) {
        if let Ok(mut redirects) = in_process_redirects().lock() {
            *redirects = std::mem::take(&mut self.0);
        }
    }
}

/// Dial `localhost:imap_port` and `localhost:smtp_port` for Gmail's servers in
/// this process, for example a closed port to test an unreachable server.
pub fn redirect_gmail(imap_port: u16, smtp_port: u16) -> RedirectGuard {
    let mut redirects = in_process_redirects().lock().expect("fixture redirects");
    let previous = std::mem::replace(
        &mut *redirects,
        vec![
            (
                crate::GMAIL_IMAP_HOST.into(),
                crate::GMAIL_IMAP_PORT,
                "localhost".into(),
                imap_port,
            ),
            (
                crate::GMAIL_SMTP_HOST.into(),
                crate::GMAIL_SMTP_PORT,
                "localhost".into(),
                smtp_port,
            ),
        ],
    );
    RedirectGuard(previous)
}

/// Also dial `localhost:to_port` for `host:port` in this process, for
/// example a guessed `imap.example.org:993`. Restored when the guard drops.
pub fn redirect_host(host: &str, port: u16, to_port: u16) -> RedirectGuard {
    let mut redirects = in_process_redirects().lock().expect("fixture redirects");
    let previous = redirects.clone();
    redirects.push((host.into(), port, "localhost".into(), to_port));
    RedirectGuard(previous)
}

pub(crate) fn redirect(host: &str, port: u16) -> Option<(String, u16)> {
    if let Some((_, _, to_host, to_port)) = in_process_redirects()
        .lock()
        .ok()?
        .iter()
        .find(|(h, p, _, _)| h.eq_ignore_ascii_case(host) && *p == port)
    {
        return Some((to_host.clone(), *to_port));
    }
    let rules = std::env::var("EMAIL_TEST_FIXTURE_ENDPOINTS").ok()?;
    rules.split(',').find_map(|rule| {
        let (from, to) = rule.trim().split_once('=')?;
        let (from_host, from_port) = from.rsplit_once(':')?;
        if !from_host.eq_ignore_ascii_case(host) || from_port.parse::<u16>().ok()? != port {
            return None;
        }
        let (to_host, to_port) = to.rsplit_once(':')?;
        Some((to_host.to_string(), to_port.parse().ok()?))
    })
}

fn server_config() -> Arc<rustls::ServerConfig> {
    let tls = tls();
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(tls.key_der.clone()));
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    Arc::new(
        rustls::ServerConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS12, &rustls::version::TLS13])
            .expect("fixture TLS versions")
            .with_no_client_auth()
            .with_single_cert(tls.chain.clone(), key)
            .expect("fixture certificate"),
    )
}

#[derive(Clone, Debug)]
pub struct FixtureMessage {
    pub uid: u32,
    pub flags: Vec<String>,
    pub raw: Vec<u8>,
}

/// One message accepted by the SMTP fixture.
#[derive(Clone, Debug)]
pub struct Delivery {
    pub username: String,
    pub mail_from: String,
    pub recipients: Vec<String>,
    pub data: String,
}

#[derive(Default)]
struct State {
    mailboxes: BTreeMap<String, Vec<FixtureMessage>>,
    /// All Mail's current name, or `None` when hidden from IMAP ("Show in IMAP" off).
    all_mail: Option<String>,
    hidden_all_mail: Option<(String, Vec<FixtureMessage>)>,
    /// Answer LIST with BAD (a server hiccup).
    fail_list: bool,
    /// Refuse the next MOVE with this NO text (for example throttling).
    fail_next_move: Option<String>,
    next_uid: BTreeMap<String, u32>,
    /// Commands received, with LOGIN and AUTH arguments removed.
    commands: Vec<String>,
    deliveries: Vec<Delivery>,
}

const MAILBOXES: [(&str, &str); 6] = [
    ("INBOX", "\\HasNoChildren"),
    ("[Gmail]/All Mail", "\\All \\HasNoChildren"),
    ("[Gmail]/Sent Mail", "\\HasNoChildren \\Sent"),
    ("[Gmail]/Spam", "\\HasNoChildren \\Junk"),
    ("[Gmail]/Trash", "\\HasNoChildren \\Trash"),
    ("Receipts", "\\HasNoChildren"),
];

fn synthetic(uid: u32, from: &str, subject: &str, date: &str, body: &str) -> Vec<u8> {
    format!(
        "From: {from}\r\nTo: Fixture User <{FIXTURE_USER}>\r\nSubject: {subject}\r\nDate: {date}\r\n\
         Message-ID: <fixture-{uid}@example.com>\r\nMIME-Version: 1.0\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\r\n{body}\r\n"
    )
    .into_bytes()
}

/// Running fixture. Dropping it stops accepting connections.
pub struct GmailFixture {
    pub imap_port: u16,
    pub smtp_port: u16,
    state: Arc<Mutex<State>>,
    stop: Arc<AtomicBool>,
    _redirect: RedirectGuard,
}

impl GmailFixture {
    /// Start on loopback with three synthetic inbox messages, and trust the
    /// fixture CA in this process.
    pub fn start() -> std::io::Result<Self> {
        *in_process_trust().lock().expect("fixture trust") = true;
        let mut state = State::default();
        state.all_mail = Some("[Gmail]/All Mail".into());
        for (name, _) in MAILBOXES {
            state.mailboxes.insert(name.to_string(), Vec::new());
            state.next_uid.insert(name.to_string(), 1);
        }
        let seed = [
            (
                "Maya Fixture <maya@example.com>",
                "Quarterly planning notes",
                "Tue, 10 Nov 2026 09:15:00 +0000",
                "Let's review the roadmap on Friday.",
                true,
            ),
            (
                "Weekly Digest <news@example.net>",
                "Your weekly digest",
                "Wed, 11 Nov 2026 07:00:00 +0000",
                "Five stories picked for you.",
                false,
            ),
            (
                "Ola Fixture <ola@example.org>",
                "Invoice 2026-11",
                "Wed, 11 Nov 2026 12:30:00 +0000",
                "The November invoice is attached as text.",
                false,
            ),
        ];
        for (from, subject, date, body, seen) in seed {
            let uid = Self::next_uid(&mut state, "INBOX");
            let raw = synthetic(uid, from, subject, date, body);
            let flags = if seen {
                vec!["\\Seen".to_string()]
            } else {
                Vec::new()
            };
            state
                .mailboxes
                .get_mut("INBOX")
                .unwrap()
                .push(FixtureMessage { uid, flags, raw });
        }
        let state = Arc::new(Mutex::new(state));
        let stop = Arc::new(AtomicBool::new(false));
        let config = server_config();
        let imap = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let smtp = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let (imap_port, smtp_port) = (imap.local_addr()?.port(), smtp.local_addr()?.port());
        let fixture = Self {
            imap_port,
            smtp_port,
            state: state.clone(),
            stop: stop.clone(),
            _redirect: redirect_gmail(imap_port, smtp_port),
        };
        spawn_listener(
            imap,
            config.clone(),
            state.clone(),
            stop.clone(),
            serve_imap,
        );
        spawn_listener(smtp, config, state, stop, serve_smtp);
        Ok(fixture)
    }

    fn next_uid(state: &mut State, mailbox: &str) -> u32 {
        let next = state.next_uid.entry(mailbox.to_string()).or_insert(1);
        let uid = *next;
        *next += 1;
        uid
    }

    /// New mail arriving in INBOX, as if someone just sent it. Returns its UID.
    pub fn deliver(&self, from: &str, subject: &str, body: &str) -> u32 {
        let mut state = self.state.lock().unwrap();
        let uid = Self::next_uid(&mut state, "INBOX");
        let raw = synthetic(uid, from, subject, "Thu, 12 Nov 2026 08:00:00 +0000", body);
        state
            .mailboxes
            .get_mut("INBOX")
            .unwrap()
            .push(FixtureMessage {
                uid,
                flags: Vec::new(),
                raw,
            });
        uid
    }

    /// Deliver raw RFC 822 bytes to INBOX, for example a message the client cannot decode.
    pub fn deliver_raw(&self, raw: &[u8]) -> u32 {
        let mut state = self.state.lock().unwrap();
        let uid = Self::next_uid(&mut state, "INBOX");
        state
            .mailboxes
            .get_mut("INBOX")
            .unwrap()
            .push(FixtureMessage {
                uid,
                flags: Vec::new(),
                raw: raw.to_vec(),
            });
        uid
    }

    /// Replace a message's bytes in place (test-only; real IMAP messages never change).
    pub fn replace_raw(&self, mailbox: &str, uid: u32, raw: &[u8]) {
        let mut state = self.state.lock().unwrap();
        if let Some(message) = state
            .mailboxes
            .get_mut(mailbox)
            .and_then(|m| m.iter_mut().find(|m| m.uid == uid))
        {
            message.raw = raw.to_vec();
        }
    }

    /// Another client (for example Gmail on the web) marks a message read or unread.
    pub fn set_seen(&self, mailbox: &str, uid: u32, seen: bool) {
        let mut state = self.state.lock().unwrap();
        if let Some(message) = state
            .mailboxes
            .get_mut(mailbox)
            .and_then(|m| m.iter_mut().find(|m| m.uid == uid))
        {
            message.flags.retain(|flag| flag != "\\Seen");
            if seen {
                message.flags.push("\\Seen".into());
            }
        }
    }

    /// Rename All Mail (localized accounts, for example `[Google Mail]/Alle
    /// Nachrichten`) or hide it from IMAP with `None`. Its messages are kept.
    pub fn set_all_mail(&self, name: Option<&str>) {
        let mut state = self.state.lock().unwrap();
        let current = state.all_mail.clone();
        let key = current.clone().unwrap_or_else(|| "[Gmail]/All Mail".into());
        if let Some(name) = name {
            if let Some(messages) = state.mailboxes.remove(&key) {
                state.mailboxes.insert(name.into(), messages);
            }
            if let Some(next) = state.next_uid.remove(&key) {
                state.next_uid.insert(name.into(), next);
            }
        }
        state.all_mail = name.map(str::to_owned);
        if state.all_mail.is_none() {
            // Hidden mailboxes cannot be selected or moved to.
            let messages = state.mailboxes.remove(&key).unwrap_or_default();
            state.hidden_all_mail = Some((key, messages));
        } else if let Some((_, messages)) = state.hidden_all_mail.take() {
            let name = state.all_mail.clone().unwrap();
            state.mailboxes.entry(name).or_default().extend(messages);
        }
    }

    /// Make LIST fail until switched back.
    pub fn set_list_fails(&self, fails: bool) {
        self.state.lock().unwrap().fail_list = fails;
    }

    /// Refuse the next MOVE with `NO <text>`.
    pub fn fail_next_move(&self, text: &str) {
        self.state.lock().unwrap().fail_next_move = Some(text.into());
    }

    /// PEM of the fixture CA, for `EMAIL_TEST_FIXTURE_CA`.
    pub fn ca_pem(&self) -> &'static str {
        &tls().ca_pem
    }

    pub fn mailbox(&self, name: &str) -> Vec<FixtureMessage> {
        self.state
            .lock()
            .unwrap()
            .mailboxes
            .get(name)
            .cloned()
            .unwrap_or_default()
    }

    pub fn commands(&self) -> Vec<String> {
        self.state.lock().unwrap().commands.clone()
    }

    pub fn deliveries(&self) -> Vec<Delivery> {
        self.state.lock().unwrap().deliveries.clone()
    }
}

impl Drop for GmailFixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        // Wake both accept loops so their threads exit.
        let _ = TcpStream::connect((Ipv4Addr::LOCALHOST, self.imap_port));
        let _ = TcpStream::connect((Ipv4Addr::LOCALHOST, self.smtp_port));
    }
}

type Serve = fn(&mut dyn Stream, &Arc<Mutex<State>>) -> std::io::Result<()>;
trait Stream: Read + Write {}
impl<T: Read + Write> Stream for T {}

fn spawn_listener(
    listener: TcpListener,
    config: Arc<rustls::ServerConfig>,
    state: Arc<Mutex<State>>,
    stop: Arc<AtomicBool>,
    serve: Serve,
) {
    thread::spawn(move || {
        for socket in listener.incoming() {
            if stop.load(Ordering::Acquire) {
                break;
            }
            let Ok(socket) = socket else { continue };
            let config = config.clone();
            let state = state.clone();
            thread::spawn(move || {
                let _ = socket.set_read_timeout(Some(Duration::from_secs(30)));
                let Ok(connection) = rustls::ServerConnection::new(config) else {
                    return;
                };
                let mut stream = rustls::StreamOwned::new(connection, socket);
                let _ = serve(&mut stream, &state);
                stream.conn.send_close_notify();
                let _ = stream.flush();
            });
        }
    });
}

fn read_line(reader: &mut BufReader<&mut dyn Stream>) -> std::io::Result<Option<String>> {
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    Ok(Some(line.trim_end_matches(['\r', '\n']).to_string()))
}

/// Split IMAP arguments: atoms, quoted strings (with `\\` escapes) and parenthesized lists.
fn imap_args(input: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut chars = input.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c == ' ' {
            chars.next();
        } else if c == '"' {
            chars.next();
            let mut value = String::new();
            while let Some(c) = chars.next() {
                match c {
                    '\\' => value.extend(chars.next()),
                    '"' => break,
                    other => value.push(other),
                }
            }
            args.push(value);
        } else if c == '(' {
            let mut value = String::new();
            for c in chars.by_ref() {
                value.push(c);
                if c == ')' {
                    break;
                }
            }
            args.push(value);
        } else {
            let mut value = String::new();
            while let Some(&c) = chars.peek() {
                if c == ' ' {
                    break;
                }
                value.push(c);
                chars.next();
            }
            args.push(value);
        }
    }
    args
}

fn uid_set(set: &str, messages: &[FixtureMessage]) -> Vec<u32> {
    let max = messages.iter().map(|m| m.uid).max().unwrap_or(0);
    let mut wanted = Vec::new();
    for part in set.split(',') {
        let bound = |v: &str| {
            if v == "*" {
                Some(max)
            } else {
                v.parse::<u32>().ok()
            }
        };
        let (low, high) = match part.split_once(':') {
            Some((a, b)) => (bound(a), bound(b)),
            None => (bound(part), bound(part)),
        };
        if let (Some(a), Some(b)) = (low, high) {
            let (a, b) = (a.min(b), a.max(b));
            wanted.extend(
                messages
                    .iter()
                    .map(|m| m.uid)
                    .filter(|uid| (a..=b).contains(uid)),
            );
        }
    }
    wanted.sort_unstable();
    wanted.dedup();
    wanted
}

/// Gmail signs in the same mailbox regardless of case, dots and `+tags` in a
/// gmail.com or googlemail.com address.
fn login_response(username: &str, password: &str) -> Result<(), &'static str> {
    let compact: String = password.chars().filter(|c| !c.is_whitespace()).collect();
    let username = if crate::mailbox_identity(username) == crate::mailbox_identity(FIXTURE_USER) {
        FIXTURE_USER.to_string()
    } else {
        username.to_ascii_lowercase()
    };
    match username.as_str() {
        FIXTURE_USER | FIXTURE_LATER if compact == FIXTURE_APP_PASSWORD => Ok(()),
        FIXTURE_APP_PASSWORD_REQUIRED => Err(APP_PASSWORD_REQUIRED),
        FIXTURE_IMAP_DISABLED => Err(IMAP_DISABLED),
        FIXTURE_WEB_LOGIN => Err(WEB_LOGIN),
        _ => Err(WRONG_PASSWORD),
    }
}

fn serve_imap(stream: &mut dyn Stream, state: &Arc<Mutex<State>>) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream);
    reader
        .get_mut()
        .write_all(b"* OK Gimap ready for requests from 127.0.0.1 (Email test fixture)\r\n")?;
    reader.get_mut().flush()?;
    let mut selected: Option<String> = None;
    let mut authenticated = false;
    loop {
        let Some(line) = read_line(&mut reader)? else {
            return Ok(());
        };
        let stream = reader.get_mut();
        let (tag, rest) = line.split_once(' ').unwrap_or((line.as_str(), ""));
        let (command, args) = rest.split_once(' ').unwrap_or((rest, ""));
        let mut command = command.to_ascii_uppercase();
        let mut args = args.to_string();
        if command == "UID" {
            let (sub, more) = args.split_once(' ').unwrap_or((args.as_str(), ""));
            command = format!("UID {}", sub.to_ascii_uppercase());
            args = more.to_string();
        }
        state.lock().unwrap().commands.push(if command == "LOGIN" {
            "LOGIN [redacted]".to_string()
        } else {
            format!("{command} {args}").trim_end().to_string()
        });
        let mut out = String::new();
        let done = match command.as_str() {
            "CAPABILITY" => {
                out.push_str("* CAPABILITY IMAP4rev1 UNSELECT IDLE NAMESPACE QUOTA ID XLIST CHILDREN X-GM-EXT-1 UIDPLUS ENABLE MOVE CONDSTORE ESEARCH UTF8=ACCEPT LIST-EXTENDED LIST-STATUS LITERAL- SPECIAL-USE APPENDLIMIT=35651584\r\n");
                format!("{tag} OK Thats all she wrote! (Success)")
            }
            "LOGIN" => {
                let parts = imap_args(&args);
                if parts.first().is_some_and(|user| {
                    user.eq_ignore_ascii_case(FIXTURE_SLOW)
                        || user.eq_ignore_ascii_case(FIXTURE_LATER)
                }) {
                    thread::sleep(SLOW_LOGIN);
                }
                match login_response(
                    parts.first().map(String::as_str).unwrap_or(""),
                    parts.get(1).map(String::as_str).unwrap_or(""),
                ) {
                    Ok(()) => {
                        authenticated = true;
                        format!("{tag} OK {FIXTURE_USER} authenticated (Success)")
                    }
                    Err(response) => format!("{tag} {response}"),
                }
            }
            "LOGOUT" => {
                stream.write_all(
                    format!("* BYE LOGOUT Requested\r\n{tag} OK 73 good day (Success)\r\n")
                        .as_bytes(),
                )?;
                stream.flush()?;
                return Ok(());
            }
            "NOOP" => format!("{tag} OK Success"),
            _ if !authenticated => format!("{tag} BAD Command not valid before LOGIN"),
            "SELECT" | "EXAMINE" => {
                let name = imap_args(&args).into_iter().next().unwrap_or_default();
                let guard = state.lock().unwrap();
                match guard.mailboxes.get(&name) {
                    Some(messages) => {
                        let next = guard.next_uid.get(&name).copied().unwrap_or(1);
                        out.push_str("* FLAGS (\\Answered \\Flagged \\Draft \\Deleted \\Seen $NotPhishing $Phishing)\r\n");
                        out.push_str("* OK [PERMANENTFLAGS (\\Answered \\Flagged \\Draft \\Deleted \\Seen $NotPhishing $Phishing \\*)] Flags permitted.\r\n");
                        out.push_str(&format!(
                            "* OK [UIDVALIDITY {UID_VALIDITY}] UIDs valid.\r\n"
                        ));
                        out.push_str(&format!("* {} EXISTS\r\n* 0 RECENT\r\n", messages.len()));
                        out.push_str(&format!("* OK [UIDNEXT {next}] Predicted next UID.\r\n"));
                        selected = Some(name.clone());
                        format!("{tag} OK [READ-WRITE] {name} selected. (Success)")
                    }
                    None => format!("{tag} NO [NONEXISTENT] Unknown Mailbox: {name} (Failure)"),
                }
            }
            "LIST" if state.lock().unwrap().fail_list => {
                format!("{tag} BAD Temporary system problem")
            }
            "LIST" => {
                let all_mail = state.lock().unwrap().all_mail.clone();
                for (name, attributes) in MAILBOXES {
                    let name = if attributes.contains("\\All") {
                        match &all_mail {
                            Some(current) => current.as_str(),
                            None => continue,
                        }
                    } else {
                        name
                    };
                    out.push_str(&format!("* LIST ({attributes}) \"/\" \"{name}\"\r\n"));
                }
                format!("{tag} OK Success")
            }
            "UID SEARCH" => {
                let guard = state.lock().unwrap();
                let messages = selected.as_ref().and_then(|name| guard.mailboxes.get(name));
                let uids = messages
                    .map(|m| {
                        m.iter()
                            .map(|m| m.uid.to_string())
                            .collect::<Vec<_>>()
                            .join(" ")
                    })
                    .unwrap_or_default();
                out.push_str(&format!("* SEARCH {uids}\r\n").replace("SEARCH \r\n", "SEARCH\r\n"));
                format!("{tag} OK SEARCH completed (Success)")
            }
            "UID FETCH" => {
                let (set, items) = args.split_once(' ').unwrap_or((args.as_str(), ""));
                let guard = state.lock().unwrap();
                let messages = selected
                    .as_ref()
                    .and_then(|name| guard.mailboxes.get(name))
                    .cloned()
                    .unwrap_or_default();
                let mut bytes = out.into_bytes();
                for uid in uid_set(set, &messages) {
                    let (index, message) = messages
                        .iter()
                        .enumerate()
                        .find(|(_, m)| m.uid == uid)
                        .unwrap();
                    let seq = index + 1;
                    if items.to_ascii_uppercase().contains("BODY.PEEK[]") {
                        bytes.extend_from_slice(
                            format!(
                                "* {seq} FETCH (UID {uid} BODY[]<0> {{{}}}\r\n",
                                message.raw.len()
                            )
                            .as_bytes(),
                        );
                        bytes.extend_from_slice(&message.raw);
                        bytes.extend_from_slice(b")\r\n");
                    } else {
                        bytes.extend_from_slice(format!(
                            "* {seq} FETCH (UID {uid} FLAGS ({}) INTERNALDATE \"11-Nov-2026 12:{:02}:00 +0000\" RFC822.SIZE {})\r\n",
                            message.flags.join(" "), uid % 60, message.raw.len()
                        ).as_bytes());
                    }
                }
                drop(guard);
                bytes.extend_from_slice(format!("{tag} OK Success\r\n").as_bytes());
                stream.write_all(&bytes)?;
                stream.flush()?;
                continue;
            }
            "UID STORE" => {
                let parts: Vec<&str> = args.splitn(3, ' ').collect();
                let (set, operation, flags) = (
                    parts.first().copied().unwrap_or(""),
                    parts.get(1).copied().unwrap_or(""),
                    parts.get(2).copied().unwrap_or(""),
                );
                let flags: Vec<String> = flags
                    .trim_matches(['(', ')'])
                    .split_whitespace()
                    .map(str::to_string)
                    .collect();
                let mut guard = state.lock().unwrap();
                if let Some(messages) = selected
                    .as_ref()
                    .and_then(|name| guard.mailboxes.get_mut(name))
                {
                    for uid in uid_set(set, messages) {
                        let message = messages.iter_mut().find(|m| m.uid == uid).unwrap();
                        if operation.starts_with('+') {
                            for flag in &flags {
                                if !message.flags.contains(flag) {
                                    message.flags.push(flag.clone());
                                }
                            }
                        } else if operation.starts_with('-') {
                            message.flags.retain(|flag| !flags.contains(flag));
                        }
                    }
                }
                format!("{tag} OK Success")
            }
            "UID MOVE" => {
                let (set, destination) = args.split_once(' ').unwrap_or((args.as_str(), ""));
                let destination = imap_args(destination)
                    .into_iter()
                    .next()
                    .unwrap_or_default();
                let mut guard = state.lock().unwrap();
                let Some(source) = selected.clone() else {
                    out.push_str(&format!("{tag} NO No mailbox selected\r\n"));
                    stream.write_all(out.as_bytes())?;
                    continue;
                };
                if let Some(text) = guard.fail_next_move.take() {
                    format!("{tag} NO {text}")
                } else if !guard.mailboxes.contains_key(&destination) {
                    format!("{tag} NO [TRYCREATE] No folder {destination} (Failure)")
                } else {
                    let wanted = uid_set(set, guard.mailboxes.get(&source).unwrap());
                    let mut moved = Vec::new();
                    let messages = guard.mailboxes.get_mut(&source).unwrap();
                    for uid in wanted.iter().rev() {
                        let index = messages.iter().position(|m| m.uid == *uid).unwrap();
                        out.push_str(&format!("* {} EXPUNGE\r\n", index + 1));
                        moved.push(messages.remove(index));
                    }
                    for mut message in moved.into_iter().rev() {
                        message.uid = GmailFixture::next_uid(&mut guard, &destination);
                        guard.mailboxes.get_mut(&destination).unwrap().push(message);
                    }
                    format!("{tag} OK Success")
                }
            }
            _ => format!("{tag} BAD Unknown command"),
        };
        out.push_str(&done);
        out.push_str("\r\n");
        stream.write_all(out.as_bytes())?;
        stream.flush()?;
    }
}

fn smtp_credentials(argument: &str) -> Option<(String, String)> {
    let decoded = base64_decode(argument.trim())?;
    let mut parts = decoded.split(|byte| *byte == 0).skip(1);
    let user = String::from_utf8(parts.next()?.to_vec()).ok()?;
    let password = String::from_utf8(parts.next()?.to_vec()).ok()?;
    Some((user, password))
}

fn serve_smtp(stream: &mut dyn Stream, state: &Arc<Mutex<State>>) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream);
    let send = |reader: &mut BufReader<&mut dyn Stream>, text: &str| -> std::io::Result<()> {
        reader.get_mut().write_all(text.as_bytes())?;
        reader.get_mut().flush()
    };
    send(
        &mut reader,
        "220 smtp.gmail.com ESMTP Email test fixture\r\n",
    )?;
    let mut user: Option<String> = None;
    let mut mail_from = String::new();
    let mut recipients = Vec::new();
    loop {
        let Some(line) = read_line(&mut reader)? else {
            return Ok(());
        };
        let upper = line.to_ascii_uppercase();
        state
            .lock()
            .unwrap()
            .commands
            .push(if upper.starts_with("AUTH") {
                "SMTP AUTH [redacted]".to_string()
            } else {
                format!("SMTP {line}")
            });
        if upper.starts_with("EHLO") || upper.starts_with("HELO") {
            send(
                &mut reader,
                "250-smtp.gmail.com at your service\r\n250-SIZE 35882577\r\n250-8BITMIME\r\n250-AUTH LOGIN PLAIN XOAUTH2 PLAIN-CLIENTTOKEN OAUTHBEARER XOAUTH\r\n250-ENHANCEDSTATUSCODES\r\n250 SMTPUTF8\r\n",
            )?;
        } else if let Some(argument) = upper.strip_prefix("AUTH PLAIN ").map(|_| &line[11..]) {
            match smtp_credentials(argument).filter(|(u, p)| login_response(u, p).is_ok()) {
                Some((name, _)) => {
                    user = Some(name);
                    send(&mut reader, "235 2.7.0 Accepted\r\n")?;
                }
                None => {
                    let reply = match smtp_credentials(argument)
                        .map(|(u, _)| u.to_ascii_lowercase())
                    {
                        Some(u) if u == FIXTURE_APP_PASSWORD_REQUIRED => {
                            "534-5.7.9 Application-specific password required. For more information, go to\r\n534 5.7.9  https://support.google.com/mail/?p=InvalidSecondFactor\r\n"
                        }
                        _ => {
                            "535-5.7.8 Username and Password not accepted. For more information, go to\r\n535 5.7.8  https://support.google.com/mail/?p=BadCredentials\r\n"
                        }
                    };
                    send(&mut reader, reply)?;
                }
            }
        } else if upper == "NOOP" {
            send(&mut reader, "250 2.0.0 OK\r\n")?;
        } else if upper.starts_with("MAIL FROM:") {
            if user.is_none() {
                send(
                    &mut reader,
                    "530-5.7.0 Authentication Required.\r\n530 5.7.0  https://support.google.com/mail/?p=WantAuthError\r\n",
                )?;
                continue;
            }
            mail_from = line[10..]
                .trim()
                .trim_matches(['<', '>'])
                .split('>')
                .next()
                .unwrap_or("")
                .to_string();
            recipients.clear();
            send(&mut reader, "250 2.1.0 OK\r\n")?;
        } else if upper.starts_with("RCPT TO:") {
            recipients.push(
                line[8..]
                    .trim()
                    .trim_matches(['<', '>'])
                    .split('>')
                    .next()
                    .unwrap_or("")
                    .to_string(),
            );
            send(&mut reader, "250 2.1.5 OK\r\n")?;
        } else if upper == "DATA" {
            send(&mut reader, "354 Go ahead\r\n")?;
            let mut data = String::new();
            loop {
                let Some(line) = read_line(&mut reader)? else {
                    return Ok(());
                };
                if line == "." {
                    break;
                }
                data.push_str(
                    line.strip_prefix('.')
                        .filter(|_| line.starts_with(".."))
                        .unwrap_or(&line),
                );
                data.push_str("\r\n");
            }
            state.lock().unwrap().deliveries.push(Delivery {
                username: user.clone().unwrap_or_default(),
                mail_from: mail_from.clone(),
                recipients: std::mem::take(&mut recipients),
                data,
            });
            send(&mut reader, "250 2.0.0 OK fixture accepted\r\n")?;
        } else if upper == "RSET" {
            recipients.clear();
            send(&mut reader, "250 2.1.5 Flushed\r\n")?;
        } else if upper == "QUIT" {
            send(&mut reader, "221 2.0.0 closing connection\r\n")?;
            return Ok(());
        } else {
            send(&mut reader, "502 5.5.1 Unrecognized command.\r\n")?;
        }
    }
}
