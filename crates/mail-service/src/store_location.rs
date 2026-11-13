//! Where the desktop apps keep their default mail store, and which store this
//! process opened. Every other store is isolated from the real one.
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static OPENED: OnceLock<PathBuf> = OnceLock::new();

/// Records the first store this process opened successfully. Each front end (a
/// desktop app, the MCP server) works on one store, which decides its
/// Scheduling credential; a process should not switch stores.
pub(crate) fn note_opened_store(path: &Path) {
    let _ = OPENED.set(path.to_path_buf());
}

pub(crate) fn opened_store() -> Option<&'static Path> {
    OPENED.get().map(PathBuf::as_path)
}

/// The directory of the default store, named by the distribution profile:
/// `~/Library/Application Support/<MACOS_DATA_DIRECTORY>` on macOS and
/// `$XDG_DATA_HOME/<LINUX_DATA_DIRECTORY>` (or `~/.local/share/...`) elsewhere.
/// The store itself is `mail.sqlite` in it.
pub fn default_store_directory() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        // Same source as FileManager's Application Support URL in the app:
        // the account's home directory, not $HOME.
        account_home()
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
            .map(|home| home.join("Library/Application Support").join(crate::profile::MACOS_DATA_DIRECTORY))
    }
    #[cfg(not(target_os = "macos"))]
    {
        // Exactly as the Linux app has always resolved it, so existing stores
        // stay where they are.
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
            .map(|base| base.join(crate::profile::LINUX_DATA_DIRECTORY))
    }
}

#[cfg(target_os = "macos")]
fn account_home() -> Option<PathBuf> {
    use std::ffi::CStr;
    use std::os::unix::ffi::OsStrExt;
    let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
    let mut result: *mut libc::passwd = std::ptr::null_mut();
    let mut buffer = vec![0 as libc::c_char; 16 * 1024];
    // SAFETY: getpwuid_r writes only into `entry` and `buffer`, which outlive
    // the borrowed pw_dir string copied below.
    let status = unsafe {
        libc::getpwuid_r(libc::getuid(), &mut entry, buffer.as_mut_ptr(), buffer.len(), &mut result)
    };
    if status != 0 || result.is_null() || entry.pw_dir.is_null() {
        return None;
    }
    let directory = unsafe { CStr::from_ptr(entry.pw_dir) };
    Some(PathBuf::from(std::ffi::OsStr::from_bytes(directory.to_bytes())))
}

/// True when `path` is the default store or another file in its directory,
/// however it is spelled. A store there shares the directory's settings and
/// pictures, so only a separate directory isolates a store. When the default
/// location cannot be determined, every store counts as the default one: it
/// keeps the real Scheduling connection and never gets demo data.
pub fn is_default_store(path: &Path) -> bool {
    let Some(directory) = default_store_directory() else {
        return true;
    };
    if is_store_in(path, &directory) {
        return true;
    }
    // An empty or relative XDG_DATA_HOME makes the app's own directory
    // relative; the spec's ~/.local/share location still counts as real.
    #[cfg(not(target_os = "macos"))]
    if !directory.is_absolute() {
        if let Some(home) = std::env::var_os("HOME") {
            let spec = PathBuf::from(home).join(".local/share").join(crate::profile::LINUX_DATA_DIRECTORY);
            return is_store_in(path, &spec);
        }
    }
    false
}

/// One spelling per store: the resolved directory and the file name, or the
/// absolute path when the directory does not exist yet.
pub fn canonical_store_path(path: &Path) -> PathBuf {
    let resolved = match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => {
            let parent = if parent.as_os_str().is_empty() { Path::new(".") } else { parent };
            std::fs::canonicalize(parent).ok().map(|parent| parent.join(name))
        }
        _ => None,
    };
    resolved
        .or_else(|| std::path::absolute(path).ok())
        .unwrap_or_else(|| path.to_path_buf())
}

/// The file itself (through a symlink or hard link) and its directory are
/// compared by identity, so symlinks, `..` and letter case cannot disguise the
/// default store or its directory.
fn is_store_in(path: &Path, directory: &Path) -> bool {
    if same_existing_item(path, &directory.join("mail.sqlite")) {
        return true;
    }
    let Some(parent) = path.parent() else {
        return false;
    };
    let parent = if parent.as_os_str().is_empty() { Path::new(".") } else { parent };
    same_directory(parent, directory)
}

fn same_existing_item(a: &Path, b: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let (Ok(x), Ok(y)) = (std::fs::metadata(a), std::fs::metadata(b)) {
            return x.dev() == y.dev() && x.ino() == y.ino();
        }
    }
    false
}

fn same_directory(a: &Path, b: &Path) -> bool {
    if a.exists() && b.exists() {
        return same_existing_item(a, b);
    }
    // A directory that does not exist yet holds no store; compare spellings.
    matches!((std::path::absolute(a), std::path::absolute(b)), (Ok(x), Ok(y)) if x == y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_store_is_recognized_however_it_is_spelled() {
        let root = std::env::temp_dir().join(format!("email-store-location-{}", uuid::Uuid::new_v4()));
        let real = root.join("real");
        std::fs::create_dir_all(&real).unwrap();
        assert!(is_store_in(&real.join("mail.sqlite"), &real));
        assert!(is_store_in(&real.join("../real/mail.sqlite"), &real));
        // Another file in the default directory would share its settings.
        assert!(is_store_in(&real.join("other.sqlite"), &real));
        assert!(!is_store_in(&root.join("demo/mail.sqlite"), &real));
        assert!(!is_store_in(&root.join("realish/mail.sqlite"), &real));
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&real, root.join("link")).unwrap();
            assert!(is_store_in(&root.join("link/mail.sqlite"), &real));
            std::fs::write(real.join("mail.sqlite"), b"").unwrap();
            std::fs::create_dir(root.join("elsewhere")).unwrap();
            std::fs::hard_link(real.join("mail.sqlite"), root.join("elsewhere/copy.sqlite")).unwrap();
            assert!(is_store_in(&root.join("elsewhere/copy.sqlite"), &real));
            std::os::unix::fs::symlink(real.join("mail.sqlite"), root.join("elsewhere/link.sqlite")).unwrap();
            assert!(is_store_in(&root.join("elsewhere/link.sqlite"), &real));
        }
        #[cfg(target_os = "macos")]
        assert!(is_store_in(&real.join("MAIL.sqlite"), &real));
        std::fs::remove_dir_all(root).unwrap();
        if let Some(directory) = default_store_directory() {
            assert!(is_default_store(&directory.join("mail.sqlite")));
        }
    }
}
