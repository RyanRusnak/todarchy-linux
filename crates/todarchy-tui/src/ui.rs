// ui.rs — all rendering. Pure function of &App → frame; never mutates
// state. Aesthetic: minimal / airy — no panel boxes, just whitespace, dim
// hairline rules between columns, and an accent bar on the selected row.
// Colors are ANSI/indexed so the terminal's Omarchy theme paints the whole
// UI; icons come from theme::glyphs() (Nerd Font, TODARCHY_ASCII fallback)
// and the one accent hue from theme::accent().

use ratatui::{
    prelude::*,
    widgets::{
        Block, BorderType, Borders, Clear, List, ListItem, ListState, Padding, Paragraph,
        Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap,
    },
};

use crate::app::{App, PaletteItem};
use crate::theme::{accent, glyphs};
use crate::{markdown, model};

const DIM: Color = Color::DarkGray;
const CTX: Color = Color::Cyan;

pub fn render(frame: &mut Frame, app: &App) {
    let area = frame.area();
    if area.width < 8 || area.height < 3 {
        return;
    }

    // vertical: 1 blank top pad · body · 1-line status bar
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1), Constraint::Length(1)])
        .split(area);
    let body = rows[1];

    let w = body.width;
    let show_sidebar = w >= 44;
    let show_detail = app.show_detail && w >= 92;

    let mut constraints: Vec<Constraint> = Vec::new();
    if show_sidebar {
        constraints.push(Constraint::Length(22));
    }
    constraints.push(Constraint::Min(24));
    if show_detail {
        // roughly half the app for the note/detail pane
        constraints.push(Constraint::Percentage(50));
    }
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(constraints)
        .split(body);

    let mut i = 0;
    if show_sidebar {
        render_sidebar(frame, app, cols[i]);
        i += 1;
    }
    render_list(frame, app, cols[i]);
    i += 1;
    if show_detail {
        render_detail(frame, app, cols[i]);
    }
    render_status_bar(frame, app, rows[2]);

    if app.palette.is_some() {
        render_palette(frame, app, area);
    }
}

fn render_sidebar(frame: &mut Frame, app: &App, area: Rect) {
    let g = glyphs();
    let ac = accent();
    // hairline on the right edge only — no full box
    let block = Block::default()
        .borders(Borders::RIGHT)
        .border_style(Style::default().fg(DIM))
        .padding(Padding::horizontal(1));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(vec![
        Span::styled(format!("{} ", g.brand), Style::default().fg(ac)),
        Span::styled("todarchy", Style::default().fg(ac).add_modifier(Modifier::BOLD)),
    ]));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("LISTS", Style::default().fg(ac).add_modifier(Modifier::BOLD))));

    let mut proj_idx = 0usize;
    for id in app.lists() {
        let active = id == app.active_list;
        let open = app
            .store
            .tasks
            .iter()
            .filter(|t| t.list == id && !t.is_done())
            .count();
        // per-project icon + distinct color; inbox uses the accent inbox glyph
        let (icon, icon_color) = if id == "inbox" {
            (g.inbox, ac)
        } else {
            let proj = app.store.projects.iter().find(|p| p.id == id);
            let icon = proj.map(|p| crate::theme::project_icon(&p.icon)).unwrap_or(g.project);
            let color = proj
                .map(|p| crate::theme::project_accent(&p.accent, proj_idx))
                .unwrap_or_else(|| crate::theme::project_color(proj_idx));
            proj_idx += 1;
            (icon, color)
        };
        let label = app.list_label(&id);
        let bar = if active {
            Span::styled(format!("{} ", g.sel), Style::default().fg(ac))
        } else {
            Span::raw("  ")
        };
        let name_style = if active {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Reset)
        };
        lines.push(Line::from(vec![
            bar,
            Span::styled(format!("{icon} "), Style::default().fg(icon_color)),
            Span::styled(label, name_style),
            Span::styled(format!("  {open}"), Style::default().fg(DIM)),
        ]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("CONTEXTS", Style::default().fg(ac).add_modifier(Modifier::BOLD))));
    for (i, ctx) in app.store.contexts.iter().enumerate() {
        let active = &app.ctx_filter == ctx;
        let col = crate::theme::context_color(i);
        let bar = if active {
            Span::styled(format!("{} ", g.sel), Style::default().fg(ac))
        } else {
            Span::raw("  ")
        };
        let mut style = Style::default().fg(col);
        if active {
            style = style.add_modifier(Modifier::BOLD);
        }
        lines.push(Line::from(vec![bar, Span::styled(ctx.clone(), style)]));
    }

    frame.render_widget(Paragraph::new(lines), inner);

    // sync status pinned to the bottom of the sidebar
    let sb = Rect {
        x: inner.x,
        y: inner.y + inner.height.saturating_sub(1),
        width: inner.width,
        height: 1,
    };
    frame.render_widget(Paragraph::new(sync_line(app)), sb);
}

