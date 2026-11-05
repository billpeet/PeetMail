//! Mail sync against Microsoft Graph. Incremental via delta links, so steady-state
//! polls are one cheap request per folder.

use chrono::{Duration, Utc};
use reqwest::Method;
use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};
use url::Url;

use crate::db::{self, Account, Folder, MessageUpsert};
use crate::error::{AppError, Result};
use crate::graph::{self, PREFER_IMMUTABLE};
use crate::{outbox, sanitize};
use crate::state::AppState;

const MESSAGE_FIELDS: &str = "id,conversationId,subject,from,toRecipients,ccRecipients,receivedDateTime,\
bodyPreview,isRead,flag,hasAttachments,importance,inferenceClassification";
/// Derived-type property: present only on meeting messages, so it identifies invites and
/// responses even when the response omits `@odata.type`.
const MEETING_FIELD: &str = "microsoft.graph.eventMessage/meetingMessageType";
const PAGE_SIZE: &str = "odata.maxpagesize=100";

pub const EVENT_MAIL_CHANGED: &str = "mail-changed";
pub const EVENT_SYNC_STATUS: &str = "sync-status";

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    pub state: &'static str, // "syncing" | "idle" | "error"
    pub message: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MailChanged {
    account_id: i64,
    folder_id: i64,
}

pub async fn fetch_profile(http: &reqwest::Client, token: &str) -> Result<(String, Option<String>)> {
    let me = graph::get(http, token, &format!("{}/me?$select=displayName,mail,userPrincipalName", graph::base()), &[]).await?;
    let email = me["mail"]
        .as_str()
        .or_else(|| me["userPrincipalName"].as_str())
        .ok_or_else(|| AppError::Other("profile has no email address".into()))?
        .to_lowercase();
    Ok((email, me["displayName"].as_str().map(String::from)))
}

/// Graph request with the account's token. A 401 means the token was revoked or expired
/// early (password change, admin action, clock skew), so drop it, refresh, and retry once.
pub async fn graph_call(
    state: &AppState,
    account_id: i64,
    method: Method,
    url: &str,
    body: Option<&Value>,
    prefer: &[&str],
) -> Result<Value> {
    let token = state.access_token(account_id).await?;
    match graph::request(&state.http, &token, method.clone(), url, body, prefer).await {
        Err(AppError::Graph { status: 401, .. }) => {
            log::warn!("graph returned 401 for account {account_id}, refreshing token");
            state.invalidate_access_token(account_id).await;
            let token = state.access_token(account_id).await?;
            graph::request(&state.http, &token, method, url, body, prefer).await
        }
        other => other,
    }
}

async fn graph_get(state: &AppState, account_id: i64, url: &str, prefer: &[&str]) -> Result<Value> {
    graph_call(state, account_id, Method::GET, url, None, prefer).await
}

async fn graph_bytes(state: &AppState, account_id: i64, url: &str) -> Result<Vec<u8>> {
    let token = state.access_token(account_id).await?;
    match graph::get_bytes(&state.http, &token, url).await {
        Err(AppError::Graph { status: 401, .. }) => {
            state.invalidate_access_token(account_id).await;
            let token = state.access_token(account_id).await?;
            graph::get_bytes(&state.http, &token, url).await
        }
        other => other,
    }
}

/// Syncs every account. Errors are reported per account and don't stop the others.
pub async fn sync_all(app: &AppHandle) -> Result<()> {
    crate::telemetry::traced("sync.run", vec![], sync_all_accounts(app)).await
}

async fn sync_all_accounts(app: &AppHandle) -> Result<()> {
    let state = app.state::<AppState>();
    let _guard = state.sync_lock.lock().await;
    let accounts = state.db.with(|c| db::list_accounts(c))?;
    let _ = app.emit(EVENT_SYNC_STATUS, SyncStatus { state: "syncing", message: None });

    let mut last_error = None;
    for account in accounts.iter().filter(|a| a.signed_in) {
        let provider = if account.provider == "google" { "google" } else { "microsoft" };
        let attrs = vec![crate::telemetry::attr(crate::telemetry::Key::Provider, provider)];
        if let Err(e) = crate::telemetry::traced("sync.account", attrs, sync_account(app, &state, account)).await {
            log::error!("sync failed for {}: {e}", account.email);
            last_error = Some(format!("{}: {e}", account.email));
        }
    }

    // Inside the sync.run span, so this line links to the sync's trace.
    let synced = accounts.iter().filter(|a| a.signed_in).count();
    log::info!("sync finished: {synced} account(s), {}", if last_error.is_some() { "with errors" } else { "ok" });
    let status = match last_error {
        Some(msg) => SyncStatus { state: "error", message: Some(msg) },
        None => SyncStatus { state: "idle", message: None },
    };
    let _ = app.emit(EVENT_SYNC_STATUS, status);
    crate::notify::after_sync(app);
    tauri::async_runtime::spawn(prefetch_bodies(app.clone()));
    Ok(())
}

