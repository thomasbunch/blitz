//! The parser must produce the same actions however its input is chunked.
//! Every stream here is synthetic.

use vt::parser::{Handler, MAX_OSC, MAX_PARAMS, Params, Parser};

#[derive(Clone, Debug, PartialEq, Eq)]
enum Ev {
    Print(String),
    Exec(u8),
    Esc(Vec<u8>, u8),
    Csi(Vec<(u16, bool)>, Vec<u8>, u8),
    Osc(Vec<u8>, bool),
    Hook(Vec<(u16, bool)>, Vec<u8>, u8),
    Put(Vec<u8>),
    Unhook,
}

/// Records actions, merging adjacent prints and DCS puts, whose boundaries
/// depend on chunking.
#[derive(Default)]
struct Rec(Vec<Ev>);

fn params(p: &Params) -> Vec<(u16, bool)> {
    (0..p.len()).map(|i| (p.get(i), p.is_sub(i))).collect()
}

impl Handler for Rec {
    fn print(&mut self, s: &str) {
        assert!(!s.is_empty(), "empty print");
        assert!(!s.chars().any(char::is_control), "control in print: {s:?}");
        match self.0.last_mut() {
            Some(Ev::Print(t)) => t.push_str(s),
            _ => self.0.push(Ev::Print(s.into())),
        }
    }
    fn execute(&mut self, c0: u8) {
        self.0.push(Ev::Exec(c0));
    }
    fn esc(&mut self, inter: &[u8], fin: u8) {
        self.0.push(Ev::Esc(inter.into(), fin));
    }
    fn csi(&mut self, p: &Params, inter: &[u8], fin: u8) {
        self.0.push(Ev::Csi(params(p), inter.into(), fin));
    }
    fn osc(&mut self, data: &[u8], bel_terminated: bool) {
        self.0.push(Ev::Osc(data.into(), bel_terminated));
    }
    fn dcs_hook(&mut self, p: &Params, inter: &[u8], fin: u8) {
        self.0.push(Ev::Hook(params(p), inter.into(), fin));
    }
    fn dcs_put(&mut self, chunk: &[u8]) {
        assert!(!chunk.is_empty(), "empty put");
        match self.0.last_mut() {
            Some(Ev::Put(t)) => t.extend_from_slice(chunk),
            _ => self.0.push(Ev::Put(chunk.into())),
        }
    }
    fn dcs_unhook(&mut self) {
        self.0.push(Ev::Unhook);
    }
}

fn parse<'a>(chunks: impl IntoIterator<Item = &'a [u8]>) -> Vec<Ev> {
    let mut p = Parser::new();
    let mut r = Rec::default();
    for c in chunks {
        p.advance(&mut r, c);
    }
    r.0
}

fn one(s: &[u8]) -> Vec<Ev> {
    parse([s])
}

fn text(s: &str) -> Ev {
    Ev::Print(s.into())
}

fn csi(p: &[u16], inter: &[u8], fin: u8) -> Ev {
    Ev::Csi(p.iter().map(|&v| (v, false)).collect(), inter.into(), fin)
}

