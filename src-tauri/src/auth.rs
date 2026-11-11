//! OAuth sign-in for Microsoft and Google: authorization code + PKCE over a loopback
//! redirect. PeetMail is a public client; Google's "Desktop app" clients come with a
//! secret that isn't confidential (Google says so), which is sent when configured.

use std::collections::HashMap;
use std::time::Duration;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::AppHandle;
use tauri_plugin_opener::OpenerExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use url::Url;

use crate::error::{AppError, Result};

/// `common` accepts both work/school (M365) and personal (outlook.com) accounts.
const DEFAULT_AUTHORITY: &str = "https://login.microsoftonline.com/common/oauth2/v2.0";

static AUTHORITY: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    crate::testenv::get("PEETMAIL_AUTHORITY_URL").unwrap_or_else(|| DEFAULT_AUTHORITY.to_string())
});

/// Requested up front so later phases (send, calendar) don't need a second consent prompt.
pub const SCOPES: &str =
    "offline_access openid profile email User.Read Mail.ReadWrite Mail.Send Calendars.ReadWrite";

const GOOGLE_AUTHORIZE: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const GOOGLE_TOKEN: &str = "https://oauth2.googleapis.com/token";
/// gmail.modify: read, label, send (no permanent delete). calendar.events: read and write
/// events on your calendars.
pub const GOOGLE_SCOPES: &str =
    "openid email profile https://www.googleapis.com/auth/gmail.modify https://www.googleapis.com/auth/calendar.events";

/// Optional Microsoft permissions an account can grant later (Settings or the event editor),
/// on top of SCOPES. User.ReadBasic.All lets findRooms list the tenant's rooms where plain
/// User.Read isn't enough; users can consent to it themselves.
pub const SCOPE_DIRECTORY: &str = "User.ReadBasic.All";
/// The Places API's room list: needs an admin's approval.
pub const SCOPE_PLACES: &str = "Place.Read.All";

/// `base` plus `extra`, without repeats (scopes are case-insensitive).
pub fn merge_scopes(base: &str, extra: &[&str]) -> String {
    let mut out: Vec<&str> = Vec::new();
    for s in base.split_whitespace().chain(extra.iter().flat_map(|e| e.split_whitespace())) {
        if !out.iter().any(|o| o.eq_ignore_ascii_case(s)) {
            out.push(s);
        }
    }
    out.join(" ")
}

pub fn has_scope(scopes: &str, scope: &str) -> bool {
    scopes.split_whitespace().any(|s| s.eq_ignore_ascii_case(scope) || s.rsplit('/').next().is_some_and(|t| t.eq_ignore_ascii_case(scope)))
}

/// Where and how one identity provider signs in.
pub struct OAuthConfig {
    authorize_url: String,
    token_url: String,
    client_id: String,
    client_secret: Option<String>,
    scopes: String,
    /// Pre-fills the account picker (reconnecting a known account).
    login_hint: Option<String>,
    /// Extra authorize parameters.
    extra: &'static [(&'static str, &'static str)],
    /// Loopback host for the redirect (Entra wants "localhost", Google "127.0.0.1").
    redirect_host: &'static str,
    /// Entra wants the scopes repeated on token requests; Google doesn't.
    scope_on_token: bool,
}

impl OAuthConfig {
    pub fn microsoft(client_id: &str) -> Self {
        Self {
            authorize_url: format!("{}/authorize", *AUTHORITY),
            token_url: format!("{}/token", *AUTHORITY),
            client_id: client_id.into(),
            client_secret: None,
            scopes: SCOPES.into(),
            login_hint: None,
            extra: &[("response_mode", "query"), ("prompt", "select_account")],
            redirect_host: "localhost",
            scope_on_token: true,
        }
    }

    pub fn google(client_id: &str, client_secret: Option<&str>) -> Self {
        let base = crate::testenv::get("PEETMAIL_GOOGLE_AUTH_URL");
        Self {
            authorize_url: base.as_ref().map(|b| format!("{b}/authorize")).unwrap_or_else(|| GOOGLE_AUTHORIZE.into()),
            token_url: base.as_ref().map(|b| format!("{b}/token")).unwrap_or_else(|| GOOGLE_TOKEN.into()),
            client_id: client_id.into(),
            client_secret: client_secret.filter(|s| !s.is_empty()).map(String::from),
            scopes: GOOGLE_SCOPES.into(),
            login_hint: None,
            // offline + consent: always hand back a refresh token, even on a repeat sign-in.
            extra: &[("access_type", "offline"), ("prompt", "consent select_account")],
            redirect_host: "127.0.0.1",
            scope_on_token: false,
        }
    }
}

impl OAuthConfig {
    pub fn scopes(&self) -> &str {
        &self.scopes
    }

