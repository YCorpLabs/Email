//! Build-time distribution profile shared by the mail engine and Linux shell
//! build scripts. The profile is the one place that says which build is the
//! official one; every other build gets the neutral `unofficial` identity.
//!
//! This mirrors `scripts/distribution-profile.py`. Keep both rule sets identical.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const SELECTION_FILE: &str = "config/distribution-profile.local";
pub const OAUTH_FILE: &str = "config/oauth-desktop.local.env";
pub const DEFAULT_PROFILE: &str = "unofficial";
pub const KEYS: [&str; 19] = [
    "PROFILE",
    "DISTRIBUTION",
    "GOOGLE_CLIENT_ID_REQUIRED",
    // Whether one-click Google sign-in is offered (Google verification state).
    "GOOGLE_OAUTH_PUBLIC",
    "MACOS_BUNDLE_ID",
    "MACOS_DATA_DIRECTORY",
    "KEYCHAIN_NAMESPACE",
    "LINUX_APP_ID",
    "LINUX_DATA_DIRECTORY",
    "LINUX_SECRET_COLLECTION",
    "LINUX_DISPATCH_UNIT",
    "MCP_SERVER_NAME",
    "SUPPORT_ADDRESS",
    "FEEDBACK_ADDRESS",
    "SCHEDULING_ORIGIN",
    "UPDATE_FEED_URL",
    "UPDATE_PUBLIC_ED_KEY",
    "UPDATE_ALLOWED_HOSTS",
    // Per-draft email open tracking (Track opens). Development builds only.
    "OPEN_TRACKING",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    pub path: PathBuf,
    values: BTreeMap<String, String>,
}

impl Profile {
    pub fn get(&self, key: &str) -> &str {
        self.values.get(key).map(String::as_str).unwrap_or_default()
    }
    pub fn flag(&self, key: &str) -> bool {
        self.get(key) == "true"
    }
}

fn alnum(c: char) -> bool {
    c.is_ascii_alphanumeric()
}

fn word(value: &str, max: usize, first: fn(char) -> bool, rest: fn(char) -> bool) -> bool {
    let mut chars = value.chars();
    value.len() <= max && chars.next().is_some_and(first) && chars.all(rest)
}

fn components(value: &str, min: usize, max: usize, part: fn(&str) -> bool) -> bool {
    let parts: Vec<_> = value.split('.').collect();
    (min..=max).contains(&parts.len()) && parts.into_iter().all(part)
}

fn host(value: &str) -> bool {
    components(value, 2, 127, |label| {
        (1..=63).contains(&label.len())
            && label.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            && !label.starts_with('-')
            && !label.ends_with('-')
    })
}

fn address(value: &str) -> bool {
    let Some((local, domain)) = value.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && local.chars().all(|c| alnum(c) || "._%+-".contains(c))
        && components(domain, 2, 127, |label| {
            !label.is_empty() && label.chars().all(|c| alnum(c) || c == '-')
        })
}

/// Base64 of 32 bytes, not all zero; matches Python's validating decoder.
fn ed_public_key(value: &str) -> bool {
    if value.len() != 44 || !value.ends_with('=') {
        return false;
    }
    let (mut bits, mut count, mut bytes) = (0u32, 0u32, Vec::with_capacity(32));
    for c in value[..43].bytes() {
        let digit = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return false,
        };
        bits = (bits << 6) | u32::from(digit);
        count += 6;
        if count >= 8 {
            count -= 8;
            bytes.push((bits >> count) as u8);
            bits &= (1 << count) - 1;
        }
    }
    bytes.len() == 32 && bytes.iter().any(|b| *b != 0)
}

