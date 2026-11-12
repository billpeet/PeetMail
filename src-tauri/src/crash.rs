//! Crash and error reports to Sentry: Rust panics, native crashes (minidumps, captured by a
//! small crash-reporter process), and errors from the webview (sent over IPC, so the
//! webview never talks to the network).
//!
//! Reports are on by default with an opt-out (Settings → Privacy). Every report goes
//! through `scrub` (addresses, names, tokens, IDs, paths) and is stripped of the host name;
//! `send_default_pii` stays off. Users are identified only by a random install ID.
//!
//! The DSN is compiled in by CI (`PEETMAIL_SENTRY_DSN`); without it (local builds) nothing
//! is sent. Debug builds can point at a fake server with the same variable at run time.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

#[cfg(desktop)]
use sentry::integrations::minidump::MinidumpIntegration;
use sentry::protocol::{Breadcrumb, Context, Event, Exception, Frame, Level, Mechanism, Stacktrace, User};
use sentry::ClientOptions;

use crate::db;
use crate::error::Result;
use crate::scrub::{scrub, scrub_value};
use crate::testenv;

pub const KEY_CRASH_REPORTS: &str = "crash_reports";
pub const KEY_INSTALL_ID: &str = "install_id";

/// The user's choice. Checked for every report, so turning reports off takes effect at once.
static ENABLED: AtomicBool = AtomicBool::new(true);
static GUARD: Mutex<Option<sentry::ClientInitGuard>> = Mutex::new(None);

/// The last log lines (scrubbed), attached to every report as breadcrumbs. Kept here rather
/// than in Sentry's scope because the SDK's scopes are per thread: lines logged on a sync
/// worker would never reach an error captured on the IPC thread.
static TRAIL: Mutex<VecDeque<Breadcrumb>> = Mutex::new(VecDeque::new());
const TRAIL_LEN: usize = 50;
/// Attached to every report for the same reason.
static INSTALL_ID: RwLock<Option<String>> = RwLock::new(None);

/// Wraps the app's logger: every line still goes to the log file, and info and above are
/// also kept (scrubbed) in the trail. Log lines never become reports of their own, so a
/// flaky network can't flood Sentry with sync errors.
pub struct TrailLogger(pub Box<dyn log::Log>);

impl log::Log for TrailLogger {
    fn enabled(&self, md: &log::Metadata<'_>) -> bool {
        self.0.enabled(md)
    }

    fn log(&self, record: &log::Record<'_>) {
        self.0.log(record);
        crate::telemetry::export_log(record);
        if record.level() <= log::Level::Info {
            let level = match record.level() {
                log::Level::Error => Level::Error,
                log::Level::Warn => Level::Warning,
                _ => Level::Info,
            };
            let crumb = Breadcrumb {
                ty: "default".into(),
                category: Some(record.target().to_string()),
                level,
                message: Some(scrub(&record.args().to_string())),
                ..Default::default()
            };
            let mut trail = TRAIL.lock().unwrap_or_else(|e| e.into_inner());
            if trail.len() == TRAIL_LEN {
                trail.pop_front();
            }
            trail.push_back(crumb);
        }
    }

    fn flush(&self) {
        self.0.flush();
    }
}

/// Adds the shared trail and install ID, then scrubs. Runs for every report.
fn prepare(event: &mut Event<'static>) {
    if event.breadcrumbs.values.is_empty() {
        event.breadcrumbs.values = TRAIL.lock().unwrap_or_else(|e| e.into_inner()).iter().cloned().collect();
    }
    if let Some(id) = INSTALL_ID.read().unwrap_or_else(|e| e.into_inner()).clone() {
        event.user = Some(User { id: Some(id), ..Default::default() });
    }
    scrub_event(event);
}

fn dsn() -> Option<String> {
    testenv::get("PEETMAIL_SENTRY_DSN").or_else(|| option_env!("PEETMAIL_SENTRY_DSN").map(String::from)).filter(|d| !d.trim().is_empty())
}

pub fn release() -> String {
    format!("peetmail@{}", env!("CARGO_PKG_VERSION"))
}

pub fn environment() -> &'static str {
    option_env!("PEETMAIL_CHANNEL").filter(|c| !c.is_empty()).unwrap_or("dev")
}

