//! macOS prompt budget and coherent, memory-only credential cache.
//! The coordinator contains random revisions only, never credentials or account IDs.
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::PathBuf,
    sync::Mutex,
};

pub(super) trait Backend {
    fn read(&self, service: &str, id: &str, interactive: bool) -> Result<Option<String>, String>;
    fn write(&self, service: &str, id: &str, value: &str, interactive: bool) -> Result<(), String>;
    fn delete(&self, service: &str, id: &str, interactive: bool) -> Result<(), String>;
}
#[derive(Default)]
struct State {
    values: HashMap<String, (String, String)>,
    attempted: HashSet<String>,
    blocked: bool,
}
pub(super) struct Access {
    root: PathBuf,
    state: Mutex<State>,
}
impl Access {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            state: Mutex::new(State::default()),
        }
    }
    fn key(service: &str, id: &str) -> String {
        format!("{:x}", Sha256::digest(format!("{service}\0{id}")))
    }
    fn file(&self, name: &str) -> Result<File, String> {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(self.root.join(name))
            .map_err(|_| "Credential coordinator unavailable")?;
        let metadata = file
            .metadata()
            .map_err(|_| "Credential coordinator unavailable")?;
        if !metadata.is_file()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o077 != 0
        {
            return Err("Unsafe credential coordinator file".into());
        }
        Ok(file)
    }
    fn lock(&self) -> Result<File, String> {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt};
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.root)
            .map_err(|_| "Credential coordinator unavailable")?;
        let metadata =
            fs::symlink_metadata(&self.root).map_err(|_| "Credential coordinator unavailable")?;
        if !metadata.is_dir()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o077 != 0
        {
            return Err("Unsafe credential coordinator directory".into());
        }
        let file = self.file("lock")?;
        file.lock()
            .map_err(|_| "Credential coordinator unavailable")?;
        Ok(file)
    }
    fn revision(&self, key: &str) -> Result<String, String> {
        let mut revision = String::new();
        self.file(key)?
            .take(128)
            .read_to_string(&mut revision)
            .map_err(|_| "Credential revision unavailable")?;
        Ok(revision)
    }
    fn advance(&self, key: &str) -> Result<String, String> {
        let revision = uuid::Uuid::new_v4().to_string();
        let mut file = self.file(key)?;
        file.set_len(0)
            .map_err(|_| "Credential revision unavailable")?;
        file.write_all(revision.as_bytes())
            .and_then(|_| file.sync_all())
            .map_err(|_| "Credential revision unavailable")?;
        Ok(revision)
    }
    #[cfg(test)]
    pub fn read(
        &self,
        backend: &impl Backend,
        service: &str,
        previous: Option<&str>,
        id: &str,
    ) -> Result<Option<String>, String> {
        self.read_fallbacks(
            backend,
            service,
            &previous.into_iter().collect::<Vec<_>>(),
            id,
        )
    }
    pub fn read_fallbacks(
        &self,
        backend: &impl Backend,
        service: &str,
        previous: &[&str],
        id: &str,
    ) -> Result<Option<String>, String> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| "Credential cache unavailable")?;
        let _lock = self.lock()?;
        let key = Self::key(service, id);
        let revision = self.revision(&key)?;
        if let Some((cached_revision, value)) = state.values.get(&key) {
            if *cached_revision == revision {
                return Ok(Some(value.clone()));
            }
        }
        state.values.remove(&key);
        // Probe silently after the first attempt, including after denial, lock,
        // or a cross-process change. Unlocks/new approvals can recover without
        // relaunching, but background polling cannot ask again.
        let interactive = !state.blocked && state.attempted.insert(key.clone());
        let result = backend.read(service, id, interactive).and_then(|value| {
            if value.is_some() {
                return Ok((value, revision.clone()));
            }
            let mut value = None;
            for previous in previous {
                value = backend.read(previous, id, interactive)?;
                if value.is_some() {
                    break;
                }
            }
            let mut revision = revision.clone();
            if let Some(value) = &value {
                // Invalidate other processes before touching the OS store, even
                // if the write fails or this process crashes during mutation.
                let persisted = match self.advance(&key) {
                    Ok(next) => {
                        revision = next;
                        backend.write(service, id, value, interactive)
                            .and_then(|()| match backend.read(service, id, false)? {
                                Some(saved) if saved == *value => Ok(()),
                                _ => Err("Credential migration could not be verified".into()),
                            }).is_ok()
                    }
                    Err(_) => {
                        if let Ok(observed) = self.revision(&key) { revision = observed; }
                        false
                    },
                };
                if !persisted {
                    state.blocked = true;
                    // Preserve the authorized read for this process. Never turn
                    // an authorized read into repeated prompts or lost sync.
                    eprintln!("Email: credential migration could not be persisted; authorized value retained in memory. Retry migration after resolving secure storage access.");
                }
            }
            Ok((value, revision))
        });
        match result {
            Ok((value, revision)) => {
                if let Some(value) = &value {
                    state.values.insert(key, (revision, value.clone()));
                }
                Ok(value)
            }
            Err(error) => {
                state.blocked = true;
                Err(error)
            }
        }
    }
    pub fn revision_for(&self, service: &str, id: &str) -> Result<String, String> {
        let _lock = self.lock()?;
        self.revision(&Self::key(service, id))
    }
    pub fn operation_lock(&self) -> Result<File, String> {
        let file = {
            let _setup = self.lock()?;
            self.file("booking-operation")?
        };
        // Never wait for booking while holding the credential lock: an active
        // booking operation may itself need to read/write its credential.
        file.lock().map_err(|_| "Booking connection is busy")?;
        Ok(file)
    }
    pub fn write(
        &self,
        backend: &impl Backend,
        service: &str,
        id: &str,
        value: &str,
    ) -> Result<(), String> {
        self.write_if_revision(backend, service, id, value, None)
            .map(|_| ())
    }
    pub fn write_if_revision(
        &self,
        backend: &impl Backend,
        service: &str,
        id: &str,
        value: &str,
        expected: Option<&str>,
    ) -> Result<String, String> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| "Credential cache unavailable")?;
        let _lock = self.lock()?;
        let key = Self::key(service, id);
        if let Some(expected) = expected {
            if self.revision(&key)? != expected {
                return Err(
                    "Credential changed in another process; refreshed credentials were not saved"
                        .into(),
                );
            }
        }
        state.values.remove(&key);
        let revision = self.advance(&key)?;
        let interactive = !state.blocked && state.attempted.insert(key.clone());
        if let Err(error) = backend.write(service, id, value, interactive) {
            state.blocked = true;
            return Err(error);
        }
        state.values.insert(key, (revision.clone(), value.into()));
        Ok(revision)
    }
    #[cfg(test)]
    pub fn delete(
        &self,
        backend: &impl Backend,
        service: &str,
        previous: Option<&str>,
        id: &str,
    ) -> Result<(), String> {
        self.delete_fallbacks(
            backend,
            service,
            &previous.into_iter().collect::<Vec<_>>(),
            id,
        )
    }
    fn delete_fallbacks(
        &self,
        backend: &impl Backend,
        service: &str,
        previous: &[&str],
        id: &str,
    ) -> Result<(), String> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| "Credential cache unavailable")?;
        let _lock = self.lock()?;
        let key = Self::key(service, id);
        state.values.remove(&key);
        self.advance(&key)?;
        let interactive = !state.blocked && state.attempted.insert(key);
        // Previous first: a partial deletion must never expose an older value
        // through the fallback after the current item was successfully removed.
        let result = previous
            .iter()
            .try_for_each(|previous| backend.delete(previous, id, interactive))
            .and_then(|_| backend.delete(service, id, interactive));
        if result.is_err() {
            state.blocked = true;
        }
        result
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    struct Keyring;
    // security-framework's guard always enables UI on drop. Preserve the prior
    // process policy instead, especially in noninteractive MCP processes.
    #[link(name = "Security", kind = "framework")]
    unsafe extern "C" {
        fn SecKeychainGetUserInteractionAllowed(allowed: *mut u8) -> i32;
        fn SecKeychainSetUserInteractionAllowed(allowed: u8) -> i32;
    }
    struct Interaction(u8);
    impl Drop for Interaction {
        fn drop(&mut self) {
            unsafe {
                SecKeychainSetUserInteractionAllowed(self.0);
            }
        }
    }
    fn interaction(interactive: bool) -> Result<Interaction, String> {
        let mut prior = 0;
        if unsafe { SecKeychainGetUserInteractionAllowed(&mut prior) } != 0 {
            return Err("Keychain interaction policy unavailable".into());
        }
        if !interactive && unsafe { SecKeychainSetUserInteractionAllowed(0) } != 0 {
            return Err("Keychain interaction policy unavailable".into());
        }
        Ok(Interaction(prior))
    }
    impl Backend for Keyring {
        fn read(
            &self,
            service: &str,
            id: &str,
            interactive: bool,
        ) -> Result<Option<String>, String> {
            let _policy = interaction(interactive)?;
            match keyring::Entry::new(service, id)
                .map_err(|e| e.to_string())?
                .get_password()
            {
                Ok(value) => Ok(Some(value)),
                Err(keyring::Error::NoEntry) => Ok(None),
                Err(error) => Err(error.to_string()),
            }
        }
        fn write(
            &self,
            service: &str,
            id: &str,
            value: &str,
            interactive: bool,
        ) -> Result<(), String> {
            let mut allowed = 0;
            if unsafe { SecKeychainGetUserInteractionAllowed(&mut allowed) } != 0 {
                return Err("Keychain interaction policy unavailable".into());
            }
            let entry = keyring::Entry::new(service, id).map_err(|e| e.to_string())?;
            if allowed == 0 {
                // Do not let an unattended MCP/dispatcher create the migration
                // destination under its different development code partition.
                entry
                    .get_password()
                    .map_err(|_| "Open Email to initialize credential storage".to_string())?;
            }
            let _policy = interaction(interactive)?;
            entry.set_password(value).map_err(|e| e.to_string())
        }
        fn delete(&self, service: &str, id: &str, interactive: bool) -> Result<(), String> {
            let _policy = interaction(interactive)?;
            match keyring::Entry::new(service, id)
                .map_err(|e| e.to_string())?
                .delete_credential()
            {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(error) => Err(error.to_string()),
            }
        }
    }
    fn access() -> Result<&'static Access, String> {
        static ACCESS: std::sync::OnceLock<Access> = std::sync::OnceLock::new();
        if let Some(access) = ACCESS.get() {
            return Ok(access);
        }
        let home = std::env::var_os("HOME").ok_or("Credential coordinator home unavailable")?;
        Ok(ACCESS.get_or_init(|| {
            Access::new(
                PathBuf::from(home)
                    .join("Library/Application Support")
                    .join(crate::profile::MACOS_DATA_DIRECTORY)
                    .join("credential-state-v1"),
            )
        }))
    }
    // Only the new macOS namespace has this history. Never import mail-only
    // oldest items for calendar or Scheduling identifiers.
    fn fallbacks<'a>(service: &str, previous: Option<&'a str>) -> Vec<&'a str> {
        let mut services: Vec<_> = previous.into_iter().collect();
        if service == super::super::KEYRING_SERVICE {
            services.push(super::super::SECOND_KEYRING_SERVICE);
        }
        services
    }
    pub(crate) fn read(
        service: &str,
        previous: Option<&str>,
        id: &str,
    ) -> Result<Option<String>, String> {
        access()?.read_fallbacks(&Keyring, service, &fallbacks(service, previous), id)
    }
    pub(crate) fn read_mail(id: &str) -> Result<Option<String>, String> {
        access()?.read_fallbacks(
            &Keyring,
            super::super::KEYRING_SERVICE,
            &[
                super::super::PREVIOUS_KEYRING_SERVICE.expect("macOS previous service"),
                super::super::SECOND_KEYRING_SERVICE,
                super::super::LEGACY_KEYRING_SERVICE,
            ],
            id,
        )
    }
    pub(crate) fn write(service: &str, id: &str, value: &str) -> Result<(), String> {
        access()?.write(&Keyring, service, id, value)
    }
    #[test]
    fn current_namespace_retains_v3_v2_history_without_legacy_mail_for_calendar() {
        let namespace = crate::profile::KEYCHAIN_NAMESPACE;
        assert_eq!(super::super::KEYRING_SERVICE, format!("{namespace}.v4"));
        assert_eq!(
            fallbacks(
                super::super::KEYRING_SERVICE,
                super::super::PREVIOUS_KEYRING_SERVICE
            ),
            vec![format!("{namespace}.v3"), format!("{namespace}.v2")]
        );
        assert!(fallbacks(super::super::LEGACY_KEYRING_SERVICE, None).is_empty());
    }
    #[test]
    fn interaction_scope_preserves_noninteractive_mcp_policy() {
        let original = interaction(false).unwrap();
        {
            let _nested = interaction(false).unwrap();
        }
        let mut allowed = 1;
        assert_eq!(
            unsafe { SecKeychainGetUserInteractionAllowed(&mut allowed) },
            0
        );
        assert_eq!(allowed, 0);
        let prior = original.0;
        drop(original);
        assert_eq!(
            unsafe { SecKeychainGetUserInteractionAllowed(&mut allowed) },
            0
        );
        assert_eq!(allowed, prior);
    }
    pub(crate) fn revision(service: &str, id: &str) -> Result<String, String> {
        access()?.revision_for(service, id)
    }
    pub(crate) fn write_if_revision(
        service: &str,
        id: &str,
        value: &str,
        expected: Option<&str>,
    ) -> Result<String, String> {
        access()?.write_if_revision(&Keyring, service, id, value, expected)
    }
    pub(crate) fn booking_lock() -> Result<File, String> {
        access()?.operation_lock()
    }
    pub(crate) fn delete(service: &str, previous: Option<&str>, id: &str) -> Result<(), String> {
        access()?.delete_fallbacks(&Keyring, service, &fallbacks(service, previous), id)
    }
}
#[cfg(target_os = "macos")]
pub(super) use macos::{booking_lock, delete, read, read_mail, revision, write, write_if_revision};

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};
    #[derive(Default)]
    struct Vault {
        values: Mutex<HashMap<(String, String), String>>,
        calls: Mutex<Vec<(String, bool)>>,
        locked: Mutex<bool>,
        fail_write: Mutex<bool>,
        fail_delete: Mutex<Option<String>>,
    }
    impl Backend for Vault {
        fn read(
            &self,
            service: &str,
            id: &str,
            interactive: bool,
        ) -> Result<Option<String>, String> {
            self.calls
                .lock()
                .unwrap()
                .push((format!("read:{service}:{id}"), interactive));
            if *self.locked.lock().unwrap() {
                return Err("synthetic lock or denial".into());
            }
            Ok(self
                .values
                .lock()
                .unwrap()
                .get(&(service.into(), id.into()))
                .cloned())
        }
        fn write(
            &self,
            service: &str,
            id: &str,
            value: &str,
            interactive: bool,
        ) -> Result<(), String> {
            self.calls
                .lock()
                .unwrap()
                .push((format!("write:{service}:{id}"), interactive));
            if *self.fail_write.lock().unwrap() {
                return Err("synthetic persistence failure".into());
            }
            self.values
                .lock()
                .unwrap()
                .insert((service.into(), id.into()), value.into());
            Ok(())
        }
        fn delete(&self, service: &str, id: &str, interactive: bool) -> Result<(), String> {
            self.calls
                .lock()
                .unwrap()
                .push((format!("delete:{service}:{id}"), interactive));
            if self.fail_delete.lock().unwrap().as_deref() == Some(service) {
                return Err("synthetic deletion failure".into());
            }
            self.values
                .lock()
                .unwrap()
                .remove(&(service.into(), id.into()));
            Ok(())
        }
    }
    struct Fixture {
        root: PathBuf,
        vault: Vault,
        access: Access,
    }
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir()
                .join(format!("email-credential-test-{}", uuid::Uuid::new_v4()));
            Self {
                access: Access::new(root.clone()),
                root,
                vault: Vault::default(),
            }
        }
        fn seed(&self, service: &str, id: &str, value: &str) {
            self.vault
                .values
                .lock()
                .unwrap()
                .insert((service.into(), id.into()), value.into());
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
    #[test]
    fn concurrent_success_and_failed_migration_read_authorized_value_once() {
        let f = Fixture::new();
        f.seed("old", "account", "synthetic");
        *f.vault.fail_write.lock().unwrap() = true;
        let barrier = Arc::new(Barrier::new(8));
        std::thread::scope(|scope| {
            for _ in 0..8 {
                let barrier = barrier.clone();
                let f = &f;
                scope.spawn(move || {
                    barrier.wait();
                    assert_eq!(
                        f.access
                            .read(&f.vault, "new", Some("old"), "account")
                            .unwrap()
                            .as_deref(),
                        Some("synthetic")
                    );
                });
            }
        });
        let calls = f.vault.calls.lock().unwrap();
        assert_eq!(calls.len(), 3); // current, old, one failed copy
        assert!(
            !f.vault
                .values
                .lock()
                .unwrap()
                .contains_key(&("new".into(), "account".into()))
        );
    }
    #[test]
    fn oldest_mail_namespace_is_adopted_once_even_when_persistence_fails() {
        let f = Fixture::new();
        f.seed("v1", "mail", "authorized-v1");
        *f.vault.fail_write.lock().unwrap() = true;
        for _ in 0..3 {
            assert_eq!(
                f.access
                    .read_fallbacks(&f.vault, "v3", &["v2", "v1"], "mail")
                    .unwrap()
                    .as_deref(),
                Some("authorized-v1")
            );
        }
        assert_eq!(f.vault.calls.lock().unwrap().len(), 4);
    }
    #[test]
    fn migration_failure_stops_later_dialogs_without_disabling_authorized_sync() {
        let f = Fixture::new();
        f.seed("old", "mail", "authorized");
        *f.vault.fail_write.lock().unwrap() = true;
        assert_eq!(
            f.access
                .read(&f.vault, "new", Some("old"), "mail")
                .unwrap()
                .as_deref(),
            Some("authorized")
        );
        f.access.read(&f.vault, "new", None, "calendar").unwrap();
        assert!(!f.vault.calls.lock().unwrap().last().unwrap().1);
        *f.vault.locked.lock().unwrap() = true;
        assert_eq!(
            f.access
                .read(&f.vault, "new", Some("old"), "mail")
                .unwrap()
                .as_deref(),
            Some("authorized")
        );
    }
    #[test]
    fn migration_success_is_cached_and_survives_fresh_reader() {
        let f = Fixture::new();
        f.seed("old", "account", "synthetic");
        assert_eq!(
            f.access
                .read(&f.vault, "new", Some("old"), "account")
                .unwrap()
                .as_deref(),
            Some("synthetic")
        );
        let fresh = Access::new(f.root.clone());
        assert_eq!(
            fresh
                .read(&f.vault, "new", Some("old"), "account")
                .unwrap()
                .as_deref(),
            Some("synthetic")
        );
        assert_eq!(
            f.vault
                .calls
                .lock()
                .unwrap()
                .iter()
                .filter(|(op, _)| op == "read:old:account")
                .count(),
            1
        );
    }
    #[test]
    fn historical_copy_is_verified_without_a_second_interactive_read() {
        let f = Fixture::new();
        f.seed("v3", "mail", "authorized-v3");
        f.seed("v2", "mail", "stale-v2");
        assert_eq!(
            f.access
                .read_fallbacks(&f.vault, "v4", &["v3", "v2"], "mail")
                .unwrap()
                .as_deref(),
            Some("authorized-v3")
        );
        let calls = f.vault.calls.lock().unwrap().clone();
        assert_eq!(
            calls,
            vec![
                ("read:v4:mail".into(), true),
                ("read:v3:mail".into(), true),
                ("write:v4:mail".into(), true),
                ("read:v4:mail".into(), false),
            ]
        );
        assert_eq!(
            f.vault
                .values
                .lock()
                .unwrap()
                .get(&("v3".into(), "mail".into()))
                .map(String::as_str),
            Some("authorized-v3")
        );
        let fresh = Access::new(f.root.clone());
        assert_eq!(
            fresh
                .read_fallbacks(&f.vault, "v4", &["v3", "v2"], "mail")
                .unwrap()
                .as_deref(),
            Some("authorized-v3")
        );
        assert_eq!(f.vault.calls.lock().unwrap().len(), 5);
    }
    #[test]
    fn failed_silent_readback_keeps_authorized_value_without_claiming_migration() {
        struct Unverified(Vault);
        impl Backend for Unverified {
            fn read(
                &self,
                service: &str,
                id: &str,
                interactive: bool,
            ) -> Result<Option<String>, String> {
                if service == "v4" && !interactive {
                    return Err("synthetic silent read denied".into());
                }
                self.0.read(service, id, interactive)
            }
            fn write(
                &self,
                service: &str,
                id: &str,
                value: &str,
                interactive: bool,
            ) -> Result<(), String> {
                self.0.write(service, id, value, interactive)
            }
            fn delete(&self, service: &str, id: &str, interactive: bool) -> Result<(), String> {
                self.0.delete(service, id, interactive)
            }
        }
        let f = Fixture::new();
        f.seed("v3", "mail", "authorized-v3");
        let vault = Unverified(Vault::default());
        vault
            .0
            .values
            .lock()
            .unwrap()
            .insert(("v3".into(), "mail".into()), "authorized-v3".into());
        for _ in 0..2 {
            assert_eq!(
                f.access
                    .read(&vault, "v4", Some("v3"), "mail")
                    .unwrap()
                    .as_deref(),
                Some("authorized-v3")
            );
        }
        assert!(f.access.state.lock().unwrap().blocked);
        assert_eq!(vault.0.calls.lock().unwrap().len(), 3);
        // The historical item is retained even if destination verification fails.
        assert!(
            vault
                .0
                .values
                .lock()
                .unwrap()
                .contains_key(&("v3".into(), "mail".into()))
        );
    }
    #[test]
    fn removing_current_item_cleans_all_importable_generations_first() {
        let f = Fixture::new();
        for service in ["v4", "v3", "v2"] {
            f.seed(service, "mail", "synthetic");
        }
        f.access
            .delete_fallbacks(&f.vault, "v4", &["v3", "v2"], "mail")
            .unwrap();
        assert!(f.vault.values.lock().unwrap().is_empty());
        assert_eq!(
            f.vault
                .calls
                .lock()
                .unwrap()
                .iter()
                .map(|(op, _)| op.as_str())
                .collect::<Vec<_>>(),
            vec!["delete:v3:mail", "delete:v2:mail", "delete:v4:mail"]
        );
    }
    #[test]
    fn denial_stops_other_item_dialogs_but_unlock_recovers_silently() {
        let f = Fixture::new();
        *f.vault.locked.lock().unwrap() = true;
        assert!(f.access.read(&f.vault, "new", None, "mail").is_err());
        assert!(f.access.read(&f.vault, "new", None, "calendar").is_err());
        assert!(f.access.read(&f.vault, "new", None, "booking").is_err());
        *f.vault.locked.lock().unwrap() = false;
        f.seed("new", "mail", "unlocked");
        assert_eq!(
            f.access
                .read(&f.vault, "new", None, "mail")
                .unwrap()
                .as_deref(),
            Some("unlocked")
        );
        let calls = f.vault.calls.lock().unwrap();
        assert!(calls[0].1);
        assert!(calls[1..].iter().all(|(_, interactive)| !interactive));
    }
    #[test]
    fn independent_handles_invalidate_cached_success_missing_and_failure() {
        let f = Fixture::new();
        let other = Access::new(f.root.clone());
        assert!(
            f.access
                .read(&f.vault, "new", None, "account")
                .unwrap()
                .is_none()
        );
        other.write(&f.vault, "new", "account", "first").unwrap();
        assert_eq!(
            f.access
                .read(&f.vault, "new", None, "account")
                .unwrap()
                .as_deref(),
            Some("first")
        );
        other.write(&f.vault, "new", "account", "second").unwrap();
        assert_eq!(
            f.access
                .read(&f.vault, "new", None, "account")
                .unwrap()
                .as_deref(),
            Some("second")
        );
        other.delete(&f.vault, "new", None, "account").unwrap();
        assert!(
            f.access
                .read(&f.vault, "new", None, "account")
                .unwrap()
                .is_none()
        );
        *f.vault.locked.lock().unwrap() = true;
        assert!(f.access.read(&f.vault, "new", None, "account").is_err());
        *f.vault.locked.lock().unwrap() = false;
        other.write(&f.vault, "new", "account", "readded").unwrap();
        assert_eq!(
            f.access
                .read(&f.vault, "new", None, "account")
                .unwrap()
                .as_deref(),
            Some("readded")
        );
    }
    #[test]
    fn partial_delete_cannot_resurrect_previous_credentials() {
        let f = Fixture::new();
        f.seed("old", "account", "stale");
        f.seed("new", "account", "current");
        *f.vault.fail_delete.lock().unwrap() = Some("old".into());
        assert!(
            f.access
                .delete(&f.vault, "new", Some("old"), "account")
                .is_err()
        );
        assert_eq!(
            f.access
                .read(&f.vault, "new", Some("old"), "account")
                .unwrap()
                .as_deref(),
            Some("current")
        );
        *f.vault.fail_delete.lock().unwrap() = Some("new".into());
        assert!(
            f.access
                .delete(&f.vault, "new", Some("old"), "account")
                .is_err()
        );
        assert!(
            !f.vault
                .values
                .lock()
                .unwrap()
                .contains_key(&("old".into(), "account".into()))
        );
        *f.vault.fail_delete.lock().unwrap() = None;
        f.access
            .delete(&f.vault, "new", Some("old"), "account")
            .unwrap();
        assert!(
            f.access
                .read(&f.vault, "new", Some("old"), "account")
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn failed_write_invalidates_other_readers_and_does_not_cache_uncommitted_value() {
        let f = Fixture::new();
        f.seed("new", "account", "old");
        let other = Access::new(f.root.clone());
        f.access.read(&f.vault, "new", None, "account").unwrap();
        *f.vault.fail_write.lock().unwrap() = true;
        assert!(
            other
                .write(&f.vault, "new", "account", "uncommitted")
                .is_err()
        );
        assert_eq!(
            f.access
                .read(&f.vault, "new", None, "account")
                .unwrap()
                .as_deref(),
            Some("old")
        );
        assert_eq!(
            f.vault
                .calls
                .lock()
                .unwrap()
                .iter()
                .filter(|(op, _)| op == "read:new:account")
                .count(),
            2
        );
    }
    #[test]
    fn coordinator_rejects_symlink_and_never_writes_secrets_to_disk() {
        let f = Fixture::new();
        f.access
            .write(&f.vault, "new", "account", "never-on-disk")
            .unwrap();
        for entry in fs::read_dir(&f.root).unwrap() {
            assert!(
                !fs::read_to_string(entry.unwrap().path())
                    .unwrap()
                    .contains("never-on-disk")
            );
        }
        let key = Access::key("new", "account");
        fs::remove_file(f.root.join(&key)).unwrap();
        std::os::unix::fs::symlink(f.root.join("lock"), f.root.join(key)).unwrap();
        assert!(f.access.read(&f.vault, "new", None, "account").is_err());
    }
}

#[cfg(test)]
mod process_tests {
    use super::*;
    struct FileVault(PathBuf);
    impl Backend for FileVault {
        fn read(&self, _: &str, _: &str, _: bool) -> Result<Option<String>, String> {
            match fs::read_to_string(&self.0) {
                Ok(value) => Ok(Some(value)),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(_) => Err("synthetic store unavailable".into()),
            }
        }
        fn write(&self, _: &str, _: &str, value: &str, _: bool) -> Result<(), String> {
            fs::write(&self.0, value).map_err(|_| "synthetic write failed".into())
        }
        fn delete(&self, _: &str, _: &str, _: bool) -> Result<(), String> {
            match fs::remove_file(&self.0) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(_) => Err("synthetic delete failed".into()),
            }
        }
    }
    #[test]
    fn cross_process_revision_fences_restarts_refreshes_and_remove_readd() {
        const CHILD_ROOT: &str = "EMAIL_SYNTHETIC_CREDENTIAL_CHILD_ROOT";
        if let Some(root) = std::env::var_os(CHILD_ROOT) {
            let root = PathBuf::from(root);
            let access = Access::new(root.join("coordinator"));
            let vault = FileVault(root.join("synthetic-os-secret"));
            if std::env::var("EMAIL_SYNTHETIC_CREDENTIAL_CHILD_ACTION").unwrap() == "delete" {
                access.delete(&vault, "new", None, "account").unwrap();
            } else {
                access
                    .write(&vault, "new", "account", "synthetic-new")
                    .unwrap();
            }
            return;
        }
        let root = std::env::temp_dir().join(format!("email-process-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let access = Access::new(root.join("coordinator"));
        let vault = FileVault(root.join("synthetic-os-secret"));
        let child = |action| {
            let output=std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact","credential_access::process_tests::cross_process_revision_fences_restarts_refreshes_and_remove_readd"])
                .env(CHILD_ROOT,&root).env("EMAIL_SYNTHETIC_CREDENTIAL_CHILD_ACTION",action).output().unwrap();
            assert!(
                output.status.success(),
                "child failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        access
            .write(&vault, "new", "account", "synthetic-old")
            .unwrap();
        let captured = access.revision_for("new", "account").unwrap();
        assert_eq!(
            access
                .read(&vault, "new", None, "account")
                .unwrap()
                .as_deref(),
            Some("synthetic-old")
        );
        child("write");
        assert!(
            access
                .write_if_revision(&vault, "new", "account", "stale-refresh", Some(&captured))
                .is_err()
        );
        assert_eq!(
            access
                .read(&vault, "new", None, "account")
                .unwrap()
                .as_deref(),
            Some("synthetic-new")
        );
        child("delete");
        assert!(
            access
                .read(&vault, "new", None, "account")
                .unwrap()
                .is_none()
        );
        child("write");
        assert_eq!(
            access
                .read(&vault, "new", None, "account")
                .unwrap()
                .as_deref(),
            Some("synthetic-new")
        );
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn booking_operation_lock_is_separate_from_credential_lock() {
        let root =
            std::env::temp_dir().join(format!("email-booking-lock-{}", uuid::Uuid::new_v4()));
        let access = Access::new(root.clone());
        let operation = access.operation_lock().unwrap();
        // Reading revisions during a booking operation must not deadlock.
        access.revision_for("new", "booking").unwrap();
        let other = access.file("booking-operation").unwrap();
        assert!(other.try_lock().is_err());
        drop(operation);
        other.try_lock().unwrap();
        drop(other);
        fs::remove_dir_all(root).unwrap();
    }
}
