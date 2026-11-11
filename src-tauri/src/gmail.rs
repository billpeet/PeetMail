//! Gmail, through the Gmail API. Maps Gmail's labels onto PeetMail's folder model:
//!
//! | Labels on the message            | Local folder role |
//! |----------------------------------|-------------------|
//! | TRASH                            | deleteditems      |
//! | SPAM                             | junkemail         |
//! | only DRAFT                       | (not kept)        |
//! | PeetMail/Snoozed (our label)     | snoozed           |
//! | INBOX                            | inbox             |
//! | SENT                             | sentitems         |
//! | anything else                    | archive           |
//!
//! Sync: the first run lists inbox, sent, archived, trashed and spam mail inside the cache window;
//! after that the History API returns what changed since the last historyId, and each
//! changed message's metadata is fetched again (its labels say where it now lives). Threads
//! are Gmail's own threadId. Triage ops become label changes (archive = remove INBOX).

use std::collections::{HashMap, HashSet};

use base64::engine::general_purpose::{STANDARD, URL_SAFE, URL_SAFE_NO_PAD};
use base64::Engine;
use chrono::{Duration, TimeZone, Utc};
use reqwest::Method;
use serde_json::{json, Value};
use crate::context::Core;

use crate::db::{self, MessageUpsert};
use crate::error::{AppError, Result};
use crate::outbox::Op;
use crate::state::AppState;
use crate::{mail, outbox, sanitize};

const DEFAULT_BASE: &str = "https://gmail.googleapis.com/gmail/v1/users/me";
static BASE: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| crate::testenv::get("PEETMAIL_GMAIL_URL").unwrap_or_else(|| DEFAULT_BASE.to_string()));

pub fn base() -> &'static str {
    &BASE
}

pub const SNOOZE_LABEL: &str = "PeetMail/Snoozed";
/// Parallel metadata fetches (the quota budget below sets the actual pace).
const CONCURRENCY: usize = 8;

// ---------- quota ----------
//
// Gmail allows 15,000 quota units per user per minute (about 250 a second) and every call
// has a cost (messages.get is 5). Background work (sync, body prefetch) spends from a
// per-account budget well under that, so what you open yourself always has headroom.
// Quota errors that still happen are retried by graph::send.

tokio::task_local! {
    /// Set while syncing or prefetching: those calls wait for budget.
    static BACKGROUND: bool;
}

/// Runs `f` as background work (budgeted).
pub async fn background<F: std::future::Future>(f: F) -> F::Output {
    BACKGROUND.scope(true, f).await
}

fn in_background() -> bool {
    BACKGROUND.try_with(|b| *b).unwrap_or(false)
}

/// Background units per second, and how many can go at once. Burst + one second's refill
/// must stay under Google's ~250 units/s short-term limit: at most 200 in any second.
const BUDGET_PER_SEC: f64 = 100.0;
const BUDGET_BURST: f64 = 100.0;

struct Bucket {
    units: f64,
    at: std::time::Instant,
}

static BUCKETS: std::sync::LazyLock<std::sync::Mutex<HashMap<i64, Bucket>>> = std::sync::LazyLock::new(Default::default);

/// Waits until `cost` units of the account's background budget are free, then spends them.
async fn spend(account_id: i64, cost: f64) {
    loop {
        let wait = {
            let mut buckets = BUCKETS.lock().expect("quota buckets");
            let b = buckets.entry(account_id).or_insert(Bucket { units: BUDGET_BURST, at: std::time::Instant::now() });
            let now = std::time::Instant::now();
            b.units = (b.units + now.duration_since(b.at).as_secs_f64() * BUDGET_PER_SEC).min(BUDGET_BURST);
            b.at = now;
            if b.units >= cost {
                b.units -= cost;
                return;
            }
            (cost - b.units) / BUDGET_PER_SEC
        };
        tokio::time::sleep(std::time::Duration::from_secs_f64(wait)).await;
    }
}

/// Gmail's quota cost of a call (developers.google.com/workspace/gmail/api/reference/quota).
pub fn cost(method: &Method, url: &str) -> f64 {
    let path = url.split('?').next().unwrap_or(url);
    let tail = path.strip_prefix(base()).unwrap_or(path);
    if tail.starts_with("/history") {
        2.0
    } else if tail == "/profile" || (tail == "/labels" && method == Method::GET) {
        1.0
    } else if tail.ends_with("/send") {
        100.0
    } else {
        5.0
    }
}
const METADATA: &str = "format=metadata&metadataHeaders=From&metadataHeaders=To&metadataHeaders=Cc&metadataHeaders=Subject";

pub(crate) async fn call(state: &AppState, account_id: i64, method: Method, url: &str, body: Option<&Value>) -> Result<Value> {
    if in_background() {
        spend(account_id, cost(&method, url)).await;
    }
    mail::graph_call(state, account_id, method, url, body, &[]).await
}

async fn get(state: &AppState, account_id: i64, url: &str) -> Result<Value> {
    call(state, account_id, Method::GET, url, None).await
}

/// The mailbox address, straight after sign-in.
pub async fn fetch_profile(http: &reqwest::Client, token: &str) -> Result<String> {
    let v = crate::graph::get(http, token, &format!("{}/profile", base()), &[]).await?;
    v["emailAddress"]
        .as_str()
        .map(|s| s.to_lowercase())
        .ok_or_else(|| AppError::Other("Gmail profile has no address".into()))
}

// ---------- parsing ----------

/// RFC 2047 encoded words (=?UTF-8?B?...?= / =?ISO-8859-1?Q?...?=), in case a header
/// comes back undecoded.
pub fn decode_words(s: &str) -> String {
    if !s.contains("=?") {
        return s.to_string();
    }
    let mut out = String::new();
    let mut rest = s;
    let mut last_was_word = false;
    while let Some(start) = rest.find("=?") {
        let (before, after) = rest.split_at(start);
        // Whitespace between two encoded words is dropped.
        if !(last_was_word && before.trim().is_empty()) {
            out.push_str(before);
        }
        let parts: Vec<&str> = after[2..].splitn(3, '?').collect();
        let decoded = if parts.len() == 3 {
            let (charset, enc, tail) = (parts[0], parts[1], parts[2]);
            tail.find("?=").and_then(|end| {
                let text = &tail[..end];
                let bytes = match enc.to_ascii_uppercase().as_str() {
                    "B" => STANDARD.decode(text).ok(),
                    "Q" => Some(decode_q(text)),
                    _ => None,
                }?;
                let consumed = 2 + charset.len() + 1 + enc.len() + 1 + end + 2;
                Some((decode_charset(&bytes, charset), consumed))
            })
        } else {
            None
        };
        match decoded {
            Some((text, consumed)) => {
                out.push_str(&text);
                rest = &after[consumed..];
                last_was_word = true;
            }
            None => {
                out.push_str("=?");
                rest = &after[2..];
                last_was_word = false;
            }
        }
    }
    out.push_str(rest);
    out
}

