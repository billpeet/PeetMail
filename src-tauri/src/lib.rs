mod actions;
#[cfg(target_os = "android")]
mod android;
mod auth;
mod calendar;
mod commands;
mod crash;
mod datadir;
mod db;
mod draft_files;
mod draft_sync;
mod drafts;
mod error;
mod event_edit;
mod gcal;
mod gmail;
mod gmail_send;
mod graph;
mod graph_send;
mod health;
mod items;
mod lifecycle;
mod mail;
mod meeting_links;
mod notify;
mod outbox;
mod query;
mod recurrence;
mod rooms;
mod sanitize;
mod scrub;
mod secure;
mod server_search;
mod state;
mod support;
mod sync;
mod telemetry;
mod testenv;
mod update;

use std::time::Duration;

use tauri::{Emitter, Manager};

/// Delta polls are cheap; Graph change notifications need a public webhook,
/// which a desktop app doesn't have.
const DEFAULT_SYNC_INTERVAL_SECS: u64 = 60;

/// The main window is built in code (not tauri.conf.json) so the test harness can
/// run it headless: off-screen, out of the taskbar, and never stealing focus.
/// Panics normally go only to stderr, which is gone once the terminal (or the window, for
/// an installed app) closes. Put them in the log file too, with where and a backtrace, so
/// a crash leaves evidence. The default hook still runs (stderr output unchanged).
fn install_panic_logging() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        let backtrace = std::backtrace::Backtrace::force_capture();
        log::error!("PANIC in thread '{}': {info}\n{backtrace}", thread.name().unwrap_or("unnamed"));
        log::logger().flush();
        default_hook(info);
    }));
}

/// Early dev builds used another identifier, so their data sits in sibling folders. Moves
/// it over once; a failure (say the old build is still running) is logged and we start fresh.
fn adopt_legacy_data(roaming: &std::path::Path, local: &std::path::Path) {
    for (dir, names) in [(roaming, datadir::DB_FILES), (local, datadir::WEBVIEW_FILES)] {
        match datadir::adopt_legacy(dir, names) {
            Ok(true) => log::info!("moved {} from {}", names[0], datadir::LEGACY_IDENTIFIER),
            Ok(false) => {}
            Err(e) => log::error!("couldn't move {} from {}: {e}", names[0], datadir::LEGACY_IDENTIFIER),
        }
    }
}

/// PeetMail can't start (e.g. its database is from a newer version). Says why in a message
/// box rather than vanishing, then exits. Headless test runs only log it.
fn fatal_startup_error(message: &str) -> ! {
    log::error!("cannot start: {message}");
    log::logger().flush();
    if !testenv::flag("PEETMAIL_HEADLESS") {
        show_error_box("PeetMail can't start", message);
    }
    std::process::exit(1);
}

#[cfg(windows)]
fn show_error_box(caption: &str, message: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
    let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let (text, caption) = (wide(message), wide(caption));
    // SAFETY: both strings are NUL-terminated UTF-16 that outlive the call.
    unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), caption.as_ptr(), MB_OK | MB_ICONERROR) };
}

/// Linux has no system message box API; the desktop's dialog tools do the job (zenity on
/// GNOME and most others, kdialog on KDE). Without either the reason is only in the log
/// and on stderr.
#[cfg(target_os = "linux")]
fn show_error_box(caption: &str, message: &str) {
    use std::process::Command;
    eprintln!("{caption}: {message}");
    // Inside an AppImage these point GTK and GIO at the bundled copies, which the system's
    // zenity mustn't load.
    const APPIMAGE_ENV: [&str; 10] = [
        "LD_LIBRARY_PATH", "GTK_PATH", "GTK_EXE_PREFIX", "GTK_DATA_PREFIX", "GTK_IM_MODULE_FILE",
        "GDK_PIXBUF_MODULE_FILE", "GIO_MODULE_DIR", "GSETTINGS_SCHEMA_DIR", "GI_TYPELIB_PATH", "GDK_BACKEND",
    ];
    let tries: [(&str, Vec<&str>); 2] = [
        ("zenity", vec!["--error", "--title", caption, "--text", message, "--no-markup"]),
        ("kdialog", vec!["--title", caption, "--error", message]),
    ];
    for (tool, args) in tries {
        let mut cmd = Command::new(tool);
        cmd.args(args);
        if std::env::var_os("APPIMAGE").is_some() {
            for k in APPIMAGE_ENV {
                cmd.env_remove(k);
            }
        }
        // The dialog showed (closing it with Esc also exits non-zero): don't show a second.
        if cmd.status().is_ok_and(|s| s.code() != Some(127) && s.code().is_some()) {
            return;
        }
    }
}

