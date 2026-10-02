//! Runs the real `blitz-hook` binary the way Claude Code does: payload on
//! stdin, `terminalSequence` JSON on stdout.

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const HOOK: &str = env!("CARGO_BIN_EXE_blitz-hook");

/// The pane token the tests run the hook with.
const TOKEN: &str = "5eed";

/// Runs `blitz-hook claude` and returns (stdout, exit code).
fn hook(payload: &str, token: Option<&str>) -> (String, i32) {
    let mut cmd = Command::new(HOOK);
    cmd.arg("claude")
        .env("BLITZ_PANE_ID", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    match token {
        Some(t) => cmd.env("BLITZ_PANE_TOKEN", t),
        None => cmd.env_remove("BLITZ_PANE_TOKEN"),
    };
    let mut child = cmd.spawn().expect("spawn blitz-hook");
    // An inert hook may exit without reading; a broken pipe is fine then.
    let _ = child.stdin.take().unwrap().write_all(payload.as_bytes());
    let out = child.wait_with_output().expect("wait for blitz-hook");
    assert!(
        out.stderr.is_empty(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).expect("stdout is UTF-8");
    (stdout, out.status.code().unwrap_or(-1))
}

fn notify(state: &str, msg: &str) -> String {
    format!("{{\"terminalSequence\":\"\\u001b]777;notify;blitz:{TOKEN}:{state};{msg}\\u0007\"}}\n")
}

#[test]
fn hook_prints_each_event() {
    let cases = [
        (
            r#"{"hook_event_name":"UserPromptSubmit","prompt":"hi"}"#,
            notify("working", ""),
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
