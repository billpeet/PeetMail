//! Startup health checks: things outside PeetMail that make it fail in confusing ways,
//! found early and explained in a banner (see `+page.svelte`), each counted in telemetry.
//!
//! - the database after a migration (`PRAGMA quick_check`, on its own read-only connection
//!   so a big file doesn't hold up the app)
//! - free space on the data folder's drive
//! - whether the data folder can be written to
//! - the PC's clock against a server's `Date` header (sign-ins fail when it's well off)
//! - the WebView2 runtime's version (Windows; on Linux WebKitGTK comes with the distro or
//!   the AppImage, and its version numbers aren't comparable)
//!
//! They run at startup and again every few hours (the app lives in the tray for days).
//! Measuring is separate from deciding (`evaluate`), so the decisions are unit-tested and
//! E2E tests can stand in for a measurement with `PEETMAIL_FAKE_HEALTH`.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::state::AppState;
use crate::telemetry::{attr, Key};
use crate::testenv;

pub const EVENT_HEALTH: &str = "health-changed";
const RECHECK: Duration = Duration::from_secs(6 * 3600);

/// Below this, saving new mail can start failing.
const LOW_DISK_BYTES: u64 = 500 * 1024 * 1024;
/// Entra and Google reject tokens a few minutes out; Windows usually keeps within seconds.
const CLOCK_TOLERANCE_SECS: i64 = 5 * 60;
/// The oldest WebView2 PeetMail is tested on (it updates itself unless something blocks it).
const MIN_WEBVIEW_MAJOR: u32 = 120;

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Issue {
    /// Stable id, also the telemetry value: db_integrity, disk_space, data_dir, clock, webview.
    pub id: &'static str,
    /// "error" (something is or will be broken) or "warning".
    pub severity: &'static str,
    pub title: String,
    pub detail: String,
}

/// What was measured. None: couldn't tell (offline, API failed), which is never an issue.
#[derive(Default, Debug)]
pub struct Inputs {
    /// `quick_check`'s first line, when this run migrated the database.
    pub db_check: Option<String>,
    pub free_bytes: Option<u64>,
    /// Err(reason) when a test file couldn't be written.
    pub writable: Option<std::result::Result<(), String>>,
    pub data_dir: PathBuf,
    /// Server time minus PC time.
    pub clock_skew_secs: Option<i64>,
    pub webview: Option<String>,
}

static ISSUES: Mutex<Vec<Issue>> = Mutex::new(Vec::new());

pub fn current() -> Vec<Issue> {
    ISSUES.lock().unwrap_or_else(|p| p.into_inner()).clone()
}

pub fn evaluate(i: &Inputs) -> Vec<Issue> {
    let mut out = vec![];
    if let Some(check) = i.db_check.as_deref().filter(|c| *c != "ok") {
        out.push(Issue {
            id: "db_integrity",
            severity: "error",
            title: "PeetMail's mail cache may be damaged".into(),
            detail: format!("It failed a check after the last update ({}). Your mail is safe on the server; Help → Troubleshooting says how to start the cache afresh.", short(check)),
        });
    }
    if let Some(Err(why)) = &i.writable {
        out.push(Issue {
            id: "data_dir",
            severity: "error",
            title: "PeetMail can't save to its data folder".into(),
            detail: format!("{} ({}). New mail and changes won't be kept. Check the folder isn't read-only or blocked by antivirus.", i.data_dir.display(), short(why)),
        });
    }
    if let Some(free) = i.free_bytes.filter(|f| *f < LOW_DISK_BYTES) {
        out.push(Issue {
            id: "disk_space",
            severity: "warning",
            title: format!("Your disk is almost full ({} MB free)", free / (1024 * 1024)),
            detail: "PeetMail may not be able to save new mail. Free up some space, or keep less mail on this PC under Settings → Storage.".into(),
        });
    }
    if let Some(skew) = i.clock_skew_secs.filter(|s| s.abs() > CLOCK_TOLERANCE_SECS) {
        out.push(Issue {
            id: "clock",
            severity: "warning",
            title: format!("Your PC's clock is {} {}", describe_duration(skew.unsigned_abs()), if skew > 0 { "slow" } else { "fast" }),
            detail: "Sign-ins can fail and times can look wrong. In Windows, turn on Settings → Time & language → Date & time → Set time automatically.".into(),
        });
    }
    if let Some(v) = &i.webview {
        let major = v.split('.').next().and_then(|m| m.parse::<u32>().ok());
        if major.is_some_and(|m| m < MIN_WEBVIEW_MAJOR) {
            out.push(Issue {
                id: "webview",
                severity: "warning",
                title: format!("Microsoft Edge WebView2 is out of date ({v})"),
                detail: format!("PeetMail needs version {MIN_WEBVIEW_MAJOR} or later, and parts of it may not work. Run Windows Update, or install the WebView2 Runtime from Microsoft."),
            });
        }
    }
    out
}

