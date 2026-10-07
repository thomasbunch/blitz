//! Spawns real processes on a pseudoconsole. Set `BLITZ_CONPTY_DIR` to a
//! folder with `conpty.dll` and `OpenConsole.exe` to test the bundled
//! ConPTY; otherwise the system's is used.
#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::sync::{Once, mpsc};
use std::time::{Duration, Instant};

use blitz::pty::{Pty, PtyEvent, SpawnOpts};

/// As long as a real pane's token, so sequences that carry one are too.
const TOKEN: &str = "0123456789abcdef0123456789abcdef";

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// What `out` leaves on an 80x24 screen, scrollback first.
fn text(out: &[u8]) -> String {
    let mut t = vt::Terminal::new(vt::Options {
        scrollback_lines: 100_000,
        ..Default::default()
    });
    t.feed(out);
    format!("{}\n{}", t.scrollback_text(), t.screen_text())
}

struct Run {
    started: Instant,
    chunks: Vec<(Instant, Vec<u8>)>,
    code: u32,
}

impl Run {
    fn output(&self) -> Vec<u8> {
        self.chunks
            .iter()
            .flat_map(|(_, c)| c.iter().copied())
            .collect()
    }
}

type Events = mpsc::Receiver<Result<(Instant, Vec<u8>), u32>>;

/// Starts `cmdline` and forwards its output and exit code. Answers the
/// bundled ConPTY's startup DA1 query, which otherwise holds output back
/// for about 3 s.
fn spawn(cmdline: &str, env: &[(String, String)]) -> (Pty, Instant, Events) {
    spawn_in(cmdline, env, None)
}

fn spawn_in(cmdline: &str, env: &[(String, String)], cwd: Option<&Path>) -> (Pty, Instant, Events) {
    // A failing test must not leave console hosts running.
    static JOB: Once = Once::new();
    JOB.call_once(|| blitz::pty::kill_children_on_exit().expect("job object"));
    let (tx, rx) = mpsc::channel();
    let opts = SpawnOpts {
        cmdline,
        cwd,
        env,
        cols: 80,
        rows: 24,
        pane_id: 1,
        ..Default::default()
    };
    let started = Instant::now();
    let pty = Pty::spawn(&opts, move |ev, w| match ev {
        PtyEvent::Data(d) => {
            if find(d, b"\x1b[c").is_some() {
                w.send(&b"\x1b[?62;22c"[..]);
            }
            let _ = tx.send(Ok((Instant::now(), d.to_vec())));
        }
        PtyEvent::Exit(code) => {
            let _ = tx.send(Err(code));
        }
    })
    .expect("spawn");
    (pty, started, rx)
}

/// Runs `cmdline` to completion.
fn run(cmdline: &str) -> Run {
    let (_pty, started, rx) = spawn(cmdline, &[]);
    let mut chunks = Vec::new();
    loop {
        match rx
            .recv_timeout(Duration::from_secs(30))
            .expect("no exit within 30 s")
        {
            Ok(chunk) => chunks.push(chunk),
            Err(code) => {
                return Run {
                    started,
                    chunks,
                    code,
                };
            }
        }
    }
}

/// Reads output into `out` until it holds `what`; false if the child
/// exited or 30 s passed first.
fn wait_for(rx: &Events, out: &mut Vec<u8>, what: &[u8]) -> bool {
    let deadline = Instant::now() + Duration::from_secs(30);
    while find(out, what).is_none() {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(Ok((_, chunk))) => out.extend(chunk),
            _ => return false,
        }
    }
    true
}