/// Bodies kept warm: the newest inbox messages plus every unread one.
const PREFETCH_NEWEST: i64 = 60;
const PREFETCH_MAX: i64 = 200;
const PREFETCH_CONCURRENCY: usize = 4;

/// Downloads bodies the user is likely to open next into the SQLite cache, so opening a
/// message never waits on the network. Runs after each sync; already-cached bodies are skipped.
pub async fn prefetch_bodies(app: AppHandle) {
    let state = app.state::<AppState>();
    let Ok(_guard) = state.prefetch_lock.try_lock() else { return };
    let ids = match state.db.with(|c| db::bodies_to_prefetch(c, PREFETCH_NEWEST, PREFETCH_MAX)) {
        Ok(ids) => ids,
        Err(e) => return log::error!("prefetch query: {e}"),
    };
    if ids.is_empty() {
        return;
    }
    log::info!("prefetching {} message bodies", ids.len());
    let mut tasks = tokio::task::JoinSet::new();
    for id in ids {
        while tasks.len() >= PREFETCH_CONCURRENCY {
            tasks.join_next().await;
        }
        let app = app.clone();
        // Background: Gmail calls wait for quota budget, ahead of nothing you open yourself.
        tasks.spawn(crate::gmail::background(async move {
            let state = app.state::<AppState>();
            if let Err(e) = message_body(&state, id).await {
                log::info!("prefetch body {id}: {e}");
            }
        }));
    }
    while tasks.join_next().await.is_some() {}
}

/// Folders kept in sync, inside the mail window. Sent is synced so threads show your
/// replies; Archive and Deleted Items back the Done and Trash views.
const SYNCED_FOLDERS: &[(&str, &str)] = &[("inbox", "Inbox"), ("sentitems", "Sent Items"), ("archive", "Archive"), ("deleteditems", "Deleted Items")];

/// Folders a mailbox may not have (some Exchange mailboxes have no Archive): a 404 skips them.
const OPTIONAL_FOLDERS: &[&str] = &["archive"];

async fn sync_account(app: &AppHandle, state: &AppState, account: &Account) -> Result<()> {
    sync_mail_and_calendar(app, state, account).await?;
    // Drafts problems mustn't fail the sync.
    match crate::draft_sync::pull(state, account.id).await {
        Ok(true) => {
            let _ = app.emit(crate::draft_sync::EVENT_DRAFTS_CHANGED, ());
        }
        Ok(false) => {}
        Err(e) => log::warn!("drafts sync failed for account {}: {e}", account.id),
    }
    Ok(())
}

async fn sync_mail_and_calendar(app: &AppHandle, state: &AppState, account: &Account) -> Result<()> {
    use crate::telemetry::{attr, traced, Key};
    if account.provider == "google" {
        traced("sync.mail", vec![attr(Key::Provider, "google")], crate::gmail::background(crate::gmail::sync_account(app, state, account.id))).await?;
        // Calendar problems (e.g. no calendar permission yet) mustn't stop mail.
        let cal = traced("sync.calendar", vec![attr(Key::Provider, "google")], crate::gmail::background(crate::gcal::sync_account(app, state, account.id)));
        if let Err(e) = cal.await {
            log::warn!("google calendar sync failed for {}: {e}", account.email);
        }
        return Ok(());
    }
    for (role, name) in SYNCED_FOLDERS {
        let folder = match ensure_folder(state, account.id, role, name).await {
            Ok(f) => f,
            Err(AppError::Graph { status: 404, .. }) if OPTIONAL_FOLDERS.contains(role) => {
                log::info!("no {role} folder for account {}, not syncing it", account.id);
                continue;
            }
            Err(e) => return Err(e),
        };
        let attrs = vec![crate::telemetry::attr(crate::telemetry::Key::Folder, *role)];
        crate::telemetry::traced("sync.folder", attrs, sync_folder(app, state, account.id, &folder)).await?;
    }
    // Calendar problems (e.g. a tenant that blocks calendar access) mustn't stop mail.
    if let Err(e) = traced("sync.calendar", vec![attr(Key::Provider, "microsoft")], crate::calendar::sync_account(app, state, account.id)).await {
        log::warn!("calendar sync failed for {}: {e}", account.email);
    }
    Ok(())
}

