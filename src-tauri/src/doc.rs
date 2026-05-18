// doc.rs — Automerge-backed task document.
//
// v0.2 sync rides on a user-picked, file-synced folder (iCloud / Dropbox /
// Syncthing). Each device keeps an Automerge binary doc (tasks.automerge)
// as the canonical store. When two devices edit the doc while offline and
// then reconnect, Automerge merges the two histories without losing edits.
//
// Schema inside the doc (all three apps — Linux, macOS, iOS — MUST agree):
//   version    : int = 1
//   tasks      : Map<id, Task>      — keyed by task.id
//   projects   : Map<id, Project>   — keyed by project.id
//   contexts   : List<String>
//
// The Map shape is the load-bearing CRDT choice. If tasks were a List,
// concurrent appends on two devices would both land at index N and
// Automerge would pick one as "the winner" — dropping the other device's
// task. Keyed by id, each insert is at a different key, so both survive.
// Ordering for the UI comes from each task's `pos` field (defaults to
// `created`), which is sorted client-side when we project to JSON.
//
// The React frontend, tod CLI, and todarchy-waybar all consume the JSON
// array shape they always have. `to_json()` flattens the Map to an array
// and we fan out tasks.json on every save for CLI/Waybar compatibility.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use automerge::{transaction::Transactable, ActorId, Automerge, ObjType, ReadDoc, ScalarValue, Value, ROOT};
use once_cell::sync::Lazy;
use serde_json::{json, Map, Value as Json};

/// Canonical seed actor id, byte-identical to the iOS app's `seedActor`
/// (UUID `544F4441-5243-4859-5345-454400000001`). Every device's fresh
/// doc starts from these same 16 bytes as the actor, so the operations
/// that create the root `tasks` / `projects` / `contexts` Maps produce
/// the same Automerge ObjIds across platforms. Without this, two
/// independently-created docs merge by tombstoning one side's root
/// objects — silently dropping every task in them.
const SEED_ACTOR_BYTES: [u8; 16] = [
    0x54, 0x4F, 0x44, 0x41, 0x52, 0x43, 0x48, 0x59,
    0x53, 0x45, 0x45, 0x44, 0x00, 0x00, 0x00, 0x01,
];

/// Computed once per process. iOS does the same trick: build an empty
/// doc using the seed actor, run the shape-bootstrap ops, save the
/// resulting bytes. New devices load these bytes and `fork()` for their
/// own actor — the seed history is shared, so future merges align.
static SEED_BYTES: Lazy<Vec<u8>> = Lazy::new(|| {
    let mut doc = Automerge::new().with_actor(ActorId::from(SEED_ACTOR_BYTES.to_vec()));
    let mut tx = doc.transaction();
    tx.put(ROOT, "version", 1_i64).expect("seed version");
    tx.put_object(ROOT, "tasks", ObjType::Map).expect("seed tasks");
    tx.put_object(ROOT, "projects", ObjType::Map).expect("seed projects");
    let ctx = tx.put_object(ROOT, "contexts", ObjType::List).expect("seed contexts");
    for (i, c) in ["@home", "@work", "@errands", "@mac", "@phone", "@read"]
        .iter()
        .enumerate()
    {
        tx.insert(&ctx, i, *c).expect("seed context entry");
    }
    tx.commit();
    doc.save()
});

/// Wrapper around an `Automerge` doc with task-store helpers.
pub struct TaskDoc {
    doc: Automerge,
}

impl TaskDoc {
    /// Create a fresh empty doc that shares the canonical seed history
    /// (and therefore the same root Map ObjIds) with every other
    /// todarchy install. Fresh from this seed, then `fork()` so this
    /// device gets a unique actor for its own writes.
    pub fn new() -> Result<Self> {
        let doc = Automerge::load(&SEED_BYTES)
            .context("loading canonical seed bytes")?
            .fork();
        Ok(Self { doc })
    }

