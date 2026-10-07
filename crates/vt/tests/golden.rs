//! Screen goldens: synthetic byte streams and the screen they must leave.

use vt::snapshot::attr;
use vt::{CursorShape, Options, Palette, RenderCell, Snapshot, Terminal};

const PAL: Palette = Palette {
    fg: 0xC0C0C0,
    bg: 0x101010,
    cursor: 0xFFFFFF,
    selection_bg: 0x333333,
    ansi: [
        0x000000, 0xAA0000, 0x00AA00, 0xAA5500, 0x0000AA, 0xAA00AA, 0x00AAAA, 0xAAAAAA, 0x555555,
        0xFF5555, 0x55FF55, 0xFFFF55, 0x5555FF, 0xFF55FF, 0x55FFFF, 0xFFFFFF,
    ],
};

fn feed(t: &mut Terminal, s: &str) {
    t.feed(s.as_bytes());
}

/// A terminal fed `s` in one piece, after checking that feeding it a byte
/// at a time leaves the same screen, colours and widths included.
fn run(cols: u16, rows: u16, s: &str) -> Terminal {
    let o = Options {
        cols,
        rows,
        ..Options::default()
    };
    let (mut t, mut bytewise) = (Terminal::new(o), Terminal::new(o));
    t.feed(s.as_bytes());
    for b in s.as_bytes() {
        bytewise.feed(std::slice::from_ref(b));
    }
    assert_eq!(t.screen_text(), bytewise.screen_text());
    assert_eq!(t.scrollback_text(), bytewise.scrollback_text());
    assert_eq!(t.cursor(), bytewise.cursor());
    assert_eq!(snap(&mut t), snap(&mut bytewise));
    t
}

fn snap(t: &mut Terminal) -> Snapshot {
    let mut s = Snapshot::default();
    t.snapshot(&mut s, &PAL);
    s
}

fn cell(s: &Snapshot, x: u16, y: u16) -> RenderCell {
    s.cells[y as usize * s.cols as usize + x as usize]
}

fn text(c: &RenderCell) -> &str {
    std::str::from_utf8(&c.text[..c.len as usize]).unwrap()
}

