//! Where PeetMail keeps its data, and carrying it over from the identifier the
//! early dev builds used (`com.boris.peetmail`). Remove the legacy part once nobody has that data.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// tauri.conf.json `identifier`: names the data folders.
pub const IDENTIFIER: &str = "com.peetsoftware.peetmail";
pub const LEGACY_IDENTIFIER: &str = "com.boris.peetmail";

/// The database and its WAL files, in the roaming app data dir.
pub const DB_FILES: &[&str] = &["peetmail.db", "peetmail.db-wal", "peetmail.db-shm"];
/// WebView2's profile (localStorage: hidden accounts, quick-add choices), in the local dir.
pub const WEBVIEW_FILES: &[&str] = &["EBWebView"];

/// Files besides the database that the dev copy needs: the Linux key for saved sign-ins
/// (Windows' DPAPI is per user, so the copied tokens decrypt as they are).
pub const DEV_SEED_FILES: &[&str] = &["token.key", "token.key.keyring"];

/// Where a dev build (`pnpm tauri dev`) keeps its data: `.peetmail-dev` (gitignored) in the
/// checkout it was built from. Each worktree gets its own copy, migrated to its own
/// branch's schema, and removing the worktree removes it.
pub fn dev_dir() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest.parent().unwrap_or(manifest).join(DEV_DIR_NAME)
}
pub const DEV_DIR_NAME: &str = ".peetmail-dev";

/// Gives a dev data folder a copy of the installed app's database, the first time only
/// (delete the dev folder to take a fresh copy). The copy is a `VACUUM INTO` through a
/// read-only connection, so it's consistent even with the installed app running and the
/// database is never changed (SQLite may leave empty -wal/-shm files beside it). Work the
/// installed app still owes the server is dropped from the copy before it's put in place,
/// so a dev start that fails halfway can't leave it for a later run to send again.
/// Returns whether a copy was made.
pub fn seed_dev(prod: &Path, dev: &Path) -> std::io::Result<bool> {
    let (from, to) = (prod.join(DB_FILES[0]), dev.join(DB_FILES[0]));
    if to.exists() || !from.exists() {
        return Ok(false);
    }
    std::fs::create_dir_all(dev)?;
    let partial = dev.join(format!("{}.copying", DB_FILES[0]));
    let _ = std::fs::remove_file(&partial);
    let copied = copy_without_pending_work(&from, &partial).and_then(|()| {
        for name in DEV_SEED_FILES {
            let f = prod.join(name);
            if f.exists() {
                std::fs::copy(&f, dev.join(name))?;
            }
        }
        std::fs::rename(&partial, &to)
    });
    if copied.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    copied.map(|()| true)
}

fn copy_without_pending_work(from: &Path, to: &Path) -> std::io::Result<()> {
    let conn = rusqlite::Connection::open_with_flags(from, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(std::io::Error::other)?;
    conn.execute("VACUUM INTO ?1", [to.to_string_lossy()]).map_err(std::io::Error::other)?;
    drop(conn);
    let copy = rusqlite::Connection::open(to).map_err(std::io::Error::other)?;
    crate::db::forget_pending_work(&copy).map_err(|e| std::io::Error::other(e.to_string()))
}

/// The folder Tauri's `app_data_dir` resolves to, for code that runs before Tauri is up
/// (the crash reporter starts first thing). `%APPDATA%\<id>` on Windows,
/// `$XDG_DATA_HOME/<id>` (default `~/.local/share/<id>`) on Linux.
pub fn app_data_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join(IDENTIFIER))
    } else {
        xdg_data_dir(std::env::var_os("XDG_DATA_HOME"), std::env::var_os("HOME"))
    }
}

/// The XDG base directory rule: `XDG_DATA_HOME` if it's an absolute path, else `$HOME/.local/share`.
pub fn xdg_data_dir(xdg_data_home: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    let base = xdg_data_home
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| home.map(|h| PathBuf::from(h).join(".local").join("share")))?;
    Some(base.join(IDENTIFIER))
}