    /// Load a doc from its binary file. Returns a fresh empty doc if the
    /// file is missing, zero-length, or unparseable (the latter logs a
    /// warning so we notice corruption).
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Self::new();
        }
        let bytes = std::fs::read(path)
            .with_context(|| format!("reading {}", path.display()))?;
        if bytes.is_empty() {
            return Self::new();
        }
        match Automerge::load(&bytes) {
            Ok(doc) => Ok(Self { doc }),
            Err(e) => {
                tracing::warn!(
                    "failed to parse automerge doc at {} ({e}); starting fresh",
                    path.display()
                );
                Self::new()
            }
        }
    }

    /// Load a doc from raw bytes. Used by tests; the runtime path goes
    /// through `load()` which reads a file.
    #[allow(dead_code)]
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let doc = Automerge::load(bytes)?;
        Ok(Self { doc })
    }

    /// Return the binary wire format. Used by tests.
    #[allow(dead_code)]
    pub fn to_bytes(&mut self) -> Vec<u8> {
        self.doc.save()
    }

    /// Atomic write: serialize to `<path>.tmp`, then rename. Used for the
    /// private local store where we want crash-safety; a power-off
    /// mid-write leaves either the old doc or the new one intact.
    pub fn save(&mut self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let bytes = self.doc.save();
        let tmp = path.with_extension("automerge.tmp");
        std::fs::write(&tmp, &bytes)
            .with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, path)
            .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))?;
        tracing::info!(
            "wrote automerge doc (atomic): path={} bytes={}",
            path.display(),
            bytes.len()
        );
        Ok(())
    }

    /// Direct overwrite, no sibling `.tmp` + rename. Used for the sync-
    /// folder copy because some sync daemons (the Linux Dropbox client
    /// in particular) handle the rename IN event poorly: they upload
    /// the `.tmp` file and then don't re-upload the rename target,
    /// leaving peers looking at stale content. `std::fs::write` opens
    /// with TRUNC so there's a tiny window where a reader sees an
    /// empty or partial file — in practice Automerge parse just fails
    /// and the reading device retries on the next notify event.
    pub fn save_overwrite(&mut self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let bytes = self.doc.save();
        std::fs::write(path, &bytes)
            .with_context(|| format!("writing {}", path.display()))?;
        tracing::info!(
            "wrote automerge doc (direct): path={} bytes={}",
            path.display(),
            bytes.len()
        );
        Ok(())
    }

    /// Merge another device's doc into this one. Automerge resolves
    /// concurrent operations deterministically: additions to different keys
    /// both survive, deletions tombstone, same-field edits resolve with a
    /// stable tiebreaker.
    pub fn merge(&mut self, other: &mut TaskDoc) -> Result<()> {
        self.doc.merge(&mut other.doc)?;
        Ok(())
    }

    /// Automerge change-head identifiers. Equal before/after a merge means
    /// nothing new was pulled in — used by the sync watcher to avoid
    /// echoing our own writes back to the frontend.
    pub fn heads(&self) -> Vec<automerge::ChangeHash> {
        self.doc.get_heads()
    }

    /// Delete a task or project by id. This is a separate, explicit API
    /// (rather than "absence implies delete" via apply_json) so that
    /// another device's concurrent inserts between our last load and
    /// this save aren't silently wiped.
    pub fn delete(&mut self, root_key: &str, id: &str) -> Result<()> {
        delete_entry(&mut self.doc, root_key, id)
    }

    /// Flatten the doc to the JSON shape the rest of the app consumes.
    /// Tasks and projects come back as arrays (sorted by `pos` desc for
    /// tasks, insertion order for projects) even though internally they're
    /// Maps.
    pub fn to_json(&self) -> Json {
        let mut out = Map::new();
        out.insert("version".into(), json!(1));
        out.insert("tasks".into(), read_map_as_sorted_array(&self.doc, "tasks"));
        out.insert("projects".into(), read_map_as_array(&self.doc, "projects"));
        out.insert("contexts".into(), read_list(&self.doc, ROOT, "contexts"));
        Json::Object(out)
    }

    /// Apply a full-state JSON payload (as the React frontend sends) to the
    /// doc by diffing current contents against `data`. Upserts tasks and
    /// projects by id, deletes removed ones, replaces contexts wholesale.
    /// Every change is a targeted Automerge op so merging with another
    /// device's concurrent edits stays meaningful.
    pub fn apply_json(&mut self, data: &Json) -> Result<()> {
        let mut tx = self.doc.transaction();
        tx.put(ROOT, "version", 1_i64)?;
        apply_map(&mut tx, "tasks", data.get("tasks"), "id", TASK_CLEARABLE_FIELDS)?;
        apply_map(&mut tx, "projects", data.get("projects"), "id", PROJECT_CLEARABLE_FIELDS)?;
        apply_contexts(&mut tx, data.get("contexts"))?;
        tx.commit();
        Ok(())
    }
}

