use std::io::Stdout;

use ansi_to_tui::IntoText;
use anyhow::Result;
use ratatui::Terminal;
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line as TLine, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use regex::Regex;

use crate::cli::TsMode;
use crate::input::Key;
use crate::scrollback::Scrollback;
use crate::session::human_bytes;

pub enum TuiAction {
    Stay,
    Leave,
    Quit,
}

pub struct TuiStatus {
    pub port: String,
    pub baud: String,
    pub frame: String,
    pub connected: bool,
    pub rx_bytes: u64,
    pub ts: TsMode,
    pub logging: Option<String>,
    pub uptime: String,
}

#[derive(PartialEq, Eq)]
enum Mode {
    Normal,
    Search,
    Filter,
}

pub struct Tui<B: Backend = CrosstermBackend<Stdout>> {
    terminal: Terminal<B>,
    mode: Mode,
    input: String,
    filter: Option<Regex>,
    filter_src: String,
    search: Option<Regex>,
    search_src: String,
    error: Option<String>,
    /// Index into the filtered view of the line kept in sight.
    cursor: usize,
    follow: bool,
    page: usize,
    pending_escape: bool,
}

impl Tui<CrosstermBackend<Stdout>> {
    pub fn new() -> Result<Self> {
        let terminal = Terminal::new(CrosstermBackend::new(std::io::stdout()))?;
        Ok(Tui::with_terminal(terminal))
    }
}

