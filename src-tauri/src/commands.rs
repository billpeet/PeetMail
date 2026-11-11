use serde::Serialize;
use tauri::{AppHandle, State};

use serde::Deserialize;
use tauri::Emitter;
use tauri_plugin_opener::OpenerExt;

use crate::actions::{self, Action, ActionOutcome};
use crate::{draft_files, drafts};
use crate::db::{self, Account, Contact, Cursor, Folder, Mailbox, MessageRow, RecentOpen, Scope, ThreadRow};
use crate::error::{AppError, Result};
use crate::outbox::{self, Draft, DraftKind, Op, OutboxStatus};
use crate::items;
use crate::state::{AppState, CacheLimits, Preferences, KEY_BODY_CACHE_MB, KEY_CACHE_DAYS, KEY_CLIENT_ID};
use crate::{auth, mail, query, server_search};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Bootstrap {
    /// The user's own Microsoft app (Settings → Advanced), if set.
    client_id: Option<String>,
    /// The user's own Google OAuth client ID, if set (the secret stays in Rust).
    google_client_id: Option<String>,
    /// Which providers have an app built into this build, so nobody has to set one up.
    builtin: BuiltinApps,
    accounts: Vec<Account>,
    cache_limits: CacheLimits,
    preferences: Preferences,
}

#[derive(Serialize)]
pub struct BuiltinApps {
    microsoft: bool,
    google: bool,
}

#[tauri::command]
pub fn bootstrap(state: State<'_, AppState>) -> Result<Bootstrap> {
    Ok(Bootstrap {
        client_id: state.custom_client("microsoft")?.map(|c| c.0),
        google_client_id: state.custom_client("google")?.map(|c| c.0),
        builtin: BuiltinApps {
            microsoft: crate::state::builtin_client("microsoft").is_some(),
            google: crate::state::builtin_client("google").is_some(),
        },
        accounts: state.db.with(|c| db::list_accounts(c))?,
        cache_limits: state.cache_limits()?,
        preferences: state.preferences()?,
    })
}

#[tauri::command]
pub fn set_preferences(state: State<'_, AppState>, preferences: Preferences) -> Result<()> {
    state.set_preferences(&preferences)
}

/// Saves the user's own Entra app (Settings → Advanced). Changing it signs out the accounts
/// that signed in with it (refresh tokens only work with the app that issued them);
/// accounts on the built-in app are unaffected. Cached mail is kept.
#[tauri::command]
pub async fn set_client_id(state: State<'_, AppState>, client_id: String) -> Result<()> {
    let id = client_id.trim().to_lowercase();
    if !is_guid(&id) {
        return Err(AppError::Other(
            "That doesn't look like an Application (client) ID. It should look like 00000000-0000-0000-0000-000000000000.".into(),
        ));
    }
    if state.custom_client("microsoft")?.map(|c| c.0).as_deref() == Some(id.as_str()) {
        return Ok(());
    }
    cancel_pending_sign_in(&state);
    state.db.with(|c| {
        db::set_setting(c, KEY_CLIENT_ID, &id)?;
        db::clear_app_tokens(c, "microsoft", crate::state::APP_CUSTOM)
    })?;
    state.forget_all_tokens().await;
    Ok(())
}

/// Saves the user's own Google OAuth client (a "Desktop app" client in Google Cloud).
/// Changing the ID signs out the Google accounts that used it; their cached mail is kept.
#[tauri::command]
pub async fn set_google_client(state: State<'_, AppState>, client_id: String, client_secret: Option<String>) -> Result<()> {
    let id = client_id.trim().to_string();
    if !id.ends_with(".apps.googleusercontent.com") {
        return Err(AppError::Other("That doesn't look like a Google client ID. It ends in .apps.googleusercontent.com.".into()));
    }
    let secret = client_secret.map(|s| s.trim().to_string()).unwrap_or_default();
    let changed = state.custom_client("google")?.map(|c| c.0).as_deref() != Some(id.as_str());
    cancel_pending_sign_in(&state);
    state.db.with(|c| {
        db::set_setting(c, crate::state::KEY_GOOGLE_CLIENT_ID, &id)?;
        db::set_setting(c, crate::state::KEY_GOOGLE_CLIENT_SECRET, &secret)?;
        if changed {
            db::clear_app_tokens(c, "google", crate::state::APP_CUSTOM)?;
        }
        Ok(())
    })?;
    if changed {
        state.forget_all_tokens().await;
    }
    Ok(())
}

fn is_guid(s: &str) -> bool {
    let groups: Vec<&str> = s.split('-').collect();
    groups.len() == 5
        && groups.iter().zip([8, 4, 4, 4, 12]).all(|(g, n)| g.len() == n && g.chars().all(|c| c.is_ascii_hexdigit()))
}

fn cancel_pending_sign_in(state: &AppState) {
    if let Some(tx) = state.sign_in_cancel.lock().expect("cancel mutex").take() {
        let _ = tx.send(());
    }
}

#[tauri::command]
pub fn cancel_sign_in(state: State<'_, AppState>) {
    cancel_pending_sign_in(&state);
}

/// Signs in a new (or returning) account. `oauth_app` picks the app: "builtin", "custom",
/// or (unset) the build's built-in one when it has one.
#[tauri::command]
pub async fn sign_in(app: AppHandle, state: State<'_, AppState>, provider: Option<String>, oauth_app: Option<String>, login_hint: Option<String>) -> Result<Account> {
    let provider = if provider.as_deref() == Some("google") { "google" } else { "microsoft" };
    let oauth_app = match oauth_app.as_deref() {
        Some("custom") => crate::state::APP_CUSTOM,
        Some("builtin") => crate::state::APP_BUILTIN,
        _ => state.default_app(provider),
    };
    let mut cfg = state.oauth_for_app(provider, oauth_app)?;
    // An account another PC uses (settings sync): the sign-in page suggests it.
    if let Some(hint) = login_hint.as_deref().filter(|h| !h.trim().is_empty()) {
        cfg = cfg.with_login_hint(hint);
    }
    // Starting a new sign-in abandons any earlier one still waiting on the browser.
    cancel_pending_sign_in(&state);
    let (tx, rx) = tokio::sync::oneshot::channel();
    *state.sign_in_cancel.lock().expect("cancel mutex") = Some(tx);
    let auth_attrs = vec![crate::telemetry::attr(crate::telemetry::Key::Provider, provider)];
    let (tokens, id_token) = crate::telemetry::traced("auth.sign_in", auth_attrs, auth::sign_in(&app, &state.http, &cfg, rx)).await?;
    let (email, display_name) = if provider == "google" {
        (crate::gmail::fetch_profile(&state.http, &tokens.access_token).await?, id_token.as_deref().and_then(auth::id_token_name))
    } else {
        mail::fetch_profile(&state.http, &tokens.access_token).await?
    };
    let blob = AppState::store_tokens_blob(&tokens)?;
    let account = state
        .db
        .with(|c| {
            let account = db::upsert_account(c, provider, &email, display_name.as_deref(), &blob)?;
            db::set_account_app(c, account.id, oauth_app)?;
            crate::sync::accounts::remember(c, provider, &email, oauth_app, display_name.as_deref())?;
            Ok(Account { oauth_app: oauth_app.to_string(), ..account })
        })?;
    state.remember_tokens(account.id, tokens).await;
    crate::crash::refresh_known(&state.db);

    let app2 = app.clone();
    tauri::async_runtime::spawn(async move {
        let _ = mail::sync_all(&app2).await;
    });
    Ok(account)
}

