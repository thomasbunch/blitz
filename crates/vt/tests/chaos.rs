//! Seeded random streams: the terminal must never panic, must keep the
//! cursor on the screen and wide characters whole, and must end in the
//! same state however the stream is split into chunks.
//!
//! `CHAOS_SEED` and `CHAOS_ROUNDS` replace the fixed seed and round count,
//! for long runs on fresh seeds. A failure names the seed to rerun with.

use vt::{Event, Options, Palette, Snapshot, Terminal};

const PAL: Palette = Palette {
    fg: 0xC0C0C0,
    bg: 0x101010,
    cursor: 0xFFFFFF,
    cursor_text: None,
    selection_bg: 0x333333,
    selection_fg: 0xC0C0C0,
    ansi: [0; 16],
};

/// Pieces that random streams are built from: sequence starts and ends,
/// parameters, controls, modes and text of every width.
const PIECES: &[&str] = &[
    "\x1b[",
    "\x1b]",
    "\x1bP",
    "\x1b_",
    "\x1b^",
    "\x1bX",
    "\x1b\\",
    "\x1b",
    "\x07",
    "\x18",
    "\x1a",
    "\x7f",
    "\r",
    "\n",
    "\r\n",
    "\x08",
    "\t",
    "\x0e",
    "\x0f",
    ";",
    ":",
    "?",
    ">",
    "<",
    "=",
    "$",
    "!",
    " ",
    "0",
    "1",
    "2",
    "5",
    "9",
    "12",
    "65535",
    "99999",
    "m",
    "A",
    "B",
    "C",
    "D",
    "E",
    "F",
    "G",
    "H",
    "I",
    "J",
    "K",
    "Z",
    "a",
    "d",
    "e",
    "f",
    "g",
    "s",
    "`",
    "r",
    "h",
    "l",
    "c",
    "n",
    "p",
    "u",
    "q",
    " q",
    "t",
    "@",
    "P",
    "X",
    "L",
    "M",
    "S",
    "T",
    "b",
    "\x1b[?1049h",
    "\x1b[?1049l",
    "\x1b[?47h",
    "\x1b[?47l",
    "\x1b[?1047h",
    "\x1b[?1047l",
    "\x1b[?1048h",
    "\x1b[?1048l",
    "\x1b[?6h",
    "\x1b[?6l",
    "\x1b[?7l",
    "\x1b[?7h",
    "\x1b[?25l",
    "\x1b[20h",
    "\x1b[20l",
    "\x1b[4l",
    "\x1b[3 q",
    "\x1bH",
    "\x1bE",
    "\x1b[3g",
    "\x1b[2;99r",
    "\x1b[8m",
    "\x1b[38;5;232m",
    "\x1b[?2026h",
    "\x1b[?2026l",
    "\x1b[>5u",
    "\x1b[<9u",
    "\x1b[=3;2u",
    "\x1b[?u",
    "\x1b[>4;2m",
    "\x1b[c",
    "\x1b[>0q",
    "\x1b[6n",
    "\x1b[?6n",
    "\x1b[?2026$p",
    "\x1b[18t",
    "\x1b[3;7r",
    "\x1b[r",
    "\x1b[2J",
    "\x1b[3J",
    "\x1b[999;999H",
    "\x1b[5b",
    "\x1bc",
    "\x1b[!p",
    "\x1b7",
    "\x1b8",
    "\x1bM",
    "\x1bD",
    "\x1b(0",
    "\x1b(B",
    "\x1b=",
    "\x1b]0;title\x07",
    "\x1b]8;id=x;http://a\x1b\\",
    "\x1b]8;;\x1b\\",
    "\x1b]777;notify;blitz:done;m\x07",
    "\x1b]133;A;blitz=1\x07",
    "\x1b]11;?\x07",
    "\x1b]11;#123\x07",
    "\x1b]9;4;3\x07",
    "\x1b]7;file:///C:/x\x07",
    "\x1b_Gi=1;AAAA\x1b\\",
    "\x1b[38:2::1:2:3m",
    "\x1b[4:3;58;5;9m",
    "\x1b[0m",
    "\x1b[7m",
    "\x1b[44m",
    "\x1b[4h",
    "abc",
    "hello world ",
    "中文",
    "é",
    "e\u{301}",
    "👍🏽",
    "👨‍👩‍👧",
    "👨‍👩‍👧‍👦",
    "\u{1F3F4}\u{E0067}\u{E0062}\u{E0073}\u{E0063}\u{E0074}\u{E007F}",
    "e\u{301}\u{301}\u{301}\u{301}\u{301}\u{301}\u{301}\u{301}\u{301}\u{301}\u{301}\u{301}\u{301}\u{301}\u{301}",
    "\u{20D0}\u{20D0}\u{20D0}\u{20D0}\u{20D0}\u{20D0}\u{20D0}\u{20D0}\u{20D0}\u{20D0}",
    "🇺🇸",
    "❤\u{fe0f}",
    "\u{200d}",
    "─│╭╮",
    "▐▛███▜▌",
    "\u{fffd}",
    "\u{9b}",
];

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

fn stream(rng: &mut Rng, pieces: usize) -> Vec<u8> {
    let mut out = Vec::new();
    for _ in 0..pieces {
        if rng.below(10) == 0 {
            // A raw byte, often half of a UTF-8 character.
            out.push(rng.next() as u8);
        } else {
            out.extend_from_slice(PIECES[rng.below(PIECES.len())].as_bytes());
        }
    }
    out
}

