// tod — CLI companion to the todokase TUI.
//
// Drives the same todarchy-core store as the TUI and MCP server, so writes go
// through Automerge and ride your configured sync (a task added here shows up
// on your other devices), and reads pull the latest state. The `todokase`
// binary also accepts these subcommands (it delegates to `tod`).
//
//   tod add "fix bug @work !today"
//   tod list                          # today view (overdue + due today + undated)
//   tod list --all                    # everything
//   tod done <id>                     # prefix-match on the task id
//   tod defer <id> +3d                # tomorrow, +3d, +1w, mon..sun, YYYY-MM-DD
//
// Quick-add parser mirrors the TUI's:
//   @foo   → context (stored as "@foo")
//   #proj  → project (dropped for now; added tasks land in the inbox)
//   !today !tomorrow !week           → due keyword

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use serde_json::{json, Value};
use todarchy_core::{store, NullSink};

#[derive(Parser)]
#[command(name = "tod", version, about = "Omarchy-native tasks — CLI companion to the todokase TUI")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Add a task. Supports @context !today|tomorrow|week inline.
    Add {
        /// Project/list to add to (name, id, or "inbox"). Defaults to inbox.
        #[arg(short = 'p', long = "project")]
        project: Option<String>,
        text: Vec<String>,
    },
    /// List tasks in a project (defaults to inbox).
    List {
        /// Project/list to show (name, id, or "inbox"). Defaults to inbox.
        #[arg(short = 'p', long = "project")]
        project: Option<String>,
        /// Include done + deferred tasks.
        #[arg(long)]
        all: bool,
    },
    /// Mark a task done.
    Done { id: String },
    /// Defer a task (today, tomorrow, mon..sun, +3d, +1w, YYYY-MM-DD).
    Defer { id: String, when: String },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Add { project, text } => add(&text.join(" "), project).await,
        Cmd::List { project, all } => list(project, all).await,
        Cmd::Done { id } => done(&id).await,
        Cmd::Defer { id, when } => defer(&id, &when).await,
    }
}

/// Resolve a project selector (name, id, or "inbox") to a task `list` id.
fn resolve_list(data: &Value, sel: &str) -> Result<String> {
    if sel.eq_ignore_ascii_case("inbox") {
        return Ok("inbox".into());
    }
    let projects = data.get("projects").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    if let Some(p) = projects.iter().find(|p| p.get("id").and_then(|v| v.as_str()) == Some(sel)) {
        return Ok(p["id"].as_str().unwrap().to_string());
    }
    let matches: Vec<&Value> = projects
        .iter()
        .filter(|p| {
            p.get("name")
                .and_then(|v| v.as_str())
                .map(|n| n.eq_ignore_ascii_case(sel))
                .unwrap_or(false)
        })
        .collect();
    match matches.as_slice() {
        [p] => Ok(p["id"].as_str().unwrap().to_string()),
        [] => {
            let names: Vec<String> = projects
                .iter()
                .filter_map(|p| p.get("name").and_then(|v| v.as_str()).map(String::from))
                .collect();
            anyhow::bail!("no project '{sel}'. available: inbox, {}", names.join(", "))
        }
        _ => anyhow::bail!("more than one project named '{sel}'; use its id"),
    }
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Load the store (pulls sync), let the caller mutate the JSON, save it back
/// through core (writes Automerge + pushes sync).
async fn with_store<F: FnOnce(&mut Value) -> Result<()>>(mutator: F) -> Result<()> {
    let mut data = store::load(&NullSink).await?;
    mutator(&mut data)?;
    store::save(&NullSink, data).await?;
    Ok(())
}

async fn add(text: &str, project: Option<String>) -> Result<()> {
    let parsed = parse_quick_add(text);
    if parsed.title.is_empty() {
        anyhow::bail!("refusing to add an empty task");
    }
    let title = parsed.title.clone();
    let dest = project.clone().unwrap_or_else(|| "inbox".into());
    with_store(move |data| {
        let list = match &project {
            Some(p) => resolve_list(data, p)?,
            None => "inbox".to_string(),
        };
        let arr = data["tasks"].as_array_mut().context("tasks missing")?;
        let now = now_ms();
        let mut task = json!({
            "id": uuid::Uuid::new_v4().to_string(),
            "list": list,
            "title": parsed.title,
            "note": parsed.note,
            "created": now,
            "pos": now,
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
    })
    .await?;
    println!("✓ added to {dest}: {title}");
    Ok(())
}

async fn list(project: Option<String>, all: bool) -> Result<()> {
    let data = store::load(&NullSink).await?;
    let now = now_ms();
    let target = match &project {
        Some(p) => resolve_list(&data, p)?,
        None => "inbox".to_string(),
    };

    let arr = data["tasks"].as_array().cloned().unwrap_or_default();
    let mut rows: Vec<&Value> = arr
        .iter()
        .filter(|t| {
            if t.get("list").and_then(|v| v.as_str()).unwrap_or("inbox") != target {
                return false;
            }
            let done = t.get("doneAt").and_then(|v| v.as_i64()).is_some();
            if done {
                return all;
            }
            let deferred = t
                .get("deferUntil")
                .and_then(|v| v.as_i64())
                .is_some_and(|d| d > now);
            if deferred {
                return all;
            }
            true
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
                "(no tasks in this list)"
            } else {
                "(no open tasks — use --all to include done + deferred)"
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
        let due_str = if due.is_empty() { String::new() } else { format!(" [{due}]") };
        let mark = if done { "✓" } else { "·" };
        let ctx_str = if ctx.is_empty() { String::new() } else { format!(" {ctx}") };
        println!("{mark} {id_short}  {title}{ctx_str}{due_str}");
    }
    Ok(())
}

async fn done(id: &str) -> Result<()> {
    let mut matched = false;
    with_store(|data| {
        let arr = data["tasks"].as_array_mut().context("tasks missing")?;
        for t in arr.iter_mut() {
            let tid = t.get("id").and_then(|v| v.as_str()).unwrap_or("");
            if tid == id || tid.starts_with(id) {
                t["doneAt"] = Value::from(now_ms());
                if t.get("deferUntil").is_some() {
                    t.as_object_mut().unwrap().remove("deferUntil");
                }
                matched = true;
                break;
            }
        }
        Ok(())
    })
    .await?;
    if matched {
        println!("✓ done");
    } else {
        println!("no task matching id '{id}'");
    }
    Ok(())
}

async fn defer(id: &str, when: &str) -> Result<()> {
    let Some(ts) = parse_when(when) else {
        anyhow::bail!("couldn't parse `{when}`. try: today, tomorrow, mon..sun, +3d, +1w, YYYY-MM-DD");
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
    })
    .await?;
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
            if !c.is_empty() {
                ctx = Some(format!("@{c}"));
            }
        } else if tok.starts_with('#') {
            // projects from the CLI are deferred — dropped for now
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
    QuickAdd { title: title_parts.join(" "), ctx, due, note }
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
