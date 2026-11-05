//! Conversation-level triage (archive, delete, back to the inbox, read state, flag) with undo.
//! Each action updates SQLite immediately and queues the matching outbox ops.

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::db::{self, MessageState, JUNK_ROLE};
use crate::error::Result;
use crate::outbox::{self, Op, GRACE};

/// What an action does to one message in SQLite.
type LocalChange = Box<dyn Fn(&Connection, &MessageState) -> Result<()>>;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum Action {
    /// Inbox (and snoozed) mail to Archive (Done).
    Archive,
    /// Inbox, snoozed, archived and junk mail to Deleted Items.
    Trash,
    /// Archived and deleted mail back to the inbox ("not done", restore from Trash).
    MoveToInbox,
    /// Inbox, snoozed and archived mail to Junk (Gmail: reported as spam).
    Spam,
    /// Junk back to the inbox.
    NotSpam,
    MarkRead,
    MarkUnread,
    Flag,
    Unflag,
}

impl Action {
    fn label(self) -> &'static str {
        match self {
            Action::Archive => "Archived",
            Action::Trash => "Deleted",
            Action::MoveToInbox => "Moved to Inbox",
            Action::Spam => "Marked as spam",
            Action::NotSpam => "Not spam: moved to Inbox",
            Action::MarkRead => "Marked as read",
            Action::MarkUnread => "Marked as unread",
            Action::Flag => "Flagged",
            Action::Unflag => "Unflagged",
        }
    }
}

/// What it takes to reverse one applied action.
#[derive(Debug, Clone)]
pub struct UndoEntry {
    pub label: String,
    pub account_id: i64,
    /// Local state of each touched message before the action.
    pub before: Vec<MessageState>,
    /// (outbox id, target remote id, op that reverses it on the server)
    pub ops: Vec<(i64, String, Op)>,
    /// A snooze this action set: (conversation id, the snooze it replaced, if any).
    pub snooze: Option<(String, Option<String>)>,
}

/// One undo step: a single action, or one taken on several conversations at once
/// (one `z` reverses all of it).
#[derive(Debug, Clone)]
pub struct UndoGroup {
    pub label: String,
    pub entries: Vec<UndoEntry>,
}

/// Toast and undo label for an action on `n` conversations: "Archived", "Archived 3 conversations".
pub fn batch_label(label: &str, n: usize) -> String {
    if n == 1 { label.to_string() } else { format!("{label} {n} conversations") }
}

/// Outbox destination for snoozed mail; resolved to the hidden server folder when sent.
pub const SNOOZE_DEST: &str = "peetmail:snoozed";
pub const SNOOZED_ROLE: &str = "snoozed";

fn now_iso() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

/// Snoozes a conversation until `until`: its inbox messages move to the Snoozed folder
/// now and come back (unread, on top) when it's due. Snoozing an already snoozed
/// conversation just changes the time.
pub fn snooze(conn: &mut Connection, account_id: i64, conversation_id: &str, until: chrono::DateTime<chrono::Utc>) -> Result<UndoEntry> {
    let tx = conn.transaction()?;
    let entry = snooze_in(&tx, account_id, conversation_id, until)?;
    tx.commit()?;
    Ok(entry)
}

/// Snoozes several conversations in one transaction. Returns an entry for each one that changed.
pub fn snooze_many(conn: &mut Connection, targets: &[(i64, String)], until: chrono::DateTime<chrono::Utc>) -> Result<Vec<UndoEntry>> {
    let tx = conn.transaction()?;
    let mut out = Vec::new();
    for (account_id, conversation_id) in targets {
        let entry = snooze_in(&tx, *account_id, conversation_id, until)?;
        if entry.snooze.is_some() {
            out.push(entry);
        }
    }
    tx.commit()?;
    Ok(out)
}

