//! SQLite cache. One connection behind a mutex is enough for Phase 0:
//! every query here is an indexed lookup that takes microseconds.

use std::path::Path;
use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::error::{AppError, Result};

const MIGRATIONS: &[&str] = &[
    include_str!("../migrations/0001_init.sql"),
    include_str!("../migrations/0002_outbox_search.sql"),
    include_str!("../migrations/0003_recent_opens.sql"),
    include_str!("../migrations/0004_calendar.sql"),
    include_str!("../migrations/0005_event_series.sql"),
    include_str!("../migrations/0006_meeting_messages.sql"),
    include_str!("../migrations/0007_snooze.sql"),
    include_str!("../migrations/0008_message_invites.sql"),
    include_str!("../migrations/0009_account_app.sql"),
    include_str!("../migrations/0010_outbox_series.sql"),
    include_str!("../migrations/0011_draft_files.sql"),
    include_str!("../migrations/0012_drafts.sql"),
    include_str!("../migrations/0013_draft_sync.sql"),
    include_str!("../migrations/0014_settings_items.sql"),
    include_str!("../migrations/0015_outbox_ids.sql"),
    include_str!("../migrations/0016_search_cache.sql"),
];

pub struct Db {
    conn: Mutex<Connection>,
    /// The file, for checks that use their own connection (None in memory).
    path: Option<std::path::PathBuf>,
    /// The schema this run migrated from, if it migrated (health.rs checks the result).
    migrated_from: Option<i64>,
}

/// Pre-migration backups kept next to the database (`peetmail.db.v<schema>.bak`).
const BACKUPS_KEPT: usize = 2;

impl Db {
    /// Opens (or creates) the database and brings its schema up to date. A database from a
    /// newer build is refused rather than half-understood, and an existing database is
    /// copied aside before any migration runs, so a bad migration can be undone by hand.
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        let known = MIGRATIONS.len() as i64;
        if version > known {
            return Err(AppError::Other(format!(
                "This mailbox cache was written by a newer version of PeetMail (schema {version}; this version knows up to {known}). \
                 Install the latest PeetMail to open it."
            )));
        }
        let migrating = version > 0 && version < known;
        if migrating {
            backup_before_migrating(&conn, path, version);
        }
        let mut db = Self::init(conn)?;
        db.path = Some(path.to_path_buf());
        db.migrated_from = migrating.then_some(version);
        Ok(db)
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn migrated_from(&self) -> Option<i64> {
        self.migrated_from
    }

    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> Result<Self> {
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;
             PRAGMA temp_store = MEMORY;",
        )?;
        migrate(&mut conn)?;
        Ok(Self { conn: Mutex::new(conn), path: None, migrated_from: None })
    }

    pub fn with<T>(&self, f: impl FnOnce(&mut Connection) -> Result<T>) -> Result<T> {
        let mut conn = self.conn.lock().unwrap_or_else(|poisoned| {
            // A bug panicked while holding the connection. Its transaction rolled back as the
            // panic unwound, so the connection itself is fine: keep going rather than panicking
            // on every later call (which, inside a WebView2 callback, aborts the whole app).
            log::error!("database lock was poisoned by a panic; recovering");
            self.conn.clear_poison();
            let conn = poisoned.into_inner();
            if !conn.is_autocommit() {
                let _ = conn.execute_batch("ROLLBACK");
            }
            conn
        });
        f(&mut conn)
    }
}

/// Copies the database to `<name>.v<schema>.bak` and keeps only the newest few. `VACUUM INTO`
/// writes a consistent copy even with WAL pages not yet checkpointed. A failed backup is
/// logged, not fatal: refusing to start would be worse than migrating without one.
fn backup_before_migrating(conn: &Connection, path: &Path, version: i64) {
    let (Some(dir), Some(name)) = (path.parent(), path.file_name().and_then(|n| n.to_str())) else { return };
    let backup = dir.join(format!("{name}.v{version}.bak"));
    let _ = std::fs::remove_file(&backup);
    match conn.execute("VACUUM INTO ?1", [backup.to_string_lossy()]) {
        Ok(_) => log::info!("backed up the database (schema {version}) before migrating"),
        Err(e) => log::error!("couldn't back up the database before migrating: {e}"),
    }
    prune_backups(dir, name, BACKUPS_KEPT);
}

fn prune_backups(dir: &Path, name: &str, keep: usize) {
    let prefix = format!("{name}.v");
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut backups: Vec<(i64, std::path::PathBuf)> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let file = e.file_name().into_string().ok()?;
            let v = file.strip_prefix(&prefix)?.strip_suffix(".bak")?.parse().ok()?;
            Some((v, e.path()))
        })
        .collect();
    backups.sort_by_key(|b| std::cmp::Reverse(b.0));
    for (_, old) in backups.into_iter().skip(keep) {
        let _ = std::fs::remove_file(old);
    }
}

fn migrate(conn: &mut Connection) -> Result<()> {
    migrate_to(conn, MIGRATIONS.len())
}

fn migrate_to(conn: &mut Connection, target: usize) -> Result<()> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    for (i, sql) in MIGRATIONS.iter().enumerate().take(target).skip(version as usize) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", (i + 1) as i64)?;
        tx.commit()?;
    }
    Ok(())
}

// ---------- settings ----------

pub fn get_setting(conn: &Connection, key: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get(0))
        .optional()?)
}

