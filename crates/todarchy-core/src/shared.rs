// shared.rs — coordinator for per-project encrypted shared files.
//
// The sync folder is laid out like this once shared projects exist:
//
//     <sync-folder>/tasks.automerge                       ← personal doc
//     <sync-folder>/shared_<project_id>.automerge.enc     ← one per joined shared project
//
// A project is "shared on this device" when all three are true:
//   1. The project record in the main doc carries `isShared = true`.
//   2. This device holds the symmetric key for `<project_id>` in the keyring.
//   3. (Eventually) the encrypted file exists in the sync folder — but
//      we tolerate it being absent on first launch after accept, since
//      Dropbox/iCloud may still be downloading.
//
// Two main behaviors flow through this module:
//
//   - **load_union**: read main doc → for every `isShared` project we
//     hold the key for, decrypt the sibling file → replace that
//     project's metadata + tasks in the UI projection with the
//     authoritative copy from the encrypted file.
//
//   - **save_split**: partition the incoming snapshot's tasks by
//     `task.list`. Tasks whose list is an opened shared project go
//     into that project's `PerProjectStore`; everything else stays in
//     the main doc. The project record itself stays in BOTH places
//     (main doc keeps the stub for sidebar rendering on devices that
//     don't have the key; shared file holds authoritative metadata).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use once_cell::sync::Lazy;
use serde_json::{json, Value as Json};

use crate::cryptobox;
use crate::EventSink;
use crate::doc::TaskDoc;
#[cfg(test)]
use crate::keystore::InMemoryKeyStore;
use crate::keystore::{KeyStore, LibSecretKeyStore};
use crate::per_project::{self, PerProjectStore};
use crate::sharelink;

/// All the state needed to read, write, and merge shared projects.
///
/// Holds the sync folder path, a key store (libsecret in prod, in-memory
/// in tests), and a cache of opened `PerProjectStore`s so we don't
/// re-decrypt on every save.
pub struct SharedProjectManager {
    folder: PathBuf,
    key_store: Box<dyn KeyStore>,
    /// Lazy cache of per-project stores. Opened on first use and reused
    /// across ticks so we're not re-running CryptoBox.open() for every
    /// frontend save. Reset on `forget`.
    opened: Mutex<HashMap<String, PerProjectStore>>,
}

impl SharedProjectManager {
    /// Production constructor. Uses libsecret-backed key storage.
    pub fn new(folder: impl Into<PathBuf>) -> Self {
        Self {
            folder: folder.into(),
            key_store: Box::new(LibSecretKeyStore::new()),
            opened: Mutex::new(HashMap::new()),
        }
    }

    /// Test constructor — in-memory keys so tests don't touch D-Bus.
    #[cfg(test)]
    pub fn new_in_memory(folder: impl Into<PathBuf>) -> Self {
        Self {
            folder: folder.into(),
            key_store: Box::new(InMemoryKeyStore::new()),
            opened: Mutex::new(HashMap::new()),
        }
    }

    pub fn folder(&self) -> &Path { &self.folder }

    /// Canonical on-disk path for a shared project's encrypted file.
    pub fn file_path(&self, project_id: &str) -> PathBuf {
        self.folder.join(per_project::filename_for(project_id))
    }

    /// Is this project marked shared AND we hold the key locally? If
    /// either is missing, this device sees the project but not its
    /// tasks (it hasn't joined yet, on this device).
    pub fn has_key(&self, project_id: &str) -> bool {
        matches!(self.key_store.load(project_id), Ok(Some(_)))
    }

