//! `blitz-hook mcp`: Claude Code's tools for the panes its session opens,
//! an MCP server on stdin and stdout. Each tool is a `blitz pane` op, sent
//! by [`super::call`] with the token of the pane Claude Code runs in, so
//! it reaches only the panes that pane opened. A message is one line of
//! JSON each way, and nothing else goes to stdout.

use std::fmt::Write as _;
use std::io::{self, BufRead, Write};

use crate::hook::{Json, escape_json};

/// The protocol revisions spoken, the newest first. `initialize` with any
/// other is offered the last.
const VERSIONS: [&str; 2] = ["2026-07-28", "2025-11-25"];

const SERVER: &str = concat!(
    r#"{"name":"blitz","version":""#,
    env!("CARGO_PKG_VERSION"),
    r#""}"#
);

/// The caching hints 2026-07-28 wants on `server/discover` and
/// `tools/list`: the same for every user, and for as long as this runs.
const FRESH: &str = r#""ttlMs":3600000,"cacheScope":"public""#;

const INSTRUCTIONS: &str = "Panes in blitz, the terminal this session runs in. open starts \
    a shell beside this pane or in a tab after it, typing a command if given, and returns \
    its ID; send types text and presses keys in it; read returns its last lines; list shows \
    the panes opened; close ends one. They reach only panes this session opened, never the \
    user's own, and send refuses a pane waiting for its user.";

/// What `tools/list` lists, in its order.
const TOOLS: &str = concat!(
    r#"[{"name":"list","description":"Lists the blitz panes this session opened, its agent "#,
    r#"teammates among them, and the panes those opened: ID, name, state and folder.","#,
    r#""inputSchema":{"type":"object","properties":{}},"annotations":{"readOnlyHint":true}},"#,
    r#"{"name":"open","description":"Opens a shell in a new blitz pane beside this session's "#,
    r#"pane, or in a tab after it, and returns its ID. The pane tools work only on panes this "#,
    r#"session opened.","inputSchema":{"type":"object","properties":{"#,
    r#""place":{"type":"string","enum":["right","down","tab"],"#,
    r#""description":"Beside this pane on the right, below it, or in a new tab"},"#,
    r#""cwd":{"type":"string","description":"Absolute folder to start in; this pane's unless given"},"#,
    r#""command":{"type":"string","description":"Typed at the shell's first prompt"},"#,
    r#""focus":{"type":"boolean","description":"Move the user's focus to it; blitz does only "#,
    r#"while the user is in this pane and not typing"}},"required":["place"]},"#,
    r#""annotations":{"destructiveHint":true}},"#,
    r#"{"name":"send","description":"Types text as a paste into a pane this session opened, "#,
    r#"then presses each key; put enter in keys to run what it typed. Refused for a pane "#,
    r#"waiting for its user. Works only on panes this session opened.","#,
    r#""inputSchema":{"type":"object","properties":{"#,
    r#""pane":{"type":"integer","description":"The pane's ID, from open or list"},"#,
    r#""text":{"type":"string","description":"Typed first, without its control characters"},"#,
    r#""keys":{"type":"array","items":{"type":"string"},"description":"Keys pressed after "#,
    r#"the text: enter, tab, esc, backspace, up, pgdn, home, f5, ctrl+c and the like"}},"#,
    r#""required":["pane"]},"annotations":{"destructiveHint":true}},"#,
    r#"{"name":"read","description":"Returns the last lines a pane this session opened shows, "#,
    r#"then its state or exit code. Works only on panes this session opened.","#,
    r#""inputSchema":{"type":"object","properties":{"#,
    r#""pane":{"type":"integer","description":"The pane's ID, from open or list"},"#,
    r#""lines":{"type":"integer","minimum":1,"maximum":2000,"description":"100 unless given"}},"#,
    r#""required":["pane"]},"annotations":{"readOnlyHint":true}},"#,
    r#"{"name":"close","description":"Closes a pane this session opened, ending what runs in "#,
    r#"it. Works only on panes this session opened.","#,
    r#""inputSchema":{"type":"object","properties":{"#,
    r#""pane":{"type":"integer","description":"The pane's ID, from open or list"}},"#,
    r#""required":["pane"]},"annotations":{"destructiveHint":true,"idempotentHint":true}}]"#
);