/// Signs a known account in again in place: new tokens, optionally with extra permissions
/// (`add_scopes`), keeping its mail, settings and calendar. Used for an expired sign-in and
/// for granting a permission a feature needs (rooms). It must be the same account.
#[tauri::command]
pub async fn reconnect_account(app: AppHandle, state: State<'_, AppState>, account_id: i64, add_scopes: Option<Vec<String>>) -> Result<Account> {
    let account = state
        .db
        .with(|c| db::list_accounts(c))?
        .into_iter()
        .find(|a| a.id == account_id)
        .ok_or_else(|| AppError::Other("no such account".into()))?;
    let base = state.oauth_for_account(account_id)?;
    // Keep what the account already granted; a stale or missing token just means the defaults.
    let had = state.granted_scopes(account_id).await.unwrap_or_else(|_| base.scopes().to_string());
    let extra: Vec<&str> = add_scopes.iter().flatten().map(String::as_str).collect();
    let cfg = base.with_scopes(auth::merge_scopes(&had, &extra)).with_login_hint(&account.email);

    cancel_pending_sign_in(&state);
    let (tx, rx) = tokio::sync::oneshot::channel();
    *state.sign_in_cancel.lock().expect("cancel mutex") = Some(tx);
    let (tokens, _) = auth::sign_in(&app, &state.http, &cfg, rx).await?;
    let email = if account.provider == "google" {
        crate::gmail::fetch_profile(&state.http, &tokens.access_token).await?
    } else {
        mail::fetch_profile(&state.http, &tokens.access_token).await?.0
    };
    if !email.eq_ignore_ascii_case(&account.email) {
        return Err(AppError::Auth(format!("You signed in as {email}, not {}. Try again and choose {}.", account.email, account.email)));
    }
    let blob = AppState::store_tokens_blob(&tokens)?;
    state.db.with(|c| db::set_token_blob(c, account_id, &blob))?;
    state.remember_tokens(account_id, tokens).await;
    crate::rooms::forget(account_id);

    let app2 = app.clone();
    tauri::async_runtime::spawn(async move {
        let _ = mail::sync_all(&app2).await;
    });
    Ok(Account { signed_in: true, ..account })
}

/// Signs an account out of this PC, removing its cached mail. `everywhere` (settings sync)
/// also takes it off the synced list of accounts, so the other PCs sign it out too; otherwise
/// this PC just stops offering it.
#[tauri::command]
pub async fn sign_out(state: State<'_, AppState>, account_id: i64, everywhere: Option<bool>) -> Result<()> {
    let account = state.db.with(|c| db::list_accounts(c))?.into_iter().find(|a| a.id == account_id);
    if let Some(a) = &account {
        state.db.with(|c| {
            if everywhere == Some(true) {
                crate::sync::accounts::forget_everywhere(c, &a.provider, &a.email)
            } else {
                crate::sync::accounts::forget_here(c, &a.provider, &a.email)
            }
        })?;
    }
    remove_account(&state, account_id).await
}

/// Removes an account and everything kept for it on this PC.
pub async fn remove_account(state: &AppState, account_id: i64) -> Result<()> {
    state.forget_tokens(account_id).await;
    state.db.with(|c| {
        db::delete_account(c, account_id)?;
        // Account ids can be reused: a later account mustn't inherit this one's link state.
        db::delete_setting(c, &crate::sync::store::link_key(account_id))
    })?;
    crate::crash::refresh_known(&state.db);
    Ok(())
}

#[tauri::command]
pub fn list_folders(state: State<'_, AppState>, account_id: i64) -> Result<Vec<Folder>> {
    state.db.with(|c| db::list_folders(c, account_id))
}

/// Threads in one folder, a mailbox (Done, Sent... across accounts) or, with neither, all
/// mail (search). `split` selects a tab of the split inbox (0 = Inbox, see `list_splits`)
/// and takes precedence over `mailbox` and `query`, which uses the query language in query.rs.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn list_threads(
    state: State<'_, AppState>,
    folder_id: Option<i64>,
    mailbox: Option<db::Mailbox>,
    query: Option<String>,
    split: Option<usize>,
    cursor: Option<Cursor>,
    limit: Option<i64>,
    hidden: Option<Vec<i64>>,
    with_server: Option<bool>,
) -> Result<Vec<ThreadRow>> {
    let limit = limit.unwrap_or(200).clamp(1, 1000);
    let started = std::time::Instant::now();
    let searching = split.is_none() && query.as_deref().is_some_and(|q| !q.trim().is_empty());
    let shape = query.as_deref().map(query::shape).unwrap_or_default();
    let filter = match split {
        Some(tab) => query::split_filter(&split_queries(&state)?, tab),
        None => query::compile(query.as_deref().unwrap_or("")),
    };
    // A search with server results: those join the local matches.
    let filter = match (searching, with_server) {
        (true, Some(true)) => query::with_server_hits(filter, query.as_deref().unwrap_or("")),
        _ => filter,
    };
    // Accounts toggled off in the footer.
    let filter = query::exclude_accounts(filter, hidden.as_deref().unwrap_or_default());
    let scope = match (folder_id, split, mailbox) {
        (Some(f), _, _) => Scope::Folder(f),
        (None, Some(_), _) => Mailbox::Inbox.scope(),
        (None, None, Some(m)) => m.scope(),
        (None, None, None) => Scope::AllMail,
    };
    let rows = state.db.with(|c| db::list_threads(c, scope, &filter, cursor.as_ref(), limit))?;
    // Searches only (not every list load), and only their shape: never the words.
    if searching && cursor.is_none() {
        use crate::telemetry::{attr, record_ms, span_done, Key};
        let attrs = [attr(Key::Terms, shape.0), attr(Key::Operators, shape.1), attr(Key::Results, rows.len())];
        span_done("search.query", started.elapsed(), &attrs);
        record_ms("search.query", started.elapsed().as_secs_f64() * 1000.0, &attrs[..2]);
    }
    Ok(rows)
}

/// Searches the server for the query (mail outside the cache window). Results join the
/// search's list (`list_threads` with `with_server`) until `clear_server_search`.
#[tauri::command]
pub async fn search_server(app: AppHandle, state: State<'_, AppState>, query: String, hidden: Option<Vec<i64>>) -> Result<server_search::Outcome> {
    server_search::search(&app, &state, &query, hidden.as_deref().unwrap_or_default()).await
}

/// Fetches the rest of a conversation opened from server results. True when anything came.
#[tauri::command]
pub async fn fetch_search_conversation(state: State<'_, AppState>, account_id: i64, conversation_id: String) -> Result<bool> {
    server_search::fetch_conversation(&state, account_id, &conversation_id).await
}

/// Closes the server search: mail it fetched leaves the cache again.
#[tauri::command]
pub fn clear_server_search(app: AppHandle, state: State<'_, AppState>) -> Result<()> {
    if server_search::clear(&state)? > 0 {
        let _ = app.emit(mail::EVENT_MAIL_CHANGED, serde_json::json!({ "accountId": null, "folderId": null }));
    }
    Ok(())
}

#[tauri::command]
pub fn get_thread(state: State<'_, AppState>, account_id: i64, conversation_id: String, from: Option<Mailbox>) -> Result<Vec<MessageRow>> {
    state.db.with(|c| db::get_thread(c, account_id, &conversation_id, from))
}

#[tauri::command]
pub async fn get_message_body(state: State<'_, AppState>, message_id: i64) -> Result<String> {
    mail::message_body(&state, message_id).await
}

/// Ctrl+R: retry anything queued (e.g. after being offline), then pull changes.
#[tauri::command]
pub async fn sync_now(app: AppHandle) -> Result<()> {
    outbox::kick(&app);
    mail::sync_all(&app).await
}

#[tauri::command]
pub async fn set_cache_limits(app: AppHandle, state: State<'_, AppState>, limits: CacheLimits) -> Result<()> {
    let days = limits.cache_days.clamp(7, 3650);
    let mb = limits.body_cache_mb.clamp(50, 20_000);
    let previous = state.cache_limits()?;
    state.db.with(|c| {
        db::set_setting(c, KEY_CACHE_DAYS, &days.to_string())?;
        db::set_setting(c, KEY_BODY_CACHE_MB, &mb.to_string())?;
        db::evict_bodies(c, mb * 1024 * 1024)?;
        // A wider window needs a fresh delta (the date filter is baked into the delta link).
        if days > previous.cache_days {
            db::clear_delta_links(c)?;
        }
        Ok(())
    })?;
    tauri::async_runtime::spawn(async move {
        let _ = mail::sync_all(&app).await;
    });
    Ok(())
}

