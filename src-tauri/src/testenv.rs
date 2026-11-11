//! Debug-build-only overrides used by the test harness (tests/harness).
//! Release builds ignore every one of these, so a shipped binary always talks to
//! Microsoft and stores data in the standard locations.
//!
//! | Variable                      | Effect                                                   |
//! |-------------------------------|----------------------------------------------------------|
//! | PEETMAIL_DATA_DIR             | SQLite + logs go here instead of %APPDATA%               |
//! | PEETMAIL_GRAPH_URL            | Graph base URL, e.g. http://127.0.0.1:4010/v1.0          |
//! | PEETMAIL_AUTHORITY_URL        | OAuth authority, e.g. http://127.0.0.1:4010/oauth        |
//! | PEETMAIL_GMAIL_URL            | Gmail API base, e.g. http://127.0.0.1:4011/gmail/v1/users/me |
//! | PEETMAIL_GCAL_URL             | Google Calendar API base (…/calendar/v3)                 |
//! | PEETMAIL_GOOGLE_AUTH_URL      | Google OAuth base (serves /authorize and /token)         |
//! | PEETMAIL_AUTH_AUTOFOLLOW=1    | Follow the authorize URL with reqwest, not a browser     |
//! | PEETMAIL_SYNC_INTERVAL_SECS   | Background sync interval (default 60)                    |
//! | PEETMAIL_HEADLESS=1           | Window hidden, no taskbar entry, never takes focus       |
//! | PEETMAIL_DOWNLOAD_DIR         | Attachments and diagnostics save here; Explorer isn't opened |
//! | PEETMAIL_FAKE_HEALTH          | Health check stand-ins: `disk_free_mb=50;clock_skew_secs=3600;webview=100.0;db_check=…;readonly=1` |
//! | PEETMAIL_FEEDBACK_EMAIL       | Report a problem's "Prefer email?" address               |
//! | PEETMAIL_UNDO_SEND_SECS       | Default undo-send delay (a saved setting still wins)     |
//! | PEETMAIL_OPEN_LOG             | Links and attachments the UI opens are appended here, not opened |
//! | PEETMAIL_NOTIFY_LOG           | Toasts are appended here as JSON lines, not shown        |
//! | PEETMAIL_FAKE_AUTOSTART=1     | "Start with Windows" lives in settings, not the registry |
//! | PEETMAIL_UPDATE_URL           | Update feeds: <url>/<channel>/latest.json (else GitHub)   |
//! | PEETMAIL_UPDATE_FIRST_CHECK_SECS | First background update check after this (default 60) |
//! | PEETMAIL_FAKE_UPDATE_INSTALL  | Installing an update writes {version, bytes} to this file |
//! | PEETMAIL_SENTRY_DSN           | Crash reports go here (e.g. the fake Sentry server)       |
//! | PEETMAIL_OTLP_ENDPOINT        | Telemetry goes here (OTLP/HTTP JSON, e.g. the fake collector) |
//! | PEETMAIL_OTLP_HEADERS         | Headers for it, `key=value,key=value`                     |
//! | PEETMAIL_OTLP_METRIC_INTERVAL_MS | Metric export interval (default 30s)                   |
//! | PEETMAIL_WEBVIEW2_ARGS        | WebView2 browser arguments, replacing Tauri's (DevTools port) |
//! | PEETMAIL_WINDOW_SIZE          | Window size `412x915`, below the usual minimum: the phone layout (`pnpm drive start --phone`) |
//! | PEETMAIL_BUILTIN_MS_CLIENT_ID | Act as if this Microsoft app were built in                |
//! | PEETMAIL_BUILTIN_GOOGLE_CLIENT_ID / _SECRET | Act as if this Google client were built in  |

pub fn get(name: &str) -> Option<String> {
    if cfg!(debug_assertions) {
        std::env::var(name).ok().filter(|v| !v.is_empty())
    } else {
        None
    }
}

/// `PEETMAIL_WINDOW_SIZE=412x915` as (width, height). Desktop only: a phone's window is the screen.
#[cfg(desktop)]
pub fn window_size() -> Option<(f64, f64)> {
    let v = get("PEETMAIL_WINDOW_SIZE")?;
    let (w, h) = v.split_once('x')?;
    Some((w.trim().parse().ok()?, h.trim().parse().ok()?))
}

pub fn flag(name: &str) -> bool {
    get(name).is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}
