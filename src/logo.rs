//! Startup art. Three variants are kept; `BANNER` selects the one in use.

/// Three lines, narrow enough to sit above a firmware log without pushing
/// anything off screen. This is what a session starts with.
pub const COMPACT: &str = "\
╔╦╗╔═╗╔═╗╔╦╗   serial, in color
║║║║  ║ ║║║║   ─┐ ┌─┐ ┌───┐ ┌──
╩ ╩╚═╝╚═╝╩ ╩    └─┘ └─┘   └─┘";

/// Block lettering over a bit stream. Too tall to print on every connect, but
/// it reads well in documentation and release notes.
#[allow(dead_code)]
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
#[allow(dead_code)]
pub const CONNECTOR: &str = "\
    ╭─────────────╮
 ═══┥ ● ● ● ● ● ● ┝═══   m c o m
    ╰─────────────╯      115200 8N1";

pub const BANNER: &str = COMPACT;

#[cfg(test)]
mod tests {
    use super::*;

    fn widest(art: &str) -> usize {
        art.lines().map(|l| l.chars().count()).max().unwrap_or(0)
    }

    #[test]
    fn the_startup_art_stays_small() {
        // An 80x24 terminal is the floor, and the art must not crowd out the
        // log it sits above.
        assert!(BANNER.lines().count() <= 3);
        assert!(widest(BANNER) <= 60, "{} columns", widest(BANNER));
    }

    #[test]
    fn every_variant_fits_a_standard_terminal() {
        for art in [COMPACT, BLOCK, CONNECTOR] {
            assert!(!art.is_empty());
            assert!(widest(art) <= 80, "{} columns", widest(art));
        }
    }
}