// ---------- triage ----------

fn mail_changed(app: &AppHandle, account_id: i64) {
    let _ = app.emit(mail::EVENT_MAIL_CHANGED, serde_json::json!({ "accountId": account_id, "folderId": null }));
}

/// Adds a step to the undo stack (keeping the last 50).
fn push_undo(state: &AppState, group: actions::UndoGroup) {
    let mut undo = state.undo.lock().expect("undo mutex");
    undo.push(group);
    let excess = undo.len().saturating_sub(50);
    undo.drain(..excess);
}

/// Tells the UI and the outbox that local mail changed in these accounts.
fn after_triage(app: &AppHandle, state: &AppState, accounts: impl IntoIterator<Item = i64>) {
    outbox::emit_status(app, state);
    outbox::kick(app);
    let mut seen = std::collections::HashSet::new();
    for a in accounts {
        if seen.insert(a) {
            mail_changed(app, a);
        }
    }
}

/// A conversation named by the UI.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadRef {
    account_id: i64,
    conversation_id: String,
}

fn thread_refs(targets: Vec<ThreadRef>) -> Vec<(i64, String)> {
    targets.into_iter().map(|t| (t.account_id, t.conversation_id)).collect()
}

/// Applies a triage action to a conversation. Local state changes immediately; the
/// server catches up through the outbox.
#[tauri::command]
pub fn thread_action(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: i64,
    conversation_id: String,
    action: Action,
    undoable: Option<bool>,
) -> Result<ActionOutcome> {
    let entry = state.db.with(|c| actions::apply(c, account_id, &conversation_id, action))?;
    let changed = entry.before.len();
    let label = (changed > 0).then(|| entry.label.clone());
    if changed > 0 && undoable.unwrap_or(true) {
        push_undo(&state, actions::UndoGroup { label: entry.label.clone(), entries: vec![entry] });
    }
    if changed > 0 {
        after_triage(&app, &state, [account_id]);
    }
    Ok(ActionOutcome { label, changed })
}

/// Applies a triage action to several conversations at once (a selection), as one undo
/// step. `changed` counts the conversations that changed.
#[tauri::command]
pub fn threads_action(app: AppHandle, state: State<'_, AppState>, targets: Vec<ThreadRef>, action: Action) -> Result<ActionOutcome> {
    let targets = thread_refs(targets);
    let entries = state.db.with(|c| actions::apply_many(c, &targets, action))?;
    let changed = entries.len();
    let Some(first) = entries.first() else { return Ok(ActionOutcome { label: None, changed: 0 }) };
    let label = actions::batch_label(&first.label, changed);
    let accounts: Vec<i64> = entries.iter().map(|e| e.account_id).collect();
    push_undo(&state, actions::UndoGroup { label: label.clone(), entries });
    after_triage(&app, &state, accounts);
    Ok(ActionOutcome { label: Some(label), changed })
}

fn parse_snooze_time(until: &str) -> Result<chrono::DateTime<chrono::Utc>> {
    let until = chrono::DateTime::parse_from_rfc3339(until).map_err(|_| AppError::Other(format!("\"{until}\" isn't a time")))?.with_timezone(&chrono::Utc);
    if until <= chrono::Utc::now() {
        return Err(AppError::Other("that time has already passed".into()));
    }
    Ok(until)
}

/// Snoozes a conversation until `until` (RFC 3339). Undoable like any triage action.
#[tauri::command]
pub fn snooze_thread(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: i64,
    conversation_id: String,
    until: String,
) -> Result<ActionOutcome> {
    let until = parse_snooze_time(&until)?;
    let entry = state.db.with(|c| actions::snooze(c, account_id, &conversation_id, until))?;
    let changed = entry.snooze.is_some() as usize;
    if changed > 0 {
        push_undo(&state, actions::UndoGroup { label: entry.label.clone(), entries: vec![entry] });
    }
    after_triage(&app, &state, [account_id]);
    Ok(ActionOutcome { label: (changed > 0).then(|| "Snoozed".into()), changed })
}

/// Snoozes several conversations until `until`, as one undo step.
#[tauri::command]
pub fn snooze_threads(app: AppHandle, state: State<'_, AppState>, targets: Vec<ThreadRef>, until: String) -> Result<ActionOutcome> {
    let until = parse_snooze_time(&until)?;
    let targets = thread_refs(targets);
    let entries = state.db.with(|c| actions::snooze_many(c, &targets, until))?;
    let changed = entries.len();
    let label = (changed > 0).then(|| actions::batch_label("Snoozed", changed));
    if let Some(label) = &label {
        push_undo(&state, actions::UndoGroup { label: label.clone(), entries });
    }
    after_triage(&app, &state, targets.iter().map(|t| t.0));
    Ok(ActionOutcome { label, changed })
}

/// Brings a snoozed conversation back to the inbox now.
#[tauri::command]
pub fn unsnooze_thread(app: AppHandle, state: State<'_, AppState>, account_id: i64, conversation_id: String) -> Result<()> {
    state.db.with(|c| actions::wake(c, Some((account_id, &conversation_id))))?;
    outbox::emit_status(&app, &state);
    outbox::kick(&app);
    mail_changed(&app, account_id);
    Ok(())
}

// ---------- desktop: start with Windows, quit ----------

/// Tests and the drive CLI keep "start with Windows" in settings instead of the real
/// registry Run key (PEETMAIL_FAKE_AUTOSTART=1, debug builds only).
fn fake_autostart() -> bool {
    crate::testenv::flag("PEETMAIL_FAKE_AUTOSTART")
}

#[tauri::command]
pub fn get_autostart(app: AppHandle, state: State<'_, AppState>) -> Result<bool> {
    if fake_autostart() {
        return Ok(state.db.with(|c| db::get_setting(c, "fake_autostart"))?.as_deref() == Some("1"));
    }
    #[cfg(desktop)]
    {
        use tauri_plugin_autostart::ManagerExt;
        app.autolaunch().is_enabled().map_err(|e| AppError::Other(e.to_string()))
    }
    // Phones start nothing at sign-in; background sync is a scheduled job instead.
    #[cfg(mobile)]
    {
        let _ = app;
        Ok(false)
    }
}

/// Starts PeetMail (hidden, in the tray) when you sign in to Windows.
#[tauri::command]
pub fn set_autostart(app: AppHandle, state: State<'_, AppState>, enabled: bool) -> Result<()> {
    if fake_autostart() {
        return state.db.with(|c| db::set_setting(c, "fake_autostart", if enabled { "1" } else { "0" }));
    }
    #[cfg(desktop)]
    {
        use tauri_plugin_autostart::ManagerExt;
        let l = app.autolaunch();
        if enabled { l.enable() } else { l.disable() }.map_err(|e| AppError::Other(e.to_string()))
    }
    #[cfg(mobile)]
    {
        let _ = (app, enabled);
        Err(AppError::Other("Starting at sign-in isn't available on this device.".into()))
    }
}

/// Test hook (debug builds only): panics on a background thread, so tests can check that
/// panics reach the log file.
#[tauri::command]
pub fn debug_panic() -> Result<()> {
    if !cfg!(debug_assertions) {
        return Err(AppError::Other("not available".into()));
    }
    std::thread::Builder::new()
        .name("debug-panic".into())
        .spawn(|| panic!("debug_panic: deliberate test panic"))
        .map_err(|e| AppError::Other(e.to_string()))?;
    Ok(())
}

