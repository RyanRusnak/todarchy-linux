// app.rs — the TUI's state machine. Holds the in-memory store plus all the
// interaction state (cursor, view mode, filters, prompts, palette) and
// implements every mutation and key binding. It never touches the terminal
// (that's ui.rs) or does I/O (that's main.rs, which owns the async runtime
// and the todarchy-core store calls). Mutations mark `dirty`; the event
// loop persists via `store::save`. Deletes and sync/share operations are
// returned as `AsyncCmd`s for the loop to run off the UI path.

use std::collections::HashSet;
use std::time::Instant;

use todarchy_core::SyncStatus;

use crate::model::{
    self, build_view, now_ms, parse_defer_text, parse_quick_add, Project, Store, Task, ViewRow,
};

/// A text-entry prompt shown on the bottom line.
#[derive(Clone, Debug)]
pub struct Prompt {
    pub kind: PromptKind,
    pub buf: String,
    pub err: Option<String>,
}

#[derive(Clone, Debug)]
pub enum PromptKind {
    Add,
    Edit(String),
    Search,
    Defer(String),
    AcceptLink,
    NewProject,
    RenameProject(String),
    NewContext,
    RenameContext(String),
    CommentAuthor,
    ExportJson,
    ExportMd,
    ImportJson,
}

impl PromptKind {
    pub fn label(&self) -> &'static str {
        match self {
            PromptKind::Add => "add",
            PromptKind::Edit(_) => "edit",
            PromptKind::Search => "search",
            PromptKind::Defer(_) => "defer",
            PromptKind::AcceptLink => "share link",
            PromptKind::NewProject => "new project name",
            PromptKind::RenameProject(_) => "rename project",
            PromptKind::NewContext => "new @context",
            PromptKind::RenameContext(_) => "rename context",
            PromptKind::CommentAuthor => "comment author",
            PromptKind::ExportJson => "export json → path",
            PromptKind::ExportMd => "export md → path",
            PromptKind::ImportJson => "import json ← path",
        }
    }
}

/// The command palette overlay state.
#[derive(Default)]
pub struct Palette {
    pub q: String,
    pub sel: usize,
}

/// A palette command.
pub struct Command {
    pub id: String,
    pub title: String,
    pub hint: String,
    pub keys: Vec<String>,
    pub action: Action,
}

/// Synchronous view/store mutations, plus `Async` for anything that needs
/// the runtime (network, tombstoning deletes, clipboard).
#[derive(Clone, Debug)]
pub enum Action {
    GoList(String),
    CycleMode,
    ToggleDone,
    ToggleDeferred,
    OpenAdd,
    Complete,
    Indent,
    Outdent,
    ReorderUp,
    ReorderDown,
    Collapse,
    MoveToList(String),
    OpenDefer,
    DeferAt(i64),
    DeferClear,
    Delete,
    OpenEdit,
    OpenEditNote,
    AddComment,
    Undo,
    OpenSearch,
    SetDue(String),
    SetContext(String),
    CtxFilter(String),
    CtxClear,
    DeleteContext(String),
    ToggleDetail,
    ClearDone,
    EditConfig,
    OpenPrompt(PromptKind),
    CopyText(String),
    Async(AsyncCmd),
    Quit,
}

/// Operations the event loop runs off the UI thread.
#[derive(Clone, Debug)]
pub enum AsyncCmd {
    DeleteTasks(Vec<String>),
    DeleteProject(String),
    CheckServer,
    Promote(String),
    Accept(String),
    Leave(String),
}

pub struct App {
    pub store: Store,
    pub active_list: String,
    pub cursor: usize,
    pub show_done: bool,
    pub show_deferred: bool,
    pub limit_to_first: bool,
    pub ctx_filter: String,
    pub search: String,
    pub collapsed: HashSet<String>,
    pub undo_stack: Vec<Vec<Task>>,
    pub show_detail: bool,
    pub prompt: Option<Prompt>,
    pub palette: Option<Palette>,
    pub toast: Option<(Instant, String)>,
    pub sync: SyncStatus,
    pub pending_chord: Option<(char, Instant)>,
    pub dirty: bool,
    pub quit: bool,
    /// Detail-pane note scroll offset (lines), and the task it belongs to so
    /// we can zero it when the selection changes. `detail_max_scroll` is set
    /// by the renderer (which knows the wrapped line count + viewport).
    pub detail_scroll: u16,
    pub detail_anchor: Option<String>,
    pub detail_max_scroll: std::cell::Cell<u16>,
    /// Set by a key/palette action; drained by the event loop to launch $EDITOR.
    pub editor_request: Option<EditorRequest>,
}

impl App {
    pub fn new(store: Store, sync: SyncStatus) -> Self {
        App {
            store,
            active_list: "inbox".into(),
            cursor: 0,
            show_done: false,
            show_deferred: false,
            limit_to_first: false,
            ctx_filter: String::new(),
            search: String::new(),
            collapsed: HashSet::new(),
            undo_stack: Vec::new(),
            show_detail: true,
            prompt: None,
            palette: None,
            toast: None,
            sync,
            pending_chord: None,
            dirty: false,
            quit: false,
            detail_scroll: 0,
            detail_anchor: None,
            detail_max_scroll: std::cell::Cell::new(0),
            editor_request: None,
        }
    }

    /// Replace a task's note (called by the event loop after $EDITOR returns).
    pub fn set_note(&mut self, task_id: &str, content: &str) {
        let trimmed = content.trim_end_matches('\n').to_string();
        let changed = self.task(task_id).map(|t| t.note != trimmed).unwrap_or(false);
        if !changed {
            return;
        }
        self.push_undo();
        if let Some(t) = self.task_mut(task_id) {
            t.note = trimmed;
        }
        self.dirty = true;
        self.toast("note saved");
    }

