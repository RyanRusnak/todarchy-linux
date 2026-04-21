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
}

pub fn current_status() -> SyncStatus {
    let cfg = config::load().unwrap_or_default();
    SyncStatus {
        folder: cfg.sync_folder,
        last_synced_at: cfg.last_synced_at,
        last_sync_error: cfg.last_sync_error,
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
