// sync_watcher.rs — watches the user's sync folder for another device's
// writes and merges them into the local Automerge doc.
//
// Runs as a tokio task spawned from main.rs. Re-resolves the configured
// folder on a timer (so we notice `set_sync_folder` / `clear_sync_folder`
// without restarting) and uses the `notify` crate to catch writes to
// `<sync_folder>/tasks.automerge`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Result;
use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc;

use crate::config as app_config;
use crate::doc::TaskDoc;

pub async fn run_loop(app: AppHandle) {
    // Poll the config for the currently-configured folder. When it changes
    // (user picks a new folder / clears it / we start for the first time),
    // rebuild the watcher.
    let mut active: Option<PathBuf> = None;
    let mut rx_opt: Option<mpsc::Receiver<notify::Result<Event>>> = None;
    // Hold the watcher so its inotify handle stays alive.
    let mut _watcher: Option<RecommendedWatcher> = None;

    loop {
        let desired = app_config::sync_doc_path()
            .ok()
            .flatten()
            .map(|p| {
                // We watch the folder (non-recursive) rather than the file
                // itself — the file is atomically renamed into place, which
                // means it gets a new inode each time and a file-level
                // watch would miss the swap.
                p.parent().map(Path::to_path_buf)
            })
            .flatten();

        if desired != active {
            // Rebuild the watcher for the new folder (or tear it down if
            // sync was just cleared).
            if let Some(ref folder) = desired {
                match build_watcher(folder) {
                    Ok((w, rx)) => {
                        tracing::info!("watching sync folder: {}", folder.display());
                        _watcher = Some(w);
                        rx_opt = Some(rx);
                    }
                    Err(e) => {
                        tracing::warn!(
                            "failed to watch sync folder {}: {e}",
                            folder.display()
                        );
                        _watcher = None;
                        rx_opt = None;
                    }
                }
            } else {
                _watcher = None;
                rx_opt = None;
            }
            active = desired;
        }

        // Drain any events that arrived. We drain until quiet for 400ms so
        // a single atomic-rename burst is collapsed into one reload.
        if let Some(rx) = rx_opt.as_mut() {
            let mut saw_write = false;
            // First try a short blocking recv. If nothing arrived in 1s,
            // loop back to re-check config.
            match tokio::time::timeout(Duration::from_millis(1000), rx.recv()).await {
                Ok(Some(ev)) => {
                    if interesting(&ev) { saw_write = true; }
                }
                Ok(None) => {
                    rx_opt = None;
                    continue;
                }
                Err(_) => continue,
            }
            // Drain the tail of the burst.
            loop {
                match tokio::time::timeout(Duration::from_millis(400), rx.recv()).await {
                    Ok(Some(ev)) => {
                        if interesting(&ev) { saw_write = true; }
                    }
                    Ok(None) => break,
                    Err(_) => break,
                }
            }
            if saw_write {
                if let Err(e) = apply_sync_update(&app).await {
                    tracing::warn!("sync merge failed: {e}");
                }
            }
        } else {
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }
}

fn build_watcher(
    folder: &Path,
) -> Result<(RecommendedWatcher, mpsc::Receiver<notify::Result<Event>>)> {
    let (tx, rx) = mpsc::channel::<notify::Result<Event>>(32);
    let mut w = RecommendedWatcher::new(
        move |res| {
            let _ = tx.blocking_send(res);
        },
        Config::default().with_poll_interval(Duration::from_secs(2)),
    )?;
    w.watch(folder, RecursiveMode::NonRecursive)?;
    Ok((w, rx))
}

fn interesting(ev: &notify::Result<Event>) -> bool {
    let Ok(e) = ev else { return false };
    // We only care about writes to tasks.automerge (or its .tmp sibling
    // during an atomic rename).
    e.paths.iter().any(|p| {
        p.file_name()
            .and_then(|n| n.to_str())
            .map(|n| n == "tasks.automerge" || n == "tasks.automerge.tmp")
            .unwrap_or(false)
    })
}

async fn apply_sync_update(app: &AppHandle) -> Result<()> {
    let Some(sync_path) = app_config::sync_doc_path()? else { return Ok(()); };
    if !sync_path.exists() { return Ok(()); }

    // Load both docs, merge remote into local, persist, emit.
    let local_path = crate::doc::default_doc_path()?;
    let mut local = TaskDoc::load(&local_path)?;
    let mut remote = TaskDoc::load(&sync_path)?;

    local.merge(&mut remote)?;
    local.save(&local_path)?;
    local.save(&sync_path)?; // push the merged view back out

    let json = local.to_json();
    let _ = app.emit("tasks-changed", &json);
    tracing::info!("merged external sync update");
    Ok(())
}
