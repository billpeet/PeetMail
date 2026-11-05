//! Settings sync's local bookkeeping (the `sync_items` table, migration 0017) and its own
//! settings. Every change to a synced setting goes through `touch` (from db::set_setting and
//! db::delete_setting), whether or not sync is on; changes that arrive from the server go
//! through `apply_remote`, which writes the setting without marking it as a local change.

use std::sync::LazyLock;

use rusqlite::{params, Connection, OptionalExtension};

use super::hlc;
use crate::db;
use crate::error::Result;

pub const KEY_NODE: &str = "settings_sync_node";
pub const KEY_SERVER: &str = "settings_sync_server";
pub const KEY_CURSOR: &str = "settings_sync_cursor";
pub const KEY_SECRET: &str = "settings_sync_secret";
/// `settings_sync_link:<account id>`: whether an account is linked to the sync account.
pub const LINK_PREFIX: &str = "settings_sync_link:";

pub fn link_key(account_id: i64) -> String {
    format!("{LINK_PREFIX}{account_id}")
}

/// Wakes the sync loop: after a local change (it then waits a moment for more), when the
/// window gets focus, when sync is turned on.
pub static WAKE: LazyLock<tokio::sync::Notify> = LazyLock::new(tokio::sync::Notify::new);

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// This device's name in clock stamps: random, made once.
fn node(c: &Connection) -> Result<String> {
    if let Some(n) = db::get_setting(c, KEY_NODE)? {
        return Ok(n);
    }
    let mut b = [0u8; 4];
    getrandom::fill(&mut b).expect("OS RNG unavailable");
    let n: String = b.iter().map(|x| format!("{x:02x}")).collect();
    db::set_setting(c, KEY_NODE, &n)?;
    Ok(n)
}

fn latest(c: &Connection) -> Result<Option<String>> {
    Ok(c.query_row("SELECT MAX(hlc) FROM sync_items", [], |r| r.get(0))?)
}

/// Records a local change to a synced setting (`deleted`: it was removed).
pub fn touch(c: &Connection, key: &str, deleted: bool) -> Result<()> {
    let stamp = hlc::next(latest(c)?.as_deref(), now_ms(), &node(c)?);
    c.execute(
        "INSERT INTO sync_items (key, hlc, dirty, deleted) VALUES (?1, ?2, 1, ?3)
         ON CONFLICT (key) DO UPDATE SET hlc = excluded.hlc, dirty = 1, deleted = excluded.deleted",
        params![key, stamp, deleted],
    )?;
    WAKE.notify_one();
    Ok(())
}

#[derive(Debug, Clone, PartialEq)]
pub struct Local {
    pub hlc: String,
    pub server_seq: Option<i64>,
    pub dirty: bool,
}

pub fn local(c: &Connection, key: &str) -> Result<Option<Local>> {
    Ok(c
        .query_row("SELECT hlc, server_seq, dirty FROM sync_items WHERE key = ?1", [key], |r| {
            Ok(Local { hlc: r.get(0)?, server_seq: r.get(1)?, dirty: r.get(2)? })
        })
        .optional()?)
}

/// A local change waiting to go up.
#[derive(Debug, Clone, PartialEq)]
pub struct Dirty {
    pub key: String,
    pub hlc: String,
    pub server_seq: Option<i64>,
    /// None: deleted.
    pub value: Option<String>,
}