fn decode_q(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'_' => out.push(b' '),
            b'=' if i + 2 < b.len() => {
                match u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("zz"), 16) {
                    Ok(v) => {
                        out.push(v);
                        i += 2;
                    }
                    Err(_) => out.push(b'='),
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    out
}

/// UTF-8 when it is; otherwise Latin-1 (close enough for Windows-1252 text).
pub fn decode_charset(bytes: &[u8], charset: &str) -> String {
    let cs = charset.to_ascii_lowercase();
    if (cs.is_empty() || cs.contains("utf-8") || cs.contains("utf8") || cs.contains("ascii"))
        && let Ok(s) = std::str::from_utf8(bytes) {
        return s.to_string();
    }
    match std::str::from_utf8(bytes) {
        Ok(s) if !(cs.contains("8859") || cs.contains("1252") || cs.contains("latin")) => s.to_string(),
        _ => bytes.iter().map(|&b| b as char).collect(),
    }
}

/// "Name <a@b>", "a@b", "\"Last, First\" <a@b>" -> (name, address).
fn parse_one_address(s: &str) -> Option<(Option<String>, String)> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let (Some(lt), Some(gt)) = (s.rfind('<'), s.rfind('>'))
        && lt < gt {
        let address = s[lt + 1..gt].trim().to_string();
        let name = s[..lt].trim().trim_matches('"').trim().to_string();
        return Some(((!name.is_empty()).then(|| decode_words(&name)), address));
    }
    Some((None, s.trim_matches('"').to_string()))
}

/// A header's address list, split on commas outside quotes and angle brackets.
pub fn parse_addresses(header: &str) -> Vec<(Option<String>, String)> {
    let mut out = Vec::new();
    let (mut depth_angle, mut quoted, mut cur) = (false, false, String::new());
    for ch in header.chars() {
        match ch {
            '"' => quoted = !quoted,
            '<' if !quoted => depth_angle = true,
            '>' if !quoted => depth_angle = false,
            ',' if !quoted && !depth_angle => {
                out.extend(parse_one_address(&cur));
                cur.clear();
                continue;
            }
            _ => {}
        }
        cur.push(ch);
    }
    out.extend(parse_one_address(&cur));
    out
}

fn recipients_json(header: Option<&str>) -> Option<String> {
    let list: Vec<Value> = parse_addresses(header?).into_iter().map(|(name, address)| json!({ "name": name, "address": address })).collect();
    Some(Value::Array(list).to_string())
}

/// Gmail snippets are HTML-escaped.
pub fn unescape_snippet(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let tail = &rest[amp..];
        // Entities are short: look a few characters ahead (by char: snippets carry
        // multi-byte characters such as zero-width joiners right after '&').
        let Some(semi) = tail.char_indices().take(10).find(|&(_, c)| c == ';').map(|(i, _)| i) else {
            out.push('&');
            rest = &tail[1..];
            continue;
        };
        let entity = &tail[1..semi];
        let ch = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            e if e.starts_with("#x") => u32::from_str_radix(&e[2..], 16).ok().and_then(char::from_u32),
            e if e.starts_with('#') => e[1..].parse().ok().and_then(char::from_u32),
            _ => None,
        };
        match ch {
            Some(c) => {
                out.push(c);
                rest = &tail[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn header<'a>(payload: &'a Value, name: &str) -> Option<&'a str> {
    payload["headers"]
        .as_array()?
        .iter()
        .find(|h| h["name"].as_str().is_some_and(|n| n.eq_ignore_ascii_case(name)))
        .and_then(|h| h["value"].as_str())
}

/// Which local folder a message with these labels lives in (None: not kept).
pub fn role_for(labels: &[&str], snooze_label: Option<&str>) -> Option<&'static str> {
    let has = |l: &str| labels.contains(&l);
    if has("TRASH") {
        return Some("deleteditems");
    }
    if has("SPAM") {
        return Some(db::JUNK_ROLE);
    }
    if has("DRAFT") && !has("INBOX") && !has("SENT") {
        return None;
    }
    if snooze_label.is_some_and(has) {
        return Some("snoozed");
    }
    if has("INBOX") {
        return Some("inbox");
    }
    if has("SENT") {
        return Some("sentitems");
    }
    Some("archive")
}

fn folder_name(role: &str) -> &'static str {
    match role {
        "inbox" => "Inbox",
        "sentitems" => "Sent",
        "deleteditems" => "Trash",
        db::JUNK_ROLE => "Spam",
        "snoozed" => "Snoozed",
        _ => "All Mail",
    }
}