/// `snooze` inside the caller's transaction.
fn snooze_in(tx: &Connection, account_id: i64, conversation_id: &str, until: chrono::DateTime<chrono::Utc>) -> Result<UndoEntry> {
    let inbox = db::folder_by_role(tx, account_id, "inbox")?.map(|f| f.id);
    let snoozed = db::ensure_local_folder(tx, account_id, SNOOZED_ROLE, "Snoozed")?.id;
    let all = db::conversation_states(tx, account_id, conversation_id, None)?;
    let targets: Vec<MessageState> = all.iter().filter(|m| Some(m.folder_id) == inbox).cloned().collect();
    let previous: Option<String> = tx
        .query_row("SELECT until FROM snoozes WHERE account_id = ?1 AND conversation_id = ?2", params![account_id, conversation_id], |r| r.get(0))
        .optional()?;
    if targets.is_empty() && previous.is_none() {
        return Ok(UndoEntry { label: "Snoozed".into(), account_id, before: vec![], ops: vec![], snooze: None });
    }
    let mut ops = Vec::with_capacity(targets.len());
    for m in &targets {
        db::set_folder(tx, m.id, snoozed)?;
        let id = outbox::enqueue(tx, account_id, Some(&m.remote_id), &Op::Move { destination: SNOOZE_DEST.into() }, GRACE)?;
        ops.push((id, m.remote_id.clone(), Op::Move { destination: "inbox".into() }));
    }
    tx.execute(
        "INSERT INTO snoozes (account_id, conversation_id, until) VALUES (?1, ?2, ?3)
         ON CONFLICT (account_id, conversation_id) DO UPDATE SET until = excluded.until",
        params![account_id, conversation_id, until.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()],
    )?;
    // `before` holds the moved messages; a re-time with nothing moved still needs an entry
    // so undo can restore the old time, so count it as one change.
    Ok(UndoEntry { label: "Snoozed".into(), account_id, before: targets, ops, snooze: Some((conversation_id.into(), previous)) })
}

