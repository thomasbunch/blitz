//! Replays recorded sessions of real programs through the terminal.
//!
//! The recordings are not in the repository. Point `VT_CORPUS_DIR` at a
//! directory of `.log` recordings and run
//! `cargo test -p vt --test replay -- --ignored`.
//!
//! A recording has one event per line, `t_ms \t kind \t payload`. `out`
//! payloads are the program's output with `\e`, `\r`, `\n`, `\\` and
//! `\xNN` escapes. `note` lines mark resizes (`resize 90x30 hr=...`) and
//! points where the screen was looked at (`snap LABEL: ...`). The first
//! line is a `#` header carrying `size=COLSxROWS`.

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

impl Replay {
    fn snap(&self, label: &str) -> &Snap {
        self.snaps
            .iter()
            .find(|s| s.label == label)
            .unwrap_or_else(|| panic!("no snap {label}"))
    }
}

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
            b'x' => {
                let hex = std::str::from_utf8(&b[i..i + 2]).unwrap();
                out.push(u8::from_str_radix(hex, 16).unwrap());
                i += 2;
            }
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
fn replay(path: &Path, seed: Option<u64>) -> Replay {
    let log = std::fs::read_to_string(path).unwrap();
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

fn captures() -> PathBuf {
    let dir = std::env::var_os("VT_CORPUS_DIR")
        .expect("set VT_CORPUS_DIR to a directory of .log recordings");
    PathBuf::from(dir)
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

/// Claude Code's full-screen UI at `cols` wide: its banner, the input box
/// ruled above and below, and no stray key-report text.
fn claude_screen(s: &Snap, cols: usize, banner: bool) {
    let r = rows(&s.text);
    check(s.alt, "alt screen", s);
    if banner {
        check(s.text.contains("Claude Code v2.1.287"), "banner", s);
    }
    let rules = r.iter().filter(|l| **l == rule(cols)).count();
    check(rules >= 2, &format!("two rows of {cols} rules"), s);
    check(!s.text.contains("AAAA"), "no AAAA", s);
    check(!r[0].contains('u'), "no u on row 0", s);
}

/// Claude Code's main screen after `/exit`: the full-screen UI is gone and
/// the resume hint is left.
fn claude_exited(r: &Replay) {
    let text = r.t.screen_text();
    assert!(!r.t.input_modes().alt_screen);
    assert!(text.contains("Resume this session with:"), "{text}");
    assert!(!text.contains("Claude Code v2.1.287"), "{text}");
}

#[test]
#[ignore = "needs VT_CORPUS_DIR"]
fn replay_captures() {
    let dir = captures();

    let r = replay(&dir.join("e2_claude_bundled.log"), None);
    let s = r.snap("cand-b");
    claude_screen(s, 120, true);
    // Claude puts a no-break space after its `>` prompt.
    let text = s.text.replace('\u{a0}', " ");
    let ruled = rule(120);
    let input = [ruled.as_str(), "> line1", "  line2", ruled.as_str()];
    let found = rows(&text).windows(4).any(|w| w == input);
    check(found, "\"> line1\" over \"  line2\" between rules", s);
    claude_screen(r.snap("resize-90x30"), 90, true);
    claude_screen(r.snap("resize-120x40"), 120, true);
    claude_exited(&r);

    let r = replay(&dir.join("e2_claude_bundled_nokitty.log"), None);
    claude_screen(r.snap("resize-90x30"), 90, false);
    claude_screen(r.snap("resize-120x40"), 120, false);
    assert!(!r.t.input_modes().alt_screen);

    // Inbox ConPTY repaints after a resize itself; the result must match.
    let r = replay(&dir.join("e2_claude_inbox.log"), None);
    claude_screen(r.snap("resize-90x30"), 90, true);
    claude_screen(r.snap("resize-120x40"), 120, true);
}

/// Every recording replays without a panic, and cutting its output at
/// random points leaves the same screen at every snap and at the end.
#[test]
#[ignore = "needs VT_CORPUS_DIR"]
fn replay_captures_in_random_chunks() {
    let mut logs: Vec<_> = std::fs::read_dir(captures())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "log"))
        .collect();
    logs.sort();
    assert!(!logs.is_empty());
    for log in &logs {
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
