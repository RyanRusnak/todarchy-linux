// model.rs — the task/project data model plus the pure logic ported from
// the old React frontend (src/ui/data.jsx and the view-building memo in
// app.jsx). Everything here is UI-agnostic: filtering, the depth-first
// tree walk, the sort comparator, and the natural-language parsers. Keeping
// them as free functions makes them trivially unit-testable and mirrors the
// macOS/iOS implementations byte-for-byte where the comment says so.

use chrono::{Datelike, Duration, Local, NaiveDate, TimeZone};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// A single task. Known fields are typed; anything else the sync layer or
/// the Apple apps write (comments, wasDeferred, custom keys) rides along in
/// `rest` so a round-trip through the TUI never drops data.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    #[serde(default)]
    pub list: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub ctx: String,
    #[serde(default)]
    pub due: String, // "" | "today" | "tomorrow" | "this week"
    #[serde(default)]
    pub note: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    #[serde(default)]
    pub created: i64,
    #[serde(rename = "doneAt", default, skip_serializing_if = "Option::is_none")]
    pub done_at: Option<i64>,
    #[serde(rename = "deferUntil", default, skip_serializing_if = "Option::is_none")]
    pub defer_until: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pos: Option<f64>,
    #[serde(flatten)]
    pub rest: Map<String, Value>,
}

impl Task {
    pub fn is_done(&self) -> bool {
        self.done_at.is_some()
    }
    pub fn is_deferred(&self, now: i64) -> bool {
        self.defer_until.map(|d| d > now).unwrap_or(false)
    }
    /// Display-order key: explicit `pos`, else the creation timestamp — so
    /// rows default to newest-first and Shift-J/K edits `pos` to reorder
    /// without touching `created`.
    pub fn order_key(&self) -> f64 {
        self.pos.unwrap_or(self.created as f64)
    }
}

/// A user list beyond the built-in inbox.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default = "default_icon")]
    pub icon: String,
    #[serde(default)]
    pub accent: String,
    #[serde(flatten)]
    pub rest: Map<String, Value>,
}

fn default_icon() -> String {
    "folder".into()
}

impl Project {
    pub fn is_shared(&self) -> bool {
        self.rest
            .get("isShared")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }
}

/// The whole store as the TUI holds it in memory.
#[derive(Clone, Debug, Default)]
pub struct Store {
    pub tasks: Vec<Task>,
    pub projects: Vec<Project>,
    pub contexts: Vec<String>,
}

impl Store {
    /// Parse the JSON projection `todarchy_core::store` hands us.
    pub fn from_json(v: &Value) -> Self {
        let tasks = v
            .get("tasks")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|t| serde_json::from_value(t.clone()).ok()).collect())
            .unwrap_or_default();
        let projects = v
            .get("projects")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|p| serde_json::from_value(p.clone()).ok()).collect())
            .unwrap_or_default();
        let contexts = v
            .get("contexts")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|c| c.as_str().map(str::to_string)).collect())
            .unwrap_or_else(default_contexts);
        Store { tasks, projects, contexts }
    }

    pub fn to_json(&self) -> Value {
        serde_json::json!({
            "tasks": self.tasks,
            "projects": self.projects,
            "contexts": self.contexts,
        })
    }
}

