//! Whether PeetMail is in front, on phones. Android pauses an app that leaves the screen
//! and cuts it off the network (Android 17: `blocked=APP_BACKGROUND`), so:
//!
//! - going to the background sends what the outbox holds while there's still network;
//! - coming back syncs straight away, without waiting for the timer (the snooze ticker
//!   catches up on its own once the process runs again);
//! - while in the background the sync timer skips its runs, which could only fail.
//!
//! Desktop windows are always "in front" here: they sync and notify from the tray.

use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{AppHandle, WindowEvent};

static IN_FRONT: AtomicBool = AtomicBool::new(true);

/// False only on a phone, between leaving the screen and coming back.
pub fn in_front() -> bool {
    IN_FRONT.load(Ordering::SeqCst)
}

/// What a lifecycle event asks for.
#[cfg_attr(desktop, allow(dead_code))] // desktop has no lifecycle events
#[derive(Debug, PartialEq, Eq)]
pub enum Reaction {
    /// Back on screen: catch up now.
    CatchUp,
    /// Leaving the screen: send what's queued while the network is still there.
    Flush,
    None,
}

/// Records the change and says what to do about it. Only a change counts: Android sends
/// a pause for every dialog over the activity, and some resumes without a pause first.
#[cfg_attr(desktop, allow(dead_code))]
pub fn record(in_front_now: bool) -> Reaction {
    match (IN_FRONT.swap(in_front_now, Ordering::SeqCst), in_front_now) {
        (false, true) => Reaction::CatchUp,
        (true, false) => Reaction::Flush,
        _ => Reaction::None,
    }
}

/// Called for every window event (lib.rs).
pub fn on_window_event(app: &AppHandle, event: &WindowEvent) {
    #[cfg(mobile)]
    {
        let reaction = match event {
            WindowEvent::Resumed => record(true),
            WindowEvent::Suspended => record(false),
            _ => Reaction::None,
        };
        match reaction {
            Reaction::CatchUp => {
                log::info!("back in front: syncing");
                crate::sync::store::WAKE.notify_one();
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    crate::outbox::kick(&app);
                    if let Err(e) = crate::mail::sync_all(&app).await {
                        log::error!("sync on resume: {e}");
                    }
                });
            }
            Reaction::Flush => {
                log::info!("going to the background: sending what's queued");
                crate::outbox::kick(app);
            }
            Reaction::None => {}
        }
    }
    #[cfg(desktop)]
    let _ = (app, event);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_changes_between_front_and_background_react() {
        // One test owns the global flag, so the steps run in order.
        assert!(in_front(), "starts in front: the first resume on Android is never sent");
        assert_eq!(record(true), Reaction::None);
        assert_eq!(record(false), Reaction::Flush);
        assert!(!in_front());
        assert_eq!(record(false), Reaction::None, "a second pause (a dialog) changes nothing");
        assert_eq!(record(true), Reaction::CatchUp);
        assert!(in_front());
    }
}
