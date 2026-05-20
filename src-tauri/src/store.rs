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
use crate::server_client::ServerSyncClient;

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

    // Server-relay mode: pull the remote main doc + every opened shared
    // file from the relay before any local merge runs. Treated like
    // another sync transport — does NOT supersede a configured sync
    // folder; folder + server can both be active and converge through
    // the same Automerge merges.
    if let Some((base, doc_id)) = crate::config::server_config()? {
        match pull_from_server(&base, &doc_id, &mut doc).await {
            Ok(()) => crate::sync::record_success(app),
            Err(e) => crate::sync::record_error(app, format!("server pull: {e}")),
        }
    }

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

    // Mirror the freshly-saved bytes to the relay if server-sync is on.
    // Unconditional PUT — matches the iOS app's "local-first; always
    // overwrite the server" semantics. Concurrent peer edits get
    // reconciled via Automerge on the next pull.
    if let Some((base, doc_id)) = crate::config::server_config()? {
        if let Err(e) = push_to_server(&base, &doc_id, &mut doc).await {
            sync_error.get_or_insert_with(|| format!("server push: {e}"));
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

/// Pull the main doc (and every opened shared envelope) from the relay
/// and merge their bytes into `doc` / the per-project stores. Errors
/// from individual GETs are logged but don't abort the rest — partial
/// progress is better than none.
async fn pull_from_server(base_url: &str, main_doc_id: &str, doc: &mut TaskDoc) -> Result<()> {
    let client = ServerSyncClient::new(base_url)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    if let Some((bytes, _etag)) = client
        .get(main_doc_id)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?
    {
        let mut remote = TaskDoc::from_bytes(&bytes)
            .with_context(|| "parsing main-doc bytes from server")?;
        doc.merge(&mut remote)
            .with_context(|| "merging server main-doc bytes into local doc")?;
    }

    // Per-project shared envelopes: pull each one we have a key for and
    // merge through the SharedProjectManager so the doc-of-record on
    // disk stays in lockstep with the server. The manager owns the
    // decrypt step and the merge into the in-memory shared store.
    if let Some(manager) = crate::shared::current_manager()? {
        let projection = doc.to_json();
        let shared_pids: Vec<String> = projection
            .get("projects")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter(|p| p.get("isShared").and_then(|v| v.as_bool()).unwrap_or(false))
                    .filter_map(|p| p.get("id").and_then(|v| v.as_str()).map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        for pid in shared_pids {
            if !manager.has_key(&pid) { continue; }
            match client.get(&pid).await {
                Ok(Some((bytes, _etag))) => {
                    if let Err(e) = manager.absorb_remote_envelope(&pid, &bytes) {
                        tracing::warn!("server pull for shared {pid} failed to merge: {e}");
                    }
                }
                Ok(None) => { /* no remote yet, or 304 */ }
                Err(e) => {
                    tracing::warn!("server GET shared {pid} failed: {e}");
                }
            }
        }
    }
    Ok(())
}

/// Push the main doc bytes to the relay. Also pushes every opened
/// shared envelope — same unconditional semantics as iOS. Errors are
/// surfaced to the caller (which folds them into sync-status).
async fn push_to_server(base_url: &str, main_doc_id: &str, doc: &mut TaskDoc) -> Result<()> {
    let client = ServerSyncClient::new(base_url)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let bytes = doc.to_bytes();
    client
        .put(main_doc_id, bytes)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    if let Some(manager) = crate::shared::current_manager()? {
        for (pid, bytes) in manager.opened_envelope_bytes() {
            if let Err(e) = client.put(&pid, bytes).await {
                tracing::warn!("server PUT shared {pid} failed: {e}");
            }
        }
    }
    Ok(())
}