    /// Signs in with these scopes instead of the defaults (the defaults plus extras granted later).
    pub fn with_scopes(mut self, scopes: String) -> Self {
        self.scopes = scopes;
        self
    }

    pub fn with_login_hint(mut self, email: &str) -> Self {
        self.login_hint = Some(email.into());
        self
    }
}

const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenSet {
    pub access_token: String,
    pub refresh_token: String,
    /// Unix seconds.
    pub expires_at: i64,
    /// What was asked for (and consented to) at sign-in. Entra wants the scopes again on every
    /// refresh, and asking for one never granted fails it, so each account keeps its own.
    /// None: signed in before this existed, i.e. the provider's defaults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scopes: Option<String>,
}

impl TokenSet {
    pub fn expires_soon(&self) -> bool {
        self.expires_at - chrono::Utc::now().timestamp() < 300
    }
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: i64,
    id_token: Option<String>,
}

/// The display name in an OpenID id_token (its signature doesn't matter here: it came
/// straight from the token endpoint over TLS).
pub fn id_token_name(id_token: &str) -> Option<String> {
    let payload = id_token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    v["name"].as_str().filter(|s| !s.is_empty()).map(String::from)
}

#[derive(Deserialize)]
struct TokenError {
    error: String,
    error_description: Option<String>,
}

fn random_b64(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf).expect("OS RNG unavailable");
    URL_SAFE_NO_PAD.encode(buf)
}

pub fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// Opens the system browser, waits for the redirect, and exchanges the code for tokens.
/// Resolves early with an error if `cancel` fires (user pressed Esc, or started another sign-in).
/// Returns the tokens and, when the provider sends one, the OpenID id_token.
pub async fn sign_in(
    app: &AppHandle,
    http: &reqwest::Client,
    cfg: &OAuthConfig,
    mut cancel: tokio::sync::oneshot::Receiver<()>,
) -> Result<(TokenSet, Option<String>)> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    // Both providers allow any port on a registered loopback redirect for desktop apps.
    let redirect_uri = format!("http://{}:{port}", cfg.redirect_host);
    let verifier = random_b64(64);
    let state = random_b64(16);

    let challenge = pkce_challenge(&verifier);
    let mut params: Vec<(&str, &str)> = vec![
        ("client_id", &cfg.client_id),
        ("response_type", "code"),
        ("redirect_uri", &redirect_uri),
        ("scope", &cfg.scopes),
        ("state", &state),
        ("code_challenge", &challenge),
        ("code_challenge_method", "S256"),
    ];
    params.extend(cfg.extra.iter().copied());
    if let Some(hint) = &cfg.login_hint {
        params.push(("login_hint", hint));
    }
    let auth_url = Url::parse_with_params(&cfg.authorize_url, &params).map_err(|e| AppError::Auth(e.to_string()))?;

    if crate::testenv::flag("PEETMAIL_AUTH_AUTOFOLLOW") {
        // Test harness: the fake authority redirects straight back to the loopback listener.
        let http = http.clone();
        let url = auth_url.to_string();
        tauri::async_runtime::spawn(async move {
            if let Err(e) = http.get(url).send().await {
                log::error!("auth autofollow failed: {e}");
            }
        });
    } else {
        app.opener()
            .open_url(auth_url.as_str(), None::<&str>)
            .map_err(|e| AppError::Auth(format!("could not open browser: {e}")))?;
    }

    // A wrong client ID shows an error page in the browser and never redirects back,
    // so cancellation is the only way out short of the timeout.
    let code = tokio::select! {
        r = tokio::time::timeout(SIGN_IN_TIMEOUT, wait_for_code(listener, &state)) => {
            r.map_err(|_| AppError::Auth("timed out waiting for the browser".into()))??
        }
        _ = &mut cancel => return Err(AppError::Auth("cancelled".into())),
    };

    let exchange = || async {
        let mut form = vec![
            ("client_id", cfg.client_id.as_str()),
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", redirect_uri.as_str()),
            ("code_verifier", verifier.as_str()),
        ];
        token_request(http, cfg, &cfg.scopes, &mut form, None).await
    };
    // Android cuts a backgrounded app off the network, and the redirect arrives while the
    // browser is in front: keep the code and try again until the user switches back.
    // TODO(android stage 4): the deep-link redirect brings PeetMail to the front first.
    if cfg!(mobile) {
        retry_while_offline(exchange, is_offline, OFFLINE_RETRY_EVERY, OFFLINE_RETRY_FOR, cancel).await
    } else {
        exchange().await
    }
}

/// How often, and for how long, a token exchange that couldn't reach the network is retried
/// (mobile). Authorization codes last about ten minutes.
const OFFLINE_RETRY_EVERY: Duration = Duration::from_secs(2);
const OFFLINE_RETRY_FOR: Duration = Duration::from_secs(5 * 60);

