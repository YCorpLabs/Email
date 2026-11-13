//! QA builds only (`test-fixture`): keep credentials in private files under
//! `EMAIL_TEST_CREDENTIALS_DIR`, so native-shell runs against the synthetic
//! Gmail fixture never read or write the macOS Keychain or a Secret Service
//! keyring. Release builds do not contain this module.
use super::*;
use sha2::{Digest, Sha256};

pub(super) struct FileCredentials(PathBuf);

impl FileCredentials {
    /// `None` when the variable is unset. A set variable that cannot be used
    /// is an error: QA runs must never fall back to a real keychain.
    pub(super) fn from_environment() -> Option<Result<Self, String>> {
        let dir = PathBuf::from(std::env::var_os("EMAIL_TEST_CREDENTIALS_DIR")?);
        if !dir.is_absolute() {
            // An empty or relative value would write secrets into the working folder.
            return Some(Err(
                "EMAIL_TEST_CREDENTIALS_DIR must be an absolute path".into()
            ));
        }
        #[cfg(unix)]
        let created = {
            use std::os::unix::fs::DirBuilderExt;
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(&dir)
        };
        #[cfg(not(unix))]
        let created = fs::create_dir_all(&dir);
        Some(
            created
                .map(|()| Self(dir))
                .map_err(|e| format!("EMAIL_TEST_CREDENTIALS_DIR is not usable: {e}")),
        )
    }

    fn path(&self, id: &str) -> PathBuf {
        let digest = Sha256::digest(id.as_bytes());
        self.0.join(
            digest
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>(),
        )
    }
}

impl CredentialStore for FileCredentials {
    fn read(&self, id: &str, legacy: bool) -> Result<Option<String>, String> {
        if legacy {
            return Ok(None);
        }
        match fs::read_to_string(self.path(id)) {
            Ok(value) => Ok(Some(value)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    }
    fn write(&self, id: &str, value: &str) -> Result<(), String> {
        use std::io::Write;
        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        // Private from the first byte, never readable by others in between.
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(self.path(id)).map_err(|e| e.to_string())?;
        file.write_all(value.as_bytes()).map_err(|e| e.to_string())
    }
    fn delete(&self, id: &str, legacy: bool) -> Result<(), String> {
        if legacy {
            return Ok(());
        }
        match fs::remove_file(self.path(id)) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.to_string()),
            _ => Ok(()),
        }
    }
}

/// Refuses every operation, so a misconfigured QA run fails instead of using
/// the OS keychain.
pub(super) struct UnusableCredentials(pub(super) String);

impl CredentialStore for UnusableCredentials {
    fn read(&self, _id: &str, _legacy: bool) -> Result<Option<String>, String> {
        Err(self.0.clone())
    }
    fn write(&self, _id: &str, _value: &str) -> Result<(), String> {
        Err(self.0.clone())
    }
    fn delete(&self, _id: &str, _legacy: bool) -> Result<(), String> {
        Err(self.0.clone())
    }
}