fn sync_line(app: &App) -> Line<'static> {
    let g = glyphs();
    let s = &app.sync;
    if let Some(err) = &s.last_sync_error {
        return Line::from(Span::styled(
            format!("{} {}", g.warn, truncate(err, 18)),
            Style::default().fg(Color::Red),
        ));
    }
    if !s.server_base_url.is_empty() {
        return Line::from(Span::styled(
            format!("{} {}", g.cloud, truncate(&s.server_base_url, 18)),
            Style::default().fg(Color::Green),
        ));
    }
    if !s.folder.is_empty() {
        return Line::from(Span::styled(format!("{} folder sync", g.folder_sync), Style::default().fg(Color::Green)));
    }
    Line::from(Span::styled(format!("{} local only", g.local), Style::default().fg(DIM)))
}

fn render_list(frame: &mut Frame, app: &App, area: Rect) {
    let g = glyphs();
    let ac = accent();
    let rows = app.view();

    let parts = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1), Constraint::Min(1)])
        .split(area);

    // header: list name (left) · mode + count + filters (right)
    let icon = if app.active_list == "inbox" { g.inbox } else { g.project };
    let left = vec![
        Span::raw("  "),
        Span::styled(format!("{icon} "), Style::default().fg(ac)),
        Span::styled(app.list_label(&app.active_list), Style::default().add_modifier(Modifier::BOLD)),
    ];
    let mut right = vec![
        Span::styled(format!("{} ", app.mode_label()), Style::default().fg(ac)),
        Span::styled(format!("· {} open  ", rows.len()), Style::default().fg(DIM)),
    ];
    if !app.ctx_filter.is_empty() {
        let col = crate::theme::context_color(app.context_index(&app.ctx_filter));
        right.push(Span::styled(format!("{} ", app.ctx_filter), Style::default().fg(col)));
    }
    if !app.search.is_empty() {
        right.push(Span::styled(format!("{} {} ", g.search, app.search), Style::default().fg(ac)));
    }
    frame.render_widget(Paragraph::new(justify(left, right, parts[0].width)), parts[0]);
    // hairline under the header
    frame.render_widget(
        Paragraph::new(Span::styled("─".repeat(parts[1].width as usize), Style::default().fg(DIM))),
        parts[1],
    );

    if rows.is_empty() {
        let msg = if app.search.is_empty() && app.ctx_filter.is_empty() {
            "  nothing here — press o to add".to_string()
        } else {
            "  no matches".to_string()
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(msg, Style::default().fg(DIM)))),
            centered_v(parts[2]),
        );
        return;
    }

    let now = model::now_ms();
    let items: Vec<ListItem> = rows
        .iter()
        .enumerate()
        .map(|(idx, r)| {
            let selected = idx == app.cursor;
            let t = app.task(&r.id);
            let mut spans: Vec<Span> = Vec::new();
            // selection bar
            if selected {
                spans.push(Span::styled(format!("{} ", g.sel), Style::default().fg(ac)));
            } else {
                spans.push(Span::raw("  "));
            }
            spans.push(Span::raw("  ".repeat(r.depth as usize)));
            match t {
                Some(t) => {
                    let done = t.is_done();
                    let deferred = t.is_deferred(now);
                    let (box_glyph, box_col) = if done { (g.done, Color::Green) } else { (g.open, DIM) };
                    spans.push(Span::styled(format!("{box_glyph} "), Style::default().fg(box_col)));

                    let mut title_style = if selected {
                        Style::default().add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                    };
                    if done {
                        title_style = Style::default().fg(DIM).add_modifier(Modifier::CROSSED_OUT);
                    } else if deferred {
                        title_style = Style::default().fg(DIM);
                    }
                    spans.push(Span::styled(t.title.clone(), title_style));

                    if r.has_children && r.collapsed {
                        spans.push(Span::styled("  +…", Style::default().fg(ac)));
                    }
                    if !t.ctx.is_empty() {
                        let col = crate::theme::context_color(app.context_index(&t.ctx));
                        spans.push(Span::styled(format!("   {}", t.ctx), Style::default().fg(col)));
                    }
                    if !t.due.is_empty() {
                        let col = match t.due.as_str() {
                            "today" => Color::Red,
                            "tomorrow" => Color::Yellow,
                            _ => Color::Blue,
                        };
                        spans.push(Span::styled(format!("  {} {}", g.due, t.due), Style::default().fg(col)));
                    }
                    if deferred {
                        if let Some(d) = t.defer_until {
                            spans.push(Span::styled(
                                format!("  {} {}", g.defer, model::format_defer_until(d)),
                                Style::default().fg(DIM),
                            ));
                        }
                    }
                }
                None => spans.push(Span::raw(r.id.clone())),
            }
            ListItem::new(Line::from(spans))
        })
        .collect();

    // ListState drives auto-scroll to keep the cursor visible; selection
    // styling is baked into the spans above, so highlight_style is a no-op.
    let list = List::new(items).highlight_style(Style::default());
    let mut state = ListState::default();
    state.select(Some(app.cursor.min(rows.len() - 1)));
    frame.render_stateful_widget(list, parts[2], &mut state);
}