/// xorshift64*, so failures reproduce without a dependency.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// Pieces of real-world-shaped output, plus malformed and truncated input.
const FRAGMENTS: &[&[u8]] = &[
    b"plain ascii text ",
    b"\r\n",
    b"\t",
    b"\x08",
    b"\x07",
    b"\x7f",
    "h\u{e9}llo w\u{f6}rld ".as_bytes(),
    "\u{65e5}\u{672c}\u{8a9e}".as_bytes(),
    "\u{1f642}\u{1f44d}\u{1f3fd}".as_bytes(),
    "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}".as_bytes(),
    "\u{2590}\u{259b}\u{2588}\u{2588}\u{2588}\u{259c}\u{258c} \u{25c9} \u{2605} \u{23f5}"
        .as_bytes(),
    b"\xc2\xa0",
    b"\xc2\x85\xc2\x9b",
    b"\x80",
    b"\xc3",
    b"\xe2\x82",
    b"\xf0\x9f\x99",
    b"\xe0\x80\x80",
    b"\xc0\xaf",
    b"\xed\xa0\x80",
    b"\xf4\x90\x80\x80",
    b"\xff\xfe",
    b"\x1b[m",
    b"\x1b[0m",
    b"\x1b[1;31;48;5;200m",
    b"\x1b[38:2::10:20:30m",
    b"\x1b[38;2;1;2;3m",
    b"\x1b[4:3m",
    b"\x1b[58:5:9m",
    b"\x1b[H\x1b[2J\x1b[3J",
    b"\x1b[;H",
    b"\x1b[12;40H",
    b"\x1b[?1049h",
    b"\x1b[?1049l",
    b"\x1b[?2026h",
    b"\x1b[?2026l",
    b"\x1b[?2026$p",
    b"\x1b[>4;2m",
    b"\x1b[?u",
    b"\x1b[>5u",
    b"\x1b[<u",
    b"\x1b[=5;1u",
    b"\x1b[6 q",
    b"\x1b[!p",
    b"\x1b[c",
    b"\x1b[>c",
    b"\x1b[6n",
    b"\x1b[99999999999A",
    b"\x1b[1\n2A",
    b"\x1b[1\x7f;2H",
    b"\x1b[12\x18",
    b"\x1b[3\x1a",
    b"\x1b[1;2",
    b"\x1b[1?2h",
    b"\x1b[1$2p",
    b"\x1b[1 !\"#$p",
    b"\x1b[",
    b"\x1b",
    b"\x1b7\x1b8",
    b"\x1b(B\x1b)0",
    b"\x1b#8",
    b"\x1b=\x1b>",
    b"\x1bM\x1bD\x1bE",
    b"\x1b\\",
    b"\x1b\x1b",
    b"\x1b\x7fZ",
    b"\x1b\n",
    "\x1b]0;title \u{2713}\x07".as_bytes(),
    b"\x1b]2;st title\x1b\\",
    b"\x1b]8;id=1;https://example.com/\x1b\\link\x1b]8;;\x1b\\",
    b"\x1b]777;notify;blitz:needs-you;Approve?\x07",
    b"\x1b]133;A;blitz=1\x07",
    b"\x1b]133;D;0\x07",
    b"\x1b]7;file:///C:/work\x1b\\",
    b"\x1b]9;4;1;50\x07",
    b"\x1b]11;?\x07",
    b"\x1b]0;a\nb\x7fc\x07",
    b"\x1b]0;gone\x18",
    b"\x1b]0;x",
    b"\x1b]",
    b"\x1bP>|blitz 0.1.0\x1b\\",
    b"\x1bP$qm\x1b\\",
    b"\x1bP1;2|ab\ncd\x7fe\x1b\\",
    b"\x1bP+q544e\x1b\\",
    b"\x1bPq#0;2;0;0;0#0~~@@vv@@~~$-\x1b\\",
    b"\x1bPqabc\x18",
    b"\x1bP1?2qbody\x1b\\",
    b"\x1bP",
    b"\x1b_Gf=24,s=10;AAAA\x1b\\",
    b"\x1b^pm body\x07still pm\x1b\\",
    b"\x1bXsos\x1b\\",
    b"\x1b_unterminated",
];

