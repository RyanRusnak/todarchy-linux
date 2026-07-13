// markdown.rs — render a markdown note into styled ratatui lines.
//
// Everything is expressed in ANSI/indexed colors + the app accent, so the
// output recolors with whatever Omarchy theme the terminal is running:
// headings and links use the accent, code uses the theme's green, blockquotes
// and rules use the dim (bright-black) slot. No hardcoded RGB.

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

const DIM: Color = Color::DarkGray;
const CODE: Color = Color::Green;

/// Parse `md` and return themed lines ready to drop into a Paragraph.
pub fn render(md: &str, accent: Color) -> Vec<Line<'static>> {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    let parser = Parser::new_ext(md, opts);

    let mut r = Renderer::new(accent);
    for ev in parser {
        r.event(ev);
    }
    r.finish()
}

struct Renderer {
    accent: Color,
    lines: Vec<Line<'static>>,
    cur: Vec<Span<'static>>,
    bold: i32,
    italic: i32,
    strike: i32,
    inline_code: bool,
    link: bool,
    heading: Option<HeadingLevel>,
    // each open list level: Some(next ordinal) for ordered, None for bullets
    lists: Vec<Option<u64>>,
    in_item: bool,
    quote: i32,
    in_code_block: bool,
}

impl Renderer {
    fn new(accent: Color) -> Self {
        Renderer {
            accent,
            lines: Vec::new(),
            cur: Vec::new(),
            bold: 0,
            italic: 0,
            strike: 0,
            inline_code: false,
            link: false,
            heading: None,
            lists: Vec::new(),
            in_item: false,
            quote: 0,
            in_code_block: false,
        }
    }

    /// Style for the current inline context.
    fn inline_style(&self) -> Style {
        let mut s = Style::default();
        if let Some(level) = self.heading {
            // hierarchy by weight, not by visible '#' marks
            return match level {
                HeadingLevel::H1 => Style::default()
                    .fg(self.accent)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
                HeadingLevel::H2 => Style::default().fg(self.accent).add_modifier(Modifier::BOLD),
                _ => Style::default().add_modifier(Modifier::BOLD),
            };
        }
        if self.inline_code {
            s = s.fg(CODE);
        } else if self.link {
            s = s.fg(self.accent).add_modifier(Modifier::UNDERLINED);
        } else if self.quote > 0 {
            s = s.fg(DIM).add_modifier(Modifier::ITALIC);
        }
        if self.bold > 0 {
            s = s.add_modifier(Modifier::BOLD);
        }
        if self.italic > 0 {
            s = s.add_modifier(Modifier::ITALIC);
        }
        if self.strike > 0 {
            s = s.add_modifier(Modifier::CROSSED_OUT);
        }
        s
    }

    /// Prefix a freshly-started line with blockquote bars / list indent.
    fn line_prefix(&mut self) {
        for _ in 0..self.quote {
            self.cur.push(Span::styled("▏ ".to_string(), Style::default().fg(self.accent)));
        }
        if self.in_item {
            let depth = self.lists.len().saturating_sub(1);
            if depth > 0 {
                self.cur.push(Span::raw("  ".repeat(depth)));
            }
        }
    }

    fn flush(&mut self) {
        let spans = std::mem::take(&mut self.cur);
        self.lines.push(Line::from(spans));
    }

    fn blank(&mut self) {
        // collapse consecutive blanks
        if self.lines.last().map(|l| l.spans.is_empty()).unwrap_or(false) {
            return;
        }
        self.lines.push(Line::from(""));
    }

    fn event(&mut self, ev: Event) {
        match ev {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(t) => self.text(&t),
            Event::Code(t) => {
                // color-only; padding would mangle adjacent punctuation
                self.cur.push(Span::styled(t.to_string(), Style::default().fg(CODE)));
            }
            Event::SoftBreak | Event::HardBreak => {
                self.flush();
                self.line_prefix();
            }
            Event::Rule => {
                self.blank();
                self.lines.push(Line::from(Span::styled(
                    "─".repeat(24),
                    Style::default().fg(DIM),
                )));
                self.blank();
            }
            _ => {}
        }
    }

    fn start(&mut self, tag: Tag) {
        match tag {
            Tag::Paragraph => {
                if !self.in_item {
                    self.line_prefix();
                }
            }
            Tag::Heading { level, .. } => {
                self.blank();
                self.heading = Some(level);
            }
            Tag::BlockQuote(_) => {
                self.quote += 1;
            }
            Tag::CodeBlock(_) => {
                self.in_code_block = true;
                self.blank();
            }
            Tag::List(start) => {
                self.lists.push(start);
            }
            Tag::Item => {
                self.in_item = true;
                self.line_prefix();
                let marker = match self.lists.last_mut() {
                    Some(Some(n)) => {
                        let m = format!("{n}. ");
                        *n += 1;
                        m
                    }
                    _ => "• ".to_string(),
                };
                self.cur.push(Span::styled(marker, Style::default().fg(self.accent)));
            }
            Tag::Emphasis => self.italic += 1,
            Tag::Strong => self.bold += 1,
            Tag::Strikethrough => self.strike += 1,
            Tag::Link { .. } => self.link = true,
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                if !self.in_item {
                    self.flush();
                    self.blank();
                }
            }
            TagEnd::Heading(_) => {
                self.heading = None;
                self.flush();
                self.blank();
            }
            TagEnd::BlockQuote(_) => {
                self.quote -= 1;
                self.blank();
            }
            TagEnd::CodeBlock => {
                self.in_code_block = false;
                self.blank();
            }
            TagEnd::List(_) => {
                self.lists.pop();
                if self.lists.is_empty() {
                    self.blank();
                }
            }
            TagEnd::Item => {
                self.in_item = false;
                self.flush();
            }
            TagEnd::Emphasis => self.italic -= 1,
            TagEnd::Strong => self.bold -= 1,
            TagEnd::Strikethrough => self.strike -= 1,
            TagEnd::Link => self.link = false,
            _ => {}
        }
    }

    fn text(&mut self, t: &str) {
        if self.in_code_block {
            for (i, line) in t.split('\n').enumerate() {
                if i > 0 {
                    self.flush();
                }
                // don't emit a trailing empty line from the final newline
                if line.is_empty() && self.cur.is_empty() {
                    self.cur.push(Span::styled("  ".to_string(), Style::default().fg(CODE)));
                    continue;
                }
                self.cur.push(Span::styled(
                    format!("  {line}"),
                    Style::default().fg(CODE),
                ));
            }
            return;
        }
        let style = self.inline_style();
        self.cur.push(Span::styled(t.to_string(), style));
    }

    fn finish(mut self) -> Vec<Line<'static>> {
        if !self.cur.is_empty() {
            self.flush();
        }
        // trim a trailing blank line
        while self.lines.last().map(|l| l.spans.is_empty()).unwrap_or(false) {
            self.lines.pop();
        }
        self.lines
    }
}