/// The folder row for a well-known Graph folder, fetching its id on first use. A local
/// placeholder (`wk:<role>`, made when mail was archived or deleted before the folder was
/// synced) gets its real id in place, so the mail already in it stays put.
async fn ensure_folder(state: &AppState, account_id: i64, role: &str, default_name: &str) -> Result<Folder> {
    let existing = state.db.with(|c| db::folder_by_role(c, account_id, role))?;
    if let Some(f) = &existing
        && !f.remote_id.starts_with(db::PLACEHOLDER_PREFIX) {
        return Ok(f.clone());
    }
    let f = graph_get(
        state,
        account_id,
        &format!("{}/me/mailFolders/{role}?$select=id,displayName", graph::base()),
        &[PREFER_IMMUTABLE],
    )
    .await?;
    let remote_id = f["id"].as_str().ok_or_else(|| AppError::Other(format!("{role} folder has no id")))?;
    let name = f["displayName"].as_str().unwrap_or(default_name);
    state.db.with(|c| match &existing {
        Some(placeholder) => db::resolve_placeholder(c, placeholder.id, remote_id, name),
        None => db::insert_folder(c, account_id, remote_id, role, name),
    })
}

fn initial_delta_url(folder: &Folder, since: Option<&str>, meeting_field: bool) -> String {
    let base = format!("{}/me/mailFolders/{}/messages/delta", graph::base(), folder.remote_id);
    let select = if meeting_field { format!("{MESSAGE_FIELDS},{MEETING_FIELD}") } else { MESSAGE_FIELDS.to_string() };
    let mut params = vec![("$select", select)];
    if let Some(since) = since {
        params.push(("$filter", format!("receivedDateTime ge {since}")));
    }
    Url::parse_with_params(&base, &params).expect("valid url").to_string()
}

async fn sync_folder(app: &AppHandle, state: &AppState, account_id: i64, folder: &Folder) -> Result<()> {
    let limits = state.cache_limits()?;
    let cutoff = (Utc::now() - Duration::days(limits.cache_days)).format("%Y-%m-%dT%H:%M:%SZ").to_string();

    let full_sync = folder.delta_link.is_none();
    // Ask for the meeting type too; if Graph rejects that (400), drop it, then the date filter.
    let mut meeting_field = true;
    let mut url = folder.delta_link.clone().unwrap_or_else(|| initial_delta_url(folder, Some(&cutoff), meeting_field));
    // Set if Graph rejects the server-side date filter, so we filter locally instead.
    let mut filter_locally = false;
    let mut restarted = false;
    let mut first_page = true;
    let (mut pages, mut changes) = (0usize, 0usize);

    loop {
        let page = match graph_get(state, account_id, &url, &[PREFER_IMMUTABLE, PAGE_SIZE]).await {
            Ok(p) => p,
            Err(AppError::Graph { status: 400, message }) if first_page && full_sync && meeting_field => {
                log::warn!("delta rejected the meeting-type field ({message}), retrying without it");
                meeting_field = false;
                url = initial_delta_url(folder, (!filter_locally).then_some(cutoff.as_str()), false);
                continue;
            }
            Err(AppError::Graph { status: 400, message }) if first_page && full_sync && !filter_locally => {
                log::warn!("delta $filter rejected ({message}), filtering locally");
                filter_locally = true;
                url = initial_delta_url(folder, None, meeting_field);
                continue;
            }
            Err(AppError::Graph { status: 410, .. }) if !restarted => {
                log::warn!("delta link expired for folder {}, resyncing", folder.id);
                restarted = true;
                state.db.with(|c| db::set_delta_link(c, folder.id, None))?;
                url = initial_delta_url(folder, Some(&cutoff), meeting_field);
                continue;
            }
            Err(e) => return Err(e),
        };
        first_page = false;

        let items = page["value"].as_array().cloned().unwrap_or_default();
        pages += 1;
        changes += items.len();
        if !items.is_empty() {
            state.db.with(|c| {
                // Messages with unsent local changes keep their local state until the outbox
                // catches up; the next delta after that reflects the server's truth.
                let busy = outbox::busy_remote_ids(c, account_id)?;
                let tx = c.transaction()?;
                for item in &items {
                    let Some(remote_id) = item["id"].as_str() else { continue };
                    if busy.contains(remote_id) {
                        continue;
                    }
                    if item.get("@removed").is_some() {
                        db::delete_message_in_folder(&tx, account_id, folder.id, remote_id)?;
                        continue;
                    }
                    let m = parse_message(item);
                    if filter_locally && m.received_at.as_deref().is_some_and(|at| at < cutoff.as_str()) {
                        continue;
                    }
                    db::upsert_message(&tx, account_id, folder.id, &m)?;
                }
                tx.commit()?;
                Ok(())
            })?;
            let _ = app.emit(EVENT_MAIL_CHANGED, MailChanged { account_id, folder_id: folder.id });
        }

        if let Some(next) = page["@odata.nextLink"].as_str() {
            url = next.to_string();
            continue;
        }
        let delta = page["@odata.deltaLink"].as_str();
        state.db.with(|c| db::set_delta_link(c, folder.id, delta))?;
        break;
    }

    {
        use crate::telemetry::{annotate, attr, Key};
        annotate(&[attr(Key::Pages, pages), attr(Key::Items, changes), attr(Key::FullResync, full_sync || restarted)]);
    }
    let pruned = state.db.with(|c| db::prune_older_than(c, folder.id, &cutoff))?;
    if pruned > 0 {
        let _ = app.emit(EVENT_MAIL_CHANGED, MailChanged { account_id, folder_id: folder.id });
    }
    Ok(())
}

