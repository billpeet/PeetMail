//! Help → Report a problem: a diagnostics zip, and feedback to the developers.
//!
//! The diagnostics zip holds what helps with "it won't sync" or "it won't start", and
//! nothing personal: versions, settings without secrets or user text, the shape of the
//! database (schema, table sizes, integrity), accounts by provider (no addresses), the
//! outbox by kind and state, update and privacy status, and the last few log files,
//! scrubbed like crash reports (scrub.rs).
//!
//! Feedback goes to Sentry as a user-feedback envelope (with the zip attached if asked),
//! through a client made just for it: it's an explicit action, so it works with crash
//! reports turned off, and the user's own words are sent as typed (the form says so).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

#[cfg(test)]
use crate::db;
use crate::error::{AppError, Result};
use crate::state::AppState;
use crate::testenv;

/// Log files are scrubbed and trimmed to this much in total (newest first).
const LOG_BUDGET: usize = 2 * 1024 * 1024;

/// Preferences safe to show as values: plain switches and numbers, no user text.
const SAFE_SETTINGS: &[&str] = &[
    "cache_days",
    "body_cache_mb",
    "notify",
    "close_to_tray",
    "undo_send_secs",
    "week_starts_on",
    "show_right_pane",
    "right_pane_width",
    "list_width",
    "hide_empty_splits",
    "layout",
    "right_pane_tab",
    "update_channel",
    "crash_reports",
    "usage_data",
    "privacy_notice_seen",
];

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedbackOptions {
    /// Feedback can go to the developers (the build has a Sentry DSN).
    sentry: bool,
    /// Where "prefer email?" writes to, if the build has an address.
    email: Option<String>,
}

pub fn feedback_options() -> FeedbackOptions {
    FeedbackOptions { sentry: sentry_dsn().is_some(), email: feedback_email() }
}

fn sentry_dsn() -> Option<String> {
    testenv::get("PEETMAIL_SENTRY_DSN").or_else(|| option_env!("PEETMAIL_SENTRY_DSN").map(String::from)).filter(|d| !d.trim().is_empty())
}

fn feedback_email() -> Option<String> {
    testenv::get("PEETMAIL_FEEDBACK_EMAIL").or_else(|| option_env!("PEETMAIL_FEEDBACK_EMAIL").map(String::from)).filter(|e| e.contains('@'))
}

// ---------- diagnostics ----------

/// Builds the diagnostics zip in memory.
pub fn diagnostics_zip(app: &AppHandle) -> Result<Vec<u8>> {
    let state = app.state::<AppState>();
    crate::crash::refresh_known(&state.db);
    let summary = summary(app, &state)?;
    let logs = log_files(&log_dir(app));

    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let mut add = |name: &str, bytes: &[u8]| -> Result<()> {
        zip.start_file(name, opts).map_err(|e| AppError::Other(format!("diagnostics zip: {e}")))?;
        zip.write_all(bytes)?;
        Ok(())
    };
    add("README.txt", README.as_bytes())?;
    add("summary.json", serde_json::to_string_pretty(&summary)?.as_bytes())?;
    for (name, text) in &logs {
        add(&format!("logs/{name}"), text.as_bytes())?;
    }
    let cursor = zip.finish().map_err(|e| AppError::Other(format!("diagnostics zip: {e}")))?;
    Ok(cursor.into_inner())
}

const README: &str = "PeetMail diagnostics\r\n\r\nsummary.json: versions, settings (no secrets or text you typed), the database's shape and health, accounts by provider (no addresses), the outbox by kind and state, and update and privacy status.\r\nlogs/: PeetMail's recent log files, with addresses, names, tokens and IDs removed.\r\n\r\nNo mail, subjects, contacts or passwords are in this file.\r\n";

/// Writes the zip to Downloads and shows it in Explorer. Returns its path.
pub fn export_diagnostics(app: &AppHandle) -> Result<PathBuf> {
    let bytes = diagnostics_zip(app)?;
    let dir = match testenv::get("PEETMAIL_DOWNLOAD_DIR") {
        Some(d) => PathBuf::from(d),
        None => app.path().download_dir().map_err(|e| AppError::Other(e.to_string()))?,
    };
    std::fs::create_dir_all(&dir)?;
    let name = format!("PeetMail-diagnostics-{}-{}.zip", env!("CARGO_PKG_VERSION"), chrono::Local::now().format("%Y%m%d-%H%M%S"));
    let path = dir.join(name);
    std::fs::write(&path, bytes)?;
    if testenv::get("PEETMAIL_DOWNLOAD_DIR").is_none() {
        use tauri_plugin_opener::OpenerExt;
        let _ = app.opener().reveal_item_in_dir(&path);
    }
    log::info!("diagnostics exported");
    Ok(path)
}