/// Starts reporting, first thing in `run()`. Tauri isn't up yet, so the opt-out is read
/// straight from the database file. The crash reporter re-runs this binary, and this is
/// where that copy stops: `sentry::init` never returns in it.
pub fn init_early(db_path: &Path) {
    let enabled = read_choice(db_path).unwrap_or(true);
    ENABLED.store(enabled, Ordering::SeqCst);
    if enabled {
        start(true);
    }
}

fn read_choice(db_path: &Path) -> Option<bool> {
    let conn = rusqlite::Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
    let v: Option<String> = conn.query_row("SELECT value FROM settings WHERE key = ?1", [KEY_CRASH_REPORTS], |r| r.get(0)).ok();
    Some(v.as_deref() != Some("off"))
}

fn start(minidumps: bool) {
    let Some(dsn) = dsn() else { return };
    let Ok(dsn) = dsn.parse() else {
        log::error!("crash reports: the configured DSN doesn't parse");
        return;
    };
    let mut options = ClientOptions::new();
    options.dsn = Some(dsn);
    options.release = Some(release().into());
    options.environment = Some(environment().into());
    options.send_default_pii = false;
    options.max_breadcrumbs = 50;
    options.before_send = Some(Arc::new(|mut event| {
        if !ENABLED.load(Ordering::SeqCst) {
            return None;
        }
        prepare(&mut event);
        Some(event)
    }));
    options.before_breadcrumb = Some(Arc::new(|mut b| {
        scrub_breadcrumb(&mut b);
        Some(b)
    }));
    // Android has no second process to catch a native crash with; panics are still reported.
    #[cfg(desktop)]
    if minidumps {
        options = options.add_integration(MinidumpIntegration::new().process_name("PeetMail crash reporter"));
    }
    #[cfg(mobile)]
    let _ = minidumps;
    #[cfg(target_os = "android")]
    match crate::android::tls_client() {
        Ok(client) => options.transport = Some(Arc::new(AndroidTransport(client))),
        Err(e) => {
            // Sentry's own client would abort the process on Android (android::tls_client).
            log::error!("crash reports: off, {e}");
            return;
        }
    }
    *GUARD.lock().unwrap_or_else(|e| e.into_inner()) = Some(sentry::init(options));
}

/// Sentry's reqwest transport, on the client from `android::tls_client`.
#[cfg(target_os = "android")]
struct AndroidTransport(reqwest_013::Client);

#[cfg(target_os = "android")]
impl sentry::TransportFactory for AndroidTransport {
    fn create_transport_with_options(&self, options: sentry::TransportOptions) -> Arc<dyn sentry::Transport> {
        Arc::new(sentry::transports::ReqwestHttpTransportOptions::from(options).with_client(self.0.clone()).build())
    }
}

/// After the database opens: the install ID, the saved choice, and the values to strip.
pub fn after_db(db: &db::Db) {
    let (install_id, enabled) = match db.with(|c| Ok((install_id(c)?, db::get_setting(c, KEY_CRASH_REPORTS)?.as_deref() != Some("off")))) {
        Ok(v) => v,
        Err(e) => {
            log::error!("crash reports: reading settings: {e}");
            return;
        }
    };
    ENABLED.store(enabled, Ordering::SeqCst);
    *INSTALL_ID.write().unwrap_or_else(|e| e.into_inner()) = Some(install_id.clone());
    let user = User { id: Some(install_id), ..Default::default() };
    sentry::configure_scope(|s| s.set_user(Some(user.clone())));
    #[cfg(desktop)]
    sentry::with_integration(|m: &MinidumpIntegration, _| m.set_user(Some(user.clone())));
    refresh_known(db);
}

/// A random ID for this install, made on first use. Lets someone point us at their reports
/// without saying who they are.
pub fn install_id(conn: &rusqlite::Connection) -> Result<String> {
    if let Some(id) = db::get_setting(conn, KEY_INSTALL_ID)? {
        return Ok(id);
    }
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).expect("OS RNG unavailable");
    let id: String = b.iter().map(|x| format!("{x:02x}")).collect();
    db::set_setting(conn, KEY_INSTALL_ID, &id)?;
    Ok(id)
}