fn str_field(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(String::from)
}

fn recipients(v: &Value, key: &str) -> Option<String> {
    let list = v.get(key)?.as_array()?;
    let simplified: Vec<Value> = list
        .iter()
        .map(|r| json!({ "name": r["emailAddress"]["name"], "address": r["emailAddress"]["address"] }))
        .collect();
    Some(Value::Array(simplified).to_string())
}

/// Graph fields missing from the payload map to `None` so partial delta updates
/// don't overwrite cached values.
pub fn parse_message(v: &Value) -> MessageUpsert {
    MessageUpsert {
        remote_id: str_field(v, "id").unwrap_or_default(),
        conversation_id: str_field(v, "conversationId"),
        subject: v.get("subject").map(|s| s.as_str().unwrap_or_default().to_string()),
        from_name: v.pointer("/from/emailAddress/name").and_then(Value::as_str).map(String::from),
        from_address: v.pointer("/from/emailAddress/address").and_then(Value::as_str).map(String::from),
        to_json: recipients(v, "toRecipients"),
        cc_json: recipients(v, "ccRecipients"),
        received_at: str_field(v, "receivedDateTime"),
        preview: str_field(v, "bodyPreview"),
        is_read: v.get("isRead").and_then(Value::as_bool),
        is_flagged: v.pointer("/flag/flagStatus").and_then(Value::as_str).map(|s| s == "flagged"),
        has_attachments: v.get("hasAttachments").and_then(Value::as_bool),
        importance: str_field(v, "importance"),
        inference: str_field(v, "inferenceClassification"),
        // Meeting invites and responses arrive as #microsoft.graph.eventMessage, and carry
        // meetingMessageType (when selected). Either marks a meeting message.
        message_type: match (v.get("@odata.type").and_then(Value::as_str), v.get("meetingMessageType")) {
            (Some(t), _) if t.contains("eventMessage") => Some("eventMessage".into()),
            (_, Some(Value::String(_))) => Some("eventMessage".into()),
            (Some(_), _) => Some("message".into()),
            _ => None,
        },
        meeting_message_type: str_field(v, "meetingMessageType"),
    }
}

// ---------- server search ----------

/// Searches the whole mailbox (Graph `$search`, KQL from `query::graph_search`) and keeps
/// what it finds for the open search. Returns how many messages it found.
pub async fn search_server(state: &AppState, account_id: i64, kql: &str, limit: usize, generation: u64) -> Result<usize> {
    let url = Url::parse_with_params(
        &format!("{}/me/messages", graph::base()),
        &[
            ("$search", crate::query::graph_search_param(kql)),
            ("$top", limit.to_string()),
            ("$select", format!("{MESSAGE_FIELDS},parentFolderId,isDraft")),
        ],
    )
    .expect("valid url");
    let page = graph_get(state, account_id, url.as_str(), &[PREFER_IMMUTABLE]).await?;
    keep_search_results(state, account_id, &page, true, generation)
}

