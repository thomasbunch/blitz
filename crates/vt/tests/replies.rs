//! Answers to terminal queries: one test per query blitz must answer.
//! Every stream is also fed a byte at a time and must give the same bytes.

use vt::{Options, Terminal};

fn opts() -> Options {
    Options {
        cols: 80,
        rows: 24,
        ..Options::default()
    }
}

fn replies_of(t: &mut Terminal) -> String {
    let mut out = Vec::new();
    t.take_replies(&mut out);
    String::from_utf8(out).unwrap()
}

/// The replies to `s`, checked to be the same when fed a byte at a time.
fn ask_on(t: &mut Terminal, s: &str) -> String {
    t.feed(s.as_bytes());
    replies_of(t)
}

fn ask(s: &str) -> String {
    let whole = ask_on(&mut Terminal::new(opts()), s);
    let mut t = Terminal::new(opts());
    for b in s.as_bytes() {
        t.feed(std::slice::from_ref(b));
    }
    assert_eq!(replies_of(&mut t), whole, "bytewise");
    whole
}

#[test]
fn da1_is_the_first_reply() {
    let mut t = Terminal::new(opts());
    t.feed(b"\x1b[c");
    let mut out = Vec::new();
    t.take_replies(&mut out);
    assert_eq!(out, b"\x1b[?62;22c");
    assert_eq!(ask("\x1b[0c"), "\x1b[?62;22c");
    // DA3 and other private forms are not DA1.
    assert_eq!(ask("\x1b[=c\x1b[1c"), "");
}

#[test]
fn da2() {
    assert_eq!(ask("\x1b[>c"), "\x1b[>1;0;0c");
    assert_eq!(ask("\x1b[>0c"), "\x1b[>1;0;0c");
}

#[test]
fn xtversion() {
    let want = format!("\x1bP>|blitz {}\x1b\\", env!("CARGO_PKG_VERSION"));
    assert_eq!(ask("\x1b[>0q"), want);
    assert_eq!(ask("\x1b[>q"), want);
}

#[test]
fn kitty_flags_query() {
    assert_eq!(ask("\x1b[?u"), "\x1b[?0u");
    assert_eq!(ask("\x1b[>5u\x1b[?u"), "\x1b[?5u");
    assert_eq!(ask("\x1b[>5u\x1b[?1049h\x1b[?u"), "\x1b[?0u");
}

