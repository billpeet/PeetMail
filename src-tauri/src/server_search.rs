//! Searching the server for mail outside the cache window: Graph `$search` and Gmail's `q`,
//! translated from the search query (query.rs).
//!
//! Results are stored like synced mail, in the folders they're in, so the reader, bodies,
//! attachments, triage and replies work on them unchanged. `search_cache` keeps them out
//! of the cache-window prune while the search is open and marks them as results; closing
//! the search (`clear`) drops what the search added.

use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::db;
use crate::error::{AppError, Result};
use crate::state::AppState;
use crate::{gmail, mail, query};

/// The newest matches fetched per account.
pub const LIMIT: usize = 100;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    /// Messages the server found, across accounts.
    pub found: usize,
    /// Accounts searched.
    pub searched: usize,
    /// Addresses of accounts whose search failed.
    pub failed: Vec<String>,
}

/// Searches every signed-in account the query covers (its `account:` terms, minus accounts
/// hidden in the footer). Fails only when no account could be searched at all.
pub async fn search(app: &AppHandle, state: &AppState, input: &str, hidden: &[i64]) -> Result<Outcome> {
    use crate::telemetry::{attr, span_done, Key};
    let started = std::time::Instant::now();
    // Results of an earlier search (other words) aren't results of this one.
    clear(state)?;
    let accounts: Vec<db::Account> = state
        .db
        .with(|c| db::list_accounts(c))?
        .into_iter()
        .filter(|a| a.signed_in && !hidden.contains(&a.id) && query::account_matches(input, &a.email))
        .collect();
    let mut out = Outcome { found: 0, searched: 0, failed: Vec::new() };
    for a in &accounts {
        let result = if a.provider == "google" {
            let Some(q) = query::gmail_search(input) else { continue };
            gmail::search_server(app, state, a.id, &q, LIMIT).await
        } else {
            let Some(kql) = query::graph_search(input) else { continue };
            mail::search_server(state, a.id, &kql, LIMIT).await
        };
        out.searched += 1;
        match result {
            Ok(n) => out.found += n,
            // The query's words are in the URL, which errors can carry: log the kind only.
            Err(e) => {
                log::warn!("server search failed for account {}: {}", a.id, error_kind(&e));
                out.failed.push(a.email.clone());
            }
        }
        let _ = app.emit(mail::EVENT_MAIL_CHANGED, serde_json::json!({ "accountId": a.id, "folderId": null }));
    }
    // Only the query's shape, never its words.
    let (terms, operators) = query::shape(input);
    let outcome = if out.searched == 0 {
        "unsearchable"
    } else if out.failed.len() == out.searched {
        "error"
    } else if out.failed.is_empty() {
        "ok"
    } else {
        "partial"
    };
    span_done(
        "search.server",
        started.elapsed(),
        &[attr(Key::Terms, terms), attr(Key::Operators, operators), attr(Key::Results, out.found), attr(Key::Outcome, outcome)],
    );
    match outcome {
        "unsearchable" => Err(AppError::Other("Add words, from:, to: or subject: to search the server.".into())),
        "error" => Err(AppError::Other("Couldn't reach the server. Check your connection and try again.".into())),
        _ => Ok(out),
    }
}

/// Fetches the rest of a conversation opened from server results, when the search added
/// some of it (so more may be outside the cache window). Returns whether anything came.
pub async fn fetch_conversation(state: &AppState, account_id: i64, conversation_id: &str) -> Result<bool> {
    if !state.db.with(|c| db::has_added_search_mail(c, account_id, conversation_id))? {
        return Ok(false);
    }
    let google = state.db.with(|c| db::list_accounts(c))?.iter().any(|a| a.id == account_id && a.provider == "google");
    let n = if google {
        gmail::fetch_conversation(state, account_id, conversation_id).await?
    } else {
        mail::fetch_conversation(state, account_id, conversation_id).await?
    };
    Ok(n > 0)
}

/// Closes the server search: mail it added leaves the cache.
pub fn clear(state: &AppState) -> Result<usize> {
    state.db.with(|c| db::clear_search_cache(c))
}

fn error_kind(e: &AppError) -> String {
    match e {
        AppError::Graph { status, .. } => format!("HTTP {status}"),
        AppError::Http(e) if e.is_timeout() => "timeout".into(),
        AppError::Http(_) => "network".into(),
        _ => "other".into(),
    }
}
