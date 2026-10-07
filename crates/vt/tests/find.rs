//! Searching the screen and scrollback, and jumping between prompts.

use vt::grid::Found;
use vt::snapshot::Highlight;
use vt::{Options, Snapshot, Terminal};

fn term(cols: u16, rows: u16, scrollback_lines: usize) -> Terminal {
    Terminal::new(Options {
        cols,
        rows,
        scrollback_lines,
        ..Options::default()
    })
}

fn found(start: (usize, u16), end: (usize, u16)) -> Found {
    Found { start, end }
}

fn hl(start: (u16, u16), end: (u16, u16), current: bool) -> Highlight {
    Highlight {
        start,
        end,
        current,
    }
}

fn shot(t: &mut Terminal) -> Snapshot {
    let pal = vt::Palette {
        fg: 0xffffff,
        bg: 0,
        cursor: 0xffffff,
        cursor_text: None,
        selection_bg: 0x333333,
        selection_fg: 0xffffff,
        ansi: [0; 16],
    };
    let mut s = Snapshot::default();
    t.snapshot(&mut s, &pal);
    s
}

#[test]
fn a_match_can_run_across_a_soft_wrap() {
    let mut t = term(10, 3, 100);
    t.feed(b"hello world foo\r\nhello\r\nworld");
    // "hello worl" wraps into "d foo".
    assert_eq!(
        t.find("world"),
        [found((0, 6), (1, 0)), found((3, 0), (3, 4))]
    );
    assert_eq!(t.find("hello world foo"), [found((0, 0), (1, 4))]);
    // A line break is not a soft wrap.
    assert_eq!(t.find("helloworld"), []);
}

#[test]
fn case_is_ignored_unless_the_query_has_a_capital() {
    let mut t = term(40, 2, 0);
    t.feed("Error error ERROR Straße".as_bytes());
    assert_eq!(t.find("error").len(), 3);
    assert_eq!(t.find("Error"), [found((0, 0), (0, 4))]);
    assert_eq!(t.find("ERROR"), [found((0, 12), (0, 16))]);
    assert_eq!(t.find("STRASSE"), []);
    assert_eq!(t.find("straße"), [found((0, 18), (0, 23))]);
    assert_eq!(t.find(""), []);
}

#[test]
fn matches_in_scrollback_keep_their_line_as_output_goes_on() {
    let mut t = term(20, 3, 5);
    let lines = |t: &mut Terminal, n: std::ops::Range<usize>| {
        for i in n {
            t.feed(format!("line {i}\r\n").as_bytes());
        }
    };
    lines(&mut t, 0..6);
    assert_eq!(t.find("line 1"), [found((1, 0), (1, 5))]);
    lines(&mut t, 6..16);
    // `line {i}` stays on line i while older lines drop off the top.
    assert_eq!(t.find("line 12"), [found((12, 0), (12, 6))]);
    let ones: Vec<usize> = t.find("line 1").iter().map(|f| f.start.0).collect();
    assert_eq!(ones, [10, 11, 12, 13, 14, 15]);

    // Lines 9 to 13 are scrollback, 14 to 16 the screen.
    assert_eq!(t.view_top(), 14);
    t.scroll_to(12);
    assert_eq!(t.view_top(), 12);
    t.scroll_to(0);
    assert_eq!(t.view_top(), 9, "as far as the scrollback goes");
    t.scroll_to(100);
    assert_eq!(t.view_top(), 14, "back at the bottom");
}

#[test]
fn the_alternate_screen_searches_only_itself() {
    let mut t = term(20, 2, 100);
    t.feed(b"main 1\r\nmain 2\r\nmain 3\r\n\x1b[?1049halt");
    assert_eq!(t.find("main"), []);
    assert_eq!(t.find("alt").len(), 1);
    t.feed(b"\x1b[?1049l");
    assert_eq!(t.find("main").len(), 3);
}