fn short(s: &str) -> String {
    let s = s.lines().next().unwrap_or_default().trim();
    if s.chars().count() > 80 { format!("{}…", s.chars().take(80).collect::<String>()) } else { s.to_string() }
}

fn describe_duration(secs: u64) -> String {
    match secs {
        s if s >= 2 * 86400 => format!("{} days", s / 86400),
        s if s >= 2 * 3600 => format!("{} hours", s / 3600),
        s => format!("{} minutes", s / 60),
    }
}

/// Runs the checks now and then every few hours, in the background.
pub fn start(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut first = true;
        loop {
            let inputs = measure(&app, first).await;
            first = false;
            let issues = evaluate(&inputs);
            log::info!("health: checked, {} issue(s); clock {}", issues.len(), inputs.clock_skew_secs.map_or("unknown".into(), |s| format!("{s}s off")));
            for issue in &issues {
                log::warn!("health: {}", issue.id);
                crate::telemetry::count("peetmail.health.issues", 1, &[attr(Key::Check, issue.id)]);
            }
            let changed = {
                let mut cur = ISSUES.lock().unwrap_or_else(|p| p.into_inner());
                let changed = *cur != issues;
                *cur = issues.clone();
                changed
            };
            if changed {
                let _ = app.emit(EVENT_HEALTH, &issues);
            }
            tokio::time::sleep(RECHECK).await;
        }
    });
}

async fn measure(app: &AppHandle, first: bool) -> Inputs {
    let state = app.state::<AppState>();
    let fake = fakes();
    let data_dir = state.db.path().and_then(Path::parent).map(Path::to_path_buf).unwrap_or_default();

    // The migration only happened once, at startup.
    let db_check = match (fake.get("db_check"), first, state.db.migrated_from(), state.db.path()) {
        (Some(f), ..) => Some(f.clone()),
        (None, true, Some(from), Some(path)) => {
            let path = path.to_path_buf();
            let result = tauri::async_runtime::spawn_blocking(move || quick_check(&path)).await.unwrap_or_else(|e| e.to_string());
            log::info!("health: database check after migrating from schema {from}: {}", short(&result));
            Some(result)
        }
        _ => None,
    };
    let free_bytes = match fake.get("disk_free_mb") {
        Some(mb) => mb.parse::<u64>().ok().map(|m| m * 1024 * 1024),
        None => free_space(&data_dir),
    };
    let writable = match fake.get("readonly") {
        Some(_) => Some(Err("Access is denied. (os error 5)".into())),
        None => Some(can_write(&data_dir)),
    };
    let clock_skew_secs = match fake.get("clock_skew_secs") {
        Some(s) => s.parse().ok(),
        None => clock_skew(&state.http).await,
    };
    let webview = fake.get("webview").cloned().or_else(|| if cfg!(windows) { tauri::webview_version().ok() } else { None });
    Inputs { db_check, free_bytes, writable, data_dir, clock_skew_secs, webview }
}

/// `PEETMAIL_FAKE_HEALTH="disk_free_mb=50;clock_skew_secs=3600;webview=100.0.0.0;db_check=…;readonly=1"`
fn fakes() -> std::collections::HashMap<String, String> {
    testenv::get("PEETMAIL_FAKE_HEALTH")
        .unwrap_or_default()
        .split(';')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect()
}

/// `quick_check`'s first line ("ok" when healthy), on a read-only connection of its own.
pub fn quick_check(path: &Path) -> String {
    let run = || -> rusqlite::Result<String> {
        let c = rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        c.query_row("PRAGMA quick_check", [], |r| r.get(0))
    };
    run().unwrap_or_else(|e| e.to_string())
}

#[cfg(windows)]
pub fn free_space(dir: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let mut free = 0u64;
    // SAFETY: a NUL-terminated path; the out pointer is a valid u64 (the others may be null).
    let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, std::ptr::null_mut(), std::ptr::null_mut()) };
    (ok != 0).then_some(free)
}

/// Space an unprivileged process can use (`f_bavail`, not the root-reserved `f_bfree`).
/// Android's bionic has the same call.
#[cfg(any(target_os = "linux", target_os = "android"))]
pub fn free_space(dir: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(dir.as_os_str().as_bytes()).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: a NUL-terminated path and a correctly sized out-parameter.
    let rc = unsafe { libc::statvfs(path.as_ptr(), &mut st) };
    #[allow(clippy::unnecessary_cast)] // the field types differ between libcs
    (rc == 0).then(|| st.f_bavail as u64 * st.f_frsize as u64)
}

