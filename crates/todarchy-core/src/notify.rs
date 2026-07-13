// notify.rs — desktop notifications for due tasks.
//
// Runs a tick loop (60s). Loads tasks, finds any whose deferral just
// expired, and surfaces them via the front end's notifier. The Tauri
// build routed this through tauri-plugin-notification; the TUI's sink
// shells out to `notify-send`.
//
// De-duplication: the GUI/TUI auto-clears `deferUntil` once the moment
// passes, so a task only matches the ~90s window once.

use std::sync::Arc;
use std::time::Duration;

use crate::EventSink;

pub async fn run_loop(sink: Arc<dyn EventSink>) {
    loop {
        tokio::time::sleep(Duration::from_secs(60)).await;
        if let Err(e) = tick(sink.as_ref()).await {
            tracing::warn!("notify tick failed: {e}");
        }
    }
}

async fn tick(sink: &dyn EventSink) -> anyhow::Result<()> {
    let tasks = crate::store::load(sink).await?;
    let Some(arr) = tasks.get("tasks").and_then(|t| t.as_array()) else { return Ok(()); };

    let now_ms = now_millis();

    for t in arr {
        let Some(id) = t.get("id").and_then(|v| v.as_str()) else { continue };

        // Skip completed tasks.
        if t.get("doneAt").and_then(|v| v.as_i64()).is_some() {
            continue;
        }

        // When a deferral expires, surface the task again via a notification.
        // The UI auto-clears `deferUntil` once the moment passes, so this only
        // fires for tasks whose defer window ended in the last ~minute.
        if let Some(defer) = t.get("deferUntil").and_then(|v| v.as_i64()) {
            let delta = now_ms - defer;
            if (0..90_000).contains(&delta) {
                let title = t.get("title").and_then(|v| v.as_str()).unwrap_or("task");
                sink.notify("back on your list", title);
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
