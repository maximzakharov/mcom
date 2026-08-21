#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Ctrl(char),
    Enter,
    Esc,
    Tab,
    Backspace,
    Delete,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Unknown,
}

/// Turns the raw stdin byte stream into keys for the TUI. Passthrough mode never
/// uses this: there, bytes go to the port untouched.
#[derive(Default)]
pub struct KeyDecoder {
    buf: Vec<u8>,
}

impl KeyDecoder {
    pub fn push(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    /// A lone ESC is only known to be ESC once no continuation arrives.
    pub fn flush_escape(&mut self) -> Option<Key> {
        if self.buf == [0x1b] {
            self.buf.clear();
            return Some(Key::Esc);
        }
        None
    }

    pub fn next_key(&mut self) -> Option<Key> {
        let first = *self.buf.first()?;
        match first {
            0x1b => self.decode_escape(),
            b'\r' | b'\n' => self.take(1, Key::Enter),
            b'\t' => self.take(1, Key::Tab),
            0x7f | 0x08 => self.take(1, Key::Backspace),
            0x01..=0x1a => {
                let c = (b'a' + first - 1) as char;
                self.take(1, Key::Ctrl(c))
            }
            0x00..=0x1f => self.take(1, Key::Unknown),
            _ => self.decode_utf8(),
        }
    }

    fn take(&mut self, n: usize, key: Key) -> Option<Key> {
        self.buf.drain(..n);
        Some(key)
    }

    fn decode_escape(&mut self) -> Option<Key> {
        match self.buf.get(1) {
            None => None,
            Some(b'[') => {
                let end = self.buf[2..]
                    .iter()
                    .position(|b| (0x40..=0x7e).contains(b))?
                    + 2;
                let params = &self.buf[2..end];
                let key = match (params, self.buf[end]) {
                    (_, b'A') => Key::Up,
                    (_, b'B') => Key::Down,
                    (_, b'C') => Key::Right,
                    (_, b'D') => Key::Left,
                    (_, b'H') => Key::Home,
                    (_, b'F') => Key::End,
                    (b"1", b'~') | (b"7", b'~') => Key::Home,
                    (b"4", b'~') | (b"8", b'~') => Key::End,
                    (b"3", b'~') => Key::Delete,
                    (b"5", b'~') => Key::PageUp,
                    (b"6", b'~') => Key::PageDown,
                    _ => Key::Unknown,
                };
                self.take(end + 1, key)
            }
            Some(b'O') => {
                let c = *self.buf.get(2)?;
                let key = match c {
                    b'A' => Key::Up,
                    b'B' => Key::Down,
                    b'C' => Key::Right,
                    b'D' => Key::Left,
                    b'H' => Key::Home,
                    b'F' => Key::End,
                    _ => Key::Unknown,
                };
                self.take(3, key)
            }
            // Alt-modified keys and anything else two bytes long
            Some(_) => self.take(2, Key::Unknown),
        }
    }

    fn decode_utf8(&mut self) -> Option<Key> {
        let first = self.buf[0];
        let len = match first {
            0x00..=0x7f => 1,
            0xc0..=0xdf => 2,
            0xe0..=0xef => 3,
            0xf0..=0xf7 => 4,
            _ => return self.take(1, Key::Unknown),
        };
        if self.buf.len() < len {
            return None;
        }
        match std::str::from_utf8(&self.buf[..len]) {
            Ok(s) => {
                let c = s.chars().next()?;
                self.take(len, Key::Char(c))
            }
            Err(_) => self.take(1, Key::Unknown),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(data: &[u8]) -> Vec<Key> {
        let mut d = KeyDecoder::default();
        d.push(data);
        let mut out = Vec::new();
        while let Some(k) = d.next_key() {
            out.push(k);
        }
        out
    }

    #[test]
    fn decodes_plain_characters() {
        assert_eq!(keys(b"ab"), vec![Key::Char('a'), Key::Char('b')]);
    }

    #[test]
    fn decodes_control_keys() {
        assert_eq!(keys(b"\x01"), vec![Key::Ctrl('a')]);
        assert_eq!(keys(b"\x03"), vec![Key::Ctrl('c')]);
        assert_eq!(keys(b"\r"), vec![Key::Enter]);
        assert_eq!(keys(b"\x7f"), vec![Key::Backspace]);
    }

    #[test]
    fn decodes_arrows_and_paging() {
        assert_eq!(keys(b"\x1b[A\x1b[B"), vec![Key::Up, Key::Down]);
        assert_eq!(keys(b"\x1b[5~\x1b[6~"), vec![Key::PageUp, Key::PageDown]);
        assert_eq!(keys(b"\x1bOA"), vec![Key::Up]);
        assert_eq!(keys(b"\x1b[1;5A"), vec![Key::Up]);
    }

    #[test]
    fn waits_for_incomplete_sequences() {
        let mut d = KeyDecoder::default();
        d.push(b"\x1b[");
        assert_eq!(d.next_key(), None);
        d.push(b"C");
        assert_eq!(d.next_key(), Some(Key::Right));
    }

    #[test]
    fn lone_escape_resolves_on_flush_only() {
        let mut d = KeyDecoder::default();
        d.push(b"\x1b");
        assert_eq!(d.next_key(), None);
        assert_eq!(d.flush_escape(), Some(Key::Esc));
        assert_eq!(d.flush_escape(), None);
    }

    #[test]
    fn decodes_multibyte_characters() {
        assert_eq!(keys("привет".as_bytes()).len(), 6);
        assert_eq!(keys("ж".as_bytes()), vec![Key::Char('ж')]);

        let mut d = KeyDecoder::default();
        d.push(&"ж".as_bytes()[..1]);
        assert_eq!(d.next_key(), None);
        d.push(&"ж".as_bytes()[1..]);
        assert_eq!(d.next_key(), Some(Key::Char('ж')));
    }
}
