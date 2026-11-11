//! New-mail and reminder notifications (Windows toasts).
//!
//! After each sync, unread inbox mail newer than a per-account watermark is announced,
//! then the watermark moves past everything seen. The first sync only sets the watermark,
//! so signing in (or turning notifications back on) never dumps a backlog. Nothing shows
//! while PeetMail is the focused window.

use rusqlite::{params, Connection};
use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::db;
use crate::error::Result;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewMail {
    pub account_id: i64,
    pub conversation_id: String,
    pub from: String,
    pub subject: String,
    pub preview: String,
}

/// Which mail to announce: "off", "focused" (not Other) or "all".
pub fn mode_allows(mode: &str, inference: Option<&str>) -> bool {
    match mode {
        "all" => true,
        "focused" => inference != Some("other"),
        _ => false,
    }
}

/// Unread inbox mail stored since the last call, for one account, and moves the
/// watermark past it. Returns nothing on the first call (it only sets the watermark).
///
/// The watermark is the local row id, which only grows for newly stored mail: arrival
/// times come to the second, so two messages in the same second would defeat a time
/// watermark. Mail received before `not_before` is never announced, so re-downloading old
/// mail (a wider cache window, a full resync) stays quiet.
pub fn take_new_mail(conn: &Connection, account_id: i64, my_address: &str, mode: &str, not_before: &str) -> Result<Vec<NewMail>> {
    let key = format!("notified_through_id:{account_id}");
    let mark: Option<i64> = db::get_setting(conn, &key)?.and_then(|v| v.parse().ok());
    let newest: Option<i64> = conn.query_row("SELECT MAX(id) FROM messages WHERE account_id = ?1", [account_id], |r| r.get(0))?;
    let Some(newest) = newest else { return Ok(vec![]) };
    let Some(mark) = mark else {
        db::set_setting(conn, &key, &newest.to_string())?;
        return Ok(vec![]);
    };
    if newest <= mark {
        return Ok(vec![]);
    }
    let mut stmt = conn.prepare(
        "SELECT m.conversation_id, COALESCE(NULLIF(m.from_name, ''), m.from_address, ''), m.subject, m.preview, m.inference
         FROM messages m JOIN folders f ON f.id = m.folder_id
         WHERE m.account_id = ?1 AND f.role = 'inbox' AND m.id > ?2 AND m.is_read = 0
           AND lower(COALESCE(m.from_address, '')) <> lower(?3) AND m.received_at >= ?4
         ORDER BY m.received_at, m.id",
    )?;
    let rows = stmt.query_map(params![account_id, mark, my_address, not_before], |r| {
        Ok((
            NewMail { account_id, conversation_id: r.get(0)?, from: r.get(1)?, subject: r.get(2)?, preview: r.get(3)? },
            r.get::<_, Option<String>>(4)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (mail, inference) = row?;
        if mode_allows(mode, inference.as_deref()) {
            out.push(mail);
        }
    }
    db::set_setting(conn, &key, &newest.to_string())?;
    Ok(out)
}

/// One toast, and the conversation it's about, if it's about one (a tap opens it: Android).
#[derive(Debug, Clone, PartialEq)]
pub struct Toast {
    pub title: String,
    pub body: String,
    pub open: Option<(i64, String)>,
}

impl Toast {
    fn new(title: String, body: String, open: Option<(i64, String)>) -> Self {
        Self { title, body, open }
    }
}

/// Up to three messages get a toast each; more become one summary.
pub fn toasts_for(mail: &[NewMail]) -> Vec<Toast> {
    if mail.len() <= 3 {
        return mail
            .iter()
            .map(|m| {
                let subject = if m.subject.is_empty() { "(no subject)" } else { &m.subject };
                let preview: String = m.preview.chars().take(120).collect();
                let body = if preview.is_empty() { subject.to_string() } else { format!("{subject}\n{preview}") };
                Toast::new(m.from.clone(), body, Some((m.account_id, m.conversation_id.clone())))
            })
            .collect();
    }
    let mut senders: Vec<&str> = Vec::new();
    for m in mail {
        if !senders.contains(&m.from.as_str()) {
            senders.push(&m.from);
        }
    }
    let shown = senders.iter().take(3).copied().collect::<Vec<_>>().join(", ");
    let more = senders.len().saturating_sub(3);
    let body = if more > 0 { format!("From {shown} and {more} more") } else { format!("From {shown}") };
    vec![Toast::new(format!("{} new messages", mail.len()), body, None)]
}

/// "Reminder" toasts for conversations a snooze just brought back.
pub fn reminder_toasts(conn: &Connection, woke: &[(i64, String)]) -> Result<Vec<Toast>> {
    let mut out = Vec::new();
    for (account_id, conversation_id) in woke {
        let row: Option<(String, String)> = conn
            .query_row(
                "SELECT COALESCE(NULLIF(from_name, ''), from_address, ''), subject FROM messages
                 WHERE account_id = ?1 AND conversation_id = ?2 ORDER BY received_at DESC LIMIT 1",
                params![account_id, conversation_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .ok();
        if let Some((from, subject)) = row {
            out.push(Toast::new(
                format!("Reminder: {}", if subject.is_empty() { "(no subject)" } else { &subject }),
                format!("From {from}, back in your inbox"),
                Some((*account_id, conversation_id.clone())),
            ));
        }
    }
    Ok(out)
}

/// True when the user is looking at PeetMail, so a toast would only be noise.
#[cfg(desktop)]
fn app_in_front(app: &AppHandle) -> bool {
    app.get_webview_window("main")
        .is_some_and(|w| w.is_visible().unwrap_or(false) && w.is_focused().unwrap_or(false))
}

#[cfg(mobile)]
fn app_in_front(_app: &AppHandle) -> bool {
    crate::lifecycle::in_front()
}

/// Shows toasts (or, in tests, appends them to PEETMAIL_NOTIFY_LOG as JSON lines).
pub fn show(app: &AppHandle, toasts: Vec<Toast>) {
    if toasts.is_empty() || app_in_front(app) {
        return;
    }
    if let Some(path) = crate::testenv::get("PEETMAIL_NOTIFY_LOG") {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            for t in &toasts {
                let _ = writeln!(f, "{}", serde_json::json!({ "title": t.title, "body": t.body }));
            }
        }
        return;
    }
    use tauri_plugin_notification::NotificationExt;
    for t in toasts {
        let mut n = app.notification().builder().title(t.title).body(t.body);
        // PeetPlugin.kt reads these back when the notification is tapped.
        if let Some((account_id, conversation_id)) = t.open {
            n = n.extra("accountId", account_id).extra("conversationId", conversation_id);
        }
        if let Err(e) = n.show() {
            log::warn!("notification failed: {e}");
        }
    }
}

/// After a sync: announce new mail for every signed-in account.
pub fn after_sync(app: &AppHandle) {
    let state = app.state::<crate::state::AppState>();
    let mode = state.preferences().map(|p| p.notify).unwrap_or_else(|_| "focused".into());
    let not_before = (chrono::Utc::now() - chrono::Duration::days(1)).format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let found = state.db.with(|c| {
        let mut all = Vec::new();
        for a in db::list_accounts(c)?.into_iter().filter(|a| a.signed_in) {
            all.extend(take_new_mail(c, a.id, &a.email, &mode, &not_before)?);
        }
        Ok(all)
    });
    match found {
        Ok(mail) => show(app, toasts_for(&mail)),
        Err(e) => log::error!("new-mail notifications: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Db, MessageUpsert};

    #[allow(clippy::too_many_arguments)]
    fn msg(c: &Connection, a: i64, folder: i64, id: &str, at: &str, from: &str, inference: &str, read: bool) {
        db::upsert_message(c, a, folder, &MessageUpsert {
            remote_id: id.into(),
            conversation_id: Some(format!("c-{id}")),
            received_at: Some(at.into()),
            from_name: Some(from.into()),
            from_address: Some(format!("{}@x.com", from.to_lowercase())),
            subject: Some(format!("About {id}")),
            inference: Some(inference.into()),
            is_read: Some(read),
            ..Default::default()
        })
        .unwrap();
    }

    const ANY: &str = "2000-01-01T00:00:00Z";

    #[test]
    fn announces_only_new_unread_focused_mail_from_others_after_the_first_sync() {
        let db = Db::open_in_memory().unwrap();
        db.with(|c| {
            let a = db::upsert_account(c, "microsoft", "boris@x.com", None, b"t")?.id;
            let inbox = db::insert_folder(c, a, "INBOX", "inbox", "Inbox")?.id;
            let sent = db::insert_folder(c, a, "SENT", "sentitems", "Sent")?.id;
            msg(c, a, inbox, "old", "2026-10-01T10:00:00Z", "Sam", "focused", false);
            assert!(take_new_mail(c, a, "boris@x.com", "focused", ANY)?.is_empty(), "first sync sets the watermark only");

            msg(c, a, inbox, "new", "2026-10-02T10:00:00Z", "Priya", "focused", false);
            msg(c, a, inbox, "other", "2026-10-02T10:01:00Z", "Shop", "other", false);
            msg(c, a, inbox, "read", "2026-10-02T10:02:00Z", "Jo", "focused", true);
            msg(c, a, inbox, "mine", "2026-10-02T10:03:00Z", "Boris", "focused", false);
            msg(c, a, sent, "sent", "2026-10-02T10:04:00Z", "Boris", "focused", false);
            let got = take_new_mail(c, a, "boris@x.com", "focused", ANY)?;
            assert_eq!(got.iter().map(|m| m.conversation_id.as_str()).collect::<Vec<_>>(), vec!["c-new"]);
            assert_eq!(got[0].from, "Priya");
            assert!(take_new_mail(c, a, "boris@x.com", "focused", ANY)?.is_empty(), "announced once");

            msg(c, a, inbox, "other2", "2026-10-03T10:00:00Z", "Shop", "other", false);
            assert_eq!(take_new_mail(c, a, "boris@x.com", "all", ANY)?.len(), 1);
            msg(c, a, inbox, "quiet", "2026-10-04T10:00:00Z", "Sam", "focused", false);
            assert!(take_new_mail(c, a, "boris@x.com", "off", ANY)?.is_empty());
            msg(c, a, inbox, "later", "2026-10-05T10:00:00Z", "Sam", "focused", false);
            // Turning them back on doesn't replay what arrived while they were off.
            assert_eq!(take_new_mail(c, a, "boris@x.com", "focused", ANY)?.len(), 1);

            // Same second as mail already announced: still new.
            msg(c, a, inbox, "same", "2026-10-05T10:00:00Z", "Jo", "focused", false);
            assert_eq!(take_new_mail(c, a, "boris@x.com", "focused", ANY)?.len(), 1);

            // Old mail stored late (a wider cache window) stays quiet.
            msg(c, a, inbox, "ancient", "2025-01-01T10:00:00Z", "Sam", "focused", false);
            assert!(take_new_mail(c, a, "boris@x.com", "focused", "2026-10-04T00:00:00Z")?.is_empty());
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn many_messages_become_one_summary() {
        let m = |from: &str| NewMail { account_id: 1, conversation_id: "c".into(), from: from.into(), subject: "Hi".into(), preview: "Body".into() };
        assert_eq!(toasts_for(&[m("Sam")]), vec![Toast::new("Sam".into(), "Hi\nBody".into(), Some((1, "c".into())))]);
        let many = toasts_for(&[m("Sam"), m("Priya"), m("Sam"), m("Jo"), m("Ann")]);
        assert_eq!(many, vec![Toast::new("5 new messages".into(), "From Sam, Priya, Jo and 1 more".into(), None)], "a summary opens nothing in particular");
    }
}
