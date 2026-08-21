use std::collections::VecDeque;
use std::time::Duration;

use chrono::{DateTime, Local};

use crate::cli::TsMode;

#[derive(Clone, Debug)]
pub struct Line {
    pub raw: Vec<u8>,
    pub at: DateTime<Local>,
    pub rel: Duration,
    pub delta: Duration,
}

impl Line {
    pub fn prefix(&self, ts: TsMode) -> String {
        match ts {
            TsMode::Off => String::new(),
            TsMode::Rel => format!("[{:>9.3}] ", self.rel.as_secs_f64()),
            TsMode::Abs => format!("[{}] ", self.at.format("%H:%M:%S%.3f")),
            TsMode::Delta => format!("[+{:>8.3}] ", self.delta.as_secs_f64()),
        }
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&crate::ansi::strip(&self.raw)).into_owned()
    }
}

/// Ring buffer of received lines, used by the TUI and by `Ctrl-A` redraws.
pub struct Scrollback {
    lines: VecDeque<Line>,
    cap: usize,
    dropped: u64,
}

impl Scrollback {
    pub fn new(cap: usize) -> Self {
        Scrollback {
            lines: VecDeque::with_capacity(cap.min(1024)),
            cap: cap.max(1),
            dropped: 0,
        }
    }

    pub fn push(&mut self, line: Line) {
        if self.lines.len() == self.cap {
            self.lines.pop_front();
            self.dropped += 1;
        }
        self.lines.push_back(line);
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Number of lines ever received, including those already evicted.
    pub fn total(&self) -> u64 {
        self.dropped + self.lines.len() as u64
    }

    /// Lines received since `mark`, expressed in `total()` terms.
    pub fn since(&self, mark: u64) -> impl DoubleEndedIterator<Item = &Line> {
        let skip = mark
            .saturating_sub(self.dropped)
            .min(self.lines.len() as u64) as usize;
        self.lines.iter().skip(skip)
    }

    pub fn get(&self, i: usize) -> Option<&Line> {
        self.lines.get(i)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(raw: &str) -> Line {
        Line {
            raw: raw.as_bytes().to_vec(),
            at: Local::now(),
            rel: Duration::from_millis(1500),
            delta: Duration::from_millis(250),
        }
    }

    #[test]
    fn drops_oldest_lines_when_full() {
        let mut sb = Scrollback::new(3);
        for i in 0..5 {
            sb.push(line(&i.to_string()));
        }
        assert_eq!(sb.len(), 3);
        assert_eq!(sb.dropped(), 2);
        assert_eq!(sb.get(0).unwrap().text(), "2");
        assert_eq!(sb.get(2).unwrap().text(), "4");
    }

    #[test]
    fn since_returns_only_newer_lines() {
        let mut sb = Scrollback::new(3);
        sb.push(line("a"));
        let mark = sb.total();
        sb.push(line("b"));
        sb.push(line("c"));
        let got: Vec<String> = sb.since(mark).map(|l| l.text()).collect();
        assert_eq!(got, vec!["b", "c"]);

        // Once eviction kicks in, `since` clamps to what is still held.
        sb.push(line("d"));
        sb.push(line("e"));
        assert_eq!(sb.since(mark).count(), 3);
    }

    #[test]
    fn formats_prefixes_per_mode() {
        let l = line("x");
        assert_eq!(l.prefix(TsMode::Off), "");
        assert_eq!(l.prefix(TsMode::Rel), "[    1.500] ");
        assert_eq!(l.prefix(TsMode::Delta), "[+   0.250] ");
        assert!(l.prefix(TsMode::Abs).starts_with('['));
    }

    #[test]
    fn text_strips_ansi() {
        assert_eq!(line("\x1b[31merr\x1b[0m").text(), "err");
    }
}