/// Every fragment once, random arrangements of them, then raw noise and
/// noise drawn from the bytes that matter to the state machine.
fn corpus() -> Vec<Vec<u8>> {
    let mut out = vec![FRAGMENTS.concat()];
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for _ in 0..6 {
        let mut s = Vec::new();
        while s.len() < 2048 {
            s.extend_from_slice(FRAGMENTS[rng.below(FRAGMENTS.len())]);
        }
        out.push(s);
    }
    let noise: Vec<u8> = (0..2048).map(|_| rng.next() as u8).collect();
    out.push(noise);
    let alphabet = b"\x1b[];:?$ 0123mPX_^\\\x07\x18\x1a\x7f\xc2\x85\xe2\x82\xac";
    let biased: Vec<u8> = (0..2048)
        .map(|_| alphabet[rng.below(alphabet.len())])
        .collect();
    out.push(biased);
    // Something that ends every stream in a known state, so leftover
    // parser state from a bad split shows up too.
    for s in &mut out {
        s.extend_from_slice(b"\x18end");
    }
    out
}

#[test]
fn split_every_boundary() {
    for s in corpus() {
        let whole = one(&s);
        for k in 1..s.len() {
            assert_eq!(parse([&s[..k], &s[k..]]), whole, "split at {k}");
        }
    }
}

#[test]
fn split_every_byte() {
    for s in corpus() {
        let whole = one(&s);
        for n in [1, 2, 3, 5, 8, 13] {
            assert_eq!(parse(s.chunks(n)), whole, "chunks of {n}");
        }
    }
}

fn random_splits(s: &[u8], rng: &mut Rng) -> Vec<Ev> {
    let mut cuts: Vec<usize> = (0..1 + rng.below(16))
        .map(|_| rng.below(s.len() + 1))
        .collect();
    cuts.push(0);
    cuts.push(s.len());
    cuts.sort_unstable();
    parse(cuts.windows(2).map(|w| &s[w[0]..w[1]]))
}

#[test]
fn split_random() {
    let mut rng = Rng(0xB117_2000);
    for s in corpus() {
        let whole = one(&s);
        for _ in 0..500 {
            assert_eq!(random_splits(&s, &mut rng), whole);
        }
    }
}

/// 500 seeded random splits of every `*.vt` file in `VT_CORPUS_DIR`, for
/// recorded output that cannot live in the repository.
#[test]
#[ignore = "needs VT_CORPUS_DIR"]
fn split_recorded_streams() {
    let dir = std::env::var_os("VT_CORPUS_DIR").expect("VT_CORPUS_DIR");
    let mut n = 0;
    for entry in std::fs::read_dir(dir).expect("read VT_CORPUS_DIR") {
        let path = entry.expect("dir entry").path();
        if path.extension().is_none_or(|e| e != "vt") {
            continue;
        }
        let s = std::fs::read(&path).expect("read stream");
        let whole = one(&s);
        let mut rng = Rng(0x5EED ^ s.len() as u64);
        for _ in 0..500 {
            assert_eq!(random_splits(&s, &mut rng), whole, "{}", path.display());
        }
        n += 1;
    }
    assert!(n > 0, "no .vt files found");
}

#[test]
fn parser_csi_params_and_subparams() {
    assert_eq!(
        one(b"\x1b[38:2::10:20:30;1m"),
        [Ev::Csi(
            vec![
                (38, false),
                (2, true),
                (0, true),
                (10, true),
                (20, true),
                (30, true),
                (1, false)
            ],
            vec![],
            b'm'
        )]
    );
    assert_eq!(one(b"\x1b[H"), [csi(&[], b"", b'H')]);
    assert_eq!(one(b"\x1b[;H"), [csi(&[0, 0], b"", b'H')]);
    assert_eq!(one(b"\x1b[5;H"), [csi(&[5, 0], b"", b'H')]);
    assert_eq!(one(b"\x1b[?2026$p"), [csi(&[2026], b"?$", b'p')]);
    assert_eq!(one(b"\x1b[>4;2m"), [csi(&[4, 2], b">", b'm')]);
    assert_eq!(one(b"\x1b[6 q"), [csi(&[6], b" ", b'q')]);
    assert_eq!(one(b"\x1b[99999999A"), [csi(&[u16::MAX], b"", b'A')]);
}