/// Opens a link (http, https, mailto) in the default browser or mail app. Every link the
/// UI opens comes through here, so tests (PEETMAIL_OPEN_LOG, debug builds only) record it
/// in a file instead of popping windows open on the test machine.
#[tauri::command]
pub fn open_external(app: AppHandle, url: String) -> Result<()> {
    let lower = url.to_ascii_lowercase();
    if !(lower.starts_with("https://") || lower.starts_with("http://") || lower.starts_with("mailto:")) {
        return Err(AppError::Other(format!("won't open {url}")));
    }
    if let Some(path) = crate::testenv::get("PEETMAIL_OPEN_LOG") {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
        writeln!(f, "{url}")?;
        return Ok(());
    }
    app.opener().open_url(&url, None::<&str>).map_err(|e| AppError::Other(format!("couldn't open the link: {e}")))
}

/// Really quits (closing the window only hides it to the tray).
#[tauri::command]
pub fn quit_app(app: AppHandle) {
    app.exit(0);
}

/// Android's back button on the root screen: PeetMail goes to the background, as other apps
/// do, rather than closing (it keeps its state and comes back where it was).
#[tauri::command]
pub fn move_to_background(app: AppHandle) -> Result<()> {
    #[cfg(target_os = "android")]
    {
        use tauri::Manager;
        let window = app.get_webview_window("main").ok_or_else(|| AppError::Other("no window".into()))?;
        window
            .with_webview(|webview| {
                webview.jni_handle().exec(|env, activity, _webview| {
                    if let Err(e) = env.call_method(activity, "moveTaskToBack", "(Z)Z", &[jni::objects::JValue::Bool(1)]) {
                        log::warn!("moving to the background: {e}");
                    }
                });
            })
            .map_err(|e| AppError::Other(e.to_string()))?;
    }
    #[cfg(not(target_os = "android"))]
    let _ = app;
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Privacy {
    /// False for builds without a Sentry DSN (local builds): nothing is ever sent.
    configured: bool,
    crash_reports: bool,
    /// False for builds without a telemetry endpoint.
    usage_configured: bool,
    /// The saved choice (on unless turned off); takes effect at the next start when turned on.
    usage_data: bool,
    install_id: String,
    notice_seen: bool,
}

const KEY_PRIVACY_NOTICE: &str = "privacy_notice_seen";

/// Settings → Privacy, and whether the first-run notice has been dismissed.
#[tauri::command]
pub fn privacy(state: State<'_, AppState>) -> Result<Privacy> {
    state.db.with(|c| {
        Ok(Privacy {
            configured: crate::crash::configured(),
            crash_reports: crate::crash::enabled(),
            usage_configured: crate::telemetry::configured(),
            usage_data: db::get_setting(c, crate::telemetry::KEY_USAGE_DATA)?.as_deref() != Some("off"),
            install_id: crate::crash::install_id(c)?,
            notice_seen: db::get_setting(c, KEY_PRIVACY_NOTICE)?.is_some(),
        })
    })
}

#[tauri::command]
pub fn set_crash_reports(state: State<'_, AppState>, enabled: bool) -> Result<()> {
    crate::crash::set_enabled(&state.db, enabled)
}

/// Usage and performance data: off at once; back on from the next start.
#[tauri::command]
pub fn set_usage_data(state: State<'_, AppState>, enabled: bool) -> Result<()> {
    state.db.with(|c| db::set_setting(c, crate::telemetry::KEY_USAGE_DATA, if enabled { "on" } else { "off" }))?;
    crate::telemetry::set_opted_in(enabled);
    Ok(())
}

/// Timings from the webview (keyboard actions to next paint, long tasks, first paint).
#[tauri::command]
pub fn ui_events(events: Vec<crate::telemetry::UiEvent>) {
    crate::telemetry::record_ui(&events);
}

/// Test hook (debug builds only): export buffered telemetry now.
#[tauri::command]
pub fn debug_flush_telemetry() -> Result<()> {
    if !cfg!(debug_assertions) {
        return Err(AppError::Other("not available".into()));
    }
    crate::telemetry::flush();
    Ok(())
}

/// Problems the startup health checks found (also sent as `health-changed`).
#[tauri::command]
pub fn health_issues() -> Vec<crate::health::Issue> {
    crate::health::current()
}

/// What Help → Report a problem can do in this build.
#[tauri::command]
pub fn feedback_options() -> crate::support::FeedbackOptions {
    crate::support::feedback_options()
}

/// Saves the diagnostics zip to Downloads and shows it. Returns where it went.
#[tauri::command]
pub async fn export_diagnostics(app: AppHandle) -> Result<String> {
    not_on_phones_yet("Saving diagnostics")?;
    let path = tauri::async_runtime::spawn_blocking(move || crate::support::export_diagnostics(&app))
        .await
        .map_err(|e| AppError::Other(e.to_string()))??;
    Ok(path.to_string_lossy().into_owned())
}

/// Sends feedback to the developers, with the diagnostics zip if asked.
#[tauri::command]
pub async fn send_feedback(app: AppHandle, message: String, email: Option<String>, include_diagnostics: bool) -> Result<()> {
    tauri::async_runtime::spawn_blocking(move || crate::support::send_feedback(&app, &message, email.as_deref(), include_diagnostics))
        .await
        .map_err(|e| AppError::Other(e.to_string()))?
}

const KEY_CHANGELOG_SEEN: &str = "changelog_seen";

/// Changelog lines already shown ("What's new"), by id; None before the first run records them.
#[tauri::command]
pub fn changelog_seen(state: State<'_, AppState>) -> Result<Option<Vec<String>>> {
    let raw = state.db.with(|c| db::get_setting(c, KEY_CHANGELOG_SEEN))?;
    Ok(raw.map(|r| serde_json::from_str(&r).unwrap_or_default()))
}

#[tauri::command]
pub fn set_changelog_seen(state: State<'_, AppState>, ids: Vec<String>) -> Result<()> {
    let ids: Vec<String> = ids.into_iter().filter(|i| i.len() <= 16 && i.chars().all(|c| c.is_ascii_hexdigit())).collect();
    state.db.with(|c| db::set_setting(c, KEY_CHANGELOG_SEEN, &serde_json::to_string(&ids)?))
}

#[tauri::command]
pub fn dismiss_privacy_notice(state: State<'_, AppState>) -> Result<()> {
    state.db.with(|c| db::set_setting(c, KEY_PRIVACY_NOTICE, "1"))
}

/// An error the webview didn't handle. It goes to Sentry from here (scrubbed), so the
/// webview's CSP never has to allow a network endpoint.
#[tauri::command]
pub fn report_error(name: String, message: String, stack: Option<String>, mechanism: String) {
    crate::crash::capture_js_error(&name, &message, stack.as_deref(), &mechanism);
}

/// Test hook (debug builds only): a native crash (an access violation, not a panic), so
/// tests can check that a minidump reaches the crash reporter.
#[tauri::command]
pub fn debug_crash() -> Result<()> {
    if !cfg!(debug_assertions) {
        return Err(AppError::Other("not available".into()));
    }
    log::error!("debug_crash: deliberate native crash");
    // SAFETY: deliberately unsound; the process is meant to die here.
    unsafe { std::ptr::null_mut::<u32>().write_volatile(1) };
    Ok(())
}

/// Where automatic updates stand (see update.rs).
#[tauri::command]
pub fn update_status(app: AppHandle) -> crate::update::UpdateStatus {
    crate::update::status(&app)
}

/// Checks for an update now (and downloads it if there is one).
#[tauri::command]
pub async fn check_for_updates(app: AppHandle) -> Result<crate::update::UpdateStatus> {
    Ok(crate::update::check(&app).await)
}

/// Closes PeetMail and installs the downloaded update.
#[tauri::command]
pub fn install_update(app: AppHandle) -> Result<()> {
    crate::update::install(&app)
}

#[tauri::command]
pub fn set_update_channel(app: AppHandle, channel: String) -> Result<crate::update::UpdateStatus> {
    crate::update::set_channel(&app, &channel)
}

/// Snoozed conversations, soonest back first.
#[tauri::command]
pub fn list_snoozed(state: State<'_, AppState>) -> Result<Vec<actions::SnoozedThread>> {
    state.db.with(|c| actions::list_snoozed(c))
}

/// Undoes the most recent action. Returns its label ("Archived") or None if nothing to undo.
#[tauri::command]
pub fn undo(app: AppHandle, state: State<'_, AppState>) -> Result<Option<String>> {
    let Some(group) = state.undo.lock().expect("undo mutex").pop() else { return Ok(None) };
    state.db.with(|c| actions::undo_group(c, &group))?;
    after_triage(&app, &state, group.entries.iter().map(|e| e.account_id));
    Ok(Some(group.label))
}

#[tauri::command]
pub fn outbox_status(state: State<'_, AppState>) -> Result<OutboxStatus> {
    state.db.with(|c| outbox::status(c))
}

// ---------- compose ----------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendInput {
    account_id: i64,
    kind: DraftKind,
    /// Message being replied to or forwarded.
    reference_message_id: Option<i64>,
    #[serde(default)]
    to: Vec<String>,
    #[serde(default)]
    cc: Vec<String>,
    #[serde(default)]
    bcc: Vec<String>,
    #[serde(default)]
    subject: String,
    /// The text as plain text.
    body: String,
    /// The text as the composer's rich text (HTML).
    #[serde(default)]
    html: String,
    /// Send later, at this time (RFC 3339). None: after the undo delay.
    #[serde(default)]
    send_at: Option<String>,
    /// Ids of files added with `add_draft_file`.
    #[serde(default)]
    attachments: Vec<i64>,
    /// Forwards: names of the original's attachments to leave out.
    #[serde(default)]
    dropped_attachments: Vec<String>,
    /// The saved draft this is, which sending removes.
    #[serde(default)]
    draft_id: Option<i64>,
    /// The conversation it answers, for the draft it's kept as until it has gone.
    #[serde(default)]
    conversation_id: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendOutcome {
    /// Outbox id, for undo / cancel.
    id: i64,
    /// When it goes out (UTC).
    send_at: String,
    scheduled: bool,
}

/// Queues a message for sending and returns immediately. It waits out the undo delay,
/// or until `send_at` when scheduled.
#[tauri::command]
pub fn send_message(app: AppHandle, state: State<'_, AppState>, input: SendInput) -> Result<SendOutcome> {
    let reference = match input.reference_message_id {
        Some(id) => Some(state.db.with(|c| db::message_remote(c, id))?.ok_or_else(|| AppError::Other("original message not found".into()))?.1),
        None => None,
    };
    if input.kind != DraftKind::New && reference.is_none() {
        return Err(AppError::Other("replies and forwards need the original message".into()));
    }
    let clean = |v: Vec<String>| v.into_iter().map(|a| a.trim().to_string()).filter(|a| !a.is_empty()).collect::<Vec<_>>();
    let when = match &input.send_at {
        Some(s) => {
            let t = chrono::DateTime::parse_from_rfc3339(s).map_err(|_| AppError::Other(format!("\"{s}\" isn't a time")))?.with_timezone(&chrono::Utc);
            if t <= chrono::Utc::now() {
                return Err(AppError::Other("that time has already passed".into()));
            }
            Some(t)
        }
        None => None,
    };
    let attachments = state.db.with(|c| draft_files::describe(c, &input.attachments))?;
    let total: i64 = attachments.iter().map(|f| f.size).sum();
    if total > draft_files::MAX_TOTAL {
        return Err(AppError::Other(format!(
            "the attachments add up to {}; the most a message can carry is {}",
            draft_files::megabytes(total),
            draft_files::megabytes(draft_files::MAX_TOTAL)
        )));
    }
    let draft = Draft {
        kind: input.kind,
        to: clean(input.to),
        cc: clean(input.cc),
        bcc: clean(input.bcc),
        subject: input.subject,
        body: input.body,
        html: input.html,
        send_at: when.map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
        reference_message_id: input.reference_message_id,
        attachments,
        dropped_attachments: if input.kind == DraftKind::Forward { input.dropped_attachments } else { vec![] },
        draft_id: input.draft_id,
    };
    if matches!(draft.kind, DraftKind::New | DraftKind::Forward) && draft.to.is_empty() && draft.cc.is_empty() && draft.bcc.is_empty() {
        return Err(AppError::Other("add at least one recipient".into()));
    }
    if let Some(bad) = draft.to.iter().chain(&draft.cc).chain(&draft.bcc).find(|a| !a.contains('@')) {
        return Err(AppError::Other(format!("\"{bad}\" isn't an email address")));
    }
    let delay = state.preferences()?.undo_send_secs;
    let at = when.unwrap_or_else(|| chrono::Utc::now() + chrono::Duration::seconds(delay));
    let mut draft = draft;
    let id = state.db.with(|conn| {
        // Kept as a draft until it has gone, out of the Drafts list meanwhile: back there if
        // the send is taken back or fails for good, removed (with its server copy) once it
        // has gone. One not saved as a draft yet is saved now. All or nothing: never a hidden
        // draft without its send.
        let tx = conn.transaction()?;
        let c = &tx;
        let draft_id = match input.draft_id {
            Some(d) => d,
            None => {
                let doc = drafts::DraftDoc {
                    kind: draft.kind,
                    reference_message_id: draft.reference_message_id,
                    conversation_id: input.conversation_id.clone(),
                    to: draft.to.join(", "),
                    cc: draft.cc.join(", "),
                    bcc: draft.bcc.join(", "),
                    subject: draft.subject.clone(),
                    body: draft.body.clone(),
                    html: draft.html.clone(),
                    attachments: draft.attachments.clone(),
                    dropped_attachments: draft.dropped_attachments.clone(),
                    imported: false,
                };
                drafts::save(c, None, input.account_id, &doc)?.id
            }
        };
        drafts::set_hidden(c, draft_id, true)?;
        draft.draft_id = Some(draft_id);
        let id = outbox::enqueue_at(c, input.account_id, reference.as_deref(), &Op::Send(draft.clone()), at)?;
        tx.commit()?;
        Ok(id)
    })?;
    outbox::emit_status(&app, &state);
    outbox::kick(&app);
    let _ = app.emit(EVENT_SCHEDULED, ());
    Ok(SendOutcome { id, send_at: at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true), scheduled: when.is_some() })
}