pub fn dirty(c: &Connection, limit: i64) -> Result<Vec<Dirty>> {
    let mut stmt = c.prepare(
        "SELECT i.key, i.hlc, i.server_seq, CASE WHEN i.deleted THEN NULL ELSE s.value END
           FROM sync_items i LEFT JOIN settings s ON s.key = i.key
          WHERE i.dirty = 1 ORDER BY i.key LIMIT ?1",
    )?;
    let rows = stmt
        .query_map([limit], |r| Ok(Dirty { key: r.get(0)?, hlc: r.get(1)?, server_seq: r.get(2)?, value: r.get(3)? }))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

/// Writes a change from another device: the setting (or its removal) and its stamp, in step
/// with the server's version, and not a local change.
pub fn apply_remote(c: &Connection, key: &str, value: Option<&str>, stamp: &str, seq: i64) -> Result<()> {
    match value {
        Some(v) => db::write_setting_unsynced(c, key, v)?,
        None => db::delete_setting_unsynced(c, key)?,
    }
    c.execute(
        "INSERT INTO sync_items (key, hlc, server_seq, dirty, deleted) VALUES (?1, ?2, ?3, 0, ?4)
         ON CONFLICT (key) DO UPDATE SET hlc = excluded.hlc, server_seq = excluded.server_seq, dirty = 0, deleted = excluded.deleted",
        params![key, stamp, seq, value.is_none()],
    )?;
    Ok(())
}

/// The server's version of a setting this device keeps its own (newer or equal) value for.
pub fn in_step(c: &Connection, key: &str, seq: i64, still_dirty: bool) -> Result<()> {
    c.execute("UPDATE sync_items SET server_seq = ?2, dirty = ?3 WHERE key = ?1", params![key, seq, still_dirty])?;
    Ok(())
}

/// A push of `stamp` was stored as version `seq`. A change made while it was in flight
/// (another stamp) stays dirty.
pub fn pushed(c: &Connection, key: &str, stamp: &str, seq: i64) -> Result<()> {
    c.execute(
        "UPDATE sync_items SET server_seq = ?3, dirty = CASE WHEN hlc = ?2 THEN 0 ELSE dirty END WHERE key = ?1",
        params![key, stamp, seq],
    )?;
    Ok(())
}

/// Before a device's first sync: every synced setting goes up (after the first pull has
/// merged in what the server has). Settings never edited here get the zero stamp, so they
/// lose to any value from another device.
pub fn prepare_first_sync(c: &Connection) -> Result<()> {
    let keys: Vec<String> = c
        .prepare("SELECT key FROM settings")?
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<Vec<String>>>()?
        .into_iter()
        .filter(|k| super::scope(k) == Some(super::Scope::Synced))
        .collect();
    for k in keys {
        c.execute("INSERT OR IGNORE INTO sync_items (key, hlc) VALUES (?1, ?2)", params![k, hlc::ZERO])?;
    }
    c.execute("UPDATE sync_items SET dirty = 1, server_seq = NULL", [])?;
    db::delete_setting(c, KEY_CURSOR)
}

/// After turning sync off: nothing is in step with any server any more.
pub fn forget_server(c: &Connection) -> Result<()> {
    c.execute("UPDATE sync_items SET dirty = 0, server_seq = NULL", [])?;
    for k in [KEY_SERVER, KEY_CURSOR, KEY_SECRET] {
        db::delete_setting(c, k)?;
    }
    c.execute("DELETE FROM settings WHERE substr(key, 1, ?2) = ?1", params![LINK_PREFIX, LINK_PREFIX.len() as i64])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> db::Db {
        db::Db::open_in_memory().unwrap()
    }

    #[test]
    fn changing_a_synced_setting_marks_it_and_unchanged_writes_dont() {
        let db = db();
        db.with(|c| {
            db::set_setting(c, "week_starts_on", "0")?;
            let first = local(c, "week_starts_on")?.unwrap();
            assert!(first.dirty);
            // set_preferences writes every preference on each change: same value, no new stamp.
            c.execute("UPDATE sync_items SET dirty = 0", [])?;
            db::set_setting(c, "week_starts_on", "0")?;
            assert_eq!(local(c, "week_starts_on")?.unwrap(), Local { dirty: false, ..first.clone() });
            db::set_setting(c, "week_starts_on", "1")?;
            let second = local(c, "week_starts_on")?.unwrap();
            assert!(second.dirty && second.hlc > first.hlc);
            // Local settings are never tracked.
            db::set_setting(c, "list_width", "500")?;
            assert_eq!(local(c, "list_width")?, None);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn deleting_a_synced_setting_leaves_a_tombstone_to_push() {
        let db = db();
        db.with(|c| {
            db::set_setting(c, "snippet:thanks", r#"{"name":"thanks","body":"Thanks!","order":1}"#)?;
            db::delete_setting(c, "snippet:thanks")?;
            let d = dirty(c, 100)?;
            assert_eq!(d.iter().find(|d| d.key == "snippet:thanks").map(|d| d.value.clone()), Some(None));
            // Deleting what isn't there is no change.
            c.execute("UPDATE sync_items SET dirty = 0", [])?;
            db::delete_setting(c, "snippet:nothing")?;
            assert!(dirty(c, 100)?.is_empty());
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn remote_changes_are_written_without_counting_as_local_ones() {
        let db = db();
        db.with(|c| {
            apply_remote(c, "snippet:hi", Some(r#"{"name":"hi","body":"Hi","order":1}"#), "0000000005000:0000:b", 7)?;
            assert!(db::get_setting(c, "snippet:hi")?.is_some());
            assert_eq!(local(c, "snippet:hi")?.unwrap(), Local { hlc: "0000000005000:0000:b".into(), server_seq: Some(7), dirty: false });
            // The next local edit's stamp passes the one seen, whatever this PC's clock says.
            db::set_setting(c, "snippet:hi", r#"{"name":"hi","body":"Hello","order":1}"#)?;
            assert!(local(c, "snippet:hi")?.unwrap().hlc.as_str() > "0000000005000:0000:b");
            apply_remote(c, "snippet:hi", None, "9999999999999:0000:b", 8)?;
            assert_eq!(db::get_setting(c, "snippet:hi")?, None);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn a_change_made_during_a_push_stays_dirty() {
        let db = db();
        db.with(|c| {
            db::set_setting(c, "notify", "all")?;
            let sent = local(c, "notify")?.unwrap().hlc;
            db::set_setting(c, "notify", "off")?;
            pushed(c, "notify", &sent, 3)?;
            let after = local(c, "notify")?.unwrap();
            assert_eq!((after.server_seq, after.dirty), (Some(3), true));
            pushed(c, "notify", &after.hlc, 4)?;
            assert!(!local(c, "notify")?.unwrap().dirty);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn a_first_sync_sends_everything_with_untouched_defaults_at_zero() {
        let db = db();
        db.with(|c| {
            // The default Other split comes from migration 0014, never edited here.
            db::set_setting(c, "undo_send_secs", "5")?;
            db::set_setting(c, "cache_days", "30")?;
            prepare_first_sync(c)?;
            let d = dirty(c, 100)?;
            let keys: Vec<&str> = d.iter().map(|d| d.key.as_str()).collect();
            assert_eq!(keys, ["split:other", "undo_send_secs"], "local settings stay out");
            assert_eq!(d[0].hlc, hlc::ZERO);
            assert!(d[1].hlc.as_str() > hlc::ZERO);
            assert!(d.iter().all(|d| d.server_seq.is_none()));
            Ok(())
        })
        .unwrap();
    }
}
