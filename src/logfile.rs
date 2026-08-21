use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::ansi;
use crate::cli::LogFormat;
use crate::scrollback::Line;

pub struct LogWriter {
    file: BufWriter<File>,
    path: PathBuf,
    format: LogFormat,
}

impl LogWriter {
    pub fn open(path: Option<&str>, format: LogFormat, port: &str) -> Result<Self> {
        let requested: PathBuf = match path {
            Some(p) if !p.is_empty() => PathBuf::from(p),
            _ => PathBuf::from(default_name(port)),
        };
        let file =
            File::create(&requested).with_context(|| format!("cannot create {requested:?}"))?;
        // Reported as an absolute path: "logging to mcom-....log" leaves you
        // guessing which directory the session was started from.
        let cwd = std::env::current_dir().unwrap_or_default();
        let path = absolute(requested, &cwd);
        Ok(LogWriter {
            file: BufWriter::new(file),
            path,
            format,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Raw logs mirror the byte stream; clean logs are written line by line instead.
    pub fn write_chunk(&mut self, data: &[u8]) -> Result<()> {
        if self.format != LogFormat::Raw {
            return Ok(());
        }
        self.file.write_all(data)?;
        self.file.flush()?;
        Ok(())
    }

    pub fn write_line(&mut self, line: &Line) -> Result<()> {
        if self.format != LogFormat::Clean {
            return Ok(());
        }
        let text = ansi::strip(&line.raw);
        let stamp = line.at.format("%Y-%m-%d %H:%M:%S%.3f");
        write!(self.file, "[{stamp}] ")?;
        self.file.write_all(&text)?;
        self.file.write_all(b"\n")?;
        self.file.flush()?;
        Ok(())
    }

    pub fn note(&mut self, text: &str) -> Result<()> {
        let stamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
        // Raw logs mirror a byte stream that may sit mid-line.
        if self.format == LogFormat::Raw {
            self.file.write_all(b"\n")?;
        }
        writeln!(self.file, "[{stamp}] --- {text} ---")?;
        self.file.flush()?;
        Ok(())
    }
}

fn absolute(path: PathBuf, cwd: &Path) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    }
}

fn default_name(port: &str) -> String {
    let leaf = port.rsplit('/').next().unwrap_or("port");
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    format!("mcom-{leaf}-{stamp}.log")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_log_paths_are_reported_from_the_working_directory() {
        let cwd = Path::new("/home/pi");
        assert_eq!(
            absolute(PathBuf::from("mcom-ttyACM0.log"), cwd),
            PathBuf::from("/home/pi/mcom-ttyACM0.log")
        );
        assert_eq!(
            absolute(PathBuf::from("/var/log/mcom.log"), cwd),
            PathBuf::from("/var/log/mcom.log")
        );
    }

    #[test]
    fn default_name_uses_port_leaf() {
        let n = default_name("/dev/cu.usbmodem1101");
        assert!(n.starts_with("mcom-cu.usbmodem1101-"));
        assert!(n.ends_with(".log"));
    }
}