/// A message resource (format=metadata or full) -> its folder role and fields.
pub fn parse_message(v: &Value, snooze_label: Option<&str>) -> (Option<&'static str>, MessageUpsert) {
    let labels: Vec<&str> = v["labelIds"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    let payload = &v["payload"];
    let from = header(payload, "From").and_then(|f| parse_addresses(f).into_iter().next());
    let received_at = v["internalDate"]
        .as_str()
        .and_then(|ms| ms.parse::<i64>().ok())
        .and_then(|ms| Utc.timestamp_millis_opt(ms).single())
        .map(|t| t.format("%Y-%m-%dT%H:%M:%SZ").to_string());
    let category_other = labels.iter().any(|l| matches!(*l, "CATEGORY_PROMOTIONS" | "CATEGORY_SOCIAL" | "CATEGORY_UPDATES" | "CATEGORY_FORUMS"));
    let has_attachments = payload["mimeType"].as_str() == Some("multipart/mixed")
        || payload["parts"].as_array().into_iter().flatten().any(|p| p["filename"].as_str().is_some_and(|f| !f.is_empty()));
    let m = MessageUpsert {
        remote_id: v["id"].as_str().unwrap_or_default().to_string(),
        conversation_id: v["threadId"].as_str().map(String::from),
        subject: Some(header(payload, "Subject").map(decode_words).unwrap_or_default()),
        from_name: from.as_ref().and_then(|f| f.0.clone()),
        from_address: from.map(|f| f.1),
        to_json: recipients_json(header(payload, "To")).or(Some("[]".into())),
        cc_json: recipients_json(header(payload, "Cc")).or(Some("[]".into())),
        received_at,
        preview: v["snippet"].as_str().map(unescape_snippet),
        is_read: Some(!labels.contains(&"UNREAD")),
        is_flagged: Some(labels.contains(&"STARRED")),
        has_attachments: Some(has_attachments),
        importance: Some(if labels.contains(&"IMPORTANT") { "high" } else { "normal" }.into()),
        inference: Some(if category_other { "other" } else { "focused" }.into()),
        message_type: Some("message".into()),
        meeting_message_type: None,
    };
    (role_for(&labels, snooze_label), m)
}

// ---------- bodies and attachments ----------

#[derive(Debug, Default)]
pub struct Parts {
    /// (base64url data or attachment id, charset); `true` = attachment id.
    pub html: Option<(String, bool, String)>,
    pub text: Option<(String, bool, String)>,
    /// cid -> (mime type, data or attachment id, is attachment id)
    pub inline: HashMap<String, (String, String, bool)>,
    pub attachments: Vec<mail::Attachment>,
    /// An invitation's text/calendar part.
    pub calendar: Option<(String, bool, String)>,
}

fn charset_of(part: &Value) -> String {
    header(part, "Content-Type")
        .and_then(|ct| ct.split(';').find_map(|p| p.trim().strip_prefix("charset=").map(|c| c.trim_matches('"').to_string())))
        .unwrap_or_default()
}

/// Walks a message's MIME tree.
pub fn collect_parts(payload: &Value) -> Parts {
    let mut parts = Parts::default();
    walk(payload, &mut parts);
    parts
}

fn walk(part: &Value, out: &mut Parts) {
    if let Some(children) = part["parts"].as_array() {
        for c in children {
            walk(c, out);
        }
        return;
    }
    let mime = part["mimeType"].as_str().unwrap_or("").to_ascii_lowercase();
    let filename = part["filename"].as_str().unwrap_or("");
    let body = &part["body"];
    let (content, is_id) = match (body["data"].as_str(), body["attachmentId"].as_str()) {
        (Some(d), _) => (d.to_string(), false),
        (None, Some(id)) => (id.to_string(), true),
        _ => return,
    };
    if (mime == "text/calendar" || mime == "application/ics") && out.calendar.is_none() {
        out.calendar = Some((content.clone(), is_id, charset_of(part)));
        if filename.is_empty() {
            return;
        }
    }
    let cid = header(part, "Content-ID").map(|c| c.trim().trim_matches(['<', '>']).to_string());
    let inline_disposition = header(part, "Content-Disposition").is_some_and(|d| d.trim().to_ascii_lowercase().starts_with("inline"));
    if !filename.is_empty() || (cid.is_some() && mime.starts_with("image/")) {
        if let Some(cid) = cid.filter(|_| inline_disposition || filename.is_empty() || mime.starts_with("image/")) {
            out.inline.insert(cid, (mime.clone(), content.clone(), is_id));
            if inline_disposition || filename.is_empty() {
                return;
            }
        }
        out.attachments.push(mail::Attachment {
            id: part["partId"].as_str().unwrap_or_default().to_string(),
            name: if filename.is_empty() { "attachment".into() } else { decode_words(filename) },
            content_type: mime,
            size: body["size"].as_i64().unwrap_or(0),
        });
        return;
    }
    match mime.as_str() {
        "text/html" if out.html.is_none() => out.html = Some((content, is_id, charset_of(part))),
        "text/plain" if out.text.is_none() => out.text = Some((content, is_id, charset_of(part))),
        _ => {}
    }
}

fn b64url(data: &str) -> Result<Vec<u8>> {
    let clean = data.trim_end_matches('=');
    URL_SAFE_NO_PAD.decode(clean).or_else(|_| URL_SAFE.decode(data)).map_err(|e| AppError::Other(format!("bad base64 in message: {e}")))
}

async fn attachment_bytes(state: &AppState, account_id: i64, remote_id: &str, attachment_id: &str) -> Result<Vec<u8>> {
    let v = get(state, account_id, &format!("{}/messages/{remote_id}/attachments/{attachment_id}", base())).await?;
    b64url(v["data"].as_str().unwrap_or_default())
}

pub(crate) async fn content_bytes(state: &AppState, account_id: i64, remote_id: &str, content: &str, is_id: bool) -> Result<Vec<u8>> {
    if is_id {
        attachment_bytes(state, account_id, remote_id, content).await
    } else {
        b64url(content)
    }
}

const MAX_INLINE_BYTES: usize = 5 * 1024 * 1024;

/// An invitation's iCalendar text, if the message has one.
pub async fn calendar_part(state: &AppState, account_id: i64, remote_id: &str) -> Result<Option<String>> {
    let v = get(state, account_id, &format!("{}/messages/{remote_id}?format=full", base())).await?;
    calendar_text(state, account_id, remote_id, &collect_parts(&v["payload"])).await
}

async fn calendar_text(state: &AppState, account_id: i64, remote_id: &str, parts: &Parts) -> Result<Option<String>> {
    match &parts.calendar {
        Some((content, is_id, cs)) => Ok(Some(decode_charset(&content_bytes(state, account_id, remote_id, content, *is_id).await?, cs))),
        None => Ok(None),
    }
}

/// Sanitized HTML for a message, and its invitation's iCalendar text if any (not cached
/// here; mail::message_body caches).
pub async fn fetch_body(state: &AppState, account_id: i64, remote_id: &str) -> Result<(String, Option<String>)> {
    let v = get(state, account_id, &format!("{}/messages/{remote_id}?format=full", base())).await?;
    let parts = collect_parts(&v["payload"]);
    let ics = calendar_text(state, account_id, remote_id, &parts).await.unwrap_or_else(|e| {
        log::info!("calendar part of {remote_id}: {e}");
        None
    });
    let mut html = if let Some((content, is_id, cs)) = &parts.html {
        sanitize::clean_html(&decode_charset(&content_bytes(state, account_id, remote_id, content, *is_id).await?, cs))
    } else if let Some((content, is_id, cs)) = &parts.text {
        sanitize::text_to_html(&decode_charset(&content_bytes(state, account_id, remote_id, content, *is_id).await?, cs))
    } else {
        String::new()
    };
    if html.contains("src=\"cid:") {
        let mut by_cid = HashMap::new();
        for (cid, (mime, content, is_id)) in &parts.inline {
            if !mime.starts_with("image/") || mime.contains("svg") {
                continue;
            }
            match content_bytes(state, account_id, remote_id, content, *is_id).await {
                Ok(bytes) if bytes.len() <= MAX_INLINE_BYTES => {
                    by_cid.insert(cid.clone(), format!("data:{mime};base64,{}", STANDARD.encode(bytes)));
                }
                Ok(_) => {}
                Err(e) => log::info!("inline image {cid}: {e}"),
            }
        }
        html = sanitize::rewrite_img_src(&html, |src| src.strip_prefix("cid:").and_then(|cid| by_cid.get(cid).cloned()));
    }
    Ok((html, ics))
}

/// A message resource ("full", "metadata", ...).
pub(crate) async fn get_message(state: &AppState, account_id: i64, remote_id: &str, format: &str) -> Result<Value> {
    get(state, account_id, &format!("{}/messages/{remote_id}?format={format}", base())).await
}

pub async fn list_attachments(state: &AppState, account_id: i64, remote_id: &str) -> Result<Vec<mail::Attachment>> {
    let v = get(state, account_id, &format!("{}/messages/{remote_id}?format=full", base())).await?;
    Ok(collect_parts(&v["payload"]).attachments)
}

/// An attachment's bytes. `part_id` is the MIME part id (stable, unlike Gmail's
/// attachment ids, which change between fetches).
pub async fn attachment(state: &AppState, account_id: i64, remote_id: &str, part_id: &str) -> Result<Vec<u8>> {
    let v = get(state, account_id, &format!("{}/messages/{remote_id}?format=full", base())).await?;
    fn find<'a>(p: &'a Value, id: &str) -> Option<&'a Value> {
        if p["partId"].as_str() == Some(id) {
            return Some(p);
        }
        p["parts"].as_array()?.iter().find_map(|c| find(c, id))
    }
    let part = find(&v["payload"], part_id).ok_or_else(|| AppError::Other("attachment not found".into()))?;
    match (part["body"]["data"].as_str(), part["body"]["attachmentId"].as_str()) {
        (Some(d), _) => b64url(d),
        (None, Some(id)) => attachment_bytes(state, account_id, remote_id, id).await,
        _ => Err(AppError::Other("attachment has no content".into())),
    }
}

