// per_project.rs — One encrypted shared-project file on disk.
//
// Mirrors iOS `PerProjectStore.swift`. Each instance owns one
// `shared_<project_id>.automerge.enc` file:
//
//   1. Read raw bytes from disk.
//   2. CryptoBox.open() → plaintext Automerge doc bytes.
//   3. Automerge::load() → in-memory doc with the shared schema.
//   4. Mutations go through `apply_json` like the main doc.
//   5. Saves seal the doc with the project's key and atomic-write the
//      envelope back.
//
// The on-disk format is byte-identical to what iOS writes, so an iPad
// editing a shared project produces a file Linux can decrypt + merge.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{json, Value as Json};

use crate::cryptobox;
use crate::doc::TaskDoc;

/// One encrypted shared-project file's lifecycle.
pub struct PerProjectStore {
    project_id: String,
    key: [u8; cryptobox::KEY_BYTES],
    file_path: PathBuf,
    doc: TaskDoc,
}

impl PerProjectStore {
    /// Open the file at `file_path`. If the file doesn't exist or the
    /// envelope won't decrypt, starts from a fresh empty doc — matching
    /// the iOS behavior where a missing/unreadable file is treated as
    /// "nothing joined yet" rather than an error. Callers that care
    /// about the distinction can probe with `Path::exists` first.
    pub fn open(
        file_path: &Path,
        project_id: &str,
        key: &[u8; cryptobox::KEY_BYTES],
    ) -> Result<Self> {
        let doc = if file_path.exists() {
            let bytes = std::fs::read(file_path)
                .with_context(|| format!("reading {}", file_path.display()))?;
            match cryptobox::open(&bytes, key) {
                Ok(plaintext) => {
                    TaskDoc::from_bytes(&plaintext).unwrap_or(TaskDoc::new()?)
                }
                Err(e) => {
                    tracing::warn!(
                        "decrypt failed for {}: {e}; starting fresh per-project doc",
                        file_path.display()
                    );
                    TaskDoc::new()?
                }
            }
        } else {
            TaskDoc::new()?
        };
        Ok(Self {
            project_id: project_id.to_string(),
            key: *key,
            file_path: file_path.to_path_buf(),
            doc,
        })
    }

