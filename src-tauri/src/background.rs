//! Checking for mail with PeetMail's window closed (docs/ANDROID.md Stage 5).
//!
//! Android stops a backgrounded app's timers and cuts it off the network, so on a phone a
//! WorkManager job (`SyncWorker.kt`) calls in here every 15 minutes or so, with the network
//! allowed for the length of the job. One run brings due snoozes back, sends what the
//! outbox holds (scheduled sends included) and syncs every account, then hands the
//! notifications back to Kotlin to show.
//!
//! The job runs in PeetMail's process. If the app is already running there (in the
//! background), the run uses it; if Android started the process just for the job, there is
//! no Tauri app, and the run opens the database itself (`Core::headless`).

use crate::context::Core;
use crate::{actions, mail, notify, outbox, telemetry};

/// Brings back the snoozes that are due: the inbox, the server, and a reminder. Run every
/// second while the app is up (lib.rs), and at the start of each background run.
pub fn wake_snoozes(core: &Core) {
    let state = core.state();
    match state.db.with(|c| actions::wake(c, None)) {
        Ok(woke) if !woke.is_empty() => {
            telemetry::count("peetmail.snooze.woke", woke.len() as u64, &[]);
            outbox::emit_status(core, state);
            // A background run drains the outbox next, and waits for it.
            if !core.is_background() {
                outbox::kick(core);
            }
            let mut accounts: Vec<i64> = woke.iter().map(|w| w.0).collect();
            accounts.dedup();
            for a in accounts {
                let _ = core.emit(mail::EVENT_MAIL_CHANGED, serde_json::json!({ "accountId": a, "folderId": null }));
            }
            if state.preferences().map(|p| p.notify != "off").unwrap_or(true) {
                match state.db.with(|c| notify::reminder_toasts(c, &woke)) {
                    Ok(t) => core.notify(t),
                    Err(e) => log::error!("reminder notifications: {e}"),
                }
            }
        }
        Ok(_) => {}
        Err(e) => log::error!("waking snoozes: {e}"),
    }
}

/// One background run. Returns the notifications to show.
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
pub async fn run_once(core: &Core) -> Vec<notify::Toast> {
    wake_snoozes(core);
    outbox::drain_now(core).await;
    if let Err(e) = mail::sync_all(core).await {
        log::error!("background sync: {e}");
    }
    core.take_toasts()
}

/// How often the job runs: the "Check for mail" setting, in minutes. None turns it off.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub fn interval_minutes(setting: &str) -> Option<u32> {
    match setting {
        "open" => None,
        "60" => Some(60),
        // Android's shortest period for a repeating job.
        _ => Some(15),
    }
}

/// Tells Android how often to run the job, from the setting. Called at startup and when
/// the setting changes; blocks on Kotlin, so never from the main thread.
#[cfg(target_os = "android")]
pub fn schedule(state: &crate::state::AppState) {
    let setting = state.preferences().map(|p| p.check_mail).unwrap_or_default();
    if let Err(e) = crate::android::schedule_sync(interval_minutes(&setting)) {
        log::warn!("couldn't schedule checking for mail in the background: {e}");
    }
}

#[cfg(target_os = "android")]
pub use jni_entry::set_app;

#[cfg(target_os = "android")]
mod jni_entry {
    use std::path::PathBuf;
    use std::sync::OnceLock;

    use jni::objects::{JByteArray, JClass, JString};
    use jni::sys::jstring;
    use jni::JNIEnv;
    use serde_json::json;
    use tauri::AppHandle;

    use crate::context::Core;
    use crate::error::{AppError, Result};
    use crate::notify::Toast;

    static APP: OnceLock<AppHandle> = OnceLock::new();

    /// The running app, so a job in its process uses it rather than open a second state.
    pub fn set_app(app: &AppHandle) {
        let _ = APP.set(app.clone());
    }

