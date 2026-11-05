//! A settings sync run (docs/SYNC.md): pull what changed on the server and merge it, push
//! what changed here, link this device's other mail accounts. Merging is per setting: the
//! later clock stamp wins. Runs every sync interval, shortly after a local change, and when
//! the window gets focus.

use std::time::Duration;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use url::Url;

use super::client::{self, CallError, CallResult, Client, PushItem, PushResult};
use super::crypto::MasterKey;
use super::{hlc, store, Scope};
use crate::db;
use crate::error::{AppError, Result};
use crate::secure;
use crate::state::AppState;

pub const EVENT_STATUS: &str = "settings-sync-status";
/// `{keys}`: settings another device changed, so the UI reloads them.
pub const EVENT_CHANGED: &str = "settings-changed";

/// Items pushed per request (the server takes up to 500).
const PUSH_BATCH: i64 = 200;
/// Rounds of push-merge-push before leaving the rest for the next run.
const PUSH_ROUNDS: usize = 3;
/// After a failed attempt to link an account, wait this long before the next.
const LINK_RETRY_SECS: i64 = 3600;
/// After a local change, wait for more before syncing (typing in the signature editor).
const SETTLE: Duration = Duration::from_secs(2);

/// A sign-in to a sync server waiting for the user's next step.
pub enum Pending {
    /// The server knows one of this device's accounts: unlock with the passphrase.
    Join { server: Url, session: client::Session, account_id: i64 },
    /// No account here is linked yet: make a new sync account.
    Create { server: Url },
}

#[derive(Default)]
pub struct Memory {
    pub pending: Option<Pending>,
    pub running: bool,
    pub last_synced_at: Option<String>,
    pub last_error: Option<String>,
}

/// What a device in sync keeps secret (sealed with secure.rs, like sign-in tokens).
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Secret {
    device_id: i64,
    device_token: String,
    /// base64.
    master_key: String,
}

/// This device's place in a sync account.
pub struct Joined {
    pub server: Url,
    pub device_id: i64,
    pub device_token: String,
    pub key: MasterKey,
}

impl Joined {
    pub fn client<'a>(&self, http: &'a reqwest::Client) -> Client<'a> {
        Client::new(http, self.server.clone(), Some(self.device_token.clone()))
    }
}

pub fn save_joined(c: &Connection, j: &Joined) -> Result<()> {
    let secret = Secret { device_id: j.device_id, device_token: j.device_token.clone(), master_key: STANDARD.encode(j.key.bytes()) };
    let sealed = secure::protect(&serde_json::to_vec(&secret)?)?;
    db::set_setting(c, store::KEY_SERVER, j.server.as_str())?;
    db::set_setting(c, store::KEY_SECRET, &STANDARD.encode(sealed))
}

/// This device's sync account, if sync is on.
pub fn joined(c: &Connection) -> Result<Option<Joined>> {
    let (Some(server), Some(sealed)) = (db::get_setting(c, store::KEY_SERVER)?, db::get_setting(c, store::KEY_SECRET)?) else {
        return Ok(None);
    };
    let raw = STANDARD.decode(sealed).map_err(|_| AppError::Other("settings sync's saved key is damaged".into()))?;
    let secret: Secret = serde_json::from_slice(&secure::unprotect(&raw)?)?;
    Ok(Some(Joined {
        server: Url::parse(&server).map_err(|_| AppError::Other("settings sync's server address is damaged".into()))?,
        device_id: secret.device_id,
        device_token: secret.device_token,
        key: MasterKey::from_bytes(&STANDARD.decode(secret.master_key).map_err(|_| AppError::Other("settings sync's saved key is damaged".into()))?)?,
    }))
}

/// An item's plaintext: the setting it is, its value (None: removed) and when it changed.
#[derive(Debug, Serialize, Deserialize)]
struct Plain {
    v: u32,
    key: String,
    value: Option<String>,
    hlc: String,
}

