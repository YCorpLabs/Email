//! Account-setup failures explained in words a person can act on.
//!
//! The IMAP and SMTP libraries report what the server said. Account setup needs
//! to say what to do next in one line: use an app password, ask a Workspace
//! administrator, or confirm a blocked sign-in in the browser. Both native
//! shells show [`SetupProblem`] unchanged, with the server's words behind a
//! Details disclosure. This module also recognizes well-known address domains
//! and guesses servers for the rest, without any network lookup.
use crate::{ImapConfig, ImapError, TransportSecurity};
use serde::Serialize;

/// The step of `ImapConnector::verify_account` that failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupStage {
    /// TCP or TLS connection to the IMAP server.
    Connect,
    /// IMAP `LOGIN`.
    Login,
    /// Selecting INBOX after a successful login.
    Mailbox,
    /// SMTP connection, authentication or `NOOP`.
    Smtp,
}

/// Which provider's guidance applies. Only wording changes; the protocol
/// handling is the same generic IMAP path for every provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerHint {
    Generic,
    Icloud,
    Gmail,
}

#[derive(Debug)]
pub struct SetupError {
    pub stage: SetupStage,
    pub error: ImapError,
}

/// What the server told us, reduced to the cases setup can explain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Problem {
    /// Gmail: the account has 2-Step Verification and needs an app password,
    /// usually because the regular Google password was entered.
    AppPasswordRequired,
    /// Gmail: a Google Workspace administrator turned IMAP off.
    ImapDisabled,
    /// Gmail: Google blocked the sign-in until it is confirmed in a browser.
    WebLoginRequired,
    TooManyConnections,
    /// Wrong username or password (any provider).
    Rejected,
    Unreachable,
    Tls,
    Other,
}

impl SetupError {
    pub fn new(stage: SetupStage, error: ImapError) -> Self {
        Self { stage, error }
    }

    pub(crate) fn at(stage: SetupStage) -> impl Fn(ImapError) -> Self {
        move |error| Self::new(stage, error)
    }

    fn problem(&self) -> Problem {
        let text = self.error.to_string().to_ascii_lowercase();
        let has = |needle: &str| text.contains(needle);
        // Gmail's IMAP and SMTP texts; never bare status codes, which also
        // match unrelated numbers such as addresses.
        if has("application-specific password required") {
            return Problem::AppPasswordRequired;
        }
        if has("imap access is disabled") || has("not enabled for imap") || has("imap is disabled")
        {
            return Problem::ImapDisabled;
        }
        if has("web login required") || has("webalert") || has("log in via your web browser") {
            return Problem::WebLoginRequired;
        }
        if has("too many simultaneous connections") {
            return Problem::TooManyConnections;
        }
        match (&self.error, self.stage) {
            (ImapError::Imap(imap::Error::No(_) | imap::Error::Bad(_)), SetupStage::Login) => {
                Problem::Rejected
            }
            (ImapError::Imap(imap::Error::Io(_) | imap::Error::ConnectionLost), _) => {
                Problem::Unreachable
            }
            (ImapError::Imap(imap::Error::Tls(_) | imap::Error::TlsHandshake(_)), _)
            | (ImapError::Tls(_), _) => Problem::Tls,
            (ImapError::Smtp(error), _) => {
                // 535: authentication credentials invalid (RFC 4954).
                if error.status().is_some_and(|code| code.to_string() == "535") {
                    Problem::Rejected
                } else if error.is_tls() {
                    Problem::Tls
                } else if error.status().is_none() {
                    Problem::Unreachable
                } else {
                    Problem::Other
                }
            }
            _ => Problem::Other,
        }
    }

    /// True when the servers themselves may be wrong rather than the
    /// password: nothing answered, TLS failed, or sending failed after the
    /// login worked. Setup then shows the server fields.
    pub fn servers_may_be_wrong(&self) -> bool {
        match (self.problem(), self.stage) {
            (Problem::Unreachable | Problem::Tls, _) => true,
            (Problem::Rejected, _) => false,
            (_, stage) => matches!(stage, SetupStage::Connect | SetupStage::Smtp),
        }
    }