fn render_detail(frame: &mut Frame, app: &App, area: Rect) {
    let g = glyphs();
    let ac = accent();
    let block = Block::default()
        .borders(Borders::LEFT)
        .border_style(Style::default().fg(DIM))
        .padding(Padding::horizontal(2));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let cur = app.current_id().and_then(|id| app.task(&id).cloned());
    let Some(t) = cur else {
        app.detail_max_scroll.set(0);
        frame.render_widget(
            Paragraph::new(Span::styled("no task selected", Style::default().fg(DIM))),
            inner,
        );
        return;
    };

    // ---- fixed metadata block (never scrolls) ----
    let mut meta_lines: Vec<Line> = Vec::new();
    meta_lines.push(Line::from(""));
    meta_lines.push(Line::from(Span::styled(t.title.clone(), Style::default().fg(ac).add_modifier(Modifier::BOLD))));
    meta_lines.push(Line::from(""));
    let list_icon = if t.list == "inbox" { g.inbox } else { g.project };
    meta_lines.push(meta(format!("{list_icon}  {}", app.list_label(&t.list))));
    if !t.ctx.is_empty() {
        let col = crate::theme::context_color(app.context_index(&t.ctx));
        meta_lines.push(Line::from(Span::styled(format!("{}  {}", g.context, t.ctx), Style::default().fg(col))));
    }
    if !t.due.is_empty() {
        meta_lines.push(Line::from(Span::styled(format!("{}  due {}", g.due, t.due), Style::default().fg(Color::Yellow))));
    }
    if let Some(d) = t.defer_until {
        meta_lines.push(Line::from(Span::styled(
            format!("{}  {}", g.defer, model::format_defer_until(d)),
            Style::default().fg(DIM),
        )));
    }
    if t.created > 0 {
        meta_lines.push(meta(format!("created {} ago", model::time_ago(t.created))));
    }
    meta_lines.push(Line::from(""));
    meta_lines.push(Line::from(Span::styled("NOTE", Style::default().fg(ac).add_modifier(Modifier::BOLD))));

    let meta_h = (meta_lines.len() as u16).min(inner.height.saturating_sub(2));
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(meta_h), Constraint::Min(1)])
        .split(inner);
    frame.render_widget(Paragraph::new(meta_lines), rows[0]);

    // ---- scrollable region: markdown note + comments ----
    let note_area = rows[1];
    let mut content: Vec<Line> = if t.note.trim().is_empty() {
        vec![Line::from(Span::styled("empty — press c to edit in $EDITOR", Style::default().fg(DIM)))]
    } else {
        markdown::render(&t.note, ac)
    };

    let comments = model::task_comments(&t);
    content.push(Line::from(""));
    content.push(Line::from(Span::styled(
        format!("COMMENTS ({})", comments.len()),
        Style::default().fg(ac).add_modifier(Modifier::BOLD),
    )));
    content.push(Line::from(""));
    if comments.is_empty() {
        content.push(Line::from(Span::styled("none yet — press C to add", Style::default().fg(DIM))));
    } else {
        for c in &comments {
            content.push(Line::from(vec![
                Span::styled(c.author.clone(), Style::default().fg(CTX)),
                Span::styled(format!("  · {} ago", model::time_ago(c.created)), Style::default().fg(DIM)),
            ]));
            for line in c.text.split('\n') {
                content.push(Line::from(line.to_string()));
            }
            content.push(Line::from(""));
        }
    }

    let total_full = Paragraph::new(content.clone())
        .wrap(Wrap { trim: false })
        .line_count(note_area.width) as u16;

    if total_full > note_area.height {
        // reserve one column on the right for the scrollbar
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(1), Constraint::Length(1)])
            .split(note_area);
        let text_area = cols[0];
        let para = Paragraph::new(content).wrap(Wrap { trim: false });
        let total = para.line_count(text_area.width) as u16;
        let max = total.saturating_sub(text_area.height);
        app.detail_max_scroll.set(max);
        let offset = app.detail_scroll.min(max);
        frame.render_widget(para.scroll((offset, 0)), text_area);

        let mut sb = ScrollbarState::new(total as usize)
            .position(offset as usize)
            .viewport_content_length(text_area.height as usize);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .thumb_style(Style::default().fg(ac))
                .track_style(Style::default().fg(DIM)),
            cols[1],
            &mut sb,
        );
    } else {
        app.detail_max_scroll.set(0);
        frame.render_widget(Paragraph::new(content).wrap(Wrap { trim: false }), note_area);
    }
}

