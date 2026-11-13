//! Gmail with an app password, end to end through the generic IMAP path,
//! against the synthetic loopback fixture. No real mailbox or Google server.
use super::*;
use mail_imap::fixture::{
    FIXTURE_APP_PASSWORD, FIXTURE_APP_PASSWORD_REQUIRED, FIXTURE_IMAP_DISABLED, FIXTURE_USER,
    FIXTURE_WEB_LOGIN, GmailFixture,
};
use std::sync::Arc;

#[derive(Default)]
struct MemoryVault(Mutex<HashMap<(String, bool), String>>);
impl CredentialStore for MemoryVault {
    fn read(&self, id: &str, legacy: bool) -> Result<Option<String>, String> {
        Ok(self.0.lock().unwrap().get(&(id.into(), legacy)).cloned())
    }
    fn write(&self, id: &str, value: &str) -> Result<(), String> {
        self.0
            .lock()
            .unwrap()
            .insert((id.into(), false), value.into());
        Ok(())
    }
    fn delete(&self, id: &str, legacy: bool) -> Result<(), String> {
        self.0.lock().unwrap().remove(&(id.into(), legacy));
        Ok(())
    }
}

struct Harness {
    dir: PathBuf,
    service: MailService,
    vault: Arc<MemoryVault>,
    fixture: GmailFixture,
    _serial: std::sync::MutexGuard<'static, ()>,
}

/// The fixture redirects Gmail's servers for the whole process; one test at a time.
fn serial() -> std::sync::MutexGuard<'static, ()> {
    static SERIAL: Mutex<()> = Mutex::new(());
    SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn open(dir: &Path, vault: &Arc<MemoryVault>) -> MailService {
    let mut service = MailService::open(dir.join("mail.sqlite")).unwrap();
    service.credentials = vault.clone();
    service
}

