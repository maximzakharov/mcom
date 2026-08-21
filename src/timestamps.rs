use std::time::{Duration, Instant};

use chrono::{DateTime, Local};

use crate::cli::TsMode;
use crate::scrollback::Line;

#[derive(Debug, Default)]
pub struct Feed {
    /// Bytes to write to the terminal, timestamp prefixes included.
    pub out: Vec<u8>,
    /// Lines completed by this chunk.
    pub lines: Vec<Line>,
}

/// Splits the incoming byte stream into lines and injects timestamp prefixes
/// without disturbing the escape sequences the device emits.
/// Where a line stands inside an escape sequence, so escapes can be told from
/// anything the reader would actually see.
#[derive(Copy, Clone, PartialEq, Eq)]
enum Esc {
    None,
    Start,
    Csi,
    Osc,
    OscTerminator,
}

fn advance(state: Esc, b: u8) -> Esc {
    match state {
        Esc::None if b == 0x1b => Esc::Start,
        Esc::None => Esc::None,
        Esc::Start => match b {
            b'[' => Esc::Csi,
            b']' | b'P' | b'X' | b'^' | b'_' => Esc::Osc,
            _ => Esc::None,
        },
        Esc::Csi if (0x40..=0x7e).contains(&b) => Esc::None,
        Esc::Csi => Esc::Csi,
        Esc::Osc => match b {
            0x07 => Esc::None,
            0x1b => Esc::OscTerminator,
            _ => Esc::Osc,
        },
        Esc::OscTerminator if b == b'\\' => Esc::None,
        Esc::OscTerminator => Esc::Osc,
    }
}

pub struct LineAssembler {
    cur: Vec<u8>,
    at_line_start: bool,
    pending_cr: bool,
    /// Escape bytes seen before the line showed anything. Held back so the
    /// timestamp can still be printed ahead of them, keeping the device's
    /// colors applied to its own text and not to our prefix.
    pending_esc: Vec<u8>,
    esc: Esc,
    start: Instant,
    prev_line: Instant,
    line_started: Option<(DateTime<Local>, Instant)>,
}

impl LineAssembler {
    pub fn new() -> Self {
        let now = Instant::now();
        LineAssembler {
            cur: Vec::with_capacity(256),
            at_line_start: true,
            pending_cr: false,
            pending_esc: Vec::new(),
            esc: Esc::None,
            start: now,
            prev_line: now,
            line_started: None,
        }
    }

    pub fn partial(&self) -> &[u8] {
        &self.cur
    }

    /// True when the device has written something it has not terminated yet.
    pub fn mid_line(&self) -> bool {
        !self.at_line_start || self.pending_cr
    }

    pub fn feed(&mut self, data: &[u8], ts: TsMode) -> Feed {
        let mut f = Feed::default();
        f.out.reserve(data.len() + 16);

        for &b in data {
            if self.pending_cr {
                self.pending_cr = false;
                if b == b'\n' {
                    self.release_escapes(&mut f);
                    f.out.extend_from_slice(b"\r\n");
                    self.finish_line(&mut f);
                    continue;
                }
                // Lone CR: the device is redrawing the current line in place.
                self.release_escapes(&mut f);
                f.out.push(b'\r');
                self.cur.clear();
                self.at_line_start = true;
                self.line_started = None;
            }

            match b {
                b'\r' => self.pending_cr = true,
                b'\n' => {
                    // Raw mode has no ONLCR, so carriage returns are ours to add.
                    self.release_escapes(&mut f);
                    f.out.extend_from_slice(b"\r\n");
                    self.finish_line(&mut f);
                }
                _ => {
                    let inside_escape = self.esc != Esc::None || b == 0x1b;
                    self.esc = advance(self.esc, b);
                    if self.at_line_start && inside_escape {
                        // Nothing visible yet: a line that turns out to hold
                        // only escapes gets no timestamp at all.
                        self.pending_esc.push(b);
                        self.cur.push(b);
                        continue;
                    }
                    if self.at_line_start {
                        self.begin_line(&mut f, ts);
                        let held = std::mem::take(&mut self.pending_esc);
                        f.out.extend_from_slice(&held);
                    }
                    f.out.push(b);
                    self.cur.push(b);
                }
            }
        }
        f
    }