// ---------- sync ----------

fn history_key(account_id: i64) -> String {
    format!("gmail_history:{account_id}")
}

fn snooze_label_key(account_id: i64) -> String {
    format!("gmail_snooze_label:{account_id}")
}

/// Archived, trashed and snoozed mail: everything with a folder that isn't inbox, sent or
/// spam (drafts aren't kept).
const ARCHIVE_AND_TRASH: &str = "includeSpamTrash=true&q=-in:inbox -in:sent -in:spam -in:drafts";
/// Spam (the Junk view).
const SPAM: &str = "includeSpamTrash=true&labelIds=SPAM";
/// What a full sync lists, each inside the cache window. Inbox first, so it fills in first.
const LISTINGS: &[&str] = &["labelIds=INBOX", "labelIds=SENT", ARCHIVE_AND_TRASH, SPAM];

/// Listings added after PeetMail's first Gmail sync, each with the setting that records it
/// was done. Accounts that first synced before one existed get it listed once (`backfill`),
/// then history keeps it current.
const BACKFILLS: &[(&str, &str)] = &[("gmail_all_folders", ARCHIVE_AND_TRASH), ("gmail_spam", SPAM)];

fn backfill_key(name: &str, account_id: i64) -> String {
    format!("{name}:{account_id}")
}

pub async fn sync_account(app: &Core, state: &AppState, account_id: i64) -> Result<()> {
    let start = state.db.with(|c| db::get_setting(c, &history_key(account_id)))?;
    let Some(h) = start else { return full(app, state, account_id).await };
    match incremental(app, state, account_id, &h).await {
        // Too old (Gmail keeps about a week of history): start over.
        Err(AppError::Graph { status: 404, .. }) => {
            log::warn!("gmail history {h} expired for account {account_id}, resyncing");
            return full(app, state, account_id).await;
        }
        other => other?,
    }
    for (name, listing) in BACKFILLS {
        let key = backfill_key(name, account_id);
        if state.db.with(|c| db::get_setting(c, &key))?.is_none() {
            backfill(app, state, account_id, listing, &key).await?;
        }
    }
    Ok(())
}

/// Ids of the messages one listing returns inside the cache window, newest first.
async fn list_ids(state: &AppState, account_id: i64, listing: &str, after: i64) -> Result<Vec<String>> {
    // The listing's own q (if any) and the window share one q parameter.
    let (params, q) = match listing.split_once("q=") {
        Some((p, q)) => (p.trim_end_matches('&').to_string(), format!("{q} after:{after}")),
        None => (listing.to_string(), format!("after:{after}")),
    };
    let mut ids = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut url = format!("{}/messages?{params}&q={}&maxResults=500", base(), urlencode(&q));
        if let Some(t) = &token {
            url.push_str(&format!("&pageToken={}", urlencode(t)));
        }
        let page = get(state, account_id, &url).await?;
        ids.extend(page["messages"].as_array().into_iter().flatten().filter_map(|m| m["id"].as_str().map(String::from)));
        match page["nextPageToken"].as_str() {
            Some(t) => token = Some(t.to_string()),
            None => return Ok(ids),
        }
    }
}