impl Harness {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("email-gmail-imap-{}", Uuid::new_v4()));
        let vault = Arc::new(MemoryVault::default());
        let service = open(&dir, &vault);
        let serial = serial();
        Self {
            dir,
            service,
            vault,
            fixture: GmailFixture::start().unwrap(),
            _serial: serial,
        }
    }

    /// Quit and relaunch: a fresh service over the same files and keyring.
    fn restart(&mut self) {
        self.service = open(&self.dir, &self.vault);
    }

    fn call(&self, value: Value) -> Result<Value, String> {
        self.service.request_json(&value.to_string())
    }

    /// The payload both shells send for Gmail with an app password. It names
    /// no servers: the preset dials imap.gmail.com and smtp.gmail.com, which
    /// the fixture redirects to itself.
    fn add(&self, address: &str, password: &str) -> Result<Value, String> {
        self.call(json!({
            "op": "add_imap_account", "provider": "imap", "preset": "gmail",
            "email": address, "display_name": address, "username": address, "smtp_username": address,
            "password": password,
        }))
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn gmail_app_password_account_syncs_searches_triages_sends_and_survives_restart() {
    let mut h = Harness::new();
    // Google shows app passwords in groups of four; the spaces are not part of it.
    let account = h.add(FIXTURE_USER, "abcd efgh ijkl mnop").unwrap();
    assert_eq!(account["provider"], "imap");
    let id = account["id"].as_str().unwrap().to_string();
    let saved = h.service.saved_config(&id).unwrap();
    assert_eq!(
        saved.archive_mailbox.as_deref(),
        Some("[Gmail]/All Mail"),
        "discovered from \\All"
    );
    assert_eq!(saved.username.as_deref(), Some(FIXTURE_USER));
    assert_eq!(
        h.vault.read(&id, false).unwrap().as_deref(),
        Some(FIXTURE_APP_PASSWORD)
    );

    // Sync.
    let synced = h
        .call(json!({"op": "sync_account", "account_id": id}))
        .unwrap();
    assert_eq!(synced["messages_added"], 3, "{synced}");
    assert!(synced.get("sync_error").is_none(), "{synced}");

    // Search the local cache.
    let found = h
        .call(json!({"op": "search", "query": "roadmap", "account_id": id}))
        .unwrap();
    let found = found.as_array().unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0]["subject"], "Quarterly planning notes");
    let inbox = h
        .call(json!({"op": "search", "query": "", "account_id": id, "is_inbox": true}))
        .unwrap();
    let thread = |subject: &str| {
        inbox
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["subject"] == subject)
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let digest = thread("Your weekly digest");
    let invoice = thread("Invoice 2026-11");

    // Triage: mark one read, archive another, then let sync apply them at Gmail.
    h.call(json!({"op": "batch_triage", "operation_id": "fixture-read", "kind": "read", "thread_ids": [digest]})).unwrap();
    h.call(json!({"op": "batch_triage", "operation_id": "fixture-archive", "kind": "archive", "thread_ids": [invoice]})).unwrap();
    let flushed = h
        .call(json!({"op": "sync_account", "account_id": id}))
        .unwrap();
    assert_eq!(flushed["actions_applied"], 2, "{flushed}");
    assert_eq!(flushed["pending_actions"], 0, "{flushed}");
    let server_inbox = h.fixture.mailbox("INBOX");
    assert_eq!(server_inbox.len(), 2);
    let digest_on_server = server_inbox
        .iter()
        .find(|m| String::from_utf8_lossy(&m.raw).contains("weekly digest"))
        .unwrap();
    assert!(digest_on_server.flags.contains(&"\\Seen".to_string()));
    let all_mail = h.fixture.mailbox("[Gmail]/All Mail");
    assert_eq!(all_mail.len(), 1);
    assert!(String::from_utf8_lossy(&all_mail[0].raw).contains("Invoice 2026-11"));
    let commands = h.fixture.commands();
    assert!(
        commands
            .iter()
            .any(|c| c.starts_with("UID STORE") && c.contains("+FLAGS.SILENT (\\Seen)")),
        "{commands:?}"
    );
    assert!(
        commands
            .iter()
            .any(|c| c.starts_with("UID MOVE ") && c.ends_with(" \"[Gmail]/All Mail\"")),
        "{commands:?}"
    );
    assert!(
        !commands.iter().any(|c| c.contains(FIXTURE_APP_PASSWORD)),
        "passwords are never logged"
    );

    // Send through smtp.gmail.com's stand-in.
    let draft = h
        .call(
            json!({"op": "compose_draft", "account_id": id, "to": "Friend <friend@example.com>",
        "subject": "Fixture send", "body_plain": "Sent through the Gmail app password fixture."}),
        )
        .unwrap();
    h.call(json!({"op": "send_draft", "message_id": draft["id"]}))
        .unwrap();
    let deliveries = h.fixture.deliveries();
    assert_eq!(deliveries.len(), 1);
    assert_eq!(deliveries[0].username, FIXTURE_USER);
    assert_eq!(deliveries[0].mail_from, FIXTURE_USER);
    assert_eq!(
        deliveries[0].recipients,
        vec!["friend@example.com".to_string()]
    );
    assert!(deliveries[0].data.contains("Subject: Fixture send"));
    assert!(
        deliveries[0]
            .data
            .contains(&format!("From: {FIXTURE_USER}"))
    );

    // Quit and relaunch: the account, its credential and the cache remain, and
    // the next sync picks up only new mail.
    h.restart();
    let accounts = h.call(json!({"op": "list_accounts"})).unwrap();
    assert_eq!(accounts.as_array().unwrap().len(), 1);
    assert_eq!(accounts[0]["id"], id.as_str());
    let cached = h
        .call(json!({"op": "search", "query": "roadmap", "account_id": id}))
        .unwrap();
    assert_eq!(
        cached.as_array().unwrap().len(),
        1,
        "cached mail is readable before any network"
    );
    let archived = h
        .call(json!({"op": "search", "query": "", "account_id": id, "is_inbox": true}))
        .unwrap();
    assert!(
        !archived
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["subject"] == "Invoice 2026-11"),
        "archive persists"
    );
    let new_uid = h.fixture.deliver(
        "Maya Fixture <maya@example.com>",
        "Agenda for Friday",
        "Bring the roadmap draft.",
    );
    let before = h.fixture.commands().len();
    let resynced = h
        .call(json!({"op": "sync_account", "account_id": id}))
        .unwrap();
    assert_eq!(
        resynced["messages_added"], 1,
        "only the new message is downloaded: {resynced}"
    );
    assert!(resynced.get("sync_error").is_none(), "{resynced}");
    let sync_commands = h.fixture.commands()[before..].to_vec();
    let bodies: Vec<_> = sync_commands
        .iter()
        .filter(|c| c.contains("BODY.PEEK[]"))
        .collect();
    assert_eq!(
        bodies.len(),
        1,
        "cached messages refresh flags only: {sync_commands:?}"
    );
    assert!(
        bodies[0].starts_with(&format!("UID FETCH {new_uid} ")),
        "{sync_commands:?}"
    );
    assert!(
        sync_commands
            .iter()
            .any(|c| c.starts_with("UID FETCH") && c.ends_with(" UID FLAGS")),
        "{sync_commands:?}"
    );
    let inbox = h
        .call(json!({"op": "search", "query": "", "account_id": id, "is_inbox": true}))
        .unwrap();
    let mut subjects: Vec<_> = inbox
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["subject"].as_str().unwrap().to_string())
        .collect();
    subjects.sort();
    assert_eq!(
        subjects,
        [
            "Agenda for Friday",
            "Quarterly planning notes",
            "Your weekly digest"
        ],
        "no duplicates after restart"
    );
    let agenda = || {
        h.call(json!({"op": "search", "query": "Agenda", "account_id": id}))
            .unwrap()[0]
            .clone()
    };
    assert_eq!(agenda()["unread_count"], 1);
    // Read on Gmail's website: the flags-only refresh still brings it across.
    h.fixture.set_seen("INBOX", new_uid, true);
    h.call(json!({"op": "sync_account", "account_id": id}))
        .unwrap();
    assert_eq!(agenda()["unread_count"], 0, "{}", agenda());
    let found = h
        .call(json!({"op": "search", "query": "roadmap", "account_id": id}))
        .unwrap();
    assert_eq!(found.as_array().unwrap().len(), 2, "{found}");
    assert_eq!(h.fixture.deliveries().len(), 1, "restart does not resend");
}