#[test]
fn decrqm_table() {
    let rows = [
        ("\x1b[?2026$p", "\x1b[?2026;2$y"),
        ("\x1b[?2026h\x1b[?2026$p", "\x1b[?2026;1$y"),
        ("\x1b[?2027$p", "\x1b[?2027;3$y"),
        ("\x1b[?1016$p", "\x1b[?1016;0$y"),
        ("\x1b[?1016h\x1b[?1016$p", "\x1b[?1016;0$y"),
        ("\x1b[?31337$p", "\x1b[?31337;0$y"),
        ("\x1b[?1$p", "\x1b[?1;2$y"),
        ("\x1b[?1h\x1b[?1$p", "\x1b[?1;1$y"),
        ("\x1b[?7$p", "\x1b[?7;1$y"),
        ("\x1b[?25l\x1b[?25$p", "\x1b[?25;2$y"),
        ("\x1b[?1049h\x1b[?1049$p", "\x1b[?1049;1$y"),
        (
            "\x1b[?1003h\x1b[?1003$p\x1b[?1000$p",
            "\x1b[?1003;1$y\x1b[?1000;2$y",
        ),
        ("\x1b[?2004h\x1b[?2004$p", "\x1b[?2004;1$y"),
        ("\x1b[?9001$p", "\x1b[?9001;2$y"),
        ("\x1b[4$p", "\x1b[4;2$y"),
        ("\x1b[4h\x1b[4$p", "\x1b[4;1$y"),
        ("\x1b[20$p", "\x1b[20;2$y"),
        ("\x1b[20h\x1b[20$p", "\x1b[20;1$y"),
        ("\x1b[3$p", "\x1b[3;0$y"),
        ("\x1b[?6$p", "\x1b[?6;2$y"),
        ("\x1b[?6h\x1b[?6$p", "\x1b[?6;1$y"),
        ("\x1b[?7l\x1b[?7$p", "\x1b[?7;2$y"),
        ("\x1b[?25$p", "\x1b[?25;1$y"),
        ("\x1b[?66h\x1b[?66$p", "\x1b[?66;1$y"),
        ("\x1b=\x1b[?66$p", "\x1b[?66;1$y"),
        ("\x1b[?1000h\x1b[?1000$p", "\x1b[?1000;1$y"),
        ("\x1b[?1002h\x1b[?1002$p", "\x1b[?1002;1$y"),
        ("\x1b[?1004$p", "\x1b[?1004;2$y"),
        ("\x1b[?1004h\x1b[?1004$p", "\x1b[?1004;1$y"),
        ("\x1b[?1006h\x1b[?1006$p", "\x1b[?1006;1$y"),
        ("\x1b[?1048$p", "\x1b[?1048;2$y"),
        ("\x1b[?1048h\x1b[?1048$p", "\x1b[?1048;2$y"),
        ("\x1b[?2031$p", "\x1b[?2031;2$y"),
        ("\x1b[?2031h\x1b[?2031$p", "\x1b[?2031;1$y"),
        ("\x1b[?9001h\x1b[?9001$p", "\x1b[?9001;1$y"),
        (
            "\x1b[?47h\x1b[?47$p\x1b[?1047$p\x1b[?1049$p",
            "\x1b[?47;1$y\x1b[?1047;1$y\x1b[?1049;1$y",
        ),
        ("\x1b[?1049$p", "\x1b[?1049;2$y"),
    ];
    for (q, want) in rows {
        assert_eq!(ask(q), want, "{q:?}");
    }
}

#[test]
fn cursor_position_reports() {
    assert_eq!(ask("\x1b[5;10H\x1b[6n"), "\x1b[5;10R");
    // DECXCPR: exactly two parameters, no page.
    assert_eq!(ask("\x1b[5;10H\x1b[?6n"), "\x1b[?5;10R");
    // Origin mode counts from the top margin.
    assert_eq!(ask("\x1b[3;20r\x1b[?6h\x1b[2;4H\x1b[6n"), "\x1b[2;4R");
    // Pending wrap stays on the last column.
    let mut t = Terminal::new(Options {
        cols: 10,
        rows: 3,
        ..Options::default()
    });
    assert_eq!(ask_on(&mut t, "0123456789\x1b[6n"), "\x1b[1;10R");
    // The row rewraps in two at the new width.
    t.resize(5, 3);
    assert_eq!(ask_on(&mut t, "\x1b[6n"), "\x1b[2;5R", "after a resize");
}

#[test]
fn device_status() {
    assert_eq!(ask("\x1b[5n"), "\x1b[0n");
    // Reports blitz does not make get no answer at all.
    assert_eq!(ask("\x1b[?5n\x1b[?15n\x1b[?26n\x1b[7n\x1b[n"), "");
}

#[test]
fn theme_query() {
    assert_eq!(ask("\x1b[?996n"), "\x1b[?997;1n");
    let mut t = Terminal::new(opts());
    let pal = vt::Palette {
        fg: 0,
        bg: 0xffffff,
        cursor: 0,
        selection_bg: 0,
        ansi: [0; 16],
    };
    t.set_theme(false, &pal);
    assert_eq!(ask_on(&mut t, "\x1b[?996n"), "\x1b[?997;2n");
    t.feed(b"\x1bc");
    assert_eq!(ask_on(&mut t, "\x1b[?996n"), "\x1b[?997;2n", "RIS keeps it");
}

