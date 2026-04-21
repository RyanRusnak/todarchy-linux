// Tiny dev-loop helper: writes a tasks.automerge file into the directory
// you pass in, so you can test Linux's sync merge path without waiting
// on the macOS/iOS apps. The schema matches exactly what the production
// doc.rs writes (tasks as a Map<id, Task>).
//
// Usage:
//   cd src-tauri
//   cargo run --example seed_remote -- /path/to/sync/folder
//
// Then in todarchy: Ctrl-K → "sync: choose a folder…" → pick that path.
// Alternatively, run this WHILE todarchy is open with sync already
// pointed at the folder — the sync_watcher will see the write and
// merge it in live, exactly like another device pushing would.

use std::path::PathBuf;

use automerge::{transaction::Transactable, Automerge, ObjType, ROOT};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let folder = args
        .get(1)
        .cloned()
        .expect("usage: cargo run --example seed_remote -- <sync-folder>");
    let path: PathBuf = PathBuf::from(&folder).join("tasks.automerge");
    std::fs::create_dir_all(&folder)?;

    let mut doc = Automerge::new();
    let now: i64 = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis() as i64;

    {
        let mut tx = doc.transaction();
        tx.put(ROOT, "version", 1_i64)?;

        // tasks: Map<id, Task>
        let tasks = tx.put_object(ROOT, "tasks", ObjType::Map)?;
        let t1 = tx.put_object(&tasks, "seed-1", ObjType::Map)?;
        tx.put(&t1, "id", "seed-1")?;
        tx.put(&t1, "list", "inbox")?;
        tx.put(&t1, "title", "from seed_remote (pretend this is iOS)")?;
        tx.put(&t1, "ctx", "@phone")?;
        tx.put(&t1, "due", "today")?;
        tx.put(&t1, "created", now)?;

        let t2 = tx.put_object(&tasks, "seed-2", ObjType::Map)?;
        tx.put(&t2, "id", "seed-2")?;
        tx.put(&t2, "list", "inbox")?;
        tx.put(&t2, "title", "another pretend-iOS task")?;
        tx.put(&t2, "note", "second test row")?;
        tx.put(&t2, "created", now - 10_000)?;

        // projects: Map<id, Project> — empty here
        tx.put_object(ROOT, "projects", ObjType::Map)?;

        // contexts: List<String>
        let ctx = tx.put_object(ROOT, "contexts", ObjType::List)?;
        for (i, c) in ["@home", "@work", "@errands", "@mac", "@phone", "@read"]
            .iter()
            .enumerate()
        {
            tx.insert(&ctx, i, *c)?;
        }

        tx.commit();
    }

    // Atomic write, mirroring production doc.save().
    let bytes = doc.save();
    let tmp = path.with_extension("automerge.tmp");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, &path)?;
    println!("wrote 2 tasks to {}", path.display());
    Ok(())
}