/// Fetches and stores messages in small chunks, in order, so lists fill in as it goes.
/// Returns how many fetches failed.
async fn fetch_and_store(app: &Core, state: &AppState, account_id: i64, ids: &[String]) -> Result<usize> {
    let mut failed = 0;
    for chunk in ids.chunks(50) {
        let fetched = fetch_many(app, account_id, chunk.to_vec()).await;
        let (changed, f) = store(state, account_id, &fetched)?;
        failed += f;
        if changed {
            changed_event(app, account_id);
        }
    }
    Ok(failed)
}

/// Lists one kind of mail once (`BACKFILLS`), for accounts synced before PeetMail kept it.
async fn backfill(app: &Core, state: &AppState, account_id: i64, listing: &str, key: &str) -> Result<()> {
    let after = (Utc::now() - Duration::days(state.cache_limits()?.cache_days)).timestamp();
    let ids = list_ids(state, account_id, listing, after).await?;
    let known: HashSet<String> = state.db.with(|c| {
        let mut stmt = c.prepare("SELECT remote_id FROM messages WHERE account_id = ?1")?;
        let rows = stmt.query_map([account_id], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    })?;
    let new: Vec<String> = ids.into_iter().filter(|id| !known.contains(id)).collect();
    let failed = fetch_and_store(app, state, account_id, &new).await?;
    if failed > 0 {
        return Err(incomplete(failed));
    }
    state.db.with(|c| db::set_setting(c, key, "1"))?;
    prune(app, state, account_id)
}

/// Fetches message resources, a few at a time. Missing messages come back as Err(404).
async fn fetch_many(app: &Core, account_id: i64, ids: Vec<String>) -> Vec<(String, Result<Value>)> {
    let mut set = tokio::task::JoinSet::new();
    let mut queue = ids.into_iter();
    let bg = in_background();
    let spawn = |set: &mut tokio::task::JoinSet<(String, Result<Value>)>, id: String| {
        let app = app.clone();
        // Spawned tasks don't inherit task-locals: carry the background flag over.
        set.spawn(BACKGROUND.scope(bg, async move {
            let state = app.state();
            let r = get(state, account_id, &format!("{}/messages/{id}?{METADATA}", base())).await;
            (id, r)
        }));
    };
    for id in queue.by_ref().take(CONCURRENCY) {
        spawn(&mut set, id);
    }
    let mut out = Vec::new();
    while let Some(done) = set.join_next().await {
        if let Ok(r) = done {
            out.push(r);
        }
        if let Some(id) = queue.next() {
            spawn(&mut set, id);
        }
    }
    out
}

/// A fetched message parsed (its folder role and row), or why its fetch failed.
type ParsedMessage<'a> = std::result::Result<(Option<&'static str>, MessageUpsert), &'a AppError>;