/// The request never got a connection: no network, or the OS blocked it (Android, in the
/// background). A server that answered, even with an error, doesn't count.
fn is_offline(e: &AppError) -> bool {
    matches!(e, AppError::Http(e) if e.is_connect() || (e.is_request() && e.status().is_none()))
}

/// Runs `op` until it succeeds or fails for a reason `offline` doesn't cover, waiting `every`
/// between tries. Gives up with the last error after `limit`, or with "cancelled" when
/// `cancel` fires.
async fn retry_while_offline<T, F, Fut>(
    mut op: F,
    offline: fn(&AppError) -> bool,
    every: Duration,
    limit: Duration,
    mut cancel: tokio::sync::oneshot::Receiver<()>,
) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let deadline = tokio::time::Instant::now() + limit;
    let mut logged = false;
    loop {
        match op().await {
            Err(e) if offline(&e) && tokio::time::Instant::now() + every < deadline => {
                if !logged {
                    log::info!("sign-in: no network to finish with ({e}); retrying until PeetMail is back in front");
                    logged = true;
                }
            }
            other => return other,
        }
        tokio::select! {
            _ = tokio::time::sleep(every) => {}
            _ = &mut cancel => return Err(AppError::Auth("cancelled".into())),
        }
    }
}

/// New tokens for the same scopes the account signed in with.
pub async fn refresh(http: &reqwest::Client, cfg: &OAuthConfig, current: &TokenSet) -> Result<TokenSet> {
    Ok(refresh_with_id_token(http, cfg, current).await?.0)
}

/// A refresh that also gives back the provider's id_token, when it sends one (both do with
/// `openid` in the scopes). Settings sync uses it as proof of who the account is.
pub async fn refresh_with_id_token(http: &reqwest::Client, cfg: &OAuthConfig, current: &TokenSet) -> Result<(TokenSet, Option<String>)> {
    let scopes = current.scopes.clone().unwrap_or_else(|| cfg.scopes.clone());
    let mut form = vec![("client_id", cfg.client_id.as_str()), ("grant_type", "refresh_token"), ("refresh_token", current.refresh_token.as_str())];
    token_request(http, cfg, &scopes, &mut form, Some(&current.refresh_token)).await
}

async fn token_request<'a>(
    http: &reqwest::Client,
    cfg: &'a OAuthConfig,
    scopes: &'a str,
    form: &mut Vec<(&'a str, &'a str)>,
    old_refresh: Option<&str>,
) -> Result<(TokenSet, Option<String>)> {
    if cfg.scope_on_token {
        form.push(("scope", scopes));
    }
    if let Some(secret) = &cfg.client_secret {
        form.push(("client_secret", secret));
    }
    // No Origin header here: Entra treats requests with one as SPA traffic and rejects them.
    let res = http.post(&cfg.token_url).form(&*form).send().await?;
    if !res.status().is_success() {
        let err: TokenError = res.json().await?;
        return Err(AppError::Auth(format!(
            "{}: {}",
            err.error,
            err.error_description.unwrap_or_default().lines().next().unwrap_or_default()
        )));
    }
    let t: TokenResponse = res.json().await?;
    let refresh_token = t
        .refresh_token
        .or_else(|| old_refresh.map(String::from))
        .ok_or_else(|| AppError::Auth("no refresh token returned (is offline_access granted?)".into()))?;
    Ok((
        TokenSet { access_token: t.access_token, refresh_token, expires_at: chrono::Utc::now().timestamp() + t.expires_in, scopes: Some(scopes.to_string()) },
        t.id_token,
    ))
}

async fn wait_for_code(listener: TcpListener, expected_state: &str) -> Result<String> {
    loop {
        let (mut sock, _) = listener.accept().await?;
        let mut buf = vec![0u8; 16 * 1024];
        let n = sock.read(&mut buf).await?;
        let req = String::from_utf8_lossy(&buf[..n]);
        let params = parse_redirect(&req);

        if !params.contains_key("code") && !params.contains_key("error") {
            // favicon.ico and other stray requests
            let _ = sock.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
            continue;
        }

        let outcome = if let Some(err) = params.get("error") {
            Err(AppError::Auth(format!("{err}: {}", params.get("error_description").cloned().unwrap_or_default())))
        } else if params.get("state").map(String::as_str) != Some(expected_state) {
            Err(AppError::Auth("state mismatch".into()))
        } else {
            Ok(params["code"].clone())
        };

        let message = match &outcome {
            Ok(_) if cfg!(mobile) => "Signed in. Go back to PeetMail to finish.",
            Ok(_) => "Signed in. You can close this tab and go back to PeetMail.",
            Err(_) => "Sign-in failed. Go back to PeetMail for details.",
        };
        let body = format!(
            "<!doctype html><meta charset=utf-8><title>PeetMail</title>\
             <body style=\"font:16px system-ui;background:#0b0b0d;color:#e7e7ea;display:grid;place-items:center;height:100vh;margin:0\">\
             <p>{message}</p>"
        );
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = sock.write_all(resp.as_bytes()).await;
        return outcome;
    }
}