/// Accounts' addresses and names, for the scrubber. Call after accounts change.
pub fn refresh_known(db: &db::Db) {
    match db.with(|c| db::list_accounts(c)) {
        Ok(accounts) => crate::scrub::set_known(accounts.into_iter().flat_map(|a| [Some(a.email), a.display_name]).flatten()),
        Err(e) => log::error!("crash reports: listing accounts: {e}"),
    }
}

/// Whether this build sends reports at all (a DSN is compiled in, or set for tests).
pub fn configured() -> bool {
    dsn().is_some()
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::SeqCst)
}

/// Turns reports on or off now. Turning them on in a session that started with them off
/// starts the client without native-crash capture (that needs a restart).
pub fn set_enabled(db: &db::Db, on: bool) -> Result<()> {
    db.with(|c| db::set_setting(c, KEY_CRASH_REPORTS, if on { "on" } else { "off" }))?;
    ENABLED.store(on, Ordering::SeqCst);
    if on && sentry::Hub::current().client().is_none() {
        start(false);
        after_db(db);
    }
    Ok(())
}

/// Sends what's queued, briefly. Called on exit and before the updater closes the app.
pub fn flush() {
    if let Some(client) = sentry::Hub::current().client() {
        client.flush(Some(Duration::from_secs(2)));
    }
}

/// An error the webview couldn't handle (window.onerror, an unhandled rejection).
pub fn capture_js_error(name: &str, message: &str, stack: Option<&str>, mechanism: &str) {
    let frames = parse_js_stack(stack.unwrap_or_default());
    let exception = Exception {
        ty: if name.trim().is_empty() { "Error".into() } else { name.into() },
        value: Some(message.into()),
        stacktrace: (!frames.is_empty()).then(|| Stacktrace { frames, ..Default::default() }),
        mechanism: Some(Mechanism { ty: mechanism.into(), handled: Some(false), ..Default::default() }),
        ..Default::default()
    };
    let mut event = Event { level: Level::Error, platform: "javascript".into(), exception: vec![exception].into(), ..Default::default() };
    event.tags.insert("source".into(), "webview".into());
    sentry::capture_event(event);
}

/// V8 stack lines ("    at fn (url:line:col)" or "    at url:line:col") as Sentry frames,
/// oldest first. Source maps uploaded for the release turn them back into Svelte/TS.
pub fn parse_js_stack(stack: &str) -> Vec<Frame> {
    static LINE: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"^\s*at (?:(.+?) \()?(.+?):(\d+):(\d+)\)?\s*$").expect("stack regex"));
    let mut frames: Vec<Frame> = stack
        .lines()
        .filter_map(|l| LINE.captures(l))
        .map(|c| Frame {
            function: c.get(1).map(|m| m.as_str().to_string()),
            abs_path: Some(c[2].to_string()),
            lineno: c[3].parse().ok(),
            colno: c[4].parse().ok(),
            in_app: Some(true),
            ..Default::default()
        })
        .collect();
    frames.reverse();
    frames
}

/// Strips what identifies the user or the machine from an event, in place.
pub fn scrub_event(e: &mut Event<'static>) {
    e.server_name = None;
    e.request = None;
    e.user = e.user.take().map(|u| User { id: u.id, ..Default::default() });
    if let Some(m) = &mut e.message {
        *m = scrub(m);
    }
    if let Some(l) = &mut e.logentry {
        l.message = scrub(&l.message);
        l.params.iter_mut().for_each(scrub_value);
    }
    for ex in e.exception.values.iter_mut() {
        if let Some(v) = &mut ex.value {
            *v = scrub(v);
        }
        if let Some(st) = &mut ex.stacktrace {
            scrub_frames(&mut st.frames);
        }
    }
    for t in e.threads.values.iter_mut() {
        if let Some(st) = &mut t.stacktrace {
            scrub_frames(&mut st.frames);
        }
        t.name = t.name.as_deref().map(scrub);
    }
    e.breadcrumbs.values.iter_mut().for_each(scrub_breadcrumb);
    e.extra.values_mut().for_each(scrub_value);
    e.tags.values_mut().for_each(|v| *v = scrub(v));
    for c in e.contexts.values_mut() {
        if let Context::Device(d) = c {
            // The device name is the PC's name.
            d.name = None;
        }
    }
}

fn scrub_frames(frames: &mut [Frame]) {
    for f in frames {
        f.abs_path = f.abs_path.as_deref().map(scrub);
        f.filename = f.filename.as_deref().map(scrub);
        f.vars.clear();
    }
}

