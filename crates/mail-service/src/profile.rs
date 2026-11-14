//! The build's distribution profile, compiled from `config/profiles` by
//! `build.rs`. This is the engine's only source for identity and service
//! values; builds without an official profile get the neutral `unofficial` one.
use serde_json::{Value, json};

const fn optional(value: &'static str) -> Option<&'static str> {
    if value.is_empty() { None } else { Some(value) }
}

pub const ID: &str = env!("EMAIL_PROFILE_PROFILE");
/// Prefix of every Keychain/Secret Service service name, e.g. `<namespace>.v4`.
pub const KEYCHAIN_NAMESPACE: &str = env!("EMAIL_PROFILE_KEYCHAIN_NAMESPACE");
/// `~/Library/Application Support/<directory>` on macOS.
pub const MACOS_DATA_DIRECTORY: &str = env!("EMAIL_PROFILE_MACOS_DATA_DIRECTORY");
/// `$XDG_DATA_HOME/<directory>` on Linux.
pub const LINUX_DATA_DIRECTORY: &str = env!("EMAIL_PROFILE_LINUX_DATA_DIRECTORY");
/// The dedicated Secret Service collection used for Scheduling credentials.
pub const LINUX_SECRET_COLLECTION: &str = env!("EMAIL_PROFILE_LINUX_SECRET_COLLECTION");
pub const LINUX_APP_ID: &str = env!("EMAIL_PROFILE_LINUX_APP_ID");
/// The name Agent access registers the MCP helper under in Claude Code and
/// Codex, the same default `scripts/connect-mcp.sh` uses.
pub const MCP_SERVER_NAME: &str = env!("EMAIL_PROFILE_MCP_SERVER_NAME");
pub const SUPPORT_ADDRESS: Option<&str> = optional(env!("EMAIL_PROFILE_SUPPORT_ADDRESS"));
pub const FEEDBACK_ADDRESS: Option<&str> = optional(env!("EMAIL_PROFILE_FEEDBACK_ADDRESS"));
/// The Scheduling service; also the only accepted tracking-pixel origin.
/// `None` hides Bookings, booking links and open tracking.
pub const SCHEDULING_ORIGIN: Option<&str> = optional(env!("EMAIL_PROFILE_SCHEDULING_ORIGIN"));

/// Per-draft open tracking (Track opens), from the profile's `OPEN_TRACKING`
/// key. The parsers allow it only with a Scheduling origin and never in a
/// distribution profile. `build.rs` sets `email_open_tracking` for it; without
/// that cfg, tracking registration and pixel insertion are not compiled.
pub const OPEN_TRACKING: bool = cfg!(email_open_tracking);

/// The step-by-step account setup guide on the website, linked from account
/// setup (`#gmail`, `#icloud`, `#other`). Only a build whose support channel is
/// Y Corp's own links to it, so forks and unofficial builds never point at
/// that site; a fork that sets its own support addresses gets no link.
pub fn setup_guide_url() -> Option<&'static str> {
    SUPPORT_ADDRESS
        .filter(|address| address.to_ascii_lowercase().ends_with("@ycorplabs.com"))
        .map(|_| "https://ycorplabs.com/email/guide/")
}

/// Values the native shells present. Never contains credentials.
pub fn json() -> Value {
    json!({
        "profile": ID,
        "support_address": SUPPORT_ADDRESS,
        "feedback_address": FEEDBACK_ADDRESS,
        "scheduling_origin": SCHEDULING_ORIGIN,
        "open_tracking": OPEN_TRACKING,
        "setup_guide_url": setup_guide_url(),
        "macos_data_directory": MACOS_DATA_DIRECTORY,
        "linux_data_directory": LINUX_DATA_DIRECTORY,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiled_profile_is_consistent() {
        assert_eq!(SUPPORT_ADDRESS.is_some(), FEEDBACK_ADDRESS.is_some());
        assert!(SCHEDULING_ORIGIN.is_none_or(|origin| origin.starts_with("https://")));
        assert!(!OPEN_TRACKING || SCHEDULING_ORIGIN.is_some());
        let value = json();
        assert_eq!(value["profile"], ID);
        assert!(value.get("client_id").is_none());
        // The website guide follows the support channel: official and
        // development builds link to it, the neutral profile never does.
        assert_eq!(setup_guide_url().is_some(), ID == "official" || ID == "development", "{ID}");
        assert_eq!(value["setup_guide_url"].as_str(), setup_guide_url());
    }
}
