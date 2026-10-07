//! Runs the real `blitz-hook` binary the way Claude Code does: payload on
//! stdin, `terminalSequence` JSON on stdout.

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use blitz::attention::Ev;
use blitz::hook::Json;

const HOOK: &str = env!("CARGO_BIN_EXE_blitz-hook");

/// The pane token the tests run the hook with, as long as a real one: 128
/// bits in hex.
const TOKEN: &str = "5eed0123456789abcdef0123456789ab";

/// A Claude Code session id, which every hook payload carries.
const SESSION: &str = "0b8f6a3e-1c2d-4e5f-9a7b-3c4d5e6f7a8b";

/// Runs `blitz-hook claude` and returns (stdout, exit code). Like Claude
/// Code, it writes the whole payload, which must not fail.
fn hook(payload: impl AsRef<[u8]>, token: Option<&str>) -> (String, i32) {
    let (out, code, wrote) = run_hook(payload.as_ref(), token);
    if let Err(e) = wrote {
        panic!("writing the payload: {e}");
    }
    (out, code)
}

/// Runs `blitz-hook claude`: (stdout, exit code, how writing stdin went).
fn run_hook(payload: &[u8], token: Option<&str>) -> (String, i32, std::io::Result<()>) {
    let mut cmd = Command::new(HOOK);
    cmd.arg("claude")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    match token {
        Some(t) => cmd.env("BLITZ_PANE_TOKEN", t),
        None => cmd.env_remove("BLITZ_PANE_TOKEN"),
    };
    let mut child = cmd.spawn().expect("spawn blitz-hook");
    let wrote = child.stdin.take().unwrap().write_all(payload);
    let out = child.wait_with_output().expect("wait for blitz-hook");
    assert!(
        out.stderr.is_empty(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).expect("stdout is UTF-8");
    (stdout, out.status.code().unwrap_or(-1), wrote)
}

fn notify(state: &str, msg: &str) -> String {
    format!("{{\"terminalSequence\":\"\\u001b]777;notify;blitz:{TOKEN}:{state};{msg}\\u0007\"}}\n")
}

#[test]
fn hook_prints_each_event() {
    let cases = [
        (
            r#"{"hook_event_name":"UserPromptSubmit","prompt":"hi"}"#,
            notify("working", "hi"),
        ),
        // A prompt of several lines shows as one.
        (
            r#"{"hook_event_name":"UserPromptSubmit","prompt":"fix the\n\nlogin  bug\u001b[2J"}"#,
            notify("working", "fix the login bug[2J"),
        ),
        (
            r#"{"hook_event_name":"PermissionRequest","tool_name":"Bash","tool_input":{"command":"cargo test"}}"#,
            notify("needs-you", "Bash: cargo test"),
        ),
        (
            r#"{"hook_event_name":"PermissionRequest","tool_name":"Edit","tool_input":{"file_path":"C:\\src\\main.rs"}}"#,
            notify("needs-you", "Edit: C:\\\\src\\\\main.rs"),
        ),
        (
            r#"{"hook_event_name":"PreToolUse","tool_name":"AskUserQuestion","tool_input":{"questions":[{"question":"Ship it?"}]}}"#,
            notify("needs-you", "Ship it?"),
        ),
        (
            r#"{"hook_event_name":"PreToolUse","tool_name":"ExitPlanMode","tool_input":{}}"#,
            notify("needs-you", "Plan ready"),
        ),
        (
            r#"{"hook_event_name":"Notification","notification_type":"permission_prompt","message":"Claude needs your permission to use Bash"}"#,
            notify("needs-you", "Claude needs your permission to use Bash"),
        ),
        (
            r#"{"hook_event_name":"Notification","notification_type":"elicitation_dialog","message":"Fill in the form"}"#,
            notify("needs-you", "Fill in the form"),
        ),
        (
            r#"{"hook_event_name":"Stop","stop_hook_active":false,"last_assistant_message":"Fixed the \"flaky\" test.\n\nDetails follow."}"#,
            notify("done", "Fixed the \\\"flaky\\\" test."),
        ),
        (
            r#"{"hook_event_name":"Stop","background_tasks":[{"id":"bash_1"}]}"#,
            notify("working", ""),
        ),
        (
            r#"{"hook_event_name":"StopFailure","error":"overloaded"}"#,
            notify("error", "overloaded"),
        ),
        (
            r#"{"hook_event_name":"SessionEnd","reason":"prompt_input_exit"}"#,
            notify("idle", ""),
        ),
        // Events blitz does not report print nothing.
        (
            r#"{"hook_event_name":"Stop","stop_hook_active":true}"#,
            String::new(),
        ),
        (
            r#"{"hook_event_name":"Notification","notification_type":"idle_prompt","message":"x"}"#,
            String::new(),
        ),
        (
            r#"{"hook_event_name":"PreToolUse","tool_name":"Read"}"#,
            String::new(),
        ),
        (r#"{"hook_event_name":"SubagentStop"}"#, String::new()),
        ("not json at all", String::new()),
        ("", String::new()),
    ];
    for (payload, want) in cases {
        assert_eq!(hook(payload, Some(TOKEN)), (want, 0), "{payload}");
    }
}

/// The attention event and session a pane gets from the hook's output,
/// read the way the app reads it: the `terminalSequence` Claude Code
/// writes, through the terminal, then the title's state.
fn in_pane(stdout: &str) -> Vec<(Ev, Option<String>)> {
    let seq = Json::parse(stdout)
        .expect("one JSON line")
        .get("terminalSequence")
        .and_then(Json::as_str)
        .expect("a terminalSequence")
        .to_owned();
    let mut t = vt::Terminal::new(vt::Options::default());
    t.feed(seq.as_bytes());
    let mut evs = Vec::new();
    t.take_events(&mut evs);
    evs.into_iter()
        .filter_map(|e| match e {
            vt::Event::Notify { title, .. } => {
                Ev::from_notify(&title, TOKEN).map(|(ev, id)| (ev, id.map(str::to_owned)))
            }
            _ => None,
        })
        .collect()
}

/// Every state, with the session id every real payload carries, reaches
/// the pane whole.
#[test]
fn hook_states_reach_the_pane_with_their_session() {
    let cases = [
        ("UserPromptSubmit", "", Ev::Working),
        (
            "PermissionRequest",
            r#","tool_name":"Bash","tool_input":{"command":"git push"}"#,
            Ev::NeedsYou,
        ),
        (
            "Notification",
            r#","notification_type":"permission_prompt","message":"m""#,
            Ev::NeedsYou,
        ),
        ("Stop", r#","last_assistant_message":"ok""#, Ev::Done),
        (
            "StopFailure",
            r#","error":"overloaded""#,
            Ev::Error { sticky: false },
        ),
        ("SessionEnd", "", Ev::Idle),
    ];
    for (event, rest, want) in cases {
        let payload = format!(r#"{{"hook_event_name":"{event}","session_id":"{SESSION}"{rest}}}"#);
        let (out, code) = hook(&payload, Some(TOKEN));
        assert_eq!(code, 0, "{event}");
        assert_eq!(
            in_pane(&out),
            [(want, Some(SESSION.to_owned()))],
            "{event}: {out}"
        );
    }
}

#[test]
fn hook_keeps_control_characters_out() {
    let payload = r#"{"hook_event_name":"Notification","notification_type":"permission_prompt","message":"a\u001b]0;x\u0007b\u009c\r\nc"}"#;
    assert_eq!(
        hook(payload, Some(TOKEN)),
        (notify("needs-you", "a]0;xb c"), 0)
    );
}

#[test]
fn hook_is_inert_outside_a_pane() {
    let payload = r#"{"hook_event_name":"UserPromptSubmit"}"#;
    assert_eq!(hook(payload, None), (String::new(), 0));
    assert_eq!(hook(payload, Some("")), (String::new(), 0));
    // A token that could end the sequence early is not used.
    assert_eq!(hook(payload, Some("a;b")), (String::new(), 0));
    assert_eq!(hook(payload, Some("a\x07b")), (String::new(), 0));
    // Claude Code in another terminal still writes its whole payload,
    // more than a pipe holds; the hook reads it rather than breaking the
    // pipe under the writer.
    let big = format!(
        r#"{{"hook_event_name":"Stop","last_assistant_message":"{}"}}"#,
        "x".repeat(1 << 20)
    );
    assert_eq!(hook(&big, None), (String::new(), 0));
}

#[test]
fn hook_reads_payloads_that_are_not_utf8() {
    let payload = b"{\"hook_event_name\":\"Stop\",\"last_assistant_message\":\"a\xffb\"}";
    assert_eq!(
        hook(payload, Some(TOKEN)),
        (notify("done", "a\u{FFFD}b"), 0)
    );
}

/// A payload past the 64 MiB cap is cut, does not parse and reports
/// nothing; the hook still exits at once with 0.
#[test]
fn hook_drops_a_payload_over_the_cap() {
    // Past the cap by more than a pipe buffer holds.
    let reply = "x".repeat(65 << 20);
    let payload = format!(r#"{{"hook_event_name":"Stop","last_assistant_message":"{reply}"}}"#);
    let (out, code, wrote) = run_hook(payload.as_bytes(), Some(TOKEN));
    assert_eq!((out, code), (String::new(), 0));
    // All of it is read, so Claude Code's write does not fail.
    wrote.expect("the whole payload is taken");
}

#[test]
fn hook_ignores_other_subcommands() {
    let out = Command::new(HOOK)
        .arg("codex")
        .env("BLITZ_PANE_TOKEN", TOKEN)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty());
}

#[test]
fn hook_handles_a_large_payload() {
    let reply = "word ".repeat(100_000);
    let payload = format!(r#"{{"hook_event_name":"Stop","last_assistant_message":"{reply}"}}"#);
    let (out, code) = hook(&payload, Some(TOKEN));
    assert_eq!(code, 0);
    let msg = "word ".repeat(23) + "word…";
    assert_eq!(out, notify("done", &msg));
}

#[test]
fn hook_reports_a_request_to_write_a_large_file() {
    let content = "x".repeat(3 << 20);
    let payload = format!(
        r#"{{"hook_event_name":"PermissionRequest","tool_name":"Write","tool_input":{{"file_path":"big.txt","content":"{content}"}}}}"#
    );
    assert_eq!(
        hook(&payload, Some(TOKEN)),
        (notify("needs-you", "Write: big.txt"), 0)
    );
}

#[test]
fn hook_runs_fast() {
    let payload = r#"{"hook_event_name":"UserPromptSubmit"}"#;
    // The first spawn pays for the virus scan and a cold disk cache.
    hook(payload, Some(TOKEN));
    let mut times: Vec<Duration> = (0..5)
        .map(|_| {
            let t = Instant::now();
            hook(payload, Some(TOKEN));
            t.elapsed()
        })
        .collect();
    times.sort();
    println!(
        "blitz-hook runtime: median {:?}, max {:?}",
        times[2], times[4]
    );
    assert!(times[4] < Duration::from_millis(1000), "{times:?}");
}
