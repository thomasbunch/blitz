//! Events the terminal reports to the host, mostly from OSC strings.

use vt::{Event, Options, Palette, PromptMark, Snapshot, Terminal};

fn events(s: &str) -> Vec<Event> {
    let mut t = Terminal::new(Options::default());
    t.feed(s.as_bytes());
    let mut out = Vec::new();
    t.take_events(&mut out);
    // A byte at a time gives the same events.
    let mut bytewise = Terminal::new(Options::default());
    for b in s.as_bytes() {
        bytewise.feed(std::slice::from_ref(b));
    }
    let mut again = Vec::new();
    bytewise.take_events(&mut again);
    assert_eq!(again, out, "bytewise");
    out
}

fn one(s: &str) -> Event {
    let ev = events(s);
    assert_eq!(ev.len(), 1, "{s:?}: {ev:?}");
    ev.into_iter().next().unwrap()
}

fn notify(title: &str, body: &str) -> Event {
    Event::Notify {
        title: title.into(),
        body: body.into(),
    }
}

#[test]
fn titles_with_bel_or_st() {
    assert_eq!(
        one("\x1b]0;✳ Claude Code\x1b\\"),
        Event::Title("✳ Claude Code".into())
    );
    assert_eq!(
        one("\x1b]2;◐ working\x07"),
        Event::Title("◐ working".into())
    );
    assert_eq!(one("\x1b]0;\x07"), Event::Title(String::new()));
    // Icon name only.
    assert_eq!(events("\x1b]1;icon\x07"), []);
}

#[test]
fn bell_outside_strings_only() {
    assert_eq!(
        events("a\x07b\x1b]0;t\x07"),
        [Event::Bell, Event::Title("t".into())]
    );
}

#[test]
fn blitz_hook_notifications() {
    assert_eq!(
        one("\x1b]777;notify;blitz:needs-you;Allow Bash; ls?\x07"),
        notify("blitz:needs-you", "Allow Bash; ls?")
    );
    assert_eq!(
        one("\x1b]777;notify;blitz:done;\x07"),
        notify("blitz:done", "")
    );
    assert_eq!(
        one("\x1b]777;notify;blitz:idle\x07"),
        notify("blitz:idle", "")
    );
    assert_eq!(events("\x1b]777;other;x;y\x07"), []);
    // C1 controls are dropped and long text is cut.
    let long = "x".repeat(1000);
    let Event::Notify { title, body } = one(&format!("\x1b]777;notify;{long};a\u{9b}{long}\x07"))
    else {
        panic!()
    };
    assert_eq!((title.len(), body.len()), (64, 256));
    assert!(body.starts_with("ax"));
}

#[test]
fn osc9_classify() {
    assert_eq!(one("\x1b]9;build done\x07"), notify("", "build done"));
    assert_eq!(
        one("\x1b]9;4;3;\x07"),
        Event::Progress {
            state: 3,
            pct: None
        }
    );
    assert_eq!(
        one("\x1b]9;4;1;42\x1b\\"),
        Event::Progress {
            state: 1,
            pct: Some(42)
        }
    );
    assert_eq!(
        one("\x1b]9;9;\"C:\\Users\\x y\"\x07"),
        Event::Cwd("C:\\Users\\x y".into())
    );
    assert_eq!(
        one("\x1b]9;12\x07"),
        Event::Prompt(PromptMark::A { blitz: false })
    );
    assert_eq!(
        events("\x1b]9;6;Close(0)\x07\x1b]9;8;PATH\x07\x1b]9;4;9\x07"),
        []
    );
}

#[test]
fn cwd_from_osc7() {
    assert_eq!(
        one("\x1b]7;file:///C:/Users/me/My%20Dir\x1b\\"),
        Event::Cwd("C:\\Users\\me\\My Dir".into())
    );
    assert_eq!(events("\x1b]7;kitty-shell-cwd://h/x\x07"), []);
}

#[test]
fn cwd_must_be_local() {
    // Looking at a share makes Windows sign in to its host, and a long
    // path is slow to search for a git branch.
    let long = format!("\x1b]9;9;C:{}\x07", "\\a".repeat(5000));
    for s in [
        "\x1b]9;9;\\\\192.0.2.1\\s\x07",
        "\x1b]9;9;\"//192.0.2.1/s\"\x07",
        "\x1b]9;9;\\\\?\\UNC\\192.0.2.1\\s\x07",
        "\x1b]9;9;work\\x\x07",
        "\x1b]7;file:///C:/a%1Bb\x07",
        &long,
    ] {
        assert_eq!(events(s), []);
    }
    if cfg!(windows) {
        assert_eq!(events("\x1b]7;file://192.0.2.1/s/x\x07"), []);
    }
}

