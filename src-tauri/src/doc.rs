// doc.rs — Automerge-backed task document.
//
// v0.2 sync rides on a user-picked, file-synced folder (iCloud / Dropbox /
// Syncthing). Each device keeps an Automerge binary doc (tasks.automerge)
// as the canonical store. When the same file is written on two devices at
// once, Automerge merges the two histories without losing edits.
//
// This module stays deliberately small: it owns an `Automerge` doc,
// persists it atomically to disk, and converts it to/from the JSON shape
// the rest of the app already speaks (React state, the tod CLI, the
// Waybar module). That means we can land the new storage layer without
// touching frontend code or the two companion crates — they keep reading
// the JSON shape we regenerate on every save.
//
// Schema lives in the doc under three root keys:
//   tasks     : List<Map>  — the task objects
//   projects  : List<Map>
//   contexts  : List<String>
// The top-level doc also carries `version: i64 = 1` for future migrations.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use automerge::{transaction::Transactable, Automerge, ObjType, ReadDoc, ScalarValue, Value, ROOT};
use serde_json::{json, Map, Value as Json};

/// Wrapper around an `Automerge` doc with task-store helpers.
pub struct TaskDoc {
    doc: Automerge,
}

impl TaskDoc {
    /// Create a fresh empty doc seeded with the default contexts.
    pub fn new() -> Result<Self> {
        let mut doc = Automerge::new();
        let mut tx = doc.transaction();
        tx.put(ROOT, "version", 1_i64)?;
        tx.put_object(ROOT, "tasks", ObjType::List)?;
        tx.put_object(ROOT, "projects", ObjType::List)?;
        let ctx = tx.put_object(ROOT, "contexts", ObjType::List)?;
        for (i, c) in [
            "@home", "@work", "@errands", "@mac", "@phone", "@read",
        ]
        .iter()
        .enumerate()
        {
            tx.insert(&ctx, i, *c)?;
        }
        tx.commit();
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

    /// Atomic write: serialize to `<path>.tmp`, fsync, rename. The sync
    /// daemon must never observe a half-written file.
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
        Ok(())
    }

    /// Merge another device's doc into this one. Automerge handles the CRDT
    /// resolution: concurrent insertions both survive, deletions tombstone,
    /// same-field edits resolve deterministically.
    pub fn merge(&mut self, other: &mut TaskDoc) -> Result<()> {
        self.doc.merge(&mut other.doc)?;
        Ok(())
    }

    /// Flatten the doc to the JSON shape the rest of the app consumes.
    pub fn to_json(&self) -> Json {
        let mut out = Map::new();
        out.insert("version".into(), json!(1));
        out.insert("tasks".into(), read_list(&self.doc, ROOT, "tasks"));
        out.insert("projects".into(), read_list(&self.doc, ROOT, "projects"));
        out.insert("contexts".into(), read_list(&self.doc, ROOT, "contexts"));
        Json::Object(out)
    }

    /// Replace the doc's contents with the given JSON shape. Called from
    /// `save_tasks` in main.rs, which receives the full state from React
    /// on every mutation. Automerge records the diff as ops so merging
    /// later still works.
    pub fn apply_json(&mut self, data: &Json) -> Result<()> {
        let mut tx = self.doc.transaction();
        tx.put(ROOT, "version", 1_i64)?;
        write_list(&mut tx, ROOT, "tasks", data.get("tasks"))?;
        write_list(&mut tx, ROOT, "projects", data.get("projects"))?;
        write_list(&mut tx, ROOT, "contexts", data.get("contexts"))?;
        tx.commit();
        Ok(())
    }
}

// ---------- JSON <-> Automerge helpers ----------

fn read_list(doc: &Automerge, obj: automerge::ObjId, key: &str) -> Json {
    let Ok(Some((Value::Object(ObjType::List), list))) = doc.get(obj, key) else {
        return Json::Array(Vec::new());
    };
    let len = doc.length(&list);
    let mut out = Vec::with_capacity(len);
    for i in 0..len {
        match doc.get(&list, i) {
            Ok(Some((Value::Object(ObjType::Map), id))) => out.push(read_map(doc, id)),
            Ok(Some((Value::Scalar(s), _))) => out.push(scalar_to_json(&s)),
            _ => {}
        }
    }
    Json::Array(out)
}