/// Takes a file the composer picked, dropped or pasted. The bytes are the raw request body
/// (no JSON or base64 round trip); the name and type come in `x-file-name` (percent-encoded)
/// and `x-file-type` headers.
#[tauri::command]
pub fn add_draft_file(state: State<'_, AppState>, request: tauri::ipc::Request<'_>) -> Result<draft_files::DraftFile> {
    let tauri::ipc::InvokeBody::Raw(data) = request.body() else {
        return Err(AppError::Other("expected the file's bytes".into()));
    };
    let header = |n: &str| request.headers().get(n).and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
    let name = url::form_urlencoded::parse(format!("n={}", header("x-file-name")).as_bytes())
        .next()
        .map(|(_, v)| v.into_owned())
        .unwrap_or_default();
    state.db.with(|c| draft_files::add(c, &name, &header("x-file-type"), data))
}

/// Saves the composer's message as a draft (as it's typed, and when it's closed). Returns
/// the draft's id, which later saves pass back.
#[tauri::command]
pub fn save_draft(app: AppHandle, state: State<'_, AppState>, id: Option<i64>, account_id: i64, draft: drafts::DraftDoc) -> Result<i64> {
    let id = state.db.with(|conn| {
        let tx = conn.transaction()?;
        let c = &tx;
        let saved = drafts::save(c, id, account_id, &draft)?;
        // Up to the server a moment later (one push for a burst of typing).
        outbox::enqueue_push_draft(c, account_id, saved.id)?;
        // Moved to another account (From changed): its copy in the old one goes.
        if let Some((old_account, remote_id)) = saved.moved_from {
            outbox::enqueue(c, old_account, Some(&remote_id), &Op::DeleteDraft, std::time::Duration::ZERO)?;
        }
        tx.commit()?;
        Ok(saved.id)
    })?;
    outbox::kick(&app);
    Ok(id)
}

