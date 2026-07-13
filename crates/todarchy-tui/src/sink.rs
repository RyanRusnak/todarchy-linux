// sink.rs — the bridge between todarchy-core and the TUI event loop.
//
// core's sync/notify machinery talks to the front end through the
// `EventSink` trait (three methods). The Tauri build implemented it with
// `app.emit(...)` and a notification plugin. Here we forward every event
// onto an unbounded channel the event loop selects on, and desktop
// notifications shell out to `notify-send` — the Omarchy-native path.

use serde_json::Value;
use todarchy_core::{EventSink, SyncStatus};
use tokio::sync::mpsc::UnboundedSender;

/// Everything the event loop reacts to, from any source.
#[derive(Debug)]
pub enum AppEvent {
    /// A terminal input event (key, resize, …).
    Input(crossterm::event::Event),
    /// A peer wrote via a sync transport; carries the fresh projection.
    /// A `Null` value means "reload from scratch" (sync cleared).
    TasksChanged(Value),
    /// The sync indicator changed.
    SyncStatus(SyncStatus),
    /// A deferred task came back — surface a desktop notification.
    Notify { title: String, body: String },
    /// Re-read the store from disk (after an async command mutated it).
    Reload,
    /// Periodic tick — drives deferred-task surfacing + toast expiry redraw.
    Tick,
}

/// The `EventSink` the watchers and store operations are handed. Cloning is
/// cheap (just clones the channel sender), so every background loop gets its
/// own `Arc<TuiSink>`.
pub struct TuiSink {
    tx: UnboundedSender<AppEvent>,
}

impl TuiSink {
    pub fn new(tx: UnboundedSender<AppEvent>) -> Self {
        TuiSink { tx }
    }
}

impl EventSink for TuiSink {
    fn tasks_changed(&self, json: &Value) {
        let _ = self.tx.send(AppEvent::TasksChanged(json.clone()));
    }
    fn sync_status(&self, status: &SyncStatus) {
        let _ = self.tx.send(AppEvent::SyncStatus(status.clone()));
    }
    fn notify(&self, title: &str, body: &str) {
        let _ = self.tx.send(AppEvent::Notify {
            title: title.to_string(),
            body: body.to_string(),
        });
    }
}