fn render_status_bar(frame: &mut Frame, app: &App, area: Rect) {
    let g = glyphs();
    let ac = accent();
    if let Some(prompt) = &app.prompt {
        let mut spans = vec![
            Span::styled(format!(" {} ", prompt.kind.label()), Style::default().fg(ac).add_modifier(Modifier::BOLD)),
            Span::styled("› ", Style::default().fg(DIM)),
            Span::raw(prompt.buf.clone()),
            Span::styled("▏", Style::default().fg(ac)),
        ];
        if let Some(err) = &prompt.err {
            spans.push(Span::styled(format!("   {} {err}", g.warn), Style::default().fg(Color::Red)));
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
        return;
    }
    if let Some((at, msg)) = &app.toast {
        if at.elapsed().as_millis() < 1600 {
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(format!(" {} ", g.sel), Style::default().fg(ac)),
                    Span::styled(msg.clone(), Style::default().fg(ac)),
                ])),
                area,
            );
            return;
        }
    }
    let hint = format!(
        " j/k move · o add · x done · d defer · Tab nest · {} palette · q quit",
        g.palette
    );
    frame.render_widget(Paragraph::new(Span::styled(hint, Style::default().fg(DIM))), area);
}

fn render_palette(frame: &mut Frame, app: &App, area: Rect) {
    let g = glyphs();
    let ac = accent();
    let width = area.width.min(74).max(40);
    let height = area.height.min(20).max(6);
    let rect = Rect {
        x: (area.width.saturating_sub(width)) / 2,
        y: area.height / 8,
        width,
        height,
    };
    frame.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(ac))
        .padding(Padding::horizontal(1))
        .title(Span::styled(format!(" {} palette ", g.palette), Style::default().fg(ac)));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let parts = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1), Constraint::Length(1), Constraint::Min(1)])
        .split(inner);

    let q = app.palette.as_ref().map(|p| p.q.clone()).unwrap_or_default();
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(format!("{} ", g.search), Style::default().fg(ac)),
            Span::raw(q),
            Span::styled("▏", Style::default().fg(ac)),
        ])),
        parts[0],
    );
    frame.render_widget(
        Paragraph::new(Span::styled(
            "↑↓ navigate · ↵ run · esc close · type : for commands only",
            Style::default().fg(DIM),
        )),
        parts[1],
    );
    frame.render_widget(
        Paragraph::new(Span::styled("─".repeat(parts[2].width as usize), Style::default().fg(DIM))),
        parts[2],
    );

    let items = app.palette_items();
    let sel = app.palette.as_ref().map(|p| p.sel).unwrap_or(0);
    let commands = app.commands();
    let list_items: Vec<ListItem> = items
        .iter()
        .enumerate()
        .map(|(idx, item)| {
            let bar = if idx == sel {
                Span::styled(format!("{} ", g.sel), Style::default().fg(ac))
            } else {
                Span::raw("  ")
            };
            match item {
                PaletteItem::Command(cmd_id) => {
                    let cmd = commands.iter().find(|c| &c.id == cmd_id);
                    let mut spans = vec![bar];
                    match cmd {
                        Some(c) => {
                            spans.push(Span::raw(c.title.clone()));
                            if !c.hint.is_empty() {
                                spans.push(Span::styled(format!("  — {}", c.hint), Style::default().fg(DIM)));
                            }
                            if !c.keys.is_empty() {
                                spans.push(Span::styled(format!("   {}", c.keys.join(" ")), Style::default().fg(ac)));
                            }
                        }
                        None => spans.push(Span::raw(cmd_id.clone())),
                    }
                    ListItem::new(Line::from(spans))
                }
                PaletteItem::Task { id, .. } => {
                    let title = app.task(id).map(|t| t.title.clone()).unwrap_or_default();
                    ListItem::new(Line::from(vec![
                        bar,
                        Span::styled("task  ", Style::default().fg(DIM)),
                        Span::raw(title),
                    ]))
                }
            }
        })
        .collect();

    let list = List::new(list_items).highlight_style(Style::default());
    let mut state = ListState::default();
    if !items.is_empty() {
        state.select(Some(sel.min(items.len() - 1)));
    }
    frame.render_stateful_widget(list, parts[3], &mut state);
}

