// theme.rs — the visual vocabulary: icon set + accent color.
//
// Colors are still ANSI/indexed so the terminal's Omarchy theme paints
// everything; the only "choice" we make is which ANSI slot is the accent
// (selection bar, brand, mode chip, palette frame). It defaults to magenta
// — distinct from the semantic colors below — and is overridable with
// TODARCHY_ACCENT (a color name or 0-255 index).
//
// Icons default to Nerd Font glyphs (Omarchy ships CaskaydiaMono Nerd Font).
// Set TODARCHY_ASCII=1 to fall back to plain, widely-supported Unicode.

use std::sync::OnceLock;

use ratatui::style::Color;

pub struct Glyphs {
    pub inbox: &'static str,
    pub project: &'static str,
    pub context: &'static str,
    pub done: &'static str,
    pub open: &'static str,
    pub due: &'static str,
    pub defer: &'static str,
    pub cloud: &'static str,
    pub folder_sync: &'static str,
    pub local: &'static str,
    pub warn: &'static str,
    pub search: &'static str,
    pub brand: &'static str,
    pub palette: &'static str,
    /// Left bar drawn on the selected row.
    pub sel: &'static str,
}

// Nerd Font (Private Use Area) glyphs.
const NERD: Glyphs = Glyphs {
    inbox: "\u{f01c}",
    project: "\u{f07b}",
    context: "\u{f02b}",
    done: "\u{f058}",
    open: "\u{f10c}",
    due: "\u{f073}",
    defer: "\u{f252}",
    cloud: "\u{f0c2}",
    folder_sync: "\u{f021}",
    local: "\u{f111}",
    warn: "\u{f071}",
    search: "\u{f002}",
    brand: "\u{f0ae}",
    palette: "\u{f120}",
    sel: "▎",
};

// Plain-Unicode fallback — safe in any font.
const ASCII: Glyphs = Glyphs {
    inbox: "•",
    project: "◦",
    context: "@",
    done: "✓",
    open: "○",
    due: "!",
    defer: "~",
    cloud: "↑",
    folder_sync: "⟳",
    local: "○",
    warn: "⚠",
    search: "/",
    brand: "≡",
    palette: ":",
    sel: "▎",
};

pub fn ascii() -> bool {
    static A: OnceLock<bool> = OnceLock::new();
    *A.get_or_init(|| {
        std::env::var("TODARCHY_ASCII")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    })
}

pub fn glyphs() -> &'static Glyphs {
    if ascii() {
        &ASCII
    } else {
        &NERD
    }
}

/// A stable, distinct ANSI color per project (by position in the list) so the
/// sidebar reads at a glance. Stays theme-native — these are the terminal's
/// own palette slots, not fixed RGB.
pub fn project_color(index: usize) -> Color {
    const P: [Color; 6] = [
        Color::Cyan,
        Color::Green,
        Color::Yellow,
        Color::Blue,
        Color::Magenta,
        Color::Red,
    ];
    P[index % P.len()]
}

/// A distinct ANSI color per context, by its position in the contexts list —
/// so each `@context` reads at a glance, stays consistent across devices (the
/// list order syncs), and recolors with the terminal's Omarchy theme. Magenta
/// is skipped (reserved as the app accent).
pub fn context_color(index: usize) -> Color {
    const P: [Color; 8] = [
        Color::Cyan,
        Color::Green,
        Color::Yellow,
        Color::Blue,
        Color::Red,
        Color::LightMagenta,
        Color::LightCyan,
        Color::LightGreen,
    ];
    P[index % P.len()]
}

