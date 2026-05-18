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
pub async fn load(app: &AppHandle) -> Result<Value> {
    let local_path = automerge_path()?;
    let mut doc = TaskDoc::load(&local_path)?;

    // Fold in whatever the sync folder has (if configured). Every branch
    // reports to the `sync-status` event so the UI reflects reality.
    if let Some(sync_path) = crate::config::sync_doc_path()? {
        let mut step_error: Option<String> = None;

        if sync_path.exists() {
            match TaskDoc::load(&sync_path) {
                Ok(mut remote) => {
                    if let Err(e) = doc.merge(&mut remote) {
                        step_error = Some(format!("merge: {e}"));
                    }
                }
                Err(e) => step_error = Some(format!("load remote: {e}")),
            }
        }
        // Whether or not merge happened, write back to both locations so they
        // converge: we may have had local-only edits that need to propagate.
        if let Err(e) = doc.save(&local_path) {
            step_error.get_or_insert_with(|| format!("save local: {e}"));
        }
        if let Err(e) = doc.save_overwrite(&sync_path) {
            step_error.get_or_insert_with(|| format!("save to sync folder: {e}"));
        }

        match step_error {
            Some(reason) => crate::sync::record_error(app, reason),
            None => crate::sync::record_success(app),
        }
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

    // If sharing is set up, fold each opened shared file's
    // authoritative tasks + project metadata into the projection. The
    // unioned view is what the frontend renders and what the CLI/Waybar
    // see via tasks.json.
    let json = match crate::shared::current_manager().ok().flatten() {
        Some(manager) => manager.load_union(&doc).unwrap_or_else(|e| {
            tracing::warn!("load_union failed, falling back to main doc only: {e}");
            doc.to_json()
        }),
        None => doc.to_json(),
    };
    write_json_view(&json).await?;
    Ok(json)
}

/// Save a full-state JSON payload. Applies the diff into the local Automerge
/// doc, persists it, regenerates tasks.json, and mirrors to the sync folder
/// if configured.
pub async fn save(app: &AppHandle, data: Value) -> Result<()> {
    let local_path = automerge_path()?;
    let mut doc = TaskDoc::load(&local_path)?;

    let sync_path = crate::config::sync_doc_path()?;
    let mut sync_error: Option<String> = None;

    // Always merge the sync-folder copy first so we never stomp another
    // device's concurrent edits.
    if let Some(ref sp) = sync_path {
        if sp.exists() {
            match TaskDoc::load(sp) {
                Ok(mut remote) => {
                    if let Err(e) = doc.merge(&mut remote) {
                        sync_error = Some(format!("merge: {e}"));
                    }
                }
                Err(e) => sync_error = Some(format!("load remote: {e}")),
            }
        }
    }

    // Partition the incoming snapshot. Tasks whose `list` belongs to
    // a shared project are routed into that project's encrypted file;
    // the main doc only sees the remainder. The project records stay
    // in BOTH the main doc (as stubs for sidebar rendering on peers
    // without the key) and the shared file (authoritative copy).
    let main_data = match crate::shared::current_manager().ok().flatten() {
        Some(manager) => manager.save_split(&data).unwrap_or_else(|e| {
            tracing::warn!("save_split failed, applying snapshot as-is: {e}");
            data.clone()
        }),
        None => data.clone(),
    };

    doc.apply_json(&main_data)?;
    doc.save(&local_path)?;
    if let Some(ref sp) = sync_path {
        if let Err(e) = doc.save_overwrite(sp) {
            sync_error.get_or_insert_with(|| format!("save to sync folder: {e}"));
        }
    }

    // Regenerate the unioned JSON view so CLI/Waybar see the same state
    // the GUI sees (main doc + every opened shared store overlaid).
    let projection = match crate::shared::current_manager().ok().flatten() {
        Some(manager) => manager.load_union(&doc).unwrap_or_else(|_| doc.to_json()),
        None => doc.to_json(),
    };
    write_json_view(&projection).await?;

    // Report status — only if a sync folder is configured.
    if sync_path.is_some() {
        match sync_error {
            Some(reason) => crate::sync::record_error(app, reason),
            None => crate::sync::record_success(app),
        }
    }
    Ok(())
}

/// Explicit deletions. Called by the frontend whenever the user actually
/// intends to delete a task/project — unlike `save`, which is upsert-only
/// so concurrent inserts from other devices aren't silently wiped.
pub async fn delete_many(app: &AppHandle, root_key: &str, ids: &[String]) -> Result<()> {
    let local_path = automerge_path()?;
    let mut doc = TaskDoc::load(&local_path)?;

    let sync_path = crate::config::sync_doc_path()?;
    let mut sync_error: Option<String> = None;

    if let Some(ref sp) = sync_path {
        if sp.exists() {
            match TaskDoc::load(sp) {
                Ok(mut remote) => {
                    if let Err(e) = doc.merge(&mut remote) {
                        sync_error = Some(format!("merge: {e}"));
                    }
                }
                Err(e) => sync_error = Some(format!("load remote: {e}")),
            }
        }
    }

    for id in ids {
        doc.delete(root_key, id)?;
    }
    doc.save(&local_path)?;
    if let Some(ref sp) = sync_path {
        if let Err(e) = doc.save_overwrite(sp) {
            sync_error.get_or_insert_with(|| format!("save to sync folder: {e}"));
        }
    }
    write_json_view(&doc.to_json()).await?;

    if sync_path.is_some() {
        match sync_error {
            Some(reason) => crate::sync::record_error(app, reason),
            None => crate::sync::record_success(app),
        }
    }
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
