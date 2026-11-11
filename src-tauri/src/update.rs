//! Automatic updates from GitHub Releases, on one of two channels:
//!
//! - **stable**: tagged releases, read from the repo's latest release
//!   (`releases/latest/download/latest.json`).
//! - **canary**: a build of every push to `master`, read from the fixed `canary` pre-release
//!   (`releases/download/canary/latest.json`), which CI repoints after each build.
//!
//! Updates are checked in the background, downloaded and signature-checked straight away,
//! and installed only when the user asks (footer button or `Ctrl+Shift+U`). Builds without
//! a repo compiled in (local and dev builds) never check.
//!
//! On Linux only the AppImage updates itself: the new AppImage replaces the file in place
//! (no root needed) and PeetMail relaunches. Any other install (built from source, a distro
//! package) is left to whatever installed it.
//!
//! Android has no Tauri updater: the APK installer takes its place (docs/ANDROID.md,
//! Stage 6). Until then updates there are off, with the reason shown in Settings.

use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
#[cfg(desktop)]
use tauri_plugin_updater::{Update, UpdaterExt};
use url::Url;

use crate::db;
use crate::error::{AppError, Result};
use crate::outbox;
use crate::state::AppState;
use crate::testenv;

pub const EVENT_UPDATE_STATUS: &str = "update-status";
const KEY_CHANNEL: &str = "update_channel";
pub const CHANNELS: [&str; 2] = ["stable", "canary"];

/// First check this long after start (sync gets the network first), then every few hours.
const FIRST_CHECK_SECS: u64 = 60;
const CHECK_EVERY_SECS: u64 = 6 * 60 * 60;

/// What the UI shows. `state` is one of disabled, idle, checking, downloading, ready,
/// upToDate, error.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    /// Increases with every change, so the UI can drop a status older than one it has.
    pub seq: u64,
    pub state: &'static str,
    pub channel: String,
    pub current_version: String,
    /// The version found (downloading or ready).
    pub version: Option<String>,
    pub notes: Option<String>,
    pub message: Option<String>,
    /// When the last check finished (RFC 3339, UTC).
    pub last_check: Option<String>,
}

struct Ready {
    update: Update,
    bytes: Vec<u8>,
    channel: String,
}

#[derive(Default)]
pub struct Updates {
    status: Mutex<UpdateStatus>,
    ready: Mutex<Option<Ready>>,
    checking: tokio::sync::Mutex<()>,
}

/// The channel this binary was built for (set by CI); `None` for local builds.
fn build_channel() -> Option<&'static str> {
    option_env!("PEETMAIL_CHANNEL").filter(|c| CHANNELS.contains(c))
}

/// The saved choice, else the channel the build came from, else stable.
pub fn resolve_channel(saved: Option<&str>, build: Option<&str>) -> &'static str {
    let pick = |c: Option<&str>| c.and_then(|c| CHANNELS.iter().copied().find(|k| *k == c));
    pick(saved).or(pick(build)).unwrap_or("stable")
}

/// Where a channel's manifest lives. `test_base` (debug builds only) replaces GitHub.
pub fn feed_url(channel: &str, repo: Option<&str>, test_base: Option<&str>) -> Option<Url> {
    let url = if let Some(base) = test_base {
        format!("{}/{channel}/latest.json", base.trim_end_matches('/'))
    } else {
        let repo = repo.filter(|r| r.contains('/'))?;
        match channel {
            "canary" => format!("https://github.com/{repo}/releases/download/canary/latest.json"),
            _ => format!("https://github.com/{repo}/releases/latest/download/latest.json"),
        }
    };
    Url::parse(&url).ok()
}

fn channel(app: &AppHandle) -> String {
    let saved = app.state::<AppState>().db.with(|c| db::get_setting(c, KEY_CHANNEL)).ok().flatten();
    resolve_channel(saved.as_deref(), build_channel()).to_string()
}

/// Why this install can't update itself, if it can't. Linux needs the AppImage's own path
/// (`APPIMAGE`, set by the AppImage runtime) in a folder it can write to: the updater
/// replaces that file.
pub fn not_installable(os: &str, appimage: bool, folder_writable: bool) -> Option<&'static str> {
    if os == "android" {
        Some("PeetMail on Android can't update itself yet. Install the new APK from the releases page.")
    } else if os != "linux" {
        None
    } else if !appimage {
        Some("PeetMail updates itself only when run as an AppImage. Update this copy the way you installed it.")
    } else if !folder_writable {
        Some("PeetMail can't update itself from this folder. Move the AppImage somewhere you can write to, such as your home folder.")
    } else {
        None
    }
}