#[test]
fn theme_change_reports() {
    let mut t = Terminal::new(opts());
    let light = vt::Palette {
        fg: 0,
        bg: 0xffffff,
        cursor: 0,
        selection_bg: 0,
        ansi: [0; 16],
    };
    t.set_theme(false, &light);
    assert_eq!(replies_of(&mut t), "", "not asked for");
    t.feed(b"\x1b[?2031h");
    t.set_theme(false, &light);
    assert_eq!(replies_of(&mut t), "", "nothing changed");
    t.set_theme(true, &light);
    assert_eq!(replies_of(&mut t), "\x1b[?997;1n");
    t.set_theme(false, &light);
    assert_eq!(replies_of(&mut t), "\x1b[?997;2n");
    // A palette change alone is reported too, so the program asks again.
    t.set_theme(false, &vt::Palette { fg: 1, ..light });
    assert_eq!(replies_of(&mut t), "\x1b[?997;2n");
    t.feed(b"\x1b[?2031l");
    t.set_theme(true, &light);
    assert_eq!(replies_of(&mut t), "");
}

#[test]
fn size_reports() {
    let mut t = Terminal::new(opts());
    t.set_cell_px(9, 19);
    assert_eq!(ask_on(&mut t, "\x1b[14t"), "\x1b[4;456;720t");
    assert_eq!(ask_on(&mut t, "\x1b[16t"), "\x1b[6;19;9t");
    assert_eq!(ask_on(&mut t, "\x1b[18t"), "\x1b[8;24;80t");
    // ConPTY sends CSI 1 t at startup; window operations are ignored.
    assert_eq!(ask_on(&mut t, "\x1b[1t\x1b[2t\x1b[8;10;10t"), "");
    assert_eq!(ask_on(&mut t, "\x1b[18t"), "\x1b[8;24;80t");
}

#[test]
fn kitty_graphics_probe_is_swallowed() {
    let mut t = Terminal::new(opts());
    let r = ask_on(&mut t, "\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\\x1b[c");
    assert_eq!(r, "\x1b[?62;22c");
    assert!(!t.screen_text().contains("AAAA"));
}

#[test]
fn claude_probe_batches_in_order() {
    let first = ask("\x1b[>0q\x1b[?u\x1b[c");
    assert_eq!(
        first,
        format!(
            "\x1bP>|blitz {}\x1b\\\x1b[?0u\x1b[?62;22c",
            env!("CARGO_PKG_VERSION")
        )
    );
    let second =
        ask("\x1b[?2026$p\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\\x1b[16t\x1b[?1016$p\x1b[c");
    assert_eq!(
        second,
        "\x1b[?2026;2$y\x1b[6;0;0t\x1b[?1016;0$y\x1b[?62;22c"
    );
}

#[test]
fn pending_replies_are_bounded() {
    let mut t = Terminal::new(opts());
    let flood = "\x1b[c\x1b]10;?;?;?\x07".repeat(10_000);
    let r = ask_on(&mut t, &flood);
    assert!(r.len() < 4200, "{}", r.len());
    assert!(r.ends_with('\x07') || r.ends_with('c'));
    // More output makes room again.
    t.feed(" ".repeat(100).as_bytes());
    assert_eq!(ask_on(&mut t, "\x1b[5n"), "\x1b[0n");
}

/// The host takes replies after every read, and reads are small, so the
/// limit must hold however the output is split. A reset must not refill it.
#[test]
fn replies_stay_a_share_of_output() {
    let mut t = Terminal::new(opts());
    let flood = "\x1bc\x1b[c\x1b]10;?;?;?\x07".repeat(10_000);
    let mut total = 0;
    for chunk in flood.as_bytes().chunks(11) {
        t.feed(chunk);
        total += replies_of(&mut t).len();
    }
    assert!(total <= flood.len() / 16 + 4096, "{total}");
}