    /// Promote an existing main-doc project to a shared encrypted file.
    /// Generates a key, seeds the encrypted file with the project's
    /// current tasks + metadata (with `isShared = true`), persists the
    /// key, returns the share link.
    ///
    /// Caller is responsible for following up on the main doc:
    /// 1. set the project's `isShared` to true,
    /// 2. tombstone the project's tasks (they've moved to the shared file).
    pub fn promote_to_shared(
        &self,
        project_record: &Json,
        tasks_for_project: &[Json],
    ) -> Result<PromoteResult> {
        let project_id = project_record
            .get("id")
            .and_then(|v| v.as_str())
            .context("promote_to_shared: project record has no id")?
            .to_string();

        let path = self.file_path(&project_id);
        if path.exists() {
            anyhow::bail!("shared file already exists at {}", path.display());
        }
        if self.has_key(&project_id) {
            anyhow::bail!("a key is already stored for project {project_id}");
        }
        let key = cryptobox::generate_key();
        self.key_store
            .save(&project_id, &key)
            .with_context(|| format!("saving key for {project_id}"))?;

        let mut shared_project = project_record.clone();
        if let Some(obj) = shared_project.as_object_mut() {
            obj.insert("isShared".to_string(), json!(true));
        }
        let snapshot = json!({
            "tasks": tasks_for_project,
            "projects": [shared_project],
            "contexts": []
        });

        let mut store = PerProjectStore::open(&path, &project_id, &key)?;
        store.save(&snapshot)?;
        self.cache_store(&project_id, store);

        Ok(PromoteResult {
            project_id: project_id.clone(),
            share_link: sharelink::encode(&project_id, &key),
        })
    }

    /// Accept a share link: persist the key under the project id. The
    /// encrypted file itself is expected to arrive via the sync
    /// transport (Dropbox / iCloud / Syncthing). If the file is already
    /// present we open it immediately so the UI can show the project's
    /// real name/icon; otherwise the caller's stub is what the user
    /// sees until sync delivers the bytes.
    ///
    /// Idempotent: re-accepting the same link is a no-op.
    pub fn accept_share_link(&self, url: &str) -> Result<AcceptResult> {
        let payload = sharelink::decode(url).map_err(|e| anyhow::anyhow!("{e}"))?;
        // Persist the key (overwrite if already there — same key, same
        // value, harmless; different key with same id, we trust the
        // latest paste).
        self.key_store
            .save(&payload.project_id, &payload.key)
            .with_context(|| format!("saving key for {}", payload.project_id))?;

        let path = self.file_path(&payload.project_id);
        let project_metadata = if path.exists() {
            // Open + read the project record so the caller can publish
            // it into the main doc as a non-stub entry.
            let store = PerProjectStore::open(&path, &payload.project_id, &payload.key)?;
            let snap = store.snapshot();
            let project = snap["projects"]
                .as_array()
                .and_then(|arr| arr.first())
                .cloned();
            self.cache_store(&payload.project_id, store);
            project
        } else {
            None
        };

        Ok(AcceptResult {
            project_id: payload.project_id,
            project_metadata,
        })
    }

    /// Forget a shared project on this device: delete the key, optionally
    /// remove the local encrypted file. Peers still have their copies —
    /// this is "leave the share," not "delete the project globally."
    pub fn forget_locally(&self, project_id: &str) -> Result<()> {
        self.key_store.delete(project_id)?;
        self.opened.lock().unwrap().remove(project_id);
        let path = self.file_path(project_id);
        let _ = std::fs::remove_file(path);
        Ok(())
    }

    /// Read the main doc's projection of shared projects (anywhere
    /// `isShared = true`) and union in each shared file's authoritative
    /// tasks + metadata.
    ///
    /// Returns the updated JSON projection the frontend should render.
    pub fn load_union(&self, main_doc: &TaskDoc) -> Result<Json> {
        let mut projection = main_doc.to_json();
        let projects_in_main: Vec<Json> = projection
            .get("projects")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        // Collect per-project snapshots so we can overlay all at once.
        let mut shared_tasks_by_pid: HashMap<String, Vec<Json>> = HashMap::new();
        let mut authoritative_project_by_pid: HashMap<String, Json> = HashMap::new();

        for project in &projects_in_main {
            let is_shared = project.get("isShared").and_then(|v| v.as_bool()).unwrap_or(false);
            if !is_shared { continue; }
            let pid = match project.get("id").and_then(|v| v.as_str()) {
                Some(s) => s.to_string(),
                None => continue,
            };
            let key = match self.key_store.load(&pid) {
                Ok(Some(k)) => k,
                _ => continue, // joined elsewhere but not on this device
            };
            let path = self.file_path(&pid);
            if !path.exists() { continue; }
            let snap = self.with_store(&pid, &key, &path, |store| Ok(store.snapshot()))?;
            if let Some(tasks) = snap.get("tasks").and_then(|v: &Json| v.as_array()) {
                shared_tasks_by_pid.insert(pid.clone(), tasks.clone());
            }
            if let Some(authoritative) = snap
                .get("projects")
                .and_then(|v: &Json| v.as_array())
                .and_then(|arr| arr.first().cloned())
            {
                authoritative_project_by_pid.insert(pid, authoritative);
            }
        }

        // Replace main-doc tasks whose `list` matches an opened shared
        // project with the shared file's authoritative copy.
        if let Some(tasks_arr) = projection.get_mut("tasks").and_then(|v| v.as_array_mut()) {
            tasks_arr.retain(|t| {
                let list_id = t.get("list").and_then(|v| v.as_str()).unwrap_or("");
                !shared_tasks_by_pid.contains_key(list_id)
            });
            for (_, mut shared_tasks) in shared_tasks_by_pid {
                tasks_arr.append(&mut shared_tasks);
            }
        }

        // Overwrite project metadata for shared projects with the
        // authoritative copy from the shared file.
        if !authoritative_project_by_pid.is_empty() {
            if let Some(projects_arr) = projection.get_mut("projects").and_then(|v| v.as_array_mut()) {
                for p in projects_arr.iter_mut() {
                    let pid = p.get("id").and_then(|v| v.as_str()).map(|s| s.to_string());
                    if let Some(pid) = pid {
                        if let Some(auth) = authoritative_project_by_pid.get(&pid) {
                            *p = auth.clone();
                        }
                    }
                }
            }
        }

        Ok(projection)
    }

