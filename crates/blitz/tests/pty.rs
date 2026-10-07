//! Spawns real processes on a pseudoconsole. Set `BLITZ_CONPTY_DIR` to a
//! folder with `conpty.dll` and `OpenConsole.exe` to test the bundled
//! ConPTY; otherwise the system's is used.
#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::sync::{Once, mpsc};
use std::time::{Duration, Instant};

use blitz::pty::{Pty, PtyEvent, SpawnOpts};

/// What the console host writes before any output of the child.
const PRELUDE: [&[u8]; 4] = [b"\x1b[1t", b"\x1b[c", b"\x1b[?1004h", b"\x1b[?9001h"];

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// True if `chunk` holds anything besides the console host's prelude.
fn past_prelude(chunk: &[u8]) -> bool {
    let mut rest = chunk.to_vec();
    for p in PRELUDE {
        while let Some(i) = find(&rest, p) {
            rest.drain(i..i + p.len());
        }
    }
    !rest.is_empty()
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
    // A failing test must not leave console hosts running.
    static JOB: Once = Once::new();
    JOB.call_once(|| blitz::pty::kill_children_on_exit().expect("job object"));
    let (tx, rx) = mpsc::channel();
    let opts = SpawnOpts {
        cmdline,
        env,
        cols: 80,
        rows: 24,
        pane_id: 1,
        ..Default::default()
    };
    let started = Instant::now();
    let pty = Pty::spawn(&opts, move |ev, w| match ev {
        PtyEvent::Data(d) => {
            if find(d, b"[c").is_some() {
                w.send(&b"[?62;22c"[..]);
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
    let r = run("cmd /c echo hi");
    let out = r.output();
    assert!(
        find(&out, b"hi").is_some(),
        "{}",
        String::from_utf8_lossy(&out)
    );
    assert_eq!(r.code, 0);
}

#[test]
fn pty_first_output_is_quick() {
    // The first run pays for loading the console host from disk.
    run("cmd /c echo hi");
    let r = run("cmd /c echo hi");
    let (t, _) = r
        .chunks
        .iter()
        .find(|(_, c)| past_prelude(c))
        .expect("output past the prelude");
    let ms = t.duration_since(r.started).as_secs_f64() * 1000.0;
    eprintln!("first post-prelude byte after {ms:.1} ms");
    assert!(ms < 2000.0, "first post-prelude byte after {ms:.1} ms");
}

#[test]
fn pty_uses_bundled_conpty_when_configured() {
    if std::env::var_os("BLITZ_CONPTY_DIR").is_none() {
        return;
    }
    let r = run("cmd /c echo hi");
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
    let (pty, _, rx) = spawn("cmd", &[]);
    rx.recv_timeout(Duration::from_secs(30))
        .expect("output")
        .expect("cmd exited by itself");
    pty.resize(100, 30);
    drop(pty);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx
            .recv_timeout(left)
            .expect("no exit within 10 s of closing")
        {
            Ok(_) => continue,
            Err(_) => break,
        }
    }
}

#[test]
fn pty_shells_print_prompt_marks() {
    // "" is the detected default shell, PowerShell on a stock install.
    for program in ["cmd.exe", ""] {
        if program == "cmd.exe" && std::env::var_os("PROMPT").is_some() {
            continue; // a user's own PROMPT is left alone
        }
        let launch = blitz::shell::launch(program, &[], true, "5eed");
        let mut env = launch.env.clone();
        env.push(("BLITZ_PANE_TOKEN".into(), "5eed".into()));
        let (_pty, _, rx) = spawn(&launch.cmdline, &env);
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut out = Vec::new();
        while find(&out, b"]133;A;blitz=5eed").is_none() {
            let left = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(left) {
                Ok(Ok((_, chunk))) => out.extend(chunk),
                _ => panic!(
                    "{:?}: no prompt mark in {:?}",
                    launch.cmdline.get(..40),
                    String::from_utf8_lossy(&out)
                ),
            }
        }
    }
}
