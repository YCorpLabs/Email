//! Local account generations fence credential commits across service handles.
use super::*;
use std::{fs::File, ops::Deref};

pub(super) trait CredentialStore: Send + Sync {
    fn read(&self, id: &str, legacy: bool) -> Result<Option<String>, String>;
    fn write(&self, id: &str, value: &str) -> Result<(), String>;
    fn delete(&self, id: &str, legacy: bool) -> Result<(), String>;
    fn read_mail(&self, id: &str) -> Result<Option<String>, String> {
        if let Some(value) = self.read(id, false)? {
            return Ok(Some(value));
        }
        let value = self.read(id, true)?;
        if let Some(value) = &value {
            self.write(id, value)?;
        }
        Ok(value)
    }
    fn revision(&self, _id: &str) -> Result<Option<String>, String> {
        Ok(None)
    }
    fn write_if_revision(
        &self,
        id: &str,
        value: &str,
        _expected: Option<&str>,
    ) -> Result<Option<String>, String> {
        self.write(id, value)?;
        self.revision(id)
    }
}
pub(super) struct OsCredentials;
/// Keychain items a refused or failed read already prompted for in this
/// process. Retrying them on every sync re-raised the macOS password prompt
/// each minute; they stay unavailable until Email is relaunched.
#[cfg(not(target_os = "macos"))]
fn refused_credentials() -> &'static Mutex<HashSet<(String, bool)>> {
    static REFUSED: std::sync::OnceLock<Mutex<HashSet<(String, bool)>>> =
        std::sync::OnceLock::new();
    REFUSED.get_or_init(Default::default)
}
#[cfg(not(target_os = "macos"))]
impl CredentialStore for OsCredentials {
    fn read(&self, id: &str, legacy: bool) -> Result<Option<String>, String> {
        // Sync, calendar and photo work read the same item concurrently. With an
        // item that still prompts, each parallel read raised its own dialog.
        // One read at a time: later readers find the migrated copy or the refusal.
        static READ: std::sync::OnceLock<Mutex<()>> = std::sync::OnceLock::new();
        let _serial = READ
            .get_or_init(Default::default)
            .lock()
            .map_err(|_| "Keychain lock poisoned")?;
        let key = (id.to_string(), legacy);
        if refused_credentials()
            .lock()
            .is_ok_and(|set| set.contains(&key))
        {
            return Err(
                "Keychain access for this account was refused. Relaunch Email to try again.".into(),
            );
        }
        let entry = keyring::Entry::new(
            if legacy {
                LEGACY_KEYRING_SERVICE
            } else {
                KEYRING_SERVICE
            },
            id,
        )
        .map_err(|e| e.to_string())?;
        let result = match entry.get_password() {
            Err(keyring::Error::NoEntry) if !legacy => match PREVIOUS_KEYRING_SERVICE {
                Some(previous) => {
                    match keyring::Entry::new(previous, id)
                        .map_err(|e| e.to_string())?
                        .get_password()
                    {
                        Ok(value) => {
                            // Re-create under the current service so later reads do not prompt.
                            let _ = MailService::entry(id)
                                .and_then(|e| e.set_password(&value).map_err(|e| e.to_string()));
                            Ok(value)
                        }
                        other => other,
                    }
                }
                None => Err(keyring::Error::NoEntry),
            },
            other => other,
        };
        match result {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => {
                if let Ok(mut set) = refused_credentials().lock() {
                    set.insert(key);
                }
                Err(error.to_string())
            }
        }
    }
    fn write(&self, id: &str, value: &str) -> Result<(), String> {
        MailService::entry(id)?
            .set_password(value)
            .map_err(|e| e.to_string())?;
        if let Ok(mut set) = refused_credentials().lock() {
            set.remove(&(id.to_string(), false));
        }
        Ok(())
    }
    fn delete(&self, id: &str, legacy: bool) -> Result<(), String> {
        let entry = keyring::Entry::new(
            if legacy {
                LEGACY_KEYRING_SERVICE
            } else {
                KEYRING_SERVICE
            },
            id,
        )
        .map_err(|e| e.to_string())?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => {}
            Err(error) => return Err(error.to_string()),
        }
        if let (false, Some(previous)) = (legacy, PREVIOUS_KEYRING_SERVICE) {
            match keyring::Entry::new(previous, id)
                .map_err(|e| e.to_string())?
                .delete_credential()
            {
                Ok(()) | Err(keyring::Error::NoEntry) => {}
                Err(error) => return Err(error.to_string()),
            }
        }
        Ok(())
    }
}
#[cfg(target_os = "macos")]
impl CredentialStore for OsCredentials {
    fn read(&self, id: &str, legacy: bool) -> Result<Option<String>, String> {
        super::credential_access::read(
            if legacy {
                LEGACY_KEYRING_SERVICE
            } else {
                KEYRING_SERVICE
            },
            if legacy {
                None
            } else {
                PREVIOUS_KEYRING_SERVICE
            },
            id,
        )
    }
    fn read_mail(&self, id: &str) -> Result<Option<String>, String> {
        super::credential_access::read_mail(id)
    }
    fn write(&self, id: &str, value: &str) -> Result<(), String> {
        super::credential_access::write(KEYRING_SERVICE, id, value)
    }
    fn revision(&self, id: &str) -> Result<Option<String>, String> {
        super::credential_access::revision(KEYRING_SERVICE, id).map(Some)
    }
    fn write_if_revision(
        &self,
        id: &str,
        value: &str,
        expected: Option<&str>,
    ) -> Result<Option<String>, String> {
        super::credential_access::write_if_revision(KEYRING_SERVICE, id, value, expected).map(Some)
    }
    fn delete(&self, id: &str, legacy: bool) -> Result<(), String> {
        super::credential_access::delete(
            if legacy {
                LEGACY_KEYRING_SERVICE
            } else {
                KEYRING_SERVICE
            },
            if legacy {
                None
            } else {
                PREVIOUS_KEYRING_SERVICE
            },
            id,
        )
    }
}
pub(super) struct CachedTokens {
    pub revision: Option<String>,
    pub generation: String,
    pub tokens: TokenSet,
}
pub(super) struct AccountCloudClient {
    pub credential_revision: Option<String>,
    pub client: CloudClient,
    pub account_id: String,
    pub generation: String,
}
impl Deref for AccountCloudClient {
    type Target = CloudClient;
    fn deref(&self) -> &CloudClient {
        &self.client
    }
}