#[test]
fn regular_google_password_is_refused_before_contacting_google() {
    let h = Harness::new();
    for typed in [
        "my regular password",
        "abcdefghijklmno",
        "abcd efgh ijkl mnop q",
        "abcd-efgh-ijkl-mnop",
    ] {
        let error = h.add(FIXTURE_USER, typed).unwrap_err();
        assert_eq!(error, "That looks like your Google password.", "{typed}");
        let problem = &h.service.request_envelope(
            &json!({"op": "add_imap_account", "provider": "imap", "preset": "gmail",
                "email": FIXTURE_USER, "display_name": FIXTURE_USER, "password": typed})
            .to_string(),
        )["setup_error"];
        assert_eq!(problem["code"], "google_password");
        assert_eq!(problem["action"]["label"], "Create an app password instead");
        assert_eq!(problem["action"]["url"], "https://myaccount.google.com/apppasswords");
    }
    assert!(
        h.fixture.commands().is_empty(),
        "nothing reached the server"
    );
    // Spacing and capitals as Google shows or a person retypes them are fine.
    h.add(FIXTURE_USER, " ABCD efgh  IJKL mnop ").unwrap_err();
    assert!(
        !h.fixture.commands().is_empty(),
        "a well-formed app password is tried"
    );
}

#[test]
fn google_sign_in_failures_point_to_app_passwords() {
    use mail_cloud::CloudError;
    for error in [
        CloudError::AuthDenied,
        CloudError::AuthTimeout,
        CloudError::Browser,
    ] {
        let message = google_sign_in_failure(error);
        assert!(message.contains("Gmail (app password)"), "{message}");
    }
    assert!(google_sign_in_failure(CloudError::AuthDenied).contains("Access blocked"));
}