// Known scalar fields per object type that the Linux frontend is allowed
// to clear by omission. Absence of one of these in the incoming JSON for
// an existing task/project deletes the field from the doc (so a user
// "clear due date" round-trips correctly).
//
// Any field NOT in these lists is preserved as-is when missing from the
// snapshot — that's the whole point of upsert-only: iOS-written fields
// the Linux UI doesn't model yet (`comments`, `isShared`, `claudeAccess`,
// `isInbox`) must survive a Linux save unchanged, otherwise sync silently
// corrupts cross-platform data.
const TASK_CLEARABLE_FIELDS: &[&str] = &[
    "ctx", "due", "note", "doneAt", "deferUntil", "parent", "pos",
];
const PROJECT_CLEARABLE_FIELDS: &[&str] = &["icon", "accent"];

// ---------- Read: Automerge → JSON ----------

fn read_map_as_array(doc: &Automerge, key: &str) -> Json {
    // Tolerate both the canonical Map<id, Task> schema and the older
    // List<Task> schema. macOS/iOS builds that shipped before the schema
    // update wrote tasks as a List; we read them so the migration path
    // is "Linux opens the old doc → converts to Map on next save".
    match doc.get(ROOT, key) {
        Ok(Some((Value::Object(ObjType::Map), map))) => {
            let mut out = Vec::new();
            for k in doc.keys(&map) {
                if let Ok(Some((Value::Object(ObjType::Map), entry))) = doc.get(&map, &k) {
                    out.push(read_map_contents(doc, entry));
                }
            }
            Json::Array(out)
        }
        Ok(Some((Value::Object(ObjType::List), list))) => {
            let len = doc.length(&list);
            let mut out = Vec::with_capacity(len);
            for i in 0..len {
                if let Ok(Some((Value::Object(ObjType::Map), entry))) = doc.get(&list, i) {
                    out.push(read_map_contents(doc, entry));
                }
            }
            tracing::info!(
                "read {} entries from legacy List-schema `{}`; next save will migrate to Map",
                out.len(),
                key
            );
            Json::Array(out)
        }
        _ => Json::Array(Vec::new()),
    }
}

fn read_map_as_sorted_array(doc: &Automerge, key: &str) -> Json {
    let Json::Array(mut items) = read_map_as_array(doc, key) else {
        return Json::Array(Vec::new());
    };
    // Sort by `pos` (defaulting to `created`) descending — matches the
    // frontend's legacy "newest first" behavior.
    items.sort_by_key(|t| {
        let pos = t.get("pos").and_then(|v| v.as_i64());
        let created = t.get("created").and_then(|v| v.as_i64()).unwrap_or(0);
        -pos.unwrap_or(created)
    });
    Json::Array(items)
}

fn read_list(doc: &Automerge, obj: automerge::ObjId, key: &str) -> Json {
    let Ok(Some((Value::Object(ObjType::List), list))) = doc.get(obj, key) else {
        return Json::Array(Vec::new());
    };
    let len = doc.length(&list);
    let mut out = Vec::with_capacity(len);
    for i in 0..len {
        match doc.get(&list, i) {
            Ok(Some((Value::Object(ObjType::Map), id))) => out.push(read_map_contents(doc, id)),
            Ok(Some((Value::Scalar(s), _))) => out.push(scalar_to_json(&s)),
            _ => {}
        }
    }
    Json::Array(out)
}