    /// Split a frontend snapshot across the main doc and any opened
    /// shared projects. Returns the snapshot to apply to the main doc;
    /// the shared parts are written here, so the caller still drives
    /// the main-doc save through the usual `store.rs` flow.
    pub fn save_split(&self, snapshot: &Json) -> Result<Json> {
        let mut shared_pids: Vec<String> = Vec::new();
        if let Some(projects_arr) = snapshot.get("projects").and_then(|v| v.as_array()) {
            for p in projects_arr {
                let is_shared = p.get("isShared").and_then(|v| v.as_bool()).unwrap_or(false);
                if !is_shared { continue; }
                let pid = match p.get("id").and_then(|v| v.as_str()) {
                    Some(s) => s.to_string(),
                    None => continue,
                };
                if self.has_key(&pid) {
                    shared_pids.push(pid);
                }
            }
        }

        if shared_pids.is_empty() {
            return Ok(snapshot.clone());
        }

        // Build per-pid task buckets + per-pid project records.
        let mut tasks_by_pid: HashMap<String, Vec<Json>> = HashMap::new();
        let mut project_by_pid: HashMap<String, Json> = HashMap::new();
        for pid in &shared_pids {
            tasks_by_pid.insert(pid.clone(), Vec::new());
        }
        if let Some(arr) = snapshot.get("tasks").and_then(|v| v.as_array()) {
            for t in arr {
                let list_id = t.get("list").and_then(|v| v.as_str()).unwrap_or("").to_string();
                if shared_pids.contains(&list_id) {
                    tasks_by_pid.entry(list_id).or_default().push(t.clone());
                }
            }
        }
        if let Some(arr) = snapshot.get("projects").and_then(|v| v.as_array()) {
            for p in arr {
                if let Some(pid) = p.get("id").and_then(|v| v.as_str()) {
                    if shared_pids.contains(&pid.to_string()) {
                        project_by_pid.insert(pid.to_string(), p.clone());
                    }
                }
            }
        }

        // Save each shared project's bucket into its encrypted file.
        for pid in &shared_pids {
            let key = match self.key_store.load(pid)? {
                Some(k) => k,
                None => continue,
            };
            let path = self.file_path(pid);
            let tasks = tasks_by_pid.get(pid).cloned().unwrap_or_default();
            let project_record = project_by_pid
                .get(pid)
                .cloned()
                .unwrap_or_else(|| json!({ "id": pid, "name": pid, "icon": "folder", "accent": "#7aa2f7", "isShared": true }));
            let shared_snapshot = json!({
                "tasks": tasks,
                "projects": [project_record],
                "contexts": []
            });
            self.with_store(pid, &key, &path, |store| store.save(&shared_snapshot))?;
        }

        // Build the main-doc snapshot: same as input but with shared
        // projects' tasks stripped out (their authoritative copy lives
        // in the encrypted file now). The project records themselves
        // STAY in the main doc — peers that don't hold the key still
        // need a sidebar entry to see "you've been invited."
        let mut main = snapshot.clone();
        if let Some(arr) = main.get_mut("tasks").and_then(|v| v.as_array_mut()) {
            arr.retain(|t| {
                let list_id = t.get("list").and_then(|v| v.as_str()).unwrap_or("");
                !shared_pids.contains(&list_id.to_string())
            });
        }
        Ok(main)
    }