    /// Writes out escapes that were being held for a timestamp that will never
    /// come, because the line ended without showing anything.
    fn release_escapes(&mut self, f: &mut Feed) {
        if !self.pending_esc.is_empty() {
            let held = std::mem::take(&mut self.pending_esc);
            f.out.extend_from_slice(&held);
        }
    }

    /// Emits a CR that was held back waiting to see whether an LF follows, and
    /// any escapes still waiting on a line that has gone quiet.
    pub fn flush_pending(&mut self) -> Feed {
        let mut f = Feed::default();
        if !self.pending_esc.is_empty() {
            self.release_escapes(&mut f);
            // The prefix can no longer go ahead of these, and printing it after
            // them would recolour the device's own text. Skip it for this line.
            self.at_line_start = false;
            self.line_started = Some((Local::now(), Instant::now()));
        }
        if self.pending_cr {
            self.pending_cr = false;
            f.out.push(b'\r');
            self.cur.clear();
            self.at_line_start = true;
            self.line_started = None;
        }
        f
    }

    fn begin_line(&mut self, f: &mut Feed, ts: TsMode) {
        self.at_line_start = false;
        let started = (Local::now(), Instant::now());
        self.line_started = Some(started);
        if ts == TsMode::Off {
            return;
        }
        let stub = Line {
            raw: Vec::new(),
            at: started.0,
            rel: started.1.saturating_duration_since(self.start),
            delta: started.1.saturating_duration_since(self.prev_line),
        };
        f.out.extend_from_slice(b"\x1b[90m");
        f.out.extend_from_slice(stub.prefix(ts).as_bytes());
        f.out.extend_from_slice(b"\x1b[39m");
    }

    fn finish_line(&mut self, f: &mut Feed) {
        let (at, mono) = self
            .line_started
            .take()
            .unwrap_or_else(|| (Local::now(), Instant::now()));
        f.lines.push(Line {
            raw: std::mem::take(&mut self.cur),
            at,
            rel: mono.saturating_duration_since(self.start),
            delta: mono.saturating_duration_since(self.prev_line),
        });
        self.prev_line = mono;
        self.at_line_start = true;
    }
}

impl Default for LineAssembler {
    fn default() -> Self {
        Self::new()
    }
}