fn read_map_contents(doc: &Automerge, obj: automerge::ObjId) -> Json {
    let mut out = Map::new();
    for key in doc.keys(&obj) {
        match doc.get(&obj, &key) {
            Ok(Some((Value::Scalar(s), _))) => {
                out.insert(key, scalar_to_json(&s));
            }
            Ok(Some((Value::Object(ObjType::List), inner))) => {
                let len = doc.length(&inner);
                let mut arr = Vec::with_capacity(len);
                for i in 0..len {
                    if let Ok(Some((Value::Scalar(s), _))) = doc.get(&inner, i) {
                        arr.push(scalar_to_json(&s));
                    }
                }
                out.insert(key, Json::Array(arr));
            }
            Ok(Some((Value::Object(ObjType::Map), inner))) => {
                // Nested Maps — e.g. a task's `comments` keyed by
                // commentId, as written by iOS. The Linux frontend
                // doesn't render these yet, but the projection must
                // include them so React's spread-then-save round-trips
                // them faithfully and apply_object_fields can re-upsert
                // into the existing Map.
                out.insert(key, read_map_contents(doc, inner));
            }
            _ => {}
        }
    }
    Json::Object(out)
}

fn scalar_to_json(s: &ScalarValue) -> Json {
    match s {
        ScalarValue::Str(s) => Json::String(s.to_string()),
        ScalarValue::Int(n) => json!(*n),
        ScalarValue::Uint(n) => json!(*n),
        ScalarValue::F64(n) => json!(*n),
        ScalarValue::Boolean(b) => json!(*b),
        ScalarValue::Null => Json::Null,
        ScalarValue::Timestamp(t) => json!(*t),
        ScalarValue::Counter(c) => json!(i64::from(c)),
        other => Json::String(format!("{other:?}")),
    }
}

// ---------- Write: JSON → Automerge (diff-based upserts) ----------

fn apply_map(
    tx: &mut automerge::transaction::Transaction<'_>,
    key: &str,
    value: Option<&Json>,
    id_field: &str,
    clearable: &[&str],
) -> Result<()> {
    // Upsert-only. Absence of an id in `incoming` does NOT delete the entry —
    // another device may have added it between the frontend's last load and
    // this save, and we'd otherwise tombstone their work. Intentional
    // deletions come through the `delete_tasks` / `delete_projects` Tauri
    // commands, which call `delete_entry` below with the specific ids.
    let map = match tx.get(ROOT, key)? {
        Some((Value::Object(ObjType::Map), id)) => id,
        _ => tx.put_object(ROOT, key, ObjType::Map)?,
    };

    let incoming = value.and_then(|v| v.as_array()).cloned().unwrap_or_default();
    for item in &incoming {
        let Some(item_id) = item.get(id_field).and_then(|v| v.as_str()) else {
            continue;
        };
        let entry = match tx.get(&map, item_id)? {
            Some((Value::Object(ObjType::Map), id)) => id,
            _ => tx.put_object(&map, item_id, ObjType::Map)?,
        };
        apply_object_fields(tx, entry, item, clearable)?;
    }
    Ok(())
}

/// Delete a specific entry from one of the root maps. Called from the
/// explicit `delete_tasks` / `delete_projects` Tauri commands so the user's
/// intent to delete is preserved across sync even if another device makes
/// concurrent edits.
pub fn delete_entry(doc: &mut Automerge, root_key: &str, id: &str) -> Result<()> {
    let mut tx = doc.transaction();
    if let Some((Value::Object(ObjType::Map), map)) = tx.get(ROOT, root_key)? {
        let _ = tx.delete(&map, id);
    }
    tx.commit();
    Ok(())
}