    /// Read the current snapshot, filtered to this project's tasks +
    /// the project's own metadata. iOS does the same defense-in-depth
    /// filter — a shared file shouldn't ever contain anyone else's
    /// tasks, but we don't want one bug to leak them across projects.
    pub fn snapshot(&self) -> Json {
        let full = self.doc.to_json();
        let mut out = serde_json::Map::new();
        out.insert("version".into(), json!(1));
        out.insert("contexts".into(), full.get("contexts").cloned().unwrap_or(json!([])));

        let tasks: Vec<Json> = full
            .get("tasks")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|t| t.get("list").and_then(|v| v.as_str()) == Some(self.project_id.as_str()))
            .collect();
        let projects: Vec<Json> = full
            .get("projects")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|p| p.get("id").and_then(|v| v.as_str()) == Some(self.project_id.as_str()))
            .collect();
        out.insert("tasks".into(), Json::Array(tasks));
        out.insert("projects".into(), Json::Array(projects));
        Json::Object(out)
    }

    /// Upsert a snapshot into the doc and write the sealed envelope.
    pub fn save(&mut self, snapshot: &Json) -> Result<()> {
        self.doc.apply_json(snapshot)?;
        self.flush()
    }

    /// Explicit task deletion — produces a real Automerge tombstone so
    /// peers see the removal on next merge.
    pub fn delete_task(&mut self, task_id: &str) -> Result<()> {
        self.doc.delete("tasks", task_id)?;
        self.flush()
    }

    /// Merge another encrypted envelope (e.g. freshly arrived from
    /// Dropbox) into the live doc. Wrong-key / tampered envelopes
    /// return Ok(false) without modifying state, matching iOS's
    /// "leave the bytes alone" policy.
    pub fn merge_encrypted(&mut self, envelope: &[u8]) -> Result<bool> {
        let plaintext = match cryptobox::open(envelope, &self.key) {
            Ok(p) => p,
            Err(e) => {
                tracing::debug!(
                    "merge_encrypted: decrypt failed for project {}: {e}",
                    self.project_id
                );
                return Ok(false);
            }
        };
        let mut other = TaskDoc::from_bytes(&plaintext)?;
        self.doc.merge(&mut other)?;
        Ok(true)
    }

    /// Re-read the canonical file from disk and merge it in. Used after
    /// a file-watcher event fires for the encrypted file.
    pub fn refresh_from_disk(&mut self) -> Result<bool> {
        if !self.file_path.exists() {
            return Ok(false);
        }
        let bytes = std::fs::read(&self.file_path)
            .with_context(|| format!("reading {}", self.file_path.display()))?;
        self.merge_encrypted(&bytes)
    }

    /// Scan the sync folder for conflict copies the sync daemon
    /// produced (Dropbox / iCloud / Syncthing all have their own
    /// patterns) and merge any we can decrypt. Returns the count
    /// absorbed. Files we can't decrypt are LEFT ALONE — they're
    /// either someone else's project's file, junk, or a corrupted
    /// copy that we don't want to silently destroy.
    pub fn ingest_conflict_copies(&mut self) -> Result<usize> {
        let folder = match self.file_path.parent() {
            Some(p) => p.to_path_buf(),
            None => return Ok(0),
        };
        let canonical_name = self
            .file_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        let stem = format!("shared_{}", self.project_id);
        let suffix = ".automerge.enc";

        let mut absorbed = 0usize;
        let mut to_delete: Vec<PathBuf> = Vec::new();

        let entries = match std::fs::read_dir(&folder) {
            Ok(e) => e,
            Err(_) => return Ok(0),
        };
        for entry in entries.flatten() {
            let name = match entry.file_name().to_str().map(|s| s.to_string()) {
                Some(s) => s,
                None => continue,
            };
            if name == canonical_name {
                continue;
            }
            if !name.starts_with(&stem) || !name.ends_with(suffix) {
                continue;
            }
            // The character right after the id prefix must NOT be
            // alphanumeric or '_' — otherwise a project with id
            // `p_abc_extra` would get absorbed into the store for
            // `p_abc`. iOS pins this rule too; the matching test cases
            // are in LINUX_SHARING_PROMPT.md §Testing.
            let middle = &name[stem.len()..name.len() - suffix.len()];
            let first = match middle.chars().next() {
                Some(c) => c,
                None => continue, // shouldn't happen given canonical-name guard
            };
            if first.is_alphanumeric() || first == '_' {
                continue;
            }

            let path = entry.path();
            let bytes = match std::fs::read(&path) {
                Ok(b) => b,
                Err(_) => continue,
            };
            match self.merge_encrypted(&bytes) {
                Ok(true) => {
                    absorbed += 1;
                    to_delete.push(path);
                }
                Ok(false) | Err(_) => {
                    // Either we don't hold this file's key (wrong key),
                    // it's tampered, or the bytes aren't an envelope at
                    // all. Leave on disk for the user to inspect.
                }
            }
        }

        for path in to_delete {
            let _ = std::fs::remove_file(path);
        }
        Ok(absorbed)
    }

    fn flush(&mut self) -> Result<()> {
        let plaintext = self.doc.to_bytes();
        let envelope = cryptobox::seal(&plaintext, &self.key);
        write_atomic(&self.file_path, &envelope)
    }

    pub fn file_path(&self) -> &Path { &self.file_path }
    pub fn project_id(&self) -> &str { &self.project_id }
}

/// Canonical filename for a project's shared file.
pub fn filename_for(project_id: &str) -> String {
    format!("shared_{project_id}.automerge.enc")
}