/// Merges one item from the server. Returns the settings key if this device's value changed.
/// An item this device can't read or mustn't take is skipped, not fatal: one bad item
/// shouldn't stop every other setting syncing.
fn merge(c: &Connection, key: &MasterKey, item_id: &str, seq: i64, blob: &str) -> Result<Option<String>> {
    let plain: Plain = match key.open_item(item_id, blob).ok().and_then(|b| serde_json::from_slice(&b).ok()) {
        Some(p) => p,
        None => {
            log::warn!("settings sync: skipping an item this device can't read");
            return Ok(None);
        }
    };
    if plain.v != 1 || key.item_id(&plain.key) != item_id {
        log::warn!("settings sync: skipping an item from a newer PeetMail or under the wrong name");
        return Ok(None);
    }
    // Never let the server (or a buggy device) write this device's own settings.
    if super::scope(&plain.key) != Some(Scope::Synced) || !hlc::plausible(&plain.hlc, chrono::Utc::now().timestamp_millis()) {
        log::warn!("settings sync: skipping an item that isn't a synced setting");
        return Ok(None);
    }
    let local = store::local(c, &plain.key)?;
    let local_hlc = local.as_ref().map(|l| l.hlc.as_str()).unwrap_or(hlc::ZERO);
    let has_value = db::get_setting(c, &plain.key)?.is_some() || local.is_some();
    if plain.hlc.as_str() > local_hlc || !has_value {
        store::apply_remote(c, &plain.key, plain.value.as_deref(), &plain.hlc, seq)?;
        return Ok(Some(plain.key));
    }
    // Ours is the same change or a later one: in step with this version, and ours still goes
    // up if it's later.
    store::in_step(c, &plain.key, seq, plain.hlc.as_str() < local_hlc)?;
    Ok(None)
}

/// One run against the server. Returns the settings another device changed.
async fn exchange(state: &AppState, j: &Joined) -> CallResult<Vec<String>> {
    let api = j.client(&state.http);
    let db_err = CallError::Other;
    let mut changed = Vec::new();

    loop {
        let since: i64 = state.db.with(|c| Ok(db::get_setting(c, store::KEY_CURSOR)?.and_then(|v| v.parse().ok()).unwrap_or(0))).map_err(db_err)?;
        let page = api.pull(since).await?;
        state
            .db
            .with(|c| {
                let tx = c.transaction()?;
                for it in &page.items {
                    changed.extend(merge(&tx, &j.key, &it.id, it.seq, &it.blob)?);
                }
                db::set_setting(&tx, store::KEY_CURSOR, &page.cursor.to_string())?;
                tx.commit()?;
                Ok(())
            })
            .map_err(db_err)?;
        if !page.more {
            break;
        }
    }

    for _ in 0..PUSH_ROUNDS {
        let dirty = state.db.with(|c| store::dirty(c, PUSH_BATCH)).map_err(db_err)?;
        if dirty.is_empty() {
            break;
        }
        let items: Vec<PushItem> = dirty
            .iter()
            .map(|d| {
                let id = j.key.item_id(&d.key);
                let plain = Plain { v: 1, key: d.key.clone(), value: d.value.clone(), hlc: d.hlc.clone() };
                let blob = j.key.seal_item(&id, &serde_json::to_vec(&plain).expect("plain JSON"));
                PushItem { id, blob, deleted: d.value.is_none(), base_seq: d.server_seq }
            })
            .collect();
        let res = api.push(items).await?;
        state
            .db
            .with(|c| {
                let tx = c.transaction()?;
                for r in &res.results {
                    match r {
                        PushResult::Ok { id, seq } => {
                            if let Some(d) = dirty.iter().find(|d| j.key.item_id(&d.key) == *id) {
                                store::pushed(&tx, &d.key, &d.hlc, *seq)?;
                            }
                        }
                        // Another device got there first: merge its value, and push again on
                        // its version if ours is later.
                        PushResult::Conflict { id, seq, blob, .. } => changed.extend(merge(&tx, &j.key, id, *seq, blob)?),
                    }
                }
                tx.commit()?;
                Ok(())
            })
            .map_err(db_err)?;
    }
    changed.sort();
    changed.dedup();
    Ok(changed)
}

/// Links this device's mail accounts that aren't linked yet, so any of them can sign a new
/// device in. A failure is remembered and retried an hour later; it never fails the run.
async fn link_accounts(state: &AppState, j: &Joined) -> Result<()> {
    let accounts = state.db.with(|c| db::list_accounts(c))?;
    let now = chrono::Utc::now().timestamp();
    for a in accounts.iter().filter(|a| a.signed_in) {
        let key = store::link_key(a.id);
        let current = state.db.with(|c| db::get_setting(c, &key))?;
        let skip = match current.as_deref() {
            Some("linked" | "elsewhere" | "unlinked") => true,
            Some(s) => s.strip_prefix("failed:").and_then(|t| t.parse::<i64>().ok()).is_some_and(|t| now - t < LINK_RETRY_SECS),
            None => false,
        };
        if skip {
            continue;
        }
        let outcome = match state.fresh_id_token(a.id).await {
            Ok((provider, id_token)) => match j.client(&state.http).link(&client::LinkRequest { provider, id_token }).await {
                Ok(_) => "linked".to_string(),
                Err(e) if e.code() == Some("linked_elsewhere") => "elsewhere".to_string(),
                Err(e) => {
                    log::warn!("settings sync: couldn't link an account: {e}");
                    format!("failed:{now}")
                }
            },
            Err(e) => {
                log::warn!("settings sync: no sign-in token to link an account: {}", crate::scrub::scrub(&e.to_string()));
                format!("failed:{now}")
            }
        };
        state.db.with(|c| db::set_setting(c, &key, &outcome))?;
    }
    Ok(())
}