pub fn format_duration(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn out(f: &Feed) -> String {
        String::from_utf8_lossy(&f.out).into_owned()
    }

    #[test]
    fn escape_sequences_pass_through_untouched() {
        let mut a = LineAssembler::new();
        let f = a.feed(b"\x1b[32mok\x1b[0m\r\nnext\r\n", TsMode::Off);
        assert_eq!(out(&f), "\x1b[32mok\x1b[0m\r\nnext\r\n");
        assert_eq!(f.lines.len(), 2);
        assert_eq!(f.lines[0].text(), "ok");
        assert_eq!(f.lines[1].text(), "next");
    }

    #[test]
    fn prefixes_each_line_once_even_across_chunks() {
        let mut a = LineAssembler::new();
        let f1 = a.feed(b"hel", TsMode::Rel);
        let f2 = a.feed(b"lo\r\n", TsMode::Rel);
        assert!(out(&f1).starts_with("\x1b[90m["));
        assert!(out(&f1).ends_with("hel"));
        assert_eq!(out(&f2), "lo\r\n");
        assert_eq!(f2.lines[0].text(), "hello");
    }

    #[test]
    fn empty_lines_get_no_prefix() {
        let mut a = LineAssembler::new();
        let f = a.feed(b"\r\n\r\n", TsMode::Rel);
        assert_eq!(out(&f), "\r\n\r\n");
        assert_eq!(f.lines.len(), 2);
        assert!(f.lines[0].raw.is_empty());
    }

    #[test]
    fn lone_cr_restarts_the_line_without_emitting_it() {
        let mut a = LineAssembler::new();
        let f = a.feed(b"50%\r75%\r\n", TsMode::Off);
        assert_eq!(out(&f), "50%\r75%\r\n");
        assert_eq!(f.lines.len(), 1);
        assert_eq!(f.lines[0].text(), "75%");
    }

    #[test]
    fn cr_split_across_chunks_still_pairs_with_lf() {
        let mut a = LineAssembler::new();
        let f1 = a.feed(b"done\r", TsMode::Off);
        assert_eq!(out(&f1), "done");
        assert!(f1.lines.is_empty());
        let f2 = a.feed(b"\n", TsMode::Off);
        assert_eq!(out(&f2), "\r\n");
        assert_eq!(f2.lines[0].text(), "done");
    }

    #[test]
    fn trailing_cr_is_flushed_on_tick() {
        let mut a = LineAssembler::new();
        a.feed(b"x\r", TsMode::Off);
        let f = a.flush_pending();
        assert_eq!(out(&f), "\r");
        assert!(a.partial().is_empty());
    }

    #[test]
    fn partial_line_is_visible_before_it_ends() {
        let mut a = LineAssembler::new();
        a.feed(b"prompt> ", TsMode::Off);
        assert_eq!(a.partial(), b"prompt> ");
    }

    #[test]
    fn a_line_holding_only_escapes_gets_no_timestamp() {
        // wsh firmware appends its style reset after the newline, so the reset
        // lands alone on the next line. Stamping that prints an empty row.
        let mut a = LineAssembler::new();
        let f = a.feed(b"\x1b[0m\r\n", TsMode::Rel);
        assert_eq!(out(&f), "\x1b[0m\r\n");
        assert_eq!(f.lines.len(), 1);
        assert_eq!(f.lines[0].text(), "");
    }

    #[test]
    fn the_timestamp_still_precedes_a_leading_color() {
        // Order matters: a prefix printed after the colour would reset it and
        // the device's text would lose its colour.
        let mut a = LineAssembler::new();
        let f = a.feed(b"\x1b[32mok\r\n", TsMode::Rel);
        let s = out(&f);
        let prefix_at = s.find("\x1b[90m").unwrap();
        let color_at = s.find("\x1b[32m").unwrap();
        assert!(prefix_at < color_at, "{s:?}");
        assert!(s.ends_with("\x1b[32mok\r\n"), "{s:?}");
    }

    #[test]
    fn escapes_split_across_chunks_still_hold_the_line() {
        let mut a = LineAssembler::new();
        let f1 = a.feed(b"\x1b[3", TsMode::Rel);
        assert_eq!(out(&f1), "");
        let f2 = a.feed(b"2mok\r\n", TsMode::Rel);
        let s = out(&f2);
        assert!(s.contains("\x1b[90m"), "{s:?}");
        assert!(s.ends_with("\x1b[32mok\r\n"), "{s:?}");
    }

    #[test]
    fn held_escapes_are_released_when_the_line_goes_quiet() {
        let mut a = LineAssembler::new();
        a.feed(b"\x1b[0m", TsMode::Rel);
        let f = a.flush_pending();
        assert_eq!(out(&f), "\x1b[0m");
    }

    #[test]
    fn mid_line_tracks_unterminated_output() {
        let mut a = LineAssembler::new();
        assert!(!a.mid_line());
        a.feed(b"prompt> ", TsMode::Off);
        assert!(a.mid_line());
        a.feed(b"\r\n", TsMode::Off);
        assert!(!a.mid_line());
    }

    #[test]
    fn bare_lf_terminates_a_line_and_gains_a_cr() {
        let mut a = LineAssembler::new();
        let f = a.feed(b"unix\n", TsMode::Off);
        assert_eq!(out(&f), "unix\r\n");
        assert_eq!(f.lines.len(), 1);
        assert_eq!(f.lines[0].text(), "unix");
    }
}