/// Fetches the rest of a conversation opened from server search results (its other messages
/// may be outside the cache window too).
pub async fn fetch_conversation(state: &AppState, account_id: i64, conversation_id: &str, generation: u64) -> Result<usize> {
    let url = Url::parse_with_params(
        &format!("{}/me/messages", graph::base()),
        &[
            ("$filter", format!("conversationId eq '{}'", conversation_id.replace('\'', "''"))),
            ("$top", "100".to_string()),
            ("$select", format!("{MESSAGE_FIELDS},parentFolderId,isDraft")),
        ],
    )
    .expect("valid url");
    let page = graph_get(state, account_id, url.as_str(), &[PREFER_IMMUTABLE]).await?;
    keep_search_results(state, account_id, &page, false, generation)
}

/// Stores search results in the folders they're in: mail in Deleted Items is left out (like
/// local search) and so are drafts; mail in folders PeetMail doesn't sync counts as archived.
fn keep_search_results(state: &AppState, account_id: i64, page: &Value, hit: bool, generation: u64) -> Result<usize> {
    let items = page["value"].as_array().cloned().unwrap_or_default();
    state.db.with(|c| {
        // The search closed (or another started) while this was on its way.
        if !crate::server_search::still_open(generation) {
            return Ok(0);
        }
        let folders = db::list_folders(c, account_id)?;
        let tx = c.transaction()?;
        let mut kept = 0;
        for v in &items {
            if v["isDraft"].as_bool() == Some(true) {
                continue;
            }
            let parent = v["parentFolderId"].as_str();
            let folder = folders.iter().find(|f| parent == Some(f.remote_id.as_str()));
            let folder_id = match folder {
                Some(f) if f.role.as_deref() == Some("deleteditems") => continue,
                Some(f) if f.role.is_some() => f.id,
                _ => db::ensure_local_folder(&tx, account_id, "archive", "Archive")?.id,
            };
            db::keep_search_result(&tx, account_id, folder_id, &parse_message(v), hit)?;
            kept += 1;
        }
        tx.commit()?;
        Ok(kept)
    })
}

/// Returns sanitized HTML for a message, from cache or Graph.
pub async fn message_body(state: &AppState, message_id: i64) -> Result<String> {
    use crate::telemetry::{attr, count, traced, Key};
    if let Some(html) = state.db.with(|c| db::get_body(c, message_id))? {
        count("peetmail.body.reads", 1, &[attr(Key::Cached, true)]);
        return Ok(html);
    }
    count("peetmail.body.reads", 1, &[attr(Key::Cached, false)]);
    traced("body.fetch", vec![], fetch_body(state, message_id)).await
}

