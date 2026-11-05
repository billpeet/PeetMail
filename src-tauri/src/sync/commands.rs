//! Settings → Sync's commands (docs/SYNC.md, docs/user/sync.md). Turning sync on is two
//! steps: `settings_sync_connect` finds out whether one of this device's mail accounts
//! already has a sync account on the server, then `settings_sync_join` (with the passphrase
//! or recovery key) or `settings_sync_create` (with a new passphrase) finishes.

use serde::Serialize;
use tauri::{AppHandle, State};

use super::client::{self, Client, parse_server_url};
use super::crypto::{self, KdfParams, MasterKey};
use super::engine::{self, Joined, Pending};
use super::store;
use crate::db;
use crate::error::{AppError, Result};
use crate::state::AppState;

/// The shortest passphrase accepted. It guards everything synced, and Argon2 only slows
/// guessing down.
pub const MIN_PASSPHRASE: usize = 10;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountLink {
    pub account_id: i64,
    pub email: String,
    /// "linked", "elsewhere" (another sync account has it), "unlinked" (by you), "failed"
    /// (will try again) or "pending" (not tried yet).
    pub state: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// "off", "idle", "syncing" or "error".
    pub state: String,
    pub server: Option<String>,
    pub last_synced_at: Option<String>,
    pub error: Option<String>,
    /// A sign-in waiting for its next step: "join" or "create".
    pub pending: Option<String>,
    pub accounts: Vec<AccountLink>,
}

pub fn status_of(state: &AppState) -> Result<Status> {
    let (server, links, accounts) = state.db.with(|c| {
        let server = if db::get_setting(c, store::KEY_SECRET)?.is_some() { db::get_setting(c, store::KEY_SERVER)? } else { None };
        let accounts = db::list_accounts(c)?;
        let links = accounts.iter().map(|a| db::get_setting(c, &store::link_key(a.id))).collect::<Result<Vec<_>>>()?;
        Ok((server, links, accounts))
    })?;
    let m = state.settings_sync.lock().unwrap_or_else(|p| p.into_inner());
    let st = match (&server, m.running, &m.last_error) {
        (None, _, _) => "off",
        (Some(_), true, _) => "syncing",
        (Some(_), false, Some(_)) => "error",
        _ => "idle",
    };
    Ok(Status {
        state: st.into(),
        // As typed: https://sync.example.com, not the URL form's https://sync.example.com/.
        server: server.map(|s| s.trim_end_matches('/').to_string()),
        last_synced_at: m.last_synced_at.clone(),
        // An error from the last attempt stays visible even after sync turned itself off.
        error: m.last_error.clone(),
        pending: m.pending.as_ref().map(|p| match p {
            Pending::Join { .. } => "join".into(),
            Pending::Create { .. } => "create".into(),
        }),
        accounts: accounts
            .into_iter()
            .zip(links)
            .filter(|(a, _)| a.signed_in)
            .map(|(a, link)| AccountLink {
                account_id: a.id,
                email: a.email,
                state: match link.as_deref() {
                    Some(s @ ("linked" | "elsewhere" | "unlinked")) => s.into(),
                    Some(_) => "failed".into(),
                    None => "pending".into(),
                },
            })
            .collect(),
    })
}

/// This PC's name, as the device list shows it.
fn device_name() -> String {
    let host = std::env::var("COMPUTERNAME").ok().or_else(|| std::fs::read_to_string("/etc/hostname").ok()).map(|h| h.trim().to_string()).filter(|h| !h.is_empty());
    let os = if cfg!(windows) { "Windows" } else { "Linux" };
    match host {
        Some(h) => h.chars().filter(|c| !c.is_control()).take(80).collect(),
        None => format!("PeetMail on {os}"),
    }
}

fn joined(state: &AppState) -> Result<Joined> {
    state.db.with(|c| engine::joined(c))?.ok_or_else(|| AppError::Other("Settings sync is off.".into()))
}