fn summary(app: &AppHandle, state: &AppState) -> Result<Value> {
    let db_part = state.db.with(|c| database_summary(c))?;
    let install_id = state.db.with(|c| crate::crash::install_id(c))?;
    Ok(json!({
        "generated": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        "app": {
            "version": env!("CARGO_PKG_VERSION"),
            "channel": crate::crash::environment(),
            "build": if cfg!(debug_assertions) { "debug" } else { "release" },
            "installId": install_id,
        },
        "system": {
            "os": os_version(),
            // WebView2 on Windows, WebKitGTK on Linux.
            "webview": tauri::webview_version().unwrap_or_else(|e| format!("unknown ({e})")),
            "arch": std::env::consts::ARCH,
            "linux": linux_session(),
        },
        "updates": crate::update::status(app),
        "privacy": {
            "crashReports": crate::crash::enabled(),
            "usageData": crate::telemetry::enabled(),
            "crashReportsConfigured": crate::crash::configured(),
            "usageDataConfigured": crate::telemetry::configured(),
        },
        "database": db_part,
    }))
}

/// The database's shape and health. No row contents beyond enumerations (provider,
/// sign-in app, outbox kind and state).
pub fn database_summary(c: &rusqlite::Connection) -> Result<Value> {
    let schema: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    let pages: i64 = c.query_row("PRAGMA page_count", [], |r| r.get(0))?;
    let page_size: i64 = c.query_row("PRAGMA page_size", [], |r| r.get(0))?;
    let integrity: String = c.query_row("PRAGMA quick_check", [], |r| r.get(0)).unwrap_or_else(|e| format!("failed: {e}"));

    let tables: Vec<String> = c
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name")?
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let mut rows = serde_json::Map::new();
    for t in tables {
        let n: i64 = c.query_row(&format!("SELECT COUNT(*) FROM \"{}\"", t.replace('"', "\"\"")), [], |r| r.get(0)).unwrap_or(-1);
        rows.insert(t, json!(n));
    }

    let accounts: Vec<Value> = c
        .prepare("SELECT provider, oauth_app, token_blob IS NOT NULL FROM accounts ORDER BY id")?
        .query_map([], |r| Ok(json!({ "provider": r.get::<_, String>(0)?, "app": r.get::<_, String>(1)?, "signedIn": r.get::<_, bool>(2)? })))?
        .collect::<rusqlite::Result<_>>()?;
    let outbox: Vec<Value> = c
        .prepare("SELECT json_extract(op, '$.type'), status, COUNT(*), MAX(attempts) FROM outbox GROUP BY 1, 2 ORDER BY 1, 2")?
        .query_map([], |r| Ok(json!({ "kind": r.get::<_, Option<String>>(0)?, "status": r.get::<_, String>(1)?, "count": r.get::<_, i64>(2)?, "maxAttempts": r.get::<_, Option<i64>>(3)? })))?
        .collect::<rusqlite::Result<_>>()?;
    let body_bytes: i64 = c.query_row("SELECT COALESCE(SUM(size), 0) FROM message_bodies", [], |r| r.get(0)).unwrap_or(-1);

    let mut settings = serde_json::Map::new();
    let mut stmt = c.prepare("SELECT key, value FROM settings ORDER BY key")?;
    for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
        let (k, v) = row?;
        // One setting per entry (`signature:<address>`, `gmail_history:<account>`): the rest of
        // the key can be personal, so these show as a count per prefix.
        if let Some((prefix, _)) = k.split_once(':') {
            let group = format!("{prefix}:*");
            let n = settings.get(&group).and_then(Value::as_i64).unwrap_or(0);
            settings.insert(group, json!(n + 1));
            continue;
        }
        let shown = if SAFE_SETTINGS.contains(&k.as_str()) { json!(v) } else { json!("[set]") };
        settings.insert(k, shown);
    }

    Ok(json!({
        "schema": schema,
        "sizeBytes": pages * page_size,
        "integrity": integrity,
        "rows": rows,
        "bodyCacheBytes": body_bytes,
        "accounts": accounts,
        "outbox": outbox,
        "settings": settings,
    }))
}

fn log_dir(app: &AppHandle) -> PathBuf {
    match testenv::get("PEETMAIL_DATA_DIR") {
        Some(d) => PathBuf::from(d).join("logs"),
        None if tauri::is_dev() => crate::datadir::dev_dir().join("logs"),
        None => app.path().app_log_dir().unwrap_or_default(),
    }
}

