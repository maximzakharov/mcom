use std::io;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use serialport::{SerialPort, TTYPort};

use crate::cli::FrameMode;
use crate::ports;

pub const READ_TIMEOUT: Duration = Duration::from_millis(50);

#[derive(Clone, Debug)]
pub struct PortConfig {
    pub path: String,
    pub baud: u32,
    pub frame: FrameMode,
}

pub fn open(cfg: &PortConfig) -> Result<TTYPort> {
    let port = serialport::new(&cfg.path, cfg.baud)
        .data_bits(cfg.frame.data_bits)
        .parity(cfg.frame.parity)
        .stop_bits(cfg.frame.stop_bits)
        .flow_control(serialport::FlowControl::None)
        .timeout(READ_TIMEOUT)
        .open_native()
        .map_err(|e| describe_open_error(&cfg.path, e))?;

    lock_exclusive(&port, &cfg.path)?;
    Ok(port)
}

fn describe_open_error(path: &str, e: serialport::Error) -> anyhow::Error {
    let hint = match e.kind() {
        serialport::ErrorKind::NoDevice => {
            format!("{path} is not there — is the board plugged in? Try `mcom --list`.")
        }
        serialport::ErrorKind::Io(io::ErrorKind::PermissionDenied) => format!(
            "no permission to open {path} — on Linux add yourself to the `dialout` group, \
             or check that nothing else holds it: `lsof {path}`"
        ),
        serialport::ErrorKind::Io(io::ErrorKind::ResourceBusy) => format!(
            "{path} is busy — another program still holds it. \
             A detached `screen` session is the usual culprit: `screen -ls`, then `lsof {path}`."
        ),
        _ => format!("cannot open {path}"),
    };
    anyhow::Error::new(e).context(hint)
}

#[cfg(target_os = "linux")]
fn lock_exclusive(port: &TTYPort, path: &str) -> Result<()> {
    use std::os::unix::io::AsRawFd;
    let rc = unsafe { libc::flock(port.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc != 0 {
        let err = io::Error::last_os_error();
        if err.kind() == io::ErrorKind::WouldBlock {
            anyhow::bail!(
                "{path} is locked by another process — check with `lsof {path}` \
                 (a detached `screen` session is the usual culprit)"
            );
        }
        return Err(anyhow::Error::new(err).context(format!("cannot lock {path}")));
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn lock_exclusive(_port: &TTYPort, _path: &str) -> Result<()> {
    // macOS: serialport already sets TIOCEXCL on the callout device.
    Ok(())
}

pub fn writer(port: &TTYPort) -> Result<TTYPort> {
    port.try_clone_native().context("cannot clone port handle")
}

pub fn send_break(port: &mut TTYPort) -> Result<()> {
    port.set_break()?;
    std::thread::sleep(Duration::from_millis(250));
    port.clear_break()?;
    Ok(())
}

/// Finds the port to reconnect to. The original path comes back first; failing
/// that, and unless pinned, a single new USB port is taken as the same board
/// under a new name — macOS renumbers `usbmodem` devices across resets.
pub fn find_reconnect_target(original: &str, strict: bool) -> Option<String> {
    if Path::new(original).exists() {
        return Some(original.to_string());
    }
    if strict {
        return None;
    }
    let usb: Vec<String> = ports::list()
        .into_iter()
        .filter(|p| p.is_usb)
        .map(|p| p.path)
        .collect();
    match usb.len() {
        1 => Some(usb.into_iter().next().unwrap()),
        _ => None,
    }
}

/// Read errors that mean the device went away rather than "nothing to read".
pub fn is_disconnect(e: &io::Error) -> bool {
    !matches!(
        e.kind(),
        io::ErrorKind::TimedOut | io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeouts_are_not_disconnects() {
        assert!(!is_disconnect(&io::Error::from(io::ErrorKind::TimedOut)));
        assert!(!is_disconnect(&io::Error::from(io::ErrorKind::Interrupted)));
        assert!(!is_disconnect(&io::Error::from(io::ErrorKind::WouldBlock)));
    }

    #[test]
    fn hangups_are_disconnects() {
        assert!(is_disconnect(&io::Error::from(io::ErrorKind::NotFound)));
        assert!(is_disconnect(&io::Error::from(io::ErrorKind::BrokenPipe)));
        assert!(is_disconnect(&io::Error::from(io::ErrorKind::Other)));
    }

    #[test]
    fn missing_port_has_no_target_when_pinned() {
        assert_eq!(
            find_reconnect_target("/dev/definitely-not-here", true),
            None
        );
    }
}