fn install_blocker() -> Option<&'static str> {
    let appimage = std::env::var_os("APPIMAGE").map(std::path::PathBuf::from);
    not_installable(std::env::consts::OS, appimage.is_some(), appimage.as_deref().is_some_and(folder_writable))
}

#[cfg(target_os = "linux")]
fn folder_writable(file: &std::path::Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Some(dir) = file.parent() else { return false };
    let Ok(dir) = std::ffi::CString::new(dir.as_os_str().as_bytes()) else { return false };
    // SAFETY: a NUL-terminated path.
    unsafe { libc::access(dir.as_ptr(), libc::W_OK) == 0 }
}

#[cfg(not(target_os = "linux"))]
fn folder_writable(_: &std::path::Path) -> bool {
    true
}

/// Stands in for the plugin's `Update` on Android, where nothing is ever found
/// (`not_installable` turns checks off before they'd get here).
#[cfg(mobile)]
struct Update {
    version: String,
    body: Option<String>,
}

#[cfg(mobile)]
impl Update {
    fn install(&self, _bytes: &[u8]) -> std::result::Result<(), String> {
        Err("not available on this device".into())
    }
}

/// Set just before restarting into an update, read (and cleared) by the next start.
const KEY_RESTART_TO_UPDATE: &str = "restarted_to_update";

/// Whether this start is the restart after installing an update.
#[cfg_attr(mobile, allow(dead_code))] // the restart after an update is desktop only
pub fn take_restart_flag(app: &AppHandle) -> bool {
    let Some(state) = app.try_state::<AppState>() else { return false };
    state
        .db
        .with(|c| {
            let set = db::get_setting(c, KEY_RESTART_TO_UPDATE)?.as_deref() == Some("1");
            if set {
                db::set_setting(c, KEY_RESTART_TO_UPDATE, "0")?;
            }
            Ok(set)
        })
        .unwrap_or(false)
}

fn feed(channel: &str) -> Option<Url> {
    feed_url(channel, option_env!("PEETMAIL_UPDATE_REPO"), testenv::get("PEETMAIL_UPDATE_URL").as_deref())
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn set(app: &AppHandle, f: impl FnOnce(&mut UpdateStatus)) -> UpdateStatus {
    let updates = app.state::<Updates>();
    let mut s = updates.status.lock().unwrap();
    f(&mut s);
    s.seq += 1;
    let out = s.clone();
    drop(s);
    let _ = app.emit(EVENT_UPDATE_STATUS, &out);
    out
}

pub fn status(app: &AppHandle) -> UpdateStatus {
    app.state::<Updates>().status.lock().unwrap().clone()
}

/// Sets up the state and the background checks. Call once from setup.
pub fn init(app: &AppHandle) {
    app.manage(Updates::default());
    let channel = channel(app);
    let has_feed = feed(&channel).is_some();
    let blocker = if has_feed { install_blocker() } else { None };
    let enabled = has_feed && blocker.is_none();
    set(app, |s| {
        s.state = if enabled { "idle" } else { "disabled" };
        s.message = blocker.map(String::from);
        s.channel = channel;
        s.current_version = app.package_info().version.to_string();
    });
    if !enabled {
        return;
    }
    let first = testenv::get("PEETMAIL_UPDATE_FIRST_CHECK_SECS").and_then(|v| v.parse().ok()).unwrap_or(FIRST_CHECK_SECS);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(first)).await;
        loop {
            check(&app).await;
            tokio::time::sleep(Duration::from_secs(CHECK_EVERY_SECS)).await;
        }
    });
}

