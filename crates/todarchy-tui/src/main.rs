// todarchy — the terminal UI entrypoint.
//
// Owns the async runtime, the terminal, and the todarchy-core store calls.
// Architecture:
//   - a blocking thread reads crossterm input and forwards it on a channel;
//   - the three core watcher loops (folder, server relay, notifications)
//     run as tokio tasks, each holding an `Arc<TuiSink>` that forwards
//     events onto the same channel;
//   - a 1s ticker drives deferred-task surfacing and toast expiry;
//   - the event loop draws, awaits the next event, applies it, then
//     persists via `store::save` whenever the app marks itself dirty.
//
// Also handles the `todarchy accept <todarchy://…>` subcommand used by the
// x-scheme-handler .desktop entry to ingest a share link headlessly.

mod app;
mod markdown;
mod model;
mod sink;
mod theme;
mod ui;

use std::io::Stdout;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{Event, KeyEventKind};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use todarchy_core::{store, sync, EventSink, NullSink};
use tokio::sync::mpsc::{self, UnboundedSender};

use app::{App, AsyncCmd, EditorRequest};
use model::Store;
use sink::{AppEvent, TuiSink};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    // Subcommand: accept a share link, headless, then exit. Wired to the
    // x-scheme-handler/todarchy .desktop entry.
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("gen-id") {
        // Print a fresh relay doc id to paste into config.toml on each device.
        println!("{}", todarchy_core::config::generate_main_doc_id());
        return Ok(());
    }
    if args.get(1).map(String::as_str) == Some("accept") {
        let Some(url) = args.get(2) else {
            eprintln!("usage: todarchy accept <todarchy://share/…>");
            std::process::exit(2);
        };
        let sink = NullSink;
        match todarchy_core::shared::share_accept(&sink, url.clone()).await {
            Ok(pid) => {
                println!("accepted shared project {pid}");
                return Ok(());
            }
            Err(e) => {
                eprintln!("accept failed: {e}");
                std::process::exit(1);
            }
        }
    }

    run_tui().await
}

async fn run_tui() -> Result<()> {
    // Create the documented config template on first run / migrate an old one.
    if let Err(e) = todarchy_core::config::ensure_documented() {
        tracing::warn!("could not write config template: {e}");
    }

    let (tx, mut rx) = mpsc::unbounded_channel::<AppEvent>();

    let sink: Arc<TuiSink> = Arc::new(TuiSink::new(tx.clone()));
    let dyn_sink: Arc<dyn EventSink> = sink.clone();

    // Background loops from todarchy-core — folder watcher, server poll,
    // and the due-task notifier. Each keeps its own Arc to the sink.
    tokio::spawn(todarchy_core::sync_watcher::run_loop(dyn_sink.clone()));
    tokio::spawn(todarchy_core::sync_watcher::server_poll_loop(dyn_sink.clone()));
    tokio::spawn(todarchy_core::notify::run_loop(dyn_sink.clone()));

    // Input reader thread: crossterm's read() blocks, so it lives off the
    // async runtime and forwards onto the channel. It polls (rather than
    // blocking-read) so it can be paused while we hand the terminal to
    // $EDITOR — otherwise it would steal the editor's keystrokes.
    let reader_pause = Arc::new(AtomicBool::new(false));
    {
        let tx = tx.clone();
        let pause = reader_pause.clone();
        std::thread::spawn(move || loop {
            if pause.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(20));
                continue;
            }
            match crossterm::event::poll(Duration::from_millis(50)) {
                Ok(true) => match crossterm::event::read() {
                    Ok(ev) => {
                        if tx.send(AppEvent::Input(ev)).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                },
                Ok(false) => {}
                Err(_) => break,
            }
        });
    }

    // 1s ticker.
    {
        let tx = tx.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
            loop {
                interval.tick().await;
                if tx.send(AppEvent::Tick).is_err() {
                    break;
                }
            }
        });
    }

    // Initial load through the full sync path.
    let initial = store::load(&*sink).await.unwrap_or_else(|e| {
        tracing::warn!("initial load failed: {e}");
        serde_json::json!({ "tasks": [], "projects": [], "contexts": [] })
    });
    let mut app = App::new(Store::from_json(&initial), sync::current_status());

    // Background persistence worker: the event loop hands it snapshots and
    // delete ops so disk / Dropbox / relay writes never block input or redraw.
    let (persist_tx, persist_rx) = mpsc::unbounded_channel::<PersistOp>();
    let worker = tokio::spawn(persist_worker(persist_rx, dyn_sink.clone()));

    let mut terminal = setup_terminal()?;
    let res = event_loop(&mut terminal, &mut app, &mut rx, &sink, &tx, &persist_tx, &reader_pause).await;
    restore_terminal(&mut terminal)?;

    // Flush any queued writes before exit: close the channel so the worker
    // drains its backlog, then wait for it to finish.
    drop(persist_tx);
    let _ = worker.await;
    res
}