pub fn default_contexts() -> Vec<String> {
    ["@home", "@work", "@errands", "@mac", "@phone", "@read"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// One rendered row of the flattened tree, tagging depth + child state.
#[derive(Clone, Debug)]
pub struct ViewRow {
    pub id: String,
    pub depth: u16,
    pub has_children: bool,
    pub collapsed: bool,
}

pub fn now_ms() -> i64 {
    Local::now().timestamp_millis()
}

/// Build the visible, depth-first ordered row list for a given list/view.
/// This is a faithful port of the `viewTasks` memo in app.jsx (lines
/// 399-460): filter → children index → sort comparator → DFS walk that
/// promotes visible descendants of filtered-out parents.
#[allow(clippy::too_many_arguments)]
pub fn build_view(
    tasks: &[Task],
    active_list: &str,
    search: &str,
    ctx_filter: &str,
    show_done: bool,
    show_deferred: bool,
    limit_to_first: bool,
    collapsed: &std::collections::HashSet<String>,
) -> Vec<ViewRow> {
    let now = now_ms();
    let needle = search.trim().to_lowercase();

    let in_list: Vec<&Task> = tasks.iter().filter(|t| t.list == active_list).collect();
    let has = |id: &str| in_list.iter().any(|t| t.id == id);

    let visible = |t: &Task| -> bool {
        if t.is_done() && !show_done {
            return false;
        }
        if t.is_deferred(now) && !show_deferred {
            return false;
        }
        if !ctx_filter.is_empty() && t.ctx != ctx_filter {
            return false;
        }
        if !needle.is_empty() {
            let hay = format!("{} {} {}", t.title, t.ctx, t.note).to_lowercase();
            if !hay.contains(&needle) {
                return false;
            }
        }
        true
    };

    // children index: parent id (or "" for root) -> child task ids
    use std::collections::HashMap;
    let mut children: HashMap<String, Vec<&Task>> = HashMap::new();
    for t in &in_list {
        let p = match &t.parent {
            Some(p) if has(p) => p.clone(),
            _ => String::new(),
        };
        children.entry(p).or_default().push(t);
    }

    let status_rank = |t: &Task| -> i32 {
        if t.is_done() {
            2
        } else if t.is_deferred(now) {
            1
        } else {
            0
        }
    };
    let due_rank = |d: &str| -> i32 {
        match d {
            "today" => 0,
            "tomorrow" => 1,
            "this week" => 2,
            _ => 3,
        }
    };
    let cmp = |a: &&Task, b: &&Task| -> std::cmp::Ordering {
        use std::cmp::Ordering;
        status_rank(a)
            .cmp(&status_rank(b))
            .then_with(|| due_rank(&a.due).cmp(&due_rank(&b.due)))
            .then_with(|| b.done_at.unwrap_or(0).cmp(&a.done_at.unwrap_or(0)))
            .then_with(|| b.order_key().partial_cmp(&a.order_key()).unwrap_or(Ordering::Equal))
    };

    let mut out: Vec<ViewRow> = Vec::new();
    // recursive DFS via explicit stack to avoid closure recursion pain.
    fn walk<'a>(
        parent: &str,
        depth: u16,
        children: &HashMap<String, Vec<&'a Task>>,
        cmp: &dyn Fn(&&'a Task, &&'a Task) -> std::cmp::Ordering,
        visible: &dyn Fn(&Task) -> bool,
        collapsed: &std::collections::HashSet<String>,
        out: &mut Vec<ViewRow>,
    ) {
        let mut kids: Vec<&'a Task> = children.get(parent).cloned().unwrap_or_default();
        kids.sort_by(cmp);
        for t in kids {
            let has_children = children.get(&t.id).map(|v| !v.is_empty()).unwrap_or(false);
            let is_collapsed = collapsed.contains(&t.id);
            if visible(t) {
                out.push(ViewRow {
                    id: t.id.clone(),
                    depth,
                    has_children,
                    collapsed: is_collapsed,
                });
                if has_children && !is_collapsed {
                    walk(&t.id, depth + 1, children, cmp, visible, collapsed, out);
                }
            } else if has_children {
                // parent filtered out — promote visible descendants
                walk(&t.id, depth, children, cmp, visible, collapsed, out);
            }
        }
    }
    walk("", 0, &children, &cmp, &visible, collapsed, &mut out);

    if limit_to_first {
        out.truncate(1);
    }
    out
}

// ---------- quick-add parsing (data.jsx parseQuickAdd) ----------

#[derive(Debug, Default, PartialEq)]
pub struct QuickAdd {
    pub title: String,
    pub ctx: String,
    pub due: String,
    pub note: String,
}

/// Parse "read @read !today / note here" into its parts. Mirrors the JS
/// regex behaviour: first `@word` is the context, `!(today|tomorrow|week)`
/// is the due date (week → "this week"), and ` /rest` is the note.
pub fn parse_quick_add(raw: &str) -> QuickAdd {
    let mut title = raw.to_string();
    let mut ctx = String::new();
    let mut due = String::new();
    let mut note = String::new();

    // context: first @ followed by word chars
    if let Some(at) = title.find('@') {
        let bytes = title.as_bytes();
        let mut end = at + 1;
        while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
            end += 1;
        }
        if end > at + 1 {
            ctx = title[at..end].to_string();
            title.replace_range(at..end, "");
        }
    }

    // due: !today | !tomorrow | !week (case-insensitive)
    let lower = title.to_lowercase();
    for (tok, val) in [("!today", "today"), ("!tomorrow", "tomorrow"), ("!week", "this week")] {
        if let Some(pos) = lower.find(tok) {
            due = val.to_string();
            title.replace_range(pos..pos + tok.len(), "");
            break;
        }
    }

    // note: whitespace then '/' then everything to end
    if let Some(slash) = find_note_slash(&title) {
        note = title[slash + 1..].trim().to_string();
        title.truncate(slash - 1); // drop the leading whitespace + slash
    }

    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    QuickAdd { title, ctx, due, note }
}

