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
mod sync_watcher;
mod notify;
mod doc;
mod config;
mod cryptobox;
mod sharelink;
mod keystore;
mod per_project;
mod shared;
mod server_client;

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
async fn delete_tasks(app: AppHandle, ids: Vec<String>) -> Result<(), String> {
    store::delete_many(&app, "tasks", &ids).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn delete_projects(app: AppHandle, ids: Vec<String>) -> Result<(), String> {
    store::delete_many(&app, "projects", &ids).await.map_err(|e| e.to_string())
}

#[tauri::command]
fn current_theme() -> Result<theme::ThemeTokens, String> {
    theme::read_current().map_err(|e| e.to_string())
}

// Plain text file I/O for export/import. The frontend picks the path via the
// dialog plugin, then hands it here — we write/read through std::fs so the
// destination isn't constrained by the fs plugin's scoped allowlist (the user
// explicitly chose the path in a save/open dialog, so it's already consented).
#[tauri::command]
fn write_text_file(path: String, contents: String) -> Result<(), String> {
    std::fs::write(&path, contents).map_err(|e| format!("write {path}: {e}"))
}

#[tauri::command]
fn read_text_file(path: String) -> Result<String, String> {
    std::fs::read_to_string(&path).map_err(|e| format!("read {path}: {e}"))
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
        // Handles `todarchy://share/...` URLs opened from a browser
        // or another app. Routed to share_accept via an OnOpenUrl
        // event listener registered in the setup hook below.
        .plugin(tauri_plugin_deep_link::init())
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

            // Watch the configured sync folder for another device's writes,
            // merge them into the local doc, and push a `tasks-changed`
            // event to the frontend. Noop when no sync folder is set.
            let handle3 = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                sync_watcher::run_loop(handle3).await;
            });

            // Server-relay polling. Independent of the folder watcher
            // above — server mode can run instead of, or alongside, a
            // sync folder. Hits the relay every 10 s when configured;
            // sleeps cheaply when it isn't.
            let handle3b = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                sync_watcher::server_poll_loop(handle3b).await;
            });

            // Route `todarchy://share/...` URLs (opened from a browser
            // or another app) to the share_accept command. The user
            // sees the project pop into their sidebar without having
            // to paste the link into the UI.
            let handle4 = app.handle().clone();
            use tauri_plugin_deep_link::DeepLinkExt;
            app.deep_link().on_open_url(move |event| {
                for url in event.urls() {
                    let url_str = url.to_string();
                    if !url_str.starts_with("todarchy://share/") {
                        continue;
                    }
                    let handle = handle4.clone();
                    tauri::async_runtime::spawn(async move {
                        if let Err(e) = shared::share_accept(handle, url_str.clone()).await {
                            tracing::warn!("share_accept failed for {url_str}: {e}");
                        }
                    });
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            load_tasks,
            save_tasks,
            delete_tasks,
            delete_projects,
            current_theme,
            write_text_file,
            read_text_file,
            sync::get_sync_folder,
            sync::set_sync_folder,
            sync::clear_sync_folder,
            sync::get_sync_status,
            sync::set_server_sync,
            sync::clear_server_sync,
            sync::server_healthz,
            shared::share_promote,
            shared::share_accept,
            shared::share_leave,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