// ---------- helpers ----------

fn meta(text: String) -> Line<'static> {
    Line::from(Span::styled(text, Style::default().fg(DIM)))
}

/// Left-justify `left`, right-justify `right`, padding between to `width`.
fn justify(left: Vec<Span<'static>>, right: Vec<Span<'static>>, width: u16) -> Line<'static> {
    let lw: usize = left.iter().map(|s| s.content.chars().count()).sum();
    let rw: usize = right.iter().map(|s| s.content.chars().count()).sum();
    let pad = (width as usize).saturating_sub(lw + rw);
    let mut spans = left;
    spans.push(Span::raw(" ".repeat(pad)));
    spans.extend(right);
    Line::from(spans)
}

/// A one-line rect vertically centered in `area` (for empty states).
fn centered_v(area: Rect) -> Rect {
    Rect {
        x: area.x,
        y: area.y + area.height / 2,
        width: area.width,
        height: 1,
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let t: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{t}…")
    }
}

#[cfg(test)]
mod snapshot {
    use super::*;
    use crate::app::App;
    use crate::model::{Store, Task};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use serde_json::Map;
    use todarchy_core::SyncStatus;

    fn task(id: &str, title: &str, ctx: &str, due: &str) -> Task {
        Task {
            id: id.into(),
            list: "inbox".into(),
            title: title.into(),
            ctx: ctx.into(),
            due: due.into(),
            note: String::new(),
            parent: None,
            created: model::now_ms(),
            done_at: None,
            defer_until: None,
            pos: Some(model::now_ms() as f64),
            rest: Map::new(),
        }
    }