/// Extract the project id from a shared-file name, or None if the name
/// doesn't match our convention. Used by directory scans.
pub fn project_id_from_filename(name: &str) -> Option<String> {
    let stem = "shared_";
    let suffix = ".automerge.enc";
    if !name.starts_with(stem) || !name.ends_with(suffix) {
        return None;
    }
    let id = &name[stem.len()..name.len() - suffix.len()];
    if id.is_empty() { None } else { Some(id.to_string()) }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let tmp = path.with_extension("enc.tmp");
    std::fs::write(&tmp, bytes)
        .with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path)
        .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_key() -> [u8; cryptobox::KEY_BYTES] {
        let mut k = [0u8; cryptobox::KEY_BYTES];
        for (i, b) in k.iter_mut().enumerate() {
            *b = (i as u8).wrapping_mul(7);
        }
        k
    }

    fn fresh_store(dir: &TempDir, project_id: &str) -> PerProjectStore {
        let path = dir.path().join(filename_for(project_id));
        PerProjectStore::open(&path, project_id, &test_key()).unwrap()
    }

    fn seed_with_one_task(store: &mut PerProjectStore, project_id: &str, title: &str) {
        let snapshot = json!({
            "tasks": [
                { "id": "t1", "list": project_id, "title": title, "created": 1_i64 }
            ],
            "projects": [
                { "id": project_id, "name": "shared", "icon": "folder",
                  "accent": "#7aa2f7", "isShared": true }
            ],
            "contexts": []
        });
        store.save(&snapshot).unwrap();
    }

    #[test]
    fn save_then_reopen_recovers_state() {
        let dir = TempDir::new().unwrap();
        let project_id = "p_round";
        {
            let mut s = fresh_store(&dir, project_id);
            seed_with_one_task(&mut s, project_id, "buy milk");
        }
        // Reopen — should see the prior state after decrypting from disk.
        let path = dir.path().join(filename_for(project_id));
        let store = PerProjectStore::open(&path, project_id, &test_key()).unwrap();
        let snap = store.snapshot();
        let tasks = snap["tasks"].as_array().unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0]["title"], "buy milk");
        let projects = snap["projects"].as_array().unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0]["isShared"], true);
    }

    #[test]
    fn wrong_key_yields_empty_doc_not_panic() {
        let dir = TempDir::new().unwrap();
        let project_id = "p_wrong";
        {
            let mut s = fresh_store(&dir, project_id);
            seed_with_one_task(&mut s, project_id, "secret");
        }
        let path = dir.path().join(filename_for(project_id));
        let mut wrong_key = test_key();
        wrong_key[0] ^= 0xff;
        // Opens cleanly with the wrong key but the doc is empty —
        // matches iOS's "treat as not joined yet" semantics.
        let store = PerProjectStore::open(&path, project_id, &wrong_key).unwrap();
        let snap = store.snapshot();
        assert_eq!(snap["tasks"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn snapshot_filters_out_foreign_project_data() {
        // If anything ever writes another project's tasks into a shared
        // file (test fixture, future bug, malicious peer), the snapshot
        // path filters them out so they don't bleed into the
        // shared-project UI on this device.
        let dir = TempDir::new().unwrap();
        let project_id = "p_filter";
        let mut store = fresh_store(&dir, project_id);
        let snapshot = json!({
            "tasks": [
                { "id": "t1", "list": project_id, "title": "mine", "created": 1_i64 },
                { "id": "t2", "list": "p_other", "title": "stranger", "created": 2_i64 }
            ],
            "projects": [
                { "id": project_id, "name": "mine", "icon": "folder", "accent": "#7aa2f7", "isShared": true },
                { "id": "p_other", "name": "stranger", "icon": "folder", "accent": "#000000" }
            ],
            "contexts": []
        });
        store.save(&snapshot).unwrap();

        let snap = store.snapshot();
        let tasks = snap["tasks"].as_array().unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0]["id"], "t1");
        let projects = snap["projects"].as_array().unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0]["id"], project_id);
    }

    #[test]
    fn merge_absorbs_peer_encrypted_envelope() {
        let dir = TempDir::new().unwrap();
        let project_id = "p_merge";
        let path = dir.path().join(filename_for(project_id));

        // Device A creates and seeds.
        let mut a = PerProjectStore::open(&path, project_id, &test_key()).unwrap();
        seed_with_one_task(&mut a, project_id, "from-a");

        // Device B opens its own in-memory store at a different temp
        // path, seeds a different task, and produces an encrypted
        // envelope as if it had just synced.
        let dir_b = TempDir::new().unwrap();
        let path_b = dir_b.path().join(filename_for(project_id));
        let mut b = PerProjectStore::open(&path_b, project_id, &test_key()).unwrap();
        b.save(&json!({
            "tasks": [{ "id": "t2", "list": project_id, "title": "from-b", "created": 2_i64 }],
            "projects": [{ "id": project_id, "name": "shared", "icon": "folder",
                            "accent": "#7aa2f7", "isShared": true }],
            "contexts": []
        })).unwrap();
        let envelope_b = std::fs::read(&path_b).unwrap();

        // Merging B's envelope into A surfaces both tasks.
        assert!(a.merge_encrypted(&envelope_b).unwrap());
        let snap = a.snapshot();
        let tasks = snap["tasks"].as_array().unwrap();
        let titles: Vec<_> = tasks.iter().map(|t| t["title"].as_str().unwrap()).collect();
        assert!(titles.contains(&"from-a"));
        assert!(titles.contains(&"from-b"));
    }

    #[test]
    fn merge_returns_false_on_decrypt_failure_without_panic() {
        let dir = TempDir::new().unwrap();
        let project_id = "p_bad";
        let mut store = fresh_store(&dir, project_id);
        let bogus = vec![0xde, 0xad, 0xbe, 0xef, 0x99];
        assert_eq!(store.merge_encrypted(&bogus).unwrap(), false);
    }

    #[test]
    fn ingest_absorbs_dropbox_style_conflict_file() {
        let dir = TempDir::new().unwrap();
        let project_id = "p_drop";

        // Device A's canonical file.
        let mut a = fresh_store(&dir, project_id);
        seed_with_one_task(&mut a, project_id, "a-only");

        // Build a peer envelope and drop it next to the canonical with
        // a Dropbox conflict-copy filename.
        let dir_b = TempDir::new().unwrap();
        let path_b = dir_b.path().join(filename_for(project_id));
        let mut b = PerProjectStore::open(&path_b, project_id, &test_key()).unwrap();
        b.save(&json!({
            "tasks": [{ "id": "t-b", "list": project_id, "title": "via-dropbox", "created": 99_i64 }],
            "projects": [{ "id": project_id, "name": "shared", "icon": "folder",
                            "accent": "#7aa2f7", "isShared": true }],
            "contexts": []
        })).unwrap();
        let envelope = std::fs::read(&path_b).unwrap();
        let conflict_name = format!(
            "shared_{project_id} (PEER's conflicted copy 2026-04-20).automerge.enc"
        );
        let conflict_path = dir.path().join(&conflict_name);
        std::fs::write(&conflict_path, &envelope).unwrap();

        let absorbed = a.ingest_conflict_copies().unwrap();
        assert_eq!(absorbed, 1);
        assert!(!conflict_path.exists(), "absorbed conflict file is removed");

        let titles: Vec<_> = a
            .snapshot()["tasks"]
            .as_array().unwrap().iter()
            .map(|t| t["title"].as_str().unwrap().to_string())
            .collect();
        assert!(titles.contains(&"a-only".to_string()));
        assert!(titles.contains(&"via-dropbox".to_string()));
    }

    #[test]
    fn ingest_absorbs_icloud_and_syncthing_patterns() {
        let dir = TempDir::new().unwrap();
        let project_id = "p_multi";
        let mut a = fresh_store(&dir, project_id);
        seed_with_one_task(&mut a, project_id, "base");

        // Two more peers, two different conflict patterns.
        let mk_envelope = |task_id: &str, title: &str| -> Vec<u8> {
            let d = TempDir::new().unwrap();
            let p = d.path().join(filename_for(project_id));
            let mut s = PerProjectStore::open(&p, project_id, &test_key()).unwrap();
            s.save(&json!({
                "tasks": [{ "id": task_id, "list": project_id, "title": title, "created": 1_i64 }],
                "projects": [{ "id": project_id, "name": "shared", "icon": "folder",
                                "accent": "#7aa2f7", "isShared": true }],
                "contexts": []
            })).unwrap();
            std::fs::read(&p).unwrap()
        };

        let icloud = format!("shared_{project_id} 2.automerge.enc");
        let syncthing = format!("shared_{project_id}.sync-conflict-20260420-123456-ABCDEF.automerge.enc");
        std::fs::write(dir.path().join(&icloud), mk_envelope("t-icloud", "from-icloud")).unwrap();
        std::fs::write(dir.path().join(&syncthing), mk_envelope("t-sync", "from-syncthing")).unwrap();

        let absorbed = a.ingest_conflict_copies().unwrap();
        assert_eq!(absorbed, 2);
        assert!(!dir.path().join(&icloud).exists());
        assert!(!dir.path().join(&syncthing).exists());

        let titles: Vec<_> = a.snapshot()["tasks"].as_array().unwrap()
            .iter().map(|t| t["title"].as_str().unwrap().to_string()).collect();
        assert!(titles.contains(&"from-icloud".to_string()));
        assert!(titles.contains(&"from-syncthing".to_string()));
    }

    #[test]
    fn ingest_does_not_pollute_when_id_is_a_prefix_of_another() {
        // `shared_p_abc_extra.automerge.enc` must NOT be absorbed into
        // the store for `p_abc` — the next-char rule from
        // LINUX_SHARING_PROMPT.md §F protects us.
        let dir = TempDir::new().unwrap();
        let target_id = "p_abc";
        let mut store = fresh_store(&dir, target_id);
        seed_with_one_task(&mut store, target_id, "base");

        // Build a file for the unrelated project `p_abc_extra` with the
        // same key (just to isolate the filename-matching rule from key
        // mismatch — in practice it'd be a different key too).
        let other_id = "p_abc_extra";
        let other_path = dir.path().join(filename_for(other_id));
        let mut other = PerProjectStore::open(&other_path, other_id, &test_key()).unwrap();
        other.save(&json!({
            "tasks": [{ "id": "stranger", "list": other_id, "title": "should-not-merge", "created": 1_i64 }],
            "projects": [{ "id": other_id, "name": "stranger", "icon": "folder",
                            "accent": "#000000", "isShared": true }],
            "contexts": []
        })).unwrap();

        let absorbed = store.ingest_conflict_copies().unwrap();
        assert_eq!(absorbed, 0, "prefix-collision file must NOT be absorbed");
        assert!(other_path.exists(), "other project's file untouched");

        let titles: Vec<_> = store.snapshot()["tasks"].as_array().unwrap()
            .iter().map(|t| t["title"].as_str().unwrap().to_string()).collect();
        assert!(!titles.contains(&"should-not-merge".to_string()));
    }

    #[test]
    fn ingest_leaves_undecryptable_files_alone() {
        // A conflict file we can't decrypt (wrong key, garbage, etc.)
        // must survive the sweep — never destroy bytes you can't
        // authenticate.
        let dir = TempDir::new().unwrap();
        let project_id = "p_keep";
        let mut store = fresh_store(&dir, project_id);
        seed_with_one_task(&mut store, project_id, "base");

        let foreign = dir.path().join(format!("shared_{project_id} 2.automerge.enc"));
        std::fs::write(&foreign, b"TDAR\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00garbage tag00000").unwrap();

        let absorbed = store.ingest_conflict_copies().unwrap();
        assert_eq!(absorbed, 0);
        assert!(foreign.exists(), "undecryptable conflict file is left on disk");
    }

    #[test]
    fn filename_helpers_round_trip() {
        let id = "p_abc12345";
        let name = filename_for(id);
        assert_eq!(name, "shared_p_abc12345.automerge.enc");
        assert_eq!(project_id_from_filename(&name), Some(id.to_string()));
        assert_eq!(project_id_from_filename("tasks.automerge"), None);
        assert_eq!(project_id_from_filename("shared_.automerge.enc"), None);
    }
}