/// The newest log files, scrubbed, within LOG_BUDGET (a file over the budget keeps its end).
pub fn log_files(dir: &Path) -> Vec<(String, String)> {
    let Ok(entries) = std::fs::read_dir(dir) else { return vec![] };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "log"))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    files.sort_by_key(|f| std::cmp::Reverse(f.0));
    let mut left = LOG_BUDGET;
    let mut out = vec![];
    for (_, path) in files {
        if left == 0 {
            break;
        }
        let Ok(bytes) = std::fs::read(&path) else { continue };
        let text = String::from_utf8_lossy(&bytes);
        let scrubbed: String = text.lines().map(|l| crate::scrub::scrub(l) + "\n").collect();
        let keep = if scrubbed.len() > left {
            let mut start = scrubbed.len() - left;
            while !scrubbed.is_char_boundary(start) {
                start += 1;
            }
            format!("[earlier lines dropped]\n{}", &scrubbed[start..])
        } else {
            scrubbed
        };
        left = left.saturating_sub(keep.len());
        out.push((path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), keep));
    }
    out
}

/// "Windows 11 Pro 24H2 (26100)" from the registry.
#[cfg(windows)]
fn os_version() -> String {
    let key = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
    let product = reg_string(key, "ProductName").unwrap_or_else(|| "Windows".into());
    let display = reg_string(key, "DisplayVersion").unwrap_or_default();
    let build = reg_string(key, "CurrentBuild").unwrap_or_default();
    // ProductName still says "Windows 10" on Windows 11; the build number tells them apart.
    let product = if build.parse::<u32>().is_ok_and(|b| b >= 22000) { product.replace("Windows 10", "Windows 11") } else { product };
    format!("{product} {display} ({build})").replace("  ", " ")
}

/// "Ubuntu 24.04.1 LTS (Linux 6.8.0-45-generic)" from os-release and the kernel.
#[cfg(target_os = "linux")]
fn os_version() -> String {
    let release = std::fs::read_to_string("/etc/os-release").or_else(|_| std::fs::read_to_string("/usr/lib/os-release")).unwrap_or_default();
    let kernel = std::fs::read_to_string("/proc/sys/kernel/osrelease").unwrap_or_default();
    describe_linux(&release, kernel.trim())
}

/// "Android 17 (API 37)" from the system properties.
#[cfg(target_os = "android")]
fn os_version() -> String {
    let prop = |name: &str| -> Option<String> {
        let name = std::ffi::CString::new(name).ok()?;
        let mut buf = [0 as libc::c_char; libc::PROP_VALUE_MAX as usize];
        // SAFETY: a NUL-terminated name and a buffer of PROP_VALUE_MAX bytes, as the call requires.
        let len = unsafe { libc::__system_property_get(name.as_ptr(), buf.as_mut_ptr()) };
        // SAFETY: the call NUL-terminates what it writes.
        (len > 0).then(|| unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) }.to_string_lossy().into_owned())
    };
    let release = prop("ro.build.version.release").unwrap_or_default();
    let sdk = prop("ro.build.version.sdk").unwrap_or_default();
    format!("Android {release} (API {sdk})")
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn describe_linux(os_release: &str, kernel: &str) -> String {
    let field = |name: &str| {
        os_release.lines().find_map(|l| l.strip_prefix(name)?.strip_prefix('=')).map(|v| v.trim().trim_matches(|c| c == '"' || c == '\'').to_string())
    };
    let distro = field("PRETTY_NAME").or_else(|| field("NAME")).filter(|n| !n.is_empty()).unwrap_or_else(|| "Linux".into());
    if kernel.is_empty() { distro } else { format!("{distro} (Linux {kernel})") }
}

/// Linux only: what the desktop is and how PeetMail was installed, which explain most
/// tray, keyring and rendering differences. None of it names the user or the PC.
fn linux_session() -> Option<Value> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    Some(json!({
        "desktop": var("XDG_CURRENT_DESKTOP"),
        "session": var("XDG_SESSION_TYPE"),
        "appImage": std::env::var_os("APPIMAGE").is_some(),
    }))
}

