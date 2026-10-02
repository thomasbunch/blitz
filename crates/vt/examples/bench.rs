//! Parser and screen throughput.
//!
//! ```text
//! cargo run -p vt --release --example bench
//! ```
//!
//! Feeds about 100 MB of plain ASCII, SGR-dense and mixed UTF-8 output
//! into a 120x40 terminal in 64 KiB reads, the size a pane's reader hands
//! over, and prints MB/s for each. If `VT_CORPUS_DIR` is set, the raw
//! `.vt` recordings in it are replayed the same way.

use std::time::Instant;

use vt::{Options, Terminal};

const TOTAL: usize = 100_000_000;
const READ: usize = 64 * 1024;

fn ascii() -> Vec<u8> {
    let mut out = Vec::new();
    for i in 0..10_000u32 {
        let len = 20 + (i * 37 % 100) as usize;
        out.extend((0..len).map(|j| b' ' + ((i as usize + j) % 95) as u8));
        out.extend_from_slice(b"\r\n");
    }
    out
}

/// Short words, each in its own truecolor, some bold or underlined: what
/// syntax-highlighted diffs and Claude Code's UI look like.
fn sgr() -> Vec<u8> {
    let mut out = Vec::new();
    for i in 0..20_000u32 {
        let (r, g, b) = (i * 7 % 256, i * 13 % 256, i * 29 % 256);
        out.extend(format!("\x1b[38;2;{r};{g};{b}m").bytes());
        match i % 5 {
            0 => out.extend_from_slice(b"\x1b[1m"),
            1 => out.extend_from_slice(b"\x1b[4m"),
            2 => out.extend_from_slice(b"\x1b[48;5;236m"),
            _ => {}
        }
        out.extend_from_slice(b"word_");
        out.extend((i % 10).to_string().bytes());
        out.extend_from_slice(b"\x1b[0m ");
        if i % 12 == 11 {
            out.extend_from_slice(b"\r\n");
        }
    }
    out
}

fn utf8() -> Vec<u8> {
    let parts = [
        "plain ascii text ",
        "héllo wörld ",
        "漢字テスト ",
        "─│╭╮╰╯ ",
        "▐▛███▜▌ ",
        "✻ ● ⏵ ",
        "😀👍 ",
        "e\u{301} ",
    ];
    let mut s = String::new();
    for i in 0..30_000 {
        s.push_str(parts[i % parts.len()]);
        if i % 9 == 8 {
            s.push_str("\r\n");
        }
    }
    s.into_bytes()
}

/// Feeds `sample` repeatedly until `TOTAL` bytes went in, then prints the
/// rate.
fn run(name: &str, sample: &[u8]) {
    let mut t = Terminal::new(Options {
        cols: 120,
        rows: 40,
        ..Options::default()
    });
    let stream: Vec<u8> = sample.iter().copied().cycle().take(TOTAL).collect();
    let (mut replies, mut events) = (Vec::new(), Vec::new());
    let start = Instant::now();
    for chunk in stream.chunks(READ) {
        t.feed(chunk);
        t.take_replies(&mut replies);
        t.take_events(&mut events);
        replies.clear();
        events.clear();
    }
    let secs = start.elapsed().as_secs_f64();
    let mb = stream.len() as f64 / 1e6;
    println!("{name:<10} {mb:>6.1} MB  {:>8.1} MB/s", mb / secs);
}

fn main() {
    run("ascii", &ascii());
    run("sgr", &sgr());
    run("utf8", &utf8());
    let Some(dir) = std::env::var_os("VT_CORPUS_DIR") else {
        return;
    };
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .expect("VT_CORPUS_DIR")
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "vt"))
        .collect();
    files.sort();
    let all: Vec<u8> = files
        .iter()
        .flat_map(|f| std::fs::read(f).unwrap())
        .collect();
    if !all.is_empty() {
        run("captures", &all);
    }
}