/// The arguments each tool passes on to the op of its name.
const ARGS: [(&str, &[&str]); 5] = [
    ("list", &[]),
    ("open", &["place", "cwd", "command", "focus"]),
    ("send", &["pane", "text", "keys"]),
    ("read", &["pane", "lines"]),
    ("close", &["pane"]),
];

/// What a request gets: its result's JSON, or an error's code, message
/// and data.
type Outcome = Result<String, (i32, &'static str, Option<String>)>;

/// Answers Claude Code until it closes stdin. Returns the exit code.
pub fn run() -> i32 {
    let mut out = io::stdout().lock();
    for line in io::stdin().lock().split(b'\n') {
        let Ok(line) = line else {
            break;
        };
        let line = String::from_utf8_lossy(&line);
        if line.trim().is_empty() {
            continue;
        }
        let Some(reply) = answer(&line, &super::call) else {
            continue;
        };
        if writeln!(out, "{reply}").and_then(|()| out.flush()).is_err() {
            break;
        }
    }
    0
}

/// The line that answers the message `line`, calling blitz with `call`;
/// `None` for a notification, which gets no answer, or for no request.
pub fn answer(line: &str, call: &dyn Fn(&str) -> Result<Json, String>) -> Option<String> {
    let Some(msg) = Json::parse(line) else {
        return Some(reply("null", Err((-32700, "Parse error", None))));
    };
    let (Some(method), Some(id)) = (msg.get("method").and_then(Json::as_str), msg.get("id")) else {
        return None;
    };
    // A number goes back as it came, as a double cannot hold every one;
    // anything else is written again, escaped.
    let id = match (id, Json::raw(line, "id")) {
        (Json::Num(_), Some(raw)) => raw.to_owned(),
        _ => {
            let mut text = String::new();
            id.write(&mut text);
            text
        }
    };
    let params = msg.get("params");
    let wanted = (params.and_then(|p| p.get("_meta")))
        .and_then(|m| m.get("io.modelcontextprotocol/protocolVersion"));
    let versions = format!("[\"{}\"]", VERSIONS.join("\",\""));
    let outcome = match (method, wanted) {
        (_, Some(v)) if !v.as_str().is_some_and(|v| VERSIONS.contains(&v)) => {
            let mut data = format!("{{\"supported\":{versions},\"requested\":");
            v.write(&mut data);
            Err((-32022, "Unsupported protocol version", Some(data + "}")))
        }
        ("server/discover", _) => Ok(format!(
            "{{\"resultType\":\"complete\",{FRESH},\"supportedVersions\":{versions},\
             \"capabilities\":{{\"tools\":{{}}}},\"instructions\":\"{INSTRUCTIONS}\",\
             \"_meta\":{{\"io.modelcontextprotocol/serverInfo\":{SERVER}}}}}"
        )),
        ("initialize", _) => {
            let asked = params.and_then(|p| p.get("protocolVersion"));
            let v = (VERSIONS.iter())
                .find(|v| asked.and_then(Json::as_str) == Some(v))
                .unwrap_or(&VERSIONS[1]);
            Ok(format!(
                "{{\"protocolVersion\":\"{v}\",\"capabilities\":{{\"tools\":{{}}}},\
                 \"serverInfo\":{SERVER},\"instructions\":\"{INSTRUCTIONS}\"}}"
            ))
        }
        ("ping", _) => Ok("{}".into()),
        ("tools/list", _) => Ok(format!(
            "{{\"resultType\":\"complete\",{FRESH},\"tools\":{TOOLS}}}"
        )),
        ("tools/call", _) => tool(params, call),
        _ => Err((-32601, "Method not found", None)),
    };
    Some(reply(&id, outcome))
}

/// The JSON-RPC line answering request `id` with `outcome`.
fn reply(id: &str, outcome: Outcome) -> String {
    match outcome {
        Ok(result) => format!("{{\"jsonrpc\":\"2.0\",\"id\":{id},\"result\":{result}}}"),
        Err((code, message, data)) => {
            let data = data.map_or_else(String::new, |d| format!(",\"data\":{d}"));
            format!(
                "{{\"jsonrpc\":\"2.0\",\"id\":{id},\"error\":\
                 {{\"code\":{code},\"message\":\"{message}\"{data}}}}}"
            )
        }
    }
}

/// `tools/call`: the tool's op, with the arguments it takes. What blitz
/// refuses, or no blitz to ask, is a result the model reads and acts on.
fn tool(params: Option<&Json>, call: &dyn Fn(&str) -> Result<Json, String>) -> Outcome {
    let Some(name) = params.and_then(|p| p.get("name")).and_then(Json::as_str) else {
        return Err((-32602, "Invalid params", None));
    };
    let Some(&(op, takes)) = ARGS.iter().find(|t| t.0 == name) else {
        return Err((-32602, "Unknown tool", None));
    };
    let args = match params.and_then(|p| p.get("arguments")) {
        None | Some(Json::Null) => &[][..],
        Some(Json::Obj(m)) => m,
        Some(_) => return Err((-32602, "Invalid params", None)),
    };
    let mut fields = format!("\"op\":\"{op}\"");
    // Only the tool's own: an `op` or `token` would win over the request's,
    // as the last of a key does.
    for (k, v) in args.iter().filter(|(k, _)| takes.contains(&k.as_str())) {
        let _ = write!(fields, ",\"{k}\":");
        v.write(&mut fields);
    }
    let (text, failed) = match call(&fields) {
        Ok(reply) => (said(op, &reply), false),
        Err(why) => (why, true),
    };
    let mut out =
        String::from("{\"resultType\":\"complete\",\"content\":[{\"type\":\"text\",\"text\":\"");
    escape_json(&text, &mut out);
    Ok(out + &format!("\"}}],\"isError\":{failed}}}"))
}

/// What blitz's `reply` to `op` says, as text for the model.
fn said(op: &str, reply: &Json) -> String {
    let num = |j: &Json, k| match j.get(k) {
        Some(Json::Num(n)) => n.to_string(),
        _ => "?".into(),
    };
    let text = |j: &Json, k| j.get(k).and_then(Json::as_str).unwrap_or("").to_owned();
    let state = |j: &Json| match j.get("exit") {
        Some(Json::Num(n)) => format!("exited with code {n}"),
        _ => text(j, "state"),
    };
    match op {
        "list" => {
            let panes = match reply.get("panes") {
                Some(Json::Arr(p)) if !p.is_empty() => p,
                _ => return "This session has opened no panes.".into(),
            };
            let lines: Vec<String> = (panes.iter())
                .map(|p| {
                    format!(
                        "pane {}: {}, {}, in {} (opened by pane {})",
                        num(p, "pane"),
                        text(p, "name"),
                        state(p),
                        text(p, "cwd"),
                        num(p, "parent")
                    )
                })
                .collect();
            lines.join("\n")
        }
        "open" => format!("Opened pane {}.", num(reply, "pane")),
        "read" => format!("{}\n[{}]", text(reply, "text"), state(reply)),
        "send" => "Sent.".into(),
        _ => "Closed.".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// The reply to `line`, with no blitz to call.
    fn ask(line: &str) -> Option<Json> {
        let reply = answer(line, &|_| panic!("no tool call"))?;
        Some(Json::parse(&reply).expect("a JSON reply"))
    }

    fn result(line: &str) -> Json {
        (ask(line).and_then(|r| r.get("result").cloned())).expect(line)
    }

    /// The id and error code of the reply to `line`.
    fn failure(line: &str) -> (Json, Json) {
        let r = ask(line).expect(line);
        let code = r.get("error").and_then(|e| e.get("code")).cloned();
        (r.get("id").cloned().expect("id"), code.expect(line))
    }

    fn strs(j: Option<&Json>) -> Vec<&str> {
        match j {
            Some(Json::Arr(a)) => a.iter().filter_map(Json::as_str).collect(),
            _ => Vec::new(),
        }
    }

    #[test]
    fn mcp_handshakes() {
        let d = result(r#"{"jsonrpc":"2.0","id":1,"method":"server/discover","params":{}}"#);
        assert_eq!(d.get("resultType").and_then(Json::as_str), Some("complete"));
        assert_eq!(d.get("cacheScope").and_then(Json::as_str), Some("public"));
        assert!(matches!(d.get("ttlMs"), Some(Json::Num(n)) if *n > 0.0));
        assert_eq!(strs(d.get("supportedVersions")), VERSIONS);
        let tools = d.get("capabilities").and_then(|c| c.get("tools"));
        assert_eq!(tools, Some(&Json::Obj(vec![])));
        let info = (d.get("_meta")).and_then(|m| m.get("io.modelcontextprotocol/serverInfo"));
        assert_eq!(
            info.and_then(|i| i.get("version")).and_then(Json::as_str),
            Some(env!("CARGO_PKG_VERSION"))
        );
        let said = d.get("instructions").and_then(Json::as_str).unwrap();
        assert!(said.len() < 500, "{}", said.len());

        for (asked, got) in [
            ("2026-07-28", "2026-07-28"),
            ("2025-11-25", "2025-11-25"),
            ("2024-11-05", "2025-11-25"),
        ] {
            let i = result(&format!(
                r#"{{"jsonrpc":"2.0","id":"a","method":"initialize","params":{{"protocolVersion":"{asked}","capabilities":{{}},"clientInfo":{{"name":"t","version":"0"}}}}}}"#
            ));
            assert_eq!(
                i.get("protocolVersion").and_then(Json::as_str),
                Some(got),
                "{asked}"
            );
            let name = i.get("serverInfo").and_then(|s| s.get("name"));
            assert_eq!(name.and_then(Json::as_str), Some("blitz"));
            assert!(i.get("capabilities").and_then(|c| c.get("tools")).is_some());
        }

        // Ids come back as they were sent.
        let ping = |id: &str| {
            let line = format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"ping"}}"#);
            answer(&line, &|_| panic!("no tool call"))
        };
        assert_eq!(
            ping("12345678901234567891").as_deref(),
            Some(r#"{"jsonrpc":"2.0","id":12345678901234567891,"result":{}}"#)
        );
        assert_eq!(
            ping(r#""a\"b""#).as_deref(),
            Some(r#"{"jsonrpc":"2.0","id":"a\"b","result":{}}"#)
        );
        // Control characters sent raw in a string go back escaped.
        let got = ping("\"a\u{1b}[2J\r\"").unwrap();
        assert!(!got.contains(['\u{1b}', '\r']), "{got:?}");

        // Notifications, and answers to requests never made, get none.
        for quiet in [
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":1}}"#,
            r#"{"jsonrpc":"2.0","method":"tools/call","params":{"name":"close"}}"#,
            r#"{"jsonrpc":"2.0","id":1,"result":{}}"#,
        ] {
            assert_eq!(ask(quiet), None, "{quiet}");
        }
    }

    #[test]
    fn mcp_errors() {
        let num = |n: f64| Json::Num(n);
        assert_eq!(failure("{oops"), (Json::Null, num(-32700.0)));
        assert_eq!(
            failure(r#"{"jsonrpc":"2.0","id":7,"method":"resources/list"}"#),
            (num(7.0), num(-32601.0))
        );
        for bad in [
            r#"{"jsonrpc":"2.0","id":8,"method":"tools/call"}"#,
            r#"{"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"frob"}}"#,
            r#"{"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"list","arguments":[1]}}"#,
        ] {
            assert_eq!(failure(bad), (num(8.0), num(-32602.0)), "{bad}");
        }

        let at = |v: &str| {
            format!(
                r#"{{"jsonrpc":"2.0","id":9,"method":"tools/list","params":{{"_meta":{{"io.modelcontextprotocol/protocolVersion":{v}}}}}}}"#
            )
        };
        assert!(result(&at("\"2026-07-28\"")).get("tools").is_some());
        let r = ask(&at("\"2099-01-01\"")).unwrap();
        let e = r.get("error").unwrap();
        assert_eq!(e.get("code"), Some(&num(-32022.0)));
        let data = e.get("data").unwrap();
        assert_eq!(strs(data.get("supported")), VERSIONS);
        assert_eq!(
            data.get("requested").and_then(Json::as_str),
            Some("2099-01-01")
        );
        assert_eq!(failure(&at("7")), (num(9.0), num(-32022.0)));
    }

    /// Only what reads gets readOnlyHint; what types, runs or closes is
    /// marked destructive, so Claude Code asks first.
    #[test]
    fn mcp_tools_list() {
        let r = result(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#);
        assert_eq!(r.get("resultType").and_then(Json::as_str), Some("complete"));
        assert_eq!(r.get("cacheScope").and_then(Json::as_str), Some("public"));
        assert!(matches!(r.get("ttlMs"), Some(Json::Num(n)) if *n > 0.0));
        let Some(Json::Arr(tools)) = r.get("tools") else {
            panic!("no tools");
        };
        let names: Vec<_> = tools
            .iter()
            .map(|t| t.get("name").and_then(Json::as_str))
            .collect();
        let want = ["list", "open", "send", "read", "close"];
        assert_eq!(names, want.map(Some));
        assert_eq!(want, ARGS.map(|a| a.0));
        for t in tools {
            let name = t.get("name").and_then(Json::as_str).unwrap();
            let schema = t.get("inputSchema").and_then(|s| s.get("type"));
            assert_eq!(schema.and_then(Json::as_str), Some("object"), "{name}");
            let desc = t.get("description").and_then(Json::as_str).unwrap();
            assert!(desc.contains("this session opened"), "{name}");
            let hint = |k| t.get("annotations").and_then(|a| a.get(k)).cloned();
            let looks = matches!(name, "list" | "read");
            assert_eq!(
                hint("readOnlyHint"),
                looks.then_some(Json::Bool(true)),
                "{name}"
            );
            assert_eq!(
                hint("destructiveHint"),
                (!looks).then_some(Json::Bool(true)),
                "{name}"
            );
            let again = (name == "close").then_some(Json::Bool(true));
            assert_eq!(hint("idempotentHint"), again, "{name}");
        }
    }

    #[test]
    fn mcp_tool_calls_go_to_blitz() {
        let sent = RefCell::new(Vec::new());
        let call = |fields: &str| {
            sent.borrow_mut().push(fields.to_owned());
            let reply = match fields {
                f if f.contains("\"list\"") => concat!(
                    r#"{"ok":true,"panes":[{"pane":8,"parent":3,"state":"working","#,
                    r#""exit":null,"name":"npm test","cwd":"/x"},{"pane":9,"parent":8,"#,
                    r#""state":"error","exit":1,"name":"pwsh","cwd":"/y"}]}"#
                ),
                f if f.contains("\"read\"") => {
                    r#"{"ok":true,"text":"a\nb","state":"idle","exit":null}"#
                }
                f if f.contains("\"open\"") => r#"{"ok":true,"pane":10}"#,
                _ => return Err("no pane 7 that this pane opened".to_owned()),
            };
            Ok(Json::parse(reply).unwrap())
        };
        let ask = |tool: &str, args: &str| {
            let line = format!(
                r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"{tool}","arguments":{args}}}}}"#
            );
            let r = Json::parse(&answer(&line, &call).unwrap()).unwrap();
            let r = r.get("result").cloned().expect("a result");
            assert_eq!(r.get("resultType").and_then(Json::as_str), Some("complete"));
            let text = match r.get("content") {
                Some(Json::Arr(c)) => c[0].get("text").and_then(Json::as_str).unwrap().to_owned(),
                _ => panic!("no content"),
            };
            (text, r.get("isError") == Some(&Json::Bool(true)))
        };
        assert_eq!(
            ask("list", "{}"),
            (
                "pane 8: npm test, working, in /x (opened by pane 3)\n\
                 pane 9: pwsh, exited with code 1, in /y (opened by pane 8)"
                    .into(),
                false
            )
        );
        // A tool's own arguments go on, and nothing else.
        assert_eq!(
            ask("read", r#"{"pane":7,"lines":5,"op":"close","token":"x"}"#),
            ("a\nb\n[idle]".into(), false)
        );
        assert_eq!(
            ask(
                "open",
                r#"{"place":"tab","command":"npm \"t\"","focus":true}"#
            ),
            ("Opened pane 10.".into(), false)
        );
        assert_eq!(
            ask("close", r#"{"pane":7}"#),
            ("no pane 7 that this pane opened".into(), true)
        );
        assert_eq!(
            *sent.borrow(),
            [
                r#""op":"list""#,
                r#""op":"read","pane":7,"lines":5"#,
                r#""op":"open","place":"tab","command":"npm \"t\"","focus":true"#,
                r#""op":"close","pane":7"#,
            ]
        );
        // What blitz-hook sends, blitz reads.
        for fields in sent.borrow().iter() {
            let line = format!("{{\"token\":\"t\",{fields}}}");
            assert!(super::super::parse(&line).is_ok(), "{line}");
        }

        // Without the pane's token, the model is told why.
        let line = r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"list"}}"#;
        let r = answer(line, &|_| Err(super::super::NOT_IN_PANE.into())).unwrap();
        let r = Json::parse(&r).unwrap();
        let r = r.get("result").unwrap();
        assert_eq!(r.get("isError"), Some(&Json::Bool(true)));
        let Some(Json::Arr(c)) = r.get("content") else {
            panic!("no content");
        };
        let text = c[0].get("text").and_then(Json::as_str).unwrap();
        assert!(text.contains("CLAUDE_CODE_SUBPROCESS_ENV_SCRUB"), "{text}");
    }
}