/// Saved drafts, most recently edited first.
#[tauri::command]
pub fn list_drafts(state: State<'_, AppState>) -> Result<Vec<drafts::SavedDraft>> {
    state.db.with(|c| drafts::list(c))
}

/// Discards a draft and its attached files.
#[tauri::command]
pub fn delete_draft(app: AppHandle, state: State<'_, AppState>, id: i64) -> Result<()> {
    state.db.with(|c| {
        if let Some((account_id, remote_id)) = drafts::delete(c, id, false)? {
            outbox::enqueue(c, account_id, Some(&remote_id), &Op::DeleteDraft, std::time::Duration::ZERO)?;
        }
        Ok(())
    })?;
    outbox::kick(&app);
    Ok(())
}

/// Forgets files the composer no longer wants (removed, or the message discarded).
#[tauri::command]
pub fn remove_draft_files(state: State<'_, AppState>, ids: Vec<i64>) -> Result<()> {
    state.db.with(|c| draft_files::remove(c, &ids))
}

/// Fired when the set of queued sends changes (scheduled list).
pub const EVENT_SCHEDULED: &str = "scheduled-changed";

/// Takes a send back before it goes (undo send, or cancelling a scheduled one).
/// Returns the draft so the composer can reopen, or None if it has already gone.
#[tauri::command]
pub fn cancel_send(app: AppHandle, state: State<'_, AppState>, id: i64) -> Result<Option<outbox::QueuedSend>> {
    let taken = state.db.with(|conn| {
        let tx = conn.transaction()?;
        let taken = outbox::take_send(&tx, id)?;
        // Its draft is back (the composer reopens on it).
        if let Some(d) = taken.as_ref().and_then(|q| q.draft.draft_id) {
            outbox::restore_draft(&tx, d)?;
        }
        tx.commit()?;
        Ok(taken)
    })?;
    outbox::emit_status(&app, &state);
    let _ = app.emit(EVENT_SCHEDULED, ());
    Ok(taken)
}

/// Messages scheduled to send later, soonest first.
#[tauri::command]
pub fn scheduled_sends(state: State<'_, AppState>) -> Result<Vec<outbox::QueuedSend>> {
    state.db.with(|c| outbox::queued_sends(c, true))
}

#[tauri::command]
pub fn suggest_contacts(state: State<'_, AppState>, prefix: String) -> Result<Vec<Contact>> {
    state.db.with(|c| db::suggest_contacts(c, prefix.trim(), 8))
}

// ---------- splits ----------

/// Stored one per setting, `split:<id>` (see items.rs).
const SPLIT_PREFIX: &str = "split:";

#[derive(Debug, Clone, PartialEq, serde::Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Split {
    /// Stays the same through renames and reorders. None for a split not saved yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub name: String,
    pub query: String,
}

/// A split as stored (the id is in the key).
#[derive(Debug, Clone, PartialEq, serde::Serialize, Deserialize)]
struct StoredSplit {
    name: String,
    query: String,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SplitInfo {
    /// None for the built-in Inbox tab.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub name: String,
    pub query: String,
    /// The built-in Inbox tab: everything no split claims. Always first, not editable.
    pub builtin: bool,
    pub count: i64,
    pub unread: i64,
}

/// The user's splits (excluding the built-in Inbox tab), in tab order. A new install starts
/// with Other (migration 0014).
fn load_splits(state: &AppState) -> Result<Vec<Split>> {
    let stored = state.db.with(|c| items::load_ordered::<StoredSplit>(c, SPLIT_PREFIX))?;
    Ok(stored.into_iter().map(|(id, s)| Split { id: Some(id), name: s.name, query: s.query }).collect())
}

/// Trims and drops blank splits, and gives new ones (or ones with an unusable id) a fresh id.
fn clean_splits(splits: Vec<Split>) -> Vec<(String, StoredSplit)> {
    let mut out: Vec<(String, StoredSplit)> = Vec::new();
    for s in splits {
        let stored = StoredSplit { name: s.name.trim().into(), query: s.query.trim().into() };
        if stored.name.is_empty() || stored.query.is_empty() {
            continue;
        }
        let id = s
            .id
            .filter(|id| !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
            .filter(|id| !out.iter().any(|(o, _)| o == id))
            .unwrap_or_else(crate::calendar::uuid_like);
        out.push((id, stored));
    }
    out
}

fn split_queries(state: &AppState) -> Result<Vec<String>> {
    Ok(load_splits(state)?.into_iter().map(|s| s.query).collect())
}

/// Tabs of the split inbox for the given inbox folder: the built-in Inbox first, then the
/// user's splits, each with conversation counts. Every message lands in exactly one tab.
#[tauri::command]
pub fn list_splits(state: State<'_, AppState>, folder_id: Option<i64>, hidden: Option<Vec<i64>>) -> Result<Vec<SplitInfo>> {
    let hidden = hidden.unwrap_or_default();
    let splits = load_splits(&state)?;
    let queries: Vec<String> = splits.iter().map(|s| s.query.clone()).collect();
    let tabs = std::iter::once((None, "Inbox".to_string(), String::new(), true))
        .chain(splits.into_iter().map(|s| (s.id, s.name, s.query, false)));
    state.db.with(|c| {
        tabs.enumerate()
            .map(|(i, (id, name, query, builtin))| {
                let scope = folder_id.map(Scope::Folder).unwrap_or(Mailbox::Inbox.scope());
                let filter = query::exclude_accounts(query::split_filter(&queries, i), &hidden);
                let (count, unread) = db::count_threads(c, scope, &filter)?;
                Ok(SplitInfo { id, name, query, builtin, count, unread })
            })
            .collect()
    })
}

#[tauri::command]
pub fn set_splits(state: State<'_, AppState>, splits: Vec<Split>) -> Result<()> {
    let clean = clean_splits(splits);
    state.db.with(|c| items::save_ordered(c, SPLIT_PREFIX, &clean))
}

// ---------- snippets ----------

/// Stored one per setting, `snippet:<name, ASCII-lowercased>`: names are unique that way, so
/// the same snippet made on two devices is one entry.
const SNIPPET_PREFIX: &str = "snippet:";

/// Reusable text for the composer (`;name`). The body may use {first_name}, {name},
/// {my_first_name} and {my_name}; the frontend fills them in.
#[derive(Debug, Clone, PartialEq, serde::Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snippet {
    pub name: String,
    pub body: String,
}

/// Trims, drops blank rows and rejects duplicate names (case-insensitive), so `;name`
/// always means one snippet.
fn clean_snippets(snippets: Vec<Snippet>) -> Result<Vec<Snippet>> {
    let mut out: Vec<Snippet> = Vec::new();
    for s in snippets {
        let name = s.name.trim().to_string();
        let body = s.body.trim_end().to_string();
        if name.is_empty() && body.trim().is_empty() {
            continue;
        }
        if name.is_empty() {
            return Err(AppError::Other("every snippet needs a name".into()));
        }
        if name.contains(char::is_whitespace) {
            return Err(AppError::Other(format!("snippet names can't contain spaces (\"{name}\")")));
        }
        if out.iter().any(|o| o.name.eq_ignore_ascii_case(&name)) {
            return Err(AppError::Other(format!("there's already a snippet called \"{name}\"")));
        }
        out.push(Snippet { name, body });
    }
    Ok(out)
}

#[tauri::command]
pub fn list_snippets(state: State<'_, AppState>) -> Result<Vec<Snippet>> {
    let stored = state.db.with(|c| items::load_ordered::<Snippet>(c, SNIPPET_PREFIX))?;
    Ok(stored.into_iter().map(|(_, s)| s).collect())
}

#[tauri::command]
pub fn set_snippets(state: State<'_, AppState>, snippets: Vec<Snippet>) -> Result<Vec<Snippet>> {
    let clean = clean_snippets(snippets)?;
    let entries: Vec<(String, Snippet)> = clean.iter().map(|s| (s.name.to_ascii_lowercase(), s.clone())).collect();
    state.db.with(|c| items::save_ordered(c, SNIPPET_PREFIX, &entries))?;
    Ok(clean)
}

// ---------- signatures ----------

/// Stored one per setting, `signature:<lowercase address>`.
const SIGNATURE_PREFIX: &str = "signature:";

/// An account's signature. Keyed by address rather than account id, so it survives signing
/// out and adding the account again (ids can be reused), and so send-as addresses can have
/// their own later.
#[derive(Debug, Clone, PartialEq, serde::Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Signature {
    pub address: String,
    /// Sanitized HTML.
    pub html: String,
    /// Also added to replies and forwards (always added to new messages).
    #[serde(default)]
    pub replies: bool,
    /// The organisation adds a signature on its mail server (CodeTwo, Exclaimer...), so
    /// PeetMail adds none, or recipients would get two. `html` is kept for turning it off.
    #[serde(default)]
    pub managed: bool,
}