    /// Sweep every opened shared store for conflict-copy files the sync
    /// daemon may have produced. Called periodically by the sync
    /// watcher. Returns the total number of files absorbed across all
    /// shared projects.
    pub fn ingest_all_conflict_copies(&self) -> Result<usize> {
        let mut total = 0usize;
        let mut opened = self.opened.lock().unwrap();
        for store in opened.values_mut() {
            total += store.ingest_conflict_copies().unwrap_or(0);
        }
        Ok(total)
    }

    /// Merge an encrypted envelope (just fetched from the relay) into
    /// the opened per-project store for `project_id`. Lazy-opens the
    /// store if it isn't cached yet, so a server-pull that arrives
    /// before the file watcher fires still lands correctly.
    pub fn absorb_remote_envelope(&self, project_id: &str, bytes: &[u8]) -> Result<bool> {
        let key = match self.key_store.load(project_id)? {
            Some(k) => k,
            None => return Ok(false),
        };
        let path = self.file_path(project_id);
        self.with_store(project_id, &key, &path, |store| store.merge_encrypted(bytes))
    }

    /// Return the on-disk envelope bytes for every shared project this
    /// device has opened (or has a file for). Used by the server-sync
    /// push path: each envelope is sent to `/doc/<project_id>` after a
    /// local save so peers can pull updates from the relay.
    pub fn opened_envelope_bytes(&self) -> Vec<(String, Vec<u8>)> {
        let opened = self.opened.lock().unwrap();
        opened
            .iter()
            .filter_map(|(pid, store)| {
                std::fs::read(store.file_path())
                    .ok()
                    .map(|bytes| (pid.clone(), bytes))
            })
            .collect()
    }

    // MARK: - Internals

    /// Run `f` against a `&mut PerProjectStore`, opening the store lazily
    /// if we haven't seen it before. The store stays cached in `opened`
    /// across calls so we don't re-decrypt the file every save.
    fn with_store<R>(
        &self,
        project_id: &str,
        key: &[u8; cryptobox::KEY_BYTES],
        path: &Path,
        f: impl FnOnce(&mut PerProjectStore) -> Result<R>,
    ) -> Result<R> {
        let mut opened = self.opened.lock().unwrap();
        if !opened.contains_key(project_id) {
            let store = PerProjectStore::open(path, project_id, key)?;
            opened.insert(project_id.to_string(), store);
        }
        let store = opened
            .get_mut(project_id)
            .expect("just inserted if absent");
        f(store)
    }

    fn cache_store(&self, project_id: &str, store: PerProjectStore) {
        self.opened.lock().unwrap().insert(project_id.to_string(), store);
    }
}

/// Result of a successful `promote_to_shared`.
#[derive(Debug, Clone)]
pub struct PromoteResult {
    pub project_id: String,
    pub share_link: String,
}

/// Result of a successful `accept_share_link`.
#[derive(Debug, Clone)]
pub struct AcceptResult {
    pub project_id: String,
    /// If the shared file is already on disk and decrypts cleanly, the
    /// authoritative project record from inside it — caller should
    /// upsert this into the main doc so the sidebar gets real
    /// name/accent/icon instead of a placeholder.
    pub project_metadata: Option<Json>,
}

// MARK: - Process-wide singleton

/// Process-wide manager, rebuilt whenever the sync folder changes. We
/// keep it around (rather than constructing on every call) so the
/// internal `opened` cache amortises CryptoBox.open() across saves.
static MANAGER: Lazy<Mutex<Option<Arc<SharedProjectManager>>>> =
    Lazy::new(|| Mutex::new(None));

/// Return the manager for the current sync folder. None when sync is
/// not configured — sharing requires a folder to land the encrypted
/// files in.
pub fn current_manager() -> Result<Option<Arc<SharedProjectManager>>> {
    let folder = match crate::config::sync_folder()? {
        Some(p) => p,
        None => {
            *MANAGER.lock().unwrap() = None;
            return Ok(None);
        }
    };
    let mut guard = MANAGER.lock().unwrap();
    let needs_rebuild = match guard.as_ref() {
        Some(mgr) => mgr.folder() != folder,
        None => true,
    };
    if needs_rebuild {
        *guard = Some(Arc::new(SharedProjectManager::new(folder)));
    }
    Ok(guard.clone())
}