/// Checks the channel's feed and, if there's a newer version, downloads it (the download
/// verifies the signature). A check already running is not repeated; an update already
/// downloaded for this channel is kept. If the channel changes while a check runs, its
/// result is dropped and the new channel is checked.
pub async fn check(app: &AppHandle) -> UpdateStatus {
    let updates = app.state::<Updates>();
    let Ok(_guard) = updates.checking.try_lock() else { return status(app) };
    loop {
        let channel = channel(app);
        let Some(url) = feed(&channel) else {
            return set(app, |s| s.state = "disabled");
        };
        if let Some(why) = install_blocker() {
            return set(app, |s| {
                s.state = "disabled";
                s.message = Some(why.into());
            });
        }
        if updates.ready.lock().unwrap().as_ref().is_some_and(|r| r.channel == channel) {
            return status(app);
        }
        set(app, |s| {
            s.state = "checking";
            s.message = None;
        });
        let result = find_and_download(app, url).await;
        if self::channel(app) != channel {
            log::info!("update channel changed during the check; checking again");
            continue;
        }
        crate::telemetry::count(
            "peetmail.update.checks",
            1,
            &[
                crate::telemetry::attr(crate::telemetry::Key::Channel, if channel == "canary" { "canary" } else { "stable" }),
                crate::telemetry::attr(crate::telemetry::Key::Outcome, match &result {
                    Ok(None) => "up_to_date",
                    Ok(Some(_)) => "downloaded",
                    Err(_) => "error",
                }),
            ],
        );
        return match result {
            Ok(None) => {
                log::info!("update check ({channel}): up to date");
                set(app, |s| {
                    s.state = "upToDate";
                    s.version = None;
                    s.notes = None;
                    s.last_check = Some(now());
                })
            }
            Ok(Some((update, bytes))) => {
                log::info!("update {} ({channel}) downloaded and verified", update.version);
                let (version, notes) = (update.version.clone(), update.body.clone());
                *updates.ready.lock().unwrap() = Some(Ready { update, bytes, channel });
                set(app, |s| {
                    s.state = "ready";
                    s.version = Some(version);
                    s.notes = notes.filter(|n| !n.trim().is_empty());
                    s.last_check = Some(now());
                })
            }
            Err(e) => {
                log::warn!("update check ({channel}) failed: {e}");
                set(app, |s| {
                    s.state = "error";
                    s.message = Some(e.to_string());
                    s.last_check = Some(now());
                })
            }
        };
    }
}

#[cfg(mobile)]
async fn find_and_download(_app: &AppHandle, _url: Url) -> std::result::Result<Option<(Update, Vec<u8>)>, String> {
    Err("PeetMail on this device can't update itself.".into())
}

#[cfg(desktop)]
async fn find_and_download(app: &AppHandle, url: Url) -> std::result::Result<Option<(Update, Vec<u8>)>, tauri_plugin_updater::Error> {
    let updater = app
        .updater_builder()
        .endpoints(vec![url])?
        .on_before_exit(|| {
            log::info!("closing for the update installer");
            log::logger().flush();
            crate::telemetry::shutdown();
            crate::crash::flush();
        })
        .build()?;
    let Some(update) = updater.check().await? else { return Ok(None) };
    set(app, |s| {
        s.state = "downloading";
        s.version = Some(update.version.clone());
    });
    let bytes = update.download(|_, _| {}, || {}).await?;
    Ok(Some((update, bytes)))
}

/// Switches channel. A download for the other channel is dropped; the new channel is
/// checked straight away. Going from canary to stable takes effect at the next stable
/// release newer than the running build (no downgrades).
pub fn set_channel(app: &AppHandle, channel: &str) -> Result<UpdateStatus> {
    let channel = CHANNELS.iter().find(|c| **c == channel).ok_or_else(|| AppError::Other(format!("unknown update channel {channel}")))?;
    app.state::<AppState>().db.with(|c| db::set_setting(c, KEY_CHANNEL, channel))?;
    let keep_ready = {
        let updates = app.state::<Updates>();
        let mut ready = updates.ready.lock().unwrap();
        if ready.as_ref().is_some_and(|r| r.channel != *channel) {
            *ready = None;
        }
        ready.is_some()
    };
    let enabled = feed(channel).is_some() && install_blocker().is_none();
    let out = set(app, |s| {
        s.channel = channel.to_string();
        if !keep_ready {
            s.state = if enabled { "idle" } else { "disabled" };
            s.version = None;
            s.notes = None;
        }
    });
    if enabled {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            check(&app).await;
        });
    }
    Ok(out)
}

