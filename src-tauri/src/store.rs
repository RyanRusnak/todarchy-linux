// store.rs — tasks.json read/write with atomic-replace + backup rotation.
//
// Layout:
//   ~/.local/share/todarchy/tasks.json      primary
//   ~/.local/share/todarchy/tasks.json.bak  previous revision
//
// The schema is JSON (not SQLite) by design: human-readable, grep-able,
// easy for the CLI + waybar module to share without a db driver.

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde_json::Value;
use tauri::AppHandle;
use tokio::fs;

fn data_dir() -> Result<PathBuf> {
    let home = dirs::data_local_dir()
        .or_else(dirs::home_dir)
        .context("no data dir")?;
    let d = home.join("todarchy");
    std::fs::create_dir_all(&d)?;
    Ok(d)
}

fn tasks_path() -> Result<PathBuf> {
    Ok(data_dir()?.join("tasks.json"))
}

pub async fn load(_app: &AppHandle) -> Result<Value> {
    let path = tasks_path()?;
    if !path.exists() {
        // seed with empty shape so frontend has something to edit
        let seed = serde_json::json!({
            "version": 1,
            "tasks": [],
            "projects": [],
            "contexts": ["@home", "@work", "@errands", "@mac", "@phone", "@read"]
        });
        return Ok(seed);
    }
    let text = fs::read_to_string(&path).await?;
    let v: Value = serde_json::from_str(&text)?;
    Ok(v)
}

pub async fn save(_app: &AppHandle, data: Value) -> Result<()> {
    let path = tasks_path()?;
    let bak = path.with_extension("json.bak");
    let tmp = path.with_extension("json.tmp");

    // rotate existing → .bak
    if path.exists() {
        let _ = fs::copy(&path, &bak).await;
    }
    let pretty = serde_json::to_vec_pretty(&data)?;
    fs::write(&tmp, &pretty).await?;
    // atomic rename
    fs::rename(&tmp, &path).await?;
    Ok(())
}
