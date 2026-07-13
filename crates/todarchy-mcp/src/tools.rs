// tools.rs — the MCP tools, implemented against todarchy-core's store.
//
// Every tool loads the latest state (which pulls from sync), mutates the JSON
// projection, and saves through core (which pushes to sync). Task ids are
// UUIDs; tools accept a full id, an 8-char prefix, or a unique title
// substring so the model can refer to tasks naturally.

use serde_json::{json, Value};
use todarchy_core::{store, NullSink};

pub enum ToolError {
    Unknown,
    Failed(String),
}

fn fail(msg: impl Into<String>) -> ToolError {
    ToolError::Failed(msg.into())
}

/// Tool catalog returned by tools/list.
pub fn list() -> Vec<Value> {
    vec![
        json!({
            "name": "list_projects",
            "description": "List all projects/lists (including the built-in inbox) with the number of open tasks in each.",
            "inputSchema": { "type": "object", "properties": {} }
        }),
        json!({
            "name": "list_tasks",
            "description": "List tasks. Optionally filter to one project/list, and optionally include completed tasks.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": { "type": "string", "description": "Project name or id, or 'inbox'. Omit to list tasks across all lists." },
                    "include_done": { "type": "boolean", "description": "Include completed tasks (default false)." }
                }
            }
        }),
        json!({
            "name": "add_task",
            "description": "Add a new task. Use this to capture todos or add items to a shared list (e.g. groceries).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "The task text." },
                    "project": { "type": "string", "description": "Project name or id; defaults to inbox." },
                    "context": { "type": "string", "description": "A context tag like @errands or @work." },
                    "due": { "type": "string", "enum": ["today", "tomorrow", "this week"], "description": "Optional due bucket." },
                    "note": { "type": "string", "description": "Optional longer note (markdown)." }
                },
                "required": ["title"]
            }
        }),
        json!({
            "name": "complete_task",
            "description": "Mark a task complete.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "task": { "type": "string", "description": "Task id, 8-char id prefix, or a unique substring of the title." }
                },
                "required": ["task"]
            }
        }),
        json!({
            "name": "update_task",
            "description": "Change a task's title, note, due date, or context.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "task": { "type": "string", "description": "Task id, 8-char prefix, or unique title substring." },
                    "title": { "type": "string" },
                    "note": { "type": "string" },
                    "due": { "type": "string", "enum": ["today", "tomorrow", "this week", ""], "description": "Empty string clears the due date." },
                    "context": { "type": "string" }
                },
                "required": ["task"]
            }
        }),
        json!({
            "name": "delete_task",
            "description": "Delete a task permanently.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "task": { "type": "string", "description": "Task id, 8-char prefix, or unique title substring." }
                },
                "required": ["task"]
            }
        }),
    ]
}

pub async fn call(name: &str, args: Value) -> Result<String, ToolError> {
    match name {
        "list_projects" => list_projects().await,
        "list_tasks" => list_tasks(args).await,
        "add_task" => add_task(args).await,
        "complete_task" => complete_task(args).await,
        "update_task" => update_task(args).await,
        "delete_task" => delete_task(args).await,
        _ => Err(ToolError::Unknown),
    }
}

// ---------- helpers ----------

async fn load() -> Result<Value, ToolError> {
    store::load(&NullSink).await.map_err(|e| fail(format!("load failed: {e}")))
}