/// Index of the `/` that starts an inline note (must be preceded by
/// whitespace), matching the JS `\s\/(.+)$` anchor.
fn find_note_slash(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    for i in 1..bytes.len() {
        if bytes[i] == b'/' && bytes[i - 1].is_ascii_whitespace() && i + 1 < bytes.len() {
            return Some(i);
        }
    }
    None
}

// ---------- natural-language defer parsing (data.jsx parseDeferText) ----------

fn defer_at_nine(date: NaiveDate) -> i64 {
    let dt = date.and_hms_opt(9, 0, 0).unwrap();
    Local.from_local_datetime(&dt).earliest().unwrap().timestamp_millis()
}

/// The next occurrence of a weekday, strictly ahead of today, at 09:00.
fn defer_next_dow(target: u32) -> i64 {
    let today = Local::now().date_naive();
    let cur = today.weekday().num_days_from_sunday();
    let mut delta = (target as i64 - cur as i64 + 7) % 7;
    if delta == 0 {
        delta = 7;
    }
    defer_at_nine(today + Duration::days(delta))
}

/// Parse a defer expression into an absolute ms timestamp at 09:00 local,
/// or None if unrecognized. Accepts: today · tomorrow/tmrw · weekend/this
/// weekend · next week · +Nd/+Nw/+Nm · mon..sun · YYYY-MM-DD.
pub fn parse_defer_text(input: &str) -> Option<i64> {
    let s = input.trim().to_lowercase();
    if s.is_empty() {
        return None;
    }
    let today = Local::now().date_naive();

    match s.as_str() {
        "today" => return Some(defer_at_nine(today)),
        "tomorrow" | "tmrw" => return Some(defer_at_nine(today + Duration::days(1))),
        "weekend" | "this weekend" => return Some(defer_next_dow(6)), // saturday
        "next week" => return Some(defer_next_dow(1)),                // monday
        _ => {}
    }

    // +Nd / +Nw / +Nm
    if let Some(rest) = s.strip_prefix('+') {
        let rest = rest.trim();
        if let Some(unit) = rest.chars().last() {
            if matches!(unit, 'd' | 'w' | 'm') {
                if let Ok(n) = rest[..rest.len() - 1].trim().parse::<i64>() {
                    let date = match unit {
                        'd' => today + Duration::days(n),
                        'w' => today + Duration::days(n * 7),
                        _ => add_months(today, n),
                    };
                    return Some(defer_at_nine(date));
                }
            }
        }
    }

    // weekday abbreviation
    let dow = match s.as_str() {
        "sun" => Some(0),
        "mon" => Some(1),
        "tue" => Some(2),
        "wed" => Some(3),
        "thu" => Some(4),
        "fri" => Some(5),
        "sat" => Some(6),
        _ => None,
    };
    if let Some(d) = dow {
        return Some(defer_next_dow(d));
    }

    // ISO date YYYY-MM-DD (from_ymd_opt already rejects impossible dates)
    if let Some((y, m, d)) = parse_iso(&s) {
        if let Some(date) = NaiveDate::from_ymd_opt(y, m, d) {
            return Some(defer_at_nine(date));
        }
    }

    None
}

