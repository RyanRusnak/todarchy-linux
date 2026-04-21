// sync.rs — Tauri commands for the v0.2 folder-sync setting.
//
// The user picks a filesystem path (typically one the OS syncs across
// devices — iCloud Drive, Dropbox, Syncthing, etc.) and todarchy mirrors
// its Automerge store there on every save. The sync_watcher module
// picks up external writes (another device pushing edits) and merges
// them into the local doc live.
//
// The older age / BIP39 / relay-server design is gone; v0.2 reuses the
// file sync the user already has configured on their OS.

use tauri::{AppHandle, Emitter};

use crate::config;

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
    config::save(&cfg).map_err(|e| e.to_string())?;
    tracing::info!("sync folder set: {}", cfg.sync_folder);

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
            let _ = crate::store::save(&app, json.clone()).await;
            let _ = app.emit("tasks-changed", &json);
        }
        Err(e) => tracing::warn!("set_sync_folder: store::load failed: {e}"),
    }
    Ok(())
}

#[tauri::command]
pub async fn clear_sync_folder(app: AppHandle) -> Result<(), String> {
    let mut cfg = config::load().map_err(|e| e.to_string())?;
    cfg.sync_folder = String::new();
    config::save(&cfg).map_err(|e| e.to_string())?;
    let _ = app.emit("tasks-changed", serde_json::Value::Null);
    Ok(())
}
