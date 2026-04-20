// todarchy — Tauri entrypoint
//
// Responsibilities:
//   - spin up the theme watcher (omarchy) and emit `theme-changed` events
//   - expose task CRUD commands to the frontend
//   - forward desktop notifications for due tasks
//
// See README.md for architecture.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod theme;
mod store;
mod sync;
mod notify;
mod doc;

use tauri::AppHandle;

#[tauri::command]
async fn load_tasks(app: AppHandle) -> Result<serde_json::Value, String> {
    store::load(&app).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn save_tasks(app: AppHandle, tasks: serde_json::Value) -> Result<(), String> {
    store::save(&app, tasks).await.map_err(|e| e.to_string())
}

#[tauri::command]
fn current_theme() -> Result<theme::ThemeTokens, String> {
    theme::read_current().map_err(|e| e.to_string())
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .setup(|app| {
            let handle = app.handle().clone();

            // Kick off the Omarchy theme watcher. Emits `theme-changed`
            // to the frontend whenever the user runs `omarchy-theme-set`.
            tauri::async_runtime::spawn(async move {
                if let Err(e) = theme::spawn_watcher(handle).await {
                    tracing::error!("theme watcher failed: {e}");
                }
            });

            // Periodic "are any tasks due?" check → libnotify via zbus
            let handle2 = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                notify::run_loop(handle2).await;
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            load_tasks,
            save_tasks,
            current_theme,
            sync::enroll,
            sync::sign_in,
            sync::recover_from_phrase,
            sync::push,
            sync::pull,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
