//! The Rust half of PeetMail's Android plugin; the Kotlin half is
//! `gen/android/app/src/main/java/com/peetsoftware/peetmail/PeetPlugin.kt`. Calls block
//! until Kotlin answers on Android's main thread, so they must never be made *from* the
//! main thread (sync Tauri commands run there): use the async variants in commands.

use std::path::Path;
use std::sync::OnceLock;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::plugin::{Builder, PluginHandle, TauriPlugin};
use tauri::Wry;

use crate::error::{AppError, Result};

static HANDLE: OnceLock<PluginHandle<Wry>> = OnceLock::new();

pub fn plugin() -> TauriPlugin<Wry> {
    Builder::new("peetmail")
        .setup(|_app, api| {
            let handle = api.register_android_plugin("com.peetsoftware.peetmail", "PeetPlugin")?;
            let _ = HANDLE.set(handle);
            Ok(())
        })
        .build()
}

/// Why a Kotlin call failed: its rejection code (if any) and message.
#[derive(Debug)]
pub struct Rejected {
    pub code: Option<String>,
    pub message: String,
}

fn call_raw<T: DeserializeOwned>(command: &str, payload: Value) -> std::result::Result<T, Rejected> {
    let handle = HANDLE.get().ok_or_else(|| Rejected { code: None, message: "the Android plugin isn't loaded".into() })?;
    handle.run_mobile_plugin(command, payload).map_err(|e| match e {
        tauri::plugin::mobile::PluginInvokeError::InvokeRejected(r) => Rejected { code: r.code, message: r.message.unwrap_or_default() },
        other => Rejected { code: None, message: other.to_string() },
    })
}

fn call<T: DeserializeOwned>(command: &str, payload: Value) -> Result<T> {
    call_raw(command, payload).map_err(|r| AppError::Other(r.message))
}

/// Runs a blocking call off the async runtime's threads (and off the main thread).
async fn call_async<T: DeserializeOwned + Send + 'static>(command: &'static str, payload: Value) -> Result<T> {
    tauri::async_runtime::spawn_blocking(move || call(command, payload)).await.map_err(|e| AppError::Other(e.to_string()))?
}

#[derive(Deserialize)]
struct Data {
    data: String,
}

fn decode(d: Data) -> std::result::Result<Vec<u8>, Rejected> {
    STANDARD.decode(d.data).map_err(|e| Rejected { code: None, message: e.to_string() })
}

/// Encrypts with the key that never leaves the Android Keystore.
pub fn wrap(data: &[u8]) -> std::result::Result<Vec<u8>, Rejected> {
    decode(call_raw("wrap", json!({ "data": STANDARD.encode(data) }))?)
}

/// Decrypts what `wrap` made. `code == Some("missing")` means the Keystore key is gone.
pub fn unwrap(blob: &[u8]) -> std::result::Result<Vec<u8>, Rejected> {
    decode(call_raw("unwrap", json!({ "data": STANDARD.encode(blob) }))?)
}

fn path_str(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Opens a file (it must be in the app's cache folder) in the app chosen for its type.
pub async fn open_file(path: &Path, name: &str) -> Result<()> {
    call_async::<Value>("openFile", json!({ "path": path_str(path), "name": name })).await.map(|_| ())
}

/// Copies a file into Downloads; returns the name it got there.
pub async fn save_to_downloads(path: &Path, name: &str) -> Result<String> {
    #[derive(Deserialize)]
    struct Saved {
        name: String,
    }
    let saved: Saved = call_async("saveToDownloads", json!({ "path": path_str(path), "name": name })).await?;
    Ok(saved.name)
}

/// The share sheet for a file in the app's cache folder.
pub async fn share_file(path: &Path, name: &str, title: &str) -> Result<()> {
    call_async::<Value>("shareFile", json!({ "path": path_str(path), "name": name, "title": title })).await.map(|_| ())
}

pub async fn open_notification_settings() -> Result<()> {
    call_async::<Value>("openNotificationSettings", json!({})).await.map(|_| ())
}

/// A file another app shared to PeetMail, copied into the cache.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharedFile {
    pub path: String,
    pub name: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Shared {
    pub text: Option<String>,
    pub subject: Option<String>,
    #[serde(default)]
    pub files: Vec<SharedFile>,
    /// Files Android wouldn't let PeetMail read, with why.
    #[serde(default)]
    pub failed: Vec<String>,
}

impl Shared {
    /// The folder PeetPlugin.kt copied the files into (one per share).
    pub fn files_dir(&self) -> Option<&Path> {
        self.files.first().and_then(|f| Path::new(&f.path).parent())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenTarget {
    pub account_id: i64,
    pub conversation_id: String,
}

/// What other apps handed PeetMail since the last call.
#[derive(Debug, Default, Deserialize)]
pub struct Pending {
    pub shared: Option<Shared>,
    /// The sign-in redirect (com.peetsoftware.peetmail://auth?code=…).
    pub redirect: Option<String>,
    pub open: Option<OpenTarget>,
}

pub async fn take_pending() -> Result<Pending> {
    call_async("takePending", json!({})).await
}