    /// One line and, where there is one, a link that fixes it. What the
    /// server said goes in `detail`, which the shells keep behind a Details
    /// disclosure. Never includes the password.
    pub fn explain(&self, config: &ImapConfig, hint: ServerHint) -> SetupProblem {
        let (host, port) = match self.stage {
            SetupStage::Smtp => (&config.smtp_host, config.smtp_port),
            _ => (&config.imap_host, config.imap_port),
        };
        let gmail = hint == ServerHint::Gmail;
        let said = format!("{host}:{port} said: {}", self.error);
        let said = if config.secret.is_empty() {
            said
        } else {
            said.replace(&config.secret, "[redacted]")
        };
        let problem = |code, message: String, action| SetupProblem {
            code,
            message,
            action,
            detail: said.clone(),
            servers: None,
        };
        match (self.problem(), hint) {
            (Problem::AppPasswordRequired, _) if gmail => problem(
                "google_password",
                "That looks like your Google password.".into(),
                Some(GOOGLE_CREATE_APP_PASSWORD),
            ),
            (Problem::ImapDisabled, _) if gmail => SetupProblem {
                detail: format!(
                    "Ask them to turn on IMAP in the Google Admin console: Apps › Google Workspace › Gmail › End User Access.\n{said}"
                ),
                ..problem(
                    "imap_disabled",
                    "Your Google Workspace admin has turned off IMAP for this account.".into(),
                    None,
                )
            },
            (Problem::WebLoginRequired, _) if gmail => problem(
                "web_login",
                "Google blocked this sign-in. Confirm it was you in Gmail, then try again.".into(),
                Some(SetupAction {
                    label: "Open Gmail",
                    url: GMAIL_WEB_URL,
                }),
            ),
            (Problem::TooManyConnections, _) => problem(
                "too_many_connections",
                format!("{host} has too many open connections. Quit other mail apps, then try again."),
                None,
            ),
            (Problem::Rejected, ServerHint::Gmail) => problem(
                "wrong_password",
                "Wrong email or app password.".into(),
                Some(SetupAction {
                    label: "Create a new app password",
                    url: GOOGLE_APP_PASSWORDS_URL,
                }),
            ),
            (Problem::Rejected, ServerHint::Icloud) if !looks_like_apple_app_password(&config.secret) => problem(
                "apple_password",
                "That looks like your Apple Account password.".into(),
                Some(APPLE_CREATE_APP_PASSWORD),
            ),
            (Problem::Rejected, ServerHint::Icloud) => problem(
                "wrong_password",
                "Wrong email or app-specific password.".into(),
                Some(SetupAction {
                    label: "Create a new app-specific password",
                    url: APPLE_ACCOUNT_URL,
                }),
            ),
            (Problem::Rejected, ServerHint::Generic) => {
                problem("wrong_password", "Wrong email or app password.".into(), None)
            }
            (Problem::Unreachable, _) => problem(
                "unreachable",
                format!("Couldn't reach {host}. Check your internet connection, then try again."),
                None,
            ),
            (Problem::Tls, _) => problem(
                "tls",
                format!("Couldn't make a secure connection to {host}."),
                None,
            ),
            _ => match self.stage {
                SetupStage::Mailbox => problem(
                    "mailbox",
                    format!("Signed in, but {host} couldn't open your inbox."),
                    None,
                ),
                SetupStage::Smtp => problem(
                    "smtp",
                    format!("Signed in, but sending through {host} failed."),
                    None,
                ),
                _ => problem("connect", format!("Couldn't connect to {host}."), None),
            },
        }
    }
}

/// A link that fixes a setup problem, such as Google's App passwords page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SetupAction {
    pub label: &'static str,
    pub url: &'static str,
}

/// An account-setup failure as both shells show it: `message` is one line,
/// `action` an optional link that fixes it, and `detail` what the server said
/// (behind a Details disclosure). `servers` is set when Email guessed the
/// servers and could not reach them, so setup can show them for editing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SetupProblem {
    /// Stable identifier for shells and tests, such as `wrong_password`.
    pub code: &'static str,
    pub message: String,
    pub action: Option<SetupAction>,
    pub detail: String,
    pub servers: Option<ServerGuess>,
}

impl SetupProblem {
    /// A problem found before connecting, such as a missing field.
    pub fn plain(message: impl Into<String>) -> Self {
        Self {
            code: "invalid",
            message: message.into(),
            action: None,
            detail: String::new(),
            servers: None,
        }
    }

    /// The Gmail preset was given something that is not an app password,
    /// almost always the regular Google password. Found before connecting.
    pub fn google_password() -> Self {
        Self {
            code: "google_password",
            message: "That looks like your Google password.".into(),
            action: Some(GOOGLE_CREATE_APP_PASSWORD),
            detail: "Google app passwords are 16 letters, like “abcd efgh ijkl mnop”.".into(),
            servers: None,
        }
    }
}

