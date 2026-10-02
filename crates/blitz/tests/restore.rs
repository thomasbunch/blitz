//! Saved output fed into a screen before its shell starts survives the
//! console host attaching. Set `BLITZ_CONPTY_DIR` to test the bundled
//! ConPTY; otherwise the system's is used.
#![cfg(windows)]

use std::sync::{Arc, Mutex, Once};
use std::time::{Duration, Instant};

use blitz::pane::{lock, restored};
use blitz::pty::{Pty, PtyEvent, SpawnOpts};

/// Starts `cmdline` on a screen that was fed `pre`, waits for `prompt`,
/// runs `echo after` and returns the scrollback and the screen.
fn attach(cmdline: &str, pre: &[u8], prompt: &str) -> (String, String) {
    // A failing test must not leave console hosts running.
    static JOB: Once = Once::new();
    JOB.call_once(|| blitz::pty::kill_children_on_exit().expect("job object"));
    let mut term = vt::Terminal::new(vt::Options {
        cols: 80,
        rows: 24,
        scrollback_lines: 1000,
        ..Default::default()
    });
    term.feed(pre);
    let term = Arc::new(Mutex::new(term));
    let t = term.clone();
    let pty = Pty::spawn(
        &SpawnOpts {
            cmdline,
            cols: 80,
            rows: 24,
            pane_id: 1,
            ..Default::default()
        },
        move |ev, w| {
            if let PtyEvent::Data(d) = ev {
                let mut term = lock(&t);
                term.feed(d);
                let mut replies = Vec::new();
                term.take_replies(&mut replies);
                if !replies.is_empty() {
                    w.reply(replies);
                }
            }
        },
    )
    .expect("spawn");
    let wait = |what: &str| {
        let deadline = Instant::now() + Duration::from_secs(30);
        while !lock(&term).screen_text().contains(what) {
            assert!(
                Instant::now() < deadline,
                "no {what:?} on {:?}",
                lock(&term).screen_text()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    };
    wait(prompt);
    pty.writer().send(&b"echo after\r"[..]);
    wait("\nafter");
    let term = lock(&term);
    (term.scrollback_text(), term.screen_text())
}

#[test]
fn restored_output_survives_the_shell_starting() {
    let old: String = (1..=50).map(|i| format!("old line {i}\n")).collect();
    let pre = restored(&old, "14:32", 24);
    for (cmdline, prompt) in [("cmd.exe /d", ">"), ("powershell.exe -NoLogo", "PS ")] {
        let (scrollback, screen) = attach(cmdline, &pre, prompt);
        assert!(
            scrollback.contains(&format!("{old}── restored · 14:32 ──")),
            "{cmdline}: {scrollback:?}"
        );
        assert!(!screen.contains("old line"), "{cmdline}: {screen:?}");
        assert!(
            screen.contains("echo after\nafter"),
            "{cmdline}: {screen:?}"
        );
    }
}