#[test]
fn parser_param_and_intermediate_limits() {
    let seq = |n: usize| {
        let p: Vec<String> = (1..=n).map(|i| i.to_string()).collect();
        format!("\x1b[{}m", p.join(";")).into_bytes()
    };
    let max: Vec<u16> = (1..=MAX_PARAMS as u16).collect();
    assert_eq!(one(&seq(MAX_PARAMS)), [csi(&max, b"", b'm')]);
    assert_eq!(one(&seq(MAX_PARAMS + 1)), []);
    // Overflowing at a `;` rather than at the final byte, and the rest of
    // the stream unharmed.
    let mut s = seq(MAX_PARAMS + 9);
    s.extend_from_slice(b"x");
    assert_eq!(one(&s), [text("x")]);
    assert_eq!(one(b"\x1b[1 !\"#p"), [csi(&[1], b" !\"#", b'p')]);
    assert_eq!(one(b"\x1b[1 !\"#$p"), []);
    assert_eq!(one(b"\x1b !\"#$Fx"), [text("x")]);
}

#[test]
fn parser_malformed_csi_is_ignored() {
    assert_eq!(one(b"\x1b[1?2hx"), [text("x")]);
    assert_eq!(one(b"\x1b[1$2px"), [text("x")]);
}

#[test]
fn parser_controls_inside_sequences() {
    // C0 runs without aborting the sequence; DEL is dropped.
    assert_eq!(one(b"\x1b[1\n2A"), [Ev::Exec(b'\n'), csi(&[12], b"", b'A')]);
    assert_eq!(one(b"\x1b[1\x7f;2H"), [csi(&[1, 2], b"", b'H')]);
    assert_eq!(one(b"a\x7fb"), [text("ab")]);
    // CAN and SUB abort, ESC restarts.
    assert_eq!(one(b"\x1b[12\x18x"), [Ev::Exec(0x18), text("x")]);
    assert_eq!(one(b"\x1b[3\x1ay"), [Ev::Exec(0x1a), text("y")]);
    assert_eq!(one(b"\x1b[1;2\x1b[3m"), [csi(&[3], b"", b'm')]);
}

#[test]
fn parser_esc_sequences() {
    assert_eq!(one(b"\x1b(B"), [Ev::Esc(b"(".into(), b'B')]);
    assert_eq!(one(b"\x1b7"), [Ev::Esc(vec![], b'7')]);
    assert_eq!(one(b"\x1b#8"), [Ev::Esc(b"#".into(), b'8')]);
    assert_eq!(one(b"\x1b\\x"), [text("x")]);
}

#[test]
fn parser_osc_terminators() {
    assert_eq!(one(b"\x1b]0;a\x07"), [Ev::Osc(b"0;a".into(), true)]);
    assert_eq!(one(b"\x1b]0;b\x1b\\"), [Ev::Osc(b"0;b".into(), false)]);
    // ESC ends the string even when it starts something other than ST.
    assert_eq!(
        one(b"\x1b]0;c\x1b[m"),
        [Ev::Osc(b"0;c".into(), false), csi(&[], b"", b'm')]
    );
    // Other controls are dropped from the payload; UTF-8 is kept.
    assert_eq!(
        one("\x1b]0;a\nb\x7f\u{2713}\x07".as_bytes()),
        [Ev::Osc("0;ab\u{2713}".into(), true)]
    );
    assert_eq!(one(b"\x1b]0;gone\x18x"), [Ev::Exec(0x18), text("x")]);
}

#[test]
fn parser_osc_length_limit() {
    for len in [MAX_OSC, MAX_OSC + 1] {
        let mut s = b"\x1b]52;c;".to_vec();
        s.resize(2 + len, b'A');
        s.extend_from_slice(b"\x07x");
        let mut expect = Vec::new();
        if len == MAX_OSC {
            expect.push(Ev::Osc(s[2..2 + len].into(), true));
        }
        expect.push(text("x"));
        assert_eq!(one(&s), expect, "len {len}");
        assert_eq!(parse(s.chunks(4096)), expect, "len {len}, chunked");
    }
}

