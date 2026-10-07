//! Replays recorded sessions of real programs through the terminal.
//!
//! The recordings are not in the repository. Point `VT_CORPUS_DIR` at a
//! directory of `.log` recordings and run
//! `cargo test -p vt --test replay -- --ignored`. Recordings with `claude`
//! in their name are taken to be Claude Code sessions and checked as such.
//!
//! A recording has one event per line, `t_ms \t kind \t payload`. `out`
//! payloads are the program's output with `\e`, `\r`, `\n`, `\\` and
//! `\xNN` escapes. `note` lines mark resizes (`resize 90x30 hr=...`) and
//! points where the screen was looked at (`snap LABEL: ...`); a snap
//! labelled `resize-COLSxROWS` follows a resize. The first line is a `#`
//! header carrying `size=COLSxROWS`.

use std::path::{Path, PathBuf};

use vt::{Options, Terminal};

/// The screen at one `snap` note.
struct Snap {
    label: String,
    text: String,
    alt: bool,
}

struct Replay {
    t: Terminal,
    snaps: Vec<Snap>,
}

/// Undoes the payload escapes. A `\x` without two hex digits after it is
/// kept as it is.
fn unescape(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'\\' || i + 1 == b.len() {
            out.push(b[i]);
            i += 1;
            continue;
        }
        i += 2;
        match b[i - 1] {
            b'e' => out.push(0x1b),
            b'r' => out.push(b'\r'),
            b'n' => out.push(b'\n'),
            b'x' => match b
                .get(i..i + 2)
                .filter(|h| h.iter().all(u8::is_ascii_hexdigit))
            {
                Some(h) => {
                    let h = std::str::from_utf8(h).unwrap();
                    out.push(u8::from_str_radix(h, 16).unwrap());
                    i += 2;
                }
                None => out.extend_from_slice(b"\\x"),
            },
            c => out.push(c),
        }
    }
    out
}

fn size(s: &str) -> Option<(u16, u16)> {
    let (w, h) = s.split_once('x')?;
    Some((w.parse().ok()?, h.parse().ok()?))
}

/// Xorshift, so split points are the same on every run.
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % n as u64) as usize
    }
}

/// Feeds a recording. With `seed`, every output chunk is cut at random
/// points first.
fn replay_log(log: &str, seed: Option<u64>) -> Replay {
    let mut lines = log.lines();
    let header = lines.next().unwrap_or_default();
    let (cols, rows) = header
        .split_whitespace()
        .find_map(|w| w.strip_prefix("size=").and_then(size))
        .unwrap_or((120, 40));
    let mut t = Terminal::new(Options {
        cols,
        rows,
        ..Options::default()
    });
    let mut rng = seed.map(|s| Rng(s | 1));
    let mut snaps = Vec::new();
    for line in lines {
        let mut f = line.splitn(3, '\t');
        let (_, kind, payload) = (f.next(), f.next(), f.next().unwrap_or_default());
        match kind {
            Some("out") => {
                let bytes = unescape(payload);
                match &mut rng {
                    None => t.feed(&bytes),
                    Some(rng) => {
                        let mut rest = &bytes[..];
                        while !rest.is_empty() {
                            let n = 1 + rng.below(rest.len().min(64));
                            t.feed(&rest[..n]);
                            rest = &rest[n..];
                        }
                    }
                }
            }
            Some("note") => {
                if let Some(r) = payload.strip_prefix("resize ") {
                    let (w, h) = size(r.split(' ').next().unwrap()).unwrap();
                    t.resize(w, h);
                } else if let Some(s) = payload.strip_prefix("snap ") {
                    snaps.push(Snap {
                        label: s.split(':').next().unwrap().to_string(),
                        text: t.screen_text(),
                        alt: t.input_modes().alt_screen,
                    });
                }
            }
            _ => {}
        }
    }
    Replay { t, snaps }
}

fn replay(path: &Path, seed: Option<u64>) -> Replay {
    replay_log(&std::fs::read_to_string(path).unwrap(), seed)
}

/// The recordings in `VT_CORPUS_DIR`, sorted.
fn recordings() -> Vec<PathBuf> {
    let dir = std::env::var_os("VT_CORPUS_DIR")
        .expect("set VT_CORPUS_DIR to a directory of .log recordings");
    let mut logs: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "log"))
        .collect();
    logs.sort();
    assert!(!logs.is_empty(), "no .log recordings");
    logs
}

fn rows(text: &str) -> Vec<&str> {
    text.split('\n').collect()
}

fn rule(n: usize) -> String {
    "─".repeat(n)
}

/// Fails with the whole screen, so a broken replay can be read.
fn check(ok: bool, what: &str, s: &Snap) {
    assert!(ok, "{what} at snap {}:\n{}", s.label, s.text);
}

