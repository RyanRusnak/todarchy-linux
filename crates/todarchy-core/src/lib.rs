// todarchy-core — the Tauri-free heart of the app.
//
// Everything that isn't UI lives here: the Automerge-backed task store,
// the three sync transports (local file, synced folder, HTTP relay),
// per-project ChaCha20 encrypted sharing, the keyring wrapper, and the
// due-task notification loop.
//
// The original build wired all of this into Tauri via `AppHandle` — it
// used `app.emit(...)` to push updates to a WebView and
// `app.notification()` for desktop notifications. Those were the ONLY
// two things Tauri provided the backend. They're now abstracted behind
// the `EventSink` trait below, so any front end (a Ratatui TUI, a test
// harness, a headless CLI) can drive the exact same sync/crypto code.

pub mod config;
pub mod cryptobox;
pub mod doc;
pub mod keystore;
pub mod notify;
pub mod per_project;
pub mod server_client;
pub mod shared;
pub mod sharelink;
pub mod store;
pub mod sync;
pub mod sync_watcher;

use serde_json::Value;

pub use sync::SyncStatus;

/// The bridge between core's sync/notify machinery and whatever front end
/// is driving it. In the old Tauri build these mapped to `app.emit(...)`
/// and `app.notification()`. A TUI implements this by forwarding onto an
/// in-process channel its event loop selects on, and shelling out to
/// `notify-send`. Tests use [`NullSink`].
///
/// Implementations must be cheap to call and non-blocking: they're
/// invoked from inside the store save/load path and from the background
/// watcher loops.
pub trait EventSink: Send + Sync {
    /// The full task/project/context projection changed on disk (a peer
    /// wrote via the sync folder or relay, or a local mutation just
    /// landed). Carries the same JSON the store persists to tasks.json.
    /// A `Null` value means "reload from scratch" (e.g. sync was cleared).
    fn tasks_changed(&self, json: &Value);

    /// The sync indicator changed — folder path, last-synced timestamp,
    /// last error, or server settings. Front ends render this as a status
    /// line / badge.
    fn sync_status(&self, status: &SyncStatus);

    /// Surface a desktop notification (deferred task came back, etc.).
    fn notify(&self, title: &str, body: &str);
}

/// A sink that drops every event on the floor. Used by tests and by
/// one-shot headless paths (e.g. accepting a share link from the CLI)
/// where there's no live UI to update.
pub struct NullSink;

impl EventSink for NullSink {
    fn tasks_changed(&self, _json: &Value) {}
    fn sync_status(&self, _status: &SyncStatus) {}
    fn notify(&self, _title: &str, _body: &str) {}
}