#[test]
fn snapshots_mark_the_matches_in_view() {
    let mut t = term(10, 2, 100);
    // Lines: "ab", "0123456789" wrapping into "ab", then "ab".
    t.feed(b"ab\r\n0123456789ab\r\nab");
    let ab = t.find("ab");
    let across = t.find("9a");
    assert_eq!(across, [found((1, 9), (2, 0))]);

    let mut s = shot(&mut t);
    assert_eq!(s.top, 2);
    s.highlight(&ab, Some(2));
    assert_eq!(
        s.highlights,
        [hl((0, 0), (1, 0), false), hl((0, 1), (1, 1), true)]
    );
    // A match that starts above the view is marked from its first cell.
    s.highlight(&across, None);
    assert_eq!(s.highlights, [hl((0, 0), (0, 0), false)]);

    t.scroll_to(1);
    let mut s = shot(&mut t);
    s.highlight(&across, Some(0));
    assert_eq!(s.highlights, [hl((9, 0), (0, 1), true)]);
    s.highlight(&ab, None);
    assert_eq!(s.highlights, [hl((0, 1), (1, 1), false)], "line 3 is below");

    // One that runs off the bottom is marked to the last cell.
    t.scroll_to(0);
    let mut s = shot(&mut t);
    s.highlight(&across, None);
    assert_eq!(s.highlights, [hl((9, 1), (9, 1), false)]);
}

/// blitz's own prompt start, as its shell integration sends it.
const PROMPT: &str = "\x1b]133;A;blitz=1\x07$ ";

/// A shell session on a 20 x 4 screen: a prompt and a command on lines 0,
/// 5, 10 and 15, output in between, and a fresh prompt on line 20.
fn session() -> Terminal {
    let mut t = term(20, 4, 100);
    for i in 0..20 {
        let line = if i % 5 == 0 {
            format!("{PROMPT}command {i}\r\n")
        } else {
            format!("output {i}\r\n")
        };
        t.feed(line.as_bytes());
    }
    t.feed(PROMPT.as_bytes());
    t
}

/// The text of the top row of the view.
fn top_row(t: &mut Terminal) -> String {
    let s = shot(t);
    let row = &s.cells[..usize::from(s.cols)];
    let text: String = (row.iter())
        .map(|c| std::str::from_utf8(&c.text[..usize::from(c.len)]).unwrap_or(""))
        .map(|t| if t.is_empty() { " " } else { t })
        .collect();
    text.trim_end().to_string()
}

#[test]
fn prompt_jumps_go_up_and_down_and_back_to_the_bottom() {
    let mut t = session();
    // Lines 17 to 20 are on screen.
    assert_eq!(t.view_top(), 17);
    for want in [15, 10, 5, 0] {
        assert!(t.jump_to_prompt(true));
        assert_eq!(t.view_top(), want);
    }
    assert_eq!(top_row(&mut t), "$ command 0");
    assert!(!t.jump_to_prompt(true), "no prompt above the first");
    assert_eq!(t.view_top(), 0);
    for want in [5, 10, 15] {
        assert!(t.jump_to_prompt(false));
        assert_eq!(t.view_top(), want);
    }
    // The last prompt is on the screen, so down from 15 is the bottom.
    assert!(t.jump_to_prompt(false));
    assert_eq!(t.view_top(), 17);
    assert!(!t.jump_to_prompt(false), "already at the bottom");
}

#[test]
fn prompt_jumps_need_a_prompt_that_way() {
    let mut t = term(20, 4, 100);
    // Prompt starts that are not blitz's own do not count.
    for i in 0..10 {
        t.feed(format!("\x1b]133;A\x07$ x\r\n\x1b]133;A;blitz=2\x07{i}\r\n").as_bytes());
    }
    assert!(!t.jump_to_prompt(true));

    let mut t = term(20, 4, 100);
    t.feed(format!("{PROMPT}build\r\n").as_bytes());
    for i in 0..30 {
        t.feed(format!("output {i}\r\n").as_bytes());
    }
    t.scroll_to(10);
    // Down: nothing marked below line 10, so the key is not used.
    assert!(!t.jump_to_prompt(false));
    assert_eq!(t.view_top(), 10);
    assert!(t.jump_to_prompt(true));
    assert_eq!(t.view_top(), 0);

    // Nor on the alternate screen.
    let mut t = session();
    t.feed(b"\x1b[?1049h");
    assert!(!t.jump_to_prompt(true));
}

#[test]
fn prompt_marks_survive_scrolling_off_and_rewrapping() {
    let mut t = session();
    // Narrower: each "$ command N" line wraps in two.
    t.resize(6, 4);
    let mut tops = Vec::new();
    while t.jump_to_prompt(true) {
        tops.push(top_row(&mut t));
    }
    assert_eq!(tops, ["$ comm"; 4]);
    // Wider again: the marks come back to whole lines.
    t.resize(20, 4);
    let mut rows = Vec::new();
    while t.jump_to_prompt(false) {
        rows.push(top_row(&mut t));
    }
    assert_eq!(
        rows,
        ["$ command 5", "$ command 10", "$ command 15", "output 17"]
    );
}