#[cfg(windows)]
fn reg_string(key: &str, value: &str) -> Option<String> {
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};
    let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let (k, v) = (wide(key), wide(value));
    let mut buf = vec![0u16; 256];
    let mut len = (buf.len() * 2) as u32;
    // SAFETY: NUL-terminated wide strings; `buf` holds `len` bytes.
    let rc = unsafe { RegGetValueW(HKEY_LOCAL_MACHINE, k.as_ptr(), v.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr().cast(), &mut len) };
    if rc != 0 {
        return None;
    }
    let chars = (len as usize / 2).saturating_sub(1);
    Some(String::from_utf16_lossy(&buf[..chars.min(buf.len())]))
}

// ---------- feedback ----------

/// Sends feedback to the developers (Sentry user feedback), with the diagnostics zip if
/// asked. Blocks until it's sent or a few seconds have passed.
pub fn send_feedback(app: &AppHandle, message: &str, email: Option<&str>, with_diagnostics: bool) -> Result<()> {
    let message = message.trim();
    if message.is_empty() {
        return Err(AppError::Other("Write what happened first.".into()));
    }
    let dsn = sentry_dsn().ok_or_else(|| AppError::Other("This build can't send feedback.".into()))?;
    let state = app.state::<AppState>();
    let install_id = state.db.with(|c| crate::crash::install_id(c))?;
    let attachment = if with_diagnostics { Some(diagnostics_zip(app)?) } else { None };
    let envelope = feedback_envelope(&FeedbackInput {
        dsn: &dsn,
        message,
        email: email.map(str::trim).filter(|e| !e.is_empty()),
        install_id: &install_id,
        release: &crate::crash::release(),
        environment: crate::crash::environment(),
        attachment: attachment.as_deref(),
    })?;

    let mut options = sentry::ClientOptions::new();
    options.dsn = Some(dsn.parse().map_err(|_| AppError::Other("feedback: bad DSN".into()))?);
    let client = Arc::new(sentry::Client::with_options(sentry::apply_defaults(options)));
    client.send_envelope(sentry::Envelope::from_bytes_raw(envelope).map_err(|e| AppError::Other(format!("feedback: {e}")))?);
    if !client.flush(Some(Duration::from_secs(10))) {
        return Err(AppError::Other("Couldn't send your feedback. Check your connection and try again.".into()));
    }
    log::info!("feedback sent{}", if with_diagnostics { " with diagnostics" } else { "" });
    Ok(())
}

pub struct FeedbackInput<'a> {
    pub dsn: &'a str,
    pub message: &'a str,
    pub email: Option<&'a str>,
    pub install_id: &'a str,
    pub release: &'a str,
    pub environment: &'a str,
    pub attachment: Option<&'a [u8]>,
}