// MARK: - Tauri commands

/// Promote an existing project (currently living in the main doc) to a
/// shared encrypted file. Returns the share link. The Linux side then
/// removes the project's tasks from the main doc (they live in the
/// shared file now) and flips `isShared` on the project record.
pub async fn share_promote(
    sink: &dyn EventSink,
    project_id: String,
) -> Result<String, String> {
    let manager = current_manager()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "set a sync folder first — shared files live alongside tasks.automerge".to_string())?;

    let local_path = match crate::doc::default_doc_path() {
        Ok(p) => p,
        Err(e) => return Err(e.to_string()),
    };
    let mut doc = TaskDoc::load(&local_path).map_err(|e| e.to_string())?;
    let snapshot = doc.to_json();

    let project_record = snapshot
        .get("projects")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.iter().find(|p| p.get("id").and_then(|v| v.as_str()) == Some(project_id.as_str())))
        .cloned()
        .ok_or_else(|| format!("no project with id {project_id} in main doc"))?;

    let tasks_in_project: Vec<Json> = snapshot
        .get("tasks")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|t| t.get("list").and_then(|v| v.as_str()) == Some(project_id.as_str()))
        .collect();

    let result = manager
        .promote_to_shared(&project_record, &tasks_in_project)
        .map_err(|e| e.to_string())?;

    // Update the main doc: flag isShared on the project and tombstone
    // every task that lived in this project (they're in the encrypted
    // file now). Real deletes — not absent-from-snapshot — so the
    // tombstone propagates through any subsequent merges.
    let mut shared_project = project_record.clone();
    if let Some(obj) = shared_project.as_object_mut() {
        obj.insert("isShared".to_string(), json!(true));
    }
    let main_snapshot = json!({
        "projects": [shared_project],
        "tasks": [],
        "contexts": snapshot.get("contexts").cloned().unwrap_or(json!([])),
    });
    doc.apply_json(&main_snapshot).map_err(|e| e.to_string())?;
    for task in tasks_in_project {
        if let Some(tid) = task.get("id").and_then(|v| v.as_str()) {
            let _ = doc.delete("tasks", tid);
        }
    }
    doc.save(&local_path).map_err(|e| e.to_string())?;
    // Mirror to sync folder so peers see the isShared flag.
    if let Some(sync_path) = crate::config::sync_doc_path().map_err(|e| e.to_string())? {
        let _ = doc.save_overwrite(&sync_path);
    }
    sink.tasks_changed(&doc.to_json());

    Ok(result.share_link)
}

/// Accept a `todarchy://share/...` URL. Stores the key locally and, if
/// the encrypted file is already on disk (e.g. Dropbox pre-synced it),
/// publishes the project metadata into the main doc so the sidebar
/// shows real name/icon rather than a placeholder.
pub async fn share_accept(sink: &dyn EventSink, url: String) -> Result<String, String> {
    let manager = current_manager()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "set a sync folder first — shared files live there".to_string())?;

    let result = manager.accept_share_link(&url).map_err(|e| e.to_string())?;

    // Publish a project stub (or the authoritative copy if the file is
    // already present) into the main doc. Existing entries are upsert-
    // updated; new ones get inserted.
    let local_path = crate::doc::default_doc_path().map_err(|e| e.to_string())?;
    let mut doc = TaskDoc::load(&local_path).map_err(|e| e.to_string())?;

    let project_record = result.project_metadata.unwrap_or_else(|| {
        json!({
            "id": result.project_id,
            "name": "shared project",
            "icon": "users",
            "accent": "#7aa2f7",
            "isShared": true,
        })
    });
    let snapshot = json!({
        "projects": [project_record],
        "tasks": [],
        "contexts": [],
    });
    doc.apply_json(&snapshot).map_err(|e| e.to_string())?;
    doc.save(&local_path).map_err(|e| e.to_string())?;
    if let Some(sync_path) = crate::config::sync_doc_path().map_err(|e| e.to_string())? {
        let _ = doc.save_overwrite(&sync_path);
    }
    sink.tasks_changed(&doc.to_json());

    Ok(result.project_id)
}

