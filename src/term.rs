use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use crossterm::{cursor, execute};

static RAW: AtomicBool = AtomicBool::new(false);
static ALT: AtomicBool = AtomicBool::new(false);

/// Puts the terminal back the way we found it: raw mode off, alternate screen
/// left, cursor visible, colors reset. Safe to call more than once and from a
/// panic hook.
pub fn restore() {
    let mut out = io::stdout();
    if ALT.swap(false, Ordering::SeqCst) {
        let _ = execute!(out, LeaveAlternateScreen);
    }
    if RAW.swap(false, Ordering::SeqCst) {
        let _ = disable_raw_mode();
    }
    let _ = execute!(out, cursor::Show);
    let _ = out.write_all(b"\x1b[0m\r\n");
    let _ = out.flush();
}

/// Owns the terminal state for the lifetime of a session.
pub struct TermGuard;

impl TermGuard {
    pub fn new() -> Result<Self> {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore();
            prev(info);
        }));
        enable_raw_mode()?;
        RAW.store(true, Ordering::SeqCst);
        Ok(TermGuard)
    }

    pub fn enter_alt(&mut self) -> Result<()> {
        if !ALT.swap(true, Ordering::SeqCst) {
            execute!(io::stdout(), EnterAlternateScreen, cursor::Hide)?;
        }
        Ok(())
    }

    pub fn leave_alt(&mut self) -> Result<()> {
        if ALT.swap(false, Ordering::SeqCst) {
            execute!(io::stdout(), LeaveAlternateScreen, cursor::Show)?;
        }
        Ok(())
    }

    pub fn in_alt(&self) -> bool {
        ALT.load(Ordering::SeqCst)
    }
}

impl Drop for TermGuard {
    fn drop(&mut self) {
        restore();
    }
}