/// Apply incoming field values to an existing Automerge map entry.
///
/// - Known scalar fields (`clearable`) absent from the incoming object are
///   deleted from the doc. This lets the frontend clear, say, a due date
///   by spreading the task without the `due` key.
/// - Any other field — `comments` (iOS-only Map), `isShared`,
///   `claudeAccess`, `isInbox`, or anything a future client adds — is
///   left alone when missing from the snapshot, so Linux's save doesn't
///   silently wipe iOS-written content.
/// - Nested Map sub-objects reuse the existing Map's ObjId so concurrent
///   inserts from another device (e.g. iOS appending a comment) survive
///   merge. Without this, every Linux save would `put_object` a fresh
///   Map at the key, divorcing both devices' inserts.
fn apply_object_fields(
    tx: &mut automerge::transaction::Transaction<'_>,
    obj: automerge::ObjId,
    value: &Json,
    clearable: &[&str],
) -> Result<()> {
    let Json::Object(fields) = value else {
        return Ok(());
    };

    // Phase 1: explicit clears for known scalar fields.
    for known in clearable {
        if !fields.contains_key(*known) {
            let _ = tx.delete(&obj, *known);
        }
    }

    // Phase 2: upsert every incoming field.
    for (k, v) in fields {
        match v {
            // Treat explicit null as "delete this key". Matches the iOS
            // writer, which omits a key when its Swift field is nil
            // rather than storing a Null scalar.
            Json::Null => {
                let _ = tx.delete(&obj, k.as_str());
            }
            Json::Bool(b) => {
                tx.put(&obj, k.as_str(), *b)?;
            }
            Json::Number(n) => {
                if let Some(i) = n.as_i64() {
                    tx.put(&obj, k.as_str(), i)?;
                } else if let Some(f) = n.as_f64() {
                    tx.put(&obj, k.as_str(), f)?;
                }
            }
            Json::String(s) => {
                tx.put(&obj, k.as_str(), s.clone())?;
            }
            Json::Array(items) => {
                let list = tx.put_object(&obj, k.as_str(), ObjType::List)?;
                for (i, item) in items.iter().enumerate() {
                    if let Some(s) = item.as_str() {
                        tx.insert(&list, i, s.to_string())?;
                    }
                }
            }
            Json::Object(_) => {
                let child = match tx.get(&obj, k.as_str())? {
                    Some((Value::Object(ObjType::Map), id)) => id,
                    _ => tx.put_object(&obj, k.as_str(), ObjType::Map)?,
                };
                // Sub-Maps (e.g. `comments`) are upsert-only — pass an
                // empty `clearable` so absence of a comment in the
                // incoming Map doesn't delete it from the doc.
                apply_object_fields(tx, child, v, &[])?;
            }
        }
    }
    Ok(())
}

fn apply_contexts(
    tx: &mut automerge::transaction::Transaction<'_>,
    value: Option<&Json>,
) -> Result<()> {
    let items = value.and_then(|v| v.as_array()).cloned().unwrap_or_default();
    // Contexts are short and rarely edited — a wholesale replace is fine.
    let list = tx.put_object(ROOT, "contexts", ObjType::List)?;
    for (i, item) in items.iter().enumerate() {
        if let Some(s) = item.as_str() {
            tx.insert(&list, i, s.to_string())?;
        }
    }
    Ok(())
}