fn parse_redirect(request: &str) -> HashMap<String, String> {
    let path = request.lines().next().and_then(|l| l.split_whitespace().nth(1)).unwrap_or("/");
    Url::parse(&format!("http://localhost{path}"))
        .map(|u| u.query_pairs().into_owned().collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scopes_merge_without_repeats_and_match_short_or_full_names() {
        assert_eq!(merge_scopes("openid User.Read", &["user.read", "User.ReadBasic.All"]), "openid User.Read User.ReadBasic.All");
        assert!(has_scope("openid https://graph.microsoft.com/User.ReadBasic.All", SCOPE_DIRECTORY));
        assert!(!has_scope("openid User.Read", SCOPE_DIRECTORY));
        // Tokens stored before per-account scopes still load.
        let old: TokenSet = serde_json::from_str(r#"{"access_token":"a","refresh_token":"r","expires_at":1}"#).unwrap();
        assert_eq!(old.scopes, None);
    }

    #[test]
    fn pkce_matches_rfc7636_example() {
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn reads_the_name_from_an_id_token() {
        let payload = URL_SAFE_NO_PAD.encode(br#"{"email":"b@gmail.com","name":"Boris Dragon"}"#);
        assert_eq!(id_token_name(&format!("eyJhbGciOiJub25lIn0.{payload}.")), Some("Boris Dragon".into()));
        assert_eq!(id_token_name("not-a-jwt"), None);
    }

    fn offline() -> AppError {
        AppError::Other("offline".into())
    }

    fn is_fake_offline(e: &AppError) -> bool {
        matches!(e, AppError::Other(m) if m == "offline")
    }

    #[tokio::test(start_paused = true)]
    async fn retries_a_token_exchange_until_the_network_is_back() {
        let tries = std::sync::atomic::AtomicU32::new(0);
        let (_keep, cancel) = tokio::sync::oneshot::channel();
        let op = || async {
            if tries.fetch_add(1, std::sync::atomic::Ordering::SeqCst) < 3 { Err(offline()) } else { Ok(42) }
        };
        let r = retry_while_offline(op, is_fake_offline, Duration::from_secs(2), Duration::from_secs(60), cancel).await;
        assert_eq!(r.unwrap(), 42);
        assert_eq!(tries.load(std::sync::atomic::Ordering::SeqCst), 4);
    }

    #[tokio::test(start_paused = true)]
    async fn gives_up_on_other_errors_at_once_and_on_offline_after_the_limit() {
        let (_keep, cancel) = tokio::sync::oneshot::channel();
        let r: Result<()> = retry_while_offline(|| async { Err(AppError::Auth("invalid_grant".into())) }, is_fake_offline, Duration::from_secs(2), Duration::from_secs(60), cancel).await;
        assert!(matches!(r, Err(AppError::Auth(m)) if m == "invalid_grant"));

        let tries = std::sync::atomic::AtomicU32::new(0);
        let (_keep, cancel) = tokio::sync::oneshot::channel();
        let op = || async {
            tries.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err::<(), _>(offline())
        };
        let r = retry_while_offline(op, is_fake_offline, Duration::from_secs(2), Duration::from_secs(10), cancel).await;
        assert!(r.is_err_and(|e| is_fake_offline(&e)));
        assert_eq!(tries.load(std::sync::atomic::Ordering::SeqCst), 5);
    }

    #[tokio::test(start_paused = true)]
    async fn cancelling_stops_the_retries() {
        let (stop, cancel) = tokio::sync::oneshot::channel();
        stop.send(()).unwrap();
        let r: Result<()> = retry_while_offline(|| async { Err(offline()) }, is_fake_offline, Duration::from_secs(2), Duration::from_secs(60), cancel).await;
        assert!(matches!(r, Err(AppError::Auth(m)) if m == "cancelled"));
    }

    #[test]
    fn parses_redirect_query() {
        let p = parse_redirect("GET /?code=abc%2F1&state=xyz HTTP/1.1\r\nHost: localhost\r\n\r\n");
        assert_eq!(p["code"], "abc/1");
        assert_eq!(p["state"], "xyz");
        assert!(parse_redirect("GET /favicon.ico HTTP/1.1\r\n").is_empty());
    }
}
