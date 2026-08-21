//! Startup art. Three styles are kept; `STYLE` picks the one a session prints.

// The unused styles are selected by editing `STYLE`, not by any call site.
#[allow(dead_code)]
pub enum Style {
    Compact,
    Block,
    Connector,
}

/// Compact is the only style that fits above a firmware log without costing
/// real screen space.
pub const STYLE: Style = Style::Compact;

/// Letter cells, kept apart so the glyphs read as `mcom` rather than as four
/// identical rectangles.
const LETTERS: [[&str; 4]; 3] = [
    ["╔╦╗", "╔═╗", "╔═╗", "╔╦╗"],
    ["║║║", "║  ", "║ ║", "║║║"],
    ["╩ ╩", "╚═╝", "╚═╝", "╩ ╩"],
];

/// Blue drifting into teal. Mid-tone on purpose: these stay legible on a light
/// terminal as well as a dark one, which the bright end of the palette does not.
const LETTER_COLORS: [&str; 4] = ["38;5;33", "38;5;38", "38;5;37", "38;5;30"];
const DIM: &str = "38;5;245";

const WAVE_TOP: &str = "─┐ ┌─┐ ┌───┐ ┌──";
const WAVE_BOTTOM: &str = " └─┘ └─┘   └─┘";

/// Block lettering over a bit stream. Eight lines is too many to print on every
/// connect, but it reads well in documentation and in release notes.
pub const BLOCK: &str = "\
 ███╗   ███╗ ██████╗ ██████╗ ███╗   ███╗
 ████╗ ████║██╔════╝██╔═══██╗████╗ ████║
 ██╔████╔██║██║     ██║   ██║██╔████╔██║
 ██║╚██╔╝██║██║     ██║   ██║██║╚██╔╝██║
 ██║ ╚═╝ ██║╚██████╗╚██████╔╝██║ ╚═╝ ██║
 ╚═╝     ╚═╝ ╚═════╝ ╚═════╝ ╚═╝     ╚═╝
  ─┐ ┌─┐ ┌───┐ ┌─┐ ┌──────┐ ┌─┐ ┌──  115200 8N1
   └─┘ └─┘   └─┘ └─┘      └─┘ └─┘";

/// A connector and its cable, with no lettering at all.
pub const CONNECTOR: &str = "\
    ╭─────────────╮
 ═══┥ ● ● ● ● ● ● ┝═══   m c o m
    ╰─────────────╯      115200 8N1";

/// The art as it goes to the terminal: CRLF line endings for raw mode, and a
/// blank line above and below so it is not crushed against the shell prompt.
pub fn banner() -> String {
    let art = match STYLE {
        Style::Compact => compact(),
        Style::Block => plain(BLOCK),
        Style::Connector => plain(CONNECTOR),
    };
    format!("\r\n{art}\r\n")
}

fn compact() -> String {
    let mut out = String::new();
    for (row, cells) in LETTERS.iter().enumerate() {
        for (i, cell) in cells.iter().enumerate() {
            out.push_str(&format!("\x1b[{}m{cell}\x1b[0m ", LETTER_COLORS[i]));
        }
        out.push_str("  ");
        out.push_str(&match row {
            0 => tagline(),
            1 => format!("\x1b[{DIM}m{WAVE_TOP}\x1b[0m"),
            _ => format!("\x1b[{DIM}m{WAVE_BOTTOM}\x1b[0m"),
        });
        out.push_str("\r\n");
    }
    out
}

/// "in color" is a claim, so the word carries the letter palette to back it up.
fn tagline() -> String {
    let mut out = format!("\x1b[{DIM}mserial, in \x1b[0m");
    for (i, c) in "color".chars().enumerate() {
        out.push_str(&format!(
            "\x1b[{}m{c}\x1b[0m",
            LETTER_COLORS[i % LETTER_COLORS.len()]
        ));
    }
    out
}

fn plain(art: &str) -> String {
    art.lines()
        .map(|l| format!("\x1b[{}m{l}\x1b[0m\r\n", LETTER_COLORS[1]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ansi;

    fn stripped(s: &str) -> Vec<String> {
        String::from_utf8_lossy(&ansi::strip(s.as_bytes()))
            .replace('\r', "")
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn widest(lines: &[String]) -> usize {
        lines.iter().map(|l| l.chars().count()).max().unwrap_or(0)
    }

    #[test]
    fn the_banner_has_air_above_and_below() {
        let lines = stripped(&banner());
        assert_eq!(lines.first().map(String::as_str), Some(""));
        assert_eq!(lines.last().map(String::as_str), Some(""));
        assert_eq!(lines.len(), 5, "{lines:#?}");
    }

    #[test]
    fn letters_are_spaced_apart() {
        let lines = stripped(&banner());
        assert!(lines[1].starts_with("╔╦╗ ╔═╗ ╔═╗ ╔╦╗"), "{:?}", lines[1]);
    }

    #[test]
    fn the_banner_fits_a_narrow_terminal() {
        assert!(widest(&stripped(&banner())) <= 60);
    }

    #[test]
    fn every_style_fits_a_standard_terminal() {
        for art in [BLOCK, CONNECTOR] {
            let lines = stripped(art);
            assert!(!lines.is_empty());
            assert!(widest(&lines) <= 80, "{} columns", widest(&lines));
        }
    }

    #[test]
    fn the_tagline_is_actually_colored() {
        // The word "color" must not be the same flat run as the rest.
        let t = tagline();
        assert!(t.contains(LETTER_COLORS[0]));
        assert!(t.contains(LETTER_COLORS[1]));
        assert_eq!(stripped(&t)[0], "serial, in color");
    }
}