impl From<String> for SetupProblem {
    fn from(message: String) -> Self {
        Self::plain(message)
    }
}

impl From<&str> for SetupProblem {
    fn from(message: &str) -> Self {
        Self::plain(message)
    }
}

/// Google app passwords are 16 letters; Google shows them in groups of four.
pub fn looks_like_google_app_password(secret: &str) -> bool {
    let compact: Vec<char> = secret.chars().filter(|c| !c.is_whitespace()).collect();
    compact.len() == 16 && compact.iter().all(char::is_ascii_alphabetic)
}

/// Apple app-specific passwords are 16 letters, shown as `abcd-efgh-ijkl-mnop`.
pub fn looks_like_apple_app_password(secret: &str) -> bool {
    let compact: Vec<char> = secret
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .collect();
    compact.len() == 16 && compact.iter().all(char::is_ascii_alphabetic)
}

/// The provider an address belongs to, from its domain alone. No network:
/// a custom domain hosted by Google or Apple is `Other` until the person
/// chooses its provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum KnownProvider {
    Gmail,
    Icloud,
    Outlook,
    Other,
}

const GOOGLE_DOMAINS: [&str; 2] = ["gmail.com", "googlemail.com"];
const APPLE_DOMAINS: [&str; 3] = ["icloud.com", "me.com", "mac.com"];
const MICROSOFT_DOMAINS: [&str; 4] = ["outlook.com", "hotmail.com", "live.com", "msn.com"];

/// The lowercase domain of an address, if it looks like one.
fn domain(address: &str) -> Option<String> {
    let (local, domain) = address.trim().rsplit_once('@')?;
    let domain = domain.trim_end_matches('.').to_ascii_lowercase();
    let valid = !local.is_empty()
        && domain.contains('.')
        && !domain.starts_with(['.', '-'])
        && !domain.contains("..")
        && domain
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
    valid.then_some(domain)
}

pub fn provider_for_address(address: &str) -> KnownProvider {
    match domain(address).as_deref() {
        Some(d) if GOOGLE_DOMAINS.contains(&d) => KnownProvider::Gmail,
        Some(d) if APPLE_DOMAINS.contains(&d) => KnownProvider::Icloud,
        Some(d) if MICROSOFT_DOMAINS.contains(&d) => KnownProvider::Outlook,
        _ => KnownProvider::Other,
    }
}

/// Server settings for an address whose servers the person did not enter.
/// Offline: a few providers by name, otherwise the usual `imap.<domain>` on
/// 993 (TLS) and `smtp.<domain>` on 587 (STARTTLS). Setup tries them once
/// and shows them for editing if they don't answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ServerGuess {
    pub imap_host: String,
    pub imap_port: u16,
    pub imap_security: TransportSecurity,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_security: TransportSecurity,
    /// From the provider table rather than the `imap.<domain>` pattern.
    pub known: bool,
}

pub fn guess_servers(address: &str) -> Option<ServerGuess> {
    use TransportSecurity::{ImplicitTls, StartTls};
    let domain = domain(address)?;
    let known = |imap: &str, smtp: &str, smtp_port, smtp_security| ServerGuess {
        imap_host: imap.into(),
        imap_port: 993,
        imap_security: ImplicitTls,
        smtp_host: smtp.into(),
        smtp_port,
        smtp_security,
        known: true,
    };
    Some(match domain.as_str() {
        d if GOOGLE_DOMAINS.contains(&d) => known("imap.gmail.com", "smtp.gmail.com", 465, ImplicitTls),
        d if APPLE_DOMAINS.contains(&d) => known("imap.mail.me.com", "smtp.mail.me.com", 587, StartTls),
        "yahoo.com" | "ymail.com" | "rocketmail.com" => {
            known("imap.mail.yahoo.com", "smtp.mail.yahoo.com", 465, ImplicitTls)
        }
        "aol.com" => known("imap.aol.com", "smtp.aol.com", 465, ImplicitTls),
        "fastmail.com" | "fastmail.fm" => known("imap.fastmail.com", "smtp.fastmail.com", 465, ImplicitTls),
        _ => ServerGuess {
            imap_host: format!("imap.{domain}"),
            imap_port: 993,
            imap_security: ImplicitTls,
            smtp_host: format!("smtp.{domain}"),
            smtp_port: 587,
            smtp_security: StartTls,
            known: false,
        },
    })
}

