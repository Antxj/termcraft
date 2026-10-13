//! OSC 7 current-directory reports: `ESC ] 7 ; file://host/path (BEL | ESC \)`.
//! Shells emit these from their prompt hook; the SSH-browser uses them to
//! follow the terminal folder. Sequences may be split across reads.

/// Longest report we accept (anything longer is dropped as garbage).
const MAX_LEN: usize = 4096;
const START: &[u8] = b"\x1b]7;";

#[derive(Default)]
pub struct Osc7 {
    /// Bytes of an unfinished report (after the `ESC ] 7 ;` prefix).
    partial: Option<Vec<u8>>,
    /// Bytes of a possibly split prefix at the end of the last chunk.
    prefix_tail: Vec<u8>,
}

impl Osc7 {
    /// Scan a chunk; returns the last complete directory reported in it.
    pub fn scan(&mut self, bytes: &[u8]) -> Option<String> {
        let mut data = std::mem::take(&mut self.prefix_tail);
        data.extend_from_slice(bytes);
        let mut found = None;
        let mut i = 0;
        while i < data.len() {
            if let Some(buf) = &mut self.partial {
                match data[i] {
                    0x07 => {
                        found = parse_uri(buf).or(found);
                        self.partial = None;
                    }
                    0x1b if data.get(i + 1) == Some(&b'\\') => {
                        found = parse_uri(buf).or(found);
                        self.partial = None;
                        i += 1;
                    }
                    0x1b if i + 1 == data.len() => {
                        // ST may be split: keep ESC for the next chunk.
                        self.prefix_tail.push(0x1b);
                        return found;
                    }
                    b => {
                        buf.push(b);
                        if buf.len() > MAX_LEN {
                            self.partial = None;
                        }
                    }
                }
                i += 1;
                continue;
            }
            if data[i..].starts_with(START) {
                self.partial = Some(Vec::new());
                i += START.len();
                continue;
            }
            // A prefix cut at the end of the chunk.
            let rest = &data[i..];
            if rest.len() < START.len() && START.starts_with(rest) {
                self.prefix_tail = rest.to_vec();
                break;
            }
            i += 1;
        }
        found
    }
}

/// `file://host/some%20dir` → `/some dir`.
fn parse_uri(raw: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(raw).ok()?;
    let rest = text.strip_prefix("file://")?;
    let path = &rest[rest.find('/')?..];
    Some(percent_decode(path))
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(hex) = s.get(i + 1..i + 3)
            && let Ok(b) = u8::from_str_radix(hex, 16)
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bel_and_st_terminators() {
        let mut o = Osc7::default();
        assert_eq!(
            o.scan(b"x\x1b]7;file://box/home/u\x07$ "),
            Some("/home/u".into())
        );
        assert_eq!(o.scan(b"\x1b]7;file://box/tmp\x1b\\"), Some("/tmp".into()));
        assert_eq!(o.scan(b"plain output"), None);
    }

    #[test]
    fn split_across_chunks() {
        let mut o = Osc7::default();
        assert_eq!(o.scan(b"abc\x1b]"), None);
        assert_eq!(o.scan(b"7;file://h/var/l"), None);
        assert_eq!(o.scan(b"og\x1b"), None);
        assert_eq!(o.scan(b"\\prompt"), Some("/var/log".into()));
    }

    #[test]
    fn percent_encoding_and_last_wins() {
        let mut o = Osc7::default();
        let both = b"\x1b]7;file://h/a\x07\x1b]7;file://h/my%20dir\x07";
        assert_eq!(o.scan(both), Some("/my dir".into()));
    }

    #[test]
    fn rejects_other_schemes_and_oversized() {
        let mut o = Osc7::default();
        assert_eq!(o.scan(b"\x1b]7;http://h/x\x07"), None);
        let mut long = b"\x1b]7;file://h/".to_vec();
        long.extend(std::iter::repeat_n(b'a', MAX_LEN + 10));
        long.push(0x07);
        assert_eq!(o.scan(&long), None);
    }
}