#[test]
fn gmail_app_password_setup_failures_are_explained_and_save_nothing() {
    let h = Harness::new();
    let cases = [
        (FIXTURE_USER, "zzzzyyyyxxxxwwww", "wrong_password", "Wrong email or app password."),
        (
            FIXTURE_USER,
            "my regular google password",
            "google_password",
            "That looks like your Google password.",
        ),
        (
            FIXTURE_APP_PASSWORD_REQUIRED,
            "qrstqrstqrstqrst",
            "google_password",
            "That looks like your Google password.",
        ),
        (
            FIXTURE_IMAP_DISABLED,
            FIXTURE_APP_PASSWORD,
            "imap_disabled",
            "Your Google Workspace admin has turned off IMAP for this account.",
        ),
        (
            FIXTURE_WEB_LOGIN,
            FIXTURE_APP_PASSWORD,
            "web_login",
            "Google blocked this sign-in. Confirm it was you in Gmail, then try again.",
        ),
    ];
    for (address, password, code, message) in cases {
        let request = json!({
            "op": "add_imap_account", "provider": "imap", "preset": "gmail",
            "email": address, "display_name": address, "password": password,
        });
        // request_json keeps a plain one-line error for any caller.
        let error = h.call(request.clone()).unwrap_err();
        assert_eq!(error, message, "{address}");
        // The envelope the shells read adds the structured problem.
        let envelope = h.service.request_envelope(&request.to_string());
        assert_eq!(envelope["ok"], false);
        assert_eq!(envelope["error"], message);
        let problem = &envelope["setup_error"];
        assert_eq!(problem["code"], code, "{address}: {problem}");
        assert_eq!(problem["message"], message);
        assert!(!problem.to_string().contains(password), "the password never appears in an error");
        if code == "google_password" || code == "wrong_password" {
            assert_eq!(problem["action"]["url"], "https://myaccount.google.com/apppasswords", "{problem}");
        }
        if code != "google_password" || password.len() == 16 {
            assert!(
                problem["detail"].as_str().unwrap().contains("imap.gmail.com:993 said:"),
                "server words go to the details: {problem}"
            );
        }
    }

    // A server that is not listening.
    let closed = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    {
        let _unreachable = mail_imap::fixture::redirect_gmail(closed, closed);
        let error = h.add(FIXTURE_USER, FIXTURE_APP_PASSWORD).unwrap_err();
        assert_eq!(
            error,
            "Couldn't reach imap.gmail.com. Check your internet connection, then try again."
        );
    }

    assert_eq!(h.call(json!({"op": "list_accounts"})).unwrap(), json!([]));
    assert!(
        h.vault.0.lock().unwrap().is_empty(),
        "failed setup stores no credential"
    );
}

#[test]
fn other_accounts_without_servers_try_a_guess_and_return_it_when_nothing_answers() {
    let h = Harness::new();
    let closed = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let request = json!({
        "op": "add_imap_account", "provider": "imap",
        "email": "person@example.org", "display_name": "person@example.org", "password": "secret",
    });
    {
        let _nothing = mail_imap::fixture::redirect_host("imap.example.org", 993, closed);
        let envelope = h.service.request_envelope(&request.to_string());
        let problem = &envelope["setup_error"];
        assert_eq!(problem["code"], "servers_not_found", "{envelope}");
        assert_eq!(
            envelope["error"],
            "Couldn't find the mail servers for example.org. Check the server settings."
        );
        assert_eq!(problem["servers"]["imap_host"], "imap.example.org");
        assert_eq!(problem["servers"]["imap_port"], 993);
        assert_eq!(problem["servers"]["imap_security"], "implicit_tls");
        assert_eq!(problem["servers"]["smtp_host"], "smtp.example.org");
        assert_eq!(problem["servers"]["smtp_port"], 587);
        assert_eq!(problem["servers"]["smtp_security"], "start_tls");
        assert!(problem["detail"].as_str().unwrap().starts_with("imap.example.org:993 said:"), "{problem}");
    }
    {
        // The guessed server answers but refuses the password: not a server problem.
        let _fixture = mail_imap::fixture::redirect_host("imap.example.org", 993, h.fixture.imap_port);
        let envelope = h.service.request_envelope(&request.to_string());
        let problem = &envelope["setup_error"];
        assert_eq!(problem["code"], "wrong_password", "{envelope}");
        assert_eq!(problem["message"], "Wrong email or app password.");
        assert!(problem["servers"].is_null());
    }
    assert_eq!(h.call(json!({"op": "list_accounts"})).unwrap(), json!([]));
}