impl<B: Backend> Tui<B>
where
    B::Error: std::error::Error + Send + Sync + 'static,
{
    fn with_terminal(terminal: Terminal<B>) -> Self {
        Tui {
            terminal,
            mode: Mode::Normal,
            input: String::new(),
            filter: None,
            filter_src: String::new(),
            search: None,
            search_src: String::new(),
            error: None,
            cursor: usize::MAX,
            follow: true,
            page: 10,
            pending_escape: false,
        }
    }

    fn visible(&self, sb: &Scrollback) -> Vec<usize> {
        match &self.filter {
            None => (0..sb.len()).collect(),
            Some(re) => (0..sb.len())
                .filter(|&i| sb.get(i).map(|l| re.is_match(&l.text())).unwrap_or(false))
                .collect(),
        }
    }

    pub fn on_key(&mut self, key: Key, sb: &Scrollback) -> TuiAction {
        if self.pending_escape {
            self.pending_escape = false;
            if key == Key::Char('q') {
                return TuiAction::Quit;
            }
        }
        match self.mode {
            Mode::Normal => self.normal_key(key, sb),
            _ => self.editing_key(key, sb),
        }
    }

    fn normal_key(&mut self, key: Key, sb: &Scrollback) -> TuiAction {
        let view = self.visible(sb);
        let last = view.len().saturating_sub(1);
        match key {
            Key::Char('q') | Key::Esc => return TuiAction::Leave,
            Key::Ctrl('a') => self.pending_escape = true,
            Key::Ctrl('c') => return TuiAction::Leave,
            Key::Up | Key::Char('k') => self.move_cursor(-1, last),
            Key::Down | Key::Char('j') => self.move_cursor(1, last),
            Key::PageUp => self.move_cursor(-(self.page as isize), last),
            Key::PageDown => self.move_cursor(self.page as isize, last),
            Key::Home | Key::Char('g') => {
                self.cursor = 0;
                self.follow = false;
            }
            Key::End | Key::Char('G') => {
                self.cursor = last;
                self.follow = true;
            }
            Key::Char('/') => {
                self.mode = Mode::Search;
                self.input = self.search_src.clone();
                self.error = None;
            }
            Key::Char('f') => {
                self.mode = Mode::Filter;
                self.input = self.filter_src.clone();
                self.error = None;
            }
            Key::Char('n') => self.jump_match(sb, 1),
            Key::Char('N') => self.jump_match(sb, -1),
            _ => {}
        }
        TuiAction::Stay
    }

    fn editing_key(&mut self, key: Key, sb: &Scrollback) -> TuiAction {
        match key {
            Key::Esc => {
                self.mode = Mode::Normal;
                self.input.clear();
                self.error = None;
            }
            Key::Enter => {
                self.apply_input(sb);
                self.mode = Mode::Normal;
            }
            Key::Backspace => {
                self.input.pop();
                if self.mode == Mode::Filter {
                    self.apply_filter_live();
                }
            }
            Key::Char(c) => {
                self.input.push(c);
                if self.mode == Mode::Filter {
                    self.apply_filter_live();
                }
            }
            Key::Ctrl('u') => {
                self.input.clear();
                if self.mode == Mode::Filter {
                    self.apply_filter_live();
                }
            }
            _ => {}
        }
        TuiAction::Stay
    }

    fn apply_input(&mut self, sb: &Scrollback) {
        let src = self.input.clone();
        if src.is_empty() {
            match self.mode {
                Mode::Search => {
                    self.search = None;
                    self.search_src.clear();
                }
                _ => {
                    self.filter = None;
                    self.filter_src.clear();
                }
            }
            self.error = None;
            return;
        }
        match Regex::new(&src) {
            Ok(re) => {
                self.error = None;
                if self.mode == Mode::Search {
                    self.search = Some(re);
                    self.search_src = src;
                    self.jump_match(sb, 1);
                } else {
                    self.filter = Some(re);
                    self.filter_src = src;
                    self.follow = true;
                    self.cursor = usize::MAX;
                }
            }
            Err(e) => self.error = Some(first_line(&e.to_string())),
        }
    }

    fn apply_filter_live(&mut self) {
        if self.input.is_empty() {
            self.filter = None;
            self.filter_src.clear();
            self.error = None;
            return;
        }
        match Regex::new(&self.input) {
            Ok(re) => {
                self.filter = Some(re);
                self.filter_src = self.input.clone();
                self.error = None;
            }
            Err(e) => self.error = Some(first_line(&e.to_string())),
        }
    }

    fn move_cursor(&mut self, delta: isize, last: usize) {
        let cur = self.cursor.min(last) as isize;
        let next = (cur + delta).clamp(0, last as isize) as usize;
        self.cursor = next;
        self.follow = next >= last;
    }

    fn jump_match(&mut self, sb: &Scrollback, dir: isize) {
        let Some(re) = &self.search else { return };
        let view = self.visible(sb);
        if view.is_empty() {
            return;
        }
        let last = view.len() - 1;
        let start = self.cursor.min(last);
        let len = view.len() as isize;
        for step in 1..=len {
            let idx = (start as isize + dir * step).rem_euclid(len) as usize;
            let line = view[idx];
            if sb
                .get(line)
                .map(|l| re.is_match(&l.text()))
                .unwrap_or(false)
            {
                self.cursor = idx;
                self.follow = false;
                return;
            }
        }
    }

    pub fn draw(&mut self, sb: &Scrollback, partial: &[u8], status: &TuiStatus) -> Result<()> {
        let view = self.visible(sb);
        let last = view.len().saturating_sub(1);
        if self.follow {
            self.cursor = last;
        }
        let cursor = self.cursor.min(last);

        let title = format!(
            " {} · {} {} · {} · {} · {} ",
            status.port,
            status.baud,
            status.frame,
            if status.connected {
                "connected"
            } else {
                "waiting"
            },
            human_bytes(status.rx_bytes),
            status.uptime,
        );

        let mode_line = self.hint_line(&view, cursor, sb, status);
        let search = self.search.clone();
        let follow = self.follow;
        let partial = partial.to_vec();

        self.terminal.draw(|frame| {
            let area = frame.area();
            let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(area);
            let body = chunks[0];
            let inner_h = body.height.saturating_sub(2) as usize;

            let inner_w = body.width.saturating_sub(2).max(1) as usize;
            let mut rows_left = inner_h;

            // The unfinished line sits at the bottom and takes room like any other.
            let mut tail: Option<TLine> = None;
            if follow && !partial.is_empty() {
                let parsed = partial
                    .clone()
                    .into_text()
                    .map(|t| t.lines.into_iter().next().unwrap_or_default())
                    .unwrap_or_default();
                let h = wrapped_height(parsed.width(), inner_w);
                if h < rows_left {
                    rows_left -= h;
                    tail = Some(parsed);
                }
            }

            // Fill upwards from the cursor so the line in focus is always shown
            // in full, however many screen rows its wrapped text needs.
            let mut text: Vec<TLine> = Vec::new();
            for idx in (0..=cursor).rev() {
                let Some(&line_idx) = view.get(idx) else {
                    continue;
                };
                let Some(line) = sb.get(line_idx) else {
                    continue;
                };
                let mut raw = line.prefix(status.ts).into_bytes();
                raw.extend_from_slice(&line.raw);
                let mut tline = raw
                    .into_text()
                    .map(|t| t.lines.into_iter().next().unwrap_or_default())
                    .unwrap_or_else(|_| TLine::from(line.text()));
                let h = wrapped_height(tline.width(), inner_w);
                if h > rows_left && !text.is_empty() {
                    break;
                }
                if idx == cursor {
                    tline.style = Style::default().bg(Color::Indexed(236));
                } else if let Some(re) = &search
                    && re.is_match(&line.text())
                {
                    tline.style = Style::default().bg(Color::Indexed(238));
                }
                rows_left = rows_left.saturating_sub(h);
                text.push(tline);
                if rows_left == 0 {
                    break;
                }
            }
            text.reverse();
            text.extend(tail);

            let block = Block::default()
                .borders(Borders::ALL)
                .title(title.clone())
                .border_style(Style::default().fg(Color::Indexed(240)));
            frame.render_widget(
                Paragraph::new(text).block(block).wrap(Wrap { trim: false }),
                body,
            );
            frame.render_widget(Paragraph::new(mode_line.clone()), chunks[1]);
        })?;

        self.page = self
            .terminal
            .size()
            .map(|s| s.height.saturating_sub(4).max(1) as usize)
            .unwrap_or(10);
        Ok(())
    }

    fn hint_line<'a>(
        &self,
        view: &[usize],
        cursor: usize,
        sb: &Scrollback,
        status: &TuiStatus,
    ) -> TLine<'a> {
        let dim = Style::default().fg(Color::Indexed(245));
        let accent = Style::default().fg(Color::Indexed(39));

        if let Some(err) = &self.error {
            return TLine::from(vec![Span::styled(
                format!(" regex: {err} "),
                Style::default().fg(Color::Indexed(203)),
            )]);
        }
        match self.mode {
            Mode::Search => TLine::from(vec![
                Span::styled(" /", accent),
                Span::styled(
                    format!("{}\u{2588}", self.input),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
            ]),
            Mode::Filter => TLine::from(vec![
                Span::styled(" filter: ", accent),
                Span::styled(
                    format!("{}\u{2588}", self.input),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
            ]),
            Mode::Normal => {
                let mut spans = vec![Span::styled(
                    format!(
                        " {}/{} ",
                        if view.is_empty() { 0 } else { cursor + 1 },
                        view.len()
                    ),
                    accent,
                )];
                if self.follow {
                    spans.push(Span::styled(
                        "follow ",
                        Style::default().fg(Color::Indexed(41)),
                    ));
                }
                if !self.filter_src.is_empty() {
                    spans.push(Span::styled(
                        format!("filter:{} ", self.filter_src),
                        Style::default().fg(Color::Indexed(215)),
                    ));
                }
                if !self.search_src.is_empty() {
                    spans.push(Span::styled(
                        format!("/{} ", self.search_src),
                        Style::default().fg(Color::Indexed(215)),
                    ));
                }
                if sb.dropped() > 0 {
                    spans.push(Span::styled(
                        format!("{} evicted ", sb.dropped()),
                        Style::default().fg(Color::Indexed(215)),
                    ));
                }
                if status.logging.is_some() {
                    spans.push(Span::styled(
                        "rec ",
                        Style::default().fg(Color::Indexed(203)),
                    ));
                }
                spans.push(Span::styled(
                    format!(
                        "ts:{} · / search · f filter · n/N · q back · ^A q quit",
                        status.ts.label()
                    ),
                    dim,
                ));
                TLine::from(spans)
            }
        }
    }
}

