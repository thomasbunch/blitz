//! Seeded random streams: the terminal must never panic, must keep the
//! cursor on the screen, and must end in the same state however the
//! stream is split into chunks.

use vt::{Event, Options, Palette, Snapshot, Terminal};

const PAL: Palette = Palette {
    fg: 0xC0C0C0,
    bg: 0x101010,
    cursor: 0xFFFFFF,
    selection_bg: 0x333333,
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
    "H",
    "J",
    "K",
    "r",
    "h",
    "l",
    "c",
    "n",
    "p",
    "u",
    "q",
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
    "\x1b[?1047l",
    "\x1b[?1048h",
    "\x1b[?6h",
    "\x1b[?7l",
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

fn check(t: &mut Terminal) {
    let (x, y, _) = t.cursor();
    let mut s = Snapshot::default();
    t.snapshot(&mut s, &PAL);
    let (cols, rows) = (s.cols, s.rows);
    if cols > 0 {
        assert!(x < cols && y < rows, "cursor {x},{y} outside {cols}x{rows}");
        assert_eq!(s.cells.len(), cols as usize * rows as usize);
    }
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

#[test]
fn chunking_never_changes_the_result() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for round in 0..300 {
        let seed = rng.next();
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
        b.4 = events;
        assert_eq!(a, b, "round {round}, seed {seed:#x}");
        check(&mut whole);
    }
}

#[test]
fn host_calls_between_chunks_never_panic() {
    let mut rng = Rng(0xD1B5_4A32_D192_ED03);
    for _ in 0..200 {
        let mut t = new_term(&mut rng);
        for _ in 0..20 {
            let bytes = stream(&mut rng, 40);
            t.feed(&bytes);
            match rng.below(6) {
                0 => t.resize(1 + rng.below(50) as u16, 1 + rng.below(15) as u16),
                1 => t.scroll_viewport(rng.below(40) as isize - 20),
                2 => t.on_child_exit(),
                3 => t.set_cell_px(rng.below(30) as u16, rng.below(60) as u16),
                _ => {}
            }
            check(&mut t);
            state(&mut t);
        }
    }
}
