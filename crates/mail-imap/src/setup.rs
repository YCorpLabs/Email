//! Account-setup failures explained in words a person can act on.
//!
//! The IMAP and SMTP libraries report what the server said. Account setup needs
//! to say what to do next: use an app password, ask a Workspace administrator,
//! or confirm a blocked sign-in in the browser. Both native shells show these
//! messages unchanged.
use crate::{ImapConfig, ImapError};

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

    /// A short explanation and the next step. Never includes the password.
    /// `google_sign_in` says whether this build offers Google sign-in, the way
    /// in for accounts that cannot create app passwords.
    pub fn message(&self, config: &ImapConfig, hint: ServerHint, google_sign_in: bool) -> String {
        let no_app_passwords = if google_sign_in {
            NO_APP_PASSWORDS
        } else {
            NO_APP_PASSWORDS_YET
        };
        let (host, port) = match self.stage {
            SetupStage::Smtp => (&config.smtp_host, config.smtp_port),
            _ => (&config.imap_host, config.imap_port),
        };
        let gmail = hint == ServerHint::Gmail;
        match (self.problem(), hint) {
            (Problem::AppPasswordRequired, _) if gmail => format!(
                "Google needs an app password here, not your Google Account password. \
                 With 2-Step Verification on, create one at {APP_PASSWORDS_URL} and paste the 16 letters. \
                 {no_app_passwords}"
            ),
            (Problem::ImapDisabled, _) if gmail => "Your Google Workspace administrator has turned off IMAP for this account. \
                 Ask them to enable IMAP access in the Google Admin console (Apps › Google Workspace › Gmail › End User Access), then try again."
                .into(),
            (Problem::WebLoginRequired, _) if gmail => "Google blocked this sign-in. Open Gmail in your browser on this computer, \
                 confirm that the sign-in was you, then try again."
                .into(),
            (Problem::TooManyConnections, _) => format!(
                "{host} has too many open connections for this account. Quit other mail apps or wait a few minutes, then try again."
            ),
            (Problem::Rejected, ServerHint::Gmail) => format!(
                "Google didn't accept this app password. Check the email address, or create a new app password at \
                 {APP_PASSWORDS_URL} and paste it here. Your regular Google password won't work. {no_app_passwords}"
            ),
            (Problem::Rejected, ServerHint::Icloud) => "Apple didn't accept this app-specific password. Check your iCloud address, \
                 or create a new app-specific password in your Apple Account and paste it here."
                .into(),
            (Problem::Rejected, ServerHint::Generic) => format!(
                "{host} didn't accept the username or password. Use the app password your provider issued, \
                 not your main account password."
            ),
            (Problem::Unreachable, _) => format!(
                "Couldn't reach {host}:{port}. Check your internet connection{}, then try again.",
                if hint == ServerHint::Generic { " and the server settings" } else { "" }
            ),
            (Problem::Tls, _) => format!(
                "Couldn't make a secure connection to {host}:{port}. Check the server name and security setting, then try again."
            ),
            _ => match self.stage {
                SetupStage::Mailbox => format!("Signed in to {host}, but it couldn't open your inbox: {}", self.error),
                SetupStage::Smtp => format!("Signed in to receive mail, but sending through {host}:{port} failed: {}", self.error),
                _ => format!("Couldn't connect to {host}:{port}: {}", self.error),
            },
        }
    }
}

/// Google's app-password page. Shown in setup errors; the setup windows link to
/// the help article (support.google.com/accounts/answer/185833).
const APP_PASSWORDS_URL: &str = "myaccount.google.com/apppasswords";
/// The accounts that cannot create app passwords (Google help 185833).
const NO_APP_PASSWORDS: &str = "No app passwords option? Accounts that use only security keys, \
     Advanced Protection, or a work or school policy can't use them; choose Gmail to sign in with Google instead.";
/// The same, for builds that do not offer Google sign-in yet.
const NO_APP_PASSWORDS_YET: &str = "No app passwords option? Accounts that use only security keys, \
     Advanced Protection, or a work or school policy can't use them. One-click Google sign-in is coming once \
     Google finishes reviewing the app.";

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
    fn gmail_failures_explain_the_next_step() {
        let config = gmail();
        // Exact Gmail IMAP responses, as the imap crate reports them.
        let wrong = login_no("[AUTHENTICATIONFAILED] Invalid credentials (Failure)").message(
            &config,
            ServerHint::Gmail,
            true,
        );
        assert!(
            wrong.starts_with("Google didn't accept this app password"),
            "{wrong}"
        );
        assert!(wrong.contains("security keys"), "{wrong}");
        let regular = login_no("Application-specific password required: https://support.google.com/accounts/answer/185833 (Failure)")
            .message(&config, ServerHint::Gmail, true);
        assert!(
            regular.starts_with("Google needs an app password"),
            "{regular}"
        );
        assert!(regular.contains("security keys"), "{regular}");
        let admin = login_no("IMAP access is disabled for your domain. Please contact your domain administrator for questions about this feature. (Failure)")
            .message(&config, ServerHint::Gmail, true);
        assert!(
            admin.contains("Workspace administrator has turned off IMAP"),
            "{admin}"
        );
        let web =
            login_no("[WEBALERT https://accounts.google.com/signin/continue] Web login required.")
                .message(&config, ServerHint::Gmail, true);
        assert!(web.starts_with("Google blocked this sign-in"), "{web}");
        let busy = login_no("Too many simultaneous connections. (Failure)").message(
            &config,
            ServerHint::Gmail,
            true,
        );
        assert!(
            busy.starts_with("imap.gmail.com has too many open connections"),
            "{busy}"
        );
        for message in [wrong, regular, admin, web, busy] {
            assert!(!message.contains("abcdefghijklmnop"));
        }
        // Without Google sign-in in this build, the advice must not point to it.
        let wrong_without = login_no("[AUTHENTICATIONFAILED] Invalid credentials (Failure)")
            .message(&config, ServerHint::Gmail, false);
        assert!(
            !wrong_without.contains("sign in with Google instead"),
            "{wrong_without}"
        );
        assert!(
            wrong_without.ends_with(
                "One-click Google sign-in is coming once Google finishes reviewing the app."
            ),
            "{wrong_without}"
        );
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
        assert_eq!(
            refused.message(&config, ServerHint::Gmail, true),
            "Couldn't reach imap.gmail.com:993. Check your internet connection, then try again."
        );
        let generic = login_no("LOGIN failed").message(&config, ServerHint::Generic, true);
        assert!(
            generic.starts_with("imap.gmail.com didn't accept the username or password"),
            "{generic}"
        );
        let icloud = login_no("Authentication failed").message(&config, ServerHint::Icloud, true);
        assert!(icloud.starts_with("Apple didn't accept"), "{icloud}");
        // A NO from SELECT is not a password problem.
        let mailbox = SetupError::new(
            SetupStage::Mailbox,
            ImapError::Imap(imap::Error::No("Unknown mailbox".into())),
        )
        .message(&config, ServerHint::Gmail, true);
        assert!(
            mailbox.starts_with("Signed in to imap.gmail.com, but it couldn't open your inbox"),
            "{mailbox}"
        );
    }
}