/// Installs the downloaded update: PeetMail closes, the installer runs without asking, and
/// the new version starts. (On Windows the NSIS installer relaunches it; on Linux the
/// AppImage is replaced and PeetMail restarts itself.) Refused while a message is inside
/// its undo-send window, so nobody loses the chance to take it back.
pub fn install(app: &AppHandle) -> Result<()> {
    let sending = app.state::<AppState>().db.with(|c| outbox::sends_in_progress(c))?;
    if sending > 0 {
        return Err(AppError::Other("A message is still sending. Try again in a few seconds.".into()));
    }
    let updates = app.state::<Updates>();
    let ready = updates.ready.lock().unwrap();
    let Some(ready) = ready.as_ref() else {
        return Err(AppError::Other("No update is ready to install.".into()));
    };
    log::info!("installing update {}", ready.update.version);
    if let Some(path) = testenv::get("PEETMAIL_FAKE_UPDATE_INSTALL") {
        // Tests stop here: record what would have been installed instead of running it.
        let record = serde_json::json!({ "version": ready.update.version, "bytes": ready.bytes.len() });
        std::fs::write(path, record.to_string())?;
        return Ok(());
    }
    ready.update.install(&ready.bytes).map_err(|e| AppError::Other(format!("couldn't start the installer: {e}")))?;
    if cfg!(target_os = "linux") {
        // The AppImage on disk is now the new version; run it. `restart` relaunches the
        // AppImage file itself, not the mounted copy of the old one.
        log::info!("update installed; restarting");
        // `restart` reuses this run's arguments; a `--minimized` autostart would come back
        // hidden. The flag makes the new version show its window.
        if let Err(e) = app.state::<AppState>().db.with(|c| db::set_setting(c, KEY_RESTART_TO_UPDATE, "1")) {
            log::warn!("couldn't note the restart: {e}");
        }
        log::logger().flush();
        crate::telemetry::shutdown();
        crate::crash::flush();
        app.restart();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_prefers_the_saved_choice_then_the_build() {
        assert_eq!(resolve_channel(Some("canary"), Some("stable")), "canary");
        assert_eq!(resolve_channel(None, Some("canary")), "canary");
        assert_eq!(resolve_channel(Some("nightly"), Some("canary")), "canary");
        assert_eq!(resolve_channel(None, None), "stable");
    }

    #[test]
    fn feeds_point_at_the_right_release() {
        let repo = Some("peetsoftware/peetmail");
        assert_eq!(
            feed_url("stable", repo, None).unwrap().as_str(),
            "https://github.com/peetsoftware/peetmail/releases/latest/download/latest.json"
        );
        assert_eq!(
            feed_url("canary", repo, None).unwrap().as_str(),
            "https://github.com/peetsoftware/peetmail/releases/download/canary/latest.json"
        );
        assert_eq!(feed_url("canary", repo, Some("http://127.0.0.1:4100/")).unwrap().as_str(), "http://127.0.0.1:4100/canary/latest.json");
    }

    #[test]
    fn canary_versions_sort_between_stable_releases() {
        // The version scheme from scripts/version.ts, compared the way the updater compares.
        let v = |s: &str| semver::Version::parse(s).unwrap();
        assert!(v("0.2.0") < v("0.2.1-canary.9"), "a canary is newer than the stable it follows");
        assert!(v("0.2.1-canary.9") < v("0.2.1-canary.10"), "run numbers compare as numbers");
        assert!(v("0.2.1-canary.10") < v("0.2.1"), "the next stable is newer than its canaries");
        assert!(v("0.3.0-beta.1") < v("0.3.0-canary.7"), "a canary of a beta sorts after the beta");
    }

    #[test]
    fn self_update_needs_windows_or_a_writable_appimage() {
        assert!(not_installable("windows", false, false).is_none());
        assert!(not_installable("linux", true, true).is_none());
        assert!(not_installable("linux", false, true).is_some_and(|m| m.contains("AppImage")));
        assert!(not_installable("linux", true, false).is_some_and(|m| m.contains("write to")));
        assert!(not_installable("android", false, false).is_some_and(|m| m.contains("APK")));
    }

    #[test]
    fn no_repo_means_no_updates() {
        assert!(feed_url("stable", None, None).is_none());
        assert!(feed_url("stable", Some(""), None).is_none());
    }
}