    /// Append a comment to a task, author-stamped + timestamped, matching the
    /// Apple/React `comments` object shape: { id: {id, author, text, createdAt} }.
    pub fn add_comment(&mut self, task_id: &str, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            self.toast("empty — no comment added");
            return;
        }
        self.push_undo();
        let id = model::new_uuid();
        let comment = serde_json::json!({
            "id": id,
            "author": comment_author(),
            "text": text,
            "createdAt": now_ms(),
        });
        if let Some(t) = self.task_mut(task_id) {
            let entry = t
                .rest
                .entry("comments".to_string())
                .or_insert_with(|| serde_json::json!({}));
            if !entry.is_object() {
                *entry = serde_json::json!({});
            }
            if let Some(obj) = entry.as_object_mut() {
                obj.insert(id, comment);
            }
        }
        self.dirty = true;
        self.toast("comment added");
    }

    /// Zero the note scroll when the selected task changes (called once per
    /// event-loop iteration).
    pub fn reconcile_detail(&mut self) {
        let cur = self.current_id();
        if cur != self.detail_anchor {
            self.detail_anchor = cur;
            self.detail_scroll = 0;
        }
    }

    /// Scroll the detail note, clamped to the max the renderer last measured.
    pub fn scroll_detail(&mut self, delta: i32) {
        let max = self.detail_max_scroll.get() as i32;
        let next = (self.detail_scroll as i32 + delta).clamp(0, max);
        self.detail_scroll = next as u16;
    }

    // ---------- derived views ----------

    pub fn lists(&self) -> Vec<String> {
        let mut v = vec!["inbox".to_string()];
        v.extend(self.store.projects.iter().map(|p| p.id.clone()));
        v
    }

    pub fn list_label(&self, id: &str) -> String {
        if id == "inbox" {
            "inbox".into()
        } else {
            self.store
                .projects
                .iter()
                .find(|p| p.id == id)
                .map(|p| p.name.clone())
                .unwrap_or_else(|| id.to_string())
        }
    }

    pub fn mode_label(&self) -> &'static str {
        if self.limit_to_first {
            "next"
        } else if self.show_done && self.show_deferred {
            "all"
        } else {
            "todo"
        }
    }

    pub fn view(&self) -> Vec<ViewRow> {
        build_view(
            &self.store.tasks,
            &self.active_list,
            &self.search,
            &self.ctx_filter,
            self.show_done,
            self.show_deferred,
            self.limit_to_first,
            &self.collapsed,
        )
    }

    pub fn current_id(&self) -> Option<String> {
        self.view().get(self.cursor).map(|r| r.id.clone())
    }

    pub fn task(&self, id: &str) -> Option<&Task> {
        self.store.tasks.iter().find(|t| t.id == id)
    }

    /// The palette index for a context (its position in the contexts list; a
    /// stable hash fallback for contexts not in the list, e.g. from a peer).
    pub fn context_index(&self, ctx: &str) -> usize {
        self.store
            .contexts
            .iter()
            .position(|c| c == ctx)
            .unwrap_or_else(|| self.store.contexts.len() + ctx.bytes().map(|b| b as usize).sum::<usize>())
    }

    fn task_mut(&mut self, id: &str) -> Option<&mut Task> {
        self.store.tasks.iter_mut().find(|t| t.id == id)
    }

    fn clamp_cursor(&mut self) {
        let n = self.view().len();
        if n == 0 {
            self.cursor = 0;
        } else if self.cursor >= n {
            self.cursor = n - 1;
        }
    }

    pub fn toast<S: Into<String>>(&mut self, msg: S) {
        self.toast = Some((Instant::now(), msg.into()));
    }

    /// Optimistically drop tasks from the in-memory store so the UI updates
    /// instantly; the tombstoning delete_many runs in the background. Snapshots
    /// first so `u` restores the deleted task(s).
    pub fn remove_tasks_local(&mut self, ids: &[String]) {
        self.push_undo();
        self.store.tasks.retain(|t| !ids.contains(&t.id));
        self.clamp_cursor();
    }

    /// Optimistically drop a project locally (its tasks stay put).
    pub fn remove_project_local(&mut self, id: &str) {
        self.store.projects.retain(|p| p.id != id);
        if self.active_list == id {
            self.active_list = "inbox".into();
            self.cursor = 0;
        }
        self.clamp_cursor();
    }

    /// Copy text to the clipboard and toast it — used for the share link
    /// returned by promote and the server doc id.
    pub fn copy_and_toast(&mut self, text: &str) {
        spawn_copy(text);
        self.toast(format!("link copied · {}", &text.chars().take(28).collect::<String>()));
    }

    /// Replace the store from a fresh JSON projection (a sync peer wrote, or
    /// an async command just mutated disk), keeping the cursor in range.
    pub fn adopt(&mut self, v: &serde_json::Value) {
        self.store = Store::from_json(v);
        // active_list may have vanished (project deleted on a peer) — fall
        // back to inbox so the view is never empty-of-list.
        if self.active_list != "inbox" && !self.store.projects.iter().any(|p| p.id == self.active_list) {
            self.active_list = "inbox".into();
        }
        self.clamp_cursor();
    }

    fn push_undo(&mut self) {
        self.undo_stack.push(self.store.tasks.clone());
        let len = self.undo_stack.len();
        if len > 20 {
            self.undo_stack.drain(0..len - 20);
        }
    }

    // ---------- tick (auto-surface deferred) ----------

    pub fn on_tick(&mut self) {
        let now = now_ms();
        let mut back = 0;
        for t in self.store.tasks.iter_mut() {
            if let Some(d) = t.defer_until {
                if d <= now {
                    t.defer_until = None;
                    t.rest.insert("wasDeferred".into(), serde_json::Value::Bool(true));
                    back += 1;
                }
            }
        }
        if back > 0 {
            self.dirty = true;
            self.toast(if back == 1 {
                "1 task is back".to_string()
            } else {
                format!("{back} tasks are back")
            });
        }
        // let stale prompt errors / toasts age out naturally in ui.rs
    }

    // ---------- key handling ----------

    /// Handle one key press. Returns an async command if the action needs
    /// the runtime; otherwise mutates in place and returns None.
    pub fn on_key(&mut self, key: crossterm::event::KeyEvent) -> Option<AsyncCmd> {
        use crossterm::event::{KeyCode, KeyModifiers};

        // Ctrl-C always quits.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return None;
        }

        if self.palette.is_some() {
            return self.palette_key(key);
        }
        if self.prompt.is_some() {
            return self.prompt_key(key);
        }

        // Ctrl-K opens the palette from anywhere.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('k') {
            self.palette = Some(Palette::default());
            return None;
        }

        self.normal_key(key)
    }

    fn normal_key(&mut self, key: crossterm::event::KeyEvent) -> Option<AsyncCmd> {
        use crossterm::event::{KeyCode, KeyModifiers};

        // chord staleness (700ms)
        if let Some((_, at)) = self.pending_chord {
            if at.elapsed().as_millis() > 700 {
                self.pending_chord = None;
            }
        }

        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        // Ctrl-d / Ctrl-u scroll the detail-pane note (half-page).
        if ctrl {
            match key.code {
                KeyCode::Char('d') => {
                    self.scroll_detail(6);
                    return None;
                }
                KeyCode::Char('u') => {
                    self.scroll_detail(-6);
                    return None;
                }
                _ => {}
            }
        }

        // resolve a pending prefix chord
        if let Some((prefix, _)) = self.pending_chord {
            self.pending_chord = None;
            return self.resolve_chord(prefix, key.code);
        }

        match key.code {
            KeyCode::Char('g') | KeyCode::Char('m') | KeyCode::Char('f') => {
                if let KeyCode::Char(c) = key.code {
                    self.pending_chord = Some((c, Instant::now()));
                }
                None
            }
            KeyCode::Char('j') => {
                self.move_cursor(1);
                None
            }
            KeyCode::Char('k') => {
                self.move_cursor(-1);
                None
            }
            KeyCode::Down if shift => self.apply(Action::ReorderDown),
            KeyCode::Up if shift => self.apply(Action::ReorderUp),
            KeyCode::Down => {
                self.move_cursor(1);
                None
            }
            KeyCode::Up => {
                self.move_cursor(-1);
                None
            }
            KeyCode::Char('J') => self.apply(Action::ReorderDown),
            KeyCode::Char('K') => self.apply(Action::ReorderUp),
            KeyCode::Char('G') => {
                let n = self.view().len();
                self.cursor = n.saturating_sub(1);
                None
            }
            KeyCode::Char('h') | KeyCode::Left => {
                self.shift_list(-1);
                None
            }
            KeyCode::Char('l') | KeyCode::Right => {
                self.shift_list(1);
                None
            }
            KeyCode::Char('0') => self.apply(Action::GoList("inbox".into())),
            KeyCode::Char(c @ '1'..='5') => {
                let idx = c as usize - '1' as usize;
                if let Some(p) = self.store.projects.get(idx) {
                    let id = p.id.clone();
                    self.apply(Action::GoList(id))
                } else {
                    None
                }
            }
            KeyCode::Char('o') | KeyCode::Char('a') | KeyCode::Enter => self.apply(Action::OpenAdd),
            KeyCode::Char('x') | KeyCode::Char(' ') => self.apply(Action::Complete),
            KeyCode::Char('v') => self.apply(Action::CycleMode),
            KeyCode::Char('e') => self.apply(Action::OpenEdit),
            KeyCode::Char('c') => self.apply(Action::OpenEditNote),
            KeyCode::Char('C') => self.apply(Action::AddComment),
            KeyCode::Char('d') => self.apply(Action::OpenDefer),
            KeyCode::Delete | KeyCode::Backspace => self.apply(Action::Delete),
            KeyCode::Char('u') => self.apply(Action::Undo),
            KeyCode::Char('/') => self.apply(Action::OpenSearch),
            KeyCode::Char(':') | KeyCode::Char('?') => {
                self.palette = Some(Palette::default());
                None
            }
            KeyCode::Char('i') => self.apply(Action::ToggleDetail),
            KeyCode::Tab => self.apply(Action::Indent),
            KeyCode::BackTab => self.apply(Action::Outdent),
            KeyCode::Char('z') => self.apply(Action::Collapse),
            KeyCode::Char('q') => {
                self.quit = true;
                None
            }
            KeyCode::Esc => {
                self.search.clear();
                self.ctx_filter.clear();
                self.clamp_cursor();
                None
            }
            _ => None,
        }
    }

    fn resolve_chord(&mut self, prefix: char, code: crossterm::event::KeyCode) -> Option<AsyncCmd> {
        use crossterm::event::KeyCode;
        match (prefix, code) {
            ('g', KeyCode::Char('g')) => {
                self.cursor = 0;
                None
            }
            ('g', KeyCode::Char('i')) => self.apply(Action::GoList("inbox".into())),
            ('g', KeyCode::Char(c @ '1'..='5')) => {
                let idx = c as usize - '1' as usize;
                if let Some(p) = self.store.projects.get(idx) {
                    let id = p.id.clone();
                    self.apply(Action::GoList(id))
                } else {
                    None
                }
            }
            ('g', KeyCode::Char('n')) => self.apply(Action::OpenPrompt(PromptKind::NewProject)),
            ('m', KeyCode::Char('i')) => self.apply(Action::MoveToList("inbox".into())),
            ('m', KeyCode::Char(c @ '1'..='5')) => {
                let idx = c as usize - '1' as usize;
                if let Some(p) = self.store.projects.get(idx) {
                    let id = p.id.clone();
                    self.apply(Action::MoveToList(id))
                } else {
                    None
                }
            }
            ('f', KeyCode::Char('d')) => self.apply(Action::ToggleDone),
            ('f', KeyCode::Char('s')) => self.apply(Action::ToggleDeferred),
            _ => None,
        }
    }

    fn move_cursor(&mut self, delta: i64) {
        let n = self.view().len() as i64;
        if n == 0 {
            self.cursor = 0;
            return;
        }
        let c = (self.cursor as i64 + delta).clamp(0, n - 1);
        self.cursor = c as usize;
    }

    fn shift_list(&mut self, delta: i64) {
        let lists = self.lists();
        let idx = lists.iter().position(|l| l == &self.active_list).unwrap_or(0) as i64;
        let n = lists.len() as i64;
        let new = (idx + delta).clamp(0, n - 1) as usize;
        self.active_list = lists[new].clone();
        self.cursor = 0;
    }

    // ---------- action dispatch ----------

    pub fn apply(&mut self, action: Action) -> Option<AsyncCmd> {
        match action {
            Action::GoList(id) => {
                self.active_list = id;
                self.cursor = 0;
            }
            Action::CycleMode => self.cycle_mode(),
            Action::ToggleDone => {
                self.show_done = !self.show_done;
                self.clamp_cursor();
            }
            Action::ToggleDeferred => {
                self.show_deferred = !self.show_deferred;
                self.clamp_cursor();
            }
            Action::OpenAdd => self.prompt = Some(new_prompt(PromptKind::Add)),
            Action::Complete => self.toggle_done(),
            Action::Indent => self.indent(),
            Action::Outdent => self.outdent(),
            Action::ReorderUp => self.reorder(-1),
            Action::ReorderDown => self.reorder(1),
            Action::Collapse => self.toggle_collapse(),
            Action::MoveToList(list) => self.move_to_list(&list),
            Action::OpenDefer => {
                if let Some(id) = self.current_id() {
                    self.prompt = Some(new_prompt(PromptKind::Defer(id)));
                }
            }
            Action::DeferAt(ts) => self.defer_current(Some(ts)),
            Action::DeferClear => self.defer_current(None),
            Action::Delete => {
                if let Some(id) = self.current_id() {
                    return Some(AsyncCmd::DeleteTasks(vec![id]));
                }
            }
            Action::OpenEdit => {
                if let Some(id) = self.current_id() {
                    let mut p = new_prompt(PromptKind::Edit(id.clone()));
                    p.buf = self.task(&id).map(task_edit_string).unwrap_or_default();
                    self.prompt = Some(p);
                }
            }
            Action::SetContext(ctx) => {
                if let Some(id) = self.current_id() {
                    self.push_undo();
                    if let Some(t) = self.task_mut(&id) {
                        t.ctx = ctx;
                    }
                    self.dirty = true;
                }
            }
            Action::DeleteContext(c) => {
                self.store.contexts.retain(|x| x != &c);
                if self.ctx_filter == c {
                    self.ctx_filter.clear();
                    self.cursor = 0;
                }
                self.dirty = true;
                self.toast(format!("removed context {c}"));
            }
            Action::OpenEditNote => {
                if let Some(id) = self.current_id() {
                    self.editor_request = Some(EditorRequest::Note(id));
                }
            }
            Action::AddComment => {
                if let Some(id) = self.current_id() {
                    self.editor_request = Some(EditorRequest::Comment(id));
                }
            }
            Action::Undo => self.undo(),
            Action::OpenSearch => self.prompt = Some(new_prompt(PromptKind::Search)),
            Action::SetDue(due) => self.set_due(&due),
            Action::CtxFilter(c) => {
                self.ctx_filter = if self.ctx_filter == c { String::new() } else { c };
                self.cursor = 0;
            }
            Action::CtxClear => {
                self.ctx_filter.clear();
                self.cursor = 0;
            }
            Action::ToggleDetail => self.show_detail = !self.show_detail,
            Action::ClearDone => {
                let ids: Vec<String> = self
                    .store
                    .tasks
                    .iter()
                    .filter(|t| t.is_done())
                    .map(|t| t.id.clone())
                    .collect();
                if ids.is_empty() {
                    self.toast("no done tasks");
                } else {
                    return Some(AsyncCmd::DeleteTasks(ids));
                }
            }
            Action::EditConfig => match todarchy_core::config::config_path() {
                Ok(p) => self.editor_request = Some(EditorRequest::File(p)),
                Err(e) => self.toast(format!("config path error: {e}")),
            },
            Action::OpenPrompt(kind) => self.prompt = Some(new_prompt(kind)),
            Action::CopyText(text) => {
                spawn_copy(&text);
                self.toast("copied");
            }
            Action::Async(cmd) => return Some(cmd),
            Action::Quit => self.quit = true,
        }
        None
    }

    fn cycle_mode(&mut self) {
        match self.mode_label() {
            "todo" => {
                self.limit_to_first = true;
                self.show_done = false;
                self.show_deferred = false;
            }
            "next" => {
                self.limit_to_first = false;
                self.show_done = true;
                self.show_deferred = true;
            }
            _ => {
                self.limit_to_first = false;
                self.show_done = false;
                self.show_deferred = false;
            }
        }
        self.clamp_cursor();
    }

    fn toggle_done(&mut self) {
        let Some(id) = self.current_id() else { return };
        self.push_undo();
        let now = now_ms();
        if let Some(t) = self.task_mut(&id) {
            if t.done_at.is_some() {
                t.done_at = None;
                self.toast("reopened");
            } else {
                t.done_at = Some(now);
                self.toast("completed");
            }
        }
        self.dirty = true;
        self.clamp_cursor();
    }

    fn set_due(&mut self, due: &str) {
        let Some(id) = self.current_id() else { return };
        self.push_undo();
        if let Some(t) = self.task_mut(&id) {
            t.due = due.to_string();
        }
        self.dirty = true;
    }

    fn defer_current(&mut self, ts: Option<i64>) {
        let Some(id) = self.current_id() else { return };
        self.push_undo();
        if let Some(t) = self.task_mut(&id) {
            t.defer_until = ts;
        }
        match ts {
            Some(t) => self.toast(format!("deferred · {}", model::format_defer_until(t))),
            None => self.toast("un-deferred"),
        }
        self.dirty = true;
        self.clamp_cursor();
    }

    fn move_to_list(&mut self, list: &str) {
        let Some(id) = self.current_id() else { return };
        self.push_undo();
        if let Some(t) = self.task_mut(&id) {
            t.list = list.to_string();
            t.parent = None; // moving lists breaks nesting
        }
        self.dirty = true;
        self.clamp_cursor();
        let label = self.list_label(list);
        self.toast(format!("moved → {label}"));
    }

    fn toggle_collapse(&mut self) {
        let Some(id) = self.current_id() else { return };
        if self.collapsed.contains(&id) {
            self.collapsed.remove(&id);
        } else {
            self.collapsed.insert(id);
        }
        self.clamp_cursor();
    }

    fn undo(&mut self) {
        if let Some(prev) = self.undo_stack.pop() {
            self.store.tasks = prev;
            self.dirty = true;
            self.clamp_cursor();
            self.toast("undo");
        } else {
            self.toast("nothing to undo");
        }
    }

    fn indent(&mut self) {
        let rows = self.view();
        let Some(cur) = rows.get(self.cursor).cloned() else { return };
        // find previous sibling at same depth + same parent
        let cur_task = self.task(&cur.id).cloned();
        let Some(cur_task) = cur_task else { return };
        let mut target: Option<String> = None;
        for i in (0..self.cursor).rev() {
            let r = &rows[i];
            if r.depth < cur.depth {
                break;
            }
            if r.depth == cur.depth {
                let rt = self.task(&r.id);
                let same_parent = rt.map(|t| t.parent == cur_task.parent).unwrap_or(false);
                if same_parent {
                    target = Some(r.id.clone());
                    break;
                }
            }
        }
        match target {
            Some(pid) => {
                self.push_undo();
                if let Some(t) = self.task_mut(&cur.id) {
                    t.parent = Some(pid);
                }
                self.dirty = true;
            }
            None => self.toast("no sibling above"),
        }
    }

    fn outdent(&mut self) {
        let Some(id) = self.current_id() else { return };
        let parent = self.task(&id).and_then(|t| t.parent.clone());
        let Some(parent_id) = parent else {
            self.toast("already top level");
            return;
        };
        let grandparent = self.task(&parent_id).and_then(|t| t.parent.clone());
        self.push_undo();
        if let Some(t) = self.task_mut(&id) {
            t.parent = grandparent;
        }
        self.dirty = true;
    }

    fn reorder(&mut self, delta: i64) {
        let rows = self.view();
        let ni = self.cursor as i64 + delta;
        if ni < 0 || ni as usize >= rows.len() {
            self.toast("edge of list");
            return;
        }
        let cur = rows[self.cursor].clone();
        let neigh = rows[ni as usize].clone();
        // must be in the same sort group: same status, due, parent
        let now = now_ms();
        let (a, b) = match (self.task(&cur.id), self.task(&neigh.id)) {
            (Some(a), Some(b)) => (a.clone(), b.clone()),
            _ => return,
        };
        let same_group = status_rank(&a, now) == status_rank(&b, now)
            && a.due == b.due
            && a.parent == b.parent;
        if !same_group {
            self.toast("can't cross sort group — change due/project first");
            return;
        }
        self.push_undo();
        let a_key = a.order_key();
        let b_key = b.order_key();
        if let Some(t) = self.task_mut(&cur.id) {
            t.pos = Some(b_key);
        }
        if let Some(t) = self.task_mut(&neigh.id) {
            t.pos = Some(a_key);
        }
        self.dirty = true;
        self.cursor = ni as usize;
    }

    // ---------- prompt handling ----------

    fn prompt_key(&mut self, key: crossterm::event::KeyEvent) -> Option<AsyncCmd> {
        use crossterm::event::KeyCode;
        let Some(prompt) = self.prompt.as_mut() else { return None };
        match key.code {
            KeyCode::Esc => {
                // Search prompt: Esc also clears the applied search.
                if matches!(prompt.kind, PromptKind::Search) {
                    self.search.clear();
                }
                self.prompt = None;
            }
            KeyCode::Enter => return self.commit_prompt(),
            KeyCode::Backspace => {
                prompt.buf.pop();
                if matches!(prompt.kind, PromptKind::Search) {
                    self.search = self.prompt.as_ref().unwrap().buf.clone();
                    self.cursor = 0;
                }
            }
            KeyCode::Char(c) => {
                prompt.buf.push(c);
                if matches!(prompt.kind, PromptKind::Search) {
                    self.search = self.prompt.as_ref().unwrap().buf.clone();
                    self.cursor = 0;
                }
            }
            _ => {}
        }
        None
    }

    fn commit_prompt(&mut self) -> Option<AsyncCmd> {
        let Some(prompt) = self.prompt.take() else { return None };
        let buf = prompt.buf.trim().to_string();
        match prompt.kind {
            PromptKind::Add => {
                if buf.is_empty() {
                    self.toast("empty — nothing added");
                    return None;
                }
                self.add_task(&buf);
            }
            PromptKind::Edit(id) => {
                if !buf.is_empty() {
                    // Re-parse so an edit can also change @context / !due (and
                    // dropping a token clears that field). Note is only touched
                    // if the edit actually included a /note.
                    let q = parse_quick_add(&buf);
                    let title = if q.title.is_empty() { buf.clone() } else { q.title };
                    self.push_undo();
                    if let Some(t) = self.task_mut(&id) {
                        t.title = title;
                        t.ctx = q.ctx;
                        t.due = q.due;
                        if !q.note.is_empty() {
                            t.note = q.note;
                        }
                    }
                    self.dirty = true;
                }
            }
            PromptKind::Search => { /* already applied live */ }
            PromptKind::Defer(id) => match parse_defer_text(&buf) {
                Some(ts) => {
                    self.push_undo();
                    if let Some(t) = self.task_mut(&id) {
                        t.defer_until = Some(ts);
                    }
                    self.dirty = true;
                    self.toast(format!("deferred · {}", model::format_defer_until(ts)));
                    self.clamp_cursor();
                }
                None => {
                    self.prompt = Some(Prompt {
                        kind: PromptKind::Defer(id),
                        buf,
                        err: Some("unrecognized — try tomorrow · +3d · fri · 2026-01-01".into()),
                    });
                }
            },
            PromptKind::AcceptLink => {
                if !buf.is_empty() {
                    return Some(AsyncCmd::Accept(buf));
                }
            }
            PromptKind::NewProject => {
                if !buf.is_empty() {
                    self.add_project(&buf);
                }
            }
            PromptKind::RenameProject(id) => {
                if !buf.is_empty() {
                    if let Some(p) = self.store.projects.iter_mut().find(|p| p.id == id) {
                        p.name = buf;
                    }
                    self.dirty = true;
                }
            }
            PromptKind::NewContext => {
                let mut c = buf;
                if !c.is_empty() {
                    if !c.starts_with('@') {
                        c = format!("@{c}");
                    }
                    if !self.store.contexts.contains(&c) {
                        self.store.contexts.push(c);
                        self.dirty = true;
                    }
                }
            }
            PromptKind::RenameContext(old) => {
                let mut new = buf;
                if !new.is_empty() {
                    if !new.starts_with('@') {
                        new = format!("@{new}");
                    }
                    for c in self.store.contexts.iter_mut() {
                        if *c == old {
                            *c = new.clone();
                        }
                    }
                    for t in self.store.tasks.iter_mut() {
                        if t.ctx == old {
                            t.ctx = new.clone();
                        }
                    }
                    if self.ctx_filter == old {
                        self.ctx_filter = new.clone();
                    }
                    self.dirty = true;
                    self.toast(format!("renamed to {new}"));
                }
            }
            PromptKind::CommentAuthor => {
                set_comment_author(&buf);
                self.toast(format!("comments will post as {}", comment_author()));
            }
            PromptKind::ExportJson => self.export_file(&buf, false),
            PromptKind::ExportMd => self.export_file(&buf, true),
            PromptKind::ImportJson => self.import_file(&buf),
        }
        None
    }

    fn add_task(&mut self, raw: &str) {
        let q = parse_quick_add(raw);
        let title = if q.title.is_empty() { raw.trim().to_string() } else { q.title };
        if title.is_empty() {
            self.toast("empty — nothing added");
            return;
        }
        self.push_undo();
        let now = now_ms();
        let task = Task {
            id: model::new_uuid(),
            list: self.active_list.clone(),
            title: title.clone(),
            ctx: q.ctx,
            due: q.due,
            note: q.note,
            parent: None,
            created: now,
            done_at: None,
            defer_until: None,
            pos: Some(now as f64),
            rest: serde_json::Map::new(),
        };
        self.store.tasks.push(task);
        self.dirty = true;
        self.cursor = 0;
        self.toast(format!("added: {title}"));
    }

    fn add_project(&mut self, name: &str) {
        let id = format!("p_{}", model::new_uuid());
        self.store.projects.push(Project {
            id: id.clone(),
            name: name.to_string(),
            icon: "folder".into(),
            accent: String::new(),
            rest: serde_json::Map::new(),
        });
        self.dirty = true;
        self.active_list = id;
        self.cursor = 0;
        self.toast(format!("project: {name}"));
    }

    fn export_file(&mut self, path: &str, markdown: bool) {
        let contents = if markdown {
            model::export_markdown(&self.store)
        } else {
            serde_json::to_string_pretty(&self.store.to_json()).unwrap_or_default()
        };
        match std::fs::write(path, contents) {
            Ok(()) => self.toast(format!("exported → {path}")),
            Err(e) => self.toast(format!("export failed: {e}")),
        }
    }

    fn import_file(&mut self, path: &str) {
        match std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok()) {
            Some(v) => {
                self.push_undo();
                let incoming = Store::from_json(&v);
                // merge by id: upsert tasks/projects
                for t in incoming.tasks {
                    if let Some(existing) = self.store.tasks.iter_mut().find(|x| x.id == t.id) {
                        *existing = t;
                    } else {
                        self.store.tasks.push(t);
                    }
                }
                for p in incoming.projects {
                    if !self.store.projects.iter().any(|x| x.id == p.id) {
                        self.store.projects.push(p);
                    }
                }
                self.dirty = true;
                self.toast(format!("imported ← {path}"));
            }
            None => self.toast("import failed: not valid JSON"),
        }
    }

    // ---------- palette handling ----------

    fn palette_key(&mut self, key: crossterm::event::KeyEvent) -> Option<AsyncCmd> {
        use crossterm::event::{KeyCode, KeyModifiers};
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => {
                self.palette = None;
            }
            KeyCode::Down | KeyCode::Char('n') if key.code == KeyCode::Down || ctrl => {
                let items = self.palette_items();
                if let Some(p) = self.palette.as_mut() {
                    p.sel = (p.sel + 1).min(items.len().saturating_sub(1));
                }
            }
            KeyCode::Up | KeyCode::Char('p') if key.code == KeyCode::Up || ctrl => {
                if let Some(p) = self.palette.as_mut() {
                    p.sel = p.sel.saturating_sub(1);
                }
            }
            KeyCode::Enter => {
                let items = self.palette_items();
                let sel = self.palette.as_ref().map(|p| p.sel).unwrap_or(0);
                if let Some(item) = items.get(sel) {
                    match item {
                        PaletteItem::Command(cmd_id) => {
                            let action = self
                                .commands()
                                .into_iter()
                                .find(|c| &c.id == cmd_id)
                                .map(|c| c.action);
                            self.palette = None;
                            if let Some(action) = action {
                                return self.apply(action);
                            }
                        }
                        PaletteItem::Task { list, id } => {
                            self.active_list = list.clone();
                            self.palette = None;
                            // move cursor onto the task if present in the view
                            let pos = self.view().iter().position(|r| &r.id == id);
                            if let Some(p) = pos {
                                self.cursor = p;
                            }
                        }
                    }
                }
            }
            KeyCode::Backspace => {
                if let Some(p) = self.palette.as_mut() {
                    p.q.pop();
                    p.sel = 0;
                }
            }
            KeyCode::Char(c) => {
                if let Some(p) = self.palette.as_mut() {
                    p.q.push(c);
                    p.sel = 0;
                }
            }
            _ => {}
        }
        None
    }

    /// Build the full command list (before filtering). Public so ui.rs can
    /// render the same set the palette scores.
    pub fn commands(&self) -> Vec<Command> {
        let mut c: Vec<Command> = Vec::new();
        let mut push = |id: &str, title: String, hint: &str, keys: &[&str], action: Action| {
            c.push(Command {
                id: id.into(),
                title,
                hint: hint.into(),
                keys: keys.iter().map(|s| s.to_string()).collect(),
                action,
            });
        };

        push("go-inbox", "go to → inbox".into(), "", &["g", "i"], Action::GoList("inbox".into()));
        for (i, p) in self.store.projects.iter().enumerate() {
            push(
                &format!("go-{}", p.id),
                format!("go to → {}", p.name),
                "",
                &[&format!("g{}", i + 1)],
                Action::GoList(p.id.clone()),
            );
        }
        push("new-project", "new project".into(), "", &["g", "n"], Action::OpenPrompt(PromptKind::NewProject));
        if self.active_list != "inbox" {
            let name = self.list_label(&self.active_list);
            push(
                "rename-project",
                format!("rename project “{name}”"),
                "",
                &[],
                Action::OpenPrompt(PromptKind::RenameProject(self.active_list.clone())),
            );
            push(
                "delete-project",
                format!("delete project “{name}”"),
                "keeps its tasks in place",
                &[],
                Action::Async(AsyncCmd::DeleteProject(self.active_list.clone())),
            );
        }
        let ml = self.mode_label();
        let next = if ml == "todo" { "next" } else if ml == "next" { "all" } else { "todo" };
        push("list-mode", format!("list view: {ml} → {next}"), "todo / next / all", &["v"], Action::CycleMode);
        push("toggle-done", "toggle: show done".into(), "", &["f", "d"], Action::ToggleDone);
        push("toggle-deferred", "toggle: show deferred".into(), "", &["f", "s"], Action::ToggleDeferred);
        push("add", "add task".into(), "", &["o"], Action::OpenAdd);
        push("complete", "toggle complete".into(), "", &["x"], Action::Complete);
        push("indent", "nest under sibling above".into(), "", &["tab"], Action::Indent);
        push("outdent", "un-nest".into(), "", &["⇧tab"], Action::Outdent);
        push("reorder-up", "move up".into(), "", &["⇧k"], Action::ReorderUp);
        push("reorder-down", "move down".into(), "", &["⇧j"], Action::ReorderDown);
        push("collapse", "collapse / expand children".into(), "", &["z"], Action::Collapse);
        push("move-inbox", "move → inbox".into(), "", &["m", "i"], Action::MoveToList("inbox".into()));
        for (i, p) in self.store.projects.iter().enumerate() {
            push(&format!("move-{}", p.id), format!("move → {}", p.name), "", &[&format!("m{}", i + 1)], Action::MoveToList(p.id.clone()));
        }
        push("defer", "defer…".into(), "", &["d"], Action::OpenDefer);
        push("defer-tomorrow", "defer → tomorrow".into(), "", &[], Action::DeferAt(parse_defer_text("tomorrow").unwrap_or(0)));
        push("defer-week", "defer → +1 week".into(), "", &[], Action::DeferAt(parse_defer_text("+1w").unwrap_or(0)));
        push("defer-weekend", "defer → weekend".into(), "", &[], Action::DeferAt(parse_defer_text("weekend").unwrap_or(0)));
        push("defer-clear", "un-defer".into(), "", &[], Action::DeferClear);
        push("delete", "delete task".into(), "", &["del"], Action::Delete);
        push("edit", "edit title".into(), "", &["e"], Action::OpenEdit);
        push("edit-note", "edit note (in $EDITOR)".into(), "", &["c"], Action::OpenEditNote);
        push("add-comment", "add comment (in $EDITOR)".into(), "", &["⇧c"], Action::AddComment);
        push(
            "comment-author",
            format!("set comment author (now: {})", comment_author()),
            "shown on comments you post",
            &[],
            Action::OpenPrompt(PromptKind::CommentAuthor),
        );
        push("undo", "undo".into(), "", &["u"], Action::Undo);
        push("search", "search in list".into(), "", &["/"], Action::OpenSearch);
        push("due-today", "due → today".into(), "", &[], Action::SetDue("today".into()));
        push("due-tomorrow", "due → tomorrow".into(), "", &[], Action::SetDue("tomorrow".into()));
        push("due-week", "due → this week".into(), "", &[], Action::SetDue("this week".into()));
        push("due-clear", "due → clear".into(), "", &[], Action::SetDue(String::new()));
        for ctx in &self.store.contexts {
            push(&format!("ctx-{ctx}"), format!("filter: {ctx}"), "", &[], Action::CtxFilter(ctx.clone()));
            push(&format!("setctx-{ctx}"), format!("set context → {ctx}"), "on selected task", &[], Action::SetContext(ctx.clone()));
            push(&format!("renamectx-{ctx}"), format!("rename context {ctx}"), "", &[], Action::OpenPrompt(PromptKind::RenameContext(ctx.clone())));
            push(&format!("delctx-{ctx}"), format!("delete context {ctx}"), "", &[], Action::DeleteContext(ctx.clone()));
        }
        push("setctx-clear", "set context → (none)".into(), "on selected task", &[], Action::SetContext(String::new()));
        push("ctx-clear", "filter: clear context".into(), "", &[], Action::CtxClear);
        push("new-context", "new context".into(), "", &[], Action::OpenPrompt(PromptKind::NewContext));
        push("toggle-detail", "toggle detail pane".into(), "", &["i"], Action::ToggleDetail);
        push("clear-done", "delete all done tasks".into(), "", &[], Action::ClearDone);

        // sync — configuration is a hand-edited text file (config.toml); the
        // palette just opens it and offers read-only diagnostics.
        push("sync-edit", "sync: edit config (in $EDITOR)".into(), "~/.config/todarchy/config.toml", &[], Action::EditConfig);
        push("sync-check", "sync: check server reachable".into(), "", &[], Action::Async(AsyncCmd::CheckServer));
        if !self.sync.server_main_doc_id.is_empty() {
            push("sync-copy-id", "sync: copy relay doc id".into(), "paste into other devices' config", &[], Action::CopyText(self.sync.server_main_doc_id.clone()));
        }

        // sharing
        let cur_project = if self.active_list != "inbox" { Some(self.active_list.clone()) } else { None };
        if let Some(pid) = &cur_project {
            let shared = self.store.projects.iter().find(|p| &p.id == pid).map(|p| p.is_shared()).unwrap_or(false);
            let name = self.list_label(pid);
            if shared {
                push("share-leave", format!("share: leave “{name}”"), "", &[], Action::Async(AsyncCmd::Leave(pid.clone())));
            } else {
                push("share-promote", format!("share: promote “{name}”"), "encrypt + get link", &[], Action::Async(AsyncCmd::Promote(pid.clone())));
            }
        }
        push("share-accept", "share: accept link…".into(), "paste todarchy:// link", &[], Action::OpenPrompt(PromptKind::AcceptLink));

        // export/import
        push("export-md", "export → markdown".into(), "", &[], Action::OpenPrompt(PromptKind::ExportMd));
        push("export-json", "export → json".into(), "", &[], Action::OpenPrompt(PromptKind::ExportJson));
        push("import-json", "import ← json".into(), "", &[], Action::OpenPrompt(PromptKind::ImportJson));

        push("quit", "quit todarchy".into(), "", &["q"], Action::Quit);
        c
    }

    /// The palette's filtered result list (commands + task hits), honoring
    /// the `:`/`>` command-only prefixes like palette.jsx.
    pub fn palette_items(&self) -> Vec<PaletteItem> {
        let q = self.palette.as_ref().map(|p| p.q.clone()).unwrap_or_default();
        let cmd_only = q.starts_with(':') || q.starts_with('>');
        let needle = q.trim_start_matches([':', '>']).trim().to_string();

        let mut cmd_hits: Vec<(f64, String)> = self
            .commands()
            .into_iter()
            .filter_map(|c| {
                fuzzy_score(&needle, &format!("{} {}", c.title, c.hint)).map(|s| (s, c.id))
            })
            .collect();
        cmd_hits.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

        let show_tasks = !cmd_only;
        let mut task_hits: Vec<(f64, PaletteItem)> = if show_tasks {
            self.store
                .tasks
                .iter()
                .filter_map(|t| {
                    fuzzy_score(&needle, &format!("{} {}", t.title, t.ctx)).map(|s| {
                        (s, PaletteItem::Task { list: t.list.clone(), id: t.id.clone() })
                    })
                })
                .collect()
        } else {
            Vec::new()
        };
        task_hits.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        task_hits.truncate(8);

        let is_cmd_mode = cmd_only || needle.is_empty();
        let mut out: Vec<PaletteItem> = Vec::new();
        if is_cmd_mode {
            out.extend(cmd_hits.into_iter().map(|(_, id)| PaletteItem::Command(id)));
        } else {
            out.extend(cmd_hits.into_iter().take(6).map(|(_, id)| PaletteItem::Command(id)));
        }
        out.extend(task_hits.into_iter().map(|(_, i)| i));
        out
    }
}