fn set_pending(state: &AppState, p: Option<Pending>) {
    state.settings_sync.lock().unwrap_or_else(|p| p.into_inner()).pending = p;
}

#[tauri::command]
pub fn settings_sync_status(state: State<'_, AppState>) -> Result<Status> {
    status_of(&state)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectAccount {
    pub id: i64,
    pub email: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Connected {
    /// "join": enter the passphrase. "create": choose an account and a passphrase.
    pub next: String,
    /// For "join", the account that was recognised; for "create", the ones to choose from.
    pub accounts: Vec<ConnectAccount>,
}

/// Step one: checks the server, then asks it about each of this device's accounts in turn.
/// Never creates a sync account.
#[tauri::command]
pub async fn settings_sync_connect(app: AppHandle, state: State<'_, AppState>, server: String) -> Result<Connected> {
    let url = parse_server_url(&server)?;
    let anon = Client::new(&state.http, url.clone(), None);
    let health = anon.health().await.map_err(|e| match e {
        client::CallError::Server(_) => AppError::Other("That address answers, but it isn't a PeetMail sync server.".into()),
        client::CallError::Other(e) => e,
    })?;
    if !health.ok || health.protocol != client::PROTOCOL_VERSION {
        return Err(AppError::Other(format!(
            "That sync server speaks version {} of the protocol and this PeetMail speaks {}. Update {}.",
            health.protocol,
            client::PROTOCOL_VERSION,
            if health.protocol > client::PROTOCOL_VERSION { "PeetMail" } else { "the server" }
        )));
    }
    let accounts: Vec<_> = state.db.with(|c| db::list_accounts(c))?.into_iter().filter(|a| a.signed_in).collect();
    if accounts.is_empty() {
        return Err(AppError::Other("Add a mail account first: it's how you sign in to sync.".into()));
    }
    let mut last_problem = None;
    let mut checked = 0;
    for a in &accounts {
        let (provider, id_token) = match state.fresh_id_token(a.id).await {
            Ok(t) => t,
            Err(e) => {
                last_problem = Some(e);
                continue;
            }
        };
        match anon.session(&client::SessionRequest { provider, id_token, device_name: device_name() }).await {
            Ok(session) => {
                set_pending(&state, Some(Pending::Join { server: url, session, account_id: a.id }));
                engine::emit_status(&app);
                return Ok(Connected { next: "join".into(), accounts: vec![ConnectAccount { id: a.id, email: a.email.clone() }] });
            }
            Err(e) if e.code() == Some("no_user") => checked += 1,
            Err(e) if matches!(e.code(), Some("provider_disabled" | "invalid_token")) => last_problem = Some(e.into()),
            Err(e) => return Err(e.into()),
        }
    }
    // Nobody here is known. Offer a new sync account only if some account could be checked:
    // otherwise the problem is with the accounts (or the server's setup), so say what it was.
    if checked == 0 {
        return Err(last_problem.unwrap_or_else(|| AppError::Other("None of your accounts could sign in to the sync server.".into())));
    }
    set_pending(&state, Some(Pending::Create { server: url }));
    engine::emit_status(&app);
    Ok(Connected { next: "create".into(), accounts: accounts.into_iter().map(|a| ConnectAccount { id: a.id, email: a.email }).collect() })
}

/// Derives a passphrase's key off the async runtime (Argon2 takes a moment and 64 MiB).
async fn kek(passphrase: String, params: KdfParams) -> Result<[u8; crypto::KEY_LEN]> {
    tokio::task::spawn_blocking(move || crypto::derive_kek(&passphrase, &params))
        .await
        .map_err(|e| AppError::Other(format!("key derivation stopped: {e}")))?
}

fn begin(app: &AppHandle, state: &AppState, j: &Joined, account_id: i64) -> Result<()> {
    state.db.with(|c| {
        let tx = c.transaction()?;
        engine::save_joined(&tx, j)?;
        store::prepare_first_sync(&tx)?;
        db::set_setting(&tx, &store::link_key(account_id), "linked")?;
        tx.commit()?;
        Ok(())
    })?;
    {
        let mut m = state.settings_sync.lock().unwrap_or_else(|p| p.into_inner());
        m.pending = None;
        m.last_error = None;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move { engine::run(&app).await });
    Ok(())
}

/// Step two, for an existing sync account: unlocks it with the passphrase or the recovery key.
#[tauri::command]
pub async fn settings_sync_join(app: AppHandle, state: State<'_, AppState>, secret: String) -> Result<Status> {
    let (server, session, account_id) = match &state.settings_sync.lock().unwrap_or_else(|p| p.into_inner()).pending {
        Some(Pending::Join { server, session, account_id }) => (server.clone(), session.clone(), *account_id),
        _ => return Err(AppError::Other("Connect to a sync server first.".into())),
    };
    let key = match MasterKey::from_recovery_key(&secret) {
        Some(k) => k,
        None => {
            let kek = kek(secret, session.kdf_params.clone()).await?;
            MasterKey::unwrap(&kek, &session.wrapped_key).ok_or_else(|| AppError::Other("That passphrase isn't right.".into()))?
        }
    };
    let j = Joined { server, device_id: session.device_id, device_token: session.device_token, key };
    begin(&app, &state, &j, account_id)?;
    status_of(&state)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Created {
    /// Shown once: the way back in if the passphrase is forgotten.
    pub recovery_key: String,
}

/// Step two, for a new sync account: made with `account_id` and a new passphrase.
#[tauri::command]
pub async fn settings_sync_create(app: AppHandle, state: State<'_, AppState>, account_id: i64, passphrase: String) -> Result<Created> {
    let server = match &state.settings_sync.lock().unwrap_or_else(|p| p.into_inner()).pending {
        Some(Pending::Create { server }) => server.clone(),
        _ => return Err(AppError::Other("Connect to a sync server first.".into())),
    };
    if passphrase.chars().count() < MIN_PASSPHRASE {
        return Err(AppError::Other(format!("Use a passphrase of at least {MIN_PASSPHRASE} characters.")));
    }
    let key = MasterKey::generate();
    let params = KdfParams::new();
    let wrapped_key = key.wrap(&kek(passphrase, params.clone()).await?);
    let (provider, id_token) = state.fresh_id_token(account_id).await?;
    let session = Client::new(&state.http, server.clone(), None)
        .register(&client::RegisterRequest { provider, id_token, device_name: device_name(), wrapped_key, kdf_params: params })
        .await?;
    let recovery_key = key.recovery_key();
    let j = Joined { server, device_id: session.device_id, device_token: session.device_token, key };
    begin(&app, &state, &j, account_id)?;
    Ok(Created { recovery_key })
}

/// Backs out of a sign-in waiting for its passphrase (the server had already signed this
/// device in, so that's undone).
#[tauri::command]
pub async fn settings_sync_cancel(app: AppHandle, state: State<'_, AppState>) -> Result<()> {
    let pending = state.settings_sync.lock().unwrap_or_else(|p| p.into_inner()).pending.take();
    if let Some(Pending::Join { server, session, .. }) = pending
        && let Err(e) = Client::new(&state.http, server, Some(session.device_token)).revoke(session.device_id).await
    {
        log::warn!("settings sync: couldn't undo a half-finished sign-in: {e}");
    }
    engine::emit_status(&app);
    Ok(())
}

/// Accounts another PC syncs that this one doesn't have (empty while sync is off).
#[tauri::command]
pub fn synced_accounts(state: State<'_, AppState>) -> Result<Vec<super::accounts::Elsewhere>> {
    state.db.with(|c| if engine::joined(c)?.is_some() { super::accounts::elsewhere(c) } else { Ok(vec![]) })
}

/// Stops offering an account from another PC here.
#[tauri::command]
pub fn dismiss_synced_account(state: State<'_, AppState>, provider: String, email: String) -> Result<()> {
    state.db.with(|c| super::accounts::forget_here(c, &provider, &email))
}

#[tauri::command]
pub async fn settings_sync_now(app: AppHandle, state: State<'_, AppState>) -> Result<Status> {
    engine::run(&app).await;
    status_of(&state)
}

#[tauri::command]
pub async fn settings_sync_devices(state: State<'_, AppState>) -> Result<Vec<client::Device>> {
    Ok(joined(&state)?.client(&state.http).devices().await?)
}

/// Signs a device out of sync. This one included: then sync is off here.
#[tauri::command]
pub async fn settings_sync_revoke(app: AppHandle, state: State<'_, AppState>, device_id: i64) -> Result<()> {
    let j = joined(&state)?;
    j.client(&state.http).revoke(device_id).await?;
    if device_id == j.device_id {
        engine::forget(&state)?;
    }
    engine::emit_status(&app);
    Ok(())
}

#[tauri::command]
pub async fn settings_sync_identities(state: State<'_, AppState>) -> Result<Vec<client::LinkedAccount>> {
    Ok(joined(&state)?.client(&state.http).identities().await?)
}

/// Unlinks a mail account from the sync account. It isn't linked again automatically.
#[tauri::command]
pub async fn settings_sync_unlink(app: AppHandle, state: State<'_, AppState>, identity_id: i64) -> Result<()> {
    let api = joined(&state)?;
    let api = api.client(&state.http);
    let email = api.identities().await?.into_iter().find(|i| i.id == identity_id).and_then(|i| i.email);
    api.unlink(identity_id).await?;
    if let Some(email) = email {
        state.db.with(|c| {
            for a in db::list_accounts(c)?.into_iter().filter(|a| a.email.eq_ignore_ascii_case(&email)) {
                db::set_setting(c, &store::link_key(a.id), "unlinked")?;
            }
            Ok(())
        })?;
    }
    engine::emit_status(&app);
    Ok(())
}

#[tauri::command]
pub async fn settings_sync_change_passphrase(state: State<'_, AppState>, passphrase: String) -> Result<()> {
    if passphrase.chars().count() < MIN_PASSPHRASE {
        return Err(AppError::Other(format!("Use a passphrase of at least {MIN_PASSPHRASE} characters.")));
    }
    let j = joined(&state)?;
    let params = KdfParams::new();
    let wrapped_key = j.key.wrap(&kek(passphrase, params.clone()).await?);
    j.client(&state.http).set_key(&client::KeyRequest { wrapped_key, kdf_params: params }).await?;
    Ok(())
}

/// The recovery key again, for a device already in sync.
#[tauri::command]
pub fn settings_sync_recovery_key(state: State<'_, AppState>) -> Result<String> {
    Ok(joined(&state)?.key.recovery_key())
}

/// Turns sync off on this device. Its settings stay as they are; the server forgets it.
#[tauri::command]
pub async fn settings_sync_disable(app: AppHandle, state: State<'_, AppState>) -> Result<()> {
    let _guard = state.settings_sync_lock.lock().await;
    if let Ok(j) = joined(&state)
        && let Err(e) = j.client(&state.http).revoke(j.device_id).await
    {
        log::warn!("settings sync: the server didn't hear that this device left: {e}");
    }
    engine::forget(&state)?;
    state.settings_sync.lock().unwrap_or_else(|p| p.into_inner()).last_error = None;
    engine::emit_status(&app);
    Ok(())
}

/// Deletes the sync account from the server (every device leaves it). Settings on each
/// device stay as they are.
#[tauri::command]
pub async fn settings_sync_delete_account(app: AppHandle, state: State<'_, AppState>) -> Result<()> {
    let _guard = state.settings_sync_lock.lock().await;
    let j = joined(&state)?;
    j.client(&state.http).delete_user().await?;
    engine::forget(&state)?;
    state.settings_sync.lock().unwrap_or_else(|p| p.into_inner()).last_error = None;
    engine::emit_status(&app);
    Ok(())
}