fn add_months(date: NaiveDate, n: i64) -> NaiveDate {
    if n >= 0 {
        date.checked_add_months(chrono::Months::new(n as u32)).unwrap_or(date)
    } else {
        date.checked_sub_months(chrono::Months::new((-n) as u32)).unwrap_or(date)
    }
}

fn parse_iso(s: &str) -> Option<(i32, u32, u32)> {
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() != 3 || parts[0].len() != 4 || parts[1].len() != 2 || parts[2].len() != 2 {
        return None;
    }
    Some((parts[0].parse().ok()?, parts[1].parse().ok()?, parts[2].parse().ok()?))
}

// ---------- humanized labels ----------

/// Short humanized defer label, e.g. "today 09:00", "fri 09:00", "mar 3 09:00".
pub fn format_defer_until(ts: i64) -> String {
    let d = match Local.timestamp_millis_opt(ts).single() {
        Some(d) => d,
        None => return String::new(),
    };
    let now = Local::now();
    let today = now.date_naive();
    let t = d.format("%H:%M");
    let dd = d.date_naive();
    if dd == today {
        return format!("today {t}");
    }
    if dd == today + Duration::days(1) {
        return format!("tomorrow {t}");
    }
    let diff = (dd - today).num_days();
    if diff > 1 && diff < 7 {
        return format!("{} {t}", d.format("%a").to_string().to_lowercase());
    }
    format!("{} {t}", d.format("%b %-d").to_string().to_lowercase())
}

/// A comment flattened for display.
pub struct CommentView {
    pub author: String,
    pub text: String,
    pub created: i64,
}

/// Read a task's `comments` object (`{ id: {author, text, createdAt} }`) into
/// a list sorted oldest-first.
pub fn task_comments(t: &Task) -> Vec<CommentView> {
    let mut out = Vec::new();
    if let Some(obj) = t.rest.get("comments").and_then(|v| v.as_object()) {
        for c in obj.values() {
            out.push(CommentView {
                author: c.get("author").and_then(|v| v.as_str()).unwrap_or("anon").to_string(),
                text: c.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                created: c.get("createdAt").and_then(|v| v.as_i64()).unwrap_or(0),
            });
        }
    }
    out.sort_by_key(|c| c.created);
    out
}

pub fn time_ago(ts: i64) -> String {
    let s = (now_ms() - ts) / 1000;
    if s < 60 {
        return format!("{s}s");
    }
    let m = s / 60;
    if m < 60 {
        return format!("{m}m");
    }
    let h = m / 60;
    if h < 24 {
        return format!("{h}h");
    }
    format!("{}d", h / 24)
}

/// Generate an RFC 4122 v4 UUID so Apple platforms can decode the id
/// (matches nid() in data.jsx). Uses the OS RNG via getrandom through
/// core's `rand`? We avoid a new dep by pulling 16 bytes from a small
/// splitmix seeded by the clock — but ids must be unique and parse as
/// UUIDs, so we use the system time + a process-local counter mixed in.
pub fn new_uuid() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut seed = now_ms() as u64;
    seed ^= COUNTER.fetch_add(0x9E3779B97F4A7C15, Ordering::Relaxed);
    seed ^= std::process::id() as u64;
    let mut bytes = [0u8; 16];
    let mut x = seed | 1;
    for chunk in bytes.chunks_mut(8) {
        // splitmix64
        x = x.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = x;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^= z >> 31;
        let n = chunk.len();
        chunk.copy_from_slice(&z.to_le_bytes()[..n]);
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x40; // version 4
    bytes[8] = (bytes[8] & 0x3f) | 0x80; // variant
    let h: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    )
}

