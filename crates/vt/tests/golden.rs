//! Screen goldens: synthetic byte streams and the screen they must leave.

use vt::snapshot::attr;
use vt::{Options, Palette, RenderCell, Snapshot, Terminal};

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
/// at a time leaves the same screen.
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
            "\x1b[38;5mm\x1b[8mn\x1b[0;4mo\x1b[24mp",
        ),
    );
    let s = snap(&mut t);
    let fg: Vec<u32> = (0..16).map(|x| cell(&s, x, 0).fg).collect();
    assert_eq!(
        fg,
        [
            0xAA0000, 0xFF0000, 0x010203, 0x0A141E, 0x070809, 0x070809, PAL.bg, PAL.fg, PAL.fg,
            PAL.fg, 0x5555FF, PAL.fg, PAL.fg, PAL.bg, PAL.fg, PAL.fg
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
