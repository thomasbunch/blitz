//! Input modes as the key, mouse and paste encoders see them.

use std::time::{Duration, Instant};

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

const PAL: vt::Palette = vt::Palette {
    fg: 1,
    bg: 2,
    cursor: 3,
    selection_bg: 4,
    ansi: [0; 16],
};

fn first_char(s: &vt::Snapshot) -> &[u8] {
    &s.cells[0].text[..s.cells[0].len as usize]
}

#[test]
fn synchronized_output_holds_snapshots() {
    let mut t = term("a");
    let mut s = vt::Snapshot::default();
    assert!(t.snapshot(&mut s, &PAL));
    t.feed(b"\x1b[?2026h\rb");
    let now = Instant::now();
    assert!(t.sync_pending(now));
    assert!(!t.sync_pending(now + Duration::from_millis(150)));
    assert!(!t.snapshot(&mut s, &PAL), "held during the update");
    assert_eq!(first_char(&s), b"a");
    t.feed(b"\x1b[?2026l");
    assert!(!t.sync_pending(Instant::now()));
    assert!(t.snapshot(&mut s, &PAL));
    assert_eq!(first_char(&s), b"b");
}

#[test]
fn synchronized_output_times_out() {
    let mut t = term("\x1b[?2026hx");
    let mut s = vt::Snapshot::default();
    assert!(!t.snapshot(&mut s, &PAL));
    std::thread::sleep(Duration::from_millis(160));
    assert!(!t.sync_pending(Instant::now()));
    assert!(t.snapshot(&mut s, &PAL), "shown after 150 ms");
    assert_eq!(first_char(&s), b"x");
    // The mode stays set until the program ends the update.
    let mut r = Vec::new();
    t.feed(b"\x1b[?2026$p");
    t.take_replies(&mut r);
    assert_eq!(r, b"\x1b[?2026;1$y");
}

/// Modes a crashed full-screen program leaves behind.
const LEFTOVERS: &str =
    "\x1b[?1049h\x1b[>5u\x1b[>4;2m\x1b[?1000h\x1b[?1006h\x1b[?2004h\x1b[?2026h\x1b[?1049l\x1b[>1u";

fn assert_clean(t: &Terminal) {
    let m = t.input_modes();
    assert_eq!(
        (t.kitty_stack(false), t.kitty_stack(true)),
        (&[][..], &[][..])
    );
    assert_eq!(
        (m.mouse, m.mouse_sgr, m.bracketed),
        (MouseMode::Off, false, false)
    );
    assert!(!t.sync_pending(Instant::now()));
    // Modes the program did not touch, and the ones ConPTY owns, stay.
    assert!(m.w32im && m.focus && m.decckm);
}

fn leftovers() -> Terminal {
    let mut t = term("\x1b[?9001h\x1b[?1004h\x1b[?1h");
    t.feed(LEFTOVERS.as_bytes());
    assert_eq!(t.input_modes().kitty, 1);
    t
}

#[test]
fn child_exit_resets_input_modes() {
    let mut t = leftovers();
    t.on_child_exit();
    assert_clean(&t);
}

#[test]
fn blitz_prompt_resets_input_modes() {
    let mut t = leftovers();
    t.feed(b"\x1b]133;A;blitz=1\x07");
    assert_clean(&t);
    let mut t = leftovers();
    t.feed(b"\x1b]133;A;aid=1;blitz=1\x1b\\");
    assert_clean(&t);
}

#[test]
fn other_prompts_keep_input_modes() {
    for mark in [
        "133;A",
        "133;A;redraw=0",
        "133;A;blitz=0",
        "133;B",
        "133;D;0",
        "9;12",
    ] {
        let mut t = leftovers();
        t.feed(format!("\x1b]{mark}\x07").as_bytes());
        let m = t.input_modes();
        assert_eq!(m.kitty, 1, "{mark}");
        assert!(m.bracketed && m.mouse_sgr, "{mark}");
        assert_eq!(t.kitty_stack(true), [5], "{mark}");
    }
}

#[test]
fn only_the_hosts_token_marks_a_blitz_prompt() {
    let mut t = leftovers();
    t.set_prompt_token("9c1e");
    for mark in ["133;A;blitz=1", "133;A;blitz=9c1", "133;A;blitz=9c1e0"] {
        t.feed(format!("\x1b]{mark}\x07").as_bytes());
        let m = t.input_modes();
        assert!(m.bracketed && m.kitty == 1, "{mark}");
    }
    t.feed(b"\x1b]133;A;blitz=9c1e\x07");
    assert_clean(&t);
    // A program's reset does not bring back the default token.
    t.feed(b"\x1bc");
    t.feed(LEFTOVERS.as_bytes());
    t.feed(b"\x1b]133;A;blitz=1\x07");
    assert!(t.input_modes().bracketed);
}

#[test]
fn ris_resets_input_modes() {
    let mut t = leftovers();
    t.feed(b"\x1b[?1h\x1bc\x1b[?1h");
    assert_clean(&t);
}
