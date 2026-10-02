//! Input modes as the key, mouse and paste encoders see them.

use vt::{InputModes, MouseMode, Options, Terminal};

fn term(s: &str) -> Terminal {
    let mut t = Terminal::new(Options::default());
    t.feed(s.as_bytes());
    t
}

#[test]
fn modes_start_off() {
    assert_eq!(term("").input_modes(), InputModes::default());
}

#[test]
fn dec_modes_reach_input_modes() {
    let mut t = term("\x1b[?1h\x1b=\x1b[?2004h\x1b[?1004h\x1b[?9001h\x1b[?1002;1006h");
    let m = t.input_modes();
    assert!(m.decckm && m.deckpam && m.bracketed && m.focus && m.w32im && m.mouse_sgr);
    assert_eq!(m.mouse, MouseMode::Drag);

    t.feed(b"\x1b[?1l\x1b>\x1b[?2004l\x1b[?1004l\x1b[?9001l\x1b[?1002l\x1b[?1006l");
    assert_eq!(t.input_modes(), InputModes::default());

    t.feed(b"\x1b[?66h");
    assert!(t.input_modes().deckpam);
    t.feed(b"\x1b[?1h\x1b[!p");
    assert!(
        !t.input_modes().decckm && !t.input_modes().deckpam,
        "DECSTR"
    );
}

#[test]
fn kitty_stacks_are_per_screen() {
    let mut t = term("\x1b[>1u\x1b[>5u");
    assert_eq!(t.input_modes().kitty, 5);
    t.feed(b"\x1b[?1049h");
    assert_eq!(t.input_modes().kitty, 0);
    assert!(t.input_modes().alt_screen);
    t.feed(b"\x1b[>4u\x1b[=1;2u");
    assert_eq!(t.input_modes().kitty, 5);
    assert_eq!(
        (t.kitty_stack(false), t.kitty_stack(true)),
        (&[1, 5][..], &[5][..])
    );
    t.feed(b"\x1b[<u\x1b[?1049l");
    assert_eq!(t.input_modes().kitty, 5);
    t.feed(b"\x1b[<5u");
    assert_eq!(t.input_modes().kitty, 0);
    assert_eq!(t.kitty_stack(false), []);
}

#[test]
fn modify_other_keys_is_not_sgr() {
    let mut t = term("x\x1b[>4;2my");
    t.feed(b"\x1b[>4m");
    let mut s = vt::Snapshot::default();
    let pal = vt::Palette {
        fg: 1,
        bg: 2,
        cursor: 3,
        selection_bg: 4,
        ansi: [0; 16],
    };
    t.snapshot(&mut s, &pal);
    // `4` would underline and `2` dim if this were read as SGR.
    assert_eq!(s.cells[1].attrs, 0);
    assert_eq!(t.screen_text().lines().next(), Some("xy"));
}

#[test]
fn ris_keeps_conpty_modes() {
    let mut t = term("\x1b[?9001h\x1b[?1004h\x1b[?2004h\x1b[>1u\x1b[?1000hx\x1bc");
    let m = t.input_modes();
    assert!(m.w32im && m.focus);
    assert!(!m.bracketed);
    assert_eq!((m.kitty, m.mouse), (0, MouseMode::Off));
    assert_eq!(t.screen_text().lines().next(), Some(""));
    t.feed(b"\x1b[?9001l");
    assert!(!t.input_modes().w32im);
}