/// A program that cannot start is an error, and promptly: the window must
/// not freeze on it.
#[test]
fn pty_spawn_failures_are_errors() {
    // A folder too long to be a process's current directory, which a pane
    // can still have saved: Explorer and shells both reach such folders.
    let base = std::env::temp_dir().join(format!("blitz-long-{}", std::process::id()));
    let mut long = base.clone();
    while long.as_os_str().len() < 300 {
        long.push("abcdefghijklmnopqrstuvwxyz0123456789");
    }
    std::fs::create_dir_all(Path::new(r"\\?\").join(&long)).expect("long dir");
    assert!(long.is_dir());
    let cases = [
        ("no-such-program-4b1d", None),
        (r"C:\no\such\4b1d.exe", None),
        ("cmd /d /c echo hi", Some(PathBuf::from(r"C:\no\such\4b1d"))),
        ("cmd /d /c echo hi", Some(long.clone())),
    ];
    let results: Vec<_> = (cases.into_iter())
        .map(|(cmdline, cwd)| {
            let (tx, rx) = mpsc::channel();
            let what = format!("{cmdline} in {cwd:?}");
            std::thread::spawn(move || {
                let opts = SpawnOpts {
                    cmdline,
                    cwd: cwd.as_deref(),
                    cols: 80,
                    rows: 24,
                    ..Default::default()
                };
                let _ = tx.send(Pty::spawn(&opts, |_, _| {}).map(drop));
            });
            (what, rx.recv_timeout(Duration::from_secs(10)))
        })
        .collect();
    let _ = std::fs::remove_dir_all(Path::new(r"\\?\").join(&base));
    for (what, r) in results {
        let r = r.unwrap_or_else(|_| panic!("{what}: still starting after 10 s"));
        assert!(r.is_err(), "{what}");
    }
}

#[test]
fn pty_echo_reaches_the_reader() {
    let r = run("cmd /d /c echo hi");
    let screen = text(&r.output());
    assert!(screen.lines().any(|l| l.trim_end() == "hi"), "{screen:?}");
    assert_eq!(r.code, 0);
}

#[test]
fn pty_exit_codes_come_through() {
    assert_eq!(run("cmd /d /c exit 3").code, 3);
    // What a console program ended by Ctrl+C or a closed console returns.
    assert_eq!(run("cmd /d /c exit -1073741510").code, 0xC000_013A);
}

/// The bundled ConPTY holds output back for about 3 s until its startup
/// query is answered. The best of three runs keeps a busy machine from
/// failing this.
#[test]
fn pty_first_output_is_quick() {
    // The first run pays for loading the console host from disk.
    run("cmd /d /c echo hi");
    let best = (0..3)
        .map(|_| {
            let r = run("cmd /d /c echo hi");
            let (t, _) = (r.chunks.iter())
                .find(|(_, c)| find(c, b"hi").is_some())
                .expect("the child's output");
            t.duration_since(r.started)
        })
        .min()
        .expect("three runs");
    eprintln!("the child's output after {best:?}");
    assert!(
        best < Duration::from_secs(2),
        "the child's output after {best:?}"
    );
}

#[test]
fn pty_uses_bundled_conpty_when_configured() {
    let Some(dir) = std::env::var_os("BLITZ_CONPTY_DIR").map(PathBuf::from) else {
        eprintln!("SKIPPED: BLITZ_CONPTY_DIR is not set, so the bundled ConPTY is untested");
        return;
    };
    for f in ["conpty.dll", "OpenConsole.exe"] {
        assert!(dir.join(f).is_file(), "no {f} in {}", dir.display());
    }
    let r = run("cmd /d /c echo hi");
    let (_, first) = &r.chunks[0];
    assert!(
        first.starts_with(b"\x1b[1t"),
        "{:?}",
        String::from_utf8_lossy(first)
    );
    assert_eq!(blitz::pty::inbox_notice(), None);
}

#[test]
fn pty_close_ends_an_interactive_shell() {
    let (pty, _, rx) = spawn("cmd /d", &[]);
    let mut out = Vec::new();
    assert!(wait_for(&rx, &mut out, b">"), "no prompt");
    pty.resize(100, 30);
    pty.writer().send(&b"mode con\r"[..]);
    assert!(
        wait_for(&rx, &mut out, b"Columns:"),
        "{}",
        String::from_utf8_lossy(&out)
    );
    let screen = text(&out);
    let size = |what: &str| {
        let line = screen.lines().find(|l| l.trim_start().starts_with(what));
        line.and_then(|l| l.split_whitespace().last()?.parse::<u32>().ok())
    };
    assert_eq!(
        (size("Columns:"), size("Lines:")),
        (Some(100), Some(30)),
        "{screen}"
    );
    drop(pty);
    let deadline = Instant::now() + Duration::from_secs(10);
    let code = loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx
            .recv_timeout(left)
            .expect("no exit within 10 s of closing")
        {
            Ok(_) => continue,
            Err(code) => break code,
        }
    };
    // Closing the console ends cmd as Ctrl+C would.
    assert_eq!(code, 0xC000_013A);
}

/// Clearing the console host's buffer draws its screen again without the
/// cleared text, so the pane's screen follows.
#[test]
fn pty_clear_draws_the_screen_without_the_old_text() {
    let (pty, _, rx) = spawn("cmd /d", &[]);
    let mut out = Vec::new();
    assert!(wait_for(&rx, &mut out, b">"), "no prompt");
    pty.writer().send(&b"echo marker-%OS%\r"[..]);
    assert!(wait_for(&rx, &mut out, b"marker-Windows_NT"), "no echo");
    if !pty.clear() {
        let bundled = std::env::var_os("BLITZ_CONPTY_DIR").is_some();
        assert!(!bundled, "the bundled ConPTY can clear");
        eprintln!("SKIPPED: the system's ConPTY cannot clear its buffer");
        return;
    }
    let mut t = vt::Terminal::new(vt::Options::default());
    t.feed(&out);
    let deadline = Instant::now() + Duration::from_secs(10);
    while t.screen_text().contains("marker-Windows_NT") {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(Ok((_, chunk))) => t.feed(&chunk),
            _ => panic!("still on screen:\n{}", t.screen_text()),
        }
    }
}

/// A flood of output neither stalls the pane nor loses its end, and a pane
/// whose child has gone takes input, resizes and closes without fuss.
#[test]
fn pty_takes_a_burst_and_calls_after_exit() {
    let r = run(r#"cmd /d /c "for /L %i in (1,1,20000) do @echo line %i""#);
    assert_eq!(r.code, 0);
    let screen = text(&r.output());
    assert!(
        screen.contains("line 20000"),
        "{}",
        &screen[screen.len().saturating_sub(300)..]
    );

    let (pty, _, rx) = spawn("cmd /d /c exit 0", &[]);
    while let Ok(Ok(_)) = rx.recv_timeout(Duration::from_secs(30)) {}
    pty.writer().send(&b"late\r"[..]);
    pty.resize(0, 0);
    pty.resize(0, 30);
    pty.resize(u16::MAX, u16::MAX);
    pty.close();
    pty.close();
    drop(pty);
}

/// Claude Code prints what blitz-hook returns, an OSC 777 notification as
/// long as a real pane token and session id make it, and the console host
/// must pass it on untouched.
#[test]
fn pty_hook_sequences_reach_the_reader() {
    let seq = format!(
        "\x1b]777;notify;blitz:{TOKEN}:needs-you:3f2a0c1e-0000-4000-8000-00000000abcd;Claude needs you\x07"
    );
    let ps = seq
        .replace('\x1b', "'+[char]27+'")
        .replace('\x07', "'+[char]7+'");
    let r = run(&format!(
        "powershell.exe -NoProfile -NonInteractive -Command \"[Console]::Write('{ps}')\""
    ));
    let out = r.output();
    assert!(
        find(&out, seq.as_bytes()).is_some(),
        "{:?}",
        String::from_utf8_lossy(&out)
    );
    assert_eq!(r.code, 0);
}

/// Other agents' hooks run `blitz-hook notify`, which writes to the pane's
/// console, not to the stdout the agent reads (here thrown away), and the
/// console host passes it on.
#[test]
fn pty_notify_reaches_the_reader() {
    let hook = env!("CARGO_BIN_EXE_blitz-hook");
    let cmd =
        format!("cmd /d /c \"\"{hook}\" notify needs-you \"Approve r\u{e9}sum\u{e9}\" > NUL\"");
    let env = [("BLITZ_PANE_TOKEN".to_owned(), TOKEN.to_owned())];
    let (_pty, _, rx) = spawn(&cmd, &env);
    let seq = format!("\x1b]777;notify;blitz:{TOKEN}:needs-you:v2;Approve r\u{e9}sum\u{e9}\x07");
    let mut out = Vec::new();
    assert!(
        wait_for(&rx, &mut out, seq.as_bytes()),
        "{:?}",
        String::from_utf8_lossy(&out)
    );
}

/// Each shell blitz integrates with that is on this machine, without the
/// user's profile, which could replace the prompt.
fn shells() -> Vec<String> {
    let root = PathBuf::from(std::env::var_os("SystemRoot").expect("SystemRoot"));
    let mut v = vec![
        root.join("System32").join("cmd.exe"),
        root.join("System32")
            .join("WindowsPowerShell")
            .join("v1.0")
            .join("powershell.exe"),
    ];
    let pwsh = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|d| d.join("pwsh.exe"))
        .find(|p| p.is_absolute() && p.is_file());
    match pwsh {
        Some(p) => v.push(p),
        None => eprintln!("SKIPPED: pwsh.exe is not on PATH, so PowerShell 7 is untested"),
    }
    v.into_iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect()
}

fn launch(program: &str) -> blitz::shell::Launch {
    let mut l = blitz::shell::launch(program, true, TOKEN);
    l.cmdline = l.cmdline.replacen(" -NoLogo", " -NoProfile -NoLogo", 1);
    l.env.push(("BLITZ_PANE_TOKEN".into(), TOKEN.into()));
    l
}

#[test]
fn pty_shells_print_prompt_marks() {
    let mark = format!("\x1b]133;A;blitz={TOKEN}");
    for program in shells() {
        let launch = launch(&program);
        let (_pty, _, rx) = spawn(&launch.cmdline, &launch.env);
        let mut out = Vec::new();
        assert!(
            wait_for(&rx, &mut out, mark.as_bytes()),
            "{program}: no prompt mark in {:?}",
            String::from_utf8_lossy(&out)
        );
        if !program.ends_with("cmd.exe") {
            assert!(
                wait_for(&rx, &mut out, b"\x1b]7;file:///"),
                "{program}: no folder in {:?}",
                String::from_utf8_lossy(&out)
            );
        }
    }
}

/// cmd shows a prompt the user set, between blitz's marks.
#[test]
fn pty_cmd_keeps_the_users_prompt() {
    let env = [
        ("PROMPT", blitz::shell::cmd_prompt(TOKEN, "mine$G")),
        ("BLITZ_PANE_TOKEN", TOKEN.into()),
    ]
    .map(|(k, v)| (k.to_string(), v));
    let (_pty, _, rx) = spawn("cmd.exe", &env);
    let mut out = Vec::new();
    let mark = format!("\x1b]133;A;blitz={TOKEN}");
    let shown =
        wait_for(&rx, &mut out, mark.as_bytes()) && wait_for(&rx, &mut out, b"mine>\x1b]133;B");
    assert!(shown, "{:?}", String::from_utf8_lossy(&out));
}

/// PowerShell marks where each command starts, and ends it with the code
/// of the program it ran, even the same as last time, or 1 for a failed
/// cmdlet, which leaves the last program's code behind in `$LASTEXITCODE`.
/// It does so too when the user's profile turns strict mode on, and the
/// script runs as written under each PowerShell.
#[test]
fn pty_powershell_reports_how_commands_end() {
    let shells = shells().into_iter().filter(|p| !p.ends_with("cmd.exe"));
    for (program, strict) in shells.flat_map(|p| [(p.clone(), false), (p, true)]) {
        let mut l = launch(&program);
        if strict {
            let script = format!(
                "Set-StrictMode -Version Latest\n{}",
                blitz::shell::POWERSHELL_INTEGRATION
            );
            let integration = blitz::shell::quote(blitz::shell::POWERSHELL_INTEGRATION);
            l.cmdline = l
                .cmdline
                .replace(&integration, &blitz::shell::quote(&script));
            assert!(l.cmdline.contains("Set-StrictMode"), "{}", l.cmdline);
        }
        let (pty, _, rx) = spawn(&l.cmdline, &l.env);
        let mut out = Vec::new();
        let mark = format!("\x1b]133;A;blitz={TOKEN}");
        assert!(
            wait_for(&rx, &mut out, mark.as_bytes()),
            "{program}: no prompt in {:?}",
            String::from_utf8_lossy(&out)
        );
        for (line, code) in [
            ("cmd /c exit 3", "3"),
            ("cmd /c exit 3", "3"),
            ("Get-Item blitz-nothing-here", "1"),
            ("cmd /c exit 3", "3"),
            (
                "Get-Item blitz-nothing-here -ErrorAction SilentlyContinue",
                "1",
            ),
            ("cmd /c exit 0", "0"),
        ] {
            out.clear();
            pty.writer().send(format!("{line}\r").into_bytes());
            let end = format!("\x1b]133;D;{code}\x07");
            let ended = wait_for(&rx, &mut out, end.as_bytes());
            let shown = String::from_utf8_lossy(&out);
            assert!(ended, "{program}, strict mode {strict}: {line}: {shown:?}");
            assert!(
                shown.contains("\x1b]133;C\x07"),
                "{program}: {line}: {shown:?}"
            );
        }
    }
}

/// A prompt defined after blitz's, as oh-my-posh or posh-git can, is
/// marked again from the next command on: once, though it calls the one
/// it replaced as conda's does, and still told when a command failed.
#[test]
fn pty_powershell_marks_a_prompt_defined_later() {
    let mark = format!("\x1b]133;A;blitz={TOKEN}");
    for program in shells().iter().filter(|p| !p.ends_with("cmd.exe")) {
        let l = launch(program);
        let (pty, _, rx) = spawn(&l.cmdline, &l.env);
        let mut out = Vec::new();
        // Each step looks only at the output that came after it.
        let mut step = |typed: &str, shown: &str| {
            pty.writer().send(typed.as_bytes());
            let mut new = Vec::new();
            let seen = wait_for(&rx, &mut new, shown.as_bytes());
            out.extend(new);
            seen
        };
        let ok = step("", &mark)
            && step(
                "function global:prompt { if ($global:?) { 'ok> ' } else { 'no> ' } }\r",
                "ok> ",
            )
            && step("\r", "ok> \x1b]133;B\x07")
            && step("Get-Item nope-4b1d\r", "no> \x1b]133;B\x07")
            && step(
                "$old = $function:prompt; function global:prompt { 'W' + (& $old) }\r",
                "ok> \x1b]133;B\x07",
            )
            && step("\r", "Wok> \x1b]133;B\x07");
        assert!(ok, "{program}: {:?}", String::from_utf8_lossy(&out));
    }
}

/// A shell started in a `\\?\` folder still marks its prompt and reports
/// the folder.
#[test]
fn pty_powershell_reports_a_verbatim_folder() {
    let dir = std::env::temp_dir().join(format!("blitz-verbatim-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let verbatim = PathBuf::from(format!(r"\\?\{}", dir.display()));
    let l = launch(&shells()[1]);
    let (pty, _, rx) = spawn_in(&l.cmdline, &l.env, Some(&verbatim));
    let mut out = Vec::new();
    let mark = format!("\x1b]133;A;blitz={TOKEN}");
    let marked = wait_for(&rx, &mut out, mark.as_bytes());
    let reported = wait_for(&rx, &mut out, b"\x1b]7;file:///");
    drop(pty);
    let _ = std::fs::remove_dir_all(&dir);
    let shown = String::from_utf8_lossy(&out);
    assert!(marked && reported, "{shown:?}");
    let url = shown
        .split("\x1b]7;")
        .nth(1)
        .and_then(|s| s.split('\x07').next());
    let name = dir
        .file_name()
        .expect("name")
        .to_string_lossy()
        .into_owned();
    assert!(url.is_some_and(|u| u.ends_with(&name)), "{url:?}");
}