/// Turns sync off on this device only: forgets the server, the key and what was in step.
pub fn forget(state: &AppState) -> Result<()> {
    state.db.with(|c| store::forget_server(c))?;
    let mut m = state.settings_sync.lock().unwrap_or_else(|p| p.into_inner());
    m.pending = None;
    m.last_synced_at = None;
    Ok(())
}

/// Syncs once, if sync is on. Errors end up in the status (and the log), not here, except
/// that a device the server signed out turns sync off.
pub async fn run(app: &AppHandle) {
    let state = app.state::<AppState>();
    let _guard = state.settings_sync_lock.lock().await;
    let j = match state.db.with(|c| joined(c)) {
        Ok(Some(j)) => j,
        Ok(None) => return,
        Err(e) => {
            set_result(&state, Err(e.to_string()));
            emit_status(app);
            return;
        }
    };
    state.settings_sync.lock().unwrap_or_else(|p| p.into_inner()).running = true;
    emit_status(app);
    let result = exchange(&state, &j).await;
    let outcome = match result {
        Ok(changed) => {
            if let Err(e) = link_accounts(&state, &j).await {
                log::warn!("settings sync: linking accounts: {e}");
            }
            if !changed.is_empty() {
                log::info!("settings sync: {} setting(s) changed on another device", changed.len());
                let _ = app.emit(EVENT_CHANGED, serde_json::json!({ "keys": changed }));
            }
            Ok(())
        }
        Err(e) if matches!(e.code(), Some("device_revoked" | "unauthorized")) => {
            log::warn!("settings sync: this device was signed out by the server ({e})");
            let _ = forget(&state);
            Err("This PC was signed out of settings sync (from another device, or the sync account was deleted). Turn sync on again to rejoin.".to_string())
        }
        Err(e) => {
            log::warn!("settings sync failed: {}", crate::scrub::scrub(&e.to_string()));
            Err(AppError::from(e).to_string())
        }
    };
    set_result(&state, outcome);
    emit_status(app);
}