/// A persistence request handed to the background worker. Keeps disk /
/// Dropbox / relay writes off the input+render path so the UI is instant.
enum PersistOp {
    Save(serde_json::Value),
    Delete { root: String, ids: Vec<String> },
}

/// Drains persistence ops and applies them without ever blocking the UI.
/// Coalesces a burst: only the latest snapshot is saved, and any deletes in
/// the burst are tombstoned — so mashing keys yields one write, not N network
/// round-trips. Save runs before deletes so a tombstone always wins.
async fn persist_worker(mut rx: mpsc::UnboundedReceiver<PersistOp>, sink: Arc<dyn EventSink>) {
    while let Some(first) = rx.recv().await {
        let mut batch = vec![first];
        while let Ok(op) = rx.try_recv() {
            batch.push(op);
        }
        // Process in causal order, coalescing only *consecutive* saves. A
        // delete flushes the pending save first — so if a delete is followed
        // by an undo's re-add save in the same burst, the re-add wins (the
        // task comes back), instead of the tombstone clobbering it.
        let mut pending_save: Option<serde_json::Value> = None;
        for op in batch {
            match op {
                PersistOp::Save(v) => pending_save = Some(v),
                PersistOp::Delete { root, ids } => {
                    if let Some(v) = pending_save.take() {
                        if let Err(e) = store::save(&*sink, v).await {
                            tracing::warn!("background save failed: {e}");
                        }
                    }
                    if let Err(e) = store::delete_many(&*sink, &root, &ids).await {
                        tracing::warn!("background delete failed: {e}");
                    }
                }
            }
        }
        if let Some(v) = pending_save.take() {
            if let Err(e) = store::save(&*sink, v).await {
                tracing::warn!("background save failed: {e}");
            }
        }
    }
}

async fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut App,
    rx: &mut mpsc::UnboundedReceiver<AppEvent>,
    sink: &Arc<TuiSink>,
    tx: &UnboundedSender<AppEvent>,
    persist_tx: &UnboundedSender<PersistOp>,
    reader_pause: &AtomicBool,
) -> Result<()> {
    loop {
        terminal.draw(|f| ui::render(f, app))?;

        let Some(ev) = rx.recv().await else { break };
        match ev {
            AppEvent::Input(Event::Key(k)) => {
                if k.kind == KeyEventKind::Press || k.kind == KeyEventKind::Repeat {
                    if let Some(cmd) = app.on_key(k) {
                        match cmd {
                            // Deletes: drop from the UI instantly, tombstone in
                            // the background — no network wait.
                            AsyncCmd::DeleteTasks(ids) => {
                                let n = ids.len();
                                app.remove_tasks_local(&ids);
                                let _ = persist_tx.send(PersistOp::Delete { root: "tasks".into(), ids });
                                app.toast(if n == 1 { "deleted".into() } else { format!("deleted {n}") });
                            }
                            AsyncCmd::DeleteProject(id) => {
                                app.remove_project_local(&id);
                                let _ = persist_tx.send(PersistOp::Delete { root: "projects".into(), ids: vec![id] });
                                app.toast("project deleted");
                            }
                            // Sync/share setup: rare, user expects a beat.
                            other => dispatch_async(app, sink, other).await,
                        }
                    }
                }
            }
            AppEvent::Input(_) => { /* resize etc — redraw next loop */ }
            AppEvent::TasksChanged(v) => {
                if v.is_null() {
                    let _ = tx.send(AppEvent::Reload);
                } else {
                    app.adopt(&v);
                }
            }
            AppEvent::SyncStatus(s) => app.sync = s,
            AppEvent::Notify { title, body } => {
                let _ = tokio::process::Command::new("notify-send")
                    .arg(&title)
                    .arg(&body)
                    .spawn();
            }
            AppEvent::Reload => {
                if let Ok(v) = store::load(&**sink).await {
                    app.adopt(&v);
                }
            }
            AppEvent::Tick => app.on_tick(),
        }

        // A key/palette action asked to edit in $EDITOR — hand off the
        // terminal (blocking), then fold the result back in.
        if let Some(req) = app.editor_request.take() {
            match req {
                EditorRequest::Note(id) => {
                    let initial = app.task(&id).map(|t| t.note.clone()).unwrap_or_default();
                    match tokio::task::block_in_place(|| edit_text(terminal, reader_pause, &initial)) {
                        Ok(text) => app.set_note(&id, &text),
                        Err(e) => app.toast(format!("editor failed: {e}")),
                    }
                }
                EditorRequest::Comment(id) => {
                    match tokio::task::block_in_place(|| edit_text(terminal, reader_pause, "")) {
                        Ok(text) => app.add_comment(&id, &text),
                        Err(e) => app.toast(format!("editor failed: {e}")),
                    }
                }
                EditorRequest::File(path) => {
                    match tokio::task::block_in_place(|| edit_path(terminal, reader_pause, &path)) {
                        Ok(()) => app.toast("config saved — sync re-reads within a moment"),
                        Err(e) => app.toast(format!("editor failed: {e}")),
                    }
                }
            }
        }

        // reset note scroll when the selected task changed
        app.reconcile_detail();

        if app.dirty {
            // Hand the snapshot to the background worker and move on — the
            // redraw at the top of the next loop happens immediately, without
            // waiting on disk/Dropbox/relay.
            let _ = persist_tx.send(PersistOp::Save(app.store.to_json()));
            app.dirty = false;
        }

        if app.quit {
            break;
        }
    }
    Ok(())
}