const GOOGLE_APP_PASSWORDS_URL: &str = "https://myaccount.google.com/apppasswords";
const GMAIL_WEB_URL: &str = "https://mail.google.com/";
const APPLE_ACCOUNT_URL: &str = "https://account.apple.com/";
const GOOGLE_CREATE_APP_PASSWORD: SetupAction = SetupAction {
    label: "Create an app password instead",
    url: GOOGLE_APP_PASSWORDS_URL,
};
const APPLE_CREATE_APP_PASSWORD: SetupAction = SetupAction {
    label: "Create an app-specific password instead",
    url: APPLE_ACCOUNT_URL,
};

#[cfg(test)]
mod tests {
    use super::*;

    fn gmail() -> ImapConfig {
        ImapConfig::gmail("a", "person@gmail.example", "abcd efgh ijkl mnop")
    }

    fn login_no(text: &str) -> SetupError {
        SetupError::new(
            SetupStage::Login,
            ImapError::Imap(imap::Error::No(text.into())),
        )
    }

    #[test]
    fn gmail_preset_uses_documented_servers_and_strips_app_password_spacing() {
        let config = gmail();
        assert_eq!(
            (config.imap_host.as_str(), config.imap_port),
            ("imap.gmail.com", 993)
        );
        assert_eq!(
            (config.smtp_host.as_str(), config.smtp_port),
            ("smtp.gmail.com", 465)
        );
        assert_eq!(config.imap_security, crate::TransportSecurity::ImplicitTls);
        assert_eq!(config.smtp_security, crate::TransportSecurity::ImplicitTls);
        assert_eq!(config.username, "person@gmail.example");
        assert_eq!(config.smtp_username, "person@gmail.example");
        assert_eq!(config.secret, "abcdefghijklmnop");
        assert!(config.archive_mailbox.is_none());
    }

    #[test]
    fn gmail_failures_explain_the_next_step_in_one_line() {
        let config = gmail();
        // Exact Gmail IMAP responses, as the imap crate reports them.
        let wrong = login_no("[AUTHENTICATIONFAILED] Invalid credentials (Failure)")
            .explain(&config, ServerHint::Gmail);
        assert_eq!(
            (wrong.code, wrong.message.as_str()),
            ("wrong_password", "Wrong email or app password.")
        );
        assert_eq!(
            wrong.action.map(|a| a.url),
            Some("https://myaccount.google.com/apppasswords")
        );
        assert!(wrong.detail.starts_with("imap.gmail.com:993 said: "), "{}", wrong.detail);
        assert!(wrong.detail.contains("AUTHENTICATIONFAILED"), "server words kept in detail");
        let regular = login_no("Application-specific password required: https://support.google.com/accounts/answer/185833 (Failure)")
            .explain(&config, ServerHint::Gmail);
        assert_eq!(regular.code, "google_password");
        assert_eq!(regular.message, "That looks like your Google password.");
        assert_eq!(
            regular.action,
            Some(SetupAction {
                label: "Create an app password instead",
                url: "https://myaccount.google.com/apppasswords"
            })
        );
        assert_eq!(regular, SetupProblem { detail: regular.detail.clone(), ..SetupProblem::google_password() });
        let admin = login_no("IMAP access is disabled for your domain. Please contact your domain administrator for questions about this feature. (Failure)")
            .explain(&config, ServerHint::Gmail);
        assert_eq!(admin.code, "imap_disabled");
        assert!(admin.detail.contains("End User Access"), "{}", admin.detail);
        let web =
            login_no("[WEBALERT https://accounts.google.com/signin/continue] Web login required.")
                .explain(&config, ServerHint::Gmail);
        assert!(web.message.starts_with("Google blocked this sign-in"), "{}", web.message);
        let busy = login_no("Too many simultaneous connections. (Failure)")
            .explain(&config, ServerHint::Gmail);
        assert!(
            busy.message.starts_with("imap.gmail.com has too many open connections"),
            "{}",
            busy.message
        );
        for problem in [wrong, regular, admin, web, busy] {
            // One line, no server text, no password, no promise about Google review.
            assert!(!problem.message.contains('\n') && !problem.message.contains("said"), "{}", problem.message);
            assert!(!problem.message.contains("abcdefghijklmnop") && !problem.detail.contains("abcdefghijklmnop"));
            assert!(!problem.message.contains("review") && !problem.detail.contains("review"));
        }
    }

    #[test]
    fn a_password_echoed_by_a_server_is_redacted_from_details() {
        let config = gmail();
        let echoed = login_no("LOGIN person abcdefghijklmnop rejected").explain(&config, ServerHint::Generic);
        assert!(!echoed.detail.contains("abcdefghijklmnop"), "{}", echoed.detail);
        assert!(echoed.detail.contains("[redacted]"), "{}", echoed.detail);
    }