async fn fetch_body(state: &AppState, message_id: i64) -> Result<String> {
    let (account_id, remote_id) = state
        .db
        .with(|c| db::message_remote(c, message_id))?
        .ok_or_else(|| AppError::Other("message not found".into()))?;
    if is_google(state, account_id)? {
        let (html, ics) = crate::gmail::fetch_body(state, account_id, &remote_id).await?;
        let invite = ics.as_deref().and_then(crate::gcal::parse_ics);
        state.db.with(|c| crate::gcal::record_invite(c, message_id, invite.as_ref()))?;
        return cache_body(state, message_id, html);
    }
    let v = graph_get(
        state,
        account_id,
        &format!("{}/me/messages/{remote_id}?$select=body", graph::base()),
        &[PREFER_IMMUTABLE, r#"outlook.body-content-type="html""#],
    )
    .await?;
    let content = v.pointer("/body/content").and_then(Value::as_str).unwrap_or_default();
    let started = std::time::Instant::now();
    let mut html = match v.pointer("/body/contentType").and_then(Value::as_str) {
        Some("text") => sanitize::text_to_html(content),
        _ => sanitize::clean_html(content),
    };
    {
        use crate::telemetry::{annotate, attr, record_ms, size_bucket, Key};
        let bucket = attr(Key::SizeBucket, size_bucket(content.len() as u64));
        record_ms("body.sanitize", started.elapsed().as_secs_f64() * 1000.0, std::slice::from_ref(&bucket));
        crate::telemetry::record_bytes("body.fetch", content.len() as u64, &[]);
        annotate(&[bucket, attr(Key::Provider, "microsoft")]);
    }
    if html.contains("src=\"cid:") {
        match inline_cid_images(state, account_id, &remote_id, &html).await {
            Ok(h) => html = h,
            Err(e) => log::warn!("inline images for message {message_id}: {e}"),
        }
    }
    cache_body(state, message_id, html)
}

fn is_google(state: &AppState, account_id: i64) -> Result<bool> {
    Ok(state.db.with(|c| db::account_provider(c, account_id))? == "google")
}

/// Stores a fetched body (and its text, for search) within the cache budget.
fn cache_body(state: &AppState, message_id: i64, html: String) -> Result<String> {
    let text = sanitize::html_to_text(&html);
    let max_bytes = state.cache_limits()?.body_cache_mb * 1024 * 1024;
    state.db.with(|c| {
        db::put_body(c, message_id, &html, &text)?;
        db::evict_bodies(c, max_bytes)?;
        Ok(())
    })?;
    Ok(html)
}

const MAX_INLINE_BYTES: usize = 5 * 1024 * 1024;

/// Replaces `cid:` image references with data URIs from the message's inline attachments.
async fn inline_cid_images(state: &AppState, account_id: i64, remote_id: &str, html: &str) -> Result<String> {
    let list = graph_get(
        state,
        account_id,
        &format!("{}/me/messages/{remote_id}/attachments?$filter=isInline eq true", graph::base()),
        &[PREFER_IMMUTABLE],
    )
    .await?;
    let mut by_cid = std::collections::HashMap::new();
    for a in list["value"].as_array().into_iter().flatten() {
        let (Some(cid), Some(ct), Some(bytes)) = (a["contentId"].as_str(), a["contentType"].as_str(), a["contentBytes"].as_str())
        else {
            continue;
        };
        if ct.starts_with("image/") && bytes.len() <= MAX_INLINE_BYTES * 4 / 3 {
            by_cid.insert(cid.trim_matches(['<', '>']).to_string(), format!("data:{ct};base64,{bytes}"));
        }
    }
    Ok(sanitize::rewrite_img_src(html, |src| src.strip_prefix("cid:").and_then(|cid| by_cid.get(cid).cloned())))
}

/// The cached body with remote images fetched by Rust and inlined as data URIs, so the
/// WebView never contacts the sender's servers and the CSP stays strict. Not cached.
pub async fn message_body_with_images(state: &AppState, message_id: i64) -> Result<String> {
    use base64::Engine;
    let html = message_body(state, message_id).await?;
    let mut fetched = std::collections::HashMap::new();
    for url in sanitize::remote_images(&html).iter().take(60) {
        match fetch_image(&state.http, url).await {
            Ok((ct, bytes)) => {
                let data = base64::engine::general_purpose::STANDARD.encode(bytes);
                fetched.insert(url.clone(), format!("data:{ct};base64,{data}"));
            }
            Err(e) => log::info!("image {url}: {e}"),
        }
    }
    Ok(sanitize::rewrite_img_src(&html, |src| fetched.get(&src.replace("&amp;", "&")).cloned()))
}

async fn fetch_image(http: &reqwest::Client, url: &str) -> Result<(String, Vec<u8>)> {
    let res = http.get(url).timeout(std::time::Duration::from_secs(10)).send().await?.error_for_status()?;
    let ct = res
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    // SVG can carry script; only raster formats go into the page.
    if !ct.starts_with("image/") || ct.contains("svg") {
        return Err(AppError::Other(format!("not a raster image ({ct})")));
    }
    let bytes = res.bytes().await?;
    if bytes.len() > MAX_INLINE_BYTES {
        return Err(AppError::Other("image too large".into()));
    }
    Ok((ct, bytes.to_vec()))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    pub id: String,
    pub name: String,
    pub content_type: String,
    pub size: i64,
}

fn remote_of(state: &AppState, message_id: i64) -> Result<(i64, String)> {
    state
        .db
        .with(|c| db::message_remote(c, message_id))?
        .ok_or_else(|| AppError::Other("message not found".into()))
}

/// Non-inline attachments of a message.
pub async fn list_attachments(state: &AppState, message_id: i64) -> Result<Vec<Attachment>> {
    let (account_id, remote_id) = remote_of(state, message_id)?;
    if is_google(state, account_id)? {
        return crate::gmail::list_attachments(state, account_id, &remote_id).await;
    }
    let v = graph_get(
        state,
        account_id,
        &format!("{}/me/messages/{remote_id}/attachments?$select=id,name,contentType,size,isInline", graph::base()),
        &[PREFER_IMMUTABLE],
    )
    .await?;
    Ok(v["value"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|a| !a["isInline"].as_bool().unwrap_or(false))
        .map(|a| Attachment {
            id: a["id"].as_str().unwrap_or_default().into(),
            name: a["name"].as_str().unwrap_or("attachment").into(),
            content_type: a["contentType"].as_str().unwrap_or_default().into(),
            size: a["size"].as_i64().unwrap_or(0),
        })
        .collect())
}

async fn attachment_bytes(state: &AppState, message_id: i64, attachment_id: &str) -> Result<Vec<u8>> {
    let (account_id, remote_id) = remote_of(state, message_id)?;
    if is_google(state, account_id)? {
        crate::gmail::attachment(state, account_id, &remote_id, attachment_id).await
    } else {
        graph_bytes(state, account_id, &format!("{}/me/messages/{remote_id}/attachments/{attachment_id}/$value", graph::base())).await
    }
}

/// Downloads an attachment into `dir` under a free, sanitized file name. Returns the path.
pub async fn download_attachment(
    state: &AppState,
    message_id: i64,
    attachment_id: &str,
    name: &str,
    dir: &std::path::Path,
) -> Result<std::path::PathBuf> {
    let bytes = attachment_bytes(state, message_id, attachment_id).await?;
    std::fs::create_dir_all(dir)?;
    let path = unique_path(dir, &safe_file_name(name));
    std::fs::write(&path, bytes)?;
    Ok(path)
}

/// File types that run code when opened. PeetMail won't open these straight from a
/// message (a double-click is too easy); they can still be saved and opened deliberately.
const RISKY_EXTENSIONS: &[&str] = &[
    "exe", "com", "bat", "cmd", "scr", "pif", "msi", "msp", "msc", "cpl", "dll", "sys", "drv",
    "js", "jse", "vbs", "vbe", "wsf", "wsh", "wsc", "ps1", "psm1", "psd1", "hta", "lnk", "url",
    "scf", "reg", "inf", "jar", "chm", "appx", "appxbundle", "msix", "msixbundle", "application",
    "appref-ms", "gadget", "iso", "img", "vhd", "vhdx", "xll", "library-ms", "settingcontent-ms",
    // Linux: launchers, scripts and installers.
    "desktop", "sh", "bash", "run", "appimage", "deb", "rpm", "flatpak", "flatpakref", "snap",
];

/// The extension, if it's one that runs code.
pub fn risky_extension(name: &str) -> Option<String> {
    // Windows ignores trailing dots and spaces, so "evil.exe." still runs.
    let name = name.trim_end_matches(['.', ' ']);
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    RISKY_EXTENSIONS.contains(&ext.as_str()).then_some(ext)
}

/// Saves an attachment for opening: `dir/<message id>/<name>`. The same attachment opened
/// again reuses its file (if it's still open in its app and locked, the copy there is the
/// same anyway). Marked as downloaded from the internet, so Office opens it in Protected
/// View and SmartScreen checks it.
pub async fn attachment_for_opening(
    state: &AppState,
    message_id: i64,
    attachment_id: &str,
    name: &str,
    dir: &std::path::Path,
) -> Result<std::path::PathBuf> {
    if let Some(ext) = risky_extension(name) {
        return Err(AppError::Other(format!(
            "PeetMail doesn't open .{ext} files directly, because they can run programs. Click it to save it to Downloads instead."
        )));
    }
    // A folder per attachment (two can share a name), and an existing copy is opened as it
    // is: it may be open in its app, with edits not to be overwritten.
    use sha2::Digest;
    let key: String = sha2::Sha256::digest(attachment_id.as_bytes()).iter().take(6).map(|b| format!("{b:02x}")).collect();
    let dir = dir.join(message_id.to_string()).join(key);
    let path = dir.join(safe_file_name(name));
    if !path.exists() {
        let bytes = attachment_bytes(state, message_id, attachment_id).await?;
        std::fs::create_dir_all(&dir)?;
        std::fs::write(&path, &bytes)?;
    }
    // Without the mark, Office would open it with its macros allowed: don't open it at all.
    if let Err(e) = mark_from_internet(&path) {
        log::warn!("couldn't mark an opened attachment as from the internet: {e}");
        return Err(AppError::Other(format!(
            "PeetMail couldn't mark {name} as downloaded from the internet, so it won't open it directly. Click it to save it to Downloads instead."
        )));
    }
    Ok(path)
}

/// The Mark of the Web: a Zone.Identifier stream saying the file came from the internet.
/// Linux has no such mark (and a `:` in a name is just a character), so nothing is written.
#[cfg(not(windows))]
fn mark_from_internet(_: &std::path::Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(windows)]
fn mark_from_internet(path: &std::path::Path) -> std::io::Result<()> {
    let mut stream = path.as_os_str().to_owned();
    stream.push(":Zone.Identifier");
    std::fs::write(&stream, "[ZoneTransfer]\r\nZoneId=3\r\n")
}

fn safe_file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if c.is_control() || r#"<>:"/\|?*"#.contains(c) { '_' } else { c })
        .collect();
    let trimmed = cleaned.trim().trim_matches('.').to_string();
    if trimmed.is_empty() { "attachment".into() } else { trimmed }
}