/// A signature as stored (the address is in the key).
#[derive(Debug, Clone, PartialEq, serde::Serialize, Deserialize)]
struct StoredSignature {
    html: String,
    #[serde(default)]
    replies: bool,
    #[serde(default)]
    managed: bool,
}

/// Lowercases addresses, sanitizes the HTML and drops empty signatures (unless the
/// organisation adds one); the last entry for an address wins.
fn clean_signatures(signatures: Vec<Signature>) -> Vec<Signature> {
    let mut out: Vec<Signature> = Vec::new();
    for s in signatures {
        let address = s.address.trim().to_lowercase();
        out.retain(|o| o.address != address);
        let html = crate::sanitize::clean_signature(&s.html);
        if !address.is_empty() && (html.is_some() || s.managed) {
            out.push(Signature { address, html: html.unwrap_or_default(), replies: s.replies, managed: s.managed });
        }
    }
    out
}

#[tauri::command]
pub fn list_signatures(state: State<'_, AppState>) -> Result<Vec<Signature>> {
    let stored = state.db.with(|c| items::load::<StoredSignature>(c, SIGNATURE_PREFIX))?;
    Ok(stored.into_iter().map(|(address, s)| Signature { address, html: s.html, replies: s.replies, managed: s.managed }).collect())
}

#[tauri::command]
pub fn set_signatures(state: State<'_, AppState>, signatures: Vec<Signature>) -> Result<Vec<Signature>> {
    let clean = clean_signatures(signatures);
    let entries: Vec<(String, StoredSignature)> = clean
        .iter()
        .map(|s| (s.address.clone(), StoredSignature { html: s.html.clone(), replies: s.replies, managed: s.managed }))
        .collect();
    state.db.with(|c| items::replace(c, SIGNATURE_PREFIX, &entries))?;
    Ok(clean)
}

// ---------- recent opens ----------

#[tauri::command]
pub fn record_open(state: State<'_, AppState>, account_id: i64, conversation_id: String) -> Result<()> {
    state.db.with(|c| db::record_open(c, account_id, &conversation_id))
}

#[tauri::command]
pub fn recent_opens(state: State<'_, AppState>, limit: Option<i64>) -> Result<Vec<RecentOpen>> {
    state.db.with(|c| db::recent_opens(c, limit.unwrap_or(50).clamp(1, 200)))
}

// ---------- calendar ----------

/// Events overlapping a range: `from`/`to` are UTC instants for timed events, `from_date`/
/// `to_date` local dates (YYYY-MM-DD, end exclusive) for all-day events.
#[tauri::command]
pub fn list_events(
    state: State<'_, AppState>,
    from: String,
    to: String,
    from_date: String,
    to_date: String,
) -> Result<Vec<crate::calendar::Event>> {
    state.db.with(|c| crate::calendar::list_events(c, &from, &to, &from_date, &to_date))
}

/// Creates an event (quick add). It shows at once; the outbox creates it on the server.
#[tauri::command]
pub fn create_event(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: i64,
    mut event: crate::outbox::NewEvent,
) -> Result<crate::calendar::Event> {
    if event.subject.trim().is_empty() {
        return Err(AppError::Other("an event needs a title".into()));
    }
    event.details.stamp_meeting_request();
    let created = state.db.with(|c| {
        let accounts = db::list_accounts(c)?;
        let me = accounts.iter().find(|a| a.id == account_id).ok_or_else(|| AppError::Other("unknown account".into()))?;
        crate::calendar::create(c, account_id, &event, (&me.email, me.display_name.as_deref()))
    })?;
    outbox::emit_status(&app, &state);
    outbox::kick(&app);
    let _ = app.emit(crate::calendar::EVENT_CALENDAR_CHANGED, serde_json::json!({ "accountId": account_id }));
    Ok(created)
}

/// The event behind a meeting invite (None for ordinary mail).
#[tauri::command]
pub async fn invite_event(state: State<'_, AppState>, message_id: i64) -> Result<Option<crate::calendar::Invite>> {
    crate::calendar::invite_for_message(&state, message_id).await
}

/// The event the editor works on, fresh from the server: the occurrence, or with
/// `series` its series master. `time_zone`: the user's IANA zone (for all-day dates).
#[tauri::command]
pub async fn event_for_edit(
    state: State<'_, AppState>,
    account_id: i64,
    remote_id: String,
    series: bool,
    time_zone: String,
) -> Result<crate::event_edit::EditableEvent> {
    crate::event_edit::load(&state, account_id, &remote_id, series, &time_zone).await
}

/// Saves an edit: shown at once, sent through the outbox (guests get an update).
/// `remote_id` is what the editor loaded (an occurrence or a series master).
#[tauri::command]
pub fn update_event(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: i64,
    remote_id: String,
    mut changes: crate::event_edit::EventChanges,
) -> Result<()> {
    if changes.is_empty() {
        return Ok(());
    }
    changes.stamp_meeting_request();
    if changes.subject.as_deref().is_some_and(|s| s.trim().is_empty()) {
        return Err(AppError::Other("an event needs a title".into()));
    }
    state.db.with(|c| {
        let tx = c.transaction()?;
        crate::event_edit::apply_locally(&tx, account_id, &remote_id, &changes)?;
        outbox::enqueue(&tx, account_id, Some(&remote_id), &Op::UpdateEvent { changes }, std::time::Duration::ZERO)?;
        tx.commit()?;
        Ok(())
    })?;
    outbox::emit_status(&app, &state);
    outbox::kick(&app);
    let _ = app.emit(crate::calendar::EVENT_CALENDAR_CHANGED, serde_json::json!({ "accountId": account_id }));
    Ok(())
}

/// Deletes an event (an occurrence, or a whole series via its master). `cancel`: you
/// organise it and it has guests: they get a cancellation (with `comment`).
#[tauri::command]
pub fn delete_calendar_event(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: i64,
    remote_id: String,
    cancel: bool,
    comment: Option<String>,
) -> Result<()> {
    state.db.with(|c| {
        let tx = c.transaction()?;
        crate::calendar::remove(&tx, account_id, &remote_id, cancel, comment)?;
        tx.commit()?;
        Ok(())
    })?;
    outbox::emit_status(&app, &state);
    outbox::kick(&app);
    let _ = app.emit(crate::calendar::EVENT_CALENDAR_CHANGED, serde_json::json!({ "accountId": account_id }));
    Ok(())
}

/// The online meeting a new event on this account can have ("teamsForBusiness",
/// "hangoutsMeet"...), or None.
#[tauri::command]
pub async fn meeting_provider(state: State<'_, AppState>, account_id: i64) -> Result<Option<String>> {
    crate::event_edit::meeting_provider(&state, account_id).await
}

/// Meeting rooms of a Microsoft 365 account (location suggestions); none for others.
#[tauri::command]
pub async fn list_rooms(state: State<'_, AppState>, account_id: i64) -> Result<crate::rooms::RoomList> {
    crate::rooms::list(&state, account_id).await
}