/// A Sentry envelope holding a user-feedback item (and the zip as an attachment). The
/// Rust SDK has no feedback type yet, so it's written out here in Sentry's format.
pub fn feedback_envelope(f: &FeedbackInput) -> Result<Vec<u8>> {
    let mut id = [0u8; 16];
    getrandom::fill(&mut id).expect("OS RNG unavailable");
    let event_id: String = id.iter().map(|b| format!("{b:02x}")).collect();
    let now = chrono::Utc::now();
    let mut feedback = json!({ "message": f.message, "source": "peetmail-app" });
    if let Some(e) = f.email {
        feedback["contact_email"] = json!(e);
    }
    let event = json!({
        "event_id": event_id,
        "timestamp": now.timestamp_millis() as f64 / 1000.0,
        "platform": "other",
        "level": "info",
        "release": f.release,
        "environment": f.environment,
        "user": { "id": f.install_id },
        "contexts": { "feedback": feedback },
    });
    let mut out = Vec::new();
    writeln!(out, "{}", json!({ "event_id": event_id, "sent_at": now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true), "dsn": f.dsn }))?;
    let body = serde_json::to_vec(&event)?;
    writeln!(out, "{}", json!({ "type": "feedback", "length": body.len() }))?;
    out.extend_from_slice(&body);
    out.push(b'\n');
    if let Some(zip) = f.attachment {
        writeln!(out, "{}", json!({ "type": "attachment", "length": zip.len(), "filename": "peetmail-diagnostics.zip", "content_type": "application/zip" }))?;
        out.extend_from_slice(zip);
        out.push(b'\n');
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_is_described_by_its_os_release() {
        let ubuntu = "NAME=\"Ubuntu\"\nVERSION_ID=\"24.04\"\nPRETTY_NAME=\"Ubuntu 24.04.1 LTS\"\nID=ubuntu\n";
        assert_eq!(describe_linux(ubuntu, "6.8.0-45-generic"), "Ubuntu 24.04.1 LTS (Linux 6.8.0-45-generic)");
        assert_eq!(describe_linux("NAME='Arch Linux'\nID=arch\n", "6.11.5-arch1-1"), "Arch Linux (Linux 6.11.5-arch1-1)");
        assert_eq!(describe_linux("", ""), "Linux");
        // PRETTY_NAME_EXTRA isn't PRETTY_NAME.
        assert_eq!(describe_linux("PRETTY_NAME_EXTRA=x\nNAME=Fedora Linux\n", ""), "Fedora Linux");
    }

    #[test]
    fn the_database_summary_has_shape_not_content() {
        let db = db::Db::open_in_memory().unwrap();
        let v = db
            .with(|c| {
                let a = db::upsert_account(c, "microsoft", "priya@fabrikam.io", Some("Priya Nair"), b"t")?.id;
                db::set_account_app(c, a, "custom")?;
                db::set_setting(c, "cache_days", "90")?;
                db::set_setting(c, "split:boss", r#"{"name":"Boss","query":"from:boss@contoso.com","order":2}"#)?;
                db::set_setting(c, "google_client_secret", "GOCSPX-real-secret")?;
                db::set_setting(c, "signature:priya@fabrikam.io", r#"{"html":"<p>Priya Nair, +44 7700 900123</p>"}"#)?;
                database_summary(c)
            })
            .unwrap();
        let text = v.to_string();
        assert_eq!(v["integrity"], "ok");
        assert_eq!(v["accounts"], json!([{ "provider": "microsoft", "app": "custom", "signedIn": true }]));
        assert_eq!(v["settings"]["cache_days"], "90");
        assert_eq!(v["settings"]["split:*"], 2, "the default Other split and Boss, counted");
        assert_eq!(v["settings"]["signature:*"], 1);
        assert_eq!(v["settings"]["google_client_secret"], "[set]");
        for secret in ["priya@fabrikam.io", "Priya Nair", "boss@contoso.com", "GOCSPX-real-secret", "7700 900123"] {
            assert!(!text.contains(secret), "{secret} leaked into the summary");
        }
        assert!(v["rows"]["messages"].is_number());
    }

    #[test]
    fn logs_are_scrubbed_and_trimmed_newest_first() {
        let dir = std::env::temp_dir().join(format!("peetmail-logs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("old.log"), "sync failed for ann@contoso.com\n").unwrap();
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(dir.join("PeetMail.log"), "sync ok\n".repeat(LOG_BUDGET / 8 + 100) + "last line\n").unwrap();
        std::fs::write(dir.join("notes.txt"), "not a log").unwrap();
        let logs = log_files(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(logs.len(), 1, "the newest file fills the budget");
        assert_eq!(logs[0].0, "PeetMail.log");
        assert!(logs[0].1.starts_with("[earlier lines dropped]"));
        assert!(logs[0].1.ends_with("last line\n"));

        let dir2 = std::env::temp_dir().join(format!("peetmail-logs2-{}", std::process::id()));
        std::fs::create_dir_all(&dir2).unwrap();
        std::fs::write(dir2.join("a.log"), "sync failed for ann@contoso.com\n").unwrap();
        let logs = log_files(&dir2);
        let _ = std::fs::remove_dir_all(&dir2);
        assert_eq!(logs[0].1, "sync failed for [email]\n");
    }

    #[test]
    fn feedback_envelopes_are_well_formed() {
        let zip = b"PK\x03\x04zip-bytes";
        let env = feedback_envelope(&FeedbackInput {
            dsn: "https://k@o.ingest.sentry.io/1",
            message: "The inbox froze",
            email: Some("me@example.com"),
            install_id: "abc",
            release: "peetmail@0.2.0",
            environment: "canary",
            attachment: Some(zip),
        })
        .unwrap();
        let mut lines = env.splitn(3, |b| *b == b'\n');
        let header: Value = serde_json::from_slice(lines.next().unwrap()).unwrap();
        let item: Value = serde_json::from_slice(lines.next().unwrap()).unwrap();
        assert_eq!(item["type"], "feedback");
        let rest = lines.next().unwrap();
        let len = item["length"].as_u64().unwrap() as usize;
        let event: Value = serde_json::from_slice(&rest[..len]).unwrap();
        assert_eq!(event["event_id"], header["event_id"]);
        assert_eq!(event["contexts"]["feedback"]["message"], "The inbox froze");
        assert_eq!(event["contexts"]["feedback"]["contact_email"], "me@example.com");
        assert_eq!(event["user"]["id"], "abc");
        assert!(env.windows(zip.len()).any(|w| w == zip));
    }
}
