// tod — CLI companion to the todarchy GUI.
//
// Shares ~/.local/share/todarchy/tasks.json with the GUI via fs2 file
// locking, so writes from the CLI appear in the GUI on next reload (and
// vice-versa). Schema matches the GUI shape (see src/ui/data.jsx).
//
//   tod add "fix bug @work !today"
//   tod list                          # today view (overdue + due today + undated)
//   tod list --all                    # everything
//   tod done <id>                     # prefix-match on the task id
//   tod defer <id> +3d                # tomorrow, +3d, +1w, mon..sun, YYYY-MM-DD
//
// Quick-add parser mirrors the GUI's:
//   @foo   → context (stored as "@foo")
//   #proj  → project (GUI doesn't have free-form projects from the CLI yet,
//            so this is ignored for v0.1 — added tasks land in the inbox)
//   !today !tomorrow !week           → due keyword

use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use fs2::FileExt;
use serde_json::{json, Value};

#[derive(Parser)]
#[command(name = "tod", version, about = "Omarchy-native tasks — CLI companion to the todarchy GUI")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Add a task. Supports @context #project !today|tomorrow|week inline.
    Add { text: Vec<String> },
    /// List tasks. Defaults to today.
    List {
        #[arg(long)]
        all: bool,
    },
    /// Mark a task done.
    Done { id: String },
    /// Defer a task (today, tomorrow, mon..sun, +3d, +1w, YYYY-MM-DD).
    Defer { id: String, when: String },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Add { text } => add(&text.join(" ")),
        Cmd::List { all } => list(all),
        Cmd::Done { id } => done(&id),
        Cmd::Defer { id, when } => defer(&id, &when),
    }
}