fn new_term(rng: &mut Rng) -> Terminal {
    Terminal::new(Options {
        cols: 1 + rng.below(40) as u16,
        rows: 1 + rng.below(12) as u16,
        scrollback_lines: rng.below(50),
        ambiguous_wide: rng.below(2) == 0,
    })
}

/// The fixed default, or the number in the environment variable `name`.
fn env(name: &str, default: u64) -> u64 {
    match std::env::var(name) {
        Ok(v) => {
            (v.trim().parse()).unwrap_or_else(|_| panic!("{name}={v:?} is not a decimal number"))
        }
        Err(_) => default,
    }
}

/// Ends any synchronized update, so there is a snapshot to take, and
/// checks what must hold whatever came before: the cursor on the screen,
/// each wide character next to its right half and each right half next
/// to its wide character, and whole UTF-8 in every cell.
fn check(t: &mut Terminal, ctx: &str) -> Snapshot {
    t.feed(b"\x1b[?2026l");
    let mut s = Snapshot::default();
    assert!(t.snapshot(&mut s, &PAL), "{ctx}: no snapshot");
    let (cols, rows) = (usize::from(s.cols), usize::from(s.rows));
    let (x, y, _) = t.cursor();
    assert!(
        x < s.cols && y < s.rows,
        "{ctx}: cursor {x},{y} outside {cols}x{rows}"
    );
    if let Some((x, y, _)) = s.cursor {
        assert!(x < s.cols && y < s.rows, "{ctx}: drawn cursor {x},{y}");
    }
    assert_eq!(
        (s.cells.len(), s.wrapped.len()),
        (cols * rows, rows),
        "{ctx}"
    );
    for (r, row) in s.cells.chunks(cols).enumerate() {
        for (i, c) in row.iter().enumerate() {
            let text = c.text.get(..usize::from(c.len)).map(std::str::from_utf8);
            assert!(matches!(text, Some(Ok(_))), "{ctx}: cell {i},{r}: {c:?}");
            match c.width {
                0 => assert!(
                    i > 0 && row[i - 1].width == 2 && c.len == 0,
                    "{ctx}: right half alone at {i},{r}"
                ),
                1 => {}
                2 => assert_eq!(
                    row.get(i + 1).map(|n| n.width),
                    Some(0),
                    "{ctx}: left half alone at {i},{r}"
                ),
                w => panic!("{ctx}: width {w} at {i},{r}"),
            }
        }
    }
    s
}

type State = (
    String,
    String,
    (u16, u16, bool),
    Vec<u8>,
    Vec<Event>,
    vt::InputModes,
);

fn state(t: &mut Terminal) -> State {
    let (mut r, mut e) = (Vec::new(), Vec::new());
    t.take_replies(&mut r);
    t.take_events(&mut e);
    (
        t.screen_text(),
        t.scrollback_text(),
        t.cursor(),
        r,
        e,
        t.input_modes(),
    )
}

/// A queued title, directory or bell replaces the pending one, so events
/// drained more often keep earlier ones too. Only the last of each counts.
fn latest_only(mut events: Vec<Event>) -> Vec<Event> {
    let mut seen = Vec::new();
    events.reverse();
    events.retain(|e| {
        if !matches!(e, Event::Title(_) | Event::Cwd(_) | Event::Bell) {
            return true;
        }
        let kind = std::mem::discriminant(e);
        let last = !seen.contains(&kind);
        seen.push(kind);
        last
    });
    events.reverse();
    events
}

#[test]
fn chunking_never_changes_the_result() {
    let base = env("CHAOS_SEED", 0x9E37_79B9_7F4A_7C15);
    let mut rng = Rng(base.max(1));
    for round in 0..env("CHAOS_ROUNDS", 300) {
        let seed = rng.next();
        let ctx = format!("CHAOS_SEED={base} round {round} (stream {seed:#x})");
        let mut r = Rng(seed);
        let bytes = stream(&mut r, 400);
        let mut whole = new_term(&mut Rng(seed));
        let mut split = new_term(&mut Rng(seed));
        whole.feed(&bytes);
        let (mut replies, mut events) = (Vec::new(), Vec::new());
        let mut rest = &bytes[..];
        while !rest.is_empty() {
            let n = (1 + r.below(24)).min(rest.len());
            split.feed(&rest[..n]);
            rest = &rest[n..];
            // Drain as the reader thread does; order must not change.
            split.take_replies(&mut replies);
            split.take_events(&mut events);
        }
        let (a, mut b) = (state(&mut whole), state(&mut split));
        replies.append(&mut b.3);
        events.append(&mut b.4);
        b.3 = replies;
        b.4 = latest_only(events);
        assert_eq!(a, b, "{ctx}");
        // Colours, attributes, widths and the cursor shape too.
        assert_eq!(check(&mut whole, &ctx), check(&mut split, &ctx), "{ctx}");
    }
}

#[test]
fn host_calls_between_chunks_never_panic() {
    let base = env("CHAOS_SEED", 0xD1B5_4A32_D192_ED03);
    let mut rng = Rng(base.max(1));
    for round in 0..env("CHAOS_ROUNDS", 200) {
        let mut t = new_term(&mut rng);
        for step in 0..20 {
            let bytes = stream(&mut rng, 40);
            t.feed(&bytes);
            match rng.below(6) {
                0 => t.resize(1 + rng.below(50) as u16, 1 + rng.below(15) as u16),
                1 => t.scroll_viewport(rng.below(40) as isize - 20),
                2 => t.on_child_exit(),
                3 => t.set_cell_px(rng.below(30) as u16, rng.below(60) as u16),
                _ => {}
            }
            check(
                &mut t,
                &format!("CHAOS_SEED={base} round {round} step {step}"),
            );
            state(&mut t);
        }
    }
}