/// HTTPS on an allowed host, default port, no credentials or fragment and a
/// non-root path; matches `feed_allowed` in scripts/distribution-profile.py.
fn feed_allowed(url: &str, hosts: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    if rest.contains('#') {
        return false;
    }
    let split = rest.find(['/', '?']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(split);
    let path = tail.split('?').next().unwrap_or_default();
    let (host, port) = authority.split_once(':').unwrap_or((authority, ""));
    !authority.contains('@')
        && (port.is_empty() || port == "443")
        && path.len() > 1
        && hosts.split(' ').any(|allowed| allowed == host.to_ascii_lowercase())
}

fn valid(key: &str, value: &str) -> bool {
    let optional = |check: fn(&str) -> bool| value.is_empty() || check(value);
    if value.contains("..") {
        return false;
    }
    match key {
        "PROFILE" => word(value, 40, |c| c.is_ascii_lowercase() || c.is_ascii_digit(), |c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'
        }),
        "DISTRIBUTION" | "GOOGLE_CLIENT_ID_REQUIRED" | "GOOGLE_OAUTH_PUBLIC" | "OPEN_TRACKING" => {
            value == "true" || value == "false"
        }
        "MACOS_BUNDLE_ID" => components(value, 3, 127, |part| {
            !part.is_empty() && part.chars().all(|c| alnum(c) || c == '-')
        }),
        "MACOS_DATA_DIRECTORY" | "LINUX_SECRET_COLLECTION" => {
            word(value, 64, alnum, |c| alnum(c) || " ._-".contains(c))
        }
        "KEYCHAIN_NAMESPACE" | "LINUX_DATA_DIRECTORY" | "LINUX_DISPATCH_UNIT" => {
            word(value, 64, alnum, |c| alnum(c) || "._-".contains(c))
        }
        "LINUX_APP_ID" => value.len() <= 255 && components(value, 3, 5, |part| {
            word(part, 255, |c| c.is_ascii_alphabetic() || c == '_', |c| {
                alnum(c) || c == '_' || c == '-'
            })
        }),
        "MCP_SERVER_NAME" => word(value, 64, alnum, |c| alnum(c) || c == '_' || c == '-'),
        "SUPPORT_ADDRESS" | "FEEDBACK_ADDRESS" => optional(address),
        "SCHEDULING_ORIGIN" => optional(|v| v.strip_prefix("https://").is_some_and(host)),
        "UPDATE_FEED_URL" => optional(|v| {
            v.starts_with("https://") && !v.chars().any(char::is_whitespace)
        }),
        "UPDATE_PUBLIC_ED_KEY" => optional(ed_public_key),
        "UPDATE_ALLOWED_HOSTS" => optional(|v| v.split(' ').all(host)),
        _ => false,
    }
}

/// Strict `KEY=value` lines: no quotes, expansion, `export` or duplicates.
pub fn parse(text: &str, source: &str) -> Result<BTreeMap<String, String>, String> {
    let bare_return = text.replace("\r\n", "\n").contains('\r');
    if bare_return || text.chars().any(|c| (c.is_ascii_control() && !"\t\r\n".contains(c)) || c == '\u{7f}') {
        return Err(format!("{source}: control characters are not allowed"));
    }
    let mut values = BTreeMap::new();
    for (index, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=').filter(|(k, _)| KEYS.contains(k)) else {
            return Err(format!(
                "{source}:{}: expected KEY=value with a known profile key",
                index + 1
            ));
        };
        if values.insert(key.to_owned(), value.to_owned()).is_some() {
            return Err(format!("{source}:{}: {key} is set twice", index + 1));
        }
    }
    let missing: Vec<_> = KEYS.iter().filter(|k| !values.contains_key(**k)).copied().collect();
    if !missing.is_empty() {
        return Err(format!("{source}: missing {}", missing.join(", ")));
    }
    if let Some(key) = KEYS.iter().find(|key| !valid(key, &values[**key])) {
        return Err(format!("{source}: {key} is not valid"));
    }
    let (support, feedback) = (&values["SUPPORT_ADDRESS"], &values["FEEDBACK_ADDRESS"]);
    if support.is_empty() != feedback.is_empty()
        || (!support.is_empty() && support.eq_ignore_ascii_case(feedback))
    {
        return Err(format!(
            "{source}: set both SUPPORT_ADDRESS and FEEDBACK_ADDRESS, as different addresses, or neither"
        ));
    }
    // A distributed build never tracks opens, and tracking needs the service.
    if values["OPEN_TRACKING"] == "true"
        && (values["DISTRIBUTION"] == "true" || values["SCHEDULING_ORIGIN"].is_empty())
    {
        return Err(format!(
            "{source}: OPEN_TRACKING=true needs a SCHEDULING_ORIGIN and is not allowed with DISTRIBUTION=true"
        ));
    }
    let (feed, key) = (&values["UPDATE_FEED_URL"], &values["UPDATE_PUBLIC_ED_KEY"]);
    if feed.is_empty() != key.is_empty() {
        return Err(format!(
            "{source}: UPDATE_FEED_URL and UPDATE_PUBLIC_ED_KEY must be set together"
        ));
    }
    if !feed.is_empty() && !feed_allowed(feed, &values["UPDATE_ALLOWED_HOSTS"]) {
        return Err(format!(
            "{source}: UPDATE_FEED_URL must be HTTPS on one of UPDATE_ALLOWED_HOSTS"
        ));
    }
    Ok(values)
}