pub fn can_write(dir: &Path) -> std::result::Result<(), String> {
    let probe = dir.join(".peetmail-write-test");
    std::fs::write(&probe, b"ok").map_err(|e| e.to_string())?;
    std::fs::remove_file(&probe).map_err(|e| e.to_string())
}

/// Server time minus PC time, from the `Date` header of any reply from Microsoft Graph
/// (no sign-in needed; nothing is sent but the request line). None when offline.
async fn clock_skew(http: &reqwest::Client) -> Option<i64> {
    let sent = chrono::Utc::now();
    let resp = http.head(crate::graph::base()).timeout(Duration::from_secs(10)).send().await.ok()?;
    let received = chrono::Utc::now();
    let date = resp.headers().get(reqwest::header::DATE)?.to_str().ok()?;
    skew_from_date(date, sent + (received - sent) / 2)
}

pub fn skew_from_date(header: &str, local: chrono::DateTime<chrono::Utc>) -> Option<i64> {
    let server = chrono::DateTime::parse_from_rfc2822(header).ok()?;
    Some(server.timestamp() - local.timestamp())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn healthy() -> Inputs {
        Inputs {
            db_check: Some("ok".into()),
            free_bytes: Some(50 * 1024 * 1024 * 1024),
            writable: Some(Ok(())),
            data_dir: PathBuf::from(r"C:\data"),
            clock_skew_secs: Some(2),
            webview: Some("130.0.2849.80".into()),
        }
    }

    #[test]
    fn a_healthy_pc_and_an_unknown_one_have_no_issues() {
        assert!(evaluate(&healthy()).is_empty());
        assert!(evaluate(&Inputs::default()).is_empty());
    }

    #[test]
    fn each_problem_is_its_own_issue() {
        let ids = |i: &Inputs| evaluate(i).into_iter().map(|x| x.id).collect::<Vec<_>>();
        assert_eq!(ids(&Inputs { db_check: Some("*** in database main ***\nPage 12: btreeInitPage() returns error code 11".into()), ..healthy() }), ["db_integrity"]);
        assert_eq!(ids(&Inputs { writable: Some(Err("Access is denied. (os error 5)".into())), ..healthy() }), ["data_dir"]);
        assert_eq!(ids(&Inputs { free_bytes: Some(100 * 1024 * 1024), ..healthy() }), ["disk_space"]);
        assert_eq!(ids(&Inputs { clock_skew_secs: Some(-600), ..healthy() }), ["clock"]);
        assert_eq!(ids(&Inputs { clock_skew_secs: Some(299), ..healthy() }), Vec::<&str>::new());
        assert_eq!(ids(&Inputs { webview: Some("109.0.1518.78".into()), ..healthy() }), ["webview"]);
    }

    #[test]
    fn messages_say_how_far_off_and_how_full() {
        let clock = evaluate(&Inputs { clock_skew_secs: Some(3 * 3600 + 5), ..healthy() });
        assert_eq!(clock[0].title, "Your PC's clock is 3 hours slow");
        let clock = evaluate(&Inputs { clock_skew_secs: Some(-20 * 60), ..healthy() });
        assert_eq!(clock[0].title, "Your PC's clock is 20 minutes fast");
        let disk = evaluate(&Inputs { free_bytes: Some(120 * 1024 * 1024), ..healthy() });
        assert_eq!(disk[0].title, "Your disk is almost full (120 MB free)");
    }

    #[test]
    fn the_measurements_work_on_this_pc() {
        let dir = std::env::temp_dir();
        assert!(free_space(&dir).is_some_and(|f| f > 0));
        assert_eq!(can_write(&dir), Ok(()));
        assert!(can_write(&dir.join("no-such-folder-peetmail")).is_err());

        let db = dir.join(format!("peetmail-health-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&db);
        rusqlite::Connection::open(&db).unwrap().execute_batch("CREATE TABLE t (x); INSERT INTO t VALUES (1);").unwrap();
        assert_eq!(quick_check(&db), "ok");
        let _ = std::fs::remove_file(&db);
        assert_ne!(quick_check(&dir.join("missing-peetmail.db")), "ok");
    }

    #[test]
    fn clock_skew_comes_from_the_date_header() {
        let local = chrono::DateTime::parse_from_rfc3339("2026-10-28T02:00:00Z").unwrap().to_utc();
        assert_eq!(skew_from_date("Wed, 28 Oct 2026 02:10:00 GMT", local), Some(600));
        assert_eq!(skew_from_date("Wed, 28 Oct 2026 01:59:30 GMT", local), Some(-30));
        assert_eq!(skew_from_date("not a date", local), None);
    }
}