/// Moves `names` from the legacy sibling of `dir` into `dir`, if `dir` doesn't have the
/// first of them yet and the legacy dir does. Returns whether anything moved. Both dirs sit
/// under the same parent, so this is a rename, not a copy.
pub fn adopt_legacy(dir: &Path, names: &[&str]) -> std::io::Result<bool> {
    let Some(parent) = dir.parent() else { return Ok(false) };
    let legacy = parent.join(LEGACY_IDENTIFIER);
    let Some(first) = names.first() else { return Ok(false) };
    if dir.join(first).exists() || !legacy.join(first).exists() {
        return Ok(false);
    }
    std::fs::create_dir_all(dir)?;
    for name in names {
        let from = legacy.join(name);
        if from.exists() {
            std::fs::rename(&from, dir.join(name))?;
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(unix)] // "/data" isn't an absolute path on Windows
    fn xdg_data_dir_follows_the_spec() {
        let os = |s: &str| Some(OsString::from(s));
        assert_eq!(xdg_data_dir(os("/data"), os("/home/ann")), Some(PathBuf::from("/data").join(IDENTIFIER)));
        assert_eq!(xdg_data_dir(None, os("/home/ann")), Some(PathBuf::from("/home/ann/.local/share").join(IDENTIFIER)));
        // A relative XDG_DATA_HOME is invalid and ignored.
        assert_eq!(xdg_data_dir(os("rel/dir"), os("/home/ann")), Some(PathBuf::from("/home/ann/.local/share").join(IDENTIFIER)));
        assert_eq!(xdg_data_dir(None, None), None);
    }
    use std::fs;

    fn temp_root(tag: &str) -> std::path::PathBuf {
        let mut b = [0u8; 8];
        getrandom::fill(&mut b).unwrap();
        let root = std::env::temp_dir().join(format!("peetmail-datadir-{tag}-{:016x}", u64::from_le_bytes(b)));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn moves_the_legacy_database_into_a_new_dir() {
        let root = temp_root("move");
        let legacy = root.join(LEGACY_IDENTIFIER);
        fs::create_dir_all(&legacy).unwrap();
        fs::write(legacy.join("peetmail.db"), b"db").unwrap();
        fs::write(legacy.join("peetmail.db-wal"), b"wal").unwrap();
        let dir = root.join("com.peetsoftware.peetmail");

        assert!(adopt_legacy(&dir, DB_FILES).unwrap());
        assert_eq!(fs::read(dir.join("peetmail.db")).unwrap(), b"db");
        assert_eq!(fs::read(dir.join("peetmail.db-wal")).unwrap(), b"wal");
        assert!(!legacy.join("peetmail.db").exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn never_overwrites_an_existing_database() {
        let root = temp_root("keep");
        let legacy = root.join(LEGACY_IDENTIFIER);
        let dir = root.join("com.peetsoftware.peetmail");
        fs::create_dir_all(&legacy).unwrap();
        fs::create_dir_all(&dir).unwrap();
        fs::write(legacy.join("peetmail.db"), b"old").unwrap();
        fs::write(dir.join("peetmail.db"), b"new").unwrap();

        assert!(!adopt_legacy(&dir, DB_FILES).unwrap());
        assert_eq!(fs::read(dir.join("peetmail.db")).unwrap(), b"new");
        assert!(legacy.join("peetmail.db").exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn the_dev_dir_is_in_the_checkout_and_gitignored() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        assert_eq!(dev_dir(), root.join(DEV_DIR_NAME));
        let ignore = fs::read_to_string(root.join(".gitignore")).unwrap();
        assert!(ignore.lines().any(|l| l.trim() == format!("/{DEV_DIR_NAME}")), "a copy of real mail must never be committed");
    }

    #[test]
    fn seeds_the_dev_dir_with_a_copy_and_leaves_the_original_alone() {
        let root = temp_root("seed");
        let prod = root.join(IDENTIFIER);
        let dev = root.join(DEV_DIR_NAME);
        fs::create_dir_all(&prod).unwrap();
        let db = prod.join("peetmail.db");
        {
            let c = rusqlite::Connection::open(&db).unwrap();
            c.execute_batch(
                "PRAGMA journal_mode = WAL; PRAGMA user_version = 7; CREATE TABLE t (v TEXT); INSERT INTO t VALUES ('mail');
                 CREATE TABLE outbox (id INTEGER PRIMARY KEY, op TEXT); INSERT INTO outbox (op) VALUES ('send');",
            )
            .unwrap();
        }
        fs::write(prod.join("token.key"), b"key").unwrap();
        let before = fs::read(&db).unwrap();

        assert!(seed_dev(&prod, &dev).unwrap());
        assert_eq!(fs::read(&db).unwrap(), before, "the installed app's database isn't written");
        let c = rusqlite::Connection::open(dev.join("peetmail.db")).unwrap();
        let v: String = c.query_row("SELECT v FROM t", [], |r| r.get(0)).unwrap();
        let schema: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!((v.as_str(), schema), ("mail", 7));
        let queued: i64 = c.query_row("SELECT COUNT(*) FROM outbox", [], |r| r.get(0)).unwrap();
        assert_eq!(queued, 0, "the installed app's queued sends are dropped before the copy is in place");
        assert_eq!(fs::read(dev.join("token.key")).unwrap(), b"key");
        assert!(!dev.join("peetmail.db.copying").exists());

        // Once there, the dev copy is its own: a second run doesn't replace it.
        c.execute("UPDATE t SET v = 'dev'", []).unwrap();
        drop(c);
        assert!(!seed_dev(&prod, &dev).unwrap());
        let c = rusqlite::Connection::open(dev.join("peetmail.db")).unwrap();
        let v: String = c.query_row("SELECT v FROM t", [], |r| r.get(0)).unwrap();
        assert_eq!(v, "dev");
        drop(c);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn nothing_to_seed_without_an_installed_database() {
        let root = temp_root("seed-none");
        let prod = root.join(IDENTIFIER);
        let dev = root.join(DEV_DIR_NAME);
        assert!(!seed_dev(&prod, &dev).unwrap());
        assert!(!dev.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn nothing_to_adopt_without_a_legacy_dir() {
        let root = temp_root("none");
        assert!(!adopt_legacy(&root.join("com.peetsoftware.peetmail"), WEBVIEW_FILES).unwrap());
        let _ = fs::remove_dir_all(root);
    }
}