fn unique_path(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    let p = dir.join(name);
    if !p.exists() {
        return p;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (name.to_string(), String::new()),
    };
    (1..).map(|i| dir.join(format!("{stem} ({i}){ext}"))).find(|p| !p.exists()).expect("a free file name")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_are_made_safe() {
        assert_eq!(safe_file_name(r#"..\..\evil:name?.exe"#), "_.._evil_name_.exe");
        assert_eq!(safe_file_name("   "), "attachment");
    }

    #[test]
    fn programs_are_never_opened_directly() {
        assert_eq!(risky_extension("setup.EXE").as_deref(), Some("exe"));
        assert_eq!(risky_extension("invoice.pdf.js").as_deref(), Some("js"));
        assert_eq!(risky_extension("evil.exe. . ").as_deref(), Some("exe"), "Windows drops trailing dots and spaces");
        assert_eq!(risky_extension("Shortcut.lnk").as_deref(), Some("lnk"));
        assert_eq!(risky_extension("disk.iso").as_deref(), Some("iso"));
        assert_eq!(risky_extension("Q4 plan.pdf"), None);
        assert_eq!(risky_extension("Budget.xlsx"), None);
        assert_eq!(risky_extension("README"), None);
    }

    #[test]
    fn parses_full_and_partial_messages() {
        let full = json!({
            "id": "AAA", "conversationId": "C1", "subject": "Hello",
            "from": { "emailAddress": { "name": "Sam", "address": "sam@example.com" } },
            "toRecipients": [{ "emailAddress": { "name": "Boris", "address": "boris@example.com" } }],
            "receivedDateTime": "2026-10-20T08:00:00Z", "bodyPreview": "Hi there",
            "isRead": false, "flag": { "flagStatus": "flagged" }, "hasAttachments": true
        });
        let m = parse_message(&full);
        assert_eq!(m.from_address.as_deref(), Some("sam@example.com"));
        assert_eq!(m.is_flagged, Some(true));
        assert!(m.to_json.unwrap().contains("boris@example.com"));
        assert_eq!(m.cc_json, None);

        let invite = parse_message(&json!({ "id": "I", "meetingMessageType": "meetingRequest" }));
        assert_eq!(invite.message_type.as_deref(), Some("eventMessage"));
        assert_eq!(invite.meeting_message_type.as_deref(), Some("meetingRequest"));
        let typed = parse_message(&json!({ "id": "T", "@odata.type": "#microsoft.graph.eventMessageResponse" }));
        assert_eq!(typed.message_type.as_deref(), Some("eventMessage"));

        let partial = parse_message(&json!({ "id": "AAA", "isRead": true }));
        assert_eq!(partial.is_read, Some(true));
        assert_eq!(partial.subject, None);
        assert_eq!(partial.received_at, None);
        assert_eq!(partial.message_type, None, "unknown type leaves the stored one alone");
    }

    #[test]
    fn initial_url_encodes_filter() {
        let f = Folder {
            id: 1, account_id: 1, remote_id: "INBOX=".into(), role: Some("inbox".into()),
            name: "Inbox".into(), delta_link: None, last_synced_at: None,
        };
        let u = initial_delta_url(&f, Some("2026-01-01T00:00:00Z"), false);
        assert!(u.starts_with("https://graph.microsoft.com/v1.0/me/mailFolders/INBOX=/messages/delta?"));
        assert!(u.contains("%24filter=receivedDateTime+ge+2026-01-01T00%3A00%3A00Z"));
    }
}
