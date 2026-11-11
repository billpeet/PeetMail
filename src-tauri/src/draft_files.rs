//! Files attached in the composer. The composer hands over each file's bytes as soon as
//! it's picked, dropped or pasted; they wait in SQLite, referenced from the draft by id,
//! until the message has gone (or the composer throws it away). Keeping the bytes rather
//! than a path means a scheduled send still has the file if it's moved or edited meanwhile.

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, Result};

/// The most a message's attachments may add up to. Gmail refuses more than 25 MB, and so
/// does Outlook.com; Microsoft 365 often allows more, but one limit is easier to explain.
pub const MAX_TOTAL: i64 = 25 * 1024 * 1024;

/// A file attached to a draft, as the draft (and the composer) know it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DraftFile {
    pub id: i64,
    pub name: String,
    pub mime: String,
    pub size: i64,
}

/// "12.3 MB", for messages about the limit.
pub fn megabytes(bytes: i64) -> String {
    format!("{:.1} MB", bytes as f64 / 1024.0 / 1024.0)
}

/// The type for a file name, for files that come without one (shared from another app on
/// Android). Anything not listed goes as a plain attachment.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub fn mime_for(name: &str) -> &'static str {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "pdf" => "application/pdf",
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "heic" => "image/heic",
        "txt" => "text/plain",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "ics" => "text/calendar",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "zip" => "application/zip",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        _ => "application/octet-stream",
    }
}

/// A file name that's safe to send: no folders, no control characters, never empty.
fn clean_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let cleaned: String = base.chars().filter(|c| !c.is_control()).collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() { "attachment".into() } else { cleaned.into() }
}

pub fn add(conn: &Connection, name: &str, mime: &str, data: &[u8]) -> Result<DraftFile> {
    let size = data.len() as i64;
    let name = clean_name(name);
    if size > MAX_TOTAL {
        return Err(AppError::Other(format!("{name} is {}; attachments can add up to {} at most", megabytes(size), megabytes(MAX_TOTAL))));
    }
    let mime = match mime.trim() {
        "" => "application/octet-stream".to_string(),
        m => m.to_string(),
    };
    let id = conn.query_row(
        "INSERT INTO draft_files (name, mime, size, data) VALUES (?1, ?2, ?3, ?4) RETURNING id",
        params![name, mime, size, data],
        |r| r.get(0),
    )?;
    Ok(DraftFile { id, name, mime, size })
}

/// A file's bytes. None if it's gone (sent, or discarded).
pub fn data(conn: &Connection, id: i64) -> Result<Option<Vec<u8>>> {
    Ok(conn.query_row("SELECT data FROM draft_files WHERE id = ?1", [id], |r| r.get(0)).optional()?)
}

/// A file's bytes for sending. Missing is a permanent failure (retrying won't bring it back).
/// (The message leaves the file's name out: it ends up in the log.)
pub fn for_send(conn: &Connection, f: &DraftFile) -> Result<Vec<u8>> {
    data(conn, f.id)?.ok_or_else(|| AppError::Graph { status: 400, message: "an attached file is no longer there".into() })
}

/// What's known about these files, in the order given; an error names any that are gone.
pub fn describe(conn: &Connection, ids: &[i64]) -> Result<Vec<DraftFile>> {
    let mut stmt = conn.prepare_cached("SELECT name, mime, size FROM draft_files WHERE id = ?1")?;
    ids.iter()
        .map(|&id| {
            stmt.query_row([id], |r| Ok(DraftFile { id, name: r.get(0)?, mime: r.get(1)?, size: r.get(2)? }))
                .optional()?
                .ok_or_else(|| AppError::Other("an attached file is no longer there; attach it again".into()))
        })
        .collect()
}

pub fn remove(conn: &Connection, ids: &[i64]) -> Result<()> {
    let mut stmt = conn.prepare_cached("DELETE FROM draft_files WHERE id = ?1")?;
    for id in ids {
        stmt.execute([id])?;
    }
    Ok(())
}