    // Renders a representative frame and prints it. Run with:
    //   cargo test -p todarchy-tui render_snapshot -- --nocapture
    #[test]
    fn render_snapshot() {
        std::env::set_var("TODARCHY_ASCII", "1"); // readable in plain stdout
        let mut done = task("d", "ship v0.3", "@work", "");
        done.done_at = Some(model::now_ms());
        let mut milk = task("a", "buy milk", "@errands", "today");
        milk.note = "# Shopping\n\nGet **oat milk**, not `2%`.\n\n- barista blend\n- unsweetened\n\n> check the fridge first".into();
        milk.rest.insert(
            "comments".into(),
            serde_json::json!({
                "c1": {"id": "c1", "author": "Ryan", "text": "grabbed the oat milk already", "createdAt": model::now_ms() - 3_600_000}
            }),
        );
        let proj = |id: &str, name: &str, icon: &str| crate::model::Project {
            id: id.into(),
            name: name.into(),
            icon: icon.into(),
            accent: String::new(),
            rest: serde_json::Map::new(),
        };
        let store = Store {
            tasks: vec![
                milk,
                task("b", "call dentist", "@phone", ""),
                task("c", "review PR #418", "@work", "tomorrow"),
                done,
            ],
            projects: vec![
                proj("p_work", "work", "briefcase"),
                proj("p_home", "home", "home"),
                proj("p_groc", "groceries", "cart"),
            ],
            contexts: crate::model::default_contexts(),
        };
        let mut app = App::new(store, SyncStatus {
            folder: String::new(),
            last_synced_at: None,
            last_sync_error: None,
            server_base_url: String::new(),
            server_main_doc_id: String::new(),
        });
        app.show_done = true;

        let mut terminal = Terminal::new(TestBackend::new(96, 22)).unwrap();
        terminal.draw(|f| render(f, &app)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let mut out = String::from("\n");
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        println!("{out}");
    }

    #[test]
    fn long_note_becomes_scrollable() {
        std::env::set_var("TODARCHY_ASCII", "1");
        let mut t = task("a", "big note", "@work", "");
        t.note = (1..=40).map(|i| format!("line {i} of the note")).collect::<Vec<_>>().join("\n");
        let store = Store { tasks: vec![t], projects: vec![], contexts: crate::model::default_contexts() };
        let mut app = App::new(store, SyncStatus {
            folder: String::new(), last_synced_at: None, last_sync_error: None,
            server_base_url: String::new(), server_main_doc_id: String::new(),
        });
        app.show_detail = true;

        let mut terminal = Terminal::new(TestBackend::new(100, 20)).unwrap();
        terminal.draw(|f| render(f, &app)).unwrap();
        // renderer must have measured overflow → a positive max scroll
        assert!(app.detail_max_scroll.get() > 0, "note should overflow and be scrollable");

        // scrolling is clamped to that max
        app.scroll_detail(1000);
        assert_eq!(app.detail_scroll, app.detail_max_scroll.get());
        app.scroll_detail(-1000);
        assert_eq!(app.detail_scroll, 0);
    }
}