pub fn default_doc_path() -> Result<PathBuf> {
    let dir = dirs::data_local_dir()
        .or_else(dirs::home_dir)
        .context("no data dir")?
        .join("todarchy");
    std::fs::create_dir_all(&dir).ok();
    Ok(dir.join("tasks.automerge"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_json() -> Json {
        json!({
            "version": 1,
            "tasks": [
                { "id": "t1", "list": "inbox", "title": "buy milk",
                  "ctx": "@errands", "due": "today", "note": "",
                  "created": 1700000000000_i64, "parent": null },
                { "id": "t2", "list": "p_work", "title": "ship v0.2",
                  "ctx": "@work", "due": "this week", "note": "sync + mobile",
                  "created": 1700000100000_i64, "parent": null,
                  "doneAt": 1700000200000_i64 }
            ],
            "projects": [
                { "id": "p_work", "name": "work", "icon": "briefcase", "accent": "var(--accent)" }
            ],
            "contexts": ["@home", "@work", "@errands"]
        })
    }

    #[test]
    fn json_round_trip_preserves_fields() {
        let mut d = TaskDoc::new().unwrap();
        d.apply_json(&sample_json()).unwrap();
        let out = d.to_json();

        let tasks = out.get("tasks").and_then(|v| v.as_array()).unwrap();
        assert_eq!(tasks.len(), 2);
        // newest first (higher `created` → earlier in array)
        assert_eq!(tasks[0]["id"], "t2");
        assert_eq!(tasks[0]["title"], "ship v0.2");
        assert_eq!(tasks[0]["doneAt"], 1700000200000_i64);
        assert_eq!(tasks[1]["id"], "t1");
        assert_eq!(tasks[1]["ctx"], "@errands");
        assert_eq!(tasks[1]["parent"], Json::Null);

        let projects = out.get("projects").and_then(|v| v.as_array()).unwrap();
        assert_eq!(projects[0]["name"], "work");

        let contexts = out.get("contexts").and_then(|v| v.as_array()).unwrap();
        assert_eq!(contexts[0], "@home");
    }

    #[test]
    fn upsert_updates_existing_task_fields() {
        let mut d = TaskDoc::new().unwrap();
        d.apply_json(&sample_json()).unwrap();

        // Second apply with an edited title — should stay 2 tasks, t1 retitled.
        let mut edited = sample_json();
        edited["tasks"][0]["title"] = json!("buy oat milk");
        d.apply_json(&edited).unwrap();

        let out = d.to_json();
        let tasks = out.get("tasks").and_then(|v| v.as_array()).unwrap();
        assert_eq!(tasks.len(), 2);
        let t1 = tasks.iter().find(|t| t["id"] == "t1").unwrap();
        assert_eq!(t1["title"], "buy oat milk");
    }

    #[test]
    fn apply_json_does_not_delete_missing_tasks() {
        // Regression for the sync race: if the frontend's save payload is
        // missing a task (because another device added it between the
        // frontend's last load and this save), apply_json must NOT treat
        // that absence as a delete. Otherwise we'd tombstone the other
        // device's concurrent insert. Deletions happen through `delete()`.
        let mut d = TaskDoc::new().unwrap();
        d.apply_json(&sample_json()).unwrap();

        let mut reduced = sample_json();
        reduced["tasks"].as_array_mut().unwrap().pop(); // drop t2 from incoming
        d.apply_json(&reduced).unwrap();

        let out = d.to_json();
        let tasks = out.get("tasks").and_then(|v| v.as_array()).unwrap();
        assert_eq!(tasks.len(), 2, "upsert-only semantics — t2 must survive");
    }

    #[test]
    fn explicit_delete_removes_task() {
        let mut d = TaskDoc::new().unwrap();
        d.apply_json(&sample_json()).unwrap();
        d.delete("tasks", "t1").unwrap();

        let out = d.to_json();
        let tasks = out.get("tasks").and_then(|v| v.as_array()).unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0]["id"], "t2");
    }

    #[test]
    fn save_load_round_trip() {
        let tmp = std::env::temp_dir().join(format!(
            "todarchy-test-{}.automerge",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&tmp);

        let mut d = TaskDoc::new().unwrap();
        d.apply_json(&sample_json()).unwrap();
        d.save(&tmp).unwrap();

        let reopened = TaskDoc::load(&tmp).unwrap();
        let out = reopened.to_json();
        let tasks = out.get("tasks").and_then(|v| v.as_array()).unwrap();
        assert_eq!(tasks.len(), 2);
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn merge_combines_concurrent_inserts_via_map_keys() {
        // Start from a shared base, simulate two devices, each adds a task.
        let mut base = TaskDoc::new().unwrap();
        base.apply_json(&json!({
            "tasks": [{ "id": "t0", "list": "inbox", "title": "shared",
                         "created": 1_i64, "parent": null }],
            "projects": [], "contexts": []
        })).unwrap();
        let bytes = base.to_bytes();

        let mut device_a = TaskDoc::from_bytes(&bytes).unwrap();
        let mut device_b = TaskDoc::from_bytes(&bytes).unwrap();

        // Device A adds task a1.
        let mut state_a = device_a.to_json();
        state_a["tasks"].as_array_mut().unwrap().push(json!({
            "id": "a1", "list": "inbox", "title": "from a",
            "created": 2_i64, "parent": null
        }));
        device_a.apply_json(&state_a).unwrap();

        // Device B adds task b1.
        let mut state_b = device_b.to_json();
        state_b["tasks"].as_array_mut().unwrap().push(json!({
            "id": "b1", "list": "inbox", "title": "from b",
            "created": 3_i64, "parent": null
        }));
        device_b.apply_json(&state_b).unwrap();

        // Merge B into A — both inserts survive because they're at
        // different map keys (a1 vs b1).
        device_a.merge(&mut device_b).unwrap();
        let out = device_a.to_json();
        let tasks = out.get("tasks").and_then(|v| v.as_array()).unwrap();
        assert_eq!(tasks.len(), 3, "shared + a1 + b1");
        let titles: Vec<_> = tasks.iter().map(|t| t["title"].as_str().unwrap()).collect();
        assert!(titles.contains(&"from a"));
        assert!(titles.contains(&"from b"));
        assert!(titles.contains(&"shared"));
    }

    #[test]
    fn merge_preserves_concurrent_edits_to_different_fields() {
        // Shared task. Device A edits title, device B edits due. Merge should
        // keep both edits (different fields on the same map entry).
        let mut base = TaskDoc::new().unwrap();
        base.apply_json(&json!({
            "tasks": [{ "id": "shared", "list": "inbox", "title": "original",
                         "due": "", "created": 1_i64, "parent": null }],
            "projects": [], "contexts": []
        })).unwrap();
        let bytes = base.to_bytes();

        let mut a = TaskDoc::from_bytes(&bytes).unwrap();
        let mut b = TaskDoc::from_bytes(&bytes).unwrap();

        let mut sa = a.to_json();
        sa["tasks"][0]["title"] = json!("A's edit");
        a.apply_json(&sa).unwrap();

        let mut sb = b.to_json();
        sb["tasks"][0]["due"] = json!("today");
        b.apply_json(&sb).unwrap();

        a.merge(&mut b).unwrap();
        let out = a.to_json();
        let t = &out["tasks"][0];
        assert_eq!(t["title"], "A's edit");
        assert_eq!(t["due"], "today");
    }

    #[test]
    fn apply_json_preserves_ios_only_fields_when_absent_from_snapshot() {
        // The cross-platform corruption bug: Linux must not wipe fields it
        // doesn't model when those fields are missing from the incoming
        // snapshot. iOS writes `comments` (Map<id, Comment>) on tasks and
        // `isShared` / `claudeAccess` / `isInbox` flags on projects. Older
        // versions of `apply_object_fields` aggressively deleted any key not
        // in the payload — so a single Linux save silently destroyed iOS
        // data after sync.
        let mut d = TaskDoc::new().unwrap();
        d.apply_json(&json!({
            "tasks": [{
                "id": "t1", "list": "inbox", "title": "ios-task",
                "created": 1_i64,
                "comments": {
                    "c1": { "id": "c1", "author": "Mac", "text": "hi",
                            "createdAt": 100_i64 }
                }
            }],
            "projects": [{
                "id": "p1", "name": "shared-proj", "icon": "folder",
                "accent": "#7aa2f7", "isShared": true, "claudeAccess": true,
                "isInbox": false
            }],
            "contexts": []
        })).unwrap();

        // Linux re-saves with a snapshot that doesn't include the iOS-only
        // fields (simulating either a stale frontend snapshot or a manual
        // strip somewhere in the React pipeline).
        d.apply_json(&json!({
            "tasks": [{ "id": "t1", "list": "inbox", "title": "ios-task",
                          "created": 1_i64 }],
            "projects": [{ "id": "p1", "name": "shared-proj", "icon": "folder",
                            "accent": "#7aa2f7" }],
            "contexts": []
        })).unwrap();

        let out = d.to_json();
        let t = &out["tasks"].as_array().unwrap()[0];
        let comments = t.get("comments")
            .expect("comments map must survive a Linux save")
            .as_object()
            .expect("comments is a Map<id, Comment>");
        assert_eq!(comments.len(), 1);
        assert_eq!(comments["c1"]["text"], "hi");
        assert_eq!(comments["c1"]["author"], "Mac");

        let p = &out["projects"].as_array().unwrap()[0];
        assert_eq!(p.get("isShared"), Some(&json!(true)), "isShared bool preserved");
        assert_eq!(p.get("claudeAccess"), Some(&json!(true)), "claudeAccess preserved");
        // `isInbox: false` was in the payload as an explicit false; we'd
        // expect it round-trips faithfully.
        assert_eq!(p.get("isInbox"), Some(&json!(false)));
    }

    #[test]
    fn apply_json_still_clears_known_scalar_fields_by_omission() {
        // The flip side of the previous test: known scalar fields on tasks
        // (due, doneAt, deferUntil, parent, pos, note, ctx) MUST be
        // clearable by omission, because that's how the Linux React
        // frontend de-sets them — `{ ...t, doneAt: undefined }` spreads
        // then JSON.stringify drops the key.
        let mut d = TaskDoc::new().unwrap();
        d.apply_json(&json!({
            "tasks": [{ "id": "t1", "list": "inbox", "title": "x",
                          "due": "today", "deferUntil": 100_i64,
                          "parent": "p", "pos": 1_i64,
                          "created": 1_i64 }],
            "projects": [], "contexts": []
        })).unwrap();

        d.apply_json(&json!({
            "tasks": [{ "id": "t1", "list": "inbox", "title": "x",
                          "created": 1_i64 }],
            "projects": [], "contexts": []
        })).unwrap();

        let out = d.to_json();
        let t = &out["tasks"].as_array().unwrap()[0];
        assert!(t.get("due").and_then(|v| v.as_str()).is_none(), "due cleared");
        assert!(t.get("deferUntil").and_then(|v| v.as_i64()).is_none(),
                "deferUntil cleared");
        assert!(t.get("parent").and_then(|v| v.as_str()).is_none(), "parent cleared");
        assert!(t.get("pos").and_then(|v| v.as_i64()).is_none(), "pos cleared");
    }

    #[test]
    fn explicit_null_clears_field_like_ios_writer() {
        // React sometimes sends `parent: null` (e.g. when un-nesting a
        // task). Apply should treat null as an explicit delete so the
        // resulting bytes match what iOS writes when its `parent` field
        // is nil — iOS omits the key entirely.
        let mut d = TaskDoc::new().unwrap();
        d.apply_json(&json!({
            "tasks": [{ "id": "t1", "list": "inbox", "title": "x",
                          "parent": "p1", "created": 1_i64 }],
            "projects": [], "contexts": []
        })).unwrap();
        d.apply_json(&json!({
            "tasks": [{ "id": "t1", "list": "inbox", "title": "x",
                          "parent": null, "created": 1_i64 }],
            "projects": [], "contexts": []
        })).unwrap();
        let out = d.to_json();
        let t = &out["tasks"].as_array().unwrap()[0];
        assert!(t.get("parent").and_then(|v| v.as_str()).is_none(),
                "null in snapshot deletes the parent key");
    }

    #[test]
    fn nested_comments_map_id_is_stable_across_saves() {
        // CRDT correctness: when a task already has a `comments` Map and
        // Linux re-saves the task, the Map's ObjId must be reused (not
        // replaced with a fresh put_object). Otherwise two devices' new
        // comments land in two distinct Maps and Automerge merge picks
        // one as the winner — silently dropping the other.
        let mut base = TaskDoc::new().unwrap();
        base.apply_json(&json!({
            "tasks": [{
                "id": "t1", "list": "inbox", "title": "with comments",
                "created": 1_i64,
                "comments": {
                    "c1": { "id": "c1", "author": "A", "text": "first",
                            "createdAt": 100_i64 }
                }
            }],
            "projects": [], "contexts": []
        })).unwrap();
        let bytes = base.to_bytes();

        let mut a = TaskDoc::from_bytes(&bytes).unwrap();
        let mut b = TaskDoc::from_bytes(&bytes).unwrap();

        // Device A adds c2 via a snapshot save.
        let mut sa = a.to_json();
        sa["tasks"][0]["comments"]["c2"] = json!({
            "id": "c2", "author": "A", "text": "from A", "createdAt": 200_i64
        });
        a.apply_json(&sa).unwrap();

        // Device B adds c3.
        let mut sb = b.to_json();
        sb["tasks"][0]["comments"]["c3"] = json!({
            "id": "c3", "author": "B", "text": "from B", "createdAt": 300_i64
        });
        b.apply_json(&sb).unwrap();

        // After merge, both new comments must survive (they share the
        // base Map's ObjId so commentId-keyed inserts don't collide).
        a.merge(&mut b).unwrap();
        let out = a.to_json();
        let t = &out["tasks"].as_array().unwrap()[0];
        let comments = t["comments"].as_object().unwrap();
        assert_eq!(comments.len(), 3, "c1 + c2 + c3 all preserved");
        assert!(comments.contains_key("c1"));
        assert!(comments.contains_key("c2"));
        assert!(comments.contains_key("c3"));
    }
}
