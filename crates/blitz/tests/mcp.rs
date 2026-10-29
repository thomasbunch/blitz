//! `blitz-hook mcp` as Claude Code runs it: requests on stdin, a reply a
//! line on stdout, and tool calls to a stand-in for blitz.

use std::io::Write;
use std::process::{Command, Stdio};

use blitz::control::{Op, error, parse};
use blitz::hook::Json;

const TOKEN: &str = "0123456789abcdef0123456789abcdef";

/// The replies `blitz-hook mcp` writes for `lines`, run with `token` as
/// its pane's, once its stdin closes.
fn serve(pipe: &str, token: &str, lines: &[&str]) -> Vec<Json> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_blitz-hook"))
        .arg("mcp")
        .env("BLITZ_PIPE", pipe)
        .env("BLITZ_PANE_TOKEN", token)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("run blitz-hook mcp");
    let mut stdin = child.stdin.take().expect("stdin");
    for l in lines {
        writeln!(stdin, "{l}").expect("write a request");
    }
    drop(stdin);
    let out = child.wait_with_output().expect("blitz-hook mcp exits");
    assert_eq!(out.status.code(), Some(0));
    (String::from_utf8(out.stdout).expect("UTF-8").lines())
        .map(|l| Json::parse(l).expect(l))
        .collect()
}

fn result(reply: &Json) -> &Json {
    reply.get("result").expect("a result")
}

/// A tool call's text, and whether it is an error.
fn said(reply: &Json) -> (&str, bool) {
    let r = result(reply);
    let Some(Json::Arr(content)) = r.get("content") else {
        panic!("no content");
    };
    let text = content[0].get("text").and_then(Json::as_str).expect("text");
    (text, r.get("isError") == Some(&Json::Bool(true)))
}

#[test]
fn mcp_over_stdio() {
    let pipe = blitz::control::listen(|line| match parse(&line) {
        Ok((t, Op::List)) if t == TOKEN => concat!(
            r#"{"ok":true,"panes":[{"pane":8,"parent":3,"state":"working","exit":null,"#,
            r#""name":"npm test","cwd":"/x"}]}"#
        )
        .into(),
        Ok(_) => error("no pane 9 that this pane opened"),
        Err(why) => error(why),
    })
    .expect("listen");

    let replies = serve(
        &pipe,
        TOKEN,
        &[
            r#"{"jsonrpc":"2.0","id":1,"method":"server/discover"}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"0"}}}"#,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/list","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28"}}}"#,
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"list","arguments":{}}}"#,
            r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"close","arguments":{"pane":9}}}"#,
        ],
    );
    let ids: Vec<_> = replies.iter().map(|r| r.get("id").cloned()).collect();
    assert_eq!(ids, [1.0, 2.0, 3.0, 4.0, 5.0].map(|n| Some(Json::Num(n))));

    let discover = result(&replies[0]);
    assert_eq!(
        discover.get("supportedVersions"),
        Some(&Json::Arr(vec![
            Json::Str("2026-07-28".into()),
            Json::Str("2025-11-25".into())
        ]))
    );
    let init = result(&replies[1]);
    assert_eq!(
        init.get("protocolVersion").and_then(Json::as_str),
        Some("2025-11-25")
    );
    let Some(Json::Arr(tools)) = result(&replies[2]).get("tools") else {
        panic!("no tools");
    };
    let names: Vec<_> = (tools.iter())
        .map(|t| t.get("name").and_then(Json::as_str).unwrap_or(""))
        .collect();
    assert_eq!(names, ["list", "open", "send", "read", "close"]);
    assert_eq!(
        said(&replies[3]),
        ("pane 8: npm test, working, in /x (opened by pane 3)", false)
    );
    assert_eq!(said(&replies[4]), ("no pane 9 that this pane opened", true));

    // Claude Code can be set to scrub the token from what it runs.
    let replies = serve(
        &pipe,
        "",
        &[r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"list"}}"#],
    );
    let (text, failed) = said(&replies[0]);
    assert!(
        failed && text.contains("CLAUDE_CODE_SUBPROCESS_ENV_SCRUB"),
        "{text}"
    );
}