#[test]
fn setup_provider_comes_from_the_domain_alone() {
    let h = Harness::new();
    for (email, provider) in [
        ("me@gmail.com", "gmail"),
        ("me@googlemail.com", "gmail"),
        ("me@icloud.com", "icloud"),
        ("me@me.com", "icloud"),
        ("me@mac.com", "icloud"),
        ("me@outlook.com", "outlook"),
        ("me@company.example", "other"),
        ("", "other"),
    ] {
        let value = h.call(json!({"op": "setup_provider", "email": email})).unwrap();
        assert_eq!(value, json!({"provider": provider}), "{email}");
    }
    assert!(h.fixture.commands().is_empty(), "no server is contacted");
}

#[test]
fn every_failed_request_keeps_the_plain_envelope() {
    let h = Harness::new();
    let unknown = h.service.request_envelope(r#"{"op":"no_such_operation"}"#);
    assert_eq!(unknown["ok"], false);
    assert!(unknown["error"].is_string() && unknown.get("setup_error").is_none());
    let accounts = h.service.request_envelope(r#"{"op":"list_accounts"}"#);
    assert_eq!(accounts, json!({"ok": true, "data": []}));
    let invalid = h.service.request_envelope(
        &json!({"op": "add_imap_account", "provider": "imap", "email": "nobody", "display_name": "", "password": "x"}).to_string(),
    );
    assert_eq!(invalid["error"], "valid email required");
    assert_eq!(invalid["setup_error"]["code"], "invalid");
}

#[test]
fn build_features_follow_the_release_channel() {
    let h = Harness::new();
    let features = h.call(json!({"op": "build_features"})).unwrap();
    let distribution = release::RELEASE_CHANNEL == "distribution";
    assert_eq!(features["channel"], release::RELEASE_CHANNEL);
    assert_eq!(features["distribution"], distribution);
    assert_eq!(features["google_contact_photos"], !distribution);
    assert_eq!(features["microsoft_accounts"], !distribution);
    assert_eq!(
        features["google_oauth_public"],
        release::GOOGLE_OAUTH_PUBLIC
    );
    if !distribution && option_env!("EMAIL_GOOGLE_OAUTH_PUBLIC").is_none() {
        assert_eq!(
            features["google_oauth_public"],
            env!("EMAIL_PROFILE_GOOGLE_OAUTH_PUBLIC") == "true",
            "follows the profile (development and neutral keep Google sign-in)"
        );
    }
    if !release::GOOGLE_OAUTH_PUBLIC {
        // Refused before any browser opens; existing accounts never call this.
        let gmail = h
            .call(json!({"op": "start_oauth", "provider": "gmail", "client_id": "fixture"}))
            .unwrap_err();
        assert!(
            gmail.starts_with("One-click Google sign-in isn't available in this build yet."),
            "{gmail}"
        );
    }
    if distribution {
        // Both refusals happen before any browser or network access.
        let outlook = h
            .call(json!({"op": "start_oauth", "provider": "microsoft", "client_id": "fixture"}))
            .unwrap_err();
        assert!(
            outlook.contains("Outlook and Hotmail accounts aren't available"),
            "{outlook}"
        );
        let photos = h
            .call(json!({"op": "enable_google_photos", "account_id": "any"}))
            .unwrap_err();
        assert!(
            photos.contains("Google contact photos aren't available"),
            "{photos}"
        );
        assert!(
            mail_cloud::authorize_google_photos(
                &OAuthConfig {
                    provider: CloudProvider::Gmail,
                    client_id: "fixture".into(),
                    client_secret: None
                },
                FIXTURE_USER,
            )
            .is_err()
        );
    }
}

#[test]
fn gmail_preset_is_only_for_the_generic_imap_provider() {
    let h = Harness::new();
    let error = h
        .call(json!({"op": "add_imap_account", "provider": "icloud", "preset": "gmail", "email": FIXTURE_USER,
            "display_name": FIXTURE_USER, "password": FIXTURE_APP_PASSWORD}))
        .unwrap_err();
    assert!(error.contains("uses the imap provider"), "{error}");
}

#[test]
fn gmail_preset_ignores_server_overrides_and_logs_in_once() {
    let h = Harness::new();
    // A stale or hostile request cannot send the app password elsewhere.
    let account = h
        .call(
            json!({"op": "add_imap_account", "provider": "imap", "preset": "gmail",
            "email": FIXTURE_USER, "display_name": FIXTURE_USER, "password": FIXTURE_APP_PASSWORD,
            "username": "someone.else@example.com", "smtp_username": "someone.else@example.com",
            "imap_host": "imap.attacker.example", "imap_port": 143, "imap_security": "start_tls",
            "smtp_host": "smtp.attacker.example", "smtp_port": 25, "smtp_security": "start_tls",
            "archive_mailbox": "Trash"}),
        )
        .unwrap();
    let saved = h
        .service
        .saved_config(account["id"].as_str().unwrap())
        .unwrap();
    assert_eq!(saved.imap_host.as_deref(), Some("imap.gmail.com"));
    assert_eq!(saved.imap_port, Some(993));
    assert_eq!(saved.imap_security, Some(TransportSecurity::ImplicitTls));
    assert_eq!(saved.smtp_host.as_deref(), Some("smtp.gmail.com"));
    assert_eq!(saved.smtp_port, Some(465));
    assert_eq!(saved.smtp_security, Some(TransportSecurity::ImplicitTls));
    assert_eq!(saved.username.as_deref(), Some(FIXTURE_USER));
    assert_eq!(saved.smtp_username.as_deref(), Some(FIXTURE_USER));
    assert_eq!(saved.archive_mailbox.as_deref(), Some("[Gmail]/All Mail"));
    let logins = h
        .fixture
        .commands()
        .iter()
        .filter(|c| c.as_str() == "LOGIN [redacted]")
        .count();
    assert_eq!(
        logins,
        1,
        "setup verifies and finds All Mail in one session: {:?}",
        h.fixture.commands()
    );
}

#[test]
fn archive_finds_localized_all_mail_even_when_hidden_at_setup() {
    let h = Harness::new();
    h.fixture.set_all_mail(None); // "Show in IMAP" off for All Mail
    let id = h.add(FIXTURE_USER, FIXTURE_APP_PASSWORD).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        h.service
            .saved_config(&id)
            .unwrap()
            .archive_mailbox
            .is_none(),
        "no guessed name is saved"
    );
    h.call(json!({"op": "sync_account", "account_id": id}))
        .unwrap();
    let inbox = h
        .call(json!({"op": "search", "query": "Invoice", "account_id": id}))
        .unwrap();
    let invoice = inbox[0]["id"].as_str().unwrap().to_string();
    h.call(json!({"op": "batch_triage", "operation_id": "hidden-archive", "kind": "archive", "thread_ids": [invoice]})).unwrap();
    let blocked = h
        .call(json!({"op": "sync_account", "account_id": id}))
        .unwrap();
    assert_eq!(blocked["actions_applied"], 0, "{blocked}");
    assert_eq!(
        blocked["pending_actions"], 1,
        "the archive stays queued: {blocked}"
    );
    assert!(
        blocked["warnings"].to_string().contains("Show in IMAP"),
        "{blocked}"
    );
    // The person turns All Mail back on; this account's name is localized.
    h.fixture
        .set_all_mail(Some("[Google Mail]/Alle Nachrichten"));
    let applied = h
        .call(json!({"op": "sync_account", "account_id": id}))
        .unwrap();
    assert_eq!(applied["actions_applied"], 1, "{applied}");
    assert_eq!(h.fixture.mailbox("[Google Mail]/Alle Nachrichten").len(), 1);
    assert_eq!(h.fixture.mailbox("INBOX").len(), 2);
}

#[test]
fn the_same_mailbox_cannot_be_connected_twice() {
    let h = Harness::new();
    h.add(FIXTURE_USER, FIXTURE_APP_PASSWORD).unwrap();
    // Gmail ignores case, dots and +tags in the address.
    for variant in [
        "FIXTURE.USER@gmail.com",
        "fixtureuser@gmail.com",
        "fixture.user+mail@googlemail.com",
    ] {
        let again = h.add(variant, FIXTURE_APP_PASSWORD).unwrap_err();
        assert!(
            again.contains("is already connected in Email"),
            "{variant}: {again}"
        );
    }
    assert_eq!(
        h.call(json!({"op": "list_accounts"}))
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn archive_follows_all_mail_renamed_after_setup() {
    let h = Harness::new();
    let id = h.add(FIXTURE_USER, FIXTURE_APP_PASSWORD).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        h.service
            .saved_config(&id)
            .unwrap()
            .archive_mailbox
            .as_deref(),
        Some("[Gmail]/All Mail")
    );
    // The person switches Gmail's display language.
    h.fixture.set_all_mail(Some("[Gmail]/Todos"));
    h.call(json!({"op": "sync_account", "account_id": id}))
        .unwrap();
    let invoice = h
        .call(json!({"op": "search", "query": "Invoice", "account_id": id}))
        .unwrap()[0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    h.call(json!({"op": "batch_triage", "operation_id": "renamed-archive", "kind": "archive", "thread_ids": [invoice]})).unwrap();
    let applied = h
        .call(json!({"op": "sync_account", "account_id": id}))
        .unwrap();
    assert_eq!(applied["actions_applied"], 1, "{applied}");
    assert_eq!(h.fixture.mailbox("[Gmail]/Todos").len(), 1);
}

#[test]
fn advice_is_one_line_whether_or_not_the_shell_offers_google_sign_in() {
    // Older shells send `google_sign_in`; it is accepted and no longer changes
    // the advice. Accounts without app passwords are covered in the window.
    let h = Harness::new();
    let attempt = |offered: bool| {
        h.call(
            json!({"op": "add_imap_account", "provider": "imap", "preset": "gmail",
            "email": FIXTURE_USER, "display_name": FIXTURE_USER, "password": "zzzzyyyyxxxxwwww",
            "google_sign_in": offered}),
        )
        .unwrap_err()
    };
    assert_eq!(attempt(false), "Wrong email or app password.");
    assert_eq!(attempt(true), "Wrong email or app password.");
}

#[test]
fn transient_gmail_refusal_is_reported_as_itself_and_list_failure_does_not_block_setup() {
    let h = Harness::new();
    h.fixture.set_list_fails(true);
    let id = h.add(FIXTURE_USER, FIXTURE_APP_PASSWORD).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        h.service
            .saved_config(&id)
            .unwrap()
            .archive_mailbox
            .is_none(),
        "All Mail is optional at setup"
    );
    h.fixture.set_list_fails(false);
    h.call(json!({"op": "sync_account", "account_id": id}))
        .unwrap();
    let invoice = h
        .call(json!({"op": "search", "query": "Invoice", "account_id": id}))
        .unwrap()[0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    h.call(json!({"op": "batch_triage", "operation_id": "throttled", "kind": "archive", "thread_ids": [invoice]})).unwrap();
    // First archive finds All Mail via LIST (nothing saved); a later refusal must not be blamed on "Show in IMAP".
    let applied = h
        .call(json!({"op": "sync_account", "account_id": id}))
        .unwrap();
    assert_eq!(applied["actions_applied"], 1, "{applied}");
    let digest = h
        .call(json!({"op": "search", "query": "digest", "account_id": id}))
        .unwrap()[0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    h.call(json!({"op": "batch_triage", "operation_id": "throttled-2", "kind": "archive", "thread_ids": [digest]})).unwrap();
    h.fixture
        .fail_next_move("[THROTTLED] Too many requests (Failure)");
    let refused = h
        .call(json!({"op": "sync_account", "account_id": id}))
        .unwrap();
    assert_eq!(refused["pending_actions"], 1, "{refused}");
    let warnings = refused["warnings"].to_string();
    assert!(
        warnings.contains("THROTTLED") && !warnings.contains("Show in IMAP"),
        "{warnings}"
    );
    let retried = h
        .call(json!({"op": "sync_account", "account_id": id}))
        .unwrap();
    assert_eq!(retried["actions_applied"], 1, "{retried}");
}

#[test]
fn over_budget_mail_is_deferred_not_stored_as_placeholders() {
    let h = Harness::new();
    let id = h.add(FIXTURE_USER, FIXTURE_APP_PASSWORD).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let saved = h.service.saved_config(&id).unwrap();
    // A tiny budget: each sync may download only its first message.
    let connector = h
        .service
        .imap_connector(&saved)
        .unwrap()
        .with_sync_budget(1);
    let mut inbox = Value::Null;
    for synced in 1..=3 {
        h.service.store.run_sync(&connector, &id).unwrap();
        inbox = h
            .call(json!({"op": "search", "query": "", "account_id": id, "is_inbox": true}))
            .unwrap();
        // The rest waits for the next sync instead of being stored empty.
        assert_eq!(inbox.as_array().unwrap().len(), synced, "{inbox}");
    }
    assert!(
        !inbox.to_string().contains("unavailable message"),
        "nothing became a placeholder: {inbox}"
    );
    let commands = h.fixture.commands();
    for uid in 1..=3 {
        let bodies = commands
            .iter()
            .filter(|c| c.starts_with(&format!("UID FETCH {uid} ")) && c.contains("BODY.PEEK[]"))
            .count();
        assert_eq!(bodies, 1, "UID {uid} downloaded exactly once: {commands:?}");
    }
}

#[test]
fn sync_without_a_cache_index_never_stalls() {
    use mail_core::MailAdapter;
    // A caller that cannot say what is cached (plain `MailAdapter::sync`) gets
    // placeholders for mail over the budget and a cursor that moves on, instead
    // of the same downloads on every call.
    let h = Harness::new();
    let id = h.add(FIXTURE_USER, FIXTURE_APP_PASSWORD).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let saved = h.service.saved_config(&id).unwrap();
    let connector = h
        .service
        .imap_connector(&saved)
        .unwrap()
        .with_sync_budget(1);
    let account = h
        .service
        .store
        .list_accounts()
        .unwrap()
        .into_iter()
        .find(|account| account.id == id)
        .unwrap();
    let first = connector.sync(&account, None).unwrap();
    assert_eq!(first.messages.len(), 3);
    let placeholders = first
        .messages
        .iter()
        .filter(|message| message.subject == "Message exceeds local sync limit")
        .count();
    assert_eq!(placeholders, 2, "{:?}", first.messages);
    assert_ne!(first.next_cursor, "null");
    let second = connector.sync(&account, Some(&first.next_cursor)).unwrap();
    assert_eq!(second.next_cursor, first.next_cursor, "nothing new: the cursor holds");
}

#[test]
fn undecodable_mail_is_not_downloaded_again() {
    let h = Harness::new();
    let id = h.add(FIXTURE_USER, FIXTURE_APP_PASSWORD).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let broken = b"From: Maya Fixture <maya@example.com>\r\nSubject: Broken\r\nContent-Type: text/plain\r\nContent-Transfer-Encoding: base64\r\n\r\n%%%%\r\n";
    let uid = h.fixture.deliver_raw(broken);
    h.call(json!({"op": "sync_account", "account_id": id}))
        .unwrap();
    let inbox = h
        .call(json!({"op": "search", "query": "", "account_id": id, "is_inbox": true}))
        .unwrap();
    assert!(
        inbox
            .to_string()
            .contains("Message MIME could not be decoded"),
        "{inbox}"
    );
    // IMAP messages never change: a later sync refreshes its flags only.
    let before = h.fixture.commands().len();
    let again = h
        .call(json!({"op": "sync_account", "account_id": id}))
        .unwrap();
    assert!(again.get("sync_error").is_none(), "{again}");
    let refetched = h.fixture.commands()[before..]
        .iter()
        .any(|c| c.starts_with(&format!("UID FETCH {uid} ")) && c.contains("BODY.PEEK[]"));
    assert!(!refetched, "{:?}", &h.fixture.commands()[before..]);
}