/// Writes fetched messages into their folders. Mail older than the window (history reports
/// label changes on old mail too) goes again in the prune that ends each sync.
/// Returns (anything changed, how many fetches failed).
fn store(state: &AppState, account_id: i64, fetched: &[(String, Result<Value>)]) -> Result<(bool, usize)> {
    let snooze = state.db.with(|c| db::get_setting(c, &snooze_label_key(account_id)))?;
    // Parse before taking the database lock: no parsing bug can hold (or poison) it.
    let parsed: Vec<(&String, ParsedMessage<'_>)> =
        fetched.iter().map(|(id, r)| (id, r.as_ref().map(|v| parse_message(v, snooze.as_deref())))).collect();
    state.db.with(|c| {
        let busy = outbox::busy_remote_ids(c, account_id)?;
        let tx = c.transaction()?;
        let mut changed = false;
        let mut failed = 0;
        for (id, r) in parsed {
            if busy.contains(id) {
                continue;
            }
            match r {
                Ok((role, m)) => {
                    let known: bool = tx
                        .query_row("SELECT 1 FROM messages WHERE account_id = ?1 AND remote_id = ?2", rusqlite::params![account_id, id], |_| Ok(()))
                        .is_ok();
                    match role {
                        None => {
                            if known {
                                tx.execute("DELETE FROM messages WHERE account_id = ?1 AND remote_id = ?2", rusqlite::params![account_id, id])?;
                                changed = true;
                            }
                        }
                        Some(role) => {
                            let folder = db::ensure_local_folder(&tx, account_id, role, folder_name(role))?;
                            db::upsert_message(&tx, account_id, folder.id, &m)?;
                            changed = true;
                        }
                    }
                }
                Err(AppError::Graph { status: 404, .. }) => {
                    tx.execute("DELETE FROM messages WHERE account_id = ?1 AND remote_id = ?2", rusqlite::params![account_id, id])?;
                    changed = true;
                }
                Err(e) => {
                    log::warn!("gmail message {id}: {e}");
                    failed += 1;
                }
            }
        }
        tx.commit()?;
        Ok((changed, failed))
    })
}

/// A sync that couldn't fetch everything isn't recorded as done, so the next one retries.
fn incomplete(failed: usize) -> AppError {
    AppError::Other(format!("couldn't fetch {failed} Gmail message(s); will retry on the next sync"))
}

fn changed_event(app: &Core, account_id: i64) {
    let _ = app.emit(mail::EVENT_MAIL_CHANGED, json!({ "accountId": account_id, "folderId": null }));
}

// ---------- server search ----------

/// Searches the whole mailbox (Gmail's `q`, from `query::gmail_search`; spam and trash are
/// left out, as in Gmail) and keeps what it finds for the open search. Returns how many
/// messages it found.
pub async fn search_server(app: &Core, state: &AppState, account_id: i64, q: &str, limit: usize, generation: u64) -> Result<usize> {
    let page = get(state, account_id, &format!("{}/messages?q={}&maxResults={limit}", base(), urlencode(q))).await?;
    let ids: Vec<String> = page["messages"].as_array().into_iter().flatten().filter_map(|m| m["id"].as_str().map(String::from)).collect();
    let fetched = fetch_many(app, account_id, ids).await;
    keep_search_results(state, account_id, &fetched, true, generation)
}

/// Fetches the rest of a conversation (Gmail thread) opened from server search results.
pub async fn fetch_conversation(state: &AppState, account_id: i64, thread_id: &str, generation: u64) -> Result<usize> {
    let thread = get(state, account_id, &format!("{}/threads/{}?{METADATA}", base(), urlencode(thread_id))).await?;
    let fetched: Vec<(String, Result<Value>)> = thread["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| Some((m["id"].as_str()?.to_string(), Ok(m.clone()))))
        .collect();
    keep_search_results(state, account_id, &fetched, false, generation)
}

/// Stores search results in the folders their labels put them in (trash and spam left out).
fn keep_search_results(state: &AppState, account_id: i64, fetched: &[(String, Result<Value>)], hit: bool, generation: u64) -> Result<usize> {
    let snooze = state.db.with(|c| db::get_setting(c, &snooze_label_key(account_id)))?;
    let parsed: Vec<_> = fetched.iter().filter_map(|(_, r)| r.as_ref().ok()).map(|v| parse_message(v, snooze.as_deref())).collect();
    state.db.with(|c| {
        // The search closed (or another started) while this was on its way.
        if !crate::server_search::still_open(generation) {
            return Ok(0);
        }
        let tx = c.transaction()?;
        let mut kept = 0;
        for (role, m) in &parsed {
            let Some(role) = role.filter(|r| !matches!(*r, "deleteditems" | db::JUNK_ROLE)) else { continue };
            let folder = db::ensure_local_folder(&tx, account_id, role, folder_name(role))?;
            db::keep_search_result(&tx, account_id, folder.id, m, hit)?;
            kept += 1;
        }
        tx.commit()?;
        Ok(kept)
    })
}

async fn full(app: &Core, state: &AppState, account_id: i64) -> Result<()> {
    // The historyId from before listing, so anything that changes meanwhile shows up next time.
    let profile = get(state, account_id, &format!("{}/profile", base())).await?;
    let history = profile["historyId"].as_str().map(String::from).ok_or_else(|| AppError::Other("Gmail profile has no historyId".into()))?;
    let days = state.cache_limits()?.cache_days;
    let after = (Utc::now() - Duration::days(days)).timestamp();

    let mut seen: Vec<String> = Vec::new();
    let mut seen_set = HashSet::new();
    for listing in LISTINGS {
        for id in list_ids(state, account_id, listing, after).await? {
            if seen_set.insert(id.clone()) {
                seen.push(id);
            }
        }
    }
    let failed = fetch_and_store(app, state, account_id, &seen).await?;
    if failed > 0 {
        return Err(incomplete(failed));
    }

    // A resync after expired history: drop local mail the server no longer has in the window
    // (snoozed mail is left to the snooze bookkeeping).
    state.db.with(|c| {
        let busy = outbox::busy_remote_ids(c, account_id)?;
        let mut stmt = c.prepare(
            "SELECT m.remote_id FROM messages m JOIN folders f ON f.id = m.folder_id
             WHERE m.account_id = ?1 AND f.role IN ('inbox', 'sentitems', 'archive', 'deleteditems', 'junkemail')
               AND m.id NOT IN (SELECT message_id FROM search_cache)",
        )?;
        let local: Vec<String> = stmt.query_map([account_id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        for id in local.iter().filter(|id| !seen_set.contains(*id) && !busy.contains(*id)) {
            c.execute("DELETE FROM messages WHERE account_id = ?1 AND remote_id = ?2", rusqlite::params![account_id, id])?;
        }
        db::set_setting(c, &history_key(account_id), &history)?;
        for (name, _) in BACKFILLS {
            db::set_setting(c, &backfill_key(name, account_id), "1")?;
        }
        Ok(())
    })?;
    prune(app, state, account_id)?;
    Ok(())
}

async fn incremental(app: &Core, state: &AppState, account_id: i64, start: &str) -> Result<()> {
    let mut changed: Vec<String> = Vec::new();
    let mut changed_set = HashSet::new();
    let mut deleted = HashSet::new();
    let mut token: Option<String> = None;
    let mut newest = start.to_string();
    loop {
        let mut url = format!("{}/history?startHistoryId={start}&maxResults=500", base());
        if let Some(t) = &token {
            url.push_str(&format!("&pageToken={}", urlencode(t)));
        }
        let page = get(state, account_id, &url).await?;
        for h in page["history"].as_array().into_iter().flatten() {
            for key in ["messagesAdded", "labelsAdded", "labelsRemoved"] {
                for e in h[key].as_array().into_iter().flatten() {
                    if let Some(id) = e["message"]["id"].as_str()
                        && changed_set.insert(id.to_string()) {
                        changed.push(id.to_string());
                    }
                }
            }
            for e in h["messagesDeleted"].as_array().into_iter().flatten() {
                if let Some(id) = e["message"]["id"].as_str() {
                    deleted.insert(id.to_string());
                }
            }
        }
        if let Some(h) = page["historyId"].as_str() {
            newest = h.to_string();
        }
        match page["nextPageToken"].as_str() {
            Some(t) => token = Some(t.to_string()),
            None => break,
        }
    }

    let mut any = false;
    if !deleted.is_empty() {
        any |= state.db.with(|c| {
            let busy = outbox::busy_remote_ids(c, account_id)?;
            let mut n = 0;
            for id in deleted.iter().filter(|id| !busy.contains(*id)) {
                n += c.execute("DELETE FROM messages WHERE account_id = ?1 AND remote_id = ?2", rusqlite::params![account_id, id])?;
            }
            Ok(n > 0)
        })?;
    }
    let to_fetch: Vec<String> = changed.into_iter().filter(|id| !deleted.contains(id)).collect();
    let mut failed = 0;
    for chunk in to_fetch.chunks(50) {
        let fetched = fetch_many(app, account_id, chunk.to_vec()).await;
        let (changed, f) = store(state, account_id, &fetched)?;
        any |= changed;
        failed += f;
    }
    if any {
        changed_event(app, account_id);
    }
    if failed > 0 {
        // Keep the old historyId: the next sync replays these changes.
        return Err(incomplete(failed));
    }
    state.db.with(|c| db::set_setting(c, &history_key(account_id), &newest))?;
    prune(app, state, account_id)?;
    Ok(())
}

fn prune(app: &Core, state: &AppState, account_id: i64) -> Result<()> {
    let days = state.cache_limits()?.cache_days;
    let cutoff = (Utc::now() - Duration::days(days)).format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let pruned = state.db.with(|c| {
        let mut n = 0;
        for f in db::list_folders(c, account_id)? {
            if f.role.as_deref() != Some("snoozed") {
                n += db::prune_older_than(c, f.id, &cutoff)?;
            }
        }
        Ok(n)
    })?;
    if pruned > 0 {
        changed_event(app, account_id);
    }
    Ok(())
}

fn urlencode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

// ---------- triage ----------

/// The hidden label snoozed mail carries, found or created on first use.
async fn snooze_label(state: &AppState, account_id: i64) -> Result<String> {
    let key = snooze_label_key(account_id);
    if let Some(id) = state.db.with(|c| db::get_setting(c, &key))? {
        return Ok(id);
    }
    let list = get(state, account_id, &format!("{}/labels", base())).await?;
    let found = list["labels"]
        .as_array()
        .and_then(|v| v.iter().find(|l| l["name"] == SNOOZE_LABEL))
        .and_then(|l| l["id"].as_str())
        .map(String::from);
    let id = match found {
        Some(id) => id,
        None => {
            let body = json!({ "name": SNOOZE_LABEL, "labelListVisibility": "labelHide", "messageListVisibility": "hide" });
            let created = call(state, account_id, Method::POST, &format!("{}/labels", base()), Some(&body)).await?;
            created["id"].as_str().map(String::from).ok_or_else(|| AppError::Other("created label has no id".into()))?
        }
    };
    state.db.with(|c| db::set_setting(c, &key, &id))?;
    Ok(id)
}

async fn modify(state: &AppState, account_id: i64, id: &str, add: &[&str], remove: &[&str]) -> Result<Value> {
    let body = json!({ "addLabelIds": add, "removeLabelIds": remove });
    call(state, account_id, Method::POST, &format!("{}/messages/{id}/modify", base()), Some(&body)).await
}

/// Sets labels on a message (moving it to Archive or Spam), first taking it out of Trash if
/// it's there: undoing a delete. Only untrash leaves Trash, and it can put the message back
/// in the inbox, so the labels are set again after.
async fn out_of_trash(state: &AppState, account_id: i64, id: &str, add: &[&str], remove: &[&str]) -> Result<Value> {
    let m = modify(state, account_id, id, add, remove).await?;
    if !m["labelIds"].as_array().is_some_and(|l| l.iter().any(|l| l == "TRASH")) {
        return Ok(m);
    }
    call(state, account_id, Method::POST, &format!("{}/messages/{id}/untrash", base()), None).await?;
    modify(state, account_id, id, add, remove).await
}

/// One outbox op against Gmail.
pub async fn execute(state: &AppState, account_id: i64, remote_id: Option<&str>, op: &Op) -> Result<Value> {
    let rid = || remote_id.ok_or_else(|| AppError::Other("op has no target message".into()));
    match op {
        Op::SetRead { value } => {
            if *value { modify(state, account_id, rid()?, &[], &["UNREAD"]).await } else { modify(state, account_id, rid()?, &["UNREAD"], &[]).await }
        }
        Op::SetFlag { value } => {
            if *value { modify(state, account_id, rid()?, &["STARRED"], &[]).await } else { modify(state, account_id, rid()?, &[], &["STARRED"]).await }
        }
        Op::Move { destination } => {
            let id = rid()?;
            match destination.as_str() {
                // SPAM comes off on the way to Archive (undoing "spam" on archived mail).
                "archive" => out_of_trash(state, account_id, id, &[], &["INBOX", "SPAM"]).await,
                // Gmail's "Report spam" is the SPAM label.
                db::JUNK_ROLE => {
                    let snoozed = state.db.with(|c| db::get_setting(c, &snooze_label_key(account_id)))?;
                    let remove: Vec<&str> = ["INBOX"].into_iter().chain(snoozed.as_deref()).collect();
                    out_of_trash(state, account_id, id, &["SPAM"], &remove).await
                }
                "deleteditems" => call(state, account_id, Method::POST, &format!("{}/messages/{id}/trash", base()), None).await,
                "inbox" => {
                    // Back from Trash (undo delete) or from a snooze or archive.
                    let _ = call(state, account_id, Method::POST, &format!("{}/messages/{id}/untrash", base()), None).await;
                    let snoozed = state.db.with(|c| db::get_setting(c, &snooze_label_key(account_id)))?;
                    // SPAM off too: "not spam".
                    let remove: Vec<&str> = ["SPAM"].into_iter().chain(snoozed.as_deref()).collect();
                    modify(state, account_id, id, &["INBOX"], &remove).await
                }
                d if d == crate::actions::SNOOZE_DEST => {
                    let label = snooze_label(state, account_id).await?;
                    modify(state, account_id, id, &[&label], &["INBOX", "SPAM"]).await
                }
                other => Err(AppError::Graph { status: 400, message: format!("can't move Gmail mail to {other}") }),
            }
        }
        Op::Send(d) => crate::gmail_send::send(state, account_id, remote_id, d).await,
        Op::Respond { .. } | Op::CreateEvent(_) | Op::UpdateEvent { .. } | Op::DeleteEvent { .. } => {
            crate::gcal::execute(state, account_id, remote_id, op).await
        }
        Op::PushDraft { .. } | Op::DeleteDraft => Err(AppError::Other("draft ops are handled by draft_sync".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippets_with_multibyte_characters_near_an_ampersand_dont_panic() {
        // From a real inbox: a zero-width non-joiner (3 bytes) straddling byte 10 after '&'.
        assert_eq!(unescape_snippet("&abcdefgh\u{200c}x"), "&abcdefgh\u{200c}x");
        assert_eq!(unescape_snippet("Sale &\u{200c}\u{200c}\u{200c}\u{200c} now"), "Sale &\u{200c}\u{200c}\u{200c}\u{200c} now");
        assert_eq!(unescape_snippet("Tom &amp;\u{200c} Jerry"), "Tom &\u{200c} Jerry");
        assert_eq!(unescape_snippet("é&é"), "é&é");
    }

    #[test]
    fn decodes_encoded_words_and_addresses() {
        assert_eq!(decode_words("=?UTF-8?B?Q2Fmw6k=?= menu"), "Café menu");
        assert_eq!(decode_words("=?ISO-8859-1?Q?Caf=E9_au_lait?="), "Café au lait");
        assert_eq!(decode_words("=?UTF-8?Q?a?= =?UTF-8?Q?b?="), "ab");
        assert_eq!(decode_words("plain =? text"), "plain =? text");
        assert_eq!(
            parse_addresses(r#""Nair, Priya" <priya@x.io>, sam@y.com, Jo Lee <jo@z.org>"#),
            vec![(Some("Nair, Priya".into()), "priya@x.io".into()), (None, "sam@y.com".into()), (Some("Jo Lee".into()), "jo@z.org".into())]
        );
        assert_eq!(unescape_snippet("Tom &amp; Jerry&#39;s &lt;3 &#x2014; ok"), "Tom & Jerry's <3 — ok");
    }

    #[test]
    fn quota_costs_follow_gmail_s_table() {
        let b = base();
        assert_eq!(cost(&Method::GET, &format!("{b}/messages/m1?format=full")), 5.0);
        assert_eq!(cost(&Method::GET, &format!("{b}/history?startHistoryId=1")), 2.0);
        assert_eq!(cost(&Method::GET, &format!("{b}/profile")), 1.0);
        assert_eq!(cost(&Method::POST, &format!("{b}/messages/send")), 100.0);
    }

    #[tokio::test]
    async fn background_spending_is_paced() {
        let t = std::time::Instant::now();
        // The burst goes at once; the next 50 units take half a second at 100/s.
        spend(-1, BUDGET_BURST).await;
        assert!(t.elapsed().as_millis() < 100);
        spend(-1, 50.0).await;
        assert!(t.elapsed().as_millis() >= 450, "waited {:?}", t.elapsed());
        assert!(!in_background());
        assert!(background(async { in_background() }).await);
    }

    #[test]
    fn labels_decide_the_folder() {
        assert_eq!(role_for(&["INBOX", "UNREAD"], None), Some("inbox"));
        assert_eq!(role_for(&["SENT"], None), Some("sentitems"));
        assert_eq!(role_for(&["INBOX", "SENT"], None), Some("inbox"));
        assert_eq!(role_for(&["CATEGORY_PERSONAL"], None), Some("archive"));
        assert_eq!(role_for(&["TRASH", "INBOX"], None), Some("deleteditems"));
        assert_eq!(role_for(&["SPAM", "INBOX"], None), Some("junkemail"));
        assert_eq!(role_for(&["SPAM", "Label_7"], Some("Label_7")), Some("junkemail"));
        assert_eq!(role_for(&["SPAM", "TRASH"], None), Some("deleteditems"));
        assert_eq!(role_for(&["DRAFT"], None), None);
        assert_eq!(role_for(&["Label_7"], Some("Label_7")), Some("snoozed"));
    }

    #[test]
    fn parses_a_metadata_message() {
        let v = json!({
            "id": "m1", "threadId": "t1", "labelIds": ["INBOX", "UNREAD", "STARRED", "CATEGORY_PROMOTIONS"],
            "snippet": "50% off &amp; more", "internalDate": "1792900800000",
            "payload": { "mimeType": "multipart/mixed", "headers": [
                { "name": "From", "value": "Shop <deals@shop.example>" },
                { "name": "To", "value": "me@gmail.com" },
                { "name": "Subject", "value": "=?UTF-8?B?U2FsZQ==?=" }
            ]}
        });
        let (role, m) = parse_message(&v, None);
        assert_eq!(role, Some("inbox"));
        assert_eq!(m.conversation_id.as_deref(), Some("t1"));
        assert_eq!(m.subject.as_deref(), Some("Sale"));
        assert_eq!(m.from_name.as_deref(), Some("Shop"));
        assert_eq!(m.from_address.as_deref(), Some("deals@shop.example"));
        assert_eq!(m.preview.as_deref(), Some("50% off & more"));
        assert_eq!(m.is_read, Some(false));
        assert_eq!(m.is_flagged, Some(true));
        assert_eq!(m.has_attachments, Some(true));
        assert_eq!(m.inference.as_deref(), Some("other"));
        assert_eq!(m.received_at.as_deref(), Some("2026-10-25T04:00:00Z"));
    }

    #[test]
    fn finds_body_inline_images_and_attachments_in_the_mime_tree() {
        let payload = json!({ "mimeType": "multipart/mixed", "parts": [
            { "partId": "0", "mimeType": "multipart/alternative", "parts": [
                { "partId": "0.0", "mimeType": "text/plain", "filename": "", "body": { "data": "aGk" } },
                { "partId": "0.1", "mimeType": "text/html", "filename": "", "headers": [{ "name": "Content-Type", "value": "text/html; charset=\"ISO-8859-1\"" }], "body": { "data": "PGI-aGk8L2I-" } }
            ]},
            { "partId": "1", "mimeType": "image/png", "filename": "logo.png", "headers": [
                { "name": "Content-ID", "value": "<logo@x>" }, { "name": "Content-Disposition", "value": "inline; filename=logo.png" }
            ], "body": { "attachmentId": "ATT1", "size": 10 } },
            { "partId": "2", "mimeType": "application/pdf", "filename": "Q4.pdf", "body": { "attachmentId": "ATT2", "size": 2048 } }
        ]});
        let p = collect_parts(&payload);
        assert_eq!(p.html, Some(("PGI-aGk8L2I-".into(), false, "ISO-8859-1".into())));
        assert_eq!(p.text.as_ref().map(|t| t.0.as_str()), Some("aGk"));
        assert_eq!(p.inline.get("logo@x").map(|i| i.1.as_str()), Some("ATT1"));
        assert_eq!(p.attachments.len(), 1);
        assert_eq!((p.attachments[0].id.as_str(), p.attachments[0].name.as_str(), p.attachments[0].size), ("2", "Q4.pdf", 2048));
        assert_eq!(b64url("PGI-aGk8L2I-").unwrap(), b"<b>hi</b>");
        assert_eq!(decode_charset(&[0x43, 0x61, 0x66, 0xE9], "ISO-8859-1"), "Café");
    }
}
