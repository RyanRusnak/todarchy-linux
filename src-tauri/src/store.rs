// store.rs — task storage.
//
// Canonical format is an Automerge binary doc (tasks.automerge). Every
// save also regenerates tasks.json as a derived view so the tod CLI and
// todarchy-waybar don't need any changes — they keep reading the same
// human-grep-able JSON they always have.
//
// Layout:
//   ~/.local/share/todarchy/tasks.automerge   canonical (CRDT-mergeable)
//   ~/.local/share/todarchy/tasks.json        derived view (CLI/Waybar)
//   ~/.local/share/todarchy/tasks.json.bak    previous JSON revision
//
// If the user configures a sync folder (see config.rs), we additionally
// mirror tasks.automerge there and merge any external writes on load.

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde_json::Value;
use tauri::AppHandle;
use tokio::fs;

use crate::doc::TaskDoc;

fn data_dir() -> Result<PathBuf> {
    let home = dirs::data_local_dir()
        .or_else(dirs::home_dir)
        .context("no data dir")?;
    let d = home.join("todarchy");
    std::fs::create_dir_all(&d)?;
    Ok(d)
}

fn automerge_path() -> Result<PathBuf> {
    Ok(data_dir()?.join("tasks.automerge"))
}

fn tasks_json_path() -> Result<PathBuf> {
    Ok(data_dir()?.join("tasks.json"))
}

/// Load the current task state as JSON. Internally:
///   1. Load local Automerge doc (empty if missing).
///   2. If a sync folder is configured, merge the remote Automerge doc in —
///      this picks up edits another device made while we were offline.
///   3. Write the merged doc back to disk so the two sources stay in lockstep.
///   4. Return the JSON projection the frontend consumes.
pub async fn load(_app: &AppHandle) -> Result<Value> {
    let local_path = automerge_path()?;
    let mut doc = TaskDoc::load(&local_path)?;

    // Fold in whatever the sync folder has (if configured).
    if let Some(sync_path) = crate::config::sync_doc_path()? {
        if sync_path.exists() {
            if let Ok(mut remote) = TaskDoc::load(&sync_path) {
                if let Err(e) = doc.merge(&mut remote) {
                    tracing::warn!("merge from sync folder failed: {e}");
                }
            }
        }
        // Whether or not merge happened, write back to both locations so they
        // converge: we may have had local-only edits that need to propagate.
        let _ = doc.save(&local_path);
        let _ = doc.save(&sync_path);
    } else {
        // First run: if tasks.automerge doesn't exist yet but a legacy
        // tasks.json does, migrate by seeding the doc from JSON. After this
        // save the JSON becomes a derived view.
        if !local_path.exists() {
            if let Ok(text) = fs::read_to_string(tasks_json_path()?).await {
                if let Ok(legacy) = serde_json::from_str::<Value>(&text) {
                    doc.apply_json(&legacy)?;
                    doc.save(&local_path)?;
                }
            }
        }
    }

    let json = doc.to_json();
    // Keep tasks.json up to date as the CLI/Waybar-facing view.
    write_json_view(&json).await?;
    Ok(json)
}

/// Save a full-state JSON payload. Applies the diff into the local Automerge
/// doc, persists it, regenerates tasks.json, and mirrors to the sync folder
/// if configured.
pub async fn save(_app: &AppHandle, data: Value) -> Result<()> {
    let local_path = automerge_path()?;
    let mut doc = TaskDoc::load(&local_path)?;

    // Always merge the sync-folder copy first so we never stomp another
    // device's concurrent edits.
    if let Some(sync_path) = crate::config::sync_doc_path()? {
        if sync_path.exists() {
            if let Ok(mut remote) = TaskDoc::load(&sync_path) {
                let _ = doc.merge(&mut remote);
            }
        }
    }

    doc.apply_json(&data)?;
    doc.save(&local_path)?;
    if let Some(sync_path) = crate::config::sync_doc_path()? {
        let _ = doc.save(&sync_path);
    }

    // Regenerate the JSON view from the merged doc so CLI/Waybar see the
    // same state the GUI sees.
    write_json_view(&doc.to_json()).await?;
    Ok(())
}

/// Explicit deletions. Called by the frontend whenever the user actually
/// intends to delete a task/project — unlike `save`, which is upsert-only
/// so concurrent inserts from other devices aren't silently wiped.
pub async fn delete_many(_app: &AppHandle, root_key: &str, ids: &[String]) -> Result<()> {
    let local_path = automerge_path()?;
    let mut doc = TaskDoc::load(&local_path)?;

    // Fold in remote state first so the delete persists even if another
    // device also edited the same id concurrently (Automerge tombstones
    // win over concurrent edits).
    if let Some(sync_path) = crate::config::sync_doc_path()? {
        if sync_path.exists() {
            if let Ok(mut remote) = TaskDoc::load(&sync_path) {
                let _ = doc.merge(&mut remote);
            }
        }
    }

    for id in ids {
        doc.delete(root_key, id)?;
    }
    doc.save(&local_path)?;
    if let Some(sync_path) = crate::config::sync_doc_path()? {
        let _ = doc.save(&sync_path);
    }
    write_json_view(&doc.to_json()).await?;
    Ok(())
}

async fn write_json_view(data: &Value) -> Result<()> {
    let path = tasks_json_path()?;
    let bak = path.with_extension("json.bak");
    let tmp = path.with_extension("json.tmp");
    if path.exists() {
        let _ = fs::copy(&path, &bak).await;
    }
    let pretty = serde_json::to_vec_pretty(data)?;
    fs::write(&tmp, &pretty).await?;
    fs::rename(&tmp, &path).await?;
    Ok(())
}