/// Drops files no saved draft or waiting send refers to: left behind by a send that failed
/// for good, or added in the moment before a crash. Run at startup, when no composer is open.
pub fn purge_orphans(conn: &Connection) -> Result<usize> {
    Ok(conn.execute(
        "DELETE FROM draft_files WHERE id NOT IN (
           SELECT json_extract(f.value, '$.id') FROM outbox o, json_each(o.op, '$.attachments') f
           WHERE o.status IN ('pending', 'inflight') AND json_extract(o.op, '$.type') = 'send'
           UNION
           SELECT json_extract(f.value, '$.id') FROM drafts d, json_each(d.data, '$.attachments') f)",
        [],
    )?)
}

#[cfg(test)]
mod tests {
    #[test]
    fn shared_files_get_a_type_from_their_name() {
        assert_eq!(super::mime_for("Scan.PDF"), "application/pdf");
        assert_eq!(super::mime_for("photo.jpeg"), "image/jpeg");
        assert_eq!(super::mime_for("notes"), "application/octet-stream");
        assert_eq!(super::mime_for("archive.tar.gz"), "application/octet-stream");
    }

    use super::*;
    use crate::db::{self, Db};
    use crate::outbox::{self, Draft, DraftKind, Op};

    #[test]
    fn files_are_stored_named_safely_and_described_in_order() {
        let db = Db::open_in_memory().unwrap();
        db.with(|c| {
            let a = add(c, "C:\\Users\\b\\Q4 plan.pdf", "application/pdf", b"%PDF")?;
            assert_eq!(a.name, "Q4 plan.pdf");
            assert_eq!(a.size, 4);
            let b = add(c, "", "", b"x")?;
            assert_eq!((b.name.as_str(), b.mime.as_str()), ("attachment", "application/octet-stream"));
            assert_eq!(describe(c, &[b.id, a.id])?, vec![b.clone(), a.clone()]);
            assert_eq!(data(c, a.id)?.as_deref(), Some(&b"%PDF"[..]));
            remove(c, &[a.id])?;
            assert!(data(c, a.id)?.is_none());
            assert!(describe(c, &[a.id]).is_err(), "a missing file is an error, not silently left out");
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn a_file_over_the_limit_is_refused() {
        let db = Db::open_in_memory().unwrap();
        let big = vec![0u8; MAX_TOTAL as usize + 1];
        let err = db.with(|c| add(c, "huge.iso", "", &big)).unwrap_err().to_string();
        assert!(err.contains("huge.iso is 25.0 MB"), "{err}");
    }

    #[test]
    fn purging_keeps_only_files_a_waiting_send_needs() {
        let db = Db::open_in_memory().unwrap();
        db.with(|c| {
            let acct = db::upsert_account(c, "microsoft", "b@x.com", None, b"t")?.id;
            let kept = add(c, "kept.txt", "text/plain", b"k")?;
            let orphan = add(c, "orphan.txt", "text/plain", b"o")?;
            let failed = add(c, "failed.txt", "text/plain", b"f")?;
            let draft = |f: &DraftFile| Draft { kind: DraftKind::New, to: vec!["p@x.com".into()], attachments: vec![f.clone()], ..Default::default() };
            outbox::enqueue(c, acct, None, &Op::Send(draft(&kept)), std::time::Duration::from_secs(60))?;
            let gone = outbox::enqueue(c, acct, None, &Op::Send(draft(&failed)), std::time::Duration::ZERO)?;
            c.execute("UPDATE outbox SET status = 'failed' WHERE id = ?1", [gone])?;
            assert_eq!(purge_orphans(c)?, 2);
            assert!(data(c, kept.id)?.is_some());
            assert!(data(c, orphan.id)?.is_none() && data(c, failed.id)?.is_none());
            Ok(())
        })
        .unwrap();
    }
}