/// The profile file, following the documented precedence.
pub fn selected(root: &Path, environment: Option<String>) -> Result<PathBuf, String> {
    let mut choice = environment.unwrap_or_default().trim().to_owned();
    let mut origin = "EMAIL_DISTRIBUTION_PROFILE";
    let selection = root.join(SELECTION_FILE);
    if choice.is_empty() && selection.exists() {
        // An unreadable selection must fail, never fall back to the default.
        let text = std::fs::read_to_string(&selection)
            .map_err(|e| format!("{SELECTION_FILE} cannot be read: {e}"))?;
        choice = text.trim().to_owned();
        origin = SELECTION_FILE;
    }
    if choice.is_empty() {
        return Ok(root.join("config/profiles").join(format!("{DEFAULT_PROFILE}.env")));
    }
    let path = if valid("PROFILE", &choice) {
        root.join("config/profiles").join(format!("{choice}.env"))
    } else if choice.starts_with('/') {
        PathBuf::from(&choice)
    } else if choice.ends_with(".env") {
        root.join(&choice)
    } else {
        return Err(format!(
            "{origin} must name a profile in config/profiles or a .env file path"
        ));
    };
    if !path.is_file() {
        return Err(format!("{origin} selects a profile that does not exist: {choice}"));
    }
    Ok(path)
}

/// A checkout with local OAuth values must choose its identity explicitly, so a
/// forgotten selection never silently builds the neutral identity there.
pub fn require_selection(root: &Path, environment: Option<&str>) -> Result<(), String> {
    let explicit = environment.is_some_and(|v| !v.trim().is_empty()) || root.join(SELECTION_FILE).exists();
    if !explicit && root.join(OAUTH_FILE).is_file() {
        return Err(format!(
            "{OAUTH_FILE} exists but no distribution profile is selected. Write a profile name to {SELECTION_FILE} (ignored by Git), for example `echo development > {SELECTION_FILE}` to keep the existing development identity, or `echo unofficial > ...` for a neutral build."
        ));
    }
    Ok(())
}

pub fn load_from(root: &Path, environment: Option<String>) -> Result<Profile, String> {
    let path = selected(root, environment)?;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("profile").to_owned();
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{name}: {e}"))?;
    let values = parse(&text, &name)?;
    let in_catalog = path.parent() == Some(root.join("config/profiles").as_path());
    if in_catalog && path.file_stem().and_then(|s| s.to_str()) != Some(values["PROFILE"].as_str())
    {
        return Err(format!("{name}: PROFILE must match the file name"));
    }
    let path = path.canonicalize().map_err(|e| format!("{name}: {e}"))?;
    Ok(Profile { path, values })
}