async fn save(v: Value) -> Result<(), ToolError> {
    store::save(&NullSink, v).await.map_err(|e| fail(format!("save failed: {e}")))
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn arg_str(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn projects(store: &Value) -> &[Value] {
    store.get("projects").and_then(Value::as_array).map(|a| a.as_slice()).unwrap_or(&[])
}

fn tasks(store: &Value) -> &[Value] {
    store.get("tasks").and_then(Value::as_array).map(|a| a.as_slice()).unwrap_or(&[])
}

/// Human name for a list id.
fn list_name(store: &Value, list_id: &str) -> String {
    if list_id == "inbox" {
        return "inbox".into();
    }
    projects(store)
        .iter()
        .find(|p| p.get("id").and_then(Value::as_str) == Some(list_id))
        .and_then(|p| p.get("name").and_then(Value::as_str))
        .unwrap_or(list_id)
        .to_string()
}

/// Resolve a project name-or-id to its list id.
fn resolve_project(store: &Value, sel: &str) -> Result<String, ToolError> {
    if sel.eq_ignore_ascii_case("inbox") {
        return Ok("inbox".into());
    }
    // by id
    if let Some(p) = projects(store).iter().find(|p| p.get("id").and_then(Value::as_str) == Some(sel)) {
        return Ok(p.get("id").and_then(Value::as_str).unwrap().to_string());
    }
    // by name (case-insensitive)
    let matches: Vec<&Value> = projects(store)
        .iter()
        .filter(|p| {
            p.get("name")
                .and_then(Value::as_str)
                .map(|n| n.eq_ignore_ascii_case(sel))
                .unwrap_or(false)
        })
        .collect();
    match matches.as_slice() {
        [p] => Ok(p.get("id").and_then(Value::as_str).unwrap().to_string()),
        [] => Err(fail(format!("no project named '{sel}'. Use list_projects to see options."))),
        _ => Err(fail(format!("more than one project named '{sel}'; use its id."))),
    }
}

/// Resolve a task selector (full id / 8+ char prefix / unique title substring)
/// to a full task id.
fn resolve_task(store: &Value, sel: &str) -> Result<String, ToolError> {
    let all = tasks(store);
    let id_of = |t: &Value| t.get("id").and_then(Value::as_str).unwrap_or("").to_string();

    // exact id
    if let Some(t) = all.iter().find(|t| id_of(t) == sel) {
        return Ok(id_of(t));
    }
    // id prefix (≥4 chars)
    if sel.len() >= 4 {
        let hits: Vec<String> = all.iter().map(id_of).filter(|id| id.starts_with(sel)).collect();
        if hits.len() == 1 {
            return Ok(hits[0].clone());
        }
        if hits.len() > 1 {
            return Err(fail(format!("id prefix '{sel}' matches {} tasks; be more specific.", hits.len())));
        }
    }
    // unique title substring (case-insensitive)
    let needle = sel.to_lowercase();
    let hits: Vec<&Value> = all
        .iter()
        .filter(|t| {
            t.get("title")
                .and_then(Value::as_str)
                .map(|s| s.to_lowercase().contains(&needle))
                .unwrap_or(false)
        })
        .collect();
    match hits.as_slice() {
        [t] => Ok(id_of(t)),
        [] => Err(fail(format!("no task matching '{sel}'."))),
        _ => Err(fail(format!(
            "'{sel}' matches {} tasks: {}. Be more specific or use an id.",
            hits.len(),
            hits.iter()
                .filter_map(|t| t.get("title").and_then(Value::as_str))
                .take(5)
                .collect::<Vec<_>>()
                .join(" · ")
        ))),
    }
}

fn normalize_due(due: &str) -> String {
    match due.trim().to_lowercase().as_str() {
        "week" | "this week" => "this week".into(),
        "today" => "today".into(),
        "tomorrow" => "tomorrow".into(),
        _ => String::new(),
    }
}

fn fmt_task(store: &Value, t: &Value, show_project: bool) -> String {
    let id = t.get("id").and_then(Value::as_str).unwrap_or("");
    let short = id.get(0..8).unwrap_or(id);
    let done = t.get("doneAt").and_then(Value::as_i64).is_some();
    let box_ = if done { "[x]" } else { "[ ]" };
    let title = t.get("title").and_then(Value::as_str).unwrap_or("");
    let mut s = format!("{short}  {box_} {title}");
    if let Some(ctx) = t.get("ctx").and_then(Value::as_str).filter(|c| !c.is_empty()) {
        s.push_str(&format!("  {ctx}"));
    }
    if let Some(due) = t.get("due").and_then(Value::as_str).filter(|d| !d.is_empty()) {
        s.push_str(&format!("  !{due}"));
    }
    if let Some(defer) = t.get("deferUntil").and_then(Value::as_i64) {
        if defer > now_ms() {
            s.push_str("  (deferred)");
        }
    }
    if show_project {
        let list = t.get("list").and_then(Value::as_str).unwrap_or("inbox");
        s.push_str(&format!("  [{}]", list_name(store, list)));
    }
    s
}

// ---------- tools ----------

async fn list_projects() -> Result<String, ToolError> {
    let store = load().await?;
    let now = now_ms();
    let open_in = |list: &str| {
        tasks(&store)
            .iter()
            .filter(|t| {
                t.get("list").and_then(Value::as_str) == Some(list)
                    && t.get("doneAt").and_then(Value::as_i64).is_none()
                    && t.get("deferUntil").and_then(Value::as_i64).map(|d| d <= now).unwrap_or(true)
            })
            .count()
    };
    let mut lines = vec![format!("inbox — {} open", open_in("inbox"))];
    for p in projects(&store) {
        let id = p.get("id").and_then(Value::as_str).unwrap_or("");
        let name = p.get("name").and_then(Value::as_str).unwrap_or("(unnamed)");
        let shared = p.get("isShared").and_then(Value::as_bool).unwrap_or(false);
        let tag = if shared { " (shared)" } else { "" };
        lines.push(format!("{name}{tag} — {} open", open_in(id)));
    }
    Ok(lines.join("\n"))
}

async fn list_tasks(args: Value) -> Result<String, ToolError> {
    let store = load().await?;
    let include_done = args.get("include_done").and_then(Value::as_bool).unwrap_or(false);
    let filter_list = match arg_str(&args, "project") {
        Some(p) => Some(resolve_project(&store, &p)?),
        None => None,
    };
    let now = now_ms();
    let show_project = filter_list.is_none();

    let mut rows: Vec<String> = Vec::new();
    for t in tasks(&store) {
        if let Some(ref l) = filter_list {
            if t.get("list").and_then(Value::as_str) != Some(l.as_str()) {
                continue;
            }
        }
        let done = t.get("doneAt").and_then(Value::as_i64).is_some();
        if done && !include_done {
            continue;
        }
        rows.push(fmt_task(&store, t, show_project));
        let _ = now;
    }
    if rows.is_empty() {
        return Ok("(no matching tasks)".into());
    }
    let scope = match &filter_list {
        Some(l) => format!("{} — ", list_name(&store, l)),
        None => String::new(),
    };
    Ok(format!("{scope}{} task(s)\n{}", rows.len(), rows.join("\n")))
}

async fn add_task(args: Value) -> Result<String, ToolError> {
    let title = arg_str(&args, "title").ok_or_else(|| fail("title is required"))?;
    let mut store = load().await?;
    let list = match arg_str(&args, "project") {
        Some(p) => resolve_project(&store, &p)?,
        None => "inbox".to_string(),
    };
    let now = now_ms();
    let task = json!({
        "id": uuid::Uuid::new_v4().to_string(),
        "list": list,
        "title": title,
        "ctx": arg_str(&args, "context").unwrap_or_default(),
        "due": arg_str(&args, "due").map(|d| normalize_due(&d)).unwrap_or_default(),
        "note": arg_str(&args, "note").unwrap_or_default(),
        "created": now,
        "pos": now,
    });
    let short = task["id"].as_str().unwrap()[0..8].to_string();
    let where_ = list_name(&store, &list);
    store
        .get_mut("tasks")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| fail("store missing tasks array"))?
        .push(task);
    save(store).await?;
    Ok(format!("Added \"{title}\" to {where_} (id {short})."))
}

async fn complete_task(args: Value) -> Result<String, ToolError> {
    let sel = arg_str(&args, "task").ok_or_else(|| fail("task is required"))?;
    let mut store = load().await?;
    let id = resolve_task(&store, &sel)?;
    let now = now_ms();
    let mut title = String::new();
    for t in store.get_mut("tasks").and_then(Value::as_array_mut).unwrap() {
        if t.get("id").and_then(Value::as_str) == Some(id.as_str()) {
            title = t.get("title").and_then(Value::as_str).unwrap_or("").to_string();
            t["doneAt"] = json!(now);
        }
    }
    save(store).await?;
    Ok(format!("Completed \"{title}\"."))
}

async fn update_task(args: Value) -> Result<String, ToolError> {
    let sel = arg_str(&args, "task").ok_or_else(|| fail("task is required"))?;
    let mut store = load().await?;
    let id = resolve_task(&store, &sel)?;
    let mut changed = Vec::new();
    for t in store.get_mut("tasks").and_then(Value::as_array_mut).unwrap() {
        if t.get("id").and_then(Value::as_str) != Some(id.as_str()) {
            continue;
        }
        if let Some(v) = arg_str(&args, "title") {
            t["title"] = json!(v);
            changed.push("title");
        }
        if let Some(v) = arg_str(&args, "note") {
            t["note"] = json!(v);
            changed.push("note");
        }
        if let Some(v) = arg_str(&args, "context") {
            t["ctx"] = json!(v);
            changed.push("context");
        }
        // due: present-but-empty clears it
        if let Some(v) = args.get("due").and_then(Value::as_str) {
            t["due"] = json!(normalize_due(v));
            changed.push("due");
        }
    }
    if changed.is_empty() {
        return Err(fail("nothing to update — provide title, note, due, or context"));
    }
    save(store).await?;
    Ok(format!("Updated {} on the task.", changed.join(", ")))
}

async fn delete_task(args: Value) -> Result<String, ToolError> {
    let sel = arg_str(&args, "task").ok_or_else(|| fail("task is required"))?;
    let store = load().await?;
    let id = resolve_task(&store, &sel)?;
    let title = tasks(&store)
        .iter()
        .find(|t| t.get("id").and_then(Value::as_str) == Some(id.as_str()))
        .and_then(|t| t.get("title").and_then(Value::as_str))
        .unwrap_or("")
        .to_string();
    store::delete_many(&NullSink, "tasks", std::slice::from_ref(&id))
        .await
        .map_err(|e| fail(format!("delete failed: {e}")))?;
    Ok(format!("Deleted \"{title}\"."))
}