#[test]
fn parser_dcs_strings() {
    assert_eq!(
        one(b"\x1bP1$qm\x1b\\"),
        [
            Ev::Hook(vec![(1, false)], b"$".into(), b'q'),
            Ev::Put(b"m".into()),
            Ev::Unhook
        ]
    );
    assert_eq!(
        one(b"\x1bP>|x\ny\x7fz\x1b\\"),
        [
            Ev::Hook(vec![], b">".into(), b'|'),
            Ev::Put(b"x\nyz".into()),
            Ev::Unhook
        ]
    );
    assert_eq!(
        one(b"\x1bPqab\x18c"),
        [
            Ev::Hook(vec![], vec![], b'q'),
            Ev::Put(b"ab".into()),
            Ev::Unhook,
            Ev::Exec(0x18),
            text("c")
        ]
    );
    assert_eq!(one(b"\x1bP1?2qbody\x1b\\x"), [text("x")]);
    // Too many parameters or intermediates: the whole string is ignored,
    // up to its ST.
    let p: Vec<String> = (0..40).map(|i| i.to_string()).collect();
    let many = format!("\x1bP{}qdata\x1b\\x", p.join(";"));
    assert_eq!(one(many.as_bytes()), [text("x")]);
    assert_eq!(one(b"\x1bP !\"#$qdata\x1b\\y"), [text("y")]);
}

#[test]
fn parser_escape_then_non_ascii() {
    // ESC then UTF-8 waits for a final byte, as in vte: the character is
    // lost and the next printable byte ends the escape.
    assert_eq!(
        one("\x1b\u{e9}xy".as_bytes()),
        [Ev::Esc(vec![], b'x'), text("y")]
    );
    // Inside an OSC, 0x9C is data, not ST; BEL still ends it.
    assert_eq!(
        one(b"\x1b]0;a\x9cb\x07c"),
        [Ev::Osc(b"0;a\x9cb".into(), true), text("c")]
    );
}

#[test]
fn parser_sos_pm_apc_are_swallowed() {
    assert_eq!(one(b"\x1b_Gf=24;AAAA\x07more\x1b\\x"), [text("x")]);
    assert_eq!(one(b"\x1b^pm\x1b\\y"), [text("y")]);
    assert_eq!(one(b"\x1bXsos\x1b\\z"), [text("z")]);
}

#[test]
fn parser_utf8() {
    let s = "\u{65e5}\u{672c} \u{1f642}".as_bytes();
    assert_eq!(one(s), [text("\u{65e5}\u{672c} \u{1f642}")]);
    assert_eq!(parse(s.chunks(1)), [text("\u{65e5}\u{672c} \u{1f642}")]);
    assert_eq!(one(b"a\xffb"), [text("a\u{fffd}b")]);
    assert_eq!(one(b"\xe0\x80\x80"), [text("\u{fffd}\u{fffd}\u{fffd}")]);
    assert_eq!(one(b"\xf0\x9f\x99A"), [text("\u{fffd}A")]);
    // Cut short by a control or ESC: one replacement, then the control.
    assert_eq!(one(b"\xe2\x82\n"), [text("\u{fffd}"), Ev::Exec(b'\n')]);
    assert_eq!(
        one(b"\xe2\x82\x1b[m"),
        [text("\u{fffd}"), csi(&[], b"", b'm')]
    );
    // C1 controls encoded as UTF-8 are dropped; NBSP is not one of them.
    assert_eq!(one(b"a\xc2\x85b\xc2\x9bc\xc2\xa0"), [text("abc\u{a0}")]);
    // 0x9B and 0x9D are never CSI or OSC.
    assert_eq!(one(b"\x9b1m\x9d0;x"), [text("\u{fffd}1m\u{fffd}0;x")]);
}