    /// Logging when Android started the process for the job (the app's log plugin sets up
    /// its own otherwise): to logcat, like the app's.
    fn init_logging() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            android_logger::init_once(android_logger::Config::default().with_max_level(log::LevelFilter::Info).with_tag("PeetMail"));
        });
    }

    /// Stack for the thread a check runs on. Tokio's workers get 2 MiB, which the app's syncs
    /// fit; this leaves room, and only what's touched is ever committed.
    const RUN_STACK: usize = 8 * 1024 * 1024;

    fn run(data_dir: PathBuf, key: Option<Vec<u8>>) -> Result<Vec<Toast>> {
        let core = match APP.get() {
            Some(app) => {
                // In front, the app syncs on its own timer.
                if crate::lifecycle::in_front() {
                    log::info!("background check: PeetMail is open, skipping");
                    return Ok(Vec::new());
                }
                Core::background(app)
            }
            None => {
                init_logging();
                crate::secure::init(&data_dir, false);
                if let Some(key) = key {
                    let key: [u8; 32] = key.try_into().map_err(|_| AppError::Other("the key for saved sign-ins has the wrong length".into()))?;
                    crate::secure::provide_key(key);
                }
                let db = crate::db::Db::open(&data_dir.join("peetmail.db"))?;
                // Left in flight by a process that died mid-send. Not while a drain runs here.
                if let Ok(_idle) = crate::state::OUTBOX_LOCK.try_lock() {
                    db.with(|c| crate::outbox::reset_inflight(c))?;
                }
                Core::headless(crate::state::AppState::new(db))
            }
        };
        log::info!("background check: starting");
        let toasts = tauri::async_runtime::block_on(super::run_once(&core));
        log::info!("background check: done, {} notification(s)", toasts.len());
        Ok(toasts)
    }

    fn toasts_json(toasts: &[Toast]) -> String {
        let list: Vec<_> = toasts
            .iter()
            .map(|t| {
                let (account_id, conversation_id) = t.open.clone().unzip();
                json!({ "title": t.title, "body": t.body, "accountId": account_id, "conversationId": conversation_id })
            })
            .collect();
        json!({ "toasts": list }).to_string()
    }

    /// `SyncWorker.runOnce(dataDir, key)`: one background run. `key` is the unwrapped key for
    /// saved sign-ins (Kotlin has the Keystore; there may be no plugin to ask here), or null.
    /// Returns `{"toasts": [...]}`, or `{"error": "..."}`.
    #[unsafe(no_mangle)]
    pub extern "system" fn Java_com_peetsoftware_peetmail_SyncWorker_runOnce<'l>(
        mut env: JNIEnv<'l>,
        _class: JClass<'l>,
        data_dir: JString<'l>,
        key: JByteArray<'l>,
    ) -> jstring {
        let args = (|| -> std::result::Result<(PathBuf, Option<Vec<u8>>), jni::errors::Error> {
            let dir: String = env.get_string(&data_dir)?.into();
            let key = if key.is_null() { None } else { Some(env.convert_byte_array(&key)?) };
            Ok((PathBuf::from(dir), key))
        })();
        let out = match args {
            Err(e) => json!({ "error": format!("bad arguments: {e}") }).to_string(),
            // On a thread of its own: WorkManager's thread has a Java-sized stack, and sync's
            // futures (deep in a debug build) overflowed it. A panic comes back from join.
            Ok((dir, key)) => match std::thread::Builder::new()
                .name("peetmail-check".into())
                .stack_size(RUN_STACK)
                .spawn(move || run(dir, key))
                .map_err(|e| AppError::Other(format!("couldn't start the background check: {e}")))
                .map(|t| t.join())
            {
                Err(e) => json!({ "error": e.to_string() }).to_string(),
                Ok(r) => match r {
                    Ok(Ok(toasts)) => toasts_json(&toasts),
                    Ok(Err(e)) => {
                        log::error!("background check: {e}");
                        json!({ "error": e.to_string() }).to_string()
                    }
                    Err(_) => json!({ "error": "the background check panicked" }).to_string(),
                },
            },
        };
        env.new_string(out).map(|s| s.into_raw()).unwrap_or(std::ptr::null_mut())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{self, Db, MessageUpsert};
    use crate::state::AppState;

    /// An account with a conversation whose snooze is due.
    fn due_snooze() -> (Db, i64) {
        let db = Db::open_in_memory().unwrap();
        let a = db
            .with(|c| {
                let a = db::upsert_account(c, "microsoft", "boris@x.com", None, b"t")?.id;
                let inbox = db::insert_folder(c, a, "INBOX", "inbox", "Inbox")?.id;
                db::upsert_message(c, a, inbox, &MessageUpsert {
                    remote_id: "m1".into(),
                    conversation_id: Some("c1".into()),
                    received_at: Some("2026-10-20T10:00:00Z".into()),
                    from_name: Some("Sam".into()),
                    subject: Some("Lunch".into()),
                    ..Default::default()
                })?;
                actions::snooze(c, a, "c1", chrono::Utc::now() + chrono::Duration::hours(3))?;
                c.execute("UPDATE snoozes SET until = '2000-01-01T00:00:00.000Z'", [])?;
                Ok(a)
            })
            .unwrap();
        (db, a)
    }

    #[test]
    fn a_due_snooze_in_a_background_run_keeps_its_reminder_for_the_job_to_show() {
        let (db, a) = due_snooze();
        let core = Core::headless(AppState::new(db));
        wake_snoozes(&core);
        let toasts = core.take_toasts();
        assert_eq!(toasts.len(), 1, "collected, not shown");
        assert_eq!(toasts[0].title, "Reminder: Lunch");
        assert_eq!(toasts[0].open, Some((a, "c1".to_string())), "a tap opens the conversation");
        assert!(core.take_toasts().is_empty(), "handed over once");
    }

    #[tokio::test]
    async fn a_run_returns_its_notifications_and_keeps_what_it_couldnt_send() {
        let (db, a) = due_snooze();
        // Signed out: nothing can reach a server, so the moves back to the inbox stay queued.
        db.with(|c| Ok(c.execute("UPDATE accounts SET token_blob = NULL WHERE id = ?1", [a])?)).unwrap();
        let core = Core::headless(AppState::new(db));
        let toasts = run_once(&core).await;
        assert_eq!(toasts.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(), vec!["Reminder: Lunch"]);
        let queued: i64 = core.state().db.with(|c| Ok(c.query_row("SELECT COUNT(*) FROM outbox WHERE status = 'pending'", [], |r| r.get(0))?)).unwrap();
        assert!(queued > 0, "left for the next run");
    }

    #[test]
    fn the_check_mail_setting_picks_the_period() {
        assert_eq!(interval_minutes("15"), Some(15));
        assert_eq!(interval_minutes("60"), Some(60));
        assert_eq!(interval_minutes("open"), None);
        assert_eq!(interval_minutes(""), Some(15), "unset is every 15 minutes");
    }
}