    #[test]
    fn icloud_rejections_tell_a_main_password_from_a_wrong_app_password() {
        let main = ImapConfig::icloud("a", "person@icloud.com", "MyApplePassw0rd!");
        let problem = login_no("[AUTHENTICATIONFAILED] Authentication failed.").explain(&main, ServerHint::Icloud);
        assert_eq!(
            (problem.code, problem.message.as_str()),
            ("apple_password", "That looks like your Apple Account password.")
        );
        assert_eq!(problem.action.map(|a| a.url), Some("https://account.apple.com/"));
        let app = ImapConfig::icloud("a", "person@icloud.com", "abcd-efgh-ijkl-mnop");
        let problem = login_no("[AUTHENTICATIONFAILED] Authentication failed.").explain(&app, ServerHint::Icloud);
        assert_eq!(
            (problem.code, problem.message.as_str()),
            ("wrong_password", "Wrong email or app-specific password.")
        );
        assert!(looks_like_apple_app_password("abcd efgh ijkl mnop"));
        assert!(!looks_like_apple_app_password("abcd-efgh-ijkl"));
    }

    #[test]
    fn network_and_generic_failures_name_the_server() {
        let config = gmail();
        let refused = SetupError::new(
            SetupStage::Connect,
            ImapError::Imap(imap::Error::Io(std::io::Error::from(
                std::io::ErrorKind::ConnectionRefused,
            ))),
        );
        assert!(refused.servers_may_be_wrong());
        assert_eq!(
            refused.explain(&config, ServerHint::Gmail).message,
            "Couldn't reach imap.gmail.com. Check your internet connection, then try again."
        );
        let generic = login_no("LOGIN failed");
        assert!(!generic.servers_may_be_wrong(), "the server answered");
        let generic = generic.explain(&config, ServerHint::Generic);
        assert_eq!(
            (generic.code, generic.message.as_str(), generic.action),
            ("wrong_password", "Wrong email or app password.", None)
        );
        // A NO from SELECT is not a password problem.
        let mailbox = SetupError::new(
            SetupStage::Mailbox,
            ImapError::Imap(imap::Error::No("Unknown mailbox".into())),
        )
        .explain(&config, ServerHint::Gmail);
        assert_eq!(mailbox.message, "Signed in, but imap.gmail.com couldn't open your inbox.");
        assert!(mailbox.detail.contains("Unknown mailbox"));
    }

    #[test]
    fn known_domains_choose_their_provider_without_network() {
        for (address, provider) in [
            ("someone@gmail.com", KnownProvider::Gmail),
            ("Someone@GoogleMail.com ", KnownProvider::Gmail),
            ("someone@icloud.com", KnownProvider::Icloud),
            ("someone@me.com", KnownProvider::Icloud),
            ("someone@mac.com.", KnownProvider::Icloud),
            ("someone@hotmail.com", KnownProvider::Outlook),
            ("someone@example.com", KnownProvider::Other),
            ("someone@gmail.com.example", KnownProvider::Other),
            ("not an address", KnownProvider::Other),
            ("@gmail.com", KnownProvider::Other),
        ] {
            assert_eq!(provider_for_address(address), provider, "{address}");
        }
    }

    #[test]
    fn servers_are_guessed_from_the_domain() {
        let guess = guess_servers("person@Example.org").unwrap();
        assert_eq!(
            guess,
            ServerGuess {
                imap_host: "imap.example.org".into(),
                imap_port: 993,
                imap_security: TransportSecurity::ImplicitTls,
                smtp_host: "smtp.example.org".into(),
                smtp_port: 587,
                smtp_security: TransportSecurity::StartTls,
                known: false,
            }
        );
        let yahoo = guess_servers("person@yahoo.com").unwrap();
        assert!(yahoo.known);
        assert_eq!((yahoo.imap_host.as_str(), yahoo.smtp_port), ("imap.mail.yahoo.com", 465));
        let icloud = guess_servers("person@me.com").unwrap();
        assert_eq!((icloud.imap_host.as_str(), icloud.smtp_host.as_str()), ("imap.mail.me.com", "smtp.mail.me.com"));
        for invalid in ["person", "person@localhost", "person@exa mple.com", "person@.com", "person@a..b"] {
            assert!(guess_servers(invalid).is_none(), "{invalid}");
        }
    }
}