#[derive(Clone)]
pub enum PaletteItem {
    /// A command, referenced by its stable id.
    Command(String),
    Task { list: String, id: String },
}

/// A request for the event loop to hand the terminal to $EDITOR. The loop
/// runs the editor (it can't happen inside App, which never touches the tty),
/// then calls back into `set_note` / `add_comment` with the result.
#[derive(Clone, Debug)]
pub enum EditorRequest {
    Note(String),
    Comment(String),
    /// Edit a file in place (e.g. the config) — no task write-back.
    File(std::path::PathBuf),
}

fn status_rank(t: &Task, now: i64) -> i32 {
    if t.is_done() {
        2
    } else if t.is_deferred(now) {
        1
    } else {
        0
    }
}

fn new_prompt(kind: PromptKind) -> Prompt {
    Prompt { kind, buf: String::new(), err: None }
}

/// Reconstruct the editable quick-add line for a task, so `e` can change
/// title, @context, and !due together (and dropping a token clears it).
fn task_edit_string(t: &Task) -> String {
    let mut s = t.title.clone();
    if !t.ctx.is_empty() {
        s.push(' ');
        s.push_str(&t.ctx);
    }
    if !t.due.is_empty() {
        let short = if t.due == "this week" { "week" } else { &t.due };
        s.push_str(&format!(" !{short}"));
    }
    s
}

