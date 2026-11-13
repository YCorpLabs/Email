use crate::{CloudError, CloudProvider, OAuthConfig, Result, TokenSet, now_ms};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::{RngCore, rngs::OsRng};
use reqwest::blocking::Client;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    net::TcpListener,
    process::Command,
    time::{Duration, Instant},
};
use url::Url;

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: i64,
}

fn token_set(response: TokenResponse, old_refresh: Option<String>) -> TokenSet {
    TokenSet {
        access_token: response.access_token,
        refresh_token: response.refresh_token.or(old_refresh),
        expires_at_ms: now_ms() + response.expires_in.saturating_mul(1000),
    }
}

fn token_error(response: reqwest::blocking::Response) -> CloudError {
    let status = response.status().as_u16();
    let reason = response
        .json::<serde_json::Value>()
        .ok()
        .and_then(|body| {
            body.get("error")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "unknown error".to_owned());
    CloudError::OAuthToken { status, reason }
}

fn random_urlsafe() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

#[derive(Debug, PartialEq, Eq)]
enum Callback {
    Ignore,
    Denied,
    Code(String),
}

fn parse_callback(request: &[u8], expected_state: &str) -> Callback {
    let Ok(request) = std::str::from_utf8(request) else {
        return Callback::Ignore;
    };
    let mut words = request.lines().next().unwrap_or("").split_whitespace();
    if words.next() != Some("GET") {
        return Callback::Ignore;
    }
    let Some(target) = words.next().filter(|target| target.starts_with('/')) else {
        return Callback::Ignore;
    };
    let Ok(callback) = Url::parse(&format!("http://127.0.0.1{target}")) else {
        return Callback::Ignore;
    };
    if callback.path() != "/callback" {
        return Callback::Ignore;
    }
    let params: std::collections::HashMap<_, _> = callback.query_pairs().into_owned().collect();
    if params
        .get("state")
        .is_none_or(|state| state != expected_state)
    {
        return Callback::Ignore;
    }
    if params.contains_key("error") {
        return Callback::Denied;
    }
    match params.get("code").filter(|code| !code.is_empty()) {
        Some(code) => Callback::Code(code.clone()),
        None => Callback::Ignore,
    }
}

fn open_browser(url: &Url) -> Result<()> {
    let program = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(target_os = "windows") {
        "cmd"
    } else {
        "xdg-open"
    };
    let status = if cfg!(target_os = "windows") {
        Command::new(program)
            .args(["/C", "start", "", url.as_str()])
            .status()
    } else {
        Command::new(program).arg(url.as_str()).status()
    }?;
    if status.success() {
        Ok(())
    } else {
        Err(CloudError::Browser)
    }
}

/// Open the system browser for a desktop PKCE flow and wait up to five minutes for the callback.
/// Call this from a background thread in graphical applications.
pub fn authorize_interactive(config: &OAuthConfig) -> Result<TokenSet> {
    authorize_interactive_scoped(config, config.provider.scope(), None)
}

/// Request additional Google permissions for an existing account without
/// changing the ordinary mail sign-in consent screen.
///
/// Development-only: distribution builds compile the stub below instead, so the
/// public release never requests the Contacts scopes, which would need their
/// own Google review.
#[cfg(not(email_distribution))]
pub fn authorize_google_photos(config: &OAuthConfig, email: &str) -> Result<TokenSet> {
    if config.provider != CloudProvider::Gmail {
        return Err(CloudError::ProviderMismatch);
    }
    authorize_interactive_scoped(
        config,
        "https://www.googleapis.com/auth/gmail.modify https://www.googleapis.com/auth/contacts.readonly https://www.googleapis.com/auth/contacts.other.readonly",
        Some(email),
    )
}

/// Distribution builds do not offer Google contact photos.
#[cfg(email_distribution)]
pub fn authorize_google_photos(_config: &OAuthConfig, _email: &str) -> Result<TokenSet> {
    Err(CloudError::Unsupported(
        "Google contact photos are not available in this build",
    ))
}

/// Connect Calendar independently of ordinary mail access. Store these tokens separately.
/// The login hint is a convenience only; callers must verify google_calendar_identity().
pub fn authorize_google_calendar(config: &OAuthConfig, email: &str) -> Result<TokenSet> {
    if config.provider != CloudProvider::Gmail {
        return Err(CloudError::ProviderMismatch);
    }
    authorize_interactive_scoped(
        config,
        "openid email https://www.googleapis.com/auth/calendar.events https://www.googleapis.com/auth/calendar.calendarlist.readonly",
        Some(email),
    )
}

fn authorize_interactive_scoped(
    config: &OAuthConfig,
    scope: &str,
    login_hint: Option<&str>,
) -> Result<TokenSet> {
    if config.client_id.trim().is_empty() {
        return Err(CloudError::Response("OAuth client ID is empty"));
    }
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    // Microsoft ignores the ephemeral port only for `localhost` registrations.
    // Google's desktop app flow supports the numeric loopback address directly.
    let redirect_host = match config.provider {
        CloudProvider::Gmail => "127.0.0.1",
        CloudProvider::Microsoft => "localhost",
    };
    let redirect = format!(
        "http://{redirect_host}:{}/callback",
        listener.local_addr()?.port()
    );
    let verifier = random_urlsafe();
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let state = random_urlsafe();
    let mut url = Url::parse(config.provider.auth_endpoint())?;
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("client_id", &config.client_id)
            .append_pair("redirect_uri", &redirect)
            .append_pair("response_type", "code")
            .append_pair("scope", scope)
            .append_pair("code_challenge", &challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", &state);
        if config.provider == CloudProvider::Gmail {
            q.append_pair("access_type", "offline");
            q.append_pair("prompt", "consent");
            if let Some(email) = login_hint {
                q.append_pair("include_granted_scopes", "true");
                q.append_pair("login_hint", email);
            }
        }
    }
    open_browser(&url)?;
    let deadline = Instant::now() + Duration::from_secs(300);
    let code = loop {
        if Instant::now() > deadline {
            return Err(CloudError::AuthTimeout);
        }
        match listener.accept() {
            Ok((mut stream, _)) => {
                stream.set_read_timeout(Some(Duration::from_secs(3)))?;
                let mut bytes = [0u8; 8192];
                let callback = match stream.read(&mut bytes) {
                    Ok(n) => parse_callback(&bytes[..n], &state),
                    Err(_) => Callback::Ignore,
                };
                let (status, body) = match callback {
                    Callback::Code(_) => ("200 OK", "Authorization complete. Return to Email."),
                    Callback::Denied => ("200 OK", "Authorization denied. Return to Email."),
                    Callback::Ignore => ("400 Bad Request", "Invalid callback. Return to Email."),
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
                match callback {
                    Callback::Code(code) => break code,
                    Callback::Denied => return Err(CloudError::AuthDenied),
                    Callback::Ignore => continue,
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(50))
            }
            Err(error) => return Err(error.into()),
        }
    };
    let client = Client::builder().timeout(Duration::from_secs(30)).build()?;
    let mut form = vec![
        ("client_id", config.client_id.as_str()),
        ("code", code.as_str()),
        ("code_verifier", verifier.as_str()),
        ("redirect_uri", redirect.as_str()),
        ("grant_type", "authorization_code"),
    ];
    if let Some(secret) = &config.client_secret {
        form.push(("client_secret", secret));
    }
    let response = client
        .post(config.provider.token_endpoint())
        .form(&form)
        .send()?;
    if !response.status().is_success() {
        return Err(token_error(response));
    }
    Ok(token_set(response.json()?, None))
}

pub fn refresh(config: &OAuthConfig, old: &TokenSet) -> Result<TokenSet> {
    let token = old.refresh_token.as_deref().ok_or(CloudError::Response(
        "refresh token is missing; reconnect account",
    ))?;
    let client = Client::builder().timeout(Duration::from_secs(30)).build()?;
    let mut form = vec![
        ("client_id", config.client_id.as_str()),
        ("refresh_token", token),
        ("grant_type", "refresh_token"),
    ];
    if let Some(secret) = &config.client_secret {
        form.push(("client_secret", secret));
    }
    let response = client
        .post(config.provider.token_endpoint())
        .form(&form)
        .send()?;
    if !response.status().is_success() {
        return Err(token_error(response));
    }
    Ok(token_set(response.json()?, old.refresh_token.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_local_requests_cannot_abort_or_complete_oauth() {
        assert_eq!(
            parse_callback(b"GET /favicon.ico HTTP/1.1\r\n", "secret"),
            Callback::Ignore
        );
        assert_eq!(
            parse_callback(
                b"GET /callback?state=wrong&code=stolen HTTP/1.1\r\n",
                "secret"
            ),
            Callback::Ignore
        );
        assert_eq!(
            parse_callback(
                b"POST /callback?state=secret&code=bad HTTP/1.1\r\n",
                "secret"
            ),
            Callback::Ignore
        );
        assert_eq!(
            parse_callback(
                b"GET /callback?state=secret&error=access_denied HTTP/1.1\r\n",
                "secret"
            ),
            Callback::Denied
        );
        assert_eq!(
            parse_callback(
                b"GET /callback?state=secret&code=good HTTP/1.1\r\n",
                "secret"
            ),
            Callback::Code("good".into())
        );
    }
}
