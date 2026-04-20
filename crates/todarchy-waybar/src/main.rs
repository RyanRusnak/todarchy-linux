// todarchy-waybar — emits a single JSON line for waybar's custom module.
//
//   $ todarchy-waybar
//   {"text":"●  3","tooltip":"3 due today","class":"attention"}
//
// Run at an interval of 30s from waybar config. Silent when there is
// nothing due (prints an empty-text object so the module collapses).
//
// Schema matches the GUI (see src/ui/data.jsx):
//   due:        "today" | "tomorrow" | "this week" | ""
//   doneAt:     unix millis when completed (absent → active)
//   deferUntil: unix millis — task hidden until this time

use std::path::PathBuf;
use anyhow::{Context, Result};
use serde_json::{json, Value};

fn main() -> Result<()> {
    let out = match count_due_today() {
        Ok(0) => json!({ "text": "", "tooltip": "todarchy · all clear", "class": "ok" }),
        Ok(n) => json!({
            "text": format!("●  {}", n),
            "tooltip": format!("{} due today", n),
            "class": "attention",
        }),
        Err(_) => json!({ "text": "", "tooltip": "todarchy unavailable", "class": "error" }),
    };
    println!("{}", out);
    Ok(())
}

fn count_due_today() -> Result<usize> {
    let path = tasks_path()?;
    if !path.exists() {
        return Ok(0);
    }
    let text = std::fs::read_to_string(&path)?;
    let v: Value = serde_json::from_str(&text)?;
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis() as i64;
    let mut n = 0;
    if let Some(arr) = v.get("tasks").and_then(|t| t.as_array()) {
        for t in arr {
            let done = t.get("doneAt").and_then(|x| x.as_i64()).is_some();
            if done {
                continue;
            }
            let deferred = t
                .get("deferUntil")
                .and_then(|x| x.as_i64())
                .is_some_and(|d| d > now_ms);
            if deferred {
                continue;
            }
            let due = t.get("due").and_then(|x| x.as_str()).unwrap_or("");
            if due == "today" {
                n += 1;
            }
        }
    }
    Ok(n)
}

fn tasks_path() -> Result<PathBuf> {
    Ok(dirs::data_local_dir()
        .or_else(dirs::home_dir)
        .context("no data dir")?
        .join("todarchy/tasks.json"))
}
