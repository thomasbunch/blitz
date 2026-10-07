//! Runs `blitz debug run` scripts against real processes.
#![cfg(windows)]

use std::path::PathBuf;
use std::time::{Duration, Instant};

fn file(name: &str, body: &str) -> String {
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::write(&path, body).expect("write script");
    path.to_string_lossy().into_owned()
}

fn run(args: &[&str]) -> i32 {
    let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    blitz::debug::run(&args)
}

/// Runs `script` against `cmd` with a trace; returns the exit code and the
/// trace.
fn traced(name: &str, cmd: &str, script: &str, extra: &[&str]) -> (i32, String) {
    let script = file(&format!("{name}.txt"), script);
    let trace = file(&format!("{name}.log"), "");
    let mut args = vec!["--cmd", cmd, "--script", &script, "--trace", &trace];
    args.extend_from_slice(extra);
    let code = run(&args);
    (code, std::fs::read_to_string(&trace).expect("trace"))
}

/// The output in a trace, joined back up: it is logged one chunk per line.
fn output(trace: &str) -> String {
    trace
        .lines()
        .filter_map(|l| l.split_once("\tout\t").map(|(_, text)| text))
        .collect()
}

#[test]
fn debug_run_waits_for_output_and_exit() {
    let (code, log) = traced(
        "debug-echo",
        "cmd /c echo hi",
        "waitfor 10000 hi\nwaitexit 10000\n",
        &[],
    );
    assert_eq!(code, 0, "{log}");
    // Output is logged escaped.
    assert!(output(&log).contains("hi\\r\\n"), "{log}");
    assert!(log.contains("\tnote\twaitfor ok: hi"), "{log}");
    assert!(log.contains("\tnote\texited with code 0"), "{log}");
}

/// Each `waitfor` looks after the previous match, not after all output
/// seen when it matched: both words come in one chunk here.
#[test]
fn debug_run_waits_for_each_text_in_turn() {
    let (code, log) = traced(
        "debug-two",
        "cmd /c echo foo bar",
        "waitfor 10000 foo\nwaitfor 5000 bar\nwaitexit 10000\n",
        &[],
    );
    assert_eq!(code, 0, "{log}");
    assert!(log.contains("\tnote\twaitfor ok: bar"), "{log}");
}

#[test]
fn debug_run_fails_when_text_never_shows() {
    let (code, log) = traced(
        "debug-missing",
        "cmd /c echo hi",
        "waitfor 10000 no such text\n",
        &[],
    );
    assert_eq!(code, 1);
    assert!(
        log.contains("FAILED: line 1: \"no such text\" did not show up before the program exited"),
        "{log}"
    );
}

#[test]
fn debug_run_rejects_bad_input() {
    assert_eq!(run(&[]), 2);
    assert_eq!(run(&["--script", "no-such-file.txt"]), 2);
    let script = file("debug-opts.txt", "note x\n");
    assert_eq!(run(&["--script", &script, "--bogus", "1"]), 2);
    assert_eq!(run(&["--script", &script, "--timeout", "soon"]), 2);
    let (code, log) = traced("debug-bad", "cmd /c echo hi", "frobnicate\n", &[]);
    assert_eq!(code, 1);
    assert!(
        log.contains("FAILED: line 1: unknown command \"frobnicate\""),
        "{log}"
    );
    let (code, log) = traced("debug-resize0", "cmd /c echo hi", "resize 0 5\n", &[]);
    assert_eq!(code, 1);
    assert!(log.contains("FAILED: line 1: "), "{log}");
}

/// A run past `--timeout` fails with a reason and returns, rather than
/// ending the process it runs in.
#[test]
fn debug_run_times_out() {
    let t = Instant::now();
    let (code, log) = traced(
        "debug-timeout",
        "cmd /d /k",
        "sleep 60000\n",
        &["--timeout", "1000"],
    );
    assert_eq!(code, 1, "{log}");
    assert!(
        log.contains("FAILED: line 1: timed out after 1000 ms"),
        "{log}"
    );
    assert!(t.elapsed() < Duration::from_secs(30), "{:?}", t.elapsed());
}

/// Every step against a live cmd prompt.
#[test]
fn debug_run_steps() {
    let script = r"waitfor 10000 blitz>
modes
type echo one\r
waitfor 5000 one
send echo two\r
waitfor 5000 two
expect 5000 ^two$
save row
note row is ${row}
paste echo three\r
waitfor 5000 three
mouse click 1 1
resize 100 30
snap s
history h
latency 2 x
key esc
idle 200 3000
send exit\r
waitexit 10000
";
    let (code, log) = traced("debug-steps", "cmd /d /k prompt blitz$G", script, &[]);
    assert_eq!(code, 0, "{log}");
    for note in [
        "modes InputModes",
        "waitfor ok: one",
        "expect ok: ^two$",
        "save row = ",
        "note\trow is ",
        "waitfor ok: three",
        "mouse click 1 1: nothing, mouse reporting is off",
        "resize 100x30",
        "snap s",
        "history h",
        "latency n=2",
        "key esc: \\e",
        "idle after",
        "exited with code 0",
    ] {
        assert!(log.contains(note), "{note:?} in\n{log}");
    }
}