fn set_result(state: &AppState, outcome: std::result::Result<(), String>) {
    let mut m = state.settings_sync.lock().unwrap_or_else(|p| p.into_inner());
    m.running = false;
    match outcome {
        Ok(()) => {
            m.last_synced_at = Some(chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
            m.last_error = None;
        }
        Err(e) => m.last_error = Some(e),
    }
}

pub fn emit_status(app: &AppHandle) {
    let state = app.state::<AppState>();
    match super::commands::status_of(&state) {
        Ok(s) => {
            let _ = app.emit(EVENT_STATUS, s);
        }
        Err(e) => log::warn!("settings sync status: {e}"),
    }
}

/// Runs sync in the background: every `interval`, shortly after a local change, and when
/// woken (window focus, sync turned on).
pub fn start(app: &AppHandle, interval: Duration) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        // Let startup's own work go first.
        tokio::time::sleep(Duration::from_secs(3)).await;
        loop {
            run(&app).await;
            tokio::select! {
                _ = tokio::time::sleep(interval) => {}
                _ = store::WAKE.notified() => tokio::time::sleep(SETTLE).await,
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seal(key: &MasterKey, settings_key: &str, value: Option<&str>, stamp: &str) -> (String, String) {
        let id = key.item_id(settings_key);
        let plain = Plain { v: 1, key: settings_key.into(), value: value.map(String::from), hlc: stamp.into() };
        let blob = key.seal_item(&id, &serde_json::to_vec(&plain).unwrap());
        (id, blob)
    }

    fn with_db(f: impl FnOnce(&mut Connection) -> Result<()>) {
        db::Db::open_in_memory().unwrap().with(f).unwrap();
    }

    #[test]
    fn a_later_change_from_another_device_wins_and_an_earlier_one_doesnt() {
        let key = MasterKey::generate();
        with_db(|c| {
            db::set_setting(c, "undo_send_secs", "5")?;
            let mine = store::local(c, "undo_send_secs")?.unwrap().hlc;
            // Older than ours: ignored, and ours still goes up on the server's version.
            let (id, blob) = seal(&key, "undo_send_secs", Some("20"), "0000000001000:0000:b");
            assert_eq!(merge(c, &key, &id, 4, &blob)?, None);
            assert_eq!(db::get_setting(c, "undo_send_secs")?.as_deref(), Some("5"));
            let l = store::local(c, "undo_send_secs")?.unwrap();
            assert_eq!((l.server_seq, l.dirty, l.hlc.clone()), (Some(4), true, mine.clone()));
            // Newer than ours: taken.
            let later = hlc::next(Some(&mine), chrono::Utc::now().timestamp_millis() + 60_000, "b");
            let (id, blob) = seal(&key, "undo_send_secs", Some("30"), &later);
            assert_eq!(merge(c, &key, &id, 5, &blob)?, Some("undo_send_secs".into()));
            assert_eq!(db::get_setting(c, "undo_send_secs")?.as_deref(), Some("30"));
            assert!(!store::local(c, "undo_send_secs")?.unwrap().dirty);
            Ok(())
        });
    }

    #[test]
    fn a_new_setting_and_a_removal_arrive() {
        let key = MasterKey::generate();
        with_db(|c| {
            let (id, blob) = seal(&key, "snippet:hi", Some(r#"{"name":"hi","body":"Hi","order":1}"#), "0000000002000:0000:b");
            assert_eq!(merge(c, &key, &id, 1, &blob)?, Some("snippet:hi".into()));
            let (id, blob) = seal(&key, "snippet:hi", None, "0000000003000:0000:b");
            assert_eq!(merge(c, &key, &id, 2, &blob)?, Some("snippet:hi".into()));
            assert_eq!(db::get_setting(c, "snippet:hi")?, None);
            Ok(())
        });
    }

    #[test]
    fn an_untouched_default_takes_the_other_devices_value() {
        let key = MasterKey::generate();
        with_db(|c| {
            store::prepare_first_sync(c)?; // split:other at the zero stamp
            let (id, blob) = seal(&key, "split:other", Some(r#"{"name":"Newsletters","query":"is:other","order":1}"#), "0000000002000:0000:b");
            assert_eq!(merge(c, &key, &id, 1, &blob)?, Some("split:other".into()));
            // The same default from another device (zero stamp too) is simply in step.
            with_db(|c2| {
                store::prepare_first_sync(c2)?;
                let other = db::get_setting(c2, "split:other")?.unwrap();
                let (id, blob) = seal(&key, "split:other", Some(&other), hlc::ZERO);
                assert_eq!(merge(c2, &key, &id, 1, &blob)?, None);
                assert!(!store::local(c2, "split:other")?.unwrap().dirty, "nothing to push back");
                Ok(())
            });
            Ok(())
        });
    }

    #[test]
    fn items_this_device_mustnt_take_are_skipped() {
        let key = MasterKey::generate();
        with_db(|c| {
            // A local setting (the server mustn't change this device's cache size).
            let (id, blob) = seal(&key, "cache_days", Some("1"), "0000000002000:0000:b");
            assert_eq!(merge(c, &key, &id, 1, &blob)?, None);
            assert_eq!(db::get_setting(c, "cache_days")?, None);
            // Under another item's id (the server can't swap them: AES-GCM's associated data).
            let (_, blob) = seal(&key, "notify", Some("off"), "0000000002000:0000:b");
            assert_eq!(merge(c, &key, &key.item_id("week_starts_on"), 1, &blob)?, None);
            // Another sync account's key, garbage, and a stamp a year ahead.
            let (id, blob) = seal(&MasterKey::generate(), "notify", Some("off"), "0000000002000:0000:b");
            assert_eq!(merge(c, &key, &id, 1, &blob)?, None);
            assert_eq!(merge(c, &key, &id, 1, "not-a-blob")?, None);
            let far = format!("{:013}:0000:b", chrono::Utc::now().timestamp_millis() + 365 * 86_400_000);
            let (id, blob) = seal(&key, "notify", Some("off"), &far);
            assert_eq!(merge(c, &key, &id, 1, &blob)?, None);
            assert_eq!(db::get_setting(c, "notify")?, None);
            Ok(())
        });
    }

    #[test]
    fn the_secret_is_kept_sealed_and_read_back() {
        crate::secure::init_for_tests();
        with_db(|c| {
            let j = Joined { server: Url::parse("https://sync.example.com/").unwrap(), device_id: 3, device_token: "pmd_secret".into(), key: MasterKey::generate() };
            save_joined(c, &j)?;
            let stored = db::get_setting(c, store::KEY_SECRET)?.unwrap();
            assert!(!stored.contains("pmd_secret"));
            let back = joined(c)?.unwrap();
            assert_eq!((back.server.as_str(), back.device_id, back.device_token.as_str()), ("https://sync.example.com/", 3, "pmd_secret"));
            assert_eq!(back.key, j.key);
            store::forget_server(c)?;
            assert!(joined(c)?.is_none());
            Ok(())
        });
    }
}