/// Screen rows a line of `width` columns needs once wrapped.
fn wrapped_height(width: usize, inner_w: usize) -> usize {
    width.div_ceil(inner_w.max(1)).max(1)
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or(s).trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scrollback::Line;
    use chrono::Local;
    use ratatui::backend::TestBackend;
    use std::time::Duration;

    fn sb_with(lines: &[&str]) -> Scrollback {
        let mut sb = Scrollback::new(100);
        for l in lines {
            sb.push(Line {
                raw: l.as_bytes().to_vec(),
                at: Local::now(),
                rel: Duration::ZERO,
                delta: Duration::ZERO,
            });
        }
        sb
    }

    fn headless() -> Tui<TestBackend> {
        headless_sized(80, 24)
    }

    fn headless_sized(w: u16, h: u16) -> Tui<TestBackend> {
        Tui::with_terminal(Terminal::new(TestBackend::new(w, h)).unwrap())
    }

    fn status() -> TuiStatus {
        TuiStatus {
            port: "/dev/ttyACM0".into(),
            baud: "115200".into(),
            frame: "8N1".into(),
            connected: true,
            rx_bytes: 2048,
            ts: TsMode::Off,
            logging: None,
            uptime: "0:07".into(),
        }
    }

    fn rendered(t: &Tui<TestBackend>) -> String {
        t.terminal
            .backend()
            .buffer()
            .content()
            .chunks(t.terminal.backend().buffer().area.width as usize)
            .map(|row| row.iter().map(|c| c.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn filter_narrows_the_view() {
        let sb = sb_with(&["I info", "E error one", "I info", "E error two"]);
        let mut t = headless();
        t.mode = Mode::Filter;
        for c in "^E".chars() {
            t.on_key(Key::Char(c), &sb);
        }
        t.on_key(Key::Enter, &sb);
        assert_eq!(t.visible(&sb), vec![1, 3]);
    }

    #[test]
    fn invalid_regex_is_reported_not_applied() {
        let sb = sb_with(&["a"]);
        let mut t = headless();
        t.mode = Mode::Filter;
        t.on_key(Key::Char('['), &sb);
        assert!(t.error.is_some());
        assert!(t.filter.is_none());
        assert_eq!(t.visible(&sb), vec![0]);
    }

    #[test]
    fn search_jumps_between_matches_and_wraps() {
        let sb = sb_with(&["one", "target", "three", "target"]);
        let mut t = headless();
        t.cursor = 0;
        t.search = Some(Regex::new("target").unwrap());
        t.jump_match(&sb, 1);
        assert_eq!(t.cursor, 1);
        t.jump_match(&sb, 1);
        assert_eq!(t.cursor, 3);
        t.jump_match(&sb, 1);
        assert_eq!(t.cursor, 1);
        t.jump_match(&sb, -1);
        assert_eq!(t.cursor, 3);
    }

    #[test]
    fn scrolling_up_stops_following_and_end_resumes_it() {
        let sb = sb_with(&["a", "b", "c"]);
        let mut t = headless();
        t.cursor = 2;
        t.on_key(Key::Up, &sb);
        assert_eq!(t.cursor, 1);
        assert!(!t.follow);
        t.on_key(Key::End, &sb);
        assert_eq!(t.cursor, 2);
        assert!(t.follow);
    }

    #[test]
    fn q_leaves_but_ctrl_a_q_quits() {
        let sb = sb_with(&["a"]);
        let mut t = headless();
        assert!(matches!(t.on_key(Key::Char('q'), &sb), TuiAction::Leave));
        t.on_key(Key::Ctrl('a'), &sb);
        assert!(matches!(t.on_key(Key::Char('q'), &sb), TuiAction::Quit));
    }

    #[test]
    fn draws_the_newest_lines_with_a_status_bar() {
        let sb = sb_with(&["first", "second", "third"]);
        let mut t = headless_sized(40, 8);
        t.draw(&sb, b"", &status()).unwrap();
        let screen = rendered(&t);
        assert!(screen.contains("/dev/ttyACM0"), "{screen}");
        assert!(screen.contains("third"), "{screen}");
        assert!(screen.contains("3/3"), "{screen}");
        assert!(screen.contains("follow"), "{screen}");
    }

    #[test]
    fn draws_the_unfinished_line_last() {
        let sb = sb_with(&["done"]);
        let mut t = headless_sized(40, 8);
        t.draw(&sb, b"prompt> ", &status()).unwrap();
        let screen = rendered(&t);
        assert!(screen.contains("prompt>"), "{screen}");
    }

    #[test]
    fn wrapped_height_counts_screen_rows() {
        assert_eq!(wrapped_height(0, 80), 1);
        assert_eq!(wrapped_height(80, 80), 1);
        assert_eq!(wrapped_height(81, 80), 2);
        assert_eq!(wrapped_height(240, 80), 3);
    }

    #[test]
    fn ansi_lines_survive_the_round_trip() {
        let sb = sb_with(&["\x1b[31mred\x1b[0m"]);
        assert_eq!(sb.get(0).unwrap().text(), "red");
    }
}
