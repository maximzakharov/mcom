use std::io::{Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use serialport::TTYPort;

use crate::cli::{Cli, FrameMode, TsMode};
use crate::input::{Key, KeyDecoder};
use crate::logfile::LogWriter;
use crate::ports;
use crate::scrollback::Scrollback;
use crate::serial::{self, PortConfig};
use crate::term::TermGuard;
use crate::timestamps::{LineAssembler, format_duration};
use crate::tui::{Tui, TuiAction, TuiStatus};

const TICK: Duration = Duration::from_millis(50);
const RECONNECT_EVERY: Duration = Duration::from_millis(250);
/// 200 ms of empty reads before we call the device gone.
const ZERO_READS_BEFORE_HANGUP: u32 = 20;
/// Cap on how much of the backlog is replayed when leaving the scrollback view.
const REPLAY_LIMIT: usize = 1000;

pub enum Event {
    Rx(Vec<u8>),
    Disconnected(String),
    Stdin(Vec<u8>),
    Tick,
    Signal(i32),
}

struct PortHandle {
    writer: TTYPort,
    stop: Arc<AtomicBool>,
    reader: Option<thread::JoinHandle<()>>,
}

impl PortHandle {
    fn close(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.reader.take() {
            let _ = h.join();
        }
    }
}

impl Drop for PortHandle {
    fn drop(&mut self) {
        self.close();
    }
}

pub struct Session {
    cfg: PortConfig,
    tx: Sender<Event>,
    rx: Receiver<Event>,
    term: TermGuard,
    port: Option<PortHandle>,
    asm: LineAssembler,
    sb: Scrollback,
    log: Option<LogWriter>,
    log_format: crate::cli::LogFormat,
    identity: Option<String>,
    ts: TsMode,
    escape: u8,
    echo: bool,
    reconnect: bool,
    strict_port: bool,
    escape_pending: bool,
    keys: KeyDecoder,
    tui: Option<Tui>,
    tui_mark: u64,
    rx_bytes: u64,
    started: Instant,
    since_reconnect_try: Instant,
    quit: bool,
}

pub fn run(cli: Cli) -> Result<()> {
    let path = ports::choose(cli.port.as_deref())?;
    let frame = FrameMode::parse(&cli.mode)?;
    let cfg = PortConfig {
        path,
        baud: cli.baud,
        frame,
    };

    // Opened before raw mode so failures print as ordinary, readable errors.
    let port = serial::open(&cfg)?;
    let identity = ports::device_identity(&cfg.path);

    let log = match &cli.log {
        Some(p) => Some(LogWriter::open(Some(p), cli.log_format, &cfg.path)?),
        None => None,
    };

    let (tx, rx) = channel();
    let term = TermGuard::new()?;

    let mut s = Session {
        cfg,
        tx,
        rx,
        term,
        port: None,
        asm: LineAssembler::new(),
        sb: Scrollback::new(cli.scrollback),
        log,
        log_format: cli.log_format,
        identity,
        ts: cli.ts,
        escape: ctrl_byte(cli.escape),
        echo: cli.echo,
        reconnect: !cli.no_reconnect,
        strict_port: cli.strict_port,
        escape_pending: false,
        keys: KeyDecoder::default(),
        tui: None,
        tui_mark: 0,
        rx_bytes: 0,
        started: Instant::now(),
        since_reconnect_try: Instant::now(),
        quit: false,
    };

    spawn_stdin(s.tx.clone());
    spawn_ticker(s.tx.clone());
    spawn_signals(s.tx.clone())?;
    s.attach(port)?;
    s.banner();

    let result = s.main_loop();
    s.shutdown();
    result
}

fn ctrl_byte(c: char) -> u8 {
    let c = c.to_ascii_lowercase() as u8;
    if c.is_ascii_lowercase() {
        c - b'a' + 1
    } else {
        0x01
    }
}

fn spawn_stdin(tx: Sender<Event>) {
    thread::spawn(move || {
        let mut stdin = std::io::stdin();
        let mut buf = [0u8; 4096];
        loop {
            match stdin.read(&mut buf) {
                Ok(0) => return,
                Ok(n) => {
                    if tx.send(Event::Stdin(buf[..n].to_vec())).is_err() {
                        return;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => return,
            }
        }
    });
}

fn spawn_ticker(tx: Sender<Event>) {
    thread::spawn(move || {
        while tx.send(Event::Tick).is_ok() {
            thread::sleep(TICK);
        }
    });
}

fn spawn_signals(tx: Sender<Event>) -> Result<()> {
    use signal_hook::consts::{SIGHUP, SIGINT, SIGQUIT, SIGTERM, SIGWINCH};
    let mut signals =
        signal_hook::iterator::Signals::new([SIGINT, SIGTERM, SIGHUP, SIGQUIT, SIGWINCH])?;
    thread::spawn(move || {
        for sig in signals.forever() {
            if tx.send(Event::Signal(sig)).is_err() {
                return;
            }
        }
    });
    Ok(())
}

fn spawn_reader(
    mut port: TTYPort,
    tx: Sender<Event>,
    stop: Arc<AtomicBool>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut buf = [0u8; 8192];
        // A serial port signals "nothing yet" with a timeout, never with a
        // zero-length read; a run of those means the device is gone.
        let mut zero_reads = 0u32;
        while !stop.load(Ordering::SeqCst) {
            match port.read(&mut buf) {
                Ok(0) => {
                    zero_reads += 1;
                    if zero_reads >= ZERO_READS_BEFORE_HANGUP {
                        if !stop.load(Ordering::SeqCst) {
                            let _ = tx.send(Event::Disconnected("device closed the port".into()));
                        }
                        return;
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                Ok(n) => {
                    zero_reads = 0;
                    if tx.send(Event::Rx(buf[..n].to_vec())).is_err() {
                        return;
                    }
                }
                Err(e) if !serial::is_disconnect(&e) => {}
                Err(e) => {
                    if !stop.load(Ordering::SeqCst) {
                        let _ = tx.send(Event::Disconnected(e.to_string()));
                    }
                    return;
                }
            }
        }
    })
}

impl Session {
    fn main_loop(&mut self) -> Result<()> {
        while !self.quit {
            let ev = match self.rx.recv() {
                Ok(ev) => ev,
                Err(_) => break,
            };
            match ev {
                Event::Rx(data) => self.on_rx(&data)?,
                Event::Disconnected(reason) => self.on_disconnect(&reason)?,
                Event::Stdin(data) => self.on_stdin(&data)?,
                Event::Tick => self.on_tick()?,
                Event::Signal(sig) => self.on_signal(sig)?,
            }
        }
        Ok(())
    }

    fn attach(&mut self, port: TTYPort) -> Result<()> {
        let writer = serial::writer(&port)?;
        let stop = Arc::new(AtomicBool::new(false));
        let reader = spawn_reader(port, self.tx.clone(), stop.clone());
        self.port = Some(PortHandle {
            writer,
            stop,
            reader: Some(reader),
        });
        Ok(())
    }

    fn banner(&mut self) {
        let art = crate::logo::banner();
        self.write_out(art.as_bytes());

        let msg = format!(
            "{} · {} {} · Ctrl-{} q to quit, Ctrl-{} ? for help",
            self.cfg.path,
            baud_label(self.cfg.baud),
            self.cfg.frame,
            escape_name(self.escape),
            escape_name(self.escape),
        );
        self.note(&msg, "36");
        self.write_out(b"\r\n");
    }

    /// Writes one of our own status lines, kept visually distinct from device output.
    fn note(&mut self, text: &str, color: &str) {
        let line = note_line(text, color, self.asm.mid_line());
        self.write_out(line.as_bytes());
        if let Some(log) = &mut self.log {
            let _ = log.note(text);
        }
    }

    fn write_out(&mut self, data: &[u8]) {
        if self.tui.is_some() {
            return;
        }
        let mut out = std::io::stdout().lock();
        let _ = out.write_all(data);
        let _ = out.flush();
    }

    fn on_rx(&mut self, data: &[u8]) -> Result<()> {
        self.rx_bytes += data.len() as u64;
        if self.log_format == crate::cli::LogFormat::Raw
            && let Some(log) = &mut self.log
        {
            let _ = log.write_chunk(data);
        }

        let feed = self.asm.feed(data, self.ts);
        self.write_out(&feed.out);
        for line in feed.lines {
            if let Some(log) = &mut self.log {
                let _ = log.write_line(&line);
            }
            self.sb.push(line);
        }
        Ok(())
    }

    fn on_disconnect(&mut self, reason: &str) -> Result<()> {
        self.port = None;
        if !self.reconnect {
            self.note(&format!("disconnected: {reason}"), "31");
            self.quit = true;
            return Ok(());
        }
        self.note(
            &format!("disconnected ({reason}), waiting for the device"),
            "33",
        );
        self.since_reconnect_try = Instant::now();
        Ok(())
    }

    fn on_tick(&mut self) -> Result<()> {
        let feed = self.asm.flush_pending();
        if !feed.out.is_empty() {
            self.write_out(&feed.out);
        }
        if let Some(key) = self.keys.flush_escape()
            && self.tui.is_some()
        {
            self.handle_tui_key(key)?;
        }
        if self.port.is_none() && self.reconnect {
            self.try_reconnect()?;
        }
        if self.tui.is_some() {
            self.draw_tui()?;
        }
        Ok(())
    }

    fn try_reconnect(&mut self) -> Result<()> {
        if self.since_reconnect_try.elapsed() < RECONNECT_EVERY {
            return Ok(());
        }
        self.since_reconnect_try = Instant::now();

        let Some(target) = serial::find_reconnect_target(
            &self.cfg.path,
            self.identity.as_deref(),
            self.strict_port,
        ) else {
            return Ok(());
        };
        let mut cfg = self.cfg.clone();
        cfg.path = target;
        // The device node can exist for a moment before it is usable, so a
        // failed open here is normal and simply retried on the next tick.
        if let Ok(port) = serial::open(&cfg) {
            let renamed = cfg.path != self.cfg.path;
            self.cfg = cfg;
            self.identity = ports::device_identity(&self.cfg.path).or(self.identity.take());
            self.attach(port)?;
            if renamed {
                self.note(&format!("reconnected as {}", self.cfg.path), "32");
            } else {
                self.note("reconnected", "32");
            }
        }
        Ok(())
    }

    fn on_signal(&mut self, sig: i32) -> Result<()> {
        use signal_hook::consts::SIGWINCH;
        if sig == SIGWINCH {
            if self.tui.is_some() {
                self.draw_tui()?;
            }
            return Ok(());
        }
        self.note("terminating", "33");
        self.quit = true;
        Ok(())
    }

    fn on_stdin(&mut self, data: &[u8]) -> Result<()> {
        if self.tui.is_some() {
            self.keys.push(data);
            while let Some(key) = self.keys.next_key() {
                self.handle_tui_key(key)?;
                if self.tui.is_none() {
                    // Left the scrollback view; the rest is ordinary input.
                    break;
                }
            }
            return Ok(());
        }

        let mut to_port: Vec<u8> = Vec::with_capacity(data.len());
        for &b in data {
            if self.escape_pending {
                self.escape_pending = false;
                self.escape_command(b, &mut to_port)?;
            } else if b == self.escape {
                self.escape_pending = true;
            } else {
                to_port.push(b);
            }
        }
        if !to_port.is_empty() {
            self.send(&to_port)?;
        }
        Ok(())
    }

    fn send(&mut self, data: &[u8]) -> Result<()> {
        if self.echo {
            self.write_out(data);
        }
        let Some(port) = &mut self.port else {
            self.note("not connected — input discarded", "31");
            return Ok(());
        };
        if let Err(e) = port.writer.write_all(data) {
            let msg = e.to_string();
            self.on_disconnect(&msg)?;
        }
        Ok(())
    }

    fn escape_command(&mut self, b: u8, to_port: &mut Vec<u8>) -> Result<()> {
        match b {
            b'q' => {
                self.quit = true;
            }
            b'?' | b'h' => self.print_help(),
            b's' => self.enter_tui()?,
            b't' => {
                self.ts = self.ts.next();
                let label = self.ts.label();
                self.note(&format!("timestamps: {label}"), "36");
            }
            b'l' => self.toggle_log()?,
            b'c' => {
                self.write_out(b"\x1b[2J\x1b[H");
            }
            b'b' => {
                if let Some(port) = &mut self.port {
                    let _ = serial::send_break(&mut port.writer);
                    self.note("break sent", "36");
                }
            }
            b'r' => {
                self.port = None;
                self.note("reconnecting", "33");
                self.since_reconnect_try = Instant::now() - RECONNECT_EVERY;
            }
            b'i' => self.print_status(),
            _ if b == self.escape => to_port.push(self.escape),
            _ => {}
        }
        Ok(())
    }

    fn toggle_log(&mut self) -> Result<()> {
        match self.log.take() {
            Some(log) => {
                let msg = format!("logging stopped: {}", log.path().display());
                self.note(&msg, "36");
            }
            None => match LogWriter::open(None, self.log_format, &self.cfg.path) {
                Ok(log) => {
                    let msg = format!("logging to {}", log.path().display());
                    self.log = Some(log);
                    self.note(&msg, "36");
                }
                Err(e) => self.note(&format!("cannot start log: {e}"), "31"),
            },
        }
        Ok(())
    }

    fn print_status(&mut self) {
        let state = if self.port.is_some() {
            "connected"
        } else {
            "waiting"
        };
        let log = match &self.log {
            Some(l) => l.path().display().to_string(),
            None => "off".into(),
        };
        let lines = self.sb.total();
        let msg = format!(
            "{} · {} {} · {} · up {} · {} received · {} {} · ts {} · log {}",
            self.cfg.path,
            baud_label(self.cfg.baud),
            self.cfg.frame,
            state,
            format_duration(self.started.elapsed()),
            human_bytes(self.rx_bytes),
            lines,
            if lines == 1 { "line" } else { "lines" },
            self.ts.label(),
            log,
        );
        self.note(&msg, "36");
    }

    fn print_help(&mut self) {
        let e = escape_name(self.escape);
        let lines = [
            format!("Ctrl-{e} q   quit and release the port"),
            format!("Ctrl-{e} ?   this help"),
            format!("Ctrl-{e} s   scrollback view (search, filter)"),
            format!("Ctrl-{e} t   cycle timestamps (off, rel, abs, delta)"),
            format!("Ctrl-{e} l   start or stop logging to a file"),
            format!("Ctrl-{e} c   clear the screen"),
            format!("Ctrl-{e} b   send a break"),
            format!("Ctrl-{e} r   force a reconnect"),
            format!("Ctrl-{e} i   session status"),
            format!("Ctrl-{e} Ctrl-{e}   send Ctrl-{e} to the device"),
        ];
        let mut buf = String::from("\x1b[36m");
        for l in lines {
            buf.push_str(&l);
            buf.push_str("\r\n");
        }
        buf.push_str("\x1b[0m");
        self.write_out(buf.as_bytes());
    }

    fn enter_tui(&mut self) -> Result<()> {
        self.tui_mark = self.sb.total();
        self.tui = Some(Tui::new()?);
        self.keys = KeyDecoder::default();
        self.term.enter_alt()?;
        self.draw_tui()
    }

    fn leave_tui(&mut self) -> Result<()> {
        self.tui = None;
        self.term.leave_alt()?;

        let missed: Vec<String> = self
            .sb
            .since(self.tui_mark)
            .map(|l| {
                let mut s = l.prefix(self.ts);
                if !s.is_empty() {
                    s = format!("\x1b[90m{s}\x1b[39m");
                }
                s.push_str(&String::from_utf8_lossy(&l.raw));
                s
            })
            .collect();

        let skipped = missed.len().saturating_sub(REPLAY_LIMIT);
        if skipped > 0 {
            self.note(&format!("{skipped} earlier lines omitted"), "33");
        }
        let mut buf = String::new();
        for l in missed.iter().skip(skipped) {
            buf.push_str(l);
            buf.push_str("\r\n");
        }
        self.write_out(buf.as_bytes());
        Ok(())
    }

    fn handle_tui_key(&mut self, key: Key) -> Result<()> {
        let Some(tui) = &mut self.tui else {
            return Ok(());
        };
        match tui.on_key(key, &self.sb) {
            TuiAction::Stay => self.draw_tui(),
            TuiAction::Leave => self.leave_tui(),
            TuiAction::Quit => {
                self.leave_tui()?;
                self.quit = true;
                Ok(())
            }
        }
    }

    fn draw_tui(&mut self) -> Result<()> {
        let status = TuiStatus {
            port: self.cfg.path.clone(),
            baud: baud_label(self.cfg.baud),
            frame: self.cfg.frame.to_string(),
            connected: self.port.is_some(),
            rx_bytes: self.rx_bytes,
            ts: self.ts,
            logging: self.log.as_ref().map(|l| l.path().display().to_string()),
            uptime: format_duration(self.started.elapsed()),
        };
        if let Some(tui) = &mut self.tui {
            tui.draw(&self.sb, self.asm.partial(), &status)?;
        }
        Ok(())
    }

    fn shutdown(&mut self) {
        if self.term.in_alt() {
            let _ = self.term.leave_alt();
        }
        if let Some(mut port) = self.port.take() {
            port.close();
        }
        if let Some(log) = &mut self.log {
            let _ = log.note("session ended");
        }
    }
}

fn escape_name(escape: u8) -> char {
    (b'A' + escape - 1) as char
}

fn note_line(text: &str, color: &str, mid_line: bool) -> String {
    let lead = if mid_line { "\r\n" } else { "" };
    format!("{lead}\x1b[{color}m── {text} ──\x1b[0m\r\n")
}

/// A baud rate of 0 means "leave the line speed alone", which virtual ports need.
fn baud_label(baud: u32) -> String {
    if baud == 0 {
        "line speed as-is".to_string()
    } else {
        baud.to_string()
    }
}

pub fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_maps_letter_to_control_byte() {
        assert_eq!(ctrl_byte('a'), 0x01);
        assert_eq!(ctrl_byte('A'), 0x01);
        assert_eq!(ctrl_byte('t'), 0x14);
        assert_eq!(escape_name(0x01), 'A');
        assert_eq!(escape_name(0x14), 'T');
    }

    #[test]
    fn notes_break_out_of_an_unfinished_device_line() {
        // The device left "rst" on screen with no newline; the note must not
        // be glued to it.
        assert!(note_line("disconnected", "33", true).starts_with("\r\n\x1b[33m"));
        assert!(note_line("disconnected", "33", false).starts_with("\x1b[33m"));
        assert!(note_line("disconnected", "33", false).ends_with("──\x1b[0m\r\n"));
    }

    #[test]
    fn zero_baud_is_spelled_out() {
        assert_eq!(baud_label(115200), "115200");
        assert_eq!(baud_label(0), "line speed as-is");
    }

    #[test]
    fn formats_byte_counts() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(2048), "2.0 KB");
        assert_eq!(human_bytes(5 * 1024 * 1024), "5.0 MB");
    }
}