#[test]
fn prompt_marks() {
    let ev = events(concat!(
        "\x1b]133;D;1\x07\x1b]133;A;blitz=1\x07$ \x1b]133;B\x07ls\r\n",
        "\x1b]133;C\x07\x1b]133;A;redraw=0\x07\x1b]133;D\x07\x1b]133;Q\x07",
    ));
    assert_eq!(
        ev,
        [
            PromptMark::D(Some(1)),
            PromptMark::A { blitz: true },
            PromptMark::B,
            PromptMark::C,
            PromptMark::A { blitz: false },
            PromptMark::D(None),
        ]
        .map(Event::Prompt)
    );
}

#[test]
fn hyperlinks_do_not_print() {
    let mut t = Terminal::new(Options::default());
    t.feed(b"\x1b]8;id=1;https://example.com/a;b\x1b\\link\x1b]8;;\x1b\\ text");
    assert_eq!(t.screen_text().lines().next(), Some("link text"));
    let mut ev = Vec::new();
    t.take_events(&mut ev);
    assert_eq!(ev, []);
}

#[test]
fn ignored_strings_leave_no_trace() {
    let mut t = Terminal::new(Options::default());
    t.feed(b"\x1b]52;c;aGk=\x07\x1b]104\x07\x1b]1337;SetUserVar=a=b\x07\x1bP+q544e\x1b\\ok");
    let (mut ev, mut r) = (Vec::new(), Vec::new());
    t.take_events(&mut ev);
    t.take_replies(&mut r);
    assert_eq!((ev, r), (vec![], vec![]));
    assert_eq!(t.screen_text().lines().next(), Some("ok"));
}

const PAL: Palette = Palette {
    fg: 0x111111,
    bg: 0x222222,
    cursor: 0x333333,
    selection_bg: 0,
    ansi: [0; 16],
};

fn replies(t: &mut Terminal, s: &str) -> String {
    t.feed(s.as_bytes());
    let mut r = Vec::new();
    t.take_replies(&mut r);
    String::from_utf8(r).unwrap()
}

#[test]
fn colour_queries_follow_the_host_palette() {
    let mut t = Terminal::new(Options::default());
    assert_eq!(
        replies(&mut t, "\x1b]11;?\x07"),
        "\x1b]11;rgb:1313/1414/1717\x07"
    );
    t.set_theme(false);
    assert_eq!(
        replies(&mut t, "\x1b]10;?\x1b\\"),
        "\x1b]10;rgb:2f2f/3131/3535\x1b\\"
    );
    t.snapshot(&mut Snapshot::default(), &PAL);
    assert_eq!(
        replies(&mut t, "\x1b]10;?;?;?\x07"),
        concat!(
            "\x1b]10;rgb:1111/1111/1111\x07",
            "\x1b]11;rgb:2222/2222/2222\x07",
            "\x1b]12;rgb:3333/3333/3333\x07",
        )
    );
}

#[test]
fn programs_can_set_and_reset_colours() {
    let mut t = Terminal::new(Options::default());
    t.feed(b"x\x1b]11;#102030\x07\x1b]12;rgb:ff/00/00\x1b\\");
    let mut s = Snapshot::default();
    t.snapshot(&mut s, &PAL);
    assert_eq!((s.cells[0].fg, s.cells[0].bg), (PAL.fg, 0x102030));
    assert_eq!(
        replies(&mut t, "\x1b]11;?\x07\x1b]12;?\x07"),
        "\x1b]11;rgb:1010/2020/3030\x07\x1b]12;rgb:ffff/0000/0000\x07"
    );
    t.feed(b"\x1b]111\x07\x1b]112\x07");
    assert!(t.snapshot(&mut s, &PAL));
    assert_eq!(s.cells[0].bg, PAL.bg);
    assert_eq!(
        replies(&mut t, "\x1b]11;?\x07"),
        "\x1b]11;rgb:2222/2222/2222\x07"
    );
}

#[test]
fn floods_are_coalesced_and_bounded() {
    let mut t = Terminal::new(Options::default());
    for i in 0..5000 {
        t.feed(format!("\x07\x1b]0;t{i}\x07\x1b]7;file:///C:/d{i}\x07").as_bytes());
    }
    let mut out = Vec::new();
    t.take_events(&mut out);
    assert_eq!(
        out,
        [
            Event::Bell,
            Event::Title("t4999".into()),
            Event::Cwd("C:\\d4999".into())
        ]
    );

    for i in 0..5000 {
        t.feed(format!("\x1b]9;n{i}\x07").as_bytes());
    }
    t.take_events(&mut out);
    assert_eq!(out.len(), 3 + 1024);
    assert_eq!(out.last(), Some(&notify("", "n4999")));
}