fn lines(range: std::ops::RangeInclusive<i32>) -> String {
    range
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn prompt_line() {
    let prompt = "\x1b[1;32muser@box\x1b[0m:\x1b[1;34m~/src\x1b[0m$ ";
    let mut t = run(40, 5, &format!("{prompt}ls\r\nCargo.toml  src\r\n{prompt}"));
    assert_eq!(
        t.screen_text(),
        "user@box:~/src$ ls\nCargo.toml  src\nuser@box:~/src$\n\n"
    );
    assert_eq!(t.cursor(), (16, 2, true));
    let s = snap(&mut t);
    let u = cell(&s, 0, 2);
    assert_eq!((text(&u), u.fg, u.attrs), ("u", PAL.ansi[2], attr::BOLD));
    let colon = cell(&s, 8, 2);
    assert_eq!((colon.fg, colon.bg, colon.attrs), (PAL.fg, PAL.bg, 0));
    assert_eq!(cell(&s, 9, 2).fg, PAL.ansi[4]);
    assert_eq!(s.cursor.map(|c| (c.0, c.1)), Some((16, 2)));
}

#[test]
fn sgr_colours() {
    let mut t = run(
        20,
        1,
        concat!(
            "\x1b[31ma\x1b[38;5;196mb\x1b[38;2;1;2;3mc\x1b[38:2::10:20:30md",
            "\x1b[38:2:7:8:9me\x1b[48:5:21mf\x1b[0;7mg\x1b[0;4:3mh\x1b[4:0mi",
            "\x1b[0;1;2m\x1b[22mj\x1b[94;103mk\x1b[58:2::1:2:3;39;49ml",
            "\x1b[31m\x1b[38;5mm\x1b[8mn\x1b[0;4mo\x1b[24mp",
        ),
    );
    let s = snap(&mut t);
    let fg: Vec<u32> = (0..16).map(|x| cell(&s, x, 0).fg).collect();
    assert_eq!(
        fg,
        [
            0xAA0000, 0xFF0000, 0x010203, 0x0A141E, 0x070809, 0x070809, PAL.bg, PAL.fg, PAL.fg,
            PAL.fg, 0x5555FF, PAL.fg, 0xAA0000, PAL.bg, PAL.fg, PAL.fg
        ]
    );
    assert_eq!(cell(&s, 5, 0).bg, 0x0000FF);
    assert_eq!(cell(&s, 6, 0).bg, PAL.fg);
    assert_eq!(cell(&s, 10, 0).bg, 0xFFFF55);
    assert_eq!(cell(&s, 13, 0).fg, cell(&s, 13, 0).bg, "invisible");
    let attrs: Vec<u16> = (0..16).map(|x| cell(&s, x, 0).attrs).collect();
    let (inv, ul) = (attr::INVERSE, attr::UNDERLINE);
    assert_eq!(attrs, [0, 0, 0, 0, 0, 0, inv, ul, 0, 0, 0, 0, 0, 0, ul, 0]);
}

#[test]
fn hidden_text_has_no_characters() {
    // Concealed, drawn in the background colour, and inverse with both
    // colours equal.
    let mut t = run(
        20,
        1,
        "a\x1b[8mb\x1b[28mc\x1b[38;2;16;16;16md\x1b[0;7;38;2;16;16;16me\x1b[0mf",
    );
    let s = snap(&mut t);
    let texts: Vec<&str> = (0..6).map(|x| text(&s.cells[x])).collect();
    assert_eq!(texts, ["a", "", "c", "", "", "f"]);
    assert_eq!(t.screen_text(), "abcdef", "the screen model keeps it");
}

#[test]
fn snapshot_marks_wrapped_rows() {
    let mut t = run(4, 3, "abcdef\r\ngh");
    assert_eq!(snap(&mut t).wrapped, [true, false, false]);
}

#[test]
fn wide_cjk_at_the_last_column() {
    let mut t = run(10, 3, "abcdefghi中x");
    assert_eq!(t.screen_text(), "abcdefghi\n中x\n");
    assert_eq!(t.cursor(), (3, 1, true));
    let s = snap(&mut t);
    assert_eq!((cell(&s, 9, 0).len, cell(&s, 9, 0).width), (0, 1));
    assert_eq!((text(&cell(&s, 0, 1)), cell(&s, 0, 1).width), ("中", 2));
    assert_eq!(cell(&s, 1, 1).width, 0);

    // Overwriting the right half blanks the left half.
    feed(&mut t, "\x1b[2;2HZ");
    assert_eq!(t.screen_text(), "abcdefghi\n Zx\n");

    // A wide character that just fits sets pending wrap like any other.
    let t = run(10, 3, "abcdefgh中y");
    assert_eq!(t.screen_text(), "abcdefgh中\ny\n");
}

#[test]
fn region_pump_pushes_into_scrollback() {
    // A transcript scrolled inside rows 1-7 above a fixed status line.
    let mut t = run(20, 10, "\x1b[10;1Hstatus\x1b[1;7r");
    for i in 1..=50 {
        feed(&mut t, &format!("line {i}\r\n"));
    }
    assert_eq!(t.screen_text(), format!("{}\n\n\n\nstatus", lines(45..=50)));
    assert_eq!(t.scrollback_text(), lines(1..=44));

    // A region that does not start at the top keeps nothing.
    let mut t = run(20, 10, "\x1b[2;7r");
    for i in 1..=50 {
        feed(&mut t, &format!("line {i}\r\n"));
    }
    assert_eq!(t.scrollback_text(), "");
}

#[test]
fn scrollback_is_capped() {
    let mut t = Terminal::new(Options {
        cols: 20,
        rows: 5,
        scrollback_lines: 100,
        ambiguous_wide: false,
    });
    for i in 1..=500 {
        feed(&mut t, &format!("line {i}\r\n"));
    }
    assert_eq!(t.scrollback_text(), lines(397..=496));
}

#[test]
fn alt_screen_round_trip() {
    let mut t = run(20, 5, "hello\r\nworld\x1b[31m");
    feed(&mut t, "\x1b[?1049h");
    assert!(t.input_modes().alt_screen);
    assert_eq!(t.screen_text(), "\n\n\n\n");
    feed(&mut t, "\x1b[HALT\x1b[0m");
    for _ in 0..10 {
        feed(&mut t, "x\n");
    }
    assert_eq!(t.scrollback_text(), "", "the alt screen has no scrollback");
    feed(&mut t, "\x1b[?1049l");
    assert!(!t.input_modes().alt_screen);
    assert_eq!(t.screen_text(), "hello\nworld\n\n\n");
    assert_eq!(t.cursor(), (5, 1, true));
    feed(&mut t, "!");
    assert_eq!(cell(&snap(&mut t), 5, 1).fg, PAL.ansi[1], "SGR restored");
    feed(&mut t, "\x1b[?1049h");
    assert_eq!(t.screen_text(), "\n\n\n\n", "entering clears");
}

#[test]
fn modes_47_1047_1048() {
    let mut t = run(10, 3, "main\x1b[?47hALT\x1b[?47l");
    assert_eq!(t.screen_text(), "main\n\n");
    assert_eq!(t.cursor(), (7, 0, true), "47 does not move the cursor");
    feed(&mut t, "\x1b[?47h");
    assert_eq!(t.screen_text(), "    ALT\n\n", "47 does not clear");
    feed(&mut t, "\x1b[?1047l\x1b[?1047h");
    assert_eq!(t.screen_text(), "\n\n", "leaving 1047 clears");
    feed(
        &mut t,
        "\x1b[?1047l\x1b[1;2H\x1b[?1048h\x1b[3;5H\x1b[?1048l",
    );
    assert_eq!(t.cursor(), (1, 0, true));
}

#[test]
fn erase_scrollback_with_csi_3_j() {
    let mut t = run(10, 3, "");
    for i in 0..20 {
        feed(&mut t, &format!("{i}\r\n"));
    }
    assert!(!t.scrollback_text().is_empty());
    let screen = t.screen_text();
    feed(&mut t, "\x1b[3J");
    assert_eq!(t.scrollback_text(), "");
    assert_eq!(t.screen_text(), screen);

    // How full-screen programs redraw after a resize.
    feed(&mut t, "\x1b[r\x1b[2J\x1b[3J\x1b[H");
    for i in 0..5 {
        feed(&mut t, &format!("{i}\r\n"));
    }
    assert_eq!(t.scrollback_text(), "0\n1\n2");
    assert_eq!(t.screen_text(), "3\n4\n");
}

#[test]
fn pending_wrap_and_autowrap() {
    let mut t = run(5, 3, "abcde");
    assert_eq!(t.cursor(), (4, 0, true));
    feed(&mut t, "\rX");
    assert_eq!(t.screen_text(), "Xbcde\n\n");
    assert_eq!(run(5, 3, "abcdefg").screen_text(), "abcde\nfg\n");
    assert_eq!(run(5, 3, "\x1b[?7labcdefg").screen_text(), "abcdg\n\n");
    assert_eq!(run(5, 3, "abcde\x08X").screen_text(), "abcXe\n\n");
    assert_eq!(run(5, 3, "abcde\nX").screen_text(), "abcde\n    X\n");
}

#[test]
fn cursor_movement() {
    let t = run(
        10,
        5,
        "\x1b[3;4Ha\x1b[2Ab\x1b[9Bc\x1b[20Cd\x1b[3De\x1b[1Gf\x1b[2dg\x1b[Eh\x1b[Fi",
    );
    assert_eq!(t.screen_text(), "    b\nig\nh  a\n\nf    ce  d");
    let t = run(20, 1, "a\tb\tc");
    assert_eq!(t.screen_text(), "a       b       c");
    let t = run(20, 1, "a\tb\x1b[Zc");
    assert_eq!(t.screen_text(), "a       c");
}

#[test]
fn erase_and_edit_in_line() {
    let mut t = run(10, 3, "0123456789\r\nabcdefghij\r\nABCDEFGHIJ");
    feed(&mut t, "\x1b[1;3H\x1b[K\x1b[2;3H\x1b[1K\x1b[3;5H\x1b[2X");
    assert_eq!(t.screen_text(), "01\n   defghij\nABCD  GHIJ");
    feed(&mut t, "\x1b[2@");
    assert_eq!(t.screen_text(), "01\n   defghij\nABCD    GH");
    feed(&mut t, "\x1b[3P");
    assert_eq!(t.screen_text(), "01\n   defghij\nABCD GH");
    feed(&mut t, "\x1b[2;5H\x1b[J\x1b[1;2H\x1b[1J");
    assert_eq!(t.screen_text(), "\n   d\n");
}

#[test]
fn erase_uses_the_current_background() {
    let mut t = run(4, 1, "\x1b[44m\x1b[2K\x1b[0m");
    let s = snap(&mut t);
    assert_eq!(cell(&s, 3, 0).bg, PAL.ansi[4]);
}

#[test]
fn lines_scrolling_and_repeat() {
    let mut t = run(5, 5, "a\r\nb\r\nc\r\nd\r\ne\x1b[2;4r");
    feed(&mut t, "\x1b[3;1H\x1b[L");
    assert_eq!(t.screen_text(), "a\nb\n\nc\ne");
    feed(&mut t, "\x1b[M");
    assert_eq!(t.screen_text(), "a\nb\nc\n\ne");
    feed(&mut t, "\x1b[S");
    assert_eq!(t.screen_text(), "a\nc\n\n\ne");
    feed(&mut t, "\x1b[T");
    assert_eq!(t.screen_text(), "a\n\nc\n\ne");
    feed(&mut t, "\x1b[2;1H\x1bM");
    assert_eq!(t.screen_text(), "a\n\n\nc\ne");
    feed(&mut t, "\x1b[r\x1b[4;1HX\x1b[3b");
    assert_eq!(t.screen_text(), "a\n\n\nXXXX\ne");
    assert_eq!(t.scrollback_text(), "");
    // A repeat count stops at the screen width.
    feed(&mut t, "\x1b[HY\x1b[99b");
    assert_eq!(t.screen_text(), "YYYYY\nY\n\nXXXX\ne");
}

#[test]
fn save_restore_and_resets() {
    let mut t = run(10, 3, "\x1b[2;3H\x1b[31m\x1b7\x1b[0m\x1b[HX\x1b8Y");
    assert_eq!(t.cursor(), (3, 1, true));
    assert_eq!(cell(&snap(&mut t), 2, 1).fg, PAL.ansi[1]);

    feed(&mut t, "\x1b[2;3r\x1b[!p\x1b[3;1HZ\n");
    assert_eq!(cell(&snap(&mut t), 0, 1).fg, PAL.fg, "DECSTR resets SGR");
    assert_eq!(t.scrollback_text(), "X", "DECSTR resets the margins");

    feed(&mut t, "\x1b[?25l\x1bc");
    assert_eq!(t.screen_text(), "\n\n");
    assert_eq!(t.scrollback_text(), "");
    assert_eq!(t.cursor(), (0, 0, true));
}

/// DECSTR on the alternate screen also drops the cursor saved on entry,
/// as conhost does, so leaving restores the same cursor on both sides.
#[test]
fn decstr_forgets_both_saved_cursors() {
    let t = run(10, 3, "\x1b[2;3H\x1b[?1049h\x1b[!p\x1b[3;5H\x1b[?1049l");
    assert_eq!(t.cursor(), (0, 0, true));
}

#[test]
fn combining_marks_and_line_drawing() {
    let mut t = run(10, 2, "e\u{301}x\x1b(0lqk\x1b(B q");
    assert_eq!(t.screen_text(), "e\u{301}x┌─┐ q\n");
    assert_eq!(t.cursor(), (7, 0, true));
    assert_eq!(text(&cell(&snap(&mut t), 0, 0)), "e\u{301}");

    // Marks after an ASCII stretch of mixed text still join its last
    // character.
    let mut t = run(10, 1, "ab\u{301}c \u{2500}x\u{301}");
    assert_eq!(t.screen_text(), "ab\u{301}c \u{2500}x\u{301}");
    assert_eq!(t.cursor(), (6, 0, true));
    let s = snap(&mut t);
    assert_eq!(
        (text(&cell(&s, 1, 0)), text(&cell(&s, 5, 0))),
        ("b\u{301}", "x\u{301}")
    );

    let mut t = run(10, 2, "\u{1F468}\u{200D}\u{1F469}!");
    assert_eq!(t.screen_text(), "\u{1F468}\u{200D}\u{1F469}!\n");
    assert_eq!(t.cursor(), (3, 0, true));
    assert_eq!(cell(&snap(&mut t), 0, 0).width, 2);
}

#[test]
fn whole_clusters_reach_the_snapshot() {
    // A family, a kiss and subdivision flags: the longest clusters in
    // common use, up to 32 bytes.
    let flag = |tags: &str| {
        let tags: String = (tags.chars())
            .map(|c| char::from_u32(0xE0000 + c as u32).unwrap())
            .collect();
        format!("\u{1F3F4}{tags}\u{E007F}")
    };
    let (scotland, full) = (flag("gbsct"), flag("abcdef"));
    assert_eq!(full.len(), 32);
    for cluster in [
        "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}\u{200D}\u{1F466}",
        "\u{1F469}\u{200D}\u{2764}\u{FE0F}\u{200D}\u{1F48B}\u{200D}\u{1F468}",
        &scotland,
        &full,
    ] {
        let mut t = run(10, 1, &format!("{cluster}x"));
        assert_eq!(t.screen_text(), format!("{cluster}x"));
        let s = snap(&mut t);
        assert_eq!(text(&cell(&s, 0, 0)), cluster);
        assert_eq!((cell(&s, 0, 0).width, text(&cell(&s, 2, 0))), (2, "x"));
    }
    // Past 32 bytes the code points that do not fit are dropped from the
    // screen and the snapshot alike, whole.
    let over = flag("abcdefg");
    let mut t = run(10, 1, &format!("{over}x"));
    let kept = &over[..32];
    assert_eq!(t.screen_text(), format!("{kept}x"));
    assert_eq!(text(&cell(&snap(&mut t), 0, 0)), kept);
    // Once a code point does not fit, none after it gets in, however
    // small: the cell keeps the start of the cluster, never a gap.
    let start = format!("a{}\u{20D0}", "\u{301}".repeat(11));
    let mut t = run(10, 1, &format!("{start}\u{1D167}\u{20D0}x"));
    assert_eq!(text(&cell(&snap(&mut t), 0, 0)), start);
    assert_eq!(t.screen_text(), format!("{start}x"));
}

#[test]
fn endless_marks_are_capped_per_cell() {
    // Two-byte marks fill the tail exactly; three-byte ones stop short of
    // it. The cluster still takes one cell and the cursor moves on.
    let mut t = run(4, 1, &format!("a{}b", "\u{301}".repeat(500)));
    assert_eq!(t.screen_text(), format!("a{}b", "\u{301}".repeat(14)));
    assert_eq!(t.cursor(), (2, 0, true));
    let s = snap(&mut t);
    assert_eq!(text(&cell(&s, 0, 0)), format!("a{}", "\u{301}".repeat(14)));
    let mut t = run(4, 1, &format!("\u{1F44D}{}", "\u{20D0}".repeat(50)));
    let want = format!("\u{1F44D}{}", "\u{20D0}".repeat(9));
    assert_eq!(text(&cell(&snap(&mut t), 0, 0)), want);
    assert_eq!(t.screen_text(), want);
}

#[test]
fn clusters_that_change_width() {
    // VS16 turns a text-style heart into a two-column emoji.
    let mut t = run(10, 2, "\u{2764}x\r\n\u{2764}\u{FE0F}x");
    assert_eq!(t.screen_text(), "\u{2764}x\n\u{2764}\u{FE0F}x");
    assert_eq!(t.cursor(), (3, 1, true));
    let s = snap(&mut t);
    assert_eq!((cell(&s, 0, 1).width, cell(&s, 1, 1).width), (2, 0));

    // A flag is one cluster of two regional indicators; a third starts
    // a new one.
    let t = run(10, 1, "\u{1F1FA}\u{1F1F8}\u{1F1EB}");
    assert_eq!(t.cursor(), (4, 0, true));

    // A mark with nothing before it is dropped.
    let t = run(10, 1, "a\r\u{301}b");
    assert_eq!(t.screen_text(), "b");
}

#[test]
fn edits_never_leave_half_a_wide_character() {
    // A narrow character over the left half, and a wide one offset by a
    // column over another.
    let mut t = run(6, 1, "中文\x1b[1Gx");
    assert_eq!(t.screen_text(), "x 文");
    feed(&mut t, "\x1b[2G字");
    assert_eq!(t.screen_text(), "x字");
    let s = snap(&mut t);
    assert_eq!([0, 1, 2, 3].map(|x| cell(&s, x, 0).width), [1, 2, 0, 1]);
    // DCH, ICH and ECH starting on the right half.
    assert_eq!(run(6, 1, "a中bc\x1b[3G\x1b[P").screen_text(), "a bc");
    assert_eq!(run(6, 1, "a中bc\x1b[3G\x1b[@").screen_text(), "a   bc");
    assert_eq!(run(6, 1, "a中bc\x1b[3G\x1b[X").screen_text(), "a  bc");
    // Deleting before a wide character that wrapped early leaves a blank
    // where the spacer was, not a hole in the row.
    let t = run(5, 2, "abcd中\x1b[1;1H\x1b[P\x1b[1;5Hx");
    assert_eq!(t.screen_text(), "bcd x\n中");
}

#[test]
fn wide_characters_that_cannot_fit() {
    // With autowrap off, a wide character at the last column is dropped.
    assert_eq!(run(3, 2, "\x1b[?7lab中").screen_text(), "ab\n");
    // One column holds no wide character, and dropping it neither wraps
    // nor scrolls.
    let t = run(1, 1, "a中");
    assert_eq!(
        (t.screen_text(), t.scrollback_text()),
        ("a".into(), "".into())
    );
    assert_eq!(t.cursor(), (0, 0, true));
    // A mark or REP after a dropped wide character goes with it, never
    // onto, or repeating, the cell before.
    assert_eq!(run(3, 2, "\x1b[?7lab中\x1b[b").screen_text(), "ab\n");
    assert_eq!(run(3, 2, "\x1b[?7lab中\u{301}").screen_text(), "ab\n");
    assert_eq!(
        run(3, 2, "\x1b[?7lab\u{1F44D}\u{1F3FD}").screen_text(),
        "ab\n"
    );
    assert_eq!(run(1, 1, "a中\u{301}").screen_text(), "a");
    // VS16 on the last column keeps the emoji narrow: the cluster cannot
    // grow into the next row.
    let mut t = run(3, 2, "ab\u{2764}\u{FE0F}x");
    assert_eq!(t.screen_text(), "ab\u{2764}\u{FE0F}\nx");
    let s = snap(&mut t);
    assert_eq!(
        (text(&cell(&s, 2, 0)), cell(&s, 2, 0).width),
        ("\u{2764}\u{FE0F}", 1)
    );
}

#[test]
fn margins_and_moves_from_outside_them() {
    // CUD below the region goes to the last row; CUU above it to the
    // first; CUU from below it stops at the top margin.
    assert_eq!(
        run(
            5,
            6,
            "\x1b[2;4r\x1b[5;1H\x1b[9BY\x1b[1;2H\x1b[9AX\x1b[6;3H\x1b[9AZ"
        )
        .screen_text(),
        " X\n  Z\n\n\n\nY"
    );
    // IL and DL do nothing outside the region.
    assert_eq!(
        run(
            3,
            4,
            "a\r\nb\r\nc\r\nd\x1b[2;3r\x1b[1;1H\x1b[L\x1b[4;1H\x1b[M"
        )
        .screen_text(),
        "a\nb\nc\nd"
    );
    // A one-row, inverted or off-screen region is ignored, and the cursor
    // stays put.
    assert_eq!(
        run(3, 3, "ab\x1b[3;3rc\x1b[3;2rd\x1b[9;9re").screen_text(),
        "abc\nde\n"
    );
    // SU and SD by more than the region clear it, and keep nothing.
    let t = run(3, 4, "a\r\nb\r\nc\r\nd\x1b[2;3r\x1b[99S");
    assert_eq!(
        (t.screen_text(), t.scrollback_text()),
        ("a\n\n\nd".into(), "".into())
    );
    assert_eq!(
        run(3, 4, "a\r\nb\r\nc\r\nd\x1b[2;3r\x1b[99T").screen_text(),
        "a\n\n\nd"
    );
}

#[test]
fn origin_mode() {
    // Setting and resetting DECOM homes the cursor; rows count from the
    // top margin and stop at the bottom one.
    let mut t = run(5, 5, "\x1b[2;4r\x1b[3;3H\x1b[?6h");
    assert_eq!(t.cursor(), (0, 1, true));
    feed(&mut t, "\x1b[9;2H");
    assert_eq!(t.cursor(), (1, 3, true));
    feed(&mut t, "\x1b[?6l");
    assert_eq!(t.cursor(), (0, 0, true));
    feed(&mut t, "\x1b[9;2H");
    assert_eq!(t.cursor(), (1, 4, true));
}

#[test]
fn charsets() {
    // SO and SI pick G1 and G0; DEC graphics covers `_` and `` ` `` to `~`.
    assert_eq!(run(10, 1, "\x1b)0q\x0eq\x0fq").screen_text(), "q\u{2500}q");
    assert_eq!(
        run(10, 1, "\x1b(0_`a~x\x1b(B_").screen_text(),
        " \u{25c6}\u{2592}\u{b7}\u{2502}_"
    );
    // Text outside the range passes through.
    assert_eq!(run(10, 1, "\x1b(0A\u{e9}\x1b(B").screen_text(), "A\u{e9}");
}

#[test]
fn selective_erase_acts_as_erase() {
    // DECSED and DECSEL: nothing is protected, so everything goes.
    let t = run(5, 2, "abcde\r\nfghij\x1b[1;3H\x1b[?K\x1b[2;3H\x1b[?1K");
    assert_eq!(t.screen_text(), "ab\n   ij");
    let t = run(5, 2, "abcde\r\nfghij\x1b[1;3H\x1b[?0J");
    assert_eq!(t.screen_text(), "ab\n");
}

#[test]
fn index_next_line_and_aliases() {
    assert_eq!(run(5, 3, "ab\x1bDc\x1bEd").screen_text(), "ab\n  c\nd");
    // HVP, HPR, VPR and HPA are CUP, CUF, CUD and CHA by other names.
    assert_eq!(
        run(10, 3, "\x1b[2;3fA\x1b[2aB\x1b[eC\x1b[1`D").screen_text(),
        "\n  A  B\nD     C"
    );
}

#[test]
fn tab_stops() {
    // CHT, and TBC 0 clearing the stop under the cursor.
    assert_eq!(
        run(20, 1, "\x1b[2Ix\x1b[9G\x1b[0gy\r\x1b[2Iz").screen_text(),
        "        y       x  z"
    );
    // HTS sets one; TBC 3 clears them all, and then a tab goes to the
    // last column.
    assert_eq!(
        run(20, 1, "\x1b[3G\x1bH\r\tA\x1b[3g\r\tB").screen_text(),
        format!("  A{}B", " ".repeat(16))
    );
    // Other TBC modes clear nothing.
    assert_eq!(run(20, 1, "\x1b[9G\x1b[2g\r\tx").screen_text(), "        x");
    // A width change puts the default stops back.
    let mut t = run(20, 1, "\x1b[3g");
    t.resize(24, 1);
    feed(&mut t, "\r\tx");
    assert_eq!(t.screen_text(), "        x");
}

#[test]
fn line_feed_new_line_mode() {
    assert_eq!(
        run(5, 3, "\x1b[20hab\ncd\x1b[20l\nef").screen_text(),
        "ab\ncd\n  ef"
    );
    // VT and FF act as LF, LNM included.
    assert_eq!(run(5, 3, "\x1b[20ha\x0bb\x0cc").screen_text(), "a\nb\nc");
}

#[test]
fn cursor_shapes() {
    for (q, shape) in [
        ("", CursorShape::Block),
        ("\x1b[0 q", CursorShape::Block),
        ("\x1b[1 q", CursorShape::Block),
        ("\x1b[2 q", CursorShape::Block),
        ("\x1b[3 q", CursorShape::Underline),
        ("\x1b[4 q", CursorShape::Underline),
        ("\x1b[5 q", CursorShape::Bar),
        ("\x1b[6 q", CursorShape::Bar),
        ("\x1b[7 q", CursorShape::Block),
        ("\x1b[6 q\x1bc", CursorShape::Block),
    ] {
        assert_eq!(snap(&mut run(4, 1, q)).cursor, Some((0, 0, shape)), "{q:?}");
    }
}

#[test]
fn rare_renditions_and_malformed_colours() {
    let mut t = run(
        12,
        1,
        concat!(
            "\x1b[3ma\x1b[23mb\x1b[7;27mc\x1b[21md\x1b[24me",
            "\x1b[38;5;232mf\x1b[38;5;255mg",
            "\x1b[31m\x1b[38;5mh\x1b[31m\x1b[38:2:1mi\x1b[51mj",
        ),
    );
    let s = snap(&mut t);
    assert_eq!(
        [0, 1, 2, 3, 4].map(|x| cell(&s, x, 0).attrs),
        [attr::ITALIC, 0, 0, attr::UNDERLINE, 0]
    );
    assert_eq!((cell(&s, 5, 0).fg, cell(&s, 6, 0).fg), (0x080808, 0xEEEEEE));
    // A malformed colour and an unknown code leave the colour alone.
    assert_eq!([7, 8, 9].map(|x| cell(&s, x, 0).fg), [PAL.ansi[1]; 3]);

    // Codes the snapshot does not show leave the rest alone too.
    let mut t = run(
        4,
        1,
        "\x1b[1;5;6;9;53;58;5;1;21;24mk\x1b[25;29;55;59ml\x1b[2;22mm",
    );
    let s = snap(&mut t);
    let shown = attr::BOLD | attr::ITALIC | attr::UNDERLINE | attr::INVERSE | attr::DIM;
    assert_eq!(cell(&s, 0, 0).attrs & shown, attr::BOLD);
    assert_eq!(
        [1, 2].map(|x| cell(&s, x, 0).attrs),
        [attr::BOLD, 0],
        "29 and 55 ended strike and overline"
    );
    // The cube's corners.
    let mut t = run(4, 1, "\x1b[38;5;16ma\x1b[38;5;231mb\x1b[38;5;196mc");
    let s = snap(&mut t);
    assert_eq!(
        [0, 1, 2].map(|x| cell(&s, x, 0).fg),
        [0x000000, 0xFFFFFF, 0xFF0000]
    );
}

/// Compaction must renumber the styles saved with DECSC and on entering
/// the alternate screen, colours and erase background both.
#[test]
fn style_compaction_keeps_saved_cursor_styles() {
    let mut t = run(10, 2, "\x1b[32m\x1b[?1049h\x1b[33;44m\x1b7\x1b[0m");
    for i in 0..70_000u32 {
        let (r, g, b) = (i >> 16, (i >> 8) & 255, i & 255);
        feed(&mut t, &format!("\x1b[1;1H\x1b[38;2;{r};{g};{b}mx"));
    }
    feed(&mut t, "\x1b8a\x1b[K");
    let s = snap(&mut t);
    assert_eq!(cell(&s, 0, 0).fg, PAL.ansi[3]);
    assert_eq!(
        cell(&s, 5, 0).bg,
        PAL.ansi[4],
        "erased with the saved blank"
    );
    feed(&mut t, "\x1b[?1049lb");
    assert_eq!(cell(&snap(&mut t), 0, 0).fg, PAL.ansi[2]);
}

/// Hyperlinks fill their own table, which compacts the same way.
#[test]
fn link_table_compacts_when_full() {
    let mut t = run(10, 1, "\x1b[31m");
    for i in 0..70_000u32 {
        feed(&mut t, &format!("\x1b]8;;http://x/{i}\x1b\\\x1b[1;1Hx"));
    }
    feed(&mut t, "\x1b]8;;\x1b\\y\x1b]8\x07\x1b]8;id=1\x07z");
    assert_eq!(t.screen_text(), "xyz");
    let s = snap(&mut t);
    assert_eq!([0, 1, 2].map(|x| cell(&s, x, 0).fg), [PAL.ansi[1]; 3]);
}

#[test]
fn degenerate_sizes() {
    // Zero is one; nothing reaches past the single cell.
    let mut t = Terminal::new(Options {
        cols: 0,
        rows: 0,
        ..Options::default()
    });
    feed(
        &mut t,
        "a中\x1b[5;5H\x1b[6n\x1b[9@\x1b[9P\x1b[9L\x1b[9M\x1b[9X\t\x1b[9Z",
    );
    let mut r = Vec::new();
    t.take_replies(&mut r);
    assert_eq!(r, b"\x1b[1;1R");
    t.resize(0, 0);
    t.resize(1, 3);
    assert_eq!(t.cursor().1, 0);
    feed(&mut t, "bcd");
    assert_eq!(t.screen_text(), "b\nc\nd");

    // One column does not rewrap: rows are cut, and the cursor stays on.
    let mut t = run(4, 2, "abcdef");
    t.resize(1, 2);
    assert_eq!(t.screen_text(), "a\ne");
    assert_eq!(t.cursor(), (0, 1, true));
    t.resize(4, 2);
    assert!(t.cursor().0 < 4 && t.cursor().1 < 2);

    // No scrollback: rows that leave the screen are gone, and the view
    // cannot scroll.
    let mut t = Terminal::new(Options {
        cols: 4,
        rows: 2,
        scrollback_lines: 0,
        ambiguous_wide: false,
    });
    feed(&mut t, "a\r\nb\r\nc");
    t.scroll_viewport(5);
    assert_eq!(
        (t.screen_text(), t.scrollback_text()),
        ("b\nc".into(), "".into())
    );
    assert_eq!(snap(&mut t).cursor, Some((1, 1, CursorShape::Block)));
}

#[test]
fn resize_rewraps_clusters_whole() {
    let mut t = run(4, 3, "ae\u{301}bce\u{301}d");
    assert_eq!(t.screen_text(), "ae\u{301}bc\ne\u{301}d\n");
    t.resize(3, 3);
    assert_eq!(t.screen_text(), "ae\u{301}b\nce\u{301}d\n");
    t.resize(6, 3);
    assert_eq!(t.screen_text(), "ae\u{301}bce\u{301}d\n\n");
    let s = snap(&mut t);
    assert_eq!(
        (text(&cell(&s, 1, 0)), text(&cell(&s, 4, 0))),
        ("e\u{301}", "e\u{301}")
    );

    // A mark on a wide character that wrapped early, past its spacer.
    let mut t = run(5, 2, "abcd中\u{301}x");
    t.resize(7, 2);
    assert_eq!(t.screen_text(), "abcd中\u{301}x\n");
    t.resize(6, 2);
    assert_eq!(t.screen_text(), "abcd中\u{301}\nx");
    assert_eq!(text(&cell(&snap(&mut t), 4, 0)), "中\u{301}");
}

#[test]
fn insert_mode() {
    let mut t = run(6, 1, "abc\x1b[4h\x1b[1GXY");
    assert_eq!(t.screen_text(), "XYabc");
    feed(&mut t, "\x1b[4lZ");
    assert_eq!(t.screen_text(), "XYZbc");
}

#[test]
fn viewport_follows_its_text() {
    let mut t = run(10, 3, "");
    for i in 0..10 {
        feed(&mut t, &format!("{i}\r\n"));
    }
    t.scroll_viewport(2);
    let s = snap(&mut t);
    assert_eq!(text(&cell(&s, 0, 0)), "6");
    assert_eq!(s.cursor, None);
    feed(&mut t, "x\r\n");
    assert_eq!(text(&cell(&snap(&mut t), 0, 0)), "6");
    // A trip to the alternate screen, as at each blitz prompt, comes back
    // to the same text.
    feed(&mut t, "\x1b[?1049h\x1b[?1049l");
    assert_eq!(text(&cell(&snap(&mut t), 0, 0)), "6");
    // Unless the scrollback went meanwhile.
    feed(&mut t, "\x1b[?1049h\x1b[3J\x1b[?1049l");
    assert_eq!(text(&cell(&snap(&mut t), 0, 0)), "9");
    t.scroll_viewport(2);
    t.scroll_viewport(-100);
    assert_eq!(text(&cell(&snap(&mut t), 0, 0)), "9");
}

#[test]
fn viewport_holds_its_text_once_scrollback_is_full() {
    let mut t = Terminal::new(Options {
        cols: 10,
        rows: 3,
        scrollback_lines: 5,
        ambiguous_wide: false,
    });
    for i in 10..30 {
        feed(&mut t, &format!("{i}\r\n"));
    }
    let top = |t: &mut Terminal| {
        let s = snap(t);
        format!("{}{}", text(&cell(&s, 0, 0)), text(&cell(&s, 1, 0)))
    };
    t.scroll_viewport(2);
    assert_eq!(top(&mut t), "26");
    feed(&mut t, "x\r\n");
    assert_eq!(top(&mut t), "26", "each new row evicts the oldest");
    // At the oldest row, the row it showed is gone; the view stays on top.
    t.scroll_viewport(100);
    assert_eq!(top(&mut t), "24");
    feed(&mut t, "y\r\n");
    assert_eq!(top(&mut t), "25");
}

#[test]
fn snapshot_reports_changes() {
    let mut t = run(4, 2, "hi");
    let mut s = Snapshot::default();
    assert!(t.snapshot(&mut s, &PAL));
    assert!(!t.snapshot(&mut s, &PAL));
    feed(&mut t, "!");
    assert!(t.snapshot(&mut s, &PAL));
}

#[test]
fn resize_keeps_the_cursor_row() {
    let mut t = run(10, 5, "a\r\nb\r\nc\r\nd\r\ne");
    t.resize(8, 3);
    assert_eq!(t.screen_text(), "c\nd\ne");
    assert_eq!(t.scrollback_text(), "a\nb");
    assert_eq!(t.cursor(), (1, 2, true));
    t.resize(12, 4);
    assert_eq!(t.screen_text(), "c\nd\ne\n");
    feed(&mut t, "\x1b[4;12HZ");
    assert_eq!(t.screen_text(), "c\nd\ne\n           Z");
}

#[test]
fn joiner_after_a_resize_starts_fresh() {
    // The last cluster's cell moved when its row rewrapped.
    let mut t = run(10, 4, "\x1b[4;9Hp");
    t.resize(5, 2);
    feed(&mut t, "\u{200d}\u{301}x");
    assert_eq!(t.screen_text(), "\n   px");
}

#[test]
fn resize_rewraps_the_main_screen() {
    let mut t = run(5, 3, "abcdefghij\r\n$ ");
    t.resize(10, 3);
    assert_eq!(t.screen_text(), "abcdefghij\n$\n");
    assert_eq!(t.cursor(), (2, 1, true));
    t.resize(4, 3);
    assert_eq!(t.scrollback_text(), "abcd");
    assert_eq!(t.screen_text(), "efgh\nij\n$");
    assert_eq!(t.cursor(), (2, 2, true));
    feed(&mut t, "x");
    assert_eq!(t.screen_text(), "efgh\nij\n$ x");
}

#[test]
fn resize_rewraps_past_a_pending_wrap() {
    let mut t = run(5, 2, "abcde");
    t.resize(8, 2);
    assert_eq!(t.cursor(), (5, 0, true));
    feed(&mut t, "f");
    assert_eq!(t.screen_text(), "abcdef\n");
}

#[test]
fn resize_rewraps_wide_characters_whole() {
    let mut t = run(5, 2, "abcd中");
    assert_eq!(t.screen_text(), "abcd\n中");
    t.resize(6, 2);
    assert_eq!(t.screen_text(), "abcd中\n");
    t.resize(5, 2);
    assert_eq!(t.screen_text(), "abcd\n中");
    assert_eq!(t.cursor(), (2, 1, true));
}

/// 47 and 1047 save no cursor for the main screen, and DECSTR forgets the
/// one 1049 saved, so the main screen's last row of text stands in for it.
#[test]
fn resize_under_a_bare_alternate_screen_keeps_the_main_bottom() {
    let main = "aaaaaaaaaa\r\nbbbbbbbbbb\r\ncccccccccc";
    for (enter, leave) in [
        ("\x1b[?47h", "\x1b[?47l"),
        ("\x1b[?1047h", "\x1b[?1047l"),
        ("\x1b[?1049h\x1b[!p", "\x1b[?1049l"),
    ] {
        let mut t = run(10, 3, &format!("{main}{enter}"));
        t.resize(5, 3);
        feed(&mut t, leave);
        assert_eq!(t.screen_text(), "bbbbb\nccccc\nccccc", "{enter:?}");
        assert_eq!(t.scrollback_text(), "aaaaa\naaaaa\nbbbbb", "{enter:?}");

        // Fewer rows only: the top goes to scrollback, not the bottom.
        let mut t = run(10, 3, &format!("{main}{enter}"));
        t.resize(10, 2);
        feed(&mut t, leave);
        assert_eq!(t.screen_text(), "bbbbbbbbbb\ncccccccccc", "{enter:?}");
        assert_eq!(t.scrollback_text(), "aaaaaaaaaa", "{enter:?}");
    }
    // A main screen with blank rows under its text loses those first.
    let mut t = run(10, 4, "top\r\ntext\x1b[1;1H\x1b[?47h\x1b[4;1H");
    t.resize(10, 2);
    feed(&mut t, "\x1b[?47l");
    assert_eq!(t.screen_text(), "top\ntext");
}

/// Squeezed to one column and back under the alternate screen, the main
/// screen keeps its text: its saved cursor, past the end of a one-column
/// row, must not lose its place in the rewrap.
#[test]
fn hidden_main_screen_survives_a_squeeze_to_one_column() {
    let main: String = (0..20).map(|i| format!("line {i}\r\n")).collect();
    let mut t = run(80, 3, &format!("{main}\x1b[50Gx\x1b[?1049h"));
    t.resize(1, 3);
    t.resize(80, 3);
    feed(&mut t, "\x1b[?1049l");
    assert_eq!(t.scrollback_text(), lines(0..=17));
    // One column is too narrow to rewrap into, so the rows on screen were
    // cut to it; scrollback was not.
    assert_eq!(t.screen_text(), "l\nl\n");
}

/// A change of rows alone keeps the spacer a wide character leaves when it
/// wraps early, or the next width change rewraps it around a blank.
#[test]
fn rows_only_resize_keeps_the_wrap_spacer() {
    let mut t = run(5, 2, "abcd中");
    t.resize(5, 3);
    t.resize(6, 3);
    assert_eq!(t.screen_text(), "abcd中\n\n");
}

/// Scrollback is not cut to a one-column pane, so it comes back whole; a
/// wide character in it shows there as a blank, not as half of one.
#[test]
fn one_column_view_of_wide_scrollback() {
    let mut t = Terminal::new(Options {
        cols: 4,
        rows: 1,
        scrollback_lines: 10,
        ambiguous_wide: false,
    });
    feed(&mut t, "中中\r\n中中\r\n");
    t.resize(1, 1);
    t.scroll_viewport(1);
    let c = cell(&snap(&mut t), 0, 0);
    assert_eq!((c.width, c.len), (1, 0));
    t.resize(4, 1);
    assert_eq!(t.scrollback_text(), "中中\n中中");
}

#[test]
fn resize_rewraps_the_main_screen_under_the_alternate_one() {
    let mut t = run(5, 3, "abcdefg\x1b[?1049hALT-SCREEN");
    t.resize(10, 3);
    feed(&mut t, "\x1b[?1049lX");
    assert_eq!(t.screen_text(), "abcdefgX\n\n");
}

#[test]
fn style_table_compacts_when_full() {
    let mut t = run(10, 2, "\x1b[31mR\x1b[0m");
    // More distinct colours than style ids, all drawn over one cell.
    for i in 0..70_000u32 {
        let (r, g, b) = (i >> 16, (i >> 8) & 255, i & 255);
        feed(&mut t, &format!("\x1b[1;2H\x1b[38;2;{r};{g};{b}mx"));
    }
    feed(&mut t, "\x1b[44my\x1b[K");
    let s = snap(&mut t);
    assert_eq!(cell(&s, 0, 0).fg, PAL.ansi[1], "a style still on screen");
    assert_eq!(cell(&s, 1, 0).fg, 0x01116F);
    assert_eq!(
        (cell(&s, 2, 0).fg, cell(&s, 2, 0).bg),
        (0x01116F, PAL.ansi[4])
    );
    assert_eq!(cell(&s, 5, 0).bg, PAL.ansi[4]);
}
