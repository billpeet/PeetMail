use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::auth::{self, TokenSet};
use crate::db::{self, Db};
use crate::error::{AppError, Result};
use crate::secure;
use crate::testenv;

pub const KEY_CLIENT_ID: &str = "ms_client_id";

/// The OAuth app compiled into this build (CI sets these from repo secrets).
pub const APP_BUILTIN: &str = "builtin";
/// The user's own app, set under Settings → Advanced.
pub const APP_CUSTOM: &str = "custom";

fn present(v: Option<String>) -> Option<String> {
    v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// The built-in app for a provider: (client ID, secret). Debug builds can stand one in
/// (PEETMAIL_BUILTIN_*) so tests cover the built-in sign-in path.
pub fn builtin_client(provider: &str) -> Option<(String, Option<String>)> {
    let (id, secret) = if provider == "google" {
        (
            testenv::get("PEETMAIL_BUILTIN_GOOGLE_CLIENT_ID").or_else(|| option_env!("PEETMAIL_GOOGLE_CLIENT_ID").map(String::from)),
            testenv::get("PEETMAIL_BUILTIN_GOOGLE_CLIENT_SECRET").or_else(|| option_env!("PEETMAIL_GOOGLE_CLIENT_SECRET").map(String::from)),
        )
    } else {
        (testenv::get("PEETMAIL_BUILTIN_MS_CLIENT_ID").or_else(|| option_env!("PEETMAIL_MS_CLIENT_ID").map(String::from)), None)
    };
    present(id).map(|id| (id, present(secret)))
}
pub const KEY_GOOGLE_CLIENT_ID: &str = "google_client_id";
pub const KEY_GOOGLE_CLIENT_SECRET: &str = "google_client_secret";
pub const KEY_CACHE_DAYS: &str = "cache_days";
pub const KEY_BODY_CACHE_MB: &str = "body_cache_mb";

pub const KEY_HIDE_EMPTY_SPLITS: &str = "hide_empty_splits";
pub const KEY_SHOW_RIGHT_PANE: &str = "show_right_pane";
pub const KEY_RIGHT_PANE_WIDTH: &str = "right_pane_width";
pub const DEFAULT_RIGHT_PANE_WIDTH: i64 = 320;
pub const KEY_LAYOUT: &str = "layout";
pub const KEY_LIST_WIDTH: &str = "list_width";
pub const DEFAULT_LIST_WIDTH: i64 = 420;
pub const KEY_RIGHT_PANE_TAB: &str = "right_pane_tab";
pub const KEY_WEEK_STARTS_ON: &str = "week_starts_on";
pub const KEY_UNDO_SEND_SECS: &str = "undo_send_secs";
pub const KEY_NOTIFY: &str = "notify";
pub const KEY_CLOSE_TO_TRAY: &str = "close_to_tray";
pub const DEFAULT_UNDO_SEND_SECS: i64 = 10;

pub const DEFAULT_CACHE_DAYS: i64 = 90;
/// A phone has less room, and its storage is shared with photos and apps.
pub const DEFAULT_BODY_CACHE_MB: i64 = if cfg!(target_os = "android") { 100 } else { 500 };

pub struct AppState {
    pub db: Db,
    pub http: reqwest::Client,
    tokens: tokio::sync::Mutex<HashMap<i64, TokenSet>>,
    /// Serializes sync runs so the timer and "sync now" never overlap.
    pub sync_lock: tokio::sync::Mutex<()>,
    /// Cancels the sign-in currently waiting on the browser, if any.
    pub sign_in_cancel: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    /// Held while the outbox worker drains, so only one drain runs.
    pub outbox_lock: tokio::sync::Mutex<()>,
    /// Most recent undoable actions, newest last.
    pub undo: std::sync::Mutex<Vec<crate::actions::UndoGroup>>,
    /// Held while bodies are prefetched in the background, so runs don't pile up.
    pub prefetch_lock: tokio::sync::Mutex<()>,
    /// Settings sync's state in memory: a sign-in waiting for its passphrase, the last run.
    pub settings_sync: std::sync::Mutex<crate::sync::engine::Memory>,
    /// Serializes settings sync runs (timer, "sync now", turning it on).
    pub settings_sync_lock: tokio::sync::Mutex<()>,
    /// Held while drafts are pushed to or pulled from a server, so a pull never sees a
    /// server draft whose id a push hasn't stored yet (and takes it for a new one).
    pub drafts_lock: tokio::sync::Mutex<()>,
    /// Server drafts that are really messages being sent (graph_send builds them as drafts).
    pub transient_drafts: std::sync::Mutex<std::collections::HashSet<String>>,
}

/// UI preferences that live in the settings table.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Preferences {
    /// Hide split tabs with no conversations (the Inbox tab always shows).
    pub hide_empty_splits: bool,
    /// Show the right-hand pane (Recent Opens). On by default.
    pub show_right_pane: bool,
    /// Right pane width in CSS pixels (the UI clamps it to fit the window).
    pub right_pane_width: i64,
    /// "list" (list, Enter opens a conversation) or "preview" (list + reading pane).
    pub layout: String,
    /// Width of the list in the preview layout, in CSS pixels.
    pub list_width: i64,
    /// What the right pane shows: "calendar" (day view, the default) or "recent" (Recent Opens).
    pub right_pane_tab: String,
    /// First day of the week in the calendar: 0 = Sunday, 1 = Monday ... 6 = Saturday.
    pub week_starts_on: i64,
    /// How long a sent message waits, so it can be undone. 0 sends at once.
    pub undo_send_secs: i64,
    /// New-mail notifications: "off", "focused" (default; not Other) or "all".
    pub notify: String,
    /// Closing the window keeps PeetMail running in the tray (default on).
    pub close_to_tray: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheLimits {
    /// Only mail received in the last N days is cached locally.
    pub cache_days: i64,
    /// Upper bound for cached message bodies; least-recently-read are evicted first.
    pub body_cache_mb: i64,
}

impl AppState {
    pub fn new(db: Db) -> Self {
        let http = reqwest::Client::builder()
            .user_agent(concat!("PeetMail/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("http client");
        Self {
            db,
            http,
            tokens: Default::default(),
            sync_lock: Default::default(),
            sign_in_cancel: Default::default(),
            outbox_lock: Default::default(),
            undo: Default::default(),
            prefetch_lock: Default::default(),
            drafts_lock: Default::default(),
            transient_drafts: Default::default(),
            settings_sync: Default::default(),
            settings_sync_lock: Default::default(),
        }
    }

    /// The user's own OAuth app for a provider (Settings → Advanced): (client ID, secret).
    /// The secret of a Google Desktop-app client isn't confidential (Google says so), but
    /// Google may still require it on token requests.
    pub fn custom_client(&self, provider: &str) -> Result<Option<(String, Option<String>)>> {
        let (id, secret) = self.db.with(|c| {
            Ok(if provider == "google" {
                (db::get_setting(c, KEY_GOOGLE_CLIENT_ID)?, db::get_setting(c, KEY_GOOGLE_CLIENT_SECRET)?)
            } else {
                (db::get_setting(c, KEY_CLIENT_ID)?, None)
            })
        })?;
        Ok(present(id).map(|id| (id, present(secret))))
    }

    /// The app a new sign-in uses unless one is chosen: the built-in one when this build
    /// has it, otherwise the user's own.
    pub fn default_app(&self, provider: &str) -> &'static str {
        if builtin_client(provider).is_some() { APP_BUILTIN } else { APP_CUSTOM }
    }

    /// How a provider signs in with a given app.
    pub fn oauth_for_app(&self, provider: &str, app: &str) -> Result<auth::OAuthConfig> {
        let name = if provider == "google" { "Google" } else { "Microsoft" };
        let client = if app == APP_CUSTOM {
            self.custom_client(provider)?.ok_or_else(|| AppError::Other(format!("Set up your own {name} app first (Settings → Advanced).")))?
        } else {
            builtin_client(provider).ok_or_else(|| AppError::Other(format!("This build has no built-in {name} app. Set up your own under Settings → Advanced.")))?
        };
        Ok(if provider == "google" {
            auth::OAuthConfig::google(&client.0, client.1.as_deref())
        } else {
            auth::OAuthConfig::microsoft(&client.0)
        })
    }

    /// How an existing account refreshes and reconnects: with the app it signed in with.
    pub fn oauth_for_account(&self, account_id: i64) -> Result<auth::OAuthConfig> {
        let (provider, app) = self.db.with(|c| db::account_app(c, account_id))?;
        self.oauth_for_app(&provider, &app)
    }

    pub fn cache_limits(&self) -> Result<CacheLimits> {
        self.db.with(|c| {
            let get = |k, d: i64| -> Result<i64> {
                Ok(db::get_setting(c, k)?.and_then(|v| v.parse().ok()).unwrap_or(d))
            };
            Ok(CacheLimits {
                cache_days: get(KEY_CACHE_DAYS, DEFAULT_CACHE_DAYS)?,
                body_cache_mb: get(KEY_BODY_CACHE_MB, DEFAULT_BODY_CACHE_MB)?,
            })
        })
    }

    pub fn preferences(&self) -> Result<Preferences> {
        self.db.with(|c| {
            Ok(Preferences {
                hide_empty_splits: db::get_setting(c, KEY_HIDE_EMPTY_SPLITS)?.as_deref() == Some("1"),
                show_right_pane: db::get_setting(c, KEY_SHOW_RIGHT_PANE)?.as_deref() != Some("0"),
                right_pane_width: db::get_setting(c, KEY_RIGHT_PANE_WIDTH)?
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(DEFAULT_RIGHT_PANE_WIDTH),
                layout: match db::get_setting(c, KEY_LAYOUT)?.as_deref() {
                    Some("preview") => "preview".into(),
                    _ => "list".into(),
                },
                list_width: db::get_setting(c, KEY_LIST_WIDTH)?.and_then(|v| v.parse().ok()).unwrap_or(DEFAULT_LIST_WIDTH),
                right_pane_tab: match db::get_setting(c, KEY_RIGHT_PANE_TAB)?.as_deref() {
                    // Calendar unless Recent Opens was chosen.
                    Some("recent") => "recent".into(),
                    _ => "calendar".into(),
                },
                week_starts_on: db::get_setting(c, KEY_WEEK_STARTS_ON)?
                    .and_then(|v| v.parse::<i64>().ok())
                    .filter(|d| (0..7).contains(d))
                    .unwrap_or(1),
                undo_send_secs: db::get_setting(c, KEY_UNDO_SEND_SECS)?
                    .or_else(|| crate::testenv::get("PEETMAIL_UNDO_SEND_SECS"))
                    .and_then(|v| v.parse::<i64>().ok())
                    .map(|s| s.clamp(0, 60))
                    .unwrap_or(DEFAULT_UNDO_SEND_SECS),
                notify: match db::get_setting(c, KEY_NOTIFY)?.as_deref() {
                    Some("off") => "off".into(),
                    Some("all") => "all".into(),
                    _ => "focused".into(),
                },
                close_to_tray: db::get_setting(c, KEY_CLOSE_TO_TRAY)?.as_deref() != Some("0"),
            })
        })
    }

    pub fn set_preferences(&self, p: &Preferences) -> Result<()> {
        let flag = |b: bool| if b { "1" } else { "0" };
        self.db.with(|c| {
            db::set_setting(c, KEY_HIDE_EMPTY_SPLITS, flag(p.hide_empty_splits))?;
            db::set_setting(c, KEY_SHOW_RIGHT_PANE, flag(p.show_right_pane))?;
            db::set_setting(c, KEY_RIGHT_PANE_WIDTH, &p.right_pane_width.clamp(160, 2000).to_string())?;
            db::set_setting(c, KEY_LAYOUT, if p.layout == "preview" { "preview" } else { "list" })?;
            db::set_setting(c, KEY_LIST_WIDTH, &p.list_width.clamp(200, 2000).to_string())?;
            db::set_setting(c, KEY_RIGHT_PANE_TAB, if p.right_pane_tab == "recent" { "recent" } else { "calendar" })?;
            db::set_setting(c, KEY_WEEK_STARTS_ON, &p.week_starts_on.rem_euclid(7).to_string())?;
            db::set_setting(c, KEY_UNDO_SEND_SECS, &p.undo_send_secs.clamp(0, 60).to_string())?;
            db::set_setting(c, KEY_NOTIFY, match p.notify.as_str() { "off" => "off", "all" => "all", _ => "focused" })?;
            db::set_setting(c, KEY_CLOSE_TO_TRAY, flag(p.close_to_tray))
        })
    }

    pub fn store_tokens_blob(tokens: &TokenSet) -> Result<Vec<u8>> {
        secure::protect(&serde_json::to_vec(tokens)?)
    }

    pub async fn remember_tokens(&self, account_id: i64, tokens: TokenSet) {
        self.tokens.lock().await.insert(account_id, tokens);
    }

    /// Marks the cached access token as expired so the next call refreshes it.
    /// Loads from the DB first so the refresh token survives.
    pub async fn invalidate_access_token(&self, account_id: i64) {
        let mut cache = self.tokens.lock().await;
        if let Some(t) = cache.get_mut(&account_id) {
            t.expires_at = 0;
        } else if let Ok(Some(blob)) = self.db.with(|c| db::get_token_blob(c, account_id))
            && let Ok(mut t) = secure::unprotect(&blob).and_then(|b| Ok(serde_json::from_slice::<TokenSet>(&b)?)) {
            t.expires_at = 0;
            cache.insert(account_id, t);
        }
    }

    pub async fn forget_all_tokens(&self) {
        self.tokens.lock().await.clear();
    }

    pub async fn forget_tokens(&self, account_id: i64) {
        self.tokens.lock().await.remove(&account_id);
    }

    /// The scopes an account's tokens were granted (the provider's defaults for older sign-ins).
    pub async fn granted_scopes(&self, account_id: i64) -> Result<String> {
        self.access_token(account_id).await?;
        let cached = self.tokens.lock().await.get(&account_id).and_then(|t| t.scopes.clone());
        Ok(match cached {
            Some(s) => s,
            None => self.oauth_for_account(account_id)?.scopes().to_string(),
        })
    }

    /// A just-issued OpenID id_token for an account, and its provider: settings sync's proof
    /// of who the account is (the server accepts only fresh ones). Always refreshes.
    pub async fn fresh_id_token(&self, account_id: i64) -> Result<(String, String)> {
        let mut cache = self.tokens.lock().await;
        let current: TokenSet = match cache.get(&account_id) {
            Some(t) => t.clone(),
            None => {
                let blob = self
                    .db
                    .with(|c| db::get_token_blob(c, account_id))?
                    .ok_or_else(|| AppError::Auth("account has no stored credentials".into()))?;
                serde_json::from_slice(&secure::unprotect(&blob)?)?
            }
        };
        let provider = self.db.with(|c| db::account_provider(c, account_id))?;
        let (fresh, id_token) = auth::refresh_with_id_token(&self.http, &self.oauth_for_account(account_id)?, &current).await?;
        let blob = Self::store_tokens_blob(&fresh)?;
        self.db.with(|c| db::set_token_blob(c, account_id, &blob))?;
        cache.insert(account_id, fresh);
        let id_token = id_token.ok_or_else(|| AppError::Other("the account's sign-in didn't say who it is; reconnect it under Settings → Accounts".into()))?;
        Ok((if provider == "google" { "google".into() } else { "microsoft".into() }, id_token))
    }

    /// Returns a valid access token, refreshing and persisting a new token set if needed.
    pub async fn access_token(&self, account_id: i64) -> Result<String> {
        let mut cache = self.tokens.lock().await;
        let current = match cache.get(&account_id) {
            Some(t) => t.clone(),
            None => {
                let blob = self
                    .db
                    .with(|c| db::get_token_blob(c, account_id))?
                    .ok_or_else(|| AppError::Auth("account has no stored credentials".into()))?;
                serde_json::from_slice(&secure::unprotect(&blob)?)?
            }
        };
        if !current.expires_soon() {
            cache.insert(account_id, current.clone());
            return Ok(current.access_token);
        }
        let provider = self.db.with(|c| db::account_provider(c, account_id))?;
        let p = if provider == "google" { "google" } else { "microsoft" };
        let attrs = vec![crate::telemetry::attr(crate::telemetry::Key::Provider, p)];
        let fresh = crate::telemetry::traced("auth.refresh", attrs, auth::refresh(&self.http, &self.oauth_for_account(account_id)?, &current)).await?;
        let blob = Self::store_tokens_blob(&fresh)?;
        self.db.with(|c| db::set_token_blob(c, account_id, &blob))?;
        let token = fresh.access_token.clone();
        cache.insert(account_id, fresh);
        Ok(token)
    }
}
