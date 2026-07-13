// sync.rs — sync status reporting. Configuration now lives entirely in the
// hand-edited config.toml (see config.rs); this module no longer mutates it.
// The transient status (last-synced time, last error) is in-memory only —
// it's derived state, not settings, so it doesn't belong in the config file.

use std::sync::Mutex;

use once_cell::sync::Lazy;
use serde::Serialize;

use crate::config;
use crate::EventSink;

/// In-memory sync status, updated by record_success/record_error and reset
/// each launch. Not persisted.
#[derive(Default)]
struct Runtime {
    last_synced_at: Option<i64>,
    last_sync_error: Option<String>,
}

static STATE: Lazy<Mutex<Runtime>> = Lazy::new(|| Mutex::new(Runtime::default()));

/// Wire format for the sync-status event + status line.
#[derive(Debug, Clone, Serialize)]
pub struct SyncStatus {
    pub folder: String,
    pub last_synced_at: Option<i64>,
    pub last_sync_error: Option<String>,
    #[serde(default)]
    pub server_base_url: String,
    #[serde(default)]
    pub server_main_doc_id: String,
}

pub fn current_status() -> SyncStatus {
    let cfg = config::load().unwrap_or_default();
    let st = STATE.lock().unwrap();
    SyncStatus {
        folder: cfg.sync_folder,
        last_synced_at: st.last_synced_at,
        last_sync_error: st.last_sync_error.clone(),
        server_base_url: cfg.server_base_url,
        server_main_doc_id: cfg.server_main_doc_id,
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn configured() -> bool {
    config::load()
        .map(|c| !c.sync_folder.trim().is_empty() || !c.server_base_url.trim().is_empty())
        .unwrap_or(false)
}

/// Stamp a successful sync and push status to the front end. No-op when no
/// sync transport is configured (local-only has no status to report).
pub fn record_success(sink: &dyn EventSink) {
    if !configured() {
        return;
    }
    {
        let mut st = STATE.lock().unwrap();
        st.last_synced_at = Some(now_ms());
        st.last_sync_error = None;
    }
    sink.sync_status(&current_status());
}

/// Stamp a sync failure with a human-readable reason.
pub fn record_error(sink: &dyn EventSink, reason: impl ToString) {
    let reason = reason.to_string();
    tracing::warn!("sync error: {reason}");
    {
        let mut st = STATE.lock().unwrap();
        st.last_sync_error = Some(reason);
    }
    sink.sync_status(&current_status());
}

/// Hit /healthz on the configured relay; surfaces true/false for a diagnostic.
pub async fn server_healthz() -> Result<bool, String> {
    let cfg = config::load().map_err(|e| e.to_string())?;
    if cfg.server_base_url.trim().is_empty() {
        return Ok(false);
    }
    let client = crate::server_client::ServerSyncClient::new(&cfg.server_base_url)
        .map_err(|e| e.to_string())?;
    Ok(client.healthz().await)
}