/// Char-in-order fuzzy score (higher = better, None = no match). Port of
/// data.jsx fuzzyScore.
pub fn fuzzy_score(needle: &str, hay: &str) -> Option<f64> {
    if needle.is_empty() {
        return Some(0.0001);
    }
    let needle = needle.to_lowercase();
    let hay = hay.to_lowercase();
    let hay_bytes: Vec<char> = hay.chars().collect();
    let mut hi = 0usize;
    let mut score = 0.0;
    let mut streak = 0.0;
    for c in needle.chars() {
        let mut found = None;
        for (idx, hc) in hay_bytes.iter().enumerate().skip(hi) {
            if *hc == c {
                found = Some(idx);
                break;
            }
        }
        let idx = found?;
        score += 1.0 / (1.0 + (idx - hi) as f64);
        if idx == hi {
            streak += 1.0;
            score += streak * 0.5;
        } else {
            streak = 0.0;
        }
        hi = idx + 1;
    }
    Some(score)
}

/// Path to the per-device comment-author file (local, not synced — each
/// device controls its own identity, matching the iOS app's UserDefaults key).
fn comment_author_path() -> Option<std::path::PathBuf> {
    let dir = dirs::config_dir()?.join("todarchy");
    let _ = std::fs::create_dir_all(&dir);
    Some(dir.join("comment_author"))
}

/// The display name stamped on new comments (default "Me").
pub fn comment_author() -> String {
    comment_author_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Me".to_string())
}

pub fn set_comment_author(name: &str) {
    let Some(path) = comment_author_path() else { return };
    let name = name.trim();
    if name.is_empty() {
        let _ = std::fs::remove_file(path);
    } else {
        let _ = std::fs::write(path, name);
    }
}

/// Copy to the Wayland clipboard (Omarchy is Hyprland/Wayland). Fire and
/// forget; falls back silently if wl-copy isn't present.
fn spawn_copy(text: &str) {
    use std::io::Write;
    use std::process::{Command, Stdio};
    if let Ok(mut child) = Command::new("wl-copy").stdin(Stdio::piped()).spawn() {
        if let Some(stdin) = child.stdin.as_mut() {
            let _ = stdin.write_all(text.as_bytes());
        }
        let _ = child.wait();
    }
}