impl MailService {
    // Locks coordinate updated services/processes using this same config path.
    // Never hold this across OAuth/browser/provider calls. Vault access is local.
    pub(super) fn lifecycle_lock(&self) -> Result<File, String> {
        let path = self.config_path.with_extension("lock");
        let mut options = fs::OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let file = options
            .open(path)
            .map_err(|_| "Account lifecycle lock unavailable")?;
        if !file
            .metadata()
            .map_err(|_| "Account lifecycle lock unavailable")?
            .is_file()
        {
            return Err("Invalid account lifecycle lock".into());
        }
        file.lock()
            .map_err(|_| "Account lifecycle lock unavailable")?;
        Ok(file)
    }
    pub(super) fn verify_generation(
        &self,
        saved: &SavedConfig,
    ) -> Result<Vec<SavedConfig>, String> {
        let configs = self.load_configs()?;
        if !configs.iter().any(|current| {
            current.account_id == saved.account_id && current.generation == saved.generation
        }) {
            return Err(
                "Account authorization changed; retry from the current account settings".into(),
            );
        }
        Ok(configs)
    }
    pub(super) fn client_for_generation(
        &self,
        saved: &SavedConfig,
        calendar: bool,
    ) -> Result<AccountCloudClient, String> {
        let _lifecycle = self.lifecycle_lock()?;
        self.verify_generation(saved)?;
        let key = if calendar {
            Self::calendar_vault_id(&saved.account_id)
        } else {
            saved.account_id.clone()
        };
        let mut cache = self
            .token_cache
            .lock()
            .map_err(|_| "token cache lock poisoned")?;
        let mut credential_revision = self.credentials.revision(&key)?;
        let tokens = if let Some(cached) = cache.get(&key).filter(|entry| {
            entry.generation == saved.generation && entry.revision == credential_revision
        }) {
            cached.tokens.clone()
        } else {
            // A writer may use another config path for the same OS item.
            // Bracket the read with revisions so a value can never be labeled
            // with a later writer's revision. Migration advances the revision
            // itself, so retry its now-cached result before capturing the fence.
            let mut coherent = None;
            for _ in 0..3 {
                let before = self.credentials.revision(&key)?;
                let value = if calendar {
                    self.credentials
                        .read(&key, false)?
                        .ok_or("Calendar credential unavailable")?
                } else {
                    self.secret(&key)?
                };
                let after = self.credentials.revision(&key)?;
                if before == after {
                    coherent = Some((value, after));
                    break;
                }
            }
            let (value, revision) =
                coherent.ok_or("Credential changed concurrently; retry sync")?;
            credential_revision = revision;
            let tokens: TokenSet =
                serde_json::from_str(&value).map_err(|_| "Account credential invalid")?;
            cache.insert(
                key.clone(),
                CachedTokens {
                    revision: credential_revision.clone(),
                    generation: saved.generation.clone(),
                    tokens: tokens.clone(),
                },
            );
            tokens
        };
        let provider = if calendar {
            CloudProvider::Gmail
        } else {
            match saved.provider {
                Provider::Gmail => CloudProvider::Gmail,
                Provider::Microsoft => CloudProvider::Microsoft,
                _ => return Err("not a cloud account".into()),
            }
        };
        let client = CloudClient::new(
            OAuthConfig {
                provider,
                client_id: saved.client_id.clone().ok_or("OAuth client ID missing")?,
                client_secret: saved.client_secret.clone(),
            },
            tokens,
        )
        .map_err(|e| e.to_string())?;
        Ok(AccountCloudClient {
            credential_revision,
            client,
            account_id: saved.account_id.clone(),
            generation: saved.generation.clone(),
        })
    }
    pub(super) fn commit_client_tokens(
        &self,
        client: &AccountCloudClient,
        calendar: bool,
    ) -> Result<(), String> {
        let _lifecycle = self.lifecycle_lock()?;
        let configs = self.load_configs()?;
        if !configs.iter().any(|saved| {
            saved.account_id == client.account_id
                && saved.generation == client.generation
                && (!calendar || saved.google_calendar_enabled)
        }) {
            return Err(
                "Account authorization changed; refreshed credentials were not saved".into(),
            );
        }
        let key = if calendar {
            Self::calendar_vault_id(&client.account_id)
        } else {
            client.account_id.clone()
        };
        let revision = self.credentials.revision(&key)?;
        if revision != client.credential_revision {
            return Err(
                "Credential changed in another process; refreshed credentials were not saved"
                    .into(),
            );
        }
        let tokens = client.tokens();
        let mut cache = self
            .token_cache
            .lock()
            .map_err(|_| "token cache lock poisoned")?;
        let revision = if cache.get(&key).is_none_or(|old| {
            old.generation != client.generation || old.tokens.refresh_token != tokens.refresh_token
        }) {
            self.credentials.write_if_revision(
                &key,
                &serde_json::to_string(&tokens)
                    .map_err(|_| "Account credential encoding failed")?,
                client.credential_revision.as_deref(),
            )?
        } else {
            revision
        };
        cache.insert(
            key,
            CachedTokens {
                revision,
                generation: client.generation.clone(),
                tokens,
            },
        );
        Ok(())
    }
    pub(super) fn commit_scoped_grant(
        &self,
        saved: &SavedConfig,
        calendar: bool,
        tokens: TokenSet,
    ) -> Result<(), String> {
        let _lifecycle = self.lifecycle_lock()?;
        let mut configs = self.configs.lock().map_err(|_| "config lock poisoned")?;
        let mut next = self.verify_generation(saved)?;
        let key = if calendar {
            Self::calendar_vault_id(&saved.account_id)
        } else {
            saved.account_id.clone()
        };
        let previous = if calendar {
            self.credentials.read(&key, false)?
        } else {
            Some(self.secret(&key)?)
        };
        self.credentials.write(
            &key,
            &serde_json::to_string(&tokens).map_err(|_| "Account credential encoding failed")?,
        )?;
        let item = next
            .iter_mut()
            .find(|item| item.account_id == saved.account_id)
            .ok_or("Account settings missing")?;
        item.generation = Uuid::new_v4().to_string();
        let generation = item.generation.clone();
        if calendar {
            item.google_calendar_enabled = true
        } else {
            item.google_photos_enabled = true
        }
        if let Err(error) = self.save_configs(&next) {
            if let Some(previous) = previous {
                let _ = self.credentials.write(&key, &previous);
            } else {
                let _ = self.credentials.delete(&key, false);
            }
            return Err(error);
        }
        *configs = next;
        self.clear_removed_account_caches(&saved.account_id)?;
        self.token_cache
            .lock()
            .map_err(|_| "token cache lock poisoned")?
            .insert(
                key,
                CachedTokens {
                    revision: None,
                    generation,
                    tokens,
                },
            );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};
    type WriteHook = Arc<dyn Fn(&str, &str) + Send + Sync>;
    #[derive(Default)]
    struct FakeVault {
        values: Mutex<HashMap<(String, bool), String>>,
        writes: Mutex<usize>,
        revisioned: Mutex<bool>,
        fail_delete: Mutex<HashSet<(String, bool)>>,
        after_write: Mutex<Option<WriteHook>>,
        pause_write: Mutex<Option<(Arc<Barrier>, Arc<Barrier>)>>,
    }
    impl CredentialStore for FakeVault {
        fn revision(&self, _id: &str) -> Result<Option<String>, String> {
            Ok(if *self.revisioned.lock().unwrap() {
                Some(self.writes.lock().unwrap().to_string())
            } else {
                None
            })
        }
        fn read(&self, id: &str, legacy: bool) -> Result<Option<String>, String> {
            Ok(self
                .values
                .lock()
                .unwrap()
                .get(&(id.into(), legacy))
                .cloned())
        }
        fn write(&self, id: &str, value: &str) -> Result<(), String> {
            if let Some((entered, release)) = self.pause_write.lock().unwrap().take() {
                entered.wait();
                release.wait();
            }
            *self.writes.lock().unwrap() += 1;
            self.values
                .lock()
                .unwrap()
                .insert((id.into(), false), value.into());
            let hook = self.after_write.lock().unwrap().clone();
            if let Some(hook) = hook {
                hook(id, value);
            }
            Ok(())
        }
        fn delete(&self, id: &str, legacy: bool) -> Result<(), String> {
            if self
                .fail_delete
                .lock()
                .unwrap()
                .contains(&(id.into(), legacy))
            {
                return Err("synthetic vault refusal".into());
            }
            self.values.lock().unwrap().remove(&(id.into(), legacy));
            Ok(())
        }
    }
    struct Fixture {
        dir: PathBuf,
        service: Arc<MailService>,
        vault: Arc<FakeVault>,
        revoke_endpoint: String,
    }
    impl Fixture {
        /// Removal tries to revoke Google grants; by default it reaches only a
        /// closed local port, never Google.
        fn new() -> Self {
            Self::with_revoke_endpoint(closed_local_url())
        }
        fn with_revoke_endpoint(revoke_endpoint: String) -> Self {
            let dir = std::env::temp_dir().join(format!("email-lifecycle-{}", Uuid::new_v4()));
            fs::create_dir_all(&dir).unwrap();
            let vault = Arc::new(FakeVault::default());
            let mut service = MailService::open(dir.join("mail.sqlite")).unwrap();
            service.credentials = vault.clone();
            service.google_revoke_endpoint = Some(revoke_endpoint.clone());
            Self {
                dir,
                service: Arc::new(service),
                vault,
                revoke_endpoint,
            }
        }
        fn add(&self, id: &str, label: &str) -> SavedConfig {
            let account = Account {
                id: id.into(),
                provider: Provider::Gmail,
                email: format!("{id}@example.invalid"),
                display_name: label.into(),
            };
            let saved:SavedConfig=serde_json::from_value(json!({"account_id":id,"provider":"gmail","client_id":"synthetic-client","google_calendar_enabled":true,"google_photos_enabled":true})).unwrap();
            let mail_tokens = tokens(label);
            self.service
                .persist_account(
                    &account,
                    saved,
                    &serde_json::to_string(&mail_tokens).unwrap(),
                    Some(mail_tokens),
                )
                .unwrap();
            self.vault
                .write(
                    &MailService::calendar_vault_id(id),
                    &serde_json::to_string(&tokens("calendar-initial")).unwrap(),
                )
                .unwrap();
            self.service.saved_config(id).unwrap()
        }
        fn add_other(&self, id: &str, provider: &str, secret: &str) {
            let account = Account {
                id: id.into(),
                provider: serde_json::from_value(json!(provider)).unwrap(),
                email: format!("{id}@example.invalid"),
                display_name: id.into(),
            };
            let saved: SavedConfig = serde_json::from_value(json!({"account_id":id,"provider":provider,
                "client_id":"synthetic-client","username":account.email,"smtp_username":account.email,
                "imap_host":"imap.example.invalid","smtp_host":"smtp.example.invalid"}))
            .unwrap();
            self.service
                .persist_account(&account, saved, secret, None)
                .unwrap();
        }
        fn second_service(&self) -> MailService {
            let mut service = MailService::open(self.dir.join("mail.sqlite")).unwrap();
            service.credentials = self.vault.clone();
            service.google_revoke_endpoint = Some(self.revoke_endpoint.clone());
            service
        }
    }
    fn closed_local_url() -> String {
        let closed = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        format!("http://{closed}/revoke")
    }
    /// A local stand-in for Google's revoke endpoint. It records each raw
    /// request and answers all of them with the same response.
    struct RevokeServer {
        url: String,
        requests: Arc<Mutex<Vec<String>>>,
        stop: Arc<std::sync::atomic::AtomicBool>,
        thread: Option<std::thread::JoinHandle<()>>,
    }
    impl RevokeServer {
        fn start(response: &'static str) -> Self {
            use std::io::{Read, Write};
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let url = format!("http://{}/revoke", listener.local_addr().unwrap());
            let requests = Arc::new(Mutex::new(Vec::new()));
            let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let (seen, stopped) = (requests.clone(), stop.clone());
            let thread = std::thread::spawn(move || {
                while !stopped.load(Ordering::Acquire) {
                    let Ok((mut stream, _)) = listener.accept() else {
                        std::thread::sleep(Duration::from_millis(5));
                        continue;
                    };
                    stream.set_nonblocking(false).unwrap();
                    let mut request = Vec::new();
                    let mut buffer = [0u8; 2048];
                    loop {
                        let n = stream.read(&mut buffer).unwrap();
                        if n == 0 {
                            break;
                        }
                        request.extend_from_slice(&buffer[..n]);
                        let text = String::from_utf8_lossy(&request).to_string();
                        let Some((head, body)) = text.split_once("\r\n\r\n") else {
                            continue;
                        };
                        let length = head
                            .to_ascii_lowercase()
                            .lines()
                            .find_map(|line| line.strip_prefix("content-length:"))
                            .and_then(|value| value.trim().parse().ok())
                            .unwrap_or(0usize);
                        if body.len() >= length {
                            break;
                        }
                    }
                    seen.lock()
                        .unwrap()
                        .push(String::from_utf8(request).unwrap());
                    let _ = stream.write_all(response.as_bytes());
                }
            });
            Self {
                url,
                requests,
                stop,
                thread: Some(thread),
            }
        }
        fn requests(&self) -> Vec<String> {
            self.requests.lock().unwrap().clone()
        }
    }
    impl Drop for RevokeServer {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Release);
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }
    const REVOKED: &str = "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
    fn assert_removed(fixture: &Fixture, id: &str) {
        assert!(fixture.service.saved_config(id).is_err());
        assert!(fixture.vault.read(id, false).unwrap().is_none());
        assert!(
            fixture
                .vault
                .read(&MailService::calendar_vault_id(id), false)
                .unwrap()
                .is_none()
        );
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }
    fn tokens(label: &str) -> TokenSet {
        TokenSet {
            access_token: format!("synthetic-access-{label}"),
            refresh_token: Some(format!("synthetic-refresh-{label}")),
            expires_at_ms: i64::MAX,
        }
    }
    fn captured(saved: &SavedConfig, label: &str) -> AccountCloudClient {
        AccountCloudClient {
            credential_revision: None,
            account_id: saved.account_id.clone(),
            generation: saved.generation.clone(),
            client: CloudClient::new(
                OAuthConfig {
                    provider: CloudProvider::Gmail,
                    client_id: "synthetic-client".into(),
                    client_secret: None,
                },
                tokens(label),
            )
            .unwrap(),
        }
    }
    #[test]
    fn revision_change_invalidates_cached_tokens_and_fences_old_refresh() {
        let fixture = Fixture::new();
        let saved = fixture.add("account", "old");
        *fixture.vault.revisioned.lock().unwrap() = true;
        let old = fixture.service.cloud_client(&saved).unwrap();
        fixture
            .vault
            .write("account", &serde_json::to_string(&tokens("new")).unwrap())
            .unwrap();
        assert!(fixture.service.save_cloud_tokens(&saved, &old).is_err());
        let current = fixture.service.cloud_client(&saved).unwrap();
        assert_eq!(current.tokens().access_token, "synthetic-access-new");
    }
    #[test]
    fn same_refresh_token_keeps_new_access_token_in_revision_bound_memory_cache() {
        let fixture = Fixture::new();
        let saved = fixture.add("account", "old");
        *fixture.vault.revisioned.lock().unwrap() = true;
        let original = fixture.service.cloud_client(&saved).unwrap();
        let mut renewed = original.tokens();
        renewed.access_token = "synthetic-new-access".into();
        let client = AccountCloudClient {
            credential_revision: original.credential_revision,
            generation: saved.generation.clone(),
            account_id: saved.account_id.clone(),
            client: CloudClient::new(
                OAuthConfig {
                    provider: CloudProvider::Gmail,
                    client_id: "synthetic-client".into(),
                    client_secret: None,
                },
                renewed,
            )
            .unwrap(),
        };
        let before = *fixture.vault.writes.lock().unwrap();
        fixture.service.save_cloud_tokens(&saved, &client).unwrap();
        assert_eq!(*fixture.vault.writes.lock().unwrap(), before);
        assert_eq!(
            fixture
                .service
                .cloud_client(&saved)
                .unwrap()
                .tokens()
                .access_token,
            "synthetic-new-access"
        );
    }
    #[test]
    fn stale_refresh_calendar_and_scope_callbacks_cannot_restore_removed_or_readded_credentials() {
        let fixture = Fixture::new();
        let saved = fixture.add("removed", "old");
        let kept = fixture.add("kept", "kept");
        let mail = captured(&saved, "stale-mail");
        let calendar = captured(&saved, "stale-calendar");
        fixture
            .vault
            .values
            .lock()
            .unwrap()
            .insert(("removed".into(), true), "legacy-secret".into());
        fixture.service.remove_account("removed").unwrap();
        assert!(fixture.service.save_cloud_tokens(&saved, &mail).is_err());
        assert!(
            fixture
                .service
                .commit_client_tokens(&calendar, true)
                .is_err()
        );
        assert!(fixture.vault.read("removed", false).unwrap().is_none());
        assert!(fixture.vault.read("removed", true).unwrap().is_none());
        let current = fixture.add("removed", "new");
        assert_ne!(current.generation, saved.generation);
        let before = *fixture.vault.writes.lock().unwrap();
        assert!(fixture.service.save_cloud_tokens(&saved, &mail).is_err());
        assert!(
            fixture
                .service
                .commit_client_tokens(&calendar, true)
                .is_err()
        );
        for calendar in [false, true] {
            assert!(
                fixture
                    .service
                    .commit_scoped_grant(&saved, calendar, tokens("stale-scope"))
                    .is_err()
            );
        }
        assert!(fixture.service.cloud_client(&saved).is_err());
        assert_eq!(*fixture.vault.writes.lock().unwrap(), before);
        let active = fixture.service.cloud_client(&current).unwrap();
        assert_eq!(active.tokens().access_token, "synthetic-access-new");
        assert!(
            fixture
                .service
                .save_cloud_tokens(&kept, &captured(&kept, "kept-refreshed"))
                .is_ok()
        );
        assert_eq!(
            fixture
                .service
                .token_cache
                .lock()
                .unwrap()
                .get("removed")
                .unwrap()
                .generation,
            current.generation
        );
    }
    #[test]
    fn another_handle_rejects_old_generations_and_does_not_reuse_its_stale_cache() {
        let fixture = Fixture::new();
        let old = fixture.add("account", "old");
        let second = fixture.second_service();
        let old_client = second.cloud_client(&old).unwrap();
        assert_eq!(old_client.tokens().access_token, "synthetic-access-old");
        fixture.service.remove_account("account").unwrap();
        let current = fixture.add("account", "new");
        assert!(second.save_cloud_tokens(&old, &old_client).is_err());
        assert!(second.cloud_client(&old).is_err());
        let current_client = second
            .cloud_client(&second.saved_config("account").unwrap())
            .unwrap();
        assert_eq!(current_client.generation, current.generation);
        assert_eq!(current_client.tokens().access_token, "synthetic-access-new");
    }
    #[test]
    fn refresh_local_commit_and_removal_are_serialized_without_provider_locks() {
        let fixture = Fixture::new();
        let saved = fixture.add("account", "old");
        let client = captured(&saved, "refreshed");
        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        *fixture.vault.pause_write.lock().unwrap() = Some((entered.clone(), release.clone()));
        let service = fixture.service.clone();
        let writer = std::thread::spawn(move || service.save_cloud_tokens(&saved, &client));
        entered.wait();
        // An independently opened descriptor observes the same exclusive lock;
        // updated processes cannot remove/readd while this local commit runs.
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(fixture.service.config_path.with_extension("lock"))
            .unwrap();
        assert!(lock.try_lock().is_err());
        let service = fixture.service.clone();
        let remover = std::thread::spawn(move || service.remove_account("account"));
        release.wait();
        writer.join().unwrap().unwrap();
        remover.join().unwrap().unwrap();
        assert!(fixture.vault.read("account", false).unwrap().is_none());
        assert!(
            fixture
                .service
                .token_cache
                .lock()
                .unwrap()
                .get("account")
                .is_none()
        );
    }
    #[test]
    fn scope_upgrade_rotates_generation_before_old_mail_or_calendar_callbacks_can_save() {
        for calendar in [false, true] {
            let fixture = Fixture::new();
            let saved = fixture.add("account", "old");
            let old = captured(&saved, "old-response");
            fixture
                .service
                .commit_scoped_grant(&saved, calendar, tokens("new-scope"))
                .unwrap();
            let current = fixture.service.saved_config("account").unwrap();
            assert_ne!(saved.generation, current.generation);
            let before = *fixture.vault.writes.lock().unwrap();
            assert!(fixture.service.save_cloud_tokens(&saved, &old).is_err());
            assert!(fixture.service.commit_client_tokens(&old, true).is_err());
            assert_eq!(*fixture.vault.writes.lock().unwrap(), before);
            let active = fixture
                .service
                .client_for_generation(&current, calendar)
                .unwrap();
            assert_eq!(active.tokens().access_token, "synthetic-access-new-scope");
        }
    }
    #[test]
    fn failed_vault_delete_does_not_allow_old_callbacks_to_repopulate_caches() {
        let fixture = Fixture::new();
        let saved = fixture.add("account", "old");
        let old = captured(&saved, "stale");
        fixture
            .vault
            .fail_delete
            .lock()
            .unwrap()
            .insert(("account".into(), false));
        assert!(fixture.service.remove_account("account").is_err());
        assert!(fixture.service.saved_config("account").is_err());
        assert!(
            fixture
                .vault
                .read(&MailService::calendar_vault_id("account"), false)
                .unwrap()
                .is_none()
        );
        let before = *fixture.vault.writes.lock().unwrap();
        assert!(fixture.service.save_cloud_tokens(&saved, &old).is_err());
        assert!(
            fixture
                .service
                .commit_scoped_grant(&saved, false, tokens("late"))
                .is_err()
        );
        assert_eq!(*fixture.vault.writes.lock().unwrap(), before);
        assert!(
            fixture
                .service
                .token_cache
                .lock()
                .unwrap()
                .get("account")
                .is_none()
        );
    }
    #[test]
    fn failed_scope_config_commit_rolls_back_before_waiting_removal_can_delete_credentials() {
        let fixture = Fixture::new();
        let saved = fixture.add("account", "old");
        let config = fixture.service.config_path.clone();
        let backup = config.with_extension("test-backup");
        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let hook_entered = entered.clone();
        let hook_release = release.clone();
        *fixture.vault.after_write.lock().unwrap() = Some(Arc::new(move |id, value| {
            if id != "account" {
                return;
            }
            if value.contains("synthetic-access-new-scope") {
                fs::rename(&config, &backup).unwrap();
                fs::create_dir(&config).unwrap();
                hook_entered.wait();
                hook_release.wait();
            } else if backup.exists() {
                fs::remove_dir(&config).unwrap();
                fs::rename(&backup, &config).unwrap();
            }
        }));
        let service = fixture.service.clone();
        let upgrade = std::thread::spawn(move || {
            service.commit_scoped_grant(&saved, false, tokens("new-scope"))
        });
        entered.wait();
        let service = fixture.service.clone();
        let removal = std::thread::spawn(move || service.remove_account("account"));
        release.wait();
        assert!(upgrade.join().unwrap().is_err());
        removal.join().unwrap().unwrap();
        assert!(fixture.vault.read("account", false).unwrap().is_none());
        assert!(fixture.service.saved_config("account").is_err());
    }
    #[test]
    fn removing_a_google_sign_in_account_revokes_its_grant_with_the_stored_token() {
        let google = RevokeServer::start(REVOKED);
        let fixture = Fixture::with_revoke_endpoint(google.url.clone());
        fixture.add("account", "old");
        fixture.service.remove_account("account").unwrap();
        // The token is read from the vault, so this request also shows the
        // revoke ran before removal deleted the credentials.
        let requests = google.requests();
        assert_eq!(requests.len(), 1, "{requests:?}");
        let (head, body) = requests[0].split_once("\r\n\r\n").unwrap();
        assert!(head.starts_with("POST /revoke HTTP/1.1\r\n"), "{head}");
        assert!(
            head.to_ascii_lowercase()
                .contains("content-type: application/x-www-form-urlencoded")
        );
        assert!(!head.contains("synthetic"), "token outside the body: {head}");
        assert_eq!(body, "token=synthetic-refresh-old");
        assert_removed(&fixture, "account");
    }
    #[test]
    fn removal_finishes_when_google_revoke_fails_or_is_unreachable() {
        let failing = RevokeServer::start(
            "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
        for endpoint in [failing.url.clone(), closed_local_url()] {
            let fixture = Fixture::with_revoke_endpoint(endpoint);
            fixture.add("account", "old");
            fixture.service.remove_account("account").unwrap();
            assert_removed(&fixture, "account");
        }
        assert_eq!(failing.requests().len(), 1);
    }
    #[test]
    fn app_password_and_microsoft_accounts_are_removed_without_google_revoke() {
        let google = RevokeServer::start(REVOKED);
        let fixture = Fixture::with_revoke_endpoint(google.url.clone());
        fixture.add_other("imap", "imap", "abcdefghijklmnop");
        fixture.add_other(
            "microsoft",
            "microsoft",
            &serde_json::to_string(&tokens("microsoft")).unwrap(),
        );
        for id in ["imap", "microsoft"] {
            fixture.service.remove_account(id).unwrap();
            assert_removed(&fixture, id);
        }
        assert!(google.requests().is_empty());
    }
}