/// Brings snoozed conversations back to the inbox: those due (`only` None) or one now.
/// Returns the (account, conversation) pairs that came back.
pub fn wake(conn: &mut Connection, only: Option<(i64, &str)>) -> Result<Vec<(i64, String)>> {
    let now = now_iso();
    let tx = conn.transaction()?;
    let due: Vec<(i64, String)> = {
        let mut stmt = tx.prepare(
            "SELECT account_id, conversation_id FROM snoozes
             WHERE (?1 IS NULL AND until <= ?3) OR (account_id = ?1 AND conversation_id = ?2)",
        )?;
        let rows = stmt.query_map(params![only.map(|o| o.0), only.map(|o| o.1), now], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let mut accounts = Vec::new();
    for (account_id, conversation_id) in due {
        tx.execute("DELETE FROM snoozes WHERE account_id = ?1 AND conversation_id = ?2", params![account_id, conversation_id])?;
        let (Some(inbox), Some(snoozed)) = (db::folder_by_role(&tx, account_id, "inbox")?, db::folder_by_role(&tx, account_id, SNOOZED_ROLE)?) else {
            continue;
        };
        // Only what's still in Snoozed: anything moved or deleted meanwhile stays put.
        let msgs = db::conversation_states(&tx, account_id, &conversation_id, Some(snoozed.id))?;
        for m in &msgs {
            db::set_folder(&tx, m.id, inbox.id)?;
            outbox::enqueue(&tx, account_id, Some(&m.remote_id), &Op::Move { destination: "inbox".into() }, std::time::Duration::ZERO)?;
        }
        if let Some(newest) = msgs.last() {
            db::surface(&tx, newest.id, &now)?;
            if newest.is_read {
                db::set_read(&tx, newest.id, false)?;
                outbox::enqueue(&tx, account_id, Some(&newest.remote_id), &Op::SetRead { value: false }, std::time::Duration::ZERO)?;
            }
            accounts.push((account_id, conversation_id.clone()));
        }
    }
    tx.commit()?;
    Ok(accounts)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnoozedThread {
    #[serde(flatten)]
    pub thread: db::ThreadRow,
    /// When it comes back (UTC).
    pub until: String,
}

/// Snoozed conversations, soonest back first.
pub fn list_snoozed(conn: &Connection) -> Result<Vec<SnoozedThread>> {
    let mut out = Vec::new();
    let snoozes: Vec<(i64, String, String)> = {
        let mut stmt = conn.prepare("SELECT account_id, conversation_id, until FROM snoozes ORDER BY until")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let mut threads: std::collections::HashMap<(i64, String), db::ThreadRow> = Default::default();
    let mut accounts: Vec<i64> = snoozes.iter().map(|s| s.0).collect();
    accounts.dedup();
    for a in accounts {
        if let Some(f) = db::folder_by_role(conn, a, SNOOZED_ROLE)? {
            for t in db::list_threads(conn, db::Scope::Folder(f.id), &crate::query::compile(""), None, 10_000)? {
                threads.insert((t.account_id, t.conversation_id.clone()), t);
            }
        }
    }
    for (a, conv, until) in snoozes {
        if let Some(thread) = threads.remove(&(a, conv)) {
            out.push(SnoozedThread { thread, until });
        }
    }
    Ok(out)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionOutcome {
    /// Toast text, e.g. "Archived". `None` when nothing needed changing.
    pub label: Option<String>,
    pub changed: usize,
}

/// Applies `action` to a conversation inside one transaction. Returns the undo entry
/// (empty `before` means there was nothing to do).
pub fn apply(conn: &mut Connection, account_id: i64, conversation_id: &str, action: Action) -> Result<UndoEntry> {
    let tx = conn.transaction()?;
    let entry = apply_in(&tx, account_id, conversation_id, action)?;
    tx.commit()?;
    Ok(entry)
}

/// Applies `action` to several conversations in one transaction. Returns an entry for
/// each one that changed.
pub fn apply_many(conn: &mut Connection, targets: &[(i64, String)], action: Action) -> Result<Vec<UndoEntry>> {
    let tx = conn.transaction()?;
    let mut out = Vec::new();
    for (account_id, conversation_id) in targets {
        let entry = apply_in(&tx, *account_id, conversation_id, action)?;
        if !entry.before.is_empty() {
            out.push(entry);
        }
    }
    tx.commit()?;
    Ok(out)
}

/// `apply` inside the caller's transaction.
fn apply_in(tx: &Connection, account_id: i64, conversation_id: &str, action: Action) -> Result<UndoEntry> {
    let all = db::conversation_states(tx, account_id, conversation_id, None)?;

    // Moves: which folders the messages come from and go to. Each move's undo sends a message
    // back to the folder it came from (deleting from Done puts it back in Done).
    let moves: Option<(&[&str], (&str, &str))> = match action {
        // Snoozed mail too, so e and # work in the Snoozed view. Its snooze stays: undo puts
        // it back in Snoozed, and waking only moves what's still there.
        Action::Archive => Some((&["inbox", SNOOZED_ROLE], ("archive", "Archive"))),
        Action::Trash => Some((&["inbox", SNOOZED_ROLE, "archive", JUNK_ROLE], ("deleteditems", "Deleted Items"))),
        Action::MoveToInbox => Some((&["archive", "deleteditems"], ("inbox", "Inbox"))),
        Action::Spam => Some((&["inbox", SNOOZED_ROLE, "archive"], (JUNK_ROLE, "Junk Email"))),
        Action::NotSpam => Some((&[JUNK_ROLE], ("inbox", "Inbox"))),
        _ => None,
    };
    if let Some((from, (to, name))) = moves {
        let target = db::ensure_local_folder(tx, account_id, to, name)?.id;
        let mut origin = std::collections::HashMap::new();
        for role in from {
            if let Some(f) = db::folder_by_role(tx, account_id, role)? {
                origin.insert(f.id, *role);
            }
        }
        let targets: Vec<MessageState> = all.iter().filter(|m| origin.contains_key(&m.folder_id)).cloned().collect();
        let remote = Op::Move { destination: to.into() };
        let mut ops = Vec::with_capacity(targets.len());
        for m in &targets {
            db::set_folder(tx, m.id, target)?;
            let id = outbox::enqueue(tx, account_id, Some(&m.remote_id), &remote, GRACE)?;
            // Snoozed mail goes back to the snooze folder, which the outbox resolves.
            let back = match origin[&m.folder_id] {
                SNOOZED_ROLE => SNOOZE_DEST,
                role => role,
            };
            ops.push((id, m.remote_id.clone(), Op::Move { destination: back.into() }));
        }
        return Ok(UndoEntry { label: action.label().into(), account_id, before: targets, ops, snooze: None });
    }

    // Which messages the action touches, and what it does to each.
    let (targets, local, remote, inverse): (Vec<MessageState>, LocalChange, Op, Op) =
        match action {
            Action::Archive | Action::Trash | Action::MoveToInbox | Action::Spam | Action::NotSpam => unreachable!("moves are handled above"),
            Action::MarkRead => (
                all.iter().filter(|m| !m.is_read).cloned().collect(),
                Box::new(|c, m| db::set_read(c, m.id, true)),
                Op::SetRead { value: true },
                Op::SetRead { value: false },
            ),
            // Like Superhuman: only the newest message goes back to unread.
            Action::MarkUnread => (
                all.iter().rev().find(|m| m.is_read).cloned().into_iter().collect(),
                Box::new(|c, m| db::set_read(c, m.id, false)),
                Op::SetRead { value: false },
                Op::SetRead { value: true },
            ),
            Action::Flag => (
                all.last().filter(|m| !m.is_flagged).cloned().into_iter().collect(),
                Box::new(|c, m| db::set_flagged(c, m.id, true)),
                Op::SetFlag { value: true },
                Op::SetFlag { value: false },
            ),
            Action::Unflag => (
                all.iter().filter(|m| m.is_flagged).cloned().collect(),
                Box::new(|c, m| db::set_flagged(c, m.id, false)),
                Op::SetFlag { value: false },
                Op::SetFlag { value: true },
            ),
        };

    let mut ops = Vec::with_capacity(targets.len());
    for m in &targets {
        local(tx, m)?;
        let id = outbox::enqueue(tx, account_id, Some(&m.remote_id), &remote, GRACE)?;
        ops.push((id, m.remote_id.clone(), inverse.clone()));
    }
    drop(local);
    Ok(UndoEntry { label: action.label().into(), account_id, before: targets, ops, snooze: None })
}

/// Reverses an action: cancels ops still waiting in the queue and queues inverse ops
/// for any that already reached the server. (The app undoes whole steps, `undo_group`.)
#[cfg(test)]
pub fn undo(conn: &mut Connection, entry: &UndoEntry) -> Result<()> {
    let tx = conn.transaction()?;
    undo_in(&tx, entry)?;
    tx.commit()?;
    Ok(())
}

/// Reverses a whole undo step, newest change first, in one transaction.
pub fn undo_group(conn: &mut Connection, group: &UndoGroup) -> Result<()> {
    let tx = conn.transaction()?;
    for entry in group.entries.iter().rev() {
        undo_in(&tx, entry)?;
    }
    tx.commit()?;
    Ok(())
}

fn undo_in(tx: &Connection, entry: &UndoEntry) -> Result<()> {
    for m in &entry.before {
        db::restore_state(tx, m)?;
    }
    if let Some((conversation_id, previous)) = &entry.snooze {
        match previous {
            Some(until) => tx.execute(
                "UPDATE snoozes SET until = ?3 WHERE account_id = ?1 AND conversation_id = ?2",
                params![entry.account_id, conversation_id, until],
            )?,
            None => tx.execute("DELETE FROM snoozes WHERE account_id = ?1 AND conversation_id = ?2", params![entry.account_id, conversation_id])?,
        };
    }
    for (outbox_id, remote_id, inverse) in &entry.ops {
        if !outbox::cancel(tx, *outbox_id)? {
            outbox::enqueue(tx, entry.account_id, Some(remote_id), inverse, std::time::Duration::ZERO)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Db, MessageUpsert};

    fn seed() -> (Db, i64, i64) {
        let db = Db::open_in_memory().unwrap();
        let ids = db
            .with(|c| {
                let a = db::upsert_account(c, "microsoft", "b@x.com", None, b"t")?;
                let inbox = db::insert_folder(c, a.id, "INBOX", "inbox", "Inbox")?;
                for (i, read) in [(1, true), (2, false), (3, false)] {
                    db::upsert_message(c, a.id, inbox.id, &MessageUpsert {
                        remote_id: format!("m{i}"),
                        conversation_id: Some("c1".into()),
                        received_at: Some(format!("2026-10-0{i}T10:00:00Z")),
                        is_read: Some(read),
                        ..Default::default()
                    })?;
                }
                Ok((a.id, inbox.id))
            })
            .unwrap();
        (db, ids.0, ids.1)
    }

    fn queued(c: &Connection) -> Vec<(String, String)> {
        let mut s = c.prepare("SELECT remote_id, op FROM outbox ORDER BY id").unwrap();
        s.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(|r| r.unwrap()).collect()
    }

    #[test]
    fn archive_moves_locally_queues_moves_and_undo_cancels_them() {
        let (db, a, inbox) = seed();
        db.with(|c| {
            let entry = apply(c, a, "c1", Action::Archive)?;
            assert_eq!(entry.before.len(), 3);
            assert!(db::list_threads(c, db::Scope::Folder(inbox), &crate::query::compile(""), None, 10)?.is_empty());
            assert_eq!(queued(c).len(), 3);
            assert!(queued(c)[0].1.contains("archive"));

            undo(c, &entry)?;
            assert_eq!(db::list_threads(c, db::Scope::Folder(inbox), &crate::query::compile(""), None, 10)?.len(), 1);
            assert!(queued(c).is_empty(), "pending moves are cancelled, nothing sent");
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn deleting_from_done_then_restoring_puts_mail_back_where_it_was() {
        let (db, a, inbox) = seed();
        let mailbox = |c: &Connection, m: db::Mailbox| db::list_threads(c, m.scope(), &crate::query::compile(""), None, 10).unwrap().len();
        let undo_ops = |e: &UndoEntry| e.ops.iter().map(|(_, _, op)| serde_json::to_string(op).unwrap()).collect::<Vec<_>>();
        db.with(|c| {
            apply(c, a, "c1", Action::Archive)?;
            c.execute("DELETE FROM outbox", [])?; // reached the server
            assert_eq!((mailbox(c, db::Mailbox::Inbox), mailbox(c, db::Mailbox::Done)), (0, 1));

            // # in Done deletes the archived mail; its undo sends it back to Archive, not the inbox.
            let del = apply(c, a, "c1", Action::Trash)?;
            assert_eq!(del.before.len(), 3);
            assert_eq!((mailbox(c, db::Mailbox::Done), mailbox(c, db::Mailbox::Trash)), (0, 1));
            assert!(undo_ops(&del).iter().all(|op| op.contains("\"archive\"")));
            c.execute("DELETE FROM outbox", [])?;
            undo(c, &del)?;
            assert_eq!((mailbox(c, db::Mailbox::Done), mailbox(c, db::Mailbox::Trash)), (1, 0));
            assert!(queued(c).iter().all(|(_, op)| op.contains("\"archive\"")), "inverse moves queued");
            c.execute("DELETE FROM outbox", [])?;

            // e in Done: back to the inbox, as the conversation it was.
            let back = apply(c, a, "c1", Action::MoveToInbox)?;
            assert_eq!(back.label, "Moved to Inbox");
            assert_eq!((mailbox(c, db::Mailbox::Inbox), mailbox(c, db::Mailbox::Done)), (1, 0));
            assert_eq!(db::list_threads(c, db::Scope::Folder(inbox), &crate::query::compile(""), None, 10)?[0].message_count, 3);
            assert!(queued(c).iter().all(|(_, op)| op.contains("\"inbox\"")));
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn archiving_snoozed_mail_and_undoing_puts_it_back_in_snoozed() {
        let (db, a, _) = seed();
        let mailbox = |c: &Connection, m: db::Mailbox| db::list_threads(c, m.scope(), &crate::query::compile(""), None, 10).unwrap().len();
        db.with(|c| {
            snooze(c, a, "c1", chrono::Utc::now() + chrono::Duration::hours(3))?;
            c.execute("DELETE FROM outbox", [])?;
            let done = apply(c, a, "c1", Action::Archive)?;
            assert_eq!(done.before.len(), 3);
            assert_eq!((mailbox(c, db::Mailbox::Snoozed), mailbox(c, db::Mailbox::Done)), (0, 1));
            assert!(done.ops.iter().all(|(_, _, op)| serde_json::to_string(op).unwrap().contains(SNOOZE_DEST)));
            undo(c, &done)?;
            assert_eq!((mailbox(c, db::Mailbox::Snoozed), mailbox(c, db::Mailbox::Done)), (1, 0));
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn spam_goes_to_junk_and_not_spam_brings_it_back() {
        let (db, a, _) = seed();
        let mailbox = |c: &Connection, m: db::Mailbox| db::list_threads(c, m.scope(), &crate::query::compile(""), None, 10).unwrap().len();
        let undo_ops = |e: &UndoEntry| e.ops.iter().map(|(_, _, op)| serde_json::to_string(op).unwrap()).collect::<Vec<_>>();
        db.with(|c| {
            let spam = apply(c, a, "c1", Action::Spam)?;
            assert_eq!((spam.before.len(), spam.label.as_str()), (3, "Marked as spam"));
            assert_eq!((mailbox(c, db::Mailbox::Inbox), mailbox(c, db::Mailbox::Junk), mailbox(c, db::Mailbox::All)), (0, 1, 0));
            assert!(queued(c).iter().all(|(_, op)| op.contains(r#""destination":"junkemail""#)));
            // Undo while the moves wait: cancelled, back in the inbox.
            undo(c, &spam)?;
            assert_eq!((mailbox(c, db::Mailbox::Inbox), mailbox(c, db::Mailbox::Junk)), (1, 0));
            assert!(queued(c).is_empty());

            apply(c, a, "c1", Action::Spam)?;
            c.execute("DELETE FROM outbox", [])?;
            // Spam has nothing left to move; not spam is the way back.
            assert!(apply(c, a, "c1", Action::Spam)?.before.is_empty());
            let back = apply(c, a, "c1", Action::NotSpam)?;
            assert_eq!(back.before.len(), 3);
            assert_eq!((mailbox(c, db::Mailbox::Inbox), mailbox(c, db::Mailbox::Junk)), (1, 0));
            assert!(undo_ops(&back).iter().all(|op| op.contains("\"junkemail\"")), "undoing not spam returns it to Junk");
            c.execute("DELETE FROM outbox", [])?;

            // Deleting from Junk: to Trash, and its undo goes back to Junk.
            apply(c, a, "c1", Action::Spam)?;
            c.execute("DELETE FROM outbox", [])?;
            let del = apply(c, a, "c1", Action::Trash)?;
            assert_eq!((mailbox(c, db::Mailbox::Junk), mailbox(c, db::Mailbox::Trash)), (0, 1));
            assert!(undo_ops(&del).iter().all(|op| op.contains("\"junkemail\"")));
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn restoring_from_trash_moves_to_the_inbox_and_nothing_else() {
        let (db, a, _) = seed();
        db.with(|c| {
            apply(c, a, "c1", Action::Trash)?;
            c.execute("DELETE FROM outbox", [])?;
            let back = apply(c, a, "c1", Action::MoveToInbox)?;
            assert_eq!(back.before.len(), 3);
            assert_eq!(db::list_threads(c, db::Mailbox::Inbox.scope(), &crate::query::compile(""), None, 10)?.len(), 1);
            // Inbox mail has nowhere to go "back to the inbox" from.
            assert!(apply(c, a, "c1", Action::MoveToInbox)?.before.is_empty());
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn snooze_moves_out_wakes_on_top_unread_and_undo_restores() {
        let (db, a, inbox) = seed();
        let list = |c: &Connection| db::list_threads(c, db::Scope::Folder(inbox), &crate::query::compile(""), None, 10).unwrap();
        db.with(|c| {
            // A newer conversation, so c1 isn't on top by date.
            db::upsert_message(c, a, inbox, &MessageUpsert {
                remote_id: "n1".into(), conversation_id: Some("c2".into()), received_at: Some("2026-10-20T10:00:00Z".into()), is_read: Some(true), ..Default::default()
            })?;
            apply(c, a, "c1", Action::MarkRead)?;
            c.execute("DELETE FROM outbox", [])?;

            let entry = snooze(c, a, "c1", chrono::Utc::now() + chrono::Duration::hours(3))?;
            assert_eq!(list(c).iter().map(|t| t.conversation_id.as_str()).collect::<Vec<_>>(), vec!["c2"]);
            assert_eq!(queued(c).len(), 3);
            assert!(queued(c)[0].1.contains(SNOOZE_DEST));
            assert_eq!(list_snoozed(c)?.len(), 1);
            assert!(wake(c, None)?.is_empty(), "not due yet");

            undo(c, &entry)?;
            assert_eq!(list(c).len(), 2);
            assert!(list_snoozed(c)?.is_empty());
            assert!(queued(c).is_empty());

            // Due: back in the inbox, on top despite its older date, newest message unread.
            snooze(c, a, "c1", chrono::Utc::now() + chrono::Duration::hours(3))?;
            c.execute("UPDATE snoozes SET until = '2000-01-01T00:00:00.000Z'", [])?;
            c.execute("DELETE FROM outbox", [])?;
            assert_eq!(wake(c, None)?, vec![(a, "c1".to_string())]);
            let threads = list(c);
            assert_eq!(threads[0].conversation_id, "c1");
            assert!(threads[0].reminder);
            assert_eq!(threads[0].unread_count, 1);
            assert!(!threads[1].reminder);
            let q = queued(c);
            assert_eq!(q.iter().filter(|(_, op)| op.contains(r#""destination":"inbox""#)).count(), 3);
            assert!(q.iter().any(|(rid, op)| rid == "m3" && op.contains(r#""value":false"#)));
            assert!(list_snoozed(c)?.is_empty());

            // A resync of the message keeps it on top.
            db::upsert_message(c, a, inbox, &MessageUpsert {
                remote_id: "m3".into(), conversation_id: Some("c1".into()), received_at: Some("2026-10-03T10:00:00Z".into()), ..Default::default()
            })?;
            assert_eq!(list(c)[0].conversation_id, "c1");
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn undo_after_send_queues_the_inverse() {
        let (db, a, _) = seed();
        db.with(|c| {
            let entry = apply(c, a, "c1", Action::MarkRead)?;
            assert_eq!(entry.before.len(), 2, "only unread messages are touched");
            // Simulate the worker having already taken both ops.
            c.execute("UPDATE outbox SET status = 'inflight'", [])?;
            undo(c, &entry)?;
            let q = queued(c);
            assert_eq!(q.len(), 4);
            assert!(q[2].1.contains(r#""value":false"#));
            let unread: i64 = c.query_row("SELECT COUNT(*) FROM messages WHERE is_read = 0", [], |r| r.get(0))?;
            assert_eq!(unread, 2);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn mark_unread_touches_only_the_newest_read_message() {
        let (db, a, _) = seed();
        db.with(|c| {
            apply(c, a, "c1", Action::MarkRead)?;
            let entry = apply(c, a, "c1", Action::MarkUnread)?;
            assert_eq!(entry.before.iter().map(|m| m.remote_id.as_str()).collect::<Vec<_>>(), vec!["m3"]);
            Ok(())
        })
        .unwrap();
    }

    /// Seeds a second inbox conversation, c2, with one unread message.
    fn add_c2(c: &Connection, a: i64, inbox: i64) -> Result<()> {
        db::upsert_message(c, a, inbox, &MessageUpsert {
            remote_id: "n1".into(), conversation_id: Some("c2".into()), received_at: Some("2026-10-20T10:00:00Z".into()), is_read: Some(false), ..Default::default()
        })?;
        Ok(())
    }

    #[test]
    fn a_batch_acts_on_every_conversation_and_one_undo_reverses_all_of_it() {
        let (db, a, inbox) = seed();
        let inbox_count = |c: &Connection| db::list_threads(c, db::Scope::Folder(inbox), &crate::query::compile(""), None, 10).unwrap().len();
        db.with(|c| {
            add_c2(c, a, inbox)?;
            assert_eq!(inbox_count(c), 2);
            let targets = vec![(a, "c1".to_string()), (a, "c2".to_string()), (a, "missing".to_string())];
            let entries = apply_many(c, &targets, Action::Archive)?;
            assert_eq!(entries.len(), 2, "a conversation with nothing to change gets no entry");
            assert_eq!(inbox_count(c), 0);
            assert_eq!(queued(c).len(), 4);

            let group = UndoGroup { label: batch_label("Archived", entries.len()), entries };
            assert_eq!(group.label, "Archived 2 conversations");
            undo_group(c, &group)?;
            assert_eq!(inbox_count(c), 2);
            assert!(queued(c).is_empty(), "pending moves are cancelled, nothing sent");
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn a_batch_snooze_moves_each_conversation_and_undo_clears_every_snooze() {
        let (db, a, inbox) = seed();
        db.with(|c| {
            add_c2(c, a, inbox)?;
            let targets = vec![(a, "c1".to_string()), (a, "c2".to_string())];
            let entries = snooze_many(c, &targets, chrono::Utc::now() + chrono::Duration::hours(3))?;
            assert_eq!(entries.len(), 2);
            assert_eq!(list_snoozed(c)?.len(), 2);
            undo_group(c, &UndoGroup { label: "Snoozed 2 conversations".into(), entries })?;
            assert!(list_snoozed(c)?.is_empty());
            assert_eq!(db::list_threads(c, db::Scope::Folder(inbox), &crate::query::compile(""), None, 10)?.len(), 2);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn batch_labels_name_the_count() {
        assert_eq!(batch_label("Deleted", 1), "Deleted");
        assert_eq!(batch_label("Marked as read", 5), "Marked as read 5 conversations");
    }

    #[test]
    fn undoing_after_the_ops_went_out_queues_every_inverse_and_cancels_nothing_else() {
        let (db, a, inbox) = seed();
        db.with(|c| {
            add_c2(c, a, inbox)?;
            let entries = apply_many(c, &[(a, "c1".to_string()), (a, "c2".to_string())], Action::Archive)?;
            // The worker sent every move, emptying the queue...
            c.execute("DELETE FROM outbox", [])?;
            // ...then something else was queued (opening a conversation marks it read).
            let other = outbox::enqueue(c, a, Some("elsewhere"), &Op::SetRead { value: true }, GRACE)?;

            undo_group(c, &UndoGroup { label: "Archived 2 conversations".into(), entries })?;
            let q = queued(c);
            assert!(q.iter().any(|(id, _)| id == "elsewhere"), "an unrelated op must survive the undo");
            let back: Vec<&str> = q.iter().filter(|(_, op)| op.contains("inbox")).map(|(id, _)| id.as_str()).collect();
            assert_eq!(back.len(), 4, "every archived message is moved back: {q:?}");
            assert!(other > 0);
            Ok(())
        })
        .unwrap();
    }
}