/// Everything the event popup shows beyond the cached row (description, repeat pattern...).
#[tauri::command]
pub async fn event_detail(state: State<'_, AppState>, account_id: i64, remote_id: String) -> Result<crate::calendar::EventDetail> {
    crate::calendar::event_detail(&state, account_id, &remote_id).await
}

/// RSVP to an event: updates the calendar at once and sends the reply in the background.
/// `series`: answer the whole series an occurrence belongs to.
#[tauri::command]
pub fn respond_to_event(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: i64,
    event_remote_id: String,
    response: crate::outbox::Rsvp,
    series: Option<bool>,
) -> Result<()> {
    let target = if series.unwrap_or(false) {
        state
            .db
            .with(|c| crate::calendar::event_by_remote(c, account_id, &event_remote_id))?
            .and_then(|e| e.series_master_id)
            .unwrap_or(event_remote_id)
    } else {
        event_remote_id
    };
    state.db.with(|c| crate::calendar::respond(c, account_id, &target, response))?;
    outbox::emit_status(&app, &state);
    outbox::kick(&app);
    let _ = app.emit(crate::calendar::EVENT_CALENDAR_CHANGED, serde_json::json!({ "accountId": account_id }));
    Ok(())
}

// ---------- images and attachments ----------

#[tauri::command]
pub async fn get_message_body_with_images(state: State<'_, AppState>, message_id: i64) -> Result<String> {
    mail::message_body_with_images(&state, message_id).await
}

#[tauri::command]
pub async fn list_attachments(state: State<'_, AppState>, message_id: i64) -> Result<Vec<mail::Attachment>> {
    mail::list_attachments(&state, message_id).await
}

/// Android has no Downloads folder an app can write to directly, and other apps only open
/// files shared through a content URI; both wait for docs/ANDROID.md Stage 4. Until then
/// these say so instead of saving where nobody can find the file.
fn not_on_phones_yet(what: &str) -> Result<()> {
    if cfg!(mobile) {
        return Err(AppError::Other(format!("{what} isn't available on Android yet.")));
    }
    Ok(())
}

/// Saves an attachment to Downloads and reveals it in Explorer. Returns the saved path.
#[tauri::command]
pub async fn download_attachment(
    app: AppHandle,
    state: State<'_, AppState>,
    message_id: i64,
    attachment_id: String,
    name: String,
) -> Result<String> {
    not_on_phones_yet("Saving attachments")?;
    let dir = match crate::testenv::get("PEETMAIL_DOWNLOAD_DIR") {
        Some(d) => std::path::PathBuf::from(d),
        None => tauri::Manager::path(&app).download_dir().map_err(|e| AppError::Other(e.to_string()))?,
    };
    let path = mail::download_attachment(&state, message_id, &attachment_id, &name, &dir).await?;
    if crate::testenv::get("PEETMAIL_DOWNLOAD_DIR").is_none() {
        let _ = app.opener().reveal_item_in_dir(&path);
    }
    Ok(path.to_string_lossy().into_owned())
}

/// Where attachments are copied to be opened. Windows: the user's own temp folder. Linux:
/// PeetMail's cache folder, kept private, since `/tmp` is shared by every user.
fn attachments_dir(app: &AppHandle) -> Result<std::path::PathBuf> {
    let paths = tauri::Manager::path(app);
    if cfg!(windows) {
        return Ok(paths.temp_dir().map_err(|e| AppError::Other(e.to_string()))?.join("PeetMail").join("attachments"));
    }
    let dir = paths.app_cache_dir().map_err(|e| AppError::Other(e.to_string()))?.join("attachments");
    std::fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(dir)
}

/// Opens an attachment in its default app (double-click), from a copy in PeetMail's temp
/// folder. Tests (PEETMAIL_OPEN_LOG) get the copy's path logged instead of an app opening.
#[tauri::command]
pub async fn open_attachment(
    app: AppHandle,
    state: State<'_, AppState>,
    message_id: i64,
    attachment_id: String,
    name: String,
) -> Result<()> {
    not_on_phones_yet("Opening attachments")?;
    let dir = attachments_dir(&app)?;
    let path = mail::attachment_for_opening(&state, message_id, &attachment_id, &name, &dir).await?;
    if let Some(log) = crate::testenv::get("PEETMAIL_OPEN_LOG") {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(log)?;
        writeln!(f, "{}", path.display())?;
        return Ok(());
    }
    app.opener().open_path(path.to_string_lossy(), None::<&str>).map_err(|e| AppError::Other(format!("couldn't open {name}: {e}")))
}

#[cfg(test)]
mod tests {
    #[test]
    fn signatures_are_sanitized_keyed_by_lowercase_address_and_empty_ones_dropped() {
        use super::{clean_signatures, Signature};
        let sig = |address: &str, html: &str, replies: bool, managed: bool| Signature { address: address.into(), html: html.into(), replies, managed };
        let clean = clean_signatures(vec![
            sig(" Boris@Peetmail.test ", "<p>Old</p>", false, false),
            sig("boris@peetmail.test", r#"<p onclick="x()">Boris</p>"#, true, false),
            sig("home@outlook.com", "<p></p>", true, false),
            sig("work@contoso.com", "<p></p>", false, true),
            sig("", "<p>Nobody</p>", false, false),
        ]);
        assert_eq!(clean, vec![sig("boris@peetmail.test", "<p>Boris</p>", true, false), sig("work@contoso.com", "", false, true)]);
    }

    #[test]
    fn snippets_are_trimmed_blank_rows_dropped_and_names_unique() {
        use super::{clean_snippets, Snippet};
        let snip = |name: &str, body: &str| Snippet { name: name.into(), body: body.into() };
        let clean = clean_snippets(vec![snip(" thanks ", "Thanks {first_name}!\n\n"), snip("", "  "), snip("intro", "Hi")]).unwrap();
        assert_eq!(clean, vec![snip("thanks", "Thanks {first_name}!"), snip("intro", "Hi")]);
        assert!(clean_snippets(vec![snip("a", "x"), snip("A", "y")]).unwrap_err().to_string().contains("already"));
        assert!(clean_snippets(vec![snip("", "body only")]).unwrap_err().to_string().contains("needs a name"));
        assert!(clean_snippets(vec![snip("two words", "x")]).unwrap_err().to_string().contains("spaces"));
    }

    #[test]
    fn splits_keep_their_ids_and_new_ones_get_one() {
        let split = |id: Option<&str>, n: &str, q: &str| super::Split { id: id.map(String::from), name: n.into(), query: q.into() };
        let clean = super::clean_splits(vec![
            split(Some("other"), " Other ", "is:other"),
            split(None, "Boss", "from:boss@contoso.com"),
            split(Some("other"), "Copy", "is:unread"),
            split(Some("bad key!"), "Odd", "is:flagged"),
            split(None, "  ", "dropped"),
        ]);
        let names: Vec<(&str, &str)> = clean.iter().map(|(id, s)| (id.as_str(), s.name.as_str())).collect();
        assert_eq!(names[0], ("other", "Other"));
        assert_eq!(names.len(), 4, "the blank split is dropped");
        let ids: std::collections::HashSet<&str> = names.iter().map(|n| n.0).collect();
        assert_eq!(ids.len(), 4, "a duplicate or unusable id gets a fresh one");
        assert!(!ids.contains("bad key!"));
    }

    #[test]
    fn validates_guids() {
        assert!(super::is_guid("e7f36417-119b-4407-8631-35b13727823a"));
        assert!(!super::is_guid("e7f36417119b4407863135b13727823a"));
        assert!(!super::is_guid("e7f36417-119b-4407-8631-35b13727823"));
        assert!(!super::is_guid("g7f36417-119b-4407-8631-35b13727823a"));
        assert!(!super::is_guid("--------------------------------e7f3"));
    }
}