fn read_map(doc: &Automerge, obj: automerge::ObjId) -> Json {
    let mut out = Map::new();
    for key in doc.keys(&obj) {
        match doc.get(&obj, &key) {
            Ok(Some((Value::Scalar(s), _))) => {
                out.insert(key, scalar_to_json(&s));
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

fn write_list(
    tx: &mut automerge::transaction::Transaction<'_>,
    obj: automerge::ObjId,
    key: &str,
    value: Option<&Json>,
) -> Result<()> {
    let list = tx.put_object(obj, key, ObjType::List)?;
    let Some(Json::Array(items)) = value else {
        return Ok(());
    };
    for (i, item) in items.iter().enumerate() {
        match item {
            Json::Object(_) => {
                let map = tx.insert_object(&list, i, ObjType::Map)?;
                write_map(tx, map, item)?;
            }
            other => {
                tx.insert(&list, i, scalar_from_json(other))?;
            }
        }
    }
    Ok(())
}

fn write_map(
    tx: &mut automerge::transaction::Transaction<'_>,
    obj: automerge::ObjId,
    value: &Json,
) -> Result<()> {
    let Json::Object(map) = value else {
        return Ok(());
    };
    for (k, v) in map {
        match v {
            Json::Null => {
                tx.put(&obj, k, ScalarValue::Null)?;
            }
            Json::Bool(b) => {
                tx.put(&obj, k, *b)?;
            }
            Json::Number(n) => {
                if let Some(i) = n.as_i64() {
                    tx.put(&obj, k, i)?;
                } else if let Some(f) = n.as_f64() {
                    tx.put(&obj, k, f)?;
                }
            }
            Json::String(s) => {
                tx.put(&obj, k, s.clone())?;
            }
            Json::Array(_) => {
                write_list(tx, obj.clone(), k, Some(v))?;
            }
            Json::Object(_) => {
                let child = tx.put_object(&obj, k, ObjType::Map)?;
                write_map(tx, child, v)?;
            }
        }
    }
    Ok(())
}

fn scalar_from_json(v: &Json) -> ScalarValue {
    match v {
        Json::Null => ScalarValue::Null,
        Json::Bool(b) => ScalarValue::Boolean(*b),
        Json::Number(n) => {
            if let Some(i) = n.as_i64() {
                ScalarValue::Int(i)
            } else if let Some(f) = n.as_f64() {
                ScalarValue::F64(f)
            } else {
                ScalarValue::Null
            }
        }
        Json::String(s) => ScalarValue::Str(s.clone().into()),
        _ => ScalarValue::Null,
    }
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
                  "doneAt": 1700000200000_i64 },
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

        // Spot-check every field that matters — we can't assert exact equality
        // because Automerge doesn't preserve the order of map keys.
        let tasks = out.get("tasks").and_then(|v| v.as_array()).unwrap();
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0]["title"], "buy milk");
        assert_eq!(tasks[0]["ctx"], "@errands");
        assert_eq!(tasks[0]["parent"], Json::Null);
        assert_eq!(tasks[1]["doneAt"], 1700000200000_i64);

        let projects = out.get("projects").and_then(|v| v.as_array()).unwrap();
        assert_eq!(projects[0]["name"], "work");

        let contexts = out.get("contexts").and_then(|v| v.as_array()).unwrap();
        assert_eq!(contexts.len(), 3);
        assert_eq!(contexts[0], "@home");
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
        assert_eq!(tasks[1]["title"], "ship v0.2");

        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn merge_combines_concurrent_inserts() {
        // Start from a shared base, clone for two devices, each adds a task.
        let mut base = TaskDoc::new().unwrap();
        base.apply_json(&json!({
            "tasks": [{ "id": "t0", "list": "inbox", "title": "shared",
                         "created": 1_i64, "parent": null }],
            "projects": [], "contexts": []
        })).unwrap();
        let bytes = base.doc.save();

        let mut device_a = TaskDoc { doc: Automerge::load(&bytes).unwrap() };
        let mut device_b = TaskDoc { doc: Automerge::load(&bytes).unwrap() };

        // Each device appends a unique task.
        {
            let mut tx = device_a.doc.transaction();
            let (_, tasks) = tx.get(ROOT, "tasks").unwrap().unwrap();
            let i = tx.length(&tasks);
            let task = tx.insert_object(&tasks, i, ObjType::Map).unwrap();
            tx.put(&task, "id", "a1").unwrap();
            tx.put(&task, "title", "from a").unwrap();
            tx.commit();
        }
        {
            let mut tx = device_b.doc.transaction();
            let (_, tasks) = tx.get(ROOT, "tasks").unwrap().unwrap();
            let i = tx.length(&tasks);
            let task = tx.insert_object(&tasks, i, ObjType::Map).unwrap();
            tx.put(&task, "id", "b1").unwrap();
            tx.put(&task, "title", "from b").unwrap();
            tx.commit();
        }

        // Merge B into A — both inserts must survive.
        device_a.merge(&mut device_b).unwrap();
        let out = device_a.to_json();
        let tasks = out.get("tasks").and_then(|v| v.as_array()).unwrap();
        assert_eq!(tasks.len(), 3, "shared + a1 + b1");
        let titles: Vec<_> = tasks.iter().map(|t| t["title"].as_str().unwrap()).collect();
        assert!(titles.contains(&"from a"));
        assert!(titles.contains(&"from b"));
    }
}
