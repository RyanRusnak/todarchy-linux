// sync.rs — Tauri commands for the v0.2 folder-sync setting, plus the
// status-reporting helpers that every sync-touching call site uses to
// keep the UI's "synced / error / local" indicator honest.

use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::config;

/// Wire format of the `sync-status` event + `get_sync_status` return.
#[derive(Debug, Clone, Serialize)]
pub struct SyncStatus {
    pub folder: String,
    pub last_synced_at: Option<i64>,
    pub last_sync_error: Option<String>,
    /// HTTP relay base URL if server-sync mode is on. Empty otherwise.
    /// Used by the React palette to render "sync: server — host" status
    /// and decide which command set to show.
    #[serde(default)]
    pub server_base_url: String,
    /// Doc id used on the relay for the main `tasks.automerge` bytes.
    /// Same across the user's devices to share state; the React UI
    /// exposes a "copy main doc id" command so a second device can
    /// paste the same value during server-sync setup.
    #[serde(default)]
    pub server_main_doc_id: String,
}

pub fn current_status() -> SyncStatus {
    let cfg = config::load().unwrap_or_default();
    SyncStatus {
        folder: cfg.sync_folder,
        last_synced_at: cfg.last_synced_at,
        last_sync_error: cfg.last_sync_error,
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

/// Stamp a successful sync and push the new status to the UI.
/// No-op when no sync folder is configured.
pub fn record_success(app: &AppHandle) {
    let mut cfg = match config::load() {
        Ok(c) => c,
        Err(e) => { tracing::warn!("config load failed in record_success: {e}"); return; }
    };
    if cfg.sync_folder.trim().is_empty() { return; }
    cfg.last_synced_at = Some(now_ms());
    cfg.last_sync_error = None;
    if let Err(e) = config::save(&cfg) {
        tracing::warn!("config save failed in record_success: {e}");
    }
    let _ = app.emit("sync-status", &current_status());
}

/// Stamp a sync failure with a human-readable reason. Logged and pushed
/// to the UI so the user isn't left wondering why nothing's updating.
pub fn record_error(app: &AppHandle, reason: impl ToString) {
    let reason = reason.to_string();
    tracing::warn!("sync error: {reason}");
    let mut cfg = match config::load() {
        Ok(c) => c,
        Err(e) => { tracing::warn!("config load failed in record_error: {e}"); return; }
    };
    cfg.last_sync_error = Some(reason);
    if let Err(e) = config::save(&cfg) {
        tracing::warn!("config save failed in record_error: {e}");
    }
    let _ = app.emit("sync-status", &current_status());
}

#[tauri::command]
pub fn get_sync_status() -> Result<SyncStatus, String> {
    Ok(current_status())
}

#[tauri::command]
pub fn get_sync_folder() -> Result<String, String> {
    config::load()
        .map(|c| c.sync_folder)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn set_sync_folder(app: AppHandle, folder: String) -> Result<(), String> {
    let mut cfg = config::load().map_err(|e| e.to_string())?;
    cfg.sync_folder = folder.trim().to_string();
    // Clear any previous error / stale timestamp — the new folder is
    // starting its own sync lifecycle.
    cfg.last_sync_error = None;
    cfg.last_synced_at = None;
    config::save(&cfg).map_err(|e| e.to_string())?;
    tracing::info!("sync folder set: {}", cfg.sync_folder);
    let _ = app.emit("sync-status", &current_status());

    // Seed the new folder with current state, or merge+converge if the
    // folder already had a tasks.automerge from another device.
    match crate::store::load(&app).await {
        Ok(json) => {
            let n_tasks = json.get("tasks").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);
            let n_projects = json.get("projects").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);
            tracing::info!(
                "post-merge: tasks={} projects={}",
                n_tasks, n_projects
            );
            if let Err(e) = crate::store::save(&app, json.clone()).await {
                record_error(&app, e);
            } else {
                record_success(&app);
            }
            let _ = app.emit("tasks-changed", &json);
        }
        Err(e) => {
            record_error(&app, format!("merge on folder pick: {e}"));
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn clear_sync_folder(app: AppHandle) -> Result<(), String> {
    let mut cfg = config::load().map_err(|e| e.to_string())?;
    cfg.sync_folder = String::new();
    cfg.last_synced_at = None;
    cfg.last_sync_error = None;
    config::save(&cfg).map_err(|e| e.to_string())?;
    let _ = app.emit("sync-status", &current_status());
    let _ = app.emit("tasks-changed", serde_json::Value::Null);
    Ok(())
}

// ---------- Server-relay sync mode ----------

/// Wire format for `set_server_sync` return + the React UI:
/// surfaces the doc id back to the caller so the user can copy it onto
/// other devices.
#[derive(Debug, Clone, Serialize)]
pub struct ServerSyncSetupResult {
    pub base_url: String,
    pub main_doc_id: String,
}

/// Switch to server-relay sync. `main_doc_id` is optional — when
/// omitted, we mint a fresh id matching the iOS `main_<base64url>`
/// format. The caller (React palette) surfaces the id so the user can
/// configure their other devices with the same value.
///
/// After persisting config, run a load to pull whatever's already on
/// the server (so a fresh second device adopts the remote state instead
/// of overwriting it), and push the current local state up.
#[tauri::command]
pub async fn set_server_sync(
    app: AppHandle,
    base_url: String,
    main_doc_id: Option<String>,
) -> Result<ServerSyncSetupResult, String> {
    let trimmed_url = base_url.trim().to_string();
    if trimmed_url.is_empty() {
        return Err("base URL cannot be empty".to_string());
    }
    // Liveness probe so the user finds out NOW if they've typo'd the
    // hostname rather than at the first save.
    let probe = crate::server_client::ServerSyncClient::new(&trimmed_url)
        .map_err(|e| format!("invalid URL: {e}"))?;
    if !probe.healthz().await {
        return Err("server didn't respond to /healthz".to_string());
    }

    let doc_id = main_doc_id
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(crate::config::generate_main_doc_id);

    let mut cfg = config::load().map_err(|e| e.to_string())?;
    cfg.server_base_url = trimmed_url.clone();
    cfg.server_main_doc_id = doc_id.clone();
    cfg.last_synced_at = None;
    cfg.last_sync_error = None;
    config::save(&cfg).map_err(|e| e.to_string())?;
    tracing::info!("server sync set: {trimmed_url} (id {doc_id})");
    let _ = app.emit("sync-status", &current_status());

    // Mirror set_sync_folder: trigger a load+save cycle so the server's
    // current bytes are pulled and our state is pushed.
    match crate::store::load(&app).await {
        Ok(json) => {
            if let Err(e) = crate::store::save(&app, json.clone()).await {
                record_error(&app, e);
            } else {
                record_success(&app);
            }
            let _ = app.emit("tasks-changed", &json);
        }
        Err(e) => record_error(&app, format!("server-mode initial sync: {e}")),
    }

    Ok(ServerSyncSetupResult {
        base_url: trimmed_url,
        main_doc_id: doc_id,
    })
}

#[tauri::command]
pub async fn clear_server_sync(app: AppHandle) -> Result<(), String> {
    let mut cfg = config::load().map_err(|e| e.to_string())?;
    cfg.server_base_url = String::new();
    cfg.server_main_doc_id = String::new();
    cfg.last_synced_at = None;
    cfg.last_sync_error = None;
    config::save(&cfg).map_err(|e| e.to_string())?;
    let _ = app.emit("sync-status", &current_status());
    Ok(())
}

/// Hit /healthz on the configured relay; surfaces true/false to the UI
/// for the "server reachable" status badge.
#[tauri::command]
pub async fn server_healthz() -> Result<bool, String> {
    let cfg = config::load().map_err(|e| e.to_string())?;
    if cfg.server_base_url.trim().is_empty() {
        return Ok(false);
    }
    let client = crate::server_client::ServerSyncClient::new(&cfg.server_base_url)
        .map_err(|e| e.to_string())?;
    Ok(client.healthz().await)
}