/// Export the store as a Markdown checklist grouped by list. Mirrors
/// data.jsx exportMarkdown / macOS ExportImport.exportMarkdown.
pub fn export_markdown(store: &Store) -> String {
    let mut lines: Vec<String> = vec![
        format!("# todarchy — {}", Local::now().to_rfc3339()),
        String::new(),
    ];
    let heading = |list_id: &str| -> String {
        if list_id == "inbox" {
            "inbox".into()
        } else {
            store
                .projects
                .iter()
                .find(|p| p.id == list_id)
                .map(|p| p.name.clone())
                .unwrap_or_else(|| list_id.to_string())
        }
    };
    let mut list_ids = vec!["inbox".to_string()];
    list_ids.extend(store.projects.iter().map(|p| p.id.clone()));
    for list_id in list_ids {
        let for_list: Vec<&Task> = store.tasks.iter().filter(|t| t.list == list_id).collect();
        if for_list.is_empty() {
            continue;
        }
        lines.push(format!("## {}", heading(&list_id)));
        lines.push(String::new());
        for task in for_list {
            let box_ = if task.is_done() { "[x]" } else { "[ ]" };
            let mut title = task.title.clone();
            if !task.ctx.is_empty() {
                title.push_str(&format!(" {}", task.ctx));
            }
            if !task.due.is_empty() {
                let d = if task.due == "this week" { "week" } else { &task.due };
                title.push_str(&format!(" !{d}"));
            }
            lines.push(format!("- {box_} {title}"));
            if !task.note.is_empty() {
                for line in task.note.split('\n') {
                    lines.push(format!("  > {line}"));
                }
            }
        }
        lines.push(String::new());
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Timelike;

    #[test]
    fn quick_add_parses_ctx_due_note() {
        let q = parse_quick_add("call dad @phone !today / ask about trip");
        assert_eq!(q.title, "call dad");
        assert_eq!(q.ctx, "@phone");
        assert_eq!(q.due, "today");
        assert_eq!(q.note, "ask about trip");
    }

    #[test]
    fn quick_add_week_maps_to_this_week() {
        let q = parse_quick_add("plan @work !week");
        assert_eq!(q.due, "this week");
        assert_eq!(q.title, "plan");
    }

    #[test]
    fn defer_relative_and_literals() {
        assert!(parse_defer_text("tomorrow").is_some());
        assert!(parse_defer_text("+3d").is_some());
        assert!(parse_defer_text("+1w").is_some());
        assert!(parse_defer_text("fri").is_some());
        assert!(parse_defer_text("2099-12-31").is_some());
        assert!(parse_defer_text("2025-02-31").is_none());
        assert!(parse_defer_text("gibberish").is_none());
    }

    #[test]
    fn defer_is_nine_am_local() {
        let ts = parse_defer_text("tomorrow").unwrap();
        let d = Local.timestamp_millis_opt(ts).single().unwrap();
        assert_eq!(d.hour(), 9);
        assert_eq!(d.minute(), 0);
    }

    #[test]
    fn view_promotes_descendants_of_hidden_parent() {
        let now = now_ms();
        let tasks = vec![
            Task {
                id: "p".into(),
                list: "inbox".into(),
                title: "parent".into(),
                done_at: Some(now), // hidden when show_done=false
                created: now,
                ..blank()
            },
            Task {
                id: "c".into(),
                list: "inbox".into(),
                title: "child".into(),
                parent: Some("p".into()),
                created: now,
                ..blank()
            },
        ];
        let rows = build_view(&tasks, "inbox", "", "", false, false, false, &Default::default());
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "c");
        assert_eq!(rows[0].depth, 0); // promoted to parent's depth
    }

    fn blank() -> Task {
        Task {
            id: String::new(),
            list: String::new(),
            title: String::new(),
            ctx: String::new(),
            due: String::new(),
            note: String::new(),
            parent: None,
            created: 0,
            done_at: None,
            defer_until: None,
            pos: None,
            rest: Map::new(),
        }
    }
}