/// For build scripts: resolve, validate and publish the profile to rustc as
/// `EMAIL_PROFILE_<KEY>` environment values. Any invalid profile fails the build.
pub fn emit(root: &Path) -> Profile {
    println!("cargo:rerun-if-env-changed=EMAIL_DISTRIBUTION_PROFILE");
    // A directory scan notices a created, edited or removed selection file.
    println!("cargo:rerun-if-changed={}", root.join("config").display());
    let environment = std::env::var("EMAIL_DISTRIBUTION_PROFILE").ok();
    let profile = require_selection(root, environment.as_deref())
        .and_then(|()| load_from(root, environment))
        .unwrap_or_else(|error| panic!("Distribution profile: {error}"));
    println!("cargo:rerun-if-changed={}", profile.path.display());
    for key in KEYS {
        println!("cargo:rustc-env=EMAIL_PROFILE_{key}={}", profile.get(key));
    }
    profile
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    fn catalog(name: &str) -> Profile {
        load_from(&root(), Some(name.into())).unwrap()
    }

    #[test]
    fn tracked_profiles_are_valid() {
        for name in ["official", "development", "unofficial"] {
            assert_eq!(catalog(name).get("PROFILE"), name);
        }
    }

    #[test]
    fn frozen_development_identity_is_unchanged() {
        let development = catalog("development");
        assert_eq!(development.get("MACOS_BUNDLE_ID"), "dev.email.client");
        assert_eq!(development.get("KEYCHAIN_NAMESPACE"), "email.native.mail");
        assert_eq!(development.get("MACOS_DATA_DIRECTORY"), "Email");
        assert_eq!(development.get("LINUX_DATA_DIRECTORY"), "email-client");
        assert_eq!(development.get("LINUX_SECRET_COLLECTION"), "Email");
        assert_eq!(development.get("LINUX_APP_ID"), "dev.email.client");
        assert!(!development.flag("DISTRIBUTION"));
    }

    #[test]
    fn official_and_unofficial_identities_never_collide() {
        let ids = |p: &Profile| {
            [
                "MACOS_BUNDLE_ID",
                "MACOS_DATA_DIRECTORY",
                "KEYCHAIN_NAMESPACE",
                "LINUX_APP_ID",
                "LINUX_DATA_DIRECTORY",
                "LINUX_SECRET_COLLECTION",
                "LINUX_DISPATCH_UNIT",
            ]
            .map(|k| p.get(k).to_ascii_lowercase())
        };
        let profiles = ["official", "development", "unofficial"].map(catalog);
        for (index, first) in profiles.iter().enumerate() {
            for second in &profiles[index + 1..] {
                for (a, b) in ids(first).iter().zip(ids(second).iter()) {
                    assert_ne!(a, b);
                }
            }
        }
        let official = catalog("official");
        assert_eq!(official.get("MACOS_BUNDLE_ID"), "com.ycorplabs.mail");
        assert_eq!(official.get("LINUX_APP_ID"), "com.ycorplabs.Mail");
        // Off for official releases until Google verification passes.
        assert!(!official.flag("GOOGLE_OAUTH_PUBLIC"));
        assert!(catalog("development").flag("GOOGLE_OAUTH_PUBLIC"));
        let neutral = catalog("unofficial");
        for key in ["SUPPORT_ADDRESS", "SCHEDULING_ORIGIN", "UPDATE_FEED_URL", "UPDATE_ALLOWED_HOSTS"] {
            assert!(neutral.get(key).is_empty(), "{key}");
        }
        assert!(!neutral.flag("OPEN_TRACKING"));
    }

    #[test]
    fn open_tracking_is_development_only() {
        assert!(catalog("development").flag("OPEN_TRACKING"));
        assert!(!catalog("official").flag("OPEN_TRACKING"));
        assert!(!catalog("unofficial").flag("OPEN_TRACKING"));
    }

    #[test]
    fn default_selection_is_neutral() {
        let path = selected(&root(), None).unwrap();
        if !root().join(SELECTION_FILE).exists() {
            assert!(path.ends_with("config/profiles/unofficial.env"));
        }
        assert!(selected(&root(), Some("missing-profile".into())).is_err());
        assert!(selected(&root(), Some("../outside".into())).is_err());
    }

    #[test]
    fn shared_conformance_cases() {
        let base = std::fs::read_to_string(root().join("config/profiles/unofficial.env")).unwrap();
        let cases = include_str!("../conformance.tsv");
        let mut count = 0;
        for line in cases.lines().filter(|l| !l.is_empty() && !l.starts_with('#')) {
            let mut fields = line.split('\t');
            let outcome = fields.next().unwrap();
            let mut text = base.clone();
            for edit in fields {
                let key = edit.split('=').next().unwrap();
                let old = text
                    .lines()
                    .find(|l| l.starts_with(&format!("{key}=")))
                    .unwrap_or_else(|| panic!("{key} missing from base"))
                    .to_owned();
                text = text.replacen(&old, edit, 1);
            }
            assert_eq!(parse(&text, "case").is_ok(), outcome == "ok", "{line}");
            count += 1;
        }
        assert!(count > 40);
    }

    #[test]
    fn local_oauth_values_require_an_explicit_selection() {
        let dir = std::env::temp_dir().join(format!("profile-selection-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("config")).unwrap();
        assert!(require_selection(&dir, None).is_ok());
        std::fs::write(dir.join(OAUTH_FILE), "EMAIL_GOOGLE_CLIENT_SECRET=fixture\n").unwrap();
        let error = require_selection(&dir, None).unwrap_err();
        assert!(error.contains(SELECTION_FILE) && !error.contains("fixture"));
        assert!(require_selection(&dir, Some("development")).is_ok());
        std::fs::write(dir.join(SELECTION_FILE), "development\n").unwrap();
        assert!(require_selection(&dir, None).is_ok());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rejects_malformed_profiles() {
        let base = std::fs::read_to_string(root().join("config/profiles/unofficial.env")).unwrap();
        assert!(parse(&base, "base").is_ok());
        assert!(parse(&base.replace('\n', "\r\n"), "crlf").is_ok());
        assert!(parse(&base.replace('\n', "\r"), "cr").is_err());
        for (from, to) in [
            ("SUPPORT_ADDRESS=", "SUPPORT_ADDRESS=help@example.test"),
            ("SCHEDULING_ORIGIN=", "SCHEDULING_ORIGIN=http://example.test"),
            ("SCHEDULING_ORIGIN=", "SCHEDULING_ORIGIN=https://example.test/path"),
            ("MACOS_BUNDLE_ID=invalid.email.unofficial", "MACOS_BUNDLE_ID=email"),
            ("LINUX_APP_ID=invalid.email.Unofficial", "LINUX_APP_ID=invalid.1email.App"),
            ("MACOS_DATA_DIRECTORY=invalid.email.unofficial", "MACOS_DATA_DIRECTORY=../Email"),
            ("OPEN_TRACKING=false", "OPEN_TRACKING=yes"),
            ("GOOGLE_OAUTH_PUBLIC=true", "GOOGLE_OAUTH_PUBLIC=1"),
            ("UPDATE_FEED_URL=", "UPDATE_FEED_URL=https://updates.example.test/appcast.xml"),
            ("PROFILE=unofficial", "PROFILE=unofficial\nPROFILE=again"),
            ("PROFILE=unofficial", "UNKNOWN=value\nPROFILE=unofficial"),
            ("OPEN_TRACKING=false\n", ""),
            ("PROFILE=unofficial", "PROFILE=unofficial\u{1c}"),
        ] {
            assert!(base.contains(from), "{from}");
            let changed = base.replacen(from, to, 1);
            assert!(parse(&changed, "fixture").is_err(), "{to}");
        }
        let both = base
            .replacen("SUPPORT_ADDRESS=", "SUPPORT_ADDRESS=help@example.test", 1)
            .replacen("FEEDBACK_ADDRESS=", "FEEDBACK_ADDRESS=ideas@example.test", 1);
        assert!(parse(&both, "fixture").is_ok());
        let updates = base
            .replacen("UPDATE_FEED_URL=", "UPDATE_FEED_URL=https://updates.example.test/feed.xml", 1)
            .replacen(
                "UPDATE_PUBLIC_ED_KEY=",
                "UPDATE_PUBLIC_ED_KEY=AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=",
                1,
            );
        assert!(parse(&updates, "fixture").is_err());
        let allowed = updates.replacen(
            "UPDATE_ALLOWED_HOSTS=",
            "UPDATE_ALLOWED_HOSTS=updates.example.test",
            1,
        );
        assert!(parse(&allowed, "fixture").is_ok());
        // Same refusals as the Python parser.
        for (from, to) in [
            ("/feed.xml", ""),
            ("/feed.xml", "/feed.xml#fragment"),
            ("https://updates", "https://user@updates"),
            ("example.test/feed", "example.test:8443/feed"),
            ("/feed.xml", "/feed.xml#"),
            ("https://updates", "https://@updates"),
            ("example.test/feed", "example.test:0443/feed"),
            ("example.test/feed", "example.test:abc/feed"),
            ("AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=", "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="),
        ] {
            assert!(parse(&allowed.replacen(from, to, 1), "fixture").is_err(), "{to}");
        }
        assert!(parse(&allowed.replacen("example.test/feed", "example.test:443/feed", 1), "fixture").is_ok());
    }
}