/// Android has no message box a native library can raise before the webview is up; the
/// reason goes to the log (logcat). TODO(android stage 1): show it in the webview.
#[cfg(target_os = "android")]
fn show_error_box(caption: &str, message: &str) {
    log::error!("{caption}: {message}");
}

/// Brings the main window back (tray click, second launch, tray menu).
#[cfg(desktop)]
fn show_main_window(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

/// Tray icon: click to open; menu with Open, Sync now, Quit.
#[cfg(desktop)]
fn create_tray(app: &tauri::App) -> tauri::Result<()> {
    use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
    let open = MenuItem::with_id(app, "open", "Open PeetMail", true, None::<&str>)?;
    let sync = MenuItem::with_id(app, "sync", "Sync now", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit PeetMail", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &sync, &PredefinedMenuItem::separator(app)?, &quit])?;
    let mut tray = TrayIconBuilder::with_id("main")
        .tooltip("PeetMail")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, e| match e.id.as_ref() {
            "open" => show_main_window(app),
            "sync" => {
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    outbox::kick(&app);
                    let _ = mail::sync_all(&app).await;
                });
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, e| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
                show_main_window(tray.app_handle());
            }
        });
    // The hand-tuned small icon (scripts/icons.ts): the window icon scaled down to tray size
    // goes blurry.
    match tauri::image::Image::from_bytes(include_bytes!("../icons/tray.png")) {
        Ok(icon) => tray = tray.icon(icon),
        Err(e) => {
            log::warn!("tray icon: {e}");
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
        }
    }
    tray.build(app)?;
    Ok(())
}

/// `dev_data`: a dev build gets a WebView profile (localStorage) of its own, apart from the
/// installed app's.
#[cfg(desktop)]
fn create_main_window(app: &tauri::App, dev_data: bool) -> tauri::Result<()> {
    let headless = testenv::flag("PEETMAIL_HEADLESS");
    // Started at sign-in (autostart): wait in the tray.
    // A restart to update keeps the old arguments (Linux), but the user just asked for it,
    // so the window shows.
    let after_update = update::take_restart_flag(app.handle());
    let minimized = std::env::args().any(|a| a == "--minimized") && !after_update;
    let mut builder = tauri::WebviewWindowBuilder::new(app, "main", tauri::WebviewUrl::default())
        .title("PeetMail")
        .inner_size(1280.0, 820.0)
        .min_inner_size(900.0, 560.0)
        // The title bar is drawn by the app (TitleBar.svelte); keep the native shadow.
        .decorations(false)
        .shadow(true)
        .background_color(tauri::window::Color(38, 38, 42, 255))
        // Files dropped on the window reach the page as HTML drag and drop (the composer).
        .disable_drag_drop_handler();
    // Tests: WebView2's browser arguments (the DevTools port), set on the window itself.
    // WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS can't be relied on: whether WebView2 lets it
    // override the app's own arguments varies (on CI it never did).
    if dev_data {
        builder = builder.data_directory(datadir::dev_dir().join("webview"));
    }
    if let Some(args) = testenv::get("PEETMAIL_WEBVIEW2_ARGS") {
        builder = builder.additional_browser_args(&args);
    }
    // Tests and the drive CLI: a phone-sized window, so the page lays out compact for good
    // (a viewport override lasts only as long as one DevTools connection).
    if let Some((w, h)) = testenv::window_size() {
        builder = builder.min_inner_size(w, h).inner_size(w, h);
    }
    builder = if minimized && !headless {
        builder.visible(false).center()
    } else if headless {
        // Never shown: WebView2 keeps rendering (screenshots, input over CDP) for a
        // hidden window, and nothing flashes up or steals focus.
        builder.visible(false).focused(false).skip_taskbar(true)
    } else {
        builder.center()
    };
    builder.build()?;
    Ok(())
}

