//! Runs `blitz debug run` scripts against real processes.
#![cfg(windows)]

use std::path::PathBuf;

fn file(name: &str, body: &str) -> String {
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::write(&path, body).expect("write script");
    path.to_string_lossy().into_owned()
}

fn run(args: &[&str]) -> i32 {
    let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    blitz::debug::run(&args)
}

#[test]
fn debug_run_waits_for_output_and_exit() {
    let script = file("debug-echo.txt", "waitfor 10000 hi\nwaitexit 10000\n");
    let trace = file("debug-echo.log", "");
    let code = run(&[
        "--cmd",
        "cmd /c echo hi",
        "--script",
        &script,
        "--trace",
        &trace,
    ]);
    assert_eq!(code, 0);
    let log = std::fs::read_to_string(&trace).expect("trace");
    // Output is logged escaped, one chunk per line.
    assert!(log.contains("\tout\t") && log.contains("hi\\r\\n"), "{log}");
    assert!(log.contains("\tnote\twaitfor ok: hi"), "{log}");
}

#[test]
fn debug_run_fails_when_text_never_shows() {
    let script = file("debug-missing.txt", "waitfor 10000 no such text\n");
    assert_eq!(run(&["--cmd", "cmd /c echo hi", "--script", &script]), 1);
}

#[test]
fn debug_run_rejects_bad_input() {
    assert_eq!(run(&[]), 2);
    assert_eq!(run(&["--script", "no-such-file.txt"]), 2);
    let script = file("debug-bad.txt", "frobnicate\n");
    assert_eq!(run(&["--cmd", "cmd /c echo hi", "--script", &script]), 1);
}
