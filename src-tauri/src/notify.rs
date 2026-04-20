// notify.rs — desktop notifications for due tasks.
//
// Runs a tick loop (60s). Loads tasks, finds any that became due in the
// last minute or are overdue by an exact number of hours (1h, 4h, 24h),
// and sends `notify-send`-equivalent via the tauri-plugin-notification.
//
// De-duplication: last-notified timestamp lives in
//   ~/.local/share/todarchy/state.json → notified[task_id] = unix_ts

use std::time::Duration;

use tauri::AppHandle;
use tauri_plugin_notification::NotificationExt;

pub async fn run_loop(app: AppHandle) {
    loop {
        tokio::time::sleep(Duration::from_secs(60)).await;
        if let Err(e) = tick(&app).await {
            tracing::warn!("notify tick failed: {e}");
        }
    }
}

async fn tick(app: &AppHandle) -> anyhow::Result<()> {
    let tasks = crate::store::load(app).await?;
    let Some(arr) = tasks.get("tasks").and_then(|t| t.as_array()) else { return Ok(()); };

    let now_ms = now_millis();

    for t in arr {
        let Some(id) = t.get("id").and_then(|v| v.as_str()) else { continue };

        // Skip completed tasks.
        if t.get("doneAt").and_then(|v| v.as_i64()).is_some() {
            continue;
        }

        // When a deferral expires, surface the task again via a notification.
        // The GUI auto-clears `deferUntil` once the moment passes, so this only
        // fires for tasks whose defer window ended in the last ~minute.
        if let Some(defer) = t.get("deferUntil").and_then(|v| v.as_i64()) {
            let delta = now_ms - defer;
            if (0..90_000).contains(&delta) {
                let title = t.get("title").and_then(|v| v.as_str()).unwrap_or("task");
                let _ = app.notification().builder()
                    .title("back on your list")
                    .body(title)
                    .show();
                tracing::info!("notified un-deferred task {id}");
            }
        }
    }
    Ok(())
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