/// A phone has one full-screen window: no size, decorations, tray or profile to choose.
#[cfg(mobile)]
fn create_main_window(app: &tauri::App, _dev_data: bool) -> tauri::Result<()> {
    tauri::WebviewWindowBuilder::new(app, "main", tauri::WebviewUrl::default()).build()?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let started = std::time::Instant::now();
    // A dev build (`pnpm tauri dev`, which loads the Vite server) works on its own copy of
    // the installed app's data, so its migrations never lock the installed version out.
    // Test builds embed the frontend and always pass PEETMAIL_DATA_DIR, so they never get here.
    let dev_build = tauri::is_dev();
    let mut data_dir_override = testenv::get("PEETMAIL_DATA_DIR").map(std::path::PathBuf::from);
    let is_test = data_dir_override.is_some();
    // A phone running `tauri android dev` has no checkout to keep `.peetmail-dev` in, and no
    // installed copy beside it to protect: it uses its own app data folder.
    let dev_data = cfg!(desktop) && dev_build && !is_test;
    // Logging isn't up yet: how the copy went is reported in setup.
    let mut dev_seed = Ok(false);
    if dev_data {
        let dev = datadir::dev_dir();
        if let Some(prod) = datadir::app_data_dir() {
            dev_seed = datadir::seed_dev(&prod, &dev);
        }
        data_dir_override = Some(dev);
    }
    // First, before Tauri: crash reporting covers startup, and the crash reporter process
    // (this binary run again) stops inside here.
    #[cfg(desktop)]
    if let Some(dir) = data_dir_override.clone().or_else(datadir::app_data_dir) {
        crash::init_early(&dir.join("peetmail.db"));
    }
    let sync_interval = Duration::from_secs(
        testenv::get("PEETMAIL_SYNC_INTERVAL_SECS")
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_SYNC_INTERVAL_SECS),
    );

    let mut log = tauri_plugin_log::Builder::new().level(log::LevelFilter::Info);
    if let Some(dir) = &data_dir_override {
        // Keep test logs next to the test database instead of mixing them with the real app's.
        log = log.clear_targets().targets([
            tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Stdout),
            tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Folder {
                path: dir.join("logs"),
                file_name: Some("PeetMail".into()),
            }),
        ]);
    }

    // Tests run several copies at once (and alongside the real app), so only a real
    // install is single-instance: a second launch just shows the running window. A dev
    // build shares the installed app's identifier, so it skips this too (else launching it
    // would just bring up the installed app).
    #[cfg_attr(all(mobile, not(target_os = "android")), allow(unused_mut))]
    let mut builder = tauri::Builder::default();
    #[cfg(desktop)]
    {
        if data_dir_override.is_none() {
            builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| show_main_window(app)));
        }
        builder = builder
            .plugin(tauri_plugin_autostart::init(tauri_plugin_autostart::MacosLauncher::LaunchAgent, Some(vec!["--minimized"])))
            .plugin(tauri_plugin_updater::Builder::new().build());
    }
    #[cfg(target_os = "android")]
    {
        builder = builder.plugin(android::plugin());
    }
    builder
        .plugin(tauri_plugin_notification::init())
        .on_window_event(|window, event| {
            lifecycle::on_window_event(window.app_handle(), event);
            // Coming back to PeetMail is a good moment to pick up settings changed elsewhere.
            if let tauri::WindowEvent::Focused(true) = event {
                sync::store::WAKE.notify_one();
            }
            // Closing the window hides it to the tray (unless that's turned off).
            #[cfg(desktop)]
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let state = window.app_handle().state::<state::AppState>();
                if state.preferences().map(|p| p.close_to_tray).unwrap_or(true) {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .plugin(tauri_plugin_opener::init())
        .setup(move |app| {
            // The log plugin is added here (not with the others) so its logger can be wrapped:
            // recent lines ride along with crash reports as breadcrumbs (see crash.rs).
            let (log_plugin, max_level, logger) = log.split(app.handle())?;
            app.handle().plugin(log_plugin)?;
            log::set_boxed_logger(Box::new(crash::TrailLogger(logger))).map_err(|e| tauri::Error::Anyhow(e.into()))?;
            log::set_max_level(max_level);
            install_panic_logging();
            let dir = match data_dir_override {
                Some(dir) => dir,
                None => {
                    let dir = app.path().app_data_dir()?;
                    adopt_legacy_data(&dir, &app.path().app_local_data_dir()?);
                    dir
                }
            };
            std::fs::create_dir_all(&dir)?;
            // Desktop starts crash reporting before Tauri (datadir::app_data_dir); Android's
            // data folder is only known from here.
            #[cfg(target_os = "android")]
            crash::init_early(&dir.join("peetmail.db"));
            // Mail, logs and the database live here. Some distros make home folders readable
            // by everyone; this folder isn't (Windows' %APPDATA% already isn't).
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Err(e) = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)) {
                    log::warn!("couldn't make the data folder private: {e}");
                }
            }
            match &dev_seed {
                Ok(true) => log::info!("dev build: copied the installed app's database to {}", dir.display()),
                Ok(false) => {}
                // Carrying on would start an empty mailbox that every later run keeps.
                Err(e) => fatal_startup_error(&format!("This dev build couldn't copy the installed app's database to {}: {e}", dir.display())),
            }
            secure::init(&dir, !is_test);
            // Android: the key comes out of the Keystore through Kotlin, which can't be asked
            // from the main thread (sync commands). Fetch it now, once, off that thread.
            #[cfg(target_os = "android")]
            std::thread::spawn(|| {
                if let Err(e) = secure::warm() {
                    log::warn!("couldn't load the key for saved sign-ins: {e}");
                }
            });
            let db = match db::Db::open(&dir.join("peetmail.db")) {
                Ok(db) => db,
                Err(e) => fatal_startup_error(&e.to_string()),
            };
            db.with(|c| outbox::reset_inflight(c))?;
            // A server search still open when the app closed: what it fetched goes again.
            db.with(|c| db::clear_search_cache(c))?;
            // Drafts changed here that never reached the server get another go.
            if let Err(e) = db.with(|c| {
                for (account_id, id) in drafts::unpushed(c)? {
                    outbox::enqueue_push_draft(c, account_id, id)?;
                }
                Ok(())
            }) {
                log::warn!("queueing draft pushes: {e}");
            }
            match db.with(|c| draft_files::purge_orphans(c)) {
                Ok(n) if n > 0 => log::info!("dropped {n} attached files no send needs"),
                Ok(_) => {}
                Err(e) => log::warn!("purging attached files: {e}"),
            }
            crash::after_db(&db);
            match db.with(|c| Ok((crash::install_id(c)?, db::get_setting(c, telemetry::KEY_USAGE_DATA)?))) {
                Ok((install_id, usage)) => telemetry::init(&install_id, usage.as_deref() != Some("off")),
                Err(e) => log::error!("telemetry: reading settings: {e}"),
            }
            app.manage(state::AppState::new(db));
            update::init(app.handle());
            health::start(app.handle());
            create_main_window(app, dev_data)?;
            telemetry::record_ms("app.startup", started.elapsed().as_secs_f64() * 1000.0, &[]);
            telemetry::start_sampling(app.handle().clone());
            // Headless test runs mustn't put icons in the user's taskbar.
            #[cfg(desktop)]
            if !testenv::flag("PEETMAIL_HEADLESS") {
                create_tray(app)?;
            }

            // Snoozes wake on time: a cheap indexed check each second.
            let snoozer = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    let state = snoozer.state::<state::AppState>();
                    match state.db.with(|c| actions::wake(c, None)) {
                        Ok(woke) if !woke.is_empty() => {
                            telemetry::count("peetmail.snooze.woke", woke.len() as u64, &[]);
                            outbox::emit_status(&snoozer, &state);
                            outbox::kick(&snoozer);
                            let mut accounts: Vec<i64> = woke.iter().map(|w| w.0).collect();
                            accounts.dedup();
                            for a in accounts {
                                let _ = snoozer.emit(mail::EVENT_MAIL_CHANGED, serde_json::json!({ "accountId": a, "folderId": null }));
                            }
                            if state.preferences().map(|p| p.notify != "off").unwrap_or(true) {
                                match state.db.with(|c| notify::reminder_toasts(c, &woke)) {
                                    Ok(t) => notify::show(&snoozer, t),
                                    Err(e) => log::error!("reminder notifications: {e}"),
                                }
                            }
                        }
                        Ok(_) => {}
                        Err(e) => log::error!("waking snoozes: {e}"),
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                }
            });

            sync::engine::start(app.handle(), sync_interval);

            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    // A phone in the background has no network (lifecycle.rs); coming back
                    // syncs at once instead.
                    if !lifecycle::in_front() {
                        tokio::time::sleep(sync_interval).await;
                        continue;
                    }
                    // Retry anything queued while offline before pulling changes.
                    outbox::kick(&handle);
                    if let Err(e) = mail::sync_all(&handle).await {
                        log::error!("background sync: {e}");
                    }
                    tokio::time::sleep(sync_interval).await;
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::bootstrap,
            commands::set_client_id,
            commands::sign_in,
            commands::cancel_sign_in,
            commands::sign_out,
            commands::list_folders,
            commands::list_threads,
            commands::get_thread,
            commands::search_server,
            commands::fetch_search_conversation,
            commands::clear_server_search,
            commands::get_message_body,
            commands::sync_now,
            sync::commands::settings_sync_status,
            sync::commands::synced_accounts,
            sync::commands::dismiss_synced_account,
            sync::commands::settings_sync_connect,
            sync::commands::settings_sync_join,
            sync::commands::settings_sync_create,
            sync::commands::settings_sync_cancel,
            sync::commands::settings_sync_now,
            sync::commands::settings_sync_devices,
            sync::commands::settings_sync_revoke,
            sync::commands::settings_sync_identities,
            sync::commands::settings_sync_unlink,
            sync::commands::settings_sync_change_passphrase,
            sync::commands::settings_sync_recovery_key,
            sync::commands::settings_sync_disable,
            sync::commands::settings_sync_delete_account,
            commands::set_cache_limits,
            commands::set_preferences,
            commands::record_open,
            commands::recent_opens,
            commands::list_events,
            commands::invite_event,
            commands::respond_to_event,
            commands::event_detail,
            commands::event_for_edit,
            commands::list_rooms,
            commands::meeting_provider,
            commands::reconnect_account,
            commands::update_event,
            commands::delete_calendar_event,
            commands::create_event,
            commands::snooze_thread,
            commands::snooze_threads,
            commands::unsnooze_thread,
            commands::list_snoozed,
            commands::list_snippets,
            commands::list_signatures,
            commands::set_google_client,
            commands::get_autostart,
            commands::set_autostart,
            commands::quit_app,
            commands::move_to_background,
            commands::take_pending,
            commands::notification_permission,
            commands::open_notification_settings,
            commands::open_external,
            commands::debug_panic,
            commands::set_snippets,
            commands::set_signatures,
            commands::cancel_send,
            commands::add_draft_file,
            commands::remove_draft_files,
            commands::save_draft,
            commands::list_drafts,
            commands::delete_draft,
            commands::scheduled_sends,
            commands::thread_action,
            commands::threads_action,
            commands::undo,
            commands::outbox_status,
            commands::send_message,
            commands::suggest_contacts,
            commands::list_splits,
            commands::set_splits,
            commands::get_message_body_with_images,
            commands::list_attachments,
            commands::download_attachment,
            commands::open_attachment,
            commands::update_status,
            commands::privacy,
            commands::set_crash_reports,
            commands::dismiss_privacy_notice,
            commands::changelog_seen,
            commands::set_changelog_seen,
            commands::feedback_options,
            commands::health_issues,
            commands::export_diagnostics,
            commands::send_feedback,
            commands::report_error,
            commands::set_usage_data,
            commands::ui_events,
            commands::debug_flush_telemetry,
            commands::debug_crash,
            commands::check_for_updates,
            commands::install_update,
            commands::set_update_channel,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app, event| {
            if let tauri::RunEvent::Exit = event {
                telemetry::shutdown();
                crash::flush();
            }
        });
}