pub fn set_setting(conn: &Connection, key: &str, value: &str) -> Result<()> {
    debug_assert!(crate::sync::scope(key).is_some(), "setting {key:?} isn't classified as synced or local in sync.rs");
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

pub fn delete_setting(conn: &Connection, key: &str) -> Result<()> {
    conn.execute("DELETE FROM settings WHERE key = ?1", [key])?;
    Ok(())
}

/// Every setting whose key starts with `prefix`, as (key without the prefix, value), by key.
pub fn settings_with_prefix(conn: &Connection, prefix: &str) -> Result<Vec<(String, String)>> {
    let mut stmt = conn.prepare("SELECT key, value FROM settings WHERE substr(key, 1, ?2) = ?1 ORDER BY key")?;
    let rows = stmt
        .query_map(params![prefix, prefix.chars().count() as i64], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows.into_iter().map(|(k, v)| (k[prefix.len()..].to_string(), v)).collect())
}

// ---------- models ----------

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: i64,
    pub provider: String,
    pub email: String,
    pub display_name: Option<String>,
    /// False when the account has no usable tokens, e.g. after the client ID changed.
    pub signed_in: bool,
    /// The OAuth app it signed in with: "builtin" or "custom".
    pub oauth_app: String,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Folder {
    pub id: i64,
    pub account_id: i64,
    pub remote_id: String,
    pub role: Option<String>,
    pub name: String,
    pub delta_link: Option<String>,
    pub last_synced_at: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadRow {
    pub message_id: i64,
    pub account_id: i64,
    pub conversation_id: String,
    pub subject: String,
    pub from_name: Option<String>,
    pub from_address: Option<String>,
    pub received_at: String,
    pub preview: String,
    pub message_count: i64,
    pub unread_count: i64,
    pub is_flagged: bool,
    pub has_attachments: bool,
    /// What the list sorts by: received_at, or the wake time of a snooze.
    pub sort_at: String,
    /// Back from a snooze (sorted by its wake time, not its date).
    pub reminder: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageRow {
    pub id: i64,
    pub subject: String,
    pub from_name: Option<String>,
    pub from_address: Option<String>,
    pub to: serde_json::Value,
    pub cc: serde_json::Value,
    pub received_at: String,
    pub preview: String,
    pub is_read: bool,
    pub has_attachments: bool,
    pub is_flagged: bool,
    /// Role of the folder holding this copy (inbox, sentitems, archive, ...).
    pub folder_role: String,
    /// "message", or "eventMessage" for meeting invites and responses.
    pub message_type: String,
    /// meetingRequest | meetingCancelled | meetingAccepted | meetingTenativelyAccepted | meetingDeclined
    pub meeting_message_type: Option<String>,
}

/// Fields from one Graph message. `None` means "not in this delta page",
/// so the upsert keeps the cached value.
#[derive(Debug, Default)]
pub struct MessageUpsert {
    pub remote_id: String,
    pub conversation_id: Option<String>,
    pub subject: Option<String>,
    pub from_name: Option<String>,
    pub from_address: Option<String>,
    pub to_json: Option<String>,
    pub cc_json: Option<String>,
    pub received_at: Option<String>,
    pub preview: Option<String>,
    pub is_read: Option<bool>,
    pub is_flagged: Option<bool>,
    pub has_attachments: Option<bool>,
    pub importance: Option<String>,
    pub inference: Option<String>,
    pub message_type: Option<String>,
    pub meeting_message_type: Option<String>,
}

// ---------- accounts ----------

pub fn list_accounts(conn: &Connection) -> Result<Vec<Account>> {
    let mut stmt = conn.prepare(
        "SELECT id, provider, email, display_name, token_blob IS NOT NULL, oauth_app FROM accounts ORDER BY id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(Account {
            id: r.get(0)?,
            provider: r.get(1)?,
            email: r.get(2)?,
            display_name: r.get(3)?,
            signed_in: r.get(4)?,
            oauth_app: r.get(5)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn upsert_account(
    conn: &Connection,
    provider: &str,
    email: &str,
    display_name: Option<&str>,
    token_blob: &[u8],
) -> Result<Account> {
    let (id, oauth_app): (i64, String) = conn.query_row(
        "INSERT INTO accounts (provider, email, display_name, token_blob) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (email) DO UPDATE SET display_name = excluded.display_name, token_blob = excluded.token_blob
         RETURNING id, oauth_app",
        params![provider, email, display_name, token_blob],
        |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
    )?;
    Ok(Account {
        id,
        provider: provider.into(),
        email: email.into(),
        display_name: display_name.map(Into::into),
        signed_in: true,
        oauth_app,
    })
}

/// Records which OAuth app an account signed in with ("builtin" or "custom").
pub fn set_account_app(conn: &Connection, account_id: i64, app: &str) -> Result<()> {
    conn.execute("UPDATE accounts SET oauth_app = ?2 WHERE id = ?1", params![account_id, app])?;
    Ok(())
}

/// An account's provider and OAuth app.
pub fn account_app(conn: &Connection, account_id: i64) -> Result<(String, String)> {
    Ok(conn.query_row("SELECT provider, oauth_app FROM accounts WHERE id = ?1", [account_id], |r| Ok((r.get(0)?, r.get(1)?)))?)
}

pub fn get_token_blob(conn: &Connection, account_id: i64) -> Result<Option<Vec<u8>>> {
    Ok(conn
        .query_row("SELECT token_blob FROM accounts WHERE id = ?1", [account_id], |r| r.get(0))
        .optional()?
        .flatten())
}

pub fn set_token_blob(conn: &Connection, account_id: i64, blob: &[u8]) -> Result<()> {
    conn.execute("UPDATE accounts SET token_blob = ?2 WHERE id = ?1", params![account_id, blob])?;
    Ok(())
}

/// Tokens are bound to the client ID that issued them, so changing an app's ID invalidates
/// the tokens of the accounts that signed in with it (and only those). Cached mail is kept
/// and reattached when the same mailbox signs in again.
pub fn clear_app_tokens(conn: &Connection, provider: &str, app: &str) -> Result<()> {
    conn.execute("UPDATE accounts SET token_blob = NULL WHERE provider = ?1 AND oauth_app = ?2", [provider, app])?;
    Ok(())
}

/// "microsoft" or "google".
pub fn account_provider(conn: &Connection, account_id: i64) -> Result<String> {
    Ok(conn
        .query_row("SELECT provider FROM accounts WHERE id = ?1", [account_id], |r| r.get(0))
        .optional()?
        .unwrap_or_else(|| "microsoft".into()))
}

pub fn delete_account(conn: &Connection, account_id: i64) -> Result<()> {
    conn.execute("DELETE FROM accounts WHERE id = ?1", [account_id])?;
    Ok(())
}

// ---------- folders ----------

const FOLDER_COLS: &str = "id, account_id, remote_id, role, name, delta_link, last_synced_at";

fn folder_from_row(r: &rusqlite::Row) -> rusqlite::Result<Folder> {
    Ok(Folder {
        id: r.get(0)?,
        account_id: r.get(1)?,
        remote_id: r.get(2)?,
        role: r.get(3)?,
        name: r.get(4)?,
        delta_link: r.get(5)?,
        last_synced_at: r.get(6)?,
    })
}

pub fn list_folders(conn: &Connection, account_id: i64) -> Result<Vec<Folder>> {
    let mut stmt = conn.prepare(&format!("SELECT {FOLDER_COLS} FROM folders WHERE account_id = ?1 ORDER BY id"))?;
    let rows = stmt.query_map([account_id], folder_from_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn folder_by_role(conn: &Connection, account_id: i64, role: &str) -> Result<Option<Folder>> {
    Ok(conn
        .query_row(
            &format!("SELECT {FOLDER_COLS} FROM folders WHERE account_id = ?1 AND role = ?2"),
            params![account_id, role],
            folder_from_row,
        )
        .optional()?)
}

pub fn insert_folder(conn: &Connection, account_id: i64, remote_id: &str, role: &str, name: &str) -> Result<Folder> {
    conn.execute(
        "INSERT INTO folders (account_id, remote_id, role, name) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (account_id, remote_id) DO UPDATE SET role = excluded.role, name = excluded.name",
        params![account_id, remote_id, role, name],
    )?;
    Ok(folder_by_role(conn, account_id, role)?.expect("folder just inserted"))
}

pub fn set_delta_link(conn: &Connection, folder_id: i64, delta_link: Option<&str>) -> Result<()> {
    conn.execute(
        "UPDATE folders SET delta_link = ?2, last_synced_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?1",
        params![folder_id, delta_link],
    )?;
    Ok(())
}

pub fn clear_delta_links(conn: &Connection) -> Result<()> {
    conn.execute("UPDATE folders SET delta_link = NULL", [])?;
    Ok(())
}

// ---------- messages ----------

/// Setting: the highest message id handed out before rows were deleted from the top
/// (`clear_search_cache`). New rows get ids above it, so an id that drafts, scheduled sends
/// or undo still hold never comes back as a different message.
const KEY_MESSAGE_ID_FLOOR: &str = "message_id_floor";

pub fn upsert_message(conn: &Connection, account_id: i64, folder_id: i64, m: &MessageUpsert) -> Result<()> {
    // A new row needs received_at. A partial update for a message we never cached is skipped.
    conn.execute(
        "INSERT INTO messages (id, account_id, folder_id, remote_id, conversation_id, subject, from_name, from_address,
                               to_json, cc_json, received_at, preview, is_read, is_flagged, has_attachments,
                               importance, inference, message_type, meeting_message_type, sort_at)
         SELECT MAX(COALESCE((SELECT MAX(id) FROM messages), 0),
                    COALESCE((SELECT CAST(value AS INTEGER) FROM settings WHERE key = 'message_id_floor'), 0)) + 1,
                ?1, ?2, ?3, COALESCE(?4, ?3), COALESCE(?5, ''), ?6, ?7, COALESCE(?8, '[]'), COALESCE(?9, '[]'),
                ?10, COALESCE(?11, ''), COALESCE(?12, 0), COALESCE(?13, 0), COALESCE(?14, 0), ?15, ?16,
                COALESCE(?17, 'message'), ?18, ?10
         WHERE ?10 IS NOT NULL
         ON CONFLICT (account_id, remote_id) DO UPDATE SET
           folder_id       = excluded.folder_id,
           conversation_id = COALESCE(?4,  conversation_id),
           subject         = COALESCE(?5,  subject),
           from_name       = COALESCE(?6,  from_name),
           from_address    = COALESCE(?7,  from_address),
           to_json         = COALESCE(?8,  to_json),
           cc_json         = COALESCE(?9,  cc_json),
           received_at     = COALESCE(?10, received_at),
           -- A wake time (later than the date) survives resyncs.
           sort_at         = MAX(sort_at, COALESCE(?10, sort_at)),
           preview         = COALESCE(?11, preview),
           is_read         = COALESCE(?12, is_read),
           is_flagged      = COALESCE(?13, is_flagged),
           has_attachments = COALESCE(?14, has_attachments),
           importance      = COALESCE(?15, importance),
           inference       = COALESCE(?16, inference),
           message_type    = COALESCE(?17, message_type),
           meeting_message_type = COALESCE(?18, meeting_message_type)",
        params![
            account_id, folder_id, m.remote_id, m.conversation_id, m.subject, m.from_name, m.from_address,
            m.to_json, m.cc_json, m.received_at, m.preview, m.is_read, m.is_flagged, m.has_attachments,
            m.importance, m.inference, m.message_type, m.meeting_message_type
        ],
    )?;
    if m.received_at.is_none() {
        // Partial update (delta often sends only changed props, e.g. isRead).
        conn.execute(
            "UPDATE messages SET
               conversation_id = COALESCE(?3,  conversation_id),
               subject         = COALESCE(?4,  subject),
               from_name       = COALESCE(?5,  from_name),
               from_address    = COALESCE(?6,  from_address),
               to_json         = COALESCE(?7,  to_json),
               cc_json         = COALESCE(?8,  cc_json),
               preview         = COALESCE(?9,  preview),
               is_read         = COALESCE(?10, is_read),
               is_flagged      = COALESCE(?11, is_flagged),
               has_attachments = COALESCE(?12, has_attachments),
               importance      = COALESCE(?13, importance),
               inference       = COALESCE(?14, inference)
             WHERE account_id = ?1 AND remote_id = ?2",
            params![
                account_id, m.remote_id, m.conversation_id, m.subject, m.from_name, m.from_address, m.to_json,
                m.cc_json, m.preview, m.is_read, m.is_flagged, m.has_attachments, m.importance, m.inference
            ],
        )?;
    }
    // Sync stored the whole message: an open server search no longer owns it (closing it
    // mustn't delete it). A partial update (a read flag changing) only touches the row.
    if m.received_at.is_some() {
        conn.execute(
            "UPDATE search_cache SET added = 0
             WHERE added = 1 AND message_id = (SELECT id FROM messages WHERE account_id = ?1 AND remote_id = ?2)",
            params![account_id, m.remote_id],
        )?;
    }
    Ok(())
}

/// Handles a delta `@removed` for one folder. Only drops the row if it is still in that
/// folder: a message we moved locally (archive) must survive the inbox's removal notice.
pub fn delete_message_in_folder(conn: &Connection, account_id: i64, folder_id: i64, remote_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM messages WHERE account_id = ?1 AND folder_id = ?2 AND remote_id = ?3",
        params![account_id, folder_id, remote_id],
    )?;
    Ok(())
}

/// Drops cached messages older than the configured window (not ones an open server search
/// found: see `search_cache`).
pub fn prune_older_than(conn: &Connection, folder_id: i64, cutoff_iso: &str) -> Result<usize> {
    Ok(conn.execute(
        "DELETE FROM messages WHERE folder_id = ?1 AND received_at < ?2 AND id NOT IN (SELECT message_id FROM search_cache)",
        params![folder_id, cutoff_iso],
    )?)
}

// ---------- server search ----------

/// Keeps a message the server search returned (`hit`) or that belongs to a conversation
/// opened from its results. Mail that's already cached is left as sync has it; anything
/// new goes into `folder_id` and is marked as added, so `clear_search_cache` drops it again.
pub fn keep_search_result(conn: &Connection, account_id: i64, folder_id: i64, m: &MessageUpsert, hit: bool) -> Result<()> {
    let known: Option<i64> = conn
        .query_row("SELECT id FROM messages WHERE account_id = ?1 AND remote_id = ?2", params![account_id, m.remote_id], |r| r.get(0))
        .optional()?;
    let (id, added) = match known {
        Some(id) => (id, false),
        None => {
            if m.received_at.is_none() {
                return Ok(());
            }
            upsert_message(conn, account_id, folder_id, m)?;
            let id = conn.query_row("SELECT id FROM messages WHERE account_id = ?1 AND remote_id = ?2", params![account_id, m.remote_id], |r| r.get(0))?;
            (id, true)
        }
    };
    conn.execute(
        "INSERT INTO search_cache (message_id, hit, added) VALUES (?1, ?2, ?3)
         ON CONFLICT (message_id) DO UPDATE SET hit = MAX(hit, excluded.hit)",
        params![id, hit, added],
    )?;
    Ok(())
}

/// Whether a conversation holds mail an open server search added (so the rest of it may
/// be on the server only).
pub fn has_added_search_mail(conn: &Connection, account_id: i64, conversation_id: &str) -> Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM search_cache s JOIN messages m ON m.id = s.message_id
             WHERE m.account_id = ?1 AND m.conversation_id = ?2 AND s.added = 1 LIMIT 1",
            params![account_id, conversation_id],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// Ends a server search: what it added leaves the cache again (snoozed mail stays, its
/// snooze needs it) and the rest goes back to normal pruning. Returns how many messages went.
pub fn clear_search_cache(conn: &Connection) -> Result<usize> {
    // These are usually the newest rows: keep their ids from being handed out again.
    let top: i64 = conn.query_row("SELECT COALESCE(MAX(id), 0) FROM messages", [], |r| r.get(0))?;
    let floor: i64 = get_setting(conn, KEY_MESSAGE_ID_FLOOR)?.and_then(|v| v.parse().ok()).unwrap_or(0);
    if top > floor {
        set_setting(conn, KEY_MESSAGE_ID_FLOOR, &top.to_string())?;
    }
    let n = conn.execute(
        "DELETE FROM messages WHERE id IN (SELECT message_id FROM search_cache WHERE added = 1)
           AND folder_id NOT IN (SELECT id FROM folders WHERE role = 'snoozed')",
        [],
    )?;
    conn.execute("DELETE FROM search_cache", [])?;
    Ok(n)
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cursor {
    /// The last row's `sort_at`.
    pub sort_at: String,
    pub message_id: i64,
}

/// Which messages a thread listing draws from.
pub enum Scope {
    /// One folder.
    Folder(i64),
    /// One well-known folder (`folders.role`) of every signed-in account: `Role("inbox")` is
    /// the unified inbox and its splits.
    Role(&'static str),
    /// Archived conversations with nothing left in the inbox or snoozed: a reply that lands
    /// in an archived conversation takes it out of Done and back to the inbox.
    Done,
    /// Every signed-in account's cached folders except Deleted Items (search, All mail).
    AllMail,
}

/// The mail views the sidebar lists, across accounts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Mailbox {
    Inbox,
    Done,
    Sent,
    Snoozed,
    All,
    Trash,
}

impl Mailbox {
    pub fn scope(self) -> Scope {
        match self {
            Mailbox::Inbox => Scope::Role("inbox"),
            Mailbox::Done => Scope::Done,
            Mailbox::Sent => Scope::Role("sentitems"),
            Mailbox::Snoozed => Scope::Role(crate::actions::SNOOZED_ROLE),
            Mailbox::All => Scope::AllMail,
            Mailbox::Trash => Scope::Role("deleteditems"),
        }
    }
}

/// Signed-in accounts' folders with this role, as SQL over `?` (the role).
const ROLE_FOLDERS: &str = "SELECT f.id FROM folders f JOIN accounts a ON a.id = f.account_id
                            WHERE f.role = ? AND a.token_blob IS NOT NULL";

impl Scope {
    /// SQL condition on `m` (messages), pushing any parameter it needs.
    fn sql(&self, params: &mut Vec<rusqlite::types::Value>) -> String {
        match self {
            Scope::Folder(id) => {
                params.push(rusqlite::types::Value::Integer(*id));
                "m.folder_id = ?".to_string()
            }
            Scope::Role(role) => {
                params.push(rusqlite::types::Value::Text(role.to_string()));
                format!("m.folder_id IN ({ROLE_FOLDERS})")
            }
            Scope::Done => {
                params.push(rusqlite::types::Value::Text("archive".into()));
                format!(
                    "m.folder_id IN ({ROLE_FOLDERS}) AND NOT EXISTS (
                       SELECT 1 FROM messages o JOIN folders fo ON fo.id = o.folder_id
                       WHERE o.account_id = m.account_id AND o.conversation_id = m.conversation_id
                         AND fo.role IN ('inbox', '{}'))",
                    crate::actions::SNOOZED_ROLE
                )
            }
            Scope::AllMail => "m.folder_id NOT IN (SELECT id FROM folders WHERE role = 'deleteditems')
                               AND m.account_id IN (SELECT id FROM accounts WHERE token_blob IS NOT NULL)"
                .to_string(),
        }
    }
}

/// One row per conversation (its newest matching message), newest first, keyset-paginated.
/// `filter` is a compiled query (see query.rs) over `messages m`.
pub fn list_threads(
    conn: &Connection,
    scope: Scope,
    filter: &crate::query::Filter,
    cursor: Option<&Cursor>,
    limit: i64,
) -> Result<Vec<ThreadRow>> {
    use rusqlite::types::Value as V;
    let mut params: Vec<V> = Vec::new();
    let scope_sql = scope.sql(&mut params);
    params.extend(filter.params.iter().cloned());
    let cursor_sql = match cursor {
        Some(c) => {
            params.push(V::Text(c.sort_at.clone()));
            params.push(V::Integer(c.message_id));
            "AND (sort_at, id) < (?, ?)"
        }
        None => "",
    };
    params.push(V::Integer(limit));
    let sql = format!(
        "WITH ranked AS (
           SELECT m.*,
                  ROW_NUMBER()     OVER w AS rn,
                  COUNT(*)         OVER (PARTITION BY account_id, conversation_id) AS cnt,
                  SUM(1 - is_read) OVER (PARTITION BY account_id, conversation_id) AS unread
           FROM messages m
           WHERE {scope_sql} AND ({filter})
           WINDOW w AS (PARTITION BY account_id, conversation_id ORDER BY received_at DESC, id DESC)
         )
         SELECT id, account_id, conversation_id, subject, from_name, from_address, received_at, preview,
                cnt, unread, is_flagged, has_attachments, sort_at
         FROM ranked
         WHERE rn = 1 {cursor_sql}
         ORDER BY sort_at DESC, id DESC
         LIMIT ?",
        filter = filter.sql
    );
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(params), |r| {
        Ok(ThreadRow {
            message_id: r.get(0)?,
            account_id: r.get(1)?,
            conversation_id: r.get(2)?,
            subject: r.get(3)?,
            from_name: r.get(4)?,
            from_address: r.get(5)?,
            received_at: r.get(6)?,
            preview: r.get(7)?,
            message_count: r.get(8)?,
            unread_count: r.get(9)?,
            is_flagged: r.get(10)?,
            has_attachments: r.get(11)?,
            reminder: r.get::<_, String>(12)? > r.get::<_, String>(6)?,
            sort_at: r.get(12)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Conversations (and unread conversations) in a scope matching a filter. Used for split tab counts.
pub fn count_threads(conn: &Connection, scope: Scope, filter: &crate::query::Filter) -> Result<(i64, i64)> {
    let mut params = Vec::new();
    let scope_sql = scope.sql(&mut params);
    params.extend(filter.params.iter().cloned());
    // Conversation ids are per account, so count (account, conversation) pairs.
    let sql = format!(
        "SELECT COUNT(DISTINCT account_id || ':' || conversation_id),
                COUNT(DISTINCT CASE WHEN is_read = 0 THEN account_id || ':' || conversation_id END)
         FROM messages m WHERE {scope_sql} AND ({})",
        filter.sql
    );
    Ok(conn.query_row(&sql, rusqlite::params_from_iter(params), |r| Ok((r.get(0)?, r.get(1)?)))?)
}

/// Every message of a conversation, oldest first. Includes Sent and Archive copies so the
/// reader shows the whole exchange; Deleted Items only with `with_trash` (the Trash view).
pub fn get_thread(conn: &Connection, account_id: i64, conversation_id: &str, with_trash: bool) -> Result<Vec<MessageRow>> {
    let mut stmt = conn.prepare_cached(
        "SELECT m.id, m.subject, m.from_name, m.from_address, m.to_json, m.cc_json, m.received_at, m.preview,
                m.is_read, m.has_attachments, m.is_flagged, COALESCE(f.role, ''), m.message_type, m.meeting_message_type
         FROM messages m JOIN folders f ON f.id = m.folder_id
         WHERE m.account_id = ?1 AND m.conversation_id = ?2 AND (?3 OR COALESCE(f.role, '') <> 'deleteditems')
         ORDER BY m.received_at ASC, m.id ASC",
    )?;
    let rows = stmt.query_map(params![account_id, conversation_id, with_trash], |r| {
        let to: String = r.get(4)?;
        let cc: String = r.get(5)?;
        Ok(MessageRow {
            id: r.get(0)?,
            subject: r.get(1)?,
            from_name: r.get(2)?,
            from_address: r.get(3)?,
            to: serde_json::from_str(&to).unwrap_or_default(),
            cc: serde_json::from_str(&cc).unwrap_or_default(),
            received_at: r.get(6)?,
            preview: r.get(7)?,
            is_read: r.get(8)?,
            has_attachments: r.get(9)?,
            is_flagged: r.get(10)?,
            folder_role: r.get(11)?,
            message_type: r.get(12)?,
            meeting_message_type: r.get(13)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Local state of one message, captured before an action so it can be undone.
#[derive(Debug, Clone)]
pub struct MessageState {
    pub id: i64,
    pub remote_id: String,
    pub folder_id: i64,
    pub is_read: bool,
    pub is_flagged: bool,
}

/// Messages of a conversation, optionally limited to one folder.
pub fn conversation_states(
    conn: &Connection,
    account_id: i64,
    conversation_id: &str,
    folder_id: Option<i64>,
) -> Result<Vec<MessageState>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, remote_id, folder_id, is_read, is_flagged FROM messages
         WHERE account_id = ?1 AND conversation_id = ?2 AND (?3 IS NULL OR folder_id = ?3)
         ORDER BY received_at",
    )?;
    let rows = stmt.query_map(params![account_id, conversation_id, folder_id], |r| {
        Ok(MessageState { id: r.get(0)?, remote_id: r.get(1)?, folder_id: r.get(2)?, is_read: r.get(3)?, is_flagged: r.get(4)? })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Puts a message at the top of its list (a snooze waking up).
pub fn surface(conn: &Connection, message_id: i64, at: &str) -> Result<()> {
    conn.execute("UPDATE messages SET sort_at = ?2 WHERE id = ?1", params![message_id, at])?;
    Ok(())
}

pub fn restore_state(conn: &Connection, s: &MessageState) -> Result<()> {
    conn.execute(
        "UPDATE messages SET folder_id = ?2, is_read = ?3, is_flagged = ?4 WHERE id = ?1",
        params![s.id, s.folder_id, s.is_read, s.is_flagged],
    )?;
    Ok(())
}

pub fn set_read(conn: &Connection, id: i64, value: bool) -> Result<()> {
    conn.execute("UPDATE messages SET is_read = ?2 WHERE id = ?1", params![id, value])?;
    Ok(())
}

pub fn set_flagged(conn: &Connection, id: i64, value: bool) -> Result<()> {
    conn.execute("UPDATE messages SET is_flagged = ?2 WHERE id = ?1", params![id, value])?;
    Ok(())
}

pub fn set_folder(conn: &Connection, id: i64, folder_id: i64) -> Result<()> {
    conn.execute("UPDATE messages SET folder_id = ?2 WHERE id = ?1", params![id, folder_id])?;
    Ok(())
}

/// Remote id prefix of a local placeholder folder.
pub const PLACEHOLDER_PREFIX: &str = "wk:";

/// Folder row for a well-known role, creating a local placeholder if it was never synced.
/// Placeholders use `wk:<role>` as remote id; moves address Graph by well-known name anyway.
pub fn ensure_local_folder(conn: &Connection, account_id: i64, role: &str, name: &str) -> Result<Folder> {
    if let Some(f) = folder_by_role(conn, account_id, role)? {
        return Ok(f);
    }
    insert_folder(conn, account_id, &format!("{PLACEHOLDER_PREFIX}{role}"), role, name)
}

/// Gives a placeholder folder its server id once sync finds it, keeping its row (and mail).
pub fn resolve_placeholder(conn: &Connection, folder_id: i64, remote_id: &str, name: &str) -> Result<Folder> {
    conn.execute(
        "UPDATE folders SET remote_id = ?2, name = ?3, delta_link = NULL WHERE id = ?1",
        params![folder_id, remote_id, name],
    )?;
    Ok(conn.query_row(&format!("SELECT {FOLDER_COLS} FROM folders WHERE id = ?1"), [folder_id], folder_from_row)?)
}

// ---------- recent opens ----------

const RECENT_KEEP: i64 = 200;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentOpen {
    #[serde(flatten)]
    pub thread: ThreadRow,
    pub opened_at: String,
}

/// Records that a conversation was opened (moves it to the top) and prunes old entries.
pub fn record_open(conn: &Connection, account_id: i64, conversation_id: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO recent_opens (account_id, conversation_id, opened_at)
         VALUES (?1, ?2, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
         ON CONFLICT (account_id, conversation_id) DO UPDATE SET opened_at = excluded.opened_at",
        params![account_id, conversation_id],
    )?;
    conn.execute(
        "DELETE FROM recent_opens WHERE rowid NOT IN (SELECT rowid FROM recent_opens ORDER BY opened_at DESC LIMIT ?1)",
        [RECENT_KEEP],
    )?;
    Ok(())
}

/// Recently opened conversations, newest open first, each with its newest message
/// (any folder except Deleted Items, so archived ones still show).
pub fn recent_opens(conn: &Connection, limit: i64) -> Result<Vec<RecentOpen>> {
    let mut stmt = conn.prepare_cached(
        "WITH latest AS (
           SELECT m.*,
                  ROW_NUMBER()     OVER w AS rn,
                  COUNT(*)         OVER c AS cnt,
                  SUM(1 - is_read) OVER c AS unread
           FROM messages m JOIN folders f ON f.id = m.folder_id
           WHERE COALESCE(f.role, '') <> 'deleteditems'
             AND (m.account_id, m.conversation_id) IN (SELECT account_id, conversation_id FROM recent_opens)
           WINDOW c AS (PARTITION BY m.account_id, m.conversation_id),
                  w AS (PARTITION BY m.account_id, m.conversation_id ORDER BY m.received_at DESC, m.id DESC)
         )
         SELECT l.id, l.account_id, l.conversation_id, l.subject, l.from_name, l.from_address, l.received_at,
                l.preview, l.cnt, l.unread, l.is_flagged, l.has_attachments, r.opened_at, l.sort_at
         FROM recent_opens r
         JOIN latest l ON l.account_id = r.account_id AND l.conversation_id = r.conversation_id AND l.rn = 1
         ORDER BY r.opened_at DESC
         LIMIT ?1",
    )?;
    let rows = stmt.query_map([limit], |r| {
        Ok(RecentOpen {
            thread: ThreadRow {
                message_id: r.get(0)?,
                account_id: r.get(1)?,
                conversation_id: r.get(2)?,
                subject: r.get(3)?,
                from_name: r.get(4)?,
                from_address: r.get(5)?,
                received_at: r.get(6)?,
                preview: r.get(7)?,
                message_count: r.get(8)?,
                unread_count: r.get(9)?,
                is_flagged: r.get(10)?,
                has_attachments: r.get(11)?,
                reminder: r.get::<_, String>(13)? > r.get::<_, String>(6)?,
                sort_at: r.get(13)?,
            },
            opened_at: r.get(12)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

// ---------- contacts ----------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Contact {
    pub name: Option<String>,
    pub address: String,
}

/// People you've exchanged mail with, best match first: senders plus recipients of mail you sent.
pub fn suggest_contacts(conn: &Connection, prefix: &str, limit: i64) -> Result<Vec<Contact>> {
    let like = format!("%{}%", prefix.to_lowercase());
    let mut stmt = conn.prepare_cached(
        "WITH people AS (
           SELECT from_name AS name, lower(from_address) AS address FROM messages WHERE from_address IS NOT NULL
           UNION ALL
           SELECT json_extract(j.value, '$.name'), lower(json_extract(j.value, '$.address'))
           FROM messages m JOIN folders f ON f.id = m.folder_id, json_each(m.to_json) j
           WHERE f.role = 'sentitems'
         )
         SELECT MAX(name), address, COUNT(*) AS n FROM people
         WHERE address IS NOT NULL AND (address LIKE ?1 OR lower(name) LIKE ?1)
         GROUP BY address ORDER BY n DESC LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![like, limit], |r| Ok(Contact { name: r.get(0)?, address: r.get(1)? }))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// (account_id, remote_id) for a cached message.
/// Whether any message of this conversation is cached.
pub fn has_conversation(conn: &Connection, account_id: i64, conversation_id: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM messages WHERE account_id = ?1 AND conversation_id = ?2)",
        params![account_id, conversation_id],
        |r| r.get(0),
    )?)
}

pub fn message_remote(conn: &Connection, message_id: i64) -> Result<Option<(i64, String)>> {
    Ok(conn
        .query_row("SELECT account_id, remote_id FROM messages WHERE id = ?1", [message_id], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .optional()?)
}

// ---------- body cache ----------

pub fn get_body(conn: &Connection, message_id: i64) -> Result<Option<String>> {
    let html: Option<String> = conn
        .query_row("SELECT html FROM message_bodies WHERE message_id = ?1", [message_id], |r| r.get(0))
        .optional()?;
    if html.is_some() {
        conn.execute(
            "UPDATE message_bodies SET accessed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE message_id = ?1",
            [message_id],
        )?;
    }
    Ok(html)
}

/// Stores a sanitized body and indexes its text for search.
pub fn put_body(conn: &Connection, message_id: i64, html: &str, text: &str) -> Result<()> {
    conn.execute("UPDATE messages_fts SET body = ?2 WHERE rowid = ?1", params![message_id, text])?;
    conn.execute(
        "INSERT INTO message_bodies (message_id, html, size, fetched_at, accessed_at)
         VALUES (?1, ?2, ?3, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
         ON CONFLICT (message_id) DO UPDATE SET html = excluded.html, size = excluded.size,
           fetched_at = excluded.fetched_at, accessed_at = excluded.accessed_at",
        params![message_id, html, html.len() as i64],
    )?;
    Ok(())
}

/// Inbox messages whose body isn't cached yet: the newest `newest` plus any unread,
/// newest first, at most `limit`.
pub fn bodies_to_prefetch(conn: &Connection, newest: i64, limit: i64) -> Result<Vec<i64>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id FROM (
           SELECT m.id, m.received_at, m.is_read,
                  ROW_NUMBER() OVER (ORDER BY m.received_at DESC, m.id DESC) AS rank
           FROM messages m JOIN folders f ON f.id = m.folder_id
           WHERE f.role = 'inbox'
         )
         WHERE (rank <= ?1 OR is_read = 0) AND id NOT IN (SELECT message_id FROM message_bodies)
         ORDER BY received_at DESC
         LIMIT ?2",
    )?;
    let ids = stmt.query_map(params![newest, limit], |r| r.get(0))?;
    Ok(ids.collect::<rusqlite::Result<_>>()?)
}

/// Evicts least-recently-read bodies until the cache fits in `max_bytes`.
pub fn evict_bodies(conn: &Connection, max_bytes: i64) -> Result<usize> {
    let total: i64 = conn.query_row("SELECT COALESCE(SUM(size), 0) FROM message_bodies", [], |r| r.get(0))?;
    if total <= max_bytes {
        return Ok(0);
    }
    Ok(conn.execute(
        "DELETE FROM message_bodies WHERE message_id IN (
           SELECT message_id FROM (
             SELECT message_id, size, SUM(size) OVER (ORDER BY accessed_at, message_id) AS running
             FROM message_bodies
           ) WHERE running - size < ?1
         )",
        [total - max_bytes],
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh directory under the OS temp dir, removed on drop.
    struct TempDir(std::path::PathBuf);
    impl TempDir {
        fn new(tag: &str) -> Self {
            let mut bytes = [0u8; 8];
            getrandom::fill(&mut bytes).unwrap();
            let dir = std::env::temp_dir().join(format!("peetmail-db-{tag}-{:016x}", u64::from_le_bytes(bytes)));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn user_version(path: &Path) -> i64 {
        Connection::open(path).unwrap().query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap()
    }

    fn backups(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .filter(|n| n.ends_with(".bak"))
            .collect();
        names.sort();
        names
    }

    #[test]
    fn existing_accounts_keep_the_app_they_signed_in_with() {
        let mut c = Connection::open_in_memory().unwrap();
        migrate_to(&mut c, 8).unwrap();
        // A typed-in Microsoft client ID took precedence over the built-in one; Google had none.
        set_setting(&c, "ms_client_id", "aaaaaaaa-0000-0000-0000-000000000000").unwrap();
        set_setting(&c, "google_client_id", "  ").unwrap();
        for (provider, email) in [("microsoft", "a@work.example"), ("google", "b@gmail.com")] {
            c.execute("INSERT INTO accounts (provider, email) VALUES (?1, ?2)", [provider, email]).unwrap();
        }
        migrate(&mut c).unwrap();
        let apps: Vec<(String, String)> = list_accounts(&c).unwrap().into_iter().map(|a| (a.provider, a.oauth_app)).collect();
        assert_eq!(apps, [("microsoft".into(), "custom".into()), ("google".into(), "builtin".into())]);
    }

    #[test]
    fn drafts_saved_before_draft_sync_survive_its_migration_and_still_go_up() {
        let mut c = Connection::open_in_memory().unwrap();
        // A database from the build with local drafts only (schema 12), holding a draft.
        migrate_to(&mut c, 12).unwrap();
        c.execute("INSERT INTO accounts (provider, email) VALUES ('microsoft', 'a@work.example')", []).unwrap();
        c.execute("INSERT INTO drafts (account_id, data, seq) VALUES (1, '{\"subject\":\"Old\"}', 1)", []).unwrap();
        migrate(&mut c).unwrap();
        let row = crate::drafts::get(&c, 1).unwrap().expect("the draft is still there");
        assert_eq!(row.doc.subject, "Old");
        assert!(row.dirty, "never pushed, so it's queued to go up");
        assert_eq!(crate::drafts::unpushed(&c).unwrap(), vec![(1, 1)]);
        // And it saves with the new columns.
        crate::drafts::save(&c, Some(1), 1, &crate::drafts::DraftDoc { subject: "New".into(), ..Default::default() }).unwrap();
    }

    /// A database from before item-shaped settings (schema 13), with `old` as raw settings
    /// (written directly: those keys are no longer valid for set_setting), migrated.
    fn migrated_from_13(old: &[(&str, &str)]) -> Connection {
        let mut c = Connection::open_in_memory().unwrap();
        migrate_to(&mut c, 13).unwrap();
        for (k, v) in old {
            c.execute("INSERT INTO settings (key, value) VALUES (?1, ?2)", [k, v]).unwrap();
        }
        migrate(&mut c).unwrap();
        c
    }

    fn json_settings(c: &Connection, prefix: &str) -> Vec<(String, serde_json::Value)> {
        settings_with_prefix(c, prefix).unwrap().into_iter().map(|(k, v)| (k, serde_json::from_str(&v).unwrap())).collect()
    }

    #[derive(Debug, PartialEq, serde::Deserialize)]
    struct NameQuery {
        name: String,
        query: String,
    }

    #[test]
    fn splits_become_one_setting_each_in_their_order() {
        let c = migrated_from_13(&[(
            "splits",
            r#"[{"name":"Important","query":"is:focused"},{"name":"Boss","query":"from:boss@contoso.com"},{"name":"Other","query":"is:other"},{"name":7},"junk"]"#,
        )]);
        let splits: Vec<NameQuery> = crate::items::load_ordered(&c, "split:").unwrap().into_iter().map(|(_, s)| s).collect();
        // The old Important default goes (it would claim nearly all mail); bad entries are skipped.
        assert_eq!(
            splits,
            [
                NameQuery { name: "Boss".into(), query: "from:boss@contoso.com".into() },
                NameQuery { name: "Other".into(), query: "is:other".into() }
            ]
        );
        assert_eq!(get_setting(&c, "splits").unwrap(), None);
        assert_eq!(get_setting(&c, "split:other").unwrap(), None, "the fixed id is only for the default");
    }

    #[test]
    fn an_important_split_made_after_the_built_in_inbox_stays() {
        let c = migrated_from_13(&[("splits", r#"[{"name":"Important","query":"is:focused"}]"#), ("splits_builtin_inbox", "1")]);
        let splits: Vec<(String, NameQuery)> = crate::items::load_ordered(&c, "split:").unwrap();
        assert_eq!(splits.len(), 1);
        assert_eq!(splits[0].1.name, "Important");
        assert_eq!(get_setting(&c, "splits_builtin_inbox").unwrap(), None);
    }

    #[test]
    fn new_and_unreadable_split_lists_get_the_default_other_tab() {
        for old in [&[][..], &[("splits", "not json")][..], &[("splits", r#"{"name":"x"}"#)][..]] {
            let c = migrated_from_13(old);
            let splits: Vec<(String, NameQuery)> = crate::items::load_ordered(&c, "split:").unwrap();
            assert_eq!(splits, [("other".to_string(), NameQuery { name: "Other".into(), query: "is:other".into() })], "{old:?}");
        }
        // An empty list stays empty.
        let c = migrated_from_13(&[("splits", "[]")]);
        assert!(settings_with_prefix(&c, "split:").unwrap().is_empty());
        // And so does a fresh database's: Other comes from the migration, not on every load.
        let c = Db::open_in_memory().unwrap();
        assert_eq!(c.with(|c| Ok(settings_with_prefix(c, "split:")?.len())).unwrap(), 1);
    }

    #[test]
    fn snippets_and_signatures_become_one_setting_each() {
        let c = migrated_from_13(&[
            ("snippets", r#"[{"name":"Thanks","body":"Thanks {first_name}!"},{"name":"intro","body":"Hi"},{"name":"","body":"x"},{"body":"no name"}]"#),
            (
                "signatures",
                r#"[{"address":"priya@fabrikam.io","html":"<p>Priya</p>","replies":true},{"address":"it@contoso.com","html":"","managed":true},{"html":"no address"}]"#,
            ),
        ]);
        assert_eq!(
            json_settings(&c, "snippet:"),
            [
                ("intro".to_string(), serde_json::json!({ "name": "intro", "body": "Hi", "order": 2 })),
                ("thanks".to_string(), serde_json::json!({ "name": "Thanks", "body": "Thanks {first_name}!", "order": 1 })),
            ]
        );
        assert_eq!(
            json_settings(&c, "signature:"),
            [
                ("it@contoso.com".to_string(), serde_json::json!({ "html": "", "replies": false, "managed": true })),
                ("priya@fabrikam.io".to_string(), serde_json::json!({ "html": "<p>Priya</p>", "replies": true, "managed": false })),
            ]
        );
        assert_eq!(get_setting(&c, "snippets").unwrap(), None);
        assert_eq!(get_setting(&c, "signatures").unwrap(), None);
    }

    #[test]
    fn unreadable_snippets_and_signatures_dont_stop_the_migration() {
        let c = migrated_from_13(&[("snippets", "{oops"), ("signatures", "42"), ("layout", "list")]);
        assert!(settings_with_prefix(&c, "snippet:").unwrap().is_empty());
        assert!(settings_with_prefix(&c, "signature:").unwrap().is_empty());
        assert_eq!(get_setting(&c, "layout").unwrap().as_deref(), Some("list"), "other settings are untouched");
    }

    #[test]
    fn changing_an_app_signs_out_only_its_accounts() {
        let db = Db::open_in_memory().unwrap();
        db.with(|c| {
            let builtin = upsert_account(c, "microsoft", "a@work.example", None, b"t")?.id;
            let custom = upsert_account(c, "microsoft", "b@work.example", None, b"t")?.id;
            set_account_app(c, custom, "custom")?;
            assert_eq!(account_app(c, custom)?, ("microsoft".into(), "custom".into()));

            clear_app_tokens(c, "microsoft", "custom")?;
            let signed_in: Vec<(i64, bool)> = list_accounts(c)?.into_iter().map(|a| (a.id, a.signed_in)).collect();
            assert_eq!(signed_in, [(builtin, true), (custom, false)]);

            // Signing in again keeps the row; the caller records the app it used this time.
            let again = upsert_account(c, "microsoft", "b@work.example", None, b"u")?;
            assert_eq!((again.id, again.oauth_app.as_str()), (custom, "custom"));
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn a_database_from_a_newer_build_is_refused_untouched() {
        let tmp = TempDir::new("newer");
        let path = tmp.0.join("peetmail.db");
        let newer = MIGRATIONS.len() as i64 + 1;
        Connection::open(&path).unwrap().pragma_update(None, "user_version", newer).unwrap();

        let err = Db::open(&path).err().expect("a newer schema must not open").to_string();
        assert!(err.contains("newer version of PeetMail"), "{err}");
        assert_eq!(user_version(&path), newer);
        assert!(backups(&tmp.0).is_empty());
    }

    #[test]
    fn an_older_database_is_backed_up_before_migrating() {
        let tmp = TempDir::new("upgrade");
        let path = tmp.0.join("peetmail.db");
        {
            let mut c = Connection::open(&path).unwrap();
            migrate_to(&mut c, 3).unwrap();
            set_setting(&c, "cache_days", "before").unwrap();
        }

        let db = Db::open(&path).unwrap();
        assert_eq!(db.migrated_from(), Some(3), "the health check looks at migrated databases");
        drop(db);

        assert_eq!(user_version(&path), MIGRATIONS.len() as i64);
        assert_eq!(backups(&tmp.0), ["peetmail.db.v3.bak"]);
        let backup = Connection::open(tmp.0.join("peetmail.db.v3.bak")).unwrap();
        assert_eq!(backup.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0)).unwrap(), 3);
        assert_eq!(get_setting(&backup, "cache_days").unwrap().as_deref(), Some("before"));
    }

    #[test]
    fn outbox_ids_survive_the_migration_and_are_never_reused() {
        let mut c = Connection::open_in_memory().unwrap();
        migrate_to(&mut c, 14).unwrap();
        let a = upsert_account(&c, "microsoft", "b@x.com", None, b"t").unwrap().id;
        c.execute("INSERT INTO outbox (id, account_id, remote_id, op) VALUES (7, ?1, 'm1', '{}')", [a]).unwrap();
        migrate(&mut c).unwrap();
        let kept: (i64, String) = c.query_row("SELECT id, remote_id FROM outbox", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!(kept, (7, "m1".into()));

        // Emptied, the queue carries on from the highest id it has used.
        c.execute("DELETE FROM outbox", []).unwrap();
        let next = crate::outbox::enqueue(&c, a, Some("m2"), &crate::outbox::Op::SetRead { value: true }, std::time::Duration::ZERO).unwrap();
        assert_eq!(next, 8);
    }

    #[test]
    fn new_and_current_databases_are_not_backed_up() {
        let tmp = TempDir::new("current");
        let path = tmp.0.join("peetmail.db");
        assert_eq!(Db::open(&path).unwrap().migrated_from(), None, "a new database isn't a migration");
        assert_eq!(Db::open(&path).unwrap().migrated_from(), None);
        assert!(backups(&tmp.0).is_empty());
    }

    #[test]
    fn only_the_newest_backups_are_kept() {
        let tmp = TempDir::new("prune");
        for v in [1, 2, 5, 7] {
            std::fs::write(tmp.0.join(format!("peetmail.db.v{v}.bak")), b"x").unwrap();
        }
        std::fs::write(tmp.0.join("other.db.v1.bak"), b"x").unwrap();
        prune_backups(&tmp.0, "peetmail.db", 2);
        assert_eq!(backups(&tmp.0), ["other.db.v1.bak", "peetmail.db.v5.bak", "peetmail.db.v7.bak"]);
    }

    fn seed() -> (Db, i64, i64) {
        let db = Db::open_in_memory().unwrap();
        let (a, f) = db
            .with(|c| {
                let a = upsert_account(c, "microsoft", "boris@example.com", Some("Boris"), b"x")?;
                let f = insert_folder(c, a.id, "inbox-remote", "inbox", "Inbox")?;
                Ok((a.id, f.id))
            })
            .unwrap();
        (db, a, f)
    }

    fn msg(id: &str, conv: &str, at: &str, read: bool) -> MessageUpsert {
        MessageUpsert {
            remote_id: id.into(),
            conversation_id: Some(conv.into()),
            subject: Some(format!("subject {id}")),
            received_at: Some(at.into()),
            is_read: Some(read),
            ..Default::default()
        }
    }

    #[test]
    fn threads_group_by_conversation_newest_first() {
        let (db, a, f) = seed();
        db.with(|c| {
            upsert_message(c, a, f, &msg("m1", "c1", "2026-10-01T10:00:00Z", true))?;
            upsert_message(c, a, f, &msg("m2", "c1", "2026-10-03T10:00:00Z", false))?;
            upsert_message(c, a, f, &msg("m3", "c2", "2026-10-02T10:00:00Z", false))?;
            let t = list_threads(c, Scope::Folder(f), &crate::query::compile(""), None, 10)?;
            assert_eq!(t.len(), 2);
            assert_eq!(t[0].conversation_id, "c1");
            assert_eq!(t[0].subject, "subject m2");
            assert_eq!(t[0].message_count, 2);
            assert_eq!(t[0].unread_count, 1);
            assert_eq!(t[1].conversation_id, "c2");

            let cur = Cursor { sort_at: t[0].sort_at.clone(), message_id: t[0].message_id };
            let page2 = list_threads(c, Scope::Folder(f), &crate::query::compile(""), Some(&cur), 10)?;
            assert_eq!(page2.len(), 1);
            assert_eq!(page2[0].conversation_id, "c2");
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn server_search_results_survive_the_prune_until_the_search_closes() {
        let (db, a, f) = seed();
        db.with(|c| {
            // m1 is cached; the server search finds it and m2 (older than the window), and
            // opening m2's conversation brings m3.
            upsert_message(c, a, f, &msg("m1", "c1", "2026-10-01T10:00:00Z", true))?;
            keep_search_result(c, a, f, &msg("m1", "c1", "2026-10-01T10:00:00Z", true), true)?;
            keep_search_result(c, a, f, &msg("m2", "c2", "2025-01-01T10:00:00Z", true), true)?;
            keep_search_result(c, a, f, &msg("m3", "c2", "2025-01-02T10:00:00Z", true), false)?;
            assert!(has_added_search_mail(c, a, "c2")?);
            assert!(!has_added_search_mail(c, a, "c1")?, "c1 was all cached already");

            // Sync's prune leaves them alone while the search is open.
            assert_eq!(prune_older_than(c, f, "2026-09-01T00:00:00Z")?, 0);

            // Results are the hits (local matches or not), not the rest of their conversation.
            let hits = crate::query::with_server_hits(crate::query::compile("nomatch"), "nomatch");
            let rows = list_threads(c, Scope::AllMail, &hits, None, 10)?;
            let convs: Vec<&str> = rows.iter().map(|t| t.conversation_id.as_str()).collect();
            assert_eq!(convs, ["c1", "c2"]);
            assert_eq!(rows[1].message_count, 1, "m3 is in c2's conversation but not a hit");
            assert_eq!(get_thread(c, a, "c2", false)?.len(), 2, "the reader shows the whole conversation");

            // Closing the search drops what it added and keeps what was cached.
            assert_eq!(clear_search_cache(c)?, 2);
            let left: Vec<String> = c.prepare("SELECT remote_id FROM messages")?.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
            assert_eq!(left, ["m1"]);
            assert_eq!(c.query_row("SELECT COUNT(*) FROM search_cache", [], |r| r.get::<_, i64>(0))?, 0);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn ids_freed_by_closing_a_search_are_never_handed_out_again() {
        let (db, a, f) = seed();
        db.with(|c| {
            upsert_message(c, a, f, &msg("m1", "c1", "2026-10-01T10:00:00Z", true))?;
            keep_search_result(c, a, f, &msg("old", "c2", "2025-01-01T10:00:00Z", true), true)?;
            let freed: i64 = c.query_row("SELECT id FROM messages WHERE remote_id = 'old'", [], |r| r.get(0))?;
            clear_search_cache(c)?;
            // A draft replying to the search result still holds `freed`: new mail mustn't take it.
            upsert_message(c, a, f, &msg("new", "c3", "2026-10-02T10:00:00Z", false))?;
            let id: i64 = c.query_row("SELECT id FROM messages WHERE remote_id = 'new'", [], |r| r.get(0))?;
            assert!(id > freed, "{id} must be above the freed {freed}");
            assert!(message_remote(c, freed)?.is_none(), "the draft finds its original gone, not another message");
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn mail_sync_stores_is_not_dropped_when_the_search_closes() {
        let (db, a, f) = seed();
        db.with(|c| {
            // The search found it before sync did (first sync, or a message just arriving).
            keep_search_result(c, a, f, &msg("m1", "c1", "2026-10-01T10:00:00Z", false), true)?;
            upsert_message(c, a, f, &msg("m1", "c1", "2026-10-01T10:00:00Z", false))?;
            assert_eq!(clear_search_cache(c)?, 0);
            assert_eq!(c.query_row("SELECT COUNT(*) FROM messages", [], |r| r.get::<_, i64>(0))?, 1);

            // A partial update (opening it marked it read) isn't sync storing it.
            keep_search_result(c, a, f, &msg("m2", "c2", "2025-01-01T10:00:00Z", false), true)?;
            upsert_message(c, a, f, &MessageUpsert { remote_id: "m2".into(), is_read: Some(true), ..Default::default() })?;
            assert_eq!(clear_search_cache(c)?, 1);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn a_snoozed_search_result_stays_when_the_search_closes() {
        let (db, a, f) = seed();
        db.with(|c| {
            keep_search_result(c, a, f, &msg("m1", "c1", "2025-01-01T10:00:00Z", true), true)?;
            let snoozed = ensure_local_folder(c, a, crate::actions::SNOOZED_ROLE, "Snoozed")?.id;
            c.execute("UPDATE messages SET folder_id = ?1", [snoozed])?;
            assert_eq!(clear_search_cache(c)?, 0);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn a_panic_while_holding_the_connection_doesnt_break_every_later_call() {
        let db = Db::open_in_memory().unwrap();
        let hit = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = db.with(|c| {
                let tx = c.transaction()?;
                tx.execute("INSERT INTO settings (key, value) VALUES ('cache_days', 'done')", [])?;
                panic!("bug inside a transaction");
                #[allow(unreachable_code)]
                Ok(())
            });
        }));
        assert!(hit.is_err());
        // The lock is usable again, and the half-done transaction was rolled back.
        assert_eq!(db.with(|c| get_setting(c, "cache_days")).unwrap(), None);
        db.with(|c| set_setting(c, "cache_days", "ok")).unwrap();
        assert_eq!(db.with(|c| get_setting(c, "cache_days")).unwrap().as_deref(), Some("ok"));
    }

    #[test]
    fn unified_inbox_spans_signed_in_accounts() {
        let (db, a, f) = seed();
        db.with(|c| {
            let b = upsert_account(c, "microsoft", "boris@work.example", None, b"t")?.id;
            let fb = insert_folder(c, b, "B-INBOX", "inbox", "Inbox")?.id;
            let b_sent = insert_folder(c, b, "B-SENT", "sentitems", "Sent")?.id;
            upsert_message(c, a, f, &msg("m1", "c1", "2026-10-01T10:00:00Z", false))?;
            upsert_message(c, b, fb, &msg("w1", "c1", "2026-10-02T10:00:00Z", true))?; // same conversation id, other account
            upsert_message(c, b, b_sent, &msg("w2", "c9", "2026-10-03T10:00:00Z", true))?;
            let all = |q: &str| list_threads(c, Scope::Role("inbox"), &crate::query::compile(q), None, 10).unwrap();

            let t = all("");
            assert_eq!(t.iter().map(|t| (t.account_id, t.conversation_id.as_str())).collect::<Vec<_>>(), vec![(b, "c1"), (a, "c1")]);
            assert!(t.iter().all(|t| t.message_count == 1), "conversations don't merge across accounts");
            assert_eq!(count_threads(c, Scope::Role("inbox"), &crate::query::compile(""))?, (2, 1));
            assert_eq!(all("account:work").iter().map(|t| t.account_id).collect::<Vec<_>>(), vec![b]);
            assert_eq!(all("-account:work").iter().map(|t| t.account_id).collect::<Vec<_>>(), vec![a]);

            // A signed-out account's cached mail leaves the unified inbox.
            c.execute("UPDATE accounts SET token_blob = NULL WHERE id = ?1", [b])?;
            assert_eq!(all("").len(), 1);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn partial_update_keeps_cached_fields() {
        let (db, a, f) = seed();
        db.with(|c| {
            upsert_message(c, a, f, &msg("m1", "c1", "2026-10-01T10:00:00Z", false))?;
            upsert_message(c, a, f, &MessageUpsert { remote_id: "m1".into(), is_read: Some(true), ..Default::default() })?;
            let t = get_thread(c, a, "c1", false)?;
            assert_eq!(t.len(), 1);
            assert!(t[0].is_read);
            assert_eq!(t[0].subject, "subject m1");
            // A partial update for an unknown message must not create a row.
            upsert_message(c, a, f, &MessageUpsert { remote_id: "ghost".into(), is_read: Some(true), ..Default::default() })?;
            let n: i64 = c.query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))?;
            assert_eq!(n, 1);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn clearing_tokens_signs_out_but_keeps_mail() {
        let (db, a, f) = seed();
        db.with(|c| {
            upsert_message(c, a, f, &msg("m1", "c1", "2026-10-01T10:00:00Z", false))?;
            clear_app_tokens(c, "microsoft", "builtin")?;
            let accounts = list_accounts(c)?;
            assert!(!accounts[0].signed_in);
            assert!(get_token_blob(c, a)?.is_none());
            // Signing in again with the same mailbox reuses the row and its cache.
            let again = upsert_account(c, "microsoft", "boris@example.com", None, b"y")?;
            assert_eq!(again.id, a);
            assert!(list_accounts(c)?[0].signed_in);
            assert_eq!(list_threads(c, Scope::Folder(f), &crate::query::compile(""), None, 10)?.len(), 1);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn prefetch_picks_newest_and_unread_without_cached_bodies() {
        let (db, a, f) = seed();
        db.with(|c| {
            for i in 1..=6 {
                // m1 (oldest) and m2 are unread; the rest are read.
                upsert_message(c, a, f, &msg(&format!("m{i}"), &format!("c{i}"), &format!("2026-10-0{i}T10:00:00Z"), i > 2))?;
            }
            put_body(c, 6, "cached", "")?; // m6 is newest but already cached
            // newest 2 = m6 (cached, skipped), m5; plus unread m2, m1
            assert_eq!(bodies_to_prefetch(c, 2, 10)?, vec![5, 2, 1]);
            assert_eq!(bodies_to_prefetch(c, 2, 2)?, vec![5, 2]);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn recent_opens_are_newest_first_survive_archive_and_dedupe() {
        let (db, a, f) = seed();
        db.with(|c| {
            upsert_message(c, a, f, &msg("m1", "c1", "2026-10-01T10:00:00Z", false))?;
            upsert_message(c, a, f, &msg("m2", "c1", "2026-10-02T10:00:00Z", false))?;
            upsert_message(c, a, f, &msg("m3", "c2", "2026-10-03T10:00:00Z", true))?;
            record_open(c, a, "c1")?;
            std::thread::sleep(std::time::Duration::from_millis(5));
            record_open(c, a, "c2")?;
            let r = recent_opens(c, 10)?;
            assert_eq!(r.iter().map(|x| x.thread.conversation_id.as_str()).collect::<Vec<_>>(), vec!["c2", "c1"]);
            assert_eq!(r[1].thread.subject, "subject m2", "shows the conversation's newest message");
            assert_eq!(r[1].thread.message_count, 2);

            // Reopening moves it back to the top without duplicating it.
            std::thread::sleep(std::time::Duration::from_millis(5));
            record_open(c, a, "c1")?;
            let r = recent_opens(c, 10)?;
            assert_eq!(r.len(), 2);
            assert_eq!(r[0].thread.conversation_id, "c1");

            // Archived conversations stay; deleted ones drop out.
            let archive = ensure_local_folder(c, a, "archive", "Archive")?;
            let trash = ensure_local_folder(c, a, "deleteditems", "Deleted Items")?;
            set_folder(c, 1, archive.id)?;
            set_folder(c, 2, archive.id)?;
            set_folder(c, 3, trash.id)?;
            let r = recent_opens(c, 10)?;
            assert_eq!(r.iter().map(|x| x.thread.conversation_id.as_str()).collect::<Vec<_>>(), vec!["c1"]);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn body_eviction_is_lru() {
        let (db, a, f) = seed();
        db.with(|c| {
            for (i, id) in ["m1", "m2", "m3"].iter().enumerate() {
                upsert_message(c, a, f, &msg(id, id, &format!("2026-10-0{}T10:00:00Z", i + 1), true))?;
            }
            for (id, ch) in [(1, "a"), (2, "b"), (3, "c")] {
                put_body(c, id, &ch.repeat(100), "")?;
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            get_body(c, 1)?; // touch m1, so m2 is now the oldest
            assert_eq!(evict_bodies(c, 200)?, 1);
            assert!(get_body(c, 2)?.is_none());
            assert!(get_body(c, 1)?.is_some());
            assert!(get_body(c, 3)?.is_some());
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn a_placeholder_folder_gets_its_server_id_and_keeps_its_mail() {
        let (db, a, _) = seed();
        db.with(|c| {
            let archive = ensure_local_folder(c, a, "archive", "Archive")?;
            assert!(archive.remote_id.starts_with(PLACEHOLDER_PREFIX));
            upsert_message(c, a, archive.id, &msg("m1", "c1", "2026-10-01T10:00:00Z", true))?;
            set_delta_link(c, archive.id, Some("stale"))?;

            let real = resolve_placeholder(c, archive.id, "AAMk-archive", "Archive")?;
            assert_eq!((real.id, real.remote_id.as_str(), real.delta_link.as_deref()), (archive.id, "AAMk-archive", None));
            assert_eq!(folder_by_role(c, a, "archive")?.unwrap().remote_id, "AAMk-archive");
            assert_eq!(list_threads(c, Scope::Folder(archive.id), &crate::query::compile(""), None, 10)?.len(), 1);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn mailboxes_list_their_folders_across_accounts() {
        let (db, a, inbox) = seed();
        db.with(|c| {
            let b = upsert_account(c, "google", "boris@gmail.com", None, b"x")?.id;
            let folder = |acct, role: &str| ensure_local_folder(c, acct, role, role).map(|f| f.id);
            let (sent, archive, archive_b) = (folder(a, "sentitems")?, folder(a, "archive")?, folder(b, "archive")?);
            let deleted = folder(b, "deleteditems")?;
            upsert_message(c, a, inbox, &msg("i1", "c-inbox", "2026-10-05T10:00:00Z", false))?;
            upsert_message(c, a, sent, &msg("s1", "c-sent", "2026-10-04T10:00:00Z", true))?;
            upsert_message(c, a, archive, &msg("a1", "c-done", "2026-10-03T10:00:00Z", true))?;
            upsert_message(c, b, archive_b, &msg("a2", "c-done-b", "2026-10-02T10:00:00Z", true))?;
            upsert_message(c, b, deleted, &msg("d1", "c-trash", "2026-10-01T10:00:00Z", true))?;
            // An archived conversation someone replied to is back in the inbox, not Done.
            upsert_message(c, a, archive, &msg("a3", "c-inbox", "2026-10-01T09:00:00Z", true))?;

            let convs = |m: Mailbox| -> Vec<String> {
                list_threads(c, m.scope(), &crate::query::compile(""), None, 10).unwrap().into_iter().map(|t| t.conversation_id).collect()
            };
            assert_eq!(convs(Mailbox::Inbox), ["c-inbox"]);
            assert_eq!(convs(Mailbox::Sent), ["c-sent"]);
            assert_eq!(convs(Mailbox::Done), ["c-done", "c-done-b"]);
            assert_eq!(convs(Mailbox::Trash), ["c-trash"]);
            assert_eq!(convs(Mailbox::All), ["c-inbox", "c-sent", "c-done", "c-done-b"]);

            // The Trash view's reader shows the deleted message; elsewhere it's hidden.
            assert!(get_thread(c, b, "c-trash", false)?.is_empty());
            assert_eq!(get_thread(c, b, "c-trash", true)?.len(), 1);

            // A signed-out account's mail leaves every mailbox.
            c.execute("UPDATE accounts SET token_blob = NULL WHERE id = ?1", [b])?;
            assert_eq!(convs(Mailbox::Done), ["c-done"]);
            assert!(convs(Mailbox::Trash).is_empty());
            assert_eq!(convs(Mailbox::All), ["c-inbox", "c-sent", "c-done"]);
            Ok(())
        })
        .unwrap();
    }
}