/// Map a project's icon to a Nerd Font glyph. Handles both SF Symbols names
/// as set on iOS/macOS (e.g. `house.fill`, `briefcase.fill`, `person.2.fill`)
/// and plain keywords; falls back to a folder. ASCII mode uses the plain
/// project marker.
pub fn project_icon(name: &str) -> &'static str {
    if ascii() {
        return ASCII.project;
    }
    // SF Symbols style: strip modifier suffixes (.fill/.circle/…) — take the
    // base component before the first dot.
    let base = name.trim().to_lowercase();
    let base = base.split('.').next().unwrap_or("");
    match base {
        "briefcase" | "work" | "job" => "\u{f0b1}",
        "house" | "home" => "\u{f015}",
        "star" | "sparkles" | "wedding" => "\u{f005}",
        "cart" | "bag" | "shopping" | "groceries" => "\u{f07a}",
        "heart" | "health" => "\u{f004}",
        "book" | "books" | "text" | "research" | "reading" | "study" | "doc" => "\u{f02d}",
        "flask" | "science" | "experiment" => "\u{f0c3}",
        "person" | "people" | "figure" | "team" | "family" | "users" => "\u{f0c0}",
        "calendar" | "event" | "birthday" | "party" => "\u{f073}",
        "music" | "guitars" | "pianokeys" | "drum" => "\u{f001}",
        "hammer" | "wrench" | "gearshape" | "gear" => "\u{f0ad}",
        "paintbrush" | "paintpalette" | "pencil" | "design" => "\u{f1fc}",
        "dumbbell" | "fitness" | "sportscourt" => "\u{f44b}",
        "fork" | "cup" | "takeoutbag" | "food" => "\u{f2e7}",
        "airplane" | "travel" => "\u{f072}",
        "car" | "bus" => "\u{f1b9}",
        "gift" => "\u{f06b}",
        "graduationcap" | "school" => "\u{f19d}",
        "dollarsign" | "creditcard" | "banknote" | "money" => "\u{f155}",
        "code" | "chevron" | "terminal" | "dev" | "project" => "\u{f121}",
        "cross" | "pills" | "medical" => "\u{f0fa}",
        _ => "\u{f07b}", // folder
    }
}

/// Resolve a project's color: its explicit accent (a `#rrggbb`/`#rgb` hex set
/// on iOS/macOS) rendered as truecolor so the sidebar matches your other
/// devices, else a stable ANSI slot by position (theme-native fallback).
pub fn project_accent(accent: &str, index: usize) -> Color {
    parse_hex(accent).unwrap_or_else(|| project_color(index))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accent_uses_hex_when_present() {
        assert_eq!(project_accent("#7aa2f7", 0), Color::Rgb(0x7a, 0xa2, 0xf7));
        assert_eq!(project_accent("#7dcfff", 3), Color::Rgb(0x7d, 0xcf, 0xff));
        // 3-digit shorthand expands
        assert_eq!(project_accent("#abc", 0), Color::Rgb(0xaa, 0xbb, 0xcc));
    }

    #[test]
    fn context_colors_are_distinct_for_first_several() {
        let n = 8;
        let mut seen = std::collections::HashSet::new();
        for i in 0..n {
            assert!(seen.insert(format!("{:?}", context_color(i))), "collision at {i}");
        }
    }

    #[test]
    fn accent_falls_back_to_ansi_rotation() {
        // no/invalid hex → the stable per-index ANSI slot
        assert_eq!(project_accent("", 0), project_color(0));
        assert_eq!(project_accent("var(--cyan)", 2), project_color(2));
    }
}

fn parse_hex(s: &str) -> Option<Color> {
    let h = s.trim().strip_prefix('#')?;
    let (r, g, b) = match h.len() {
        6 => (
            u8::from_str_radix(&h[0..2], 16).ok()?,
            u8::from_str_radix(&h[2..4], 16).ok()?,
            u8::from_str_radix(&h[4..6], 16).ok()?,
        ),
        3 => {
            let d = |c: &str| u8::from_str_radix(c, 16).ok().map(|v| v * 17);
            (d(&h[0..1])?, d(&h[1..2])?, d(&h[2..3])?)
        }
        _ => return None,
    };
    Some(Color::Rgb(r, g, b))
}

pub fn accent() -> Color {
    static A: OnceLock<Color> = OnceLock::new();
    *A.get_or_init(|| {
        std::env::var("TODARCHY_ACCENT")
            .ok()
            .and_then(|s| parse_color(&s))
            .unwrap_or(Color::Magenta)
    })
}

fn parse_color(s: &str) -> Option<Color> {
    let s = s.trim().to_lowercase();
    Some(match s.as_str() {
        "red" => Color::Red,
        "green" => Color::Green,
        "yellow" => Color::Yellow,
        "blue" => Color::Blue,
        "magenta" | "purple" | "pink" => Color::Magenta,
        "cyan" => Color::Cyan,
        "white" => Color::White,
        "gray" | "grey" | "darkgray" | "darkgrey" => Color::DarkGray,
        _ => return s.parse::<u8>().ok().map(Color::Indexed),
    })
}
