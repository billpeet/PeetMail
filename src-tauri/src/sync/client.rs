//! The sync server's HTTP protocol (docs/SYNC.md → Protocol). The types mirror
//! server/src/lib/protocol.ts; both sides test against server/protocol-fixtures/examples.json.

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use url::Url;

use super::crypto::KdfParams;
use crate::error::{AppError, Result};

pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Health {
    pub ok: bool,
    pub protocol: u32,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRequest {
    pub provider: String,
    pub id_token: String,
    pub device_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterRequest {
    pub provider: String,
    pub id_token: String,
    pub device_name: String,
    pub wrapped_key: String,
    pub kdf_params: KdfParams,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub device_id: i64,
    pub device_token: String,
    pub wrapped_key: String,
    pub kdf_params: KdfParams,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkRequest {
    pub provider: String,
    pub id_token: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkedAccount {
    pub id: i64,
    pub provider: String,
    pub email: Option<String>,
    pub linked_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Item {
    pub id: String,
    pub seq: i64,
    pub blob: String,
    pub deleted: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pull {
    pub items: Vec<Item>,
    pub cursor: i64,
    pub more: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushItem {
    pub id: String,
    pub blob: String,
    pub deleted: bool,
    pub base_seq: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PushRequest {
    pub items: Vec<PushItem>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum PushResult {
    Ok { id: String, seq: i64 },
    Conflict { id: String, seq: i64, blob: String, deleted: bool },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Push {
    pub results: Vec<PushResult>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    pub id: i64,
    pub name: String,
    pub created_at: String,
    pub last_seen_at: String,
    pub current: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyRequest {
    pub wrapped_key: String,
    pub kdf_params: KdfParams,
}

#[derive(Debug, Clone, Deserialize)]
struct ErrorBody {
    error: String,
    message: String,
}

/// An error the server answered with: its stable code and the sentence it gave.
#[derive(Debug, Clone, PartialEq)]
pub struct ServerError {
    pub status: u16,
    pub code: String,
    pub message: String,
}

impl From<ServerError> for AppError {
    fn from(e: ServerError) -> Self {
        AppError::Other(e.message)
    }
}

/// What a call can come back with: the server's own refusal (to act on by code), or
/// anything else (network trouble, a broken server).
#[derive(Debug)]
pub enum CallError {
    Server(ServerError),
    Other(AppError),
}

impl CallError {
    pub fn code(&self) -> Option<&str> {
        match self {
            CallError::Server(e) => Some(&e.code),
            CallError::Other(_) => None,
        }
    }
}

impl From<CallError> for AppError {
    fn from(e: CallError) -> Self {
        match e {
            CallError::Server(e) => e.into(),
            CallError::Other(e) => e,
        }
    }
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CallError::Server(e) => write!(f, "{} ({} {})", e.message, e.status, e.code),
            CallError::Other(e) => write!(f, "{e}"),
        }
    }
}

pub type CallResult<T> = std::result::Result<T, CallError>;

/// Checks and tidies a server address the user typed: https, or plain http only to this PC.
/// A bare host gets https://.
pub fn parse_server_url(input: &str) -> Result<Url> {
    let input = input.trim();
    if input.is_empty() {
        return Err(AppError::Other("Enter your sync server's address.".into()));
    }
    let with_scheme = if input.contains("://") { input.to_string() } else { format!("https://{input}") };
    let mut url = Url::parse(&with_scheme).map_err(|_| AppError::Other(format!("\"{input}\" isn't a web address.")))?;
    let path = url.path().trim_end_matches('/').to_string();
    url.set_path(&path);
    let local = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    match url.scheme() {
        "https" => {}
        "http" if local => {}
        "http" => return Err(AppError::Other("The sync server needs an https:// address (plain http only works for this PC).".into())),
        _ => return Err(AppError::Other(format!("\"{input}\" isn't a web address."))),
    }
    if url.host_str().is_none() || url.query().is_some() || url.fragment().is_some() || !url.username().is_empty() {
        return Err(AppError::Other(format!("\"{input}\" isn't a server address.")));
    }
    Ok(url)
}

pub struct Client<'a> {
    http: &'a reqwest::Client,
    base: Url,
    token: Option<String>,
}

impl<'a> Client<'a> {
    pub fn new(http: &'a reqwest::Client, base: Url, token: Option<String>) -> Self {
        Self { http, base, token }
    }

    fn url(&self, path: &str) -> Url {
        // The server may live under a path (https://example.com/peetmail-sync).
        let mut u = self.base.clone();
        let joined = format!("{}/{}", u.path().trim_end_matches('/'), path.trim_start_matches('/'));
        u.set_path(&joined);
        u
    }

    async fn call<T: DeserializeOwned>(&self, method: reqwest::Method, path: &str, query: &[(&str, String)], body: Option<&impl Serialize>) -> CallResult<T> {
        let mut req = self.http.request(method, self.url(path)).timeout(std::time::Duration::from_secs(30)).query(query);
        if let Some(t) = &self.token {
            req = req.bearer_auth(t);
        }
        if let Some(b) = body {
            req = req.json(b);
        }
        let res = req.send().await.map_err(|e| CallError::Other(AppError::Other(format!("couldn't reach the sync server: {}", describe(e)))))?;
        let status = res.status();
        if status == reqwest::StatusCode::NO_CONTENT {
            return serde_json::from_str("null").map_err(|e| CallError::Other(e.into()));
        }
        let text = res.text().await.map_err(|e| CallError::Other(e.into()))?;
        if !status.is_success() {
            return Err(match serde_json::from_str::<ErrorBody>(&text) {
                Ok(e) => CallError::Server(ServerError { status: status.as_u16(), code: e.error, message: e.message }),
                Err(_) => CallError::Other(AppError::Other(format!("the sync server answered {status}, which isn't a PeetMail sync server's answer"))),
            });
        }
        serde_json::from_str(&text).map_err(|_| CallError::Other(AppError::Other("the sync server's answer wasn't what PeetMail expected (is it a PeetMail sync server?)".into())))
    }

    pub async fn health(&self) -> CallResult<Health> {
        self.call(reqwest::Method::GET, "v1/health", &[], None::<&()>).await
    }

    pub async fn session(&self, req: &SessionRequest) -> CallResult<Session> {
        self.call(reqwest::Method::POST, "v1/session", &[], Some(req)).await
    }

    pub async fn register(&self, req: &RegisterRequest) -> CallResult<Session> {
        self.call(reqwest::Method::POST, "v1/users", &[], Some(req)).await
    }

    pub async fn identities(&self) -> CallResult<Vec<LinkedAccount>> {
        self.call(reqwest::Method::GET, "v1/identities", &[], None::<&()>).await
    }

    pub async fn link(&self, req: &LinkRequest) -> CallResult<LinkedAccount> {
        self.call(reqwest::Method::POST, "v1/identities", &[], Some(req)).await
    }

    pub async fn unlink(&self, id: i64) -> CallResult<()> {
        self.call(reqwest::Method::DELETE, &format!("v1/identities/{id}"), &[], None::<&()>).await
    }

    pub async fn pull(&self, since: i64) -> CallResult<Pull> {
        self.call(reqwest::Method::GET, "v1/items", &[("since", since.to_string())], None::<&()>).await
    }

    pub async fn push(&self, items: Vec<PushItem>) -> CallResult<Push> {
        self.call(reqwest::Method::PUT, "v1/items", &[], Some(&PushRequest { items })).await
    }

    pub async fn devices(&self) -> CallResult<Vec<Device>> {
        self.call(reqwest::Method::GET, "v1/devices", &[], None::<&()>).await
    }

    pub async fn revoke(&self, id: i64) -> CallResult<()> {
        self.call(reqwest::Method::DELETE, &format!("v1/devices/{id}"), &[], None::<&()>).await
    }

    pub async fn set_key(&self, req: &KeyRequest) -> CallResult<()> {
        self.call(reqwest::Method::PUT, "v1/key", &[], Some(req)).await
    }

    pub async fn delete_user(&self) -> CallResult<()> {
        self.call(reqwest::Method::DELETE, "v1/users/me", &[], None::<&()>).await
    }
}

/// What went wrong reaching the server, in words, without the URL reqwest puts in.
fn describe(e: reqwest::Error) -> String {
    if e.is_timeout() {
        "it took too long to answer".into()
    } else if e.is_connect() {
        "nothing answered at that address".into()
    } else {
        e.without_url().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn examples() -> Value {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../server/protocol-fixtures/examples.json");
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    /// Reading an example and writing it back gives the same JSON: same fields, same names.
    fn round_trips<T: Serialize + DeserializeOwned>(name: &str) {
        let ex = examples()[name].clone();
        assert!(!ex.is_null(), "no example {name}");
        let parsed: T = serde_json::from_value(ex.clone()).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(serde_json::to_value(parsed).unwrap(), ex, "{name}");
    }

    #[test]
    fn reads_and_writes_the_servers_examples() {
        round_trips::<Health>("health.response");
        round_trips::<SessionRequest>("session.request");
        round_trips::<RegisterRequest>("users.request");
        round_trips::<Session>("session.response");
        round_trips::<LinkRequest>("identities.request");
        round_trips::<LinkedAccount>("identity.response");
        round_trips::<Vec<LinkedAccount>>("identities.response");
        round_trips::<PushRequest>("items.push.request");
        round_trips::<Push>("items.push.response");
        round_trips::<Pull>("items.pull.response");
        round_trips::<Vec<Device>>("devices.response");
        round_trips::<KeyRequest>("key.request");
        let e: ErrorBody = serde_json::from_value(examples()["error.response"].clone()).unwrap();
        assert_eq!(e.error, "no_user");
    }

    #[test]
    fn server_addresses_need_https_except_on_this_pc() {
        assert_eq!(parse_server_url("sync.example.com").unwrap().as_str(), "https://sync.example.com/");
        assert_eq!(parse_server_url(" https://sync.example.com/peetmail/ ").unwrap().as_str(), "https://sync.example.com/peetmail");
        assert!(parse_server_url("http://localhost:3000").is_ok());
        assert!(parse_server_url("http://127.0.0.1:3000").is_ok());
        assert!(parse_server_url("http://sync.example.com").unwrap_err().to_string().contains("https"));
        for bad in ["", "ftp://x", "https://", "https://x?y=1", "https://user@x", "not a url at all"] {
            assert!(parse_server_url(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn paths_join_under_a_server_that_lives_in_a_folder() {
        let http = reqwest::Client::new();
        let c = Client::new(&http, parse_server_url("https://x.example/sync").unwrap(), None);
        assert_eq!(c.url("v1/items").as_str(), "https://x.example/sync/v1/items");
        let c = Client::new(&http, parse_server_url("https://x.example").unwrap(), None);
        assert_eq!(c.url("v1/items").as_str(), "https://x.example/v1/items");
    }
}