/// Forget a shared project locally — delete the key + encrypted file
/// from this device. Peers still have their copies; this is "leave the
/// share," not "delete the project everywhere."
pub async fn share_leave(sink: &dyn EventSink, project_id: String) -> Result<(), String> {
    let manager = current_manager()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "no sync folder configured".to_string())?;
    manager.forget_locally(&project_id).map_err(|e| e.to_string())?;
    sink.tasks_changed(&serde_json::Value::Null);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn sample_project(id: &str, name: &str) -> Json {
        json!({ "id": id, "name": name, "icon": "folder", "accent": "#7aa2f7" })
    }

    fn sample_task(id: &str, list: &str, title: &str) -> Json {
        json!({ "id": id, "list": list, "title": title, "created": 1_i64 })
    }

    /// PerProjectStore needs `&mut` access; the manager's `Mutex` hides
    /// that — these helpers borrow through the guard for tests.
    fn save_through_manager(mgr: &SharedProjectManager, snapshot: &Json) -> Json {
        mgr.save_split(snapshot).unwrap()
    }

    #[test]
    fn promote_creates_encrypted_file_and_returns_share_link() {
        let dir = TempDir::new().unwrap();
        let mgr = SharedProjectManager::new_in_memory(dir.path());
        let project = sample_project("p_share", "team");
        let tasks = vec![sample_task("t1", "p_share", "first")];

        let result = mgr.promote_to_shared(&project, &tasks).unwrap();
        assert_eq!(result.project_id, "p_share");
        assert!(result.share_link.starts_with("todarchy://share/p_share#k="));

        let path = mgr.file_path("p_share");
        assert!(path.exists(), "encrypted file should be on disk");

        // Verify the bytes are a CryptoBox envelope (don't try to
        // decrypt here — the manager owns the key).
        let bytes = std::fs::read(&path).unwrap();
        assert!(cryptobox::is_envelope(&bytes));

        // Re-promoting the same project must fail (file + key already exist).
        assert!(mgr.promote_to_shared(&project, &tasks).is_err());
    }

    #[test]
    fn accept_share_link_round_trips_a_promote() {
        // Device A promotes, produces a link. Device B accepts the same
        // link against a fresh manager (different folder, different
        // key store) and ends up with the key registered. Once the
        // file arrives in B's folder via the "sync transport" (a
        // file copy in this test), accept_share_link can read the
        // authoritative project metadata.
        let dir_a = TempDir::new().unwrap();
        let dir_b = TempDir::new().unwrap();

        let mgr_a = SharedProjectManager::new_in_memory(dir_a.path());
        let project = sample_project("p_link", "design");
        let tasks = vec![sample_task("t1", "p_link", "draft")];
        let promote = mgr_a.promote_to_shared(&project, &tasks).unwrap();

        // Simulate the sync transport copying the encrypted file into B's folder.
        let src = mgr_a.file_path("p_link");
        let dst = dir_b.path().join(per_project::filename_for("p_link"));
        std::fs::copy(&src, &dst).unwrap();

        let mgr_b = SharedProjectManager::new_in_memory(dir_b.path());
        let accept = mgr_b.accept_share_link(&promote.share_link).unwrap();
        assert_eq!(accept.project_id, "p_link");
        // The file's already on disk, so we got the authoritative metadata back.
        let meta = accept.project_metadata.expect("metadata returned when file is present");
        assert_eq!(meta["id"], "p_link");
        assert_eq!(meta["name"], "design");
        assert_eq!(meta["isShared"], true);
        assert!(mgr_b.has_key("p_link"));
    }

    #[test]
    fn accept_link_is_idempotent_when_file_absent() {
        let dir = TempDir::new().unwrap();
        let mgr = SharedProjectManager::new_in_memory(dir.path());
        // Build a fake link with a key we control. Use the encode
        // helper directly so we don't depend on promote.
        let key = [9u8; cryptobox::KEY_BYTES];
        let url = sharelink::encode("p_join", &key);

        let first = mgr.accept_share_link(&url).unwrap();
        assert_eq!(first.project_id, "p_join");
        assert!(first.project_metadata.is_none(), "no file yet, no metadata");
        // Key is now stored — re-accepting must remain a no-op success.
        assert!(mgr.has_key("p_join"));
        let second = mgr.accept_share_link(&url).unwrap();
        assert_eq!(second.project_id, "p_join");
    }

    #[test]
    fn save_split_writes_shared_tasks_to_encrypted_file() {
        let dir = TempDir::new().unwrap();
        let mgr = SharedProjectManager::new_in_memory(dir.path());
        // Seed the shared project so the key is registered.
        let project = sample_project("p_split", "team");
        mgr.promote_to_shared(&project, &[]).unwrap();

        let snapshot = json!({
            "tasks": [
                { "id": "p1", "list": "p_split", "title": "shared-1", "created": 1_i64 },
                { "id": "p2", "list": "p_split", "title": "shared-2", "created": 2_i64 },
                { "id": "i1", "list": "inbox", "title": "private-1", "created": 3_i64 }
            ],
            "projects": [
                { "id": "p_split", "name": "team", "icon": "folder", "accent": "#7aa2f7", "isShared": true }
            ],
            "contexts": []
        });

        let main_snapshot = save_through_manager(&mgr, &snapshot);
        // Main snapshot retains only the inbox task; shared tasks are
        // routed to the encrypted file.
        let main_tasks = main_snapshot["tasks"].as_array().unwrap();
        assert_eq!(main_tasks.len(), 1);
        assert_eq!(main_tasks[0]["id"], "i1");
        // Project records stay in the main snapshot so peers without
        // the key still see the project in their sidebar.
        let main_projects = main_snapshot["projects"].as_array().unwrap();
        assert_eq!(main_projects.len(), 1);
        assert_eq!(main_projects[0]["id"], "p_split");

        // Inspect the encrypted file by opening a fresh PerProjectStore.
        let key = mgr.key_store.load("p_split").unwrap().unwrap();
        let store = PerProjectStore::open(&mgr.file_path("p_split"), "p_split", &key).unwrap();
        let snap = store.snapshot();
        let shared_tasks = snap["tasks"].as_array().unwrap();
        assert_eq!(shared_tasks.len(), 2);
        let titles: Vec<_> = shared_tasks.iter().map(|t| t["title"].as_str().unwrap()).collect();
        assert!(titles.contains(&"shared-1"));
        assert!(titles.contains(&"shared-2"));
    }

    #[test]
    fn load_union_overlays_shared_tasks_and_metadata() {
        let dir = TempDir::new().unwrap();
        let mgr = SharedProjectManager::new_in_memory(dir.path());

        // Promote a project with the authoritative name "the shared name".
        let project = sample_project("p_union", "the shared name");
        mgr.promote_to_shared(&project, &[sample_task("st1", "p_union", "via-shared")])
            .unwrap();

        // Build a main doc that has the *stub* version: the project
        // exists with `isShared=true` but a placeholder name, and one
        // unrelated inbox task.
        let mut main = TaskDoc::new().unwrap();
        main.apply_json(&json!({
            "tasks": [{ "id": "i1", "list": "inbox", "title": "private", "created": 1_i64 }],
            "projects": [{ "id": "p_union", "name": "placeholder", "icon": "folder",
                            "accent": "#000000", "isShared": true }],
            "contexts": []
        })).unwrap();

        let union = mgr.load_union(&main).unwrap();
        // Tasks: inbox kept, shared overlay appended.
        let tasks = union["tasks"].as_array().unwrap();
        let titles: Vec<_> = tasks.iter().map(|t| t["title"].as_str().unwrap()).collect();
        assert!(titles.contains(&"private"));
        assert!(titles.contains(&"via-shared"));
        // Project metadata: authoritative shared-file copy wins.
        let projects = union["projects"].as_array().unwrap();
        let p = projects.iter().find(|p| p["id"] == "p_union").unwrap();
        assert_eq!(p["name"], "the shared name");
        assert_eq!(p["isShared"], true);
    }

    #[test]
    fn forget_locally_removes_key_and_file_but_leaves_main_doc_alone() {
        let dir = TempDir::new().unwrap();
        let mgr = SharedProjectManager::new_in_memory(dir.path());
        mgr.promote_to_shared(&sample_project("p_bye", "x"), &[]).unwrap();
        assert!(mgr.has_key("p_bye"));
        assert!(mgr.file_path("p_bye").exists());

        mgr.forget_locally("p_bye").unwrap();
        assert!(!mgr.has_key("p_bye"));
        assert!(!mgr.file_path("p_bye").exists());
        // A second forget should be a clean no-op.
        mgr.forget_locally("p_bye").unwrap();
    }
}