/// Run an async command off the key-handling path (deletes tombstone via
/// core, sync/share ops hit the network). Results are folded back into the
/// app as toasts + a reload where the store changed underneath us.
async fn dispatch_async(app: &mut App, sink: &Arc<TuiSink>, cmd: AsyncCmd) {
    let s: &dyn EventSink = &**sink;
    match cmd {
        AsyncCmd::DeleteTasks(ids) => {
            let n = ids.len();
            match store::delete_many(s, "tasks", &ids).await {
                Ok(()) => {
                    if let Ok(v) = store::load(s).await {
                        app.adopt(&v);
                    }
                    app.toast(if n == 1 { "deleted".into() } else { format!("deleted {n}") });
                }
                Err(e) => app.toast(format!("delete failed: {e}")),
            }
        }
        AsyncCmd::DeleteProject(id) => {
            match store::delete_many(s, "projects", std::slice::from_ref(&id)).await {
                Ok(()) => {
                    if let Ok(v) = store::load(s).await {
                        app.adopt(&v);
                    }
                    app.toast("project deleted");
                }
                Err(e) => app.toast(format!("delete failed: {e}")),
            }
        }
        AsyncCmd::CheckServer => match sync::server_healthz().await {
            Ok(true) => app.toast("server reachable"),
            Ok(false) => app.toast("server unreachable"),
            Err(e) => app.toast(format!("check failed: {e}")),
        },
        AsyncCmd::Promote(pid) => match todarchy_core::shared::share_promote(s, pid).await {
            Ok(link) => {
                app.copy_and_toast(&link);
                if let Ok(v) = store::load(s).await {
                    app.adopt(&v);
                }
            }
            Err(e) => app.toast(format!("promote failed: {e}")),
        },
        AsyncCmd::Accept(url) => match todarchy_core::shared::share_accept(s, url).await {
            Ok(pid) => {
                if let Ok(v) = store::load(s).await {
                    app.adopt(&v);
                }
                app.toast(format!("joined {pid}"));
            }
            Err(e) => app.toast(format!("accept failed: {e}")),
        },
        AsyncCmd::Leave(pid) => match todarchy_core::shared::share_leave(s, pid).await {
            Ok(()) => {
                if let Ok(v) = store::load(s).await {
                    app.adopt(&v);
                }
                app.toast("left shared project");
            }
            Err(e) => app.toast(format!("leave failed: {e}")),
        },
    }
}

/// Suspend the TUI, open `$EDITOR` on `path`, and re-enter. Blocking — call
/// via block_in_place. Pauses the input reader so the editor owns the terminal.
fn edit_path(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    pause: &AtomicBool,
    path: &std::path::Path,
) -> std::io::Result<()> {
    pause.store(true, Ordering::Relaxed);
    // Let any in-flight poll/read in the reader thread drain before we grab
    // the terminal.
    std::thread::sleep(Duration::from_millis(80));

    disable_raw_mode()?;
    crossterm::execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    let editor = std::env::var("VISUAL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| std::env::var("EDITOR").ok().filter(|s| !s.trim().is_empty()))
        .unwrap_or_else(|| "nvim".to_string());
    let mut parts = editor.split_whitespace();
    let prog = parts.next().unwrap_or("nvim");
    let args: Vec<&str> = parts.collect();
    let status = std::process::Command::new(prog).args(&args).arg(path).status();
    if status.is_err() {
        // configured editor missing — last-resort fallback
        let _ = std::process::Command::new("vi").arg(path).status();
    }

    enable_raw_mode()?;
    crossterm::execute!(terminal.backend_mut(), EnterAlternateScreen)?;
    terminal.clear()?;
    pause.store(false, Ordering::Relaxed);
    Ok(())
}

/// Edit `initial` in `$EDITOR` via a temp markdown file; return the result.
fn edit_text(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    pause: &AtomicBool,
    initial: &str,
) -> std::io::Result<String> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let path = std::env::temp_dir().join(format!("todarchy-{nanos}.md"));
    std::fs::write(&path, initial)?;
    edit_path(terminal, pause, &path)?;
    let content = std::fs::read_to_string(&path).unwrap_or_default();
    let _ = std::fs::remove_file(&path);
    Ok(content)
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    crossterm::execute!(stdout, EnterAlternateScreen)?;
    // Restore the terminal on panic so a crash doesn't leave a wrecked shell.
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = crossterm::execute!(std::io::stdout(), LeaveAlternateScreen);
        hook(info);
    }));
    let backend = CrosstermBackend::new(stdout);
    Ok(Terminal::new(backend)?)
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    disable_raw_mode()?;
    crossterm::execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}