fn scrub_breadcrumb(b: &mut Breadcrumb) {
    b.message = b.message.as_deref().map(scrub);
    b.data.values_mut().for_each(scrub_value);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn js_stacks_become_frames_oldest_first() {
        let stack = "TypeError: x is undefined\n    at render (http://tauri.localhost/_app/immutable/nodes/2.js:1:2345)\n    at http://tauri.localhost/_app/immutable/chunks/a.js:3:4\n    not a frame";
        let f = parse_js_stack(stack);
        assert_eq!(f.len(), 2);
        assert_eq!(f[0].abs_path.as_deref(), Some("http://tauri.localhost/_app/immutable/chunks/a.js"));
        assert_eq!((f[0].lineno, f[0].colno, f[0].function.as_deref()), (Some(3), Some(4), None));
        assert_eq!(f[1].function.as_deref(), Some("render"));
        assert_eq!((f[1].lineno, f[1].colno), (Some(1), Some(2345)));
    }

    #[test]
    fn events_lose_host_user_details_and_personal_text() {
        let mut e = Event {
            server_name: Some("BORIS-DESKTOP".into()),
            message: Some("failed to send to priya@fabrikam.io".into()),
            user: Some(User { id: Some("abc".into()), email: Some("me@x.com".into()), ip_address: None, ..Default::default() }),
            exception: vec![Exception {
                ty: "Error".into(),
                value: Some(r"open C:\Users\marti\Downloads\Q4.pdf: denied".into()),
                ..Default::default()
            }]
            .into(),
            ..Default::default()
        };
        e.contexts.insert("device".into(), Context::Device(Box::new(sentry::protocol::DeviceContext { name: Some("BORIS-DESKTOP".into()), ..Default::default() })));
        e.breadcrumbs.values.push(Breadcrumb { message: Some("GET https://x/y?token=abc".into()), ..Default::default() });
        scrub_event(&mut e);
        assert_eq!(e.server_name, None);
        assert_eq!(e.message.as_deref(), Some("failed to send to [email]"));
        assert_eq!(e.user, Some(User { id: Some("abc".into()), ..Default::default() }));
        assert_eq!(e.exception.values[0].value.as_deref(), Some(r"open C:\Users\[user]\Downloads\Q4.pdf: denied"));
        assert_eq!(e.breadcrumbs.values[0].message.as_deref(), Some("GET https://x/y?[query]"));
        let Some(Context::Device(d)) = e.contexts.get("device") else { panic!("device context kept") };
        assert_eq!(d.name, None);
    }

    struct Discard;
    impl log::Log for Discard {
        fn enabled(&self, _: &log::Metadata<'_>) -> bool {
            true
        }
        fn log(&self, _: &log::Record<'_>) {}
        fn flush(&self) {}
    }

    #[test]
    fn log_lines_become_a_scrubbed_trail_on_every_event() {
        use log::Log;
        let logger = TrailLogger(Box::new(Discard));
        let line = |level, msg: &str| logger.log(&log::Record::builder().level(level).target("peetmail_lib::mail").args(format_args!("{msg}")).build());
        line(log::Level::Debug, "debug lines stay local");
        for i in 0..TRAIL_LEN {
            line(log::Level::Info, &format!("filler {i}"));
        }
        line(log::Level::Warn, "sync failed for ann@contoso.com");
        *INSTALL_ID.write().unwrap() = Some("install-1".into());

        let mut e = Event::default();
        prepare(&mut e);
        let crumbs = &e.breadcrumbs.values;
        assert_eq!(crumbs.len(), TRAIL_LEN, "capped");
        assert_eq!(crumbs.last().unwrap().message.as_deref(), Some("sync failed for [email]"));
        assert_eq!(crumbs.last().unwrap().level, Level::Warning);
        assert!(crumbs.iter().all(|c| c.message.as_deref() != Some("debug lines stay local")));
        assert_eq!(e.user.and_then(|u| u.id).as_deref(), Some("install-1"));
    }

    #[test]
    fn install_id_is_made_once() {
        let db = db::Db::open_in_memory().unwrap();
        let (a, b) = db.with(|c| Ok((install_id(c)?, install_id(c)?))).unwrap();
        assert_eq!(a, b);
        assert_eq!(a.len(), 32);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
