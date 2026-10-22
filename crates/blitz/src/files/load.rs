//! Reading a file for the Files view's preview.

use std::io::Read;
use std::path::Path;

/// Bytes of a file the preview reads; past them it says the rest is not
/// shown.
pub const MAX_BYTES: u64 = 1 << 20;

/// Bytes looked at for a NUL, which marks a file as binary, as git does.
const SNIFF: usize = 8000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Preview {
    Text {
        /// Each line without its line break.
        lines: Vec<String>,
        /// Only the first [`MAX_BYTES`] were read.
        cut: bool,
    },
    /// Not text: its size in bytes.
    Binary(u64),
    /// Why it could not be read.
    Error(String),
}

/// Reads the start of the file at `path`.
pub fn read(path: &Path) -> Preview {
    let file = std::fs::File::open(path);
    let mut file = match file {
        Ok(f) => f,
        Err(e) => return Preview::Error(super::listing::reason(&e)),
    };
    let size = file.metadata().map_or(0, |m| m.len());
    let mut bytes = Vec::with_capacity(size.min(MAX_BYTES) as usize);
    if let Err(e) = (&mut file).take(MAX_BYTES).read_to_end(&mut bytes) {
        return Preview::Error(super::listing::reason(&e));
    }
    decode(&bytes, size.max(bytes.len() as u64))
}

/// The preview of a file of `size` bytes that starts with `bytes`.
pub fn decode(bytes: &[u8], size: u64) -> Preview {
    let cut = size > bytes.len() as u64;
    let text = match bytes {
        [0xff, 0xfe, rest @ ..] => utf16(rest, u16::from_le_bytes),
        [0xfe, 0xff, rest @ ..] => utf16(rest, u16::from_be_bytes),
        _ => {
            let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
            if bytes[..bytes.len().min(SNIFF)].contains(&0) {
                return Preview::Binary(size);
            }
            // A cut can split a character, so a cut file ends at its last
            // whole line, if it has one.
            let bytes = if cut {
                let end = (bytes.iter().rposition(|&b| b == b'\n')).map_or(bytes.len(), |i| i + 1);
                &bytes[..end]
            } else {
                bytes
            };
            String::from_utf8_lossy(bytes).into_owned()
        }
    };
    let mut lines: Vec<String> = (text.split('\n'))
        .map(|l| l.strip_suffix('\r').unwrap_or(l).to_owned())
        .collect();
    // A last line break ends a line rather than starting one.
    if lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    Preview::Text { lines, cut }
}

fn utf16(bytes: &[u8], unit: fn([u8; 2]) -> u16) -> String {
    let units = bytes.as_chunks::<2>().0.iter().map(|&p| unit(p));
    char::decode_utf16(units)
        .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(lines: &[&str], cut: bool) -> Preview {
        Preview::Text {
            lines: lines.iter().map(|&l| l.into()).collect(),
            cut,
        }
    }

    #[test]
    fn lines_lose_their_breaks_whatever_the_kind() {
        assert_eq!(decode(b"a\r\nb\nc", 6), text(&["a", "b", "c"], false));
        assert_eq!(decode(b"a\n", 2), text(&["a"], false));
        assert_eq!(decode(b"a\n\n", 3), text(&["a", ""], false));
        assert_eq!(decode(b"", 0), text(&[], false));
    }

    #[test]
    fn byte_order_marks_say_how_to_read() {
        assert_eq!(decode(b"\xef\xbb\xbfhi", 5), text(&["hi"], false));
        assert_eq!(decode(b"\xff\xfeh\0i\0", 6), text(&["hi"], false));
        assert_eq!(decode(b"\xfe\xff\0h\0i", 6), text(&["hi"], false));
    }

    #[test]
    fn a_nul_means_binary() {
        assert_eq!(decode(b"MZ\x90\0\x03", 4096), Preview::Binary(4096));
    }

    #[test]
    fn other_encodings_still_show() {
        // Windows-1252, as old files on Windows often are.
        assert_eq!(decode(b"caf\xe9", 4), text(&["caf\u{fffd}"], false));
    }

    #[test]
    fn a_cut_file_ends_at_its_last_whole_line() {
        let start = "one\ntwo\nthr\u{e9}".as_bytes();
        // The cut falls inside the two bytes of the last character.
        assert_eq!(
            decode(&start[..start.len() - 1], 100),
            text(&["one", "two"], true)
        );
    }

    #[test]
    fn files_are_read_up_to_the_limit() {
        let d = std::env::temp_dir().join(format!("blitz-load-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let big = d.join("big.txt");
        let line = "0123456789abcdef\n".repeat(1 << 17);
        std::fs::write(&big, &line).unwrap();
        let Preview::Text { lines, cut } = read(&big) else {
            panic!("not text");
        };
        assert!(cut);
        assert_eq!(lines.len(), (MAX_BYTES / 17) as usize);
        assert!(matches!(read(&d.join("none")), Preview::Error(_)));
        std::fs::remove_dir_all(&d).unwrap();
    }
}
