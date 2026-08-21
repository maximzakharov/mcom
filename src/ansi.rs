/// Removes escape sequences so log files and regex filters see plain text.
pub fn strip(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        if input[i] != 0x1b {
            out.push(input[i]);
            i += 1;
            continue;
        }
        i += 1;
        match input.get(i) {
            // CSI: parameters and intermediates, then one final byte
            Some(b'[') => {
                i += 1;
                while i < input.len() && !(0x40..=0x7e).contains(&input[i]) {
                    i += 1;
                }
                i += 1;
            }
            // OSC and friends: terminated by BEL or ST
            Some(b']') | Some(b'P') | Some(b'X') | Some(b'^') | Some(b'_') => {
                i += 1;
                while i < input.len() {
                    if input[i] == 0x07 {
                        i += 1;
                        break;
                    }
                    if input[i] == 0x1b && input.get(i + 1) == Some(&b'\\') {
                        i += 2;
                        break;
                    }
                    i += 1;
                }
            }
            // Two-byte sequence such as ESC ( B
            Some(_) => i += 2,
            None => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(b: &[u8]) -> String {
        String::from_utf8_lossy(&strip(b)).into_owned()
    }

    #[test]
    fn strips_sgr_colors() {
        assert_eq!(s(b"\x1b[32mI (123) wifi:\x1b[0m up"), "I (123) wifi: up");
    }

    #[test]
    fn strips_osc_titles() {
        assert_eq!(s(b"\x1b]0;title\x07rest"), "rest");
        assert_eq!(s(b"\x1b]0;title\x1b\\rest"), "rest");
    }

    #[test]
    fn keeps_plain_text_and_control_chars() {
        assert_eq!(s(b"plain\ttext\r\n"), "plain\ttext\r\n");
    }

    #[test]
    fn tolerates_truncated_sequences() {
        assert_eq!(s(b"abc\x1b["), "abc");
        assert_eq!(s(b"abc\x1b"), "abc");
    }
}
