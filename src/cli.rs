use clap::{Parser, ValueEnum};

#[derive(Parser, Debug)]
#[command(
    name = "mcom",
    version,
    about = "Serial terminal that renders colored logs, exits cleanly and always frees the port"
)]
pub struct Cli {
    /// Serial port path (auto-detected when omitted)
    pub port: Option<String>,

    /// Baud rate; 0 leaves the line speed untouched, which is what virtual
    /// ports (pty, socat) need
    #[arg(short, long, default_value_t = 115200)]
    pub baud: u32,

    /// Frame format: data bits, parity, stop bits
    #[arg(short = 'm', long, default_value = "8N1")]
    pub mode: String,

    /// List available ports and exit
    #[arg(short, long)]
    pub list: bool,

    /// Timestamp mode
    #[arg(short = 't', long, value_enum, default_value_t = TsMode::Rel)]
    pub ts: TsMode,

    /// Write session to a file (auto-named when no path is given)
    #[arg(long, value_name = "PATH", num_args = 0..=1, default_missing_value = "")]
    pub log: Option<String>,

    /// Log file contents: raw byte stream or ANSI-stripped lines with timestamps
    #[arg(long, value_enum, default_value_t = LogFormat::Clean)]
    pub log_format: LogFormat,

    /// Scrollback buffer size, in lines
    #[arg(long, default_value_t = 10_000)]
    pub scrollback: usize,

    /// Exit instead of waiting for the device to come back
    #[arg(long)]
    pub no_reconnect: bool,

    /// On reconnect, only accept the exact same port path
    #[arg(long)]
    pub strict_port: bool,

    /// Escape key, given as the letter pressed with Ctrl
    #[arg(long, value_name = "CHAR", default_value = "a")]
    pub escape: char,

    /// Send a local echo of everything typed
    #[arg(long)]
    pub echo: bool,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum TsMode {
    /// No timestamps, byte-for-byte passthrough
    Off,
    /// Seconds since the session started
    Rel,
    /// Local wall-clock time
    Abs,
    /// Time since the previous line
    Delta,
}

impl TsMode {
    pub fn next(self) -> Self {
        match self {
            TsMode::Off => TsMode::Rel,
            TsMode::Rel => TsMode::Abs,
            TsMode::Abs => TsMode::Delta,
            TsMode::Delta => TsMode::Off,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            TsMode::Off => "off",
            TsMode::Rel => "rel",
            TsMode::Abs => "abs",
            TsMode::Delta => "delta",
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum LogFormat {
    /// Exactly what the device sent, escape sequences included
    Raw,
    /// One line per line, ANSI stripped, absolute timestamps
    Clean,
}

/// Frame format parsed from strings like `8N1` or `7E2`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct FrameMode {
    pub data_bits: serialport::DataBits,
    pub parity: serialport::Parity,
    pub stop_bits: serialport::StopBits,
}

impl FrameMode {
    pub fn parse(s: &str) -> anyhow::Result<Self> {
        let b = s.as_bytes();
        if b.len() != 3 {
            anyhow::bail!("expected a 3-character frame format like 8N1, got {s:?}");
        }
        let data_bits = match b[0] {
            b'5' => serialport::DataBits::Five,
            b'6' => serialport::DataBits::Six,
            b'7' => serialport::DataBits::Seven,
            b'8' => serialport::DataBits::Eight,
            _ => anyhow::bail!("data bits must be 5..8, got {:?}", b[0] as char),
        };
        let parity = match b[1].to_ascii_uppercase() {
            b'N' => serialport::Parity::None,
            b'E' => serialport::Parity::Even,
            b'O' => serialport::Parity::Odd,
            _ => anyhow::bail!("parity must be N, E or O, got {:?}", b[1] as char),
        };
        let stop_bits = match b[2] {
            b'1' => serialport::StopBits::One,
            b'2' => serialport::StopBits::Two,
            _ => anyhow::bail!("stop bits must be 1 or 2, got {:?}", b[2] as char),
        };
        Ok(FrameMode {
            data_bits,
            parity,
            stop_bits,
        })
    }
}

impl std::fmt::Display for FrameMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let d = match self.data_bits {
            serialport::DataBits::Five => '5',
            serialport::DataBits::Six => '6',
            serialport::DataBits::Seven => '7',
            serialport::DataBits::Eight => '8',
        };
        let p = match self.parity {
            serialport::Parity::None => 'N',
            serialport::Parity::Even => 'E',
            serialport::Parity::Odd => 'O',
        };
        let s = match self.stop_bits {
            serialport::StopBits::One => '1',
            serialport::StopBits::Two => '2',
        };
        write!(f, "{d}{p}{s}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_frame_modes() {
        let m = FrameMode::parse("8N1").unwrap();
        assert_eq!(m.data_bits, serialport::DataBits::Eight);
        assert_eq!(m.parity, serialport::Parity::None);
        assert_eq!(m.stop_bits, serialport::StopBits::One);
        assert_eq!(m.to_string(), "8N1");

        assert_eq!(FrameMode::parse("7e2").unwrap().to_string(), "7E2");
        assert!(FrameMode::parse("8N").is_err());
        assert!(FrameMode::parse("9N1").is_err());
        assert!(FrameMode::parse("8X1").is_err());
        assert!(FrameMode::parse("8N3").is_err());
    }

    #[test]
    fn timestamp_modes_cycle() {
        let mut m = TsMode::Off;
        for _ in 0..4 {
            m = m.next();
        }
        assert_eq!(m, TsMode::Off);
    }
}
