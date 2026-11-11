//! What sync, the outbox and notifications need from their surroundings: the app's state,
//! somewhere to send events, and somewhere to put notifications.
//!
//! Usually that's the running Tauri app. On Android a background job (`background.rs`)
//! also syncs while PeetMail's window is closed, sometimes in a process with no Tauri app
//! at all: then the state is its own, events go nowhere (nothing is listening), and
//! notifications are handed back to Kotlin to show.

use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::notify::Toast;
use crate::state::AppState;

#[derive(Clone)]
pub struct Core {
    inner: Inner,
    /// A background run: notifications collect here instead of showing.
    toasts: Option<Arc<Mutex<Vec<Toast>>>>,
}

/// The running app, behind a trait object: unit tests build headless cores only, and this
/// way don't link Tauri's runtime (whose window code won't even load in a test binary on
/// Windows: STATUS_ENTRYPOINT_NOT_FOUND).
trait App: Send + Sync {
    fn state(&self) -> &AppState;
    fn emit(&self, event: &str, payload: serde_json::Value) -> tauri::Result<()>;
    fn show(&self, toasts: Vec<Toast>);
}

impl App for AppHandle {
    fn state(&self) -> &AppState {
        Manager::state::<AppState>(self).inner()
    }
    fn emit(&self, event: &str, payload: serde_json::Value) -> tauri::Result<()> {
        Emitter::emit(self, event, payload)
    }
    fn show(&self, toasts: Vec<Toast>) {
        crate::notify::show(self, toasts)
    }
}

#[derive(Clone)]
enum Inner {
    App(Arc<dyn App>),
    #[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
    Headless(Arc<AppState>),
}

impl Core {
    /// A background run inside the running app: its state, and its window hears the events
    /// (so the list is current when it comes back), but notifications are collected.
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    pub fn background(app: &AppHandle) -> Self {
        Self { inner: Inner::App(Arc::new(app.clone())), toasts: Some(Arc::default()) }
    }

    /// A background run with no app in the process.
    #[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
    pub fn headless(state: AppState) -> Self {
        Self { inner: Inner::Headless(Arc::new(state)), toasts: Some(Arc::default()) }
    }

    pub fn state(&self) -> &AppState {
        match &self.inner {
            Inner::App(app) => app.state(),
            Inner::Headless(state) => state,
        }
    }

    pub fn emit<S: Serialize + Clone>(&self, event: &str, payload: S) -> tauri::Result<()> {
        match &self.inner {
            Inner::App(app) => app.emit(event, serde_json::to_value(payload)?),
            Inner::Headless(_) => Ok(()),
        }
    }

    /// True for a background job's run: it does what's needed and stops, rather than keep
    /// watching (the outbox) or warm the cache for reading (body prefetch).
    pub fn is_background(&self) -> bool {
        self.toasts.is_some()
    }

    /// Shows notifications, or keeps them for the background job to show.
    pub fn notify(&self, toasts: Vec<Toast>) {
        if toasts.is_empty() {
            return;
        }
        match (&self.toasts, &self.inner) {
            (Some(kept), _) => kept.lock().unwrap_or_else(|e| e.into_inner()).extend(toasts),
            (None, Inner::App(app)) => app.show(toasts),
            (None, Inner::Headless(_)) => unreachable!("a headless core always keeps its notifications"),
        }
    }

    /// The notifications a background run collected, and forgets them.
    #[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
    pub fn take_toasts(&self) -> Vec<Toast> {
        self.toasts.as_ref().map(|t| std::mem::take(&mut *t.lock().unwrap_or_else(|e| e.into_inner()))).unwrap_or_default()
    }
}

impl From<&AppHandle> for Core {
    fn from(app: &AppHandle) -> Self {
        Self { inner: Inner::App(Arc::new(app.clone())), toasts: None }
    }
}

impl From<&Core> for Core {
    fn from(core: &Core) -> Self {
        core.clone()
    }
}