fn tasks_path() -> Result<PathBuf> {
    let d = dirs::data_local_dir()
        .or_else(dirs::home_dir)
        .context("no data dir")?
        .join("todarchy");
    std::fs::create_dir_all(&d)?;
    Ok(d.join("tasks.json"))
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn with_store<F: FnOnce(&mut Value) -> Result<()>>(mutator: F) -> Result<()> {
    let path = tasks_path()?;
    let mut f = OpenOptions::new()
        .read(true).write(true).create(true).truncate(false)
        .open(&path)?;
    f.lock_exclusive()?;

    let mut buf = String::new();
    f.read_to_string(&mut buf)?;
    let mut data: Value = if buf.trim().is_empty() {
        json!({
            "version": 1,
            "tasks": [],
            "projects": [],
            "contexts": ["@home","@work","@errands","@mac","@phone","@read"],
        })
    } else {
        serde_json::from_str(&buf)?
    };

    mutator(&mut data)?;

    let out = serde_json::to_vec_pretty(&data)?;
    f.set_len(0)?;
    f.seek(SeekFrom::Start(0))?;
    f.write_all(&out)?;
    f.unlock()?;
    Ok(())
}

fn add(text: &str) -> Result<()> {
    let parsed = parse_quick_add(text);
    if parsed.title.is_empty() {
        anyhow::bail!("refusing to add an empty task");
    }
    with_store(|data| {
        let arr = data["tasks"].as_array_mut().context("tasks missing")?;
        let mut task = json!({
            "id": uuid::Uuid::new_v4().to_string(),
            "list": "inbox",
            "title": parsed.title,
            "note": parsed.note,
            "created": now_ms(),
            "parent": Value::Null,
        });
        if let Some(ctx) = &parsed.ctx {
            task["ctx"] = Value::String(ctx.clone());
        }
        if let Some(due) = &parsed.due {
            task["due"] = Value::String(due.clone());
        }
        arr.push(task);
        Ok(())
    })?;
    println!("✓ added: {}", parsed.title);
    Ok(())
}

fn list(all: bool) -> Result<()> {
    let path = tasks_path()?;
    if !path.exists() {
        println!("(no tasks yet — add some with `tod add \"...\"`)");
        return Ok(());
    }
    let text = std::fs::read_to_string(&path)?;
    let data: Value = serde_json::from_str(&text)?;
    let now = now_ms();

    let arr = data["tasks"].as_array().cloned().unwrap_or_default();
    let mut rows: Vec<&Value> = arr
        .iter()
        .filter(|t| {
            let done = t.get("doneAt").and_then(|v| v.as_i64()).is_some();
            if done {
                return all;
            }
            // skip deferred tasks unless --all
            let deferred = t
                .get("deferUntil")
                .and_then(|v| v.as_i64())
                .is_some_and(|d| d > now);
            if deferred {
                return all;
            }
            if all {
                return true;
            }
            // today view: due keyword == today OR no due date (inbox items)
            let due = t.get("due").and_then(|v| v.as_str()).unwrap_or("");
            matches!(due, "today" | "tomorrow" | "")
        })
        .collect();

    let due_rank = |t: &&Value| -> i32 {
        match t.get("due").and_then(|v| v.as_str()).unwrap_or("") {
            "today" => 0,
            "tomorrow" => 1,
            "this week" => 2,
            _ => 3,
        }
    };
    rows.sort_by_key(|t| (due_rank(t), t.get("created").and_then(|v| v.as_i64()).unwrap_or(0)));

    if rows.is_empty() {
        println!(
            "{}",
            if all {
                "(no tasks)"
            } else {
                "(nothing due today — use `tod list --all` for everything)"
            }
        );
        return Ok(());
    }

    for t in rows {
        let id_short = t
            .get("id")
            .and_then(|v| v.as_str())
            .map(|s| s.chars().take(8).collect::<String>())
            .unwrap_or_else(|| "????????".into());
        let title = t.get("title").and_then(|v| v.as_str()).unwrap_or("(untitled)");
        let done = t.get("doneAt").and_then(|v| v.as_i64()).is_some();
        let ctx = t.get("ctx").and_then(|v| v.as_str()).unwrap_or("");
        let due = t.get("due").and_then(|v| v.as_str()).unwrap_or("");
        let due_str = if due.is_empty() {
            String::new()
        } else {
            format!(" [{due}]")
        };
        let mark = if done { "✓" } else { "·" };
        let ctx_str = if ctx.is_empty() {
            String::new()
        } else {
            format!(" {ctx}")
        };
        println!("{mark} {id_short}  {title}{ctx_str}{due_str}");
    }
    Ok(())
}

fn done(id: &str) -> Result<()> {
    let mut matched = false;
    with_store(|data| {
        let arr = data["tasks"].as_array_mut().context("tasks missing")?;
        for t in arr.iter_mut() {
            let tid = t.get("id").and_then(|v| v.as_str()).unwrap_or("");
            if tid == id || tid.starts_with(id) {
                t["doneAt"] = Value::from(now_ms());
                // clear any deferral — done supersedes
                if t.get("deferUntil").is_some() {
                    t.as_object_mut().unwrap().remove("deferUntil");
                }
                matched = true;
                break;
            }
        }
        Ok(())
    })?;
    if matched {
        println!("✓ done");
    } else {
        println!("no task matching id '{id}'");
    }
    Ok(())
}

fn defer(id: &str, when: &str) -> Result<()> {
    let Some(ts) = parse_when(when) else {
        anyhow::bail!(
            "couldn't parse `{when}`. try: today, tomorrow, mon..sun, +3d, +1w, YYYY-MM-DD"
        );
    };
    let mut matched = false;
    with_store(|data| {
        let arr = data["tasks"].as_array_mut().context("tasks missing")?;
        for t in arr.iter_mut() {
            let tid = t.get("id").and_then(|v| v.as_str()).unwrap_or("");
            if tid == id || tid.starts_with(id) {
                t["deferUntil"] = Value::from(ts);
                matched = true;
                break;
            }
        }
        Ok(())
    })?;
    if matched {
        println!("↻ deferred");
    } else {
        println!("no task matching id '{id}'");
    }
    Ok(())
}

struct QuickAdd {
    title: String,
    ctx: Option<String>,
    due: Option<String>,
    note: String,
}

fn parse_quick_add(text: &str) -> QuickAdd {
    let mut title_parts = Vec::new();
    let mut ctx = None;
    let mut due = None;
    let mut note = String::new();
    let mut in_note = false;
    for tok in text.split_whitespace() {
        if in_note {
            if !note.is_empty() {
                note.push(' ');
            }
            note.push_str(tok);
            continue;
        }
        if let Some(c) = tok.strip_prefix('@') {
            let val = if c.is_empty() { String::new() } else { format!("@{}", c) };
            if !val.is_empty() {
                ctx = Some(val);
            }
        } else if tok.starts_with('#') {
            // projects from the CLI are deferred to v0.2 — dropped for now
        } else if let Some(w) = tok.strip_prefix('!') {
            due = match w.to_ascii_lowercase().as_str() {
                "today" => Some("today".into()),
                "tomorrow" | "tmrw" => Some("tomorrow".into()),
                "week" | "thisweek" => Some("this week".into()),
                _ => due,
            };
        } else if tok == "/" {
            in_note = true;
        } else if let Some(rest) = tok.strip_prefix('/') {
            in_note = true;
            note.push_str(rest);
        } else {
            title_parts.push(tok);
        }
    }
    QuickAdd {
        title: title_parts.join(" "),
        ctx,
        due,
        note,
    }
}

fn parse_when(w: &str) -> Option<i64> {
    use chrono::{Datelike, Duration, Local, NaiveDate, Weekday};
    let now = Local::now();
    let at_9am = |d: chrono::DateTime<Local>| -> Option<i64> {
        d.date_naive()
            .and_hms_opt(9, 0, 0)
            .and_then(|x| x.and_local_timezone(Local).single())
            .map(|x| x.timestamp_millis())
    };
    match w.to_ascii_lowercase().as_str() {
        "today" => return at_9am(now),
        "tomorrow" | "tmrw" => return at_9am(now + Duration::days(1)),
        _ => {}
    }
    let wd = match w.to_ascii_lowercase().as_str() {
        "mon" | "monday" => Some(Weekday::Mon),
        "tue" | "tuesday" => Some(Weekday::Tue),
        "wed" | "wednesday" => Some(Weekday::Wed),
        "thu" | "thursday" => Some(Weekday::Thu),
        "fri" | "friday" => Some(Weekday::Fri),
        "sat" | "saturday" => Some(Weekday::Sat),
        "sun" | "sunday" => Some(Weekday::Sun),
        _ => None,
    };
    if let Some(target) = wd {
        let today = now.weekday().num_days_from_monday() as i64;
        let want = target.num_days_from_monday() as i64;
        let delta = ((want - today).rem_euclid(7)).max(1);
        return at_9am(now + Duration::days(delta));
    }
    if let Some(rest) = w.strip_prefix('+') {
        if let Some(num) = rest.get(..rest.len().saturating_sub(1)).and_then(|n| n.parse::<i64>().ok()) {
            let unit = rest.chars().last()?;
            return match unit {
                'd' => at_9am(now + Duration::days(num)),
                'w' => at_9am(now + Duration::weeks(num)),
                'h' => Some((now + Duration::hours(num)).timestamp_millis()),
                _ => None,
            };
        }
    }
    if let Ok(d) = NaiveDate::parse_from_str(w, "%Y-%m-%d") {
        return d
            .and_hms_opt(9, 0, 0)
            .and_then(|x| x.and_local_timezone(Local).single())
            .map(|x| x.timestamp_millis());
    }
    None
}