/// Claude Code's full-screen UI at `cols` wide: the input box ruled above
/// and below, and no stray key-report text.
fn claude_screen(s: &Snap, cols: usize) {
    let r = rows(&s.text);
    check(s.alt, "alt screen", s);
    let rules = r.iter().filter(|l| **l == rule(cols)).count();
    check(rules >= 2, &format!("two rows of {cols} rules"), s);
    check(!s.text.contains("AAAA"), "no AAAA", s);
    check(!r[0].contains('u'), "no u on row 0", s);
}

#[test]
#[ignore = "needs VT_CORPUS_DIR"]
fn replay_captures() {
    let claude: Vec<_> = (recordings().into_iter())
        .filter(|p| p.to_string_lossy().to_lowercase().contains("claude"))
        .collect();
    assert!(!claude.is_empty(), "no recordings with claude in the name");
    for log in &claude {
        let r = replay(log, None);
        for s in &r.snaps {
            if let Some((cols, _)) = s.label.strip_prefix("resize-").and_then(size) {
                claude_screen(s, cols.into());
            }
        }
        // Two lines typed into the input box stay one under the other.
        // Claude puts a no-break space after its `>` prompt.
        if let Some(s) = r.snaps.iter().find(|s| s.label == "cand-b") {
            let text = s.text.replace('\u{a0}', " ");
            let ruled = |l: &str| !l.is_empty() && l.chars().all(|c| c == '─');
            let found = rows(&text)
                .windows(4)
                .any(|w| ruled(w[0]) && w[1] == "> line1" && w[2] == "  line2" && ruled(w[3]));
            check(found, "\"> line1\" over \"  line2\" between rules", s);
        }
        // After `/exit` the full-screen UI is gone and the resume hint is
        // left on the main screen.
        let text = r.t.screen_text();
        if text.contains("Resume this session with:") {
            assert!(!r.t.input_modes().alt_screen, "{}", log.display());
            assert!(!text.contains("Claude Code v"), "{text}");
        }
    }
}

/// Every recording replays without a panic, and cutting its output at
/// random points leaves the same screen at every snap and at the end.
#[test]
#[ignore = "needs VT_CORPUS_DIR"]
fn replay_captures_in_random_chunks() {
    for log in &recordings() {
        let whole = replay(log, None);
        let name = log.file_name().unwrap().to_string_lossy();
        for seed in 1..=500 {
            let cut = replay(log, Some(seed));
            assert_eq!(whole.snaps.len(), cut.snaps.len());
            for (a, b) in whole.snaps.iter().zip(&cut.snaps) {
                assert_eq!(a.text, b.text, "{name} seed {seed} snap {}", a.label);
            }
            assert_eq!(whole.t.screen_text(), cut.t.screen_text(), "{name} {seed}");
            assert_eq!(whole.t.cursor(), cut.t.cursor(), "{name} {seed}");
        }
    }
}

#[test]
fn unescape_reads_every_escape() {
    assert_eq!(
        unescape(r"\e[1m\r\n\\\x41\x7f"),
        b"\x1b[1m\r\n\\A\x7f".to_vec()
    );
    // Cut short or not hex: kept as written, never a panic.
    assert_eq!(unescape(r"a\x4"), br"a\x4".to_vec());
    assert_eq!(unescape(r"\x"), br"\x".to_vec());
    assert_eq!(unescape(r"\xzz1"), br"\xzz1".to_vec());
    assert_eq!(unescape(r"\x+f"), br"\x+f".to_vec());
    assert_eq!(unescape("a\\"), b"a\\".to_vec());
}

/// The recording format itself, on a synthetic session, so the replay
/// code is exercised without a corpus.
#[test]
fn replay_of_a_small_recording() {
    let log = concat!(
        "# size=10x3 synthetic\n",
        "0\tout\t$ \\e[1mhello\\e[0m\\r\\n\n",
        "1\tnote\tsnap first: after hello\n",
        "2\tout\t\\e[?1049h\\e[Hfull\\x21\n",
        "3\tnote\tresize 6x2 hr=0\n",
        "4\tnote\tsnap resize-6x2: on the alternate screen\n",
        "5\tout\t\\e[?1049lbye\n",
        "6\tin\tignored\n",
    );
    let r = replay_log(log, None);
    let labels: Vec<_> = r.snaps.iter().map(|s| s.label.as_str()).collect();
    assert_eq!(labels, ["first", "resize-6x2"]);
    assert_eq!(
        (r.snaps[0].text.as_str(), r.snaps[0].alt),
        ("$ hello\n\n", false)
    );
    assert_eq!(
        (r.snaps[1].text.as_str(), r.snaps[1].alt),
        ("full!\n", true)
    );
    // The main screen rewrapped under the alternate one, and its cursor
    // came back where it was saved.
    assert_eq!(r.t.scrollback_text(), "$ hell");
    assert_eq!(r.t.screen_text(), "o\nbye");
    for seed in 1..=50 {
        let cut = replay_log(log, Some(seed));
        assert_eq!(cut.t.screen_text(), r.t.screen_text(), "seed {seed}");
    }
}
