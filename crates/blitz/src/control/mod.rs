//! `blitz pane`: a program in a pane opens panes of its own and drives
//! them. blitz listens where only this user's programs reach: a named pipe
//! in this session on Windows, a socket in a folder of the user's own
//! elsewhere. Every pane gets its name as `BLITZ_PIPE`. A connection
//! carries one line of JSON each way: a request, then blitz's reply.
//!
//! A request carries the pane's `BLITZ_PANE_TOKEN`, which says which pane
//! it comes from, and it may touch only the panes that pane opened and the
//! panes those opened. That keeps programs that play along apart; it is no
//! wall against another program of the same user.

use std::fmt::Write as _;
use std::io::{self, BufRead, Read, Write};
use std::path::PathBuf;
use std::time::Duration;

use crate::attention::Attn;
use crate::hook::{Json, escape_json};
use crate::layout::{Dir, PaneId};

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod win;

#[cfg(unix)]
pub use unix::listen;
#[cfg(unix)]
use unix::{connect, valid};
#[cfg(windows)]
pub use win::listen;
#[cfg(windows)]
use win::{connect, valid};

/// The longest request taken, in bytes.
const MAX_REQUEST: usize = 1 << 20;
/// The most text one `send` types, in bytes.
const MAX_TEXT: usize = 64 * 1024;
/// The most keys one `send` presses.
const MAX_KEYS: usize = 256;
/// The most lines one `read` returns.
pub const MAX_LINES: usize = 2000;
/// The most bytes one `read` returns, the oldest cut.
pub const MAX_READ: usize = 40 * 1024;
/// How long blitz takes to answer before the caller is told it did not.
pub const ANSWER: Duration = Duration::from_secs(10);

const NOT_IN_PANE: &str =
    "not in a blitz pane, or CLAUDE_CODE_SUBPROCESS_ENV_SCRUB removed BLITZ_PANE_TOKEN";

const USAGE: &str = "usage: blitz pane list
       blitz pane split [--down] [--cwd DIR] [--focus] [CMD...]
       blitz pane tab [--cwd DIR] [--focus] [CMD...]
       blitz pane send ID [--key KEY]... [TEXT]
       blitz pane read ID [--lines N]
       blitz pane close ID";

/// Where `open` puts its pane: beside the caller's pane, or in a tab after
/// the caller's tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    Beside(Dir),
    Tab,
}

/// What a request asks.
#[derive(Debug, PartialEq)]
pub enum Op {
    List,
    Open {
        place: Place,
        cwd: Option<PathBuf>,
        /// Typed at the new pane's first prompt.
        command: Option<String>,
        focus: bool,
    },
    Send {
        pane: PaneId,
        text: String,
        keys: Vec<String>,
    },
    Read {
        pane: PaneId,
        lines: usize,
    },
    Close {
        pane: PaneId,
    },
}

/// The token a request line carries and what it asks, or why it is not
/// one. The reason is blitz's own words: nothing of the request goes back.
pub fn parse(line: &str) -> Result<(String, Op), &'static str> {
    let j = Json::parse(line).ok_or("not a request")?;
    let token = (j.get("token").and_then(Json::as_str))
        .filter(|t| !t.is_empty())
        .ok_or("no token")?;
    let text = |k| match j.get(k) {
        None | Some(Json::Null) => Ok(None),
        Some(Json::Str(s)) => Ok(Some(s.clone())),
        _ => Err("a field that should be text is not"),
    };
    let pane = || {
        (j.get("pane").and_then(number))
            .map(PaneId)
            .ok_or("no pane id")
    };
    let op = match j.get("op").and_then(Json::as_str) {
        Some("list") => Op::List,
        Some("open") => Op::Open {
            place: match j.get("place").and_then(Json::as_str) {
                None | Some("right") => Place::Beside(Dir::Right),
                Some("down") => Place::Beside(Dir::Down),
                Some("tab") => Place::Tab,
                _ => return Err("place is right, down or tab"),
            },
            // As for a folder a program reports with OSC 7: blitz looks in
            // it, and a network path would make it sign in to that host.
            cwd: match text("cwd")? {
                Some(d) if vt::osc::local_dir(&d) => Some(d.into()),
                Some(_) => return Err("cwd is not a folder on this machine"),
                None => None,
            },
            command: text("command")?.filter(|c| !c.is_empty()),
            focus: j.get("focus") == Some(&Json::Bool(true)),
        },
        Some("send") => {
            let text = text("text")?.unwrap_or_default();
            let keys = match j.get("keys") {
                None | Some(Json::Null) => Vec::new(),
                Some(Json::Arr(a)) => (a.iter())
                    .map(|k| k.as_str().map(str::to_owned))
                    .collect::<Option<_>>()
                    .ok_or("keys are names such as enter")?,
                _ => return Err("keys are names such as enter"),
            };
            if text.len() > MAX_TEXT {
                return Err("text is over 64 KiB");
            }
            if keys.len() > MAX_KEYS {
                return Err("more than 256 keys");
            }
            if text.is_empty() && keys.is_empty() {
                return Err("nothing to send");
            }
            Op::Send {
                pane: pane()?,
                text,
                keys,
            }
        }
        Some("read") => Op::Read {
            pane: pane()?,
            lines: match j.get("lines") {
                None | Some(Json::Null) => 100,
                Some(n) => number(n).ok_or("lines is not a number")? as usize,
            }
            .min(MAX_LINES),
        },
        Some("close") => Op::Close { pane: pane()? },
        _ => return Err("no such op"),
    };
    Ok((token.to_owned(), op))
}

/// A whole number that fits a u32.
fn number(j: &Json) -> Option<u32> {
    match *j {
        Json::Num(n) if n.fract() == 0.0 && (0.0..=f64::from(u32::MAX)).contains(&n) => {
            Some(n as u32)
        }
        _ => None,
    }
}

/// Whether `s` is `len` lowercase hex digits, as a channel's name has.
fn hex(s: &str, len: usize) -> bool {
    s.len() == len && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Whether tokens `a` and `b` are the same, in a time that does not say
/// how much of them is.
pub fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0, |d, (x, y)| d | (x ^ y)) == 0
}

/// The reply for a request refused, saying `why`.
pub fn error(why: &str) -> String {
    let mut out = String::from("{\"ok\":false,\"error\":\"");
    escape_json(why, &mut out);
    out + "\"}"
}

/// A session's state as `list` and `read` name it.
pub fn state(a: Attn) -> &'static str {
    match a {
        Attn::Idle => "idle",
        Attn::Working => "working",
        Attn::DoneUnseen => "done",
        Attn::Error => "error",
        Attn::NeedsYou => "needs-you",
    }
}

/// What `send` types for `text` and `keys` under the modes `m`: the text
/// as a paste, which drops escapes and other control characters, then each
/// key as the keyboard would send it.
pub fn send_bytes(
    text: &str,
    keys: &[String],
    m: &vt::InputModes,
) -> Result<Vec<u8>, &'static str> {
    let mut out = Vec::new();
    if !text.is_empty() {
        vt::encode_paste(text, m.bracketed, &mut out);
    }
    for k in keys {
        let mut buf = String::new();
        let inputs = crate::debug::chord(k, &mut buf)
            .map_err(|_| "not a key: keys are names such as enter, esc, up, f5 or ctrl+c")?;
        for input in &inputs {
            vt::encode_key(input, m, &mut out);
        }
    }
    Ok(out)
}

/// The line `open` types at the new pane's first prompt for `command`,
/// with control characters dropped as `send` drops them.
pub fn typed_line(command: &str) -> String {
    let mut out = Vec::new();
    vt::encode_paste(command.trim_end(), false, &mut out);
    String::from_utf8_lossy(&out).into_owned() + "\r"
}

/// Answers one connection: reads its request line and writes the reply
/// `answer` makes of it.
fn serve(conn: &mut (impl Read + Write), answer: impl Fn(String) -> String) {
    let mut line = Vec::new();
    let read =
        io::BufReader::new((&mut *conn).take(MAX_REQUEST as u64 + 1)).read_until(b'\n', &mut line);
    // A caller that sent nothing, as one that only looks whether blitz
    // listens, has nothing for the window to answer.
    let reply = match (read, String::from_utf8(line)) {
        (Ok(1..), Ok(line)) if line.len() <= MAX_REQUEST => answer(line),
        _ => error("not a request"),
    };
    let _ = conn.write_all(format!("{reply}\n").as_bytes());
}

/// Sends `fields`, the inside of a request's JSON object, with this pane's
/// token to the blitz the pane is in, and returns its reply, or why there
/// is none or what it refused.
pub fn call(fields: &str) -> Result<Json, String> {
    let token = std::env::var("BLITZ_PANE_TOKEN").unwrap_or_default();
    if token.is_empty() {
        return Err(NOT_IN_PANE.into());
    }
    let pipe = std::env::var("BLITZ_PIPE").unwrap_or_default();
    if pipe.is_empty() {
        return Err("BLITZ_PIPE is not set: this pane's blitz has no `blitz pane`".into());
    }
    // Never a network path, which would sign in to that host.
    if !valid(&pipe) {
        return Err("BLITZ_PIPE is not one blitz made".into());
    }
    let mut conn = connect(&pipe).map_err(|e| format!("cannot reach blitz: {e}"))?;
    let mut req = String::from("{\"token\":\"");
    escape_json(&token, &mut req);
    let _ = writeln!(req, "\",{fields}}}");
    let mut reply = String::new();
    let sent = conn
        .write_all(req.as_bytes())
        .and_then(|()| io::BufReader::new(conn.take(MAX_REQUEST as u64)).read_line(&mut reply));
    sent.map_err(|e| format!("cannot reach blitz: {e}"))?;
    let reply = Json::parse(&reply).ok_or("blitz did not answer")?;
    match reply.get("ok") {
        Some(Json::Bool(true)) => Ok(reply),
        _ => Err((reply.get("error").and_then(Json::as_str))
            .unwrap_or("blitz refused")
            .to_owned()),
    }
}

/// `blitz pane ...`: returns the exit code, 0 when it worked, 1 when blitz
/// refused or could not be reached, 2 for a command it does not take.
pub fn cli(args: &[String]) -> i32 {
    let Some(fields) = request(args) else {
        eprintln!("{USAGE}");
        return 2;
    };
    let reply = match call(&fields) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("blitz: {e}");
            return 1;
        }
    };
    let mut out = String::new();
    // One pane a line, its columns apart.
    let field = |j: &Json, k| match j.get(k) {
        Some(Json::Num(n)) => n.to_string(),
        Some(Json::Str(s)) => s.replace(['\t', '\r', '\n'], " "),
        _ => "-".into(),
    };
    match args[0].as_str() {
        "list" => {
            if let Some(Json::Arr(panes)) = reply.get("panes") {
                for p in panes {
                    let cols = ["pane", "parent", "state", "name", "cwd"].map(|k| field(p, k));
                    let _ = writeln!(out, "{}", cols.join("\t"));
                }
            }
        }
        "split" | "tab" => out = field(&reply, "pane") + "\n",
        "read" => {
            out = reply
                .get("text")
                .and_then(Json::as_str)
                .unwrap_or("")
                .into();
            if !out.is_empty() {
                out.push('\n');
            }
        }
        _ => {}
    }
    // A reader that has gone, as `head` once it has its lines, is no error.
    let _ = io::stdout().write_all(out.as_bytes());
    0
}

/// The fields of the request `blitz pane` sends for `args`; `None` when
/// they are not a command it takes.
fn request(args: &[String]) -> Option<String> {
    let (cmd, rest) = args.split_first()?;
    let id = |a: &String| a.parse::<u32>().ok();
    let op = match cmd.as_str() {
        "split" | "tab" => "open",
        c @ ("list" | "send" | "read" | "close") => c,
        _ => return None,
    };
    let mut out = format!("\"op\":\"{op}\"");
    match (cmd.as_str(), rest) {
        ("list", []) => {}
        ("split" | "tab", _) => {
            let mut place = if cmd == "tab" { "tab" } else { "right" };
            let mut focus = false;
            let mut it = rest.iter();
            let mut words: Vec<&str> = Vec::new();
            while let Some(a) = it.next() {
                match a.as_str() {
                    "--down" if cmd == "split" => place = "down",
                    "--focus" => focus = true,
                    // blitz reads the folder from where it runs, not here.
                    "--cwd" => {
                        let dir = std::path::absolute(it.next()?).ok()?;
                        put(&mut out, "cwd", &dir.to_string_lossy());
                    }
                    "--" => words.extend(it.by_ref().map(String::as_str)),
                    o if o.starts_with("--") => return None,
                    w => {
                        words.push(w);
                        words.extend(it.by_ref().map(String::as_str));
                    }
                }
            }
            put(&mut out, "place", place);
            if !words.is_empty() {
                put(&mut out, "command", &words.join(" "));
            }
            if focus {
                out.push_str(",\"focus\":true");
            }
        }
        ("send", [pane, more @ ..]) => {
            let _ = write!(out, ",\"pane\":{}", id(pane)?);
            let (mut keys, mut text) = (Vec::new(), None);
            let mut it = more.iter();
            while let Some(a) = it.next() {
                match a.as_str() {
                    "--key" => keys.push(it.next()?),
                    _ if text.is_some() => return None,
                    t => text = Some(t),
                }
            }
            if keys.is_empty() && text.is_none() {
                return None;
            }
            if let Some(t) = text {
                put(&mut out, "text", t);
            }
            let mut list = String::new();
            for k in keys {
                list.push_str(if list.is_empty() { "\"" } else { ",\"" });
                escape_json(k, &mut list);
                list.push('"');
            }
            let _ = write!(out, ",\"keys\":[{list}]");
        }
        ("read" | "close", [pane]) => {
            let _ = write!(out, ",\"pane\":{}", id(pane)?);
        }
        ("read", [pane, flag, n]) if flag == "--lines" => {
            let _ = write!(out, ",\"pane\":{},\"lines\":{}", id(pane)?, id(n)?);
        }
        _ => return None,
    }
    Some(out)
}

/// Appends `"k":"v"` to the fields in `out`.
fn put(out: &mut String, k: &str, v: &str) {
    let _ = write!(out, ",\"{k}\":\"");
    escape_json(v, out);
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    fn op(line: &str) -> Result<Op, &'static str> {
        parse(line).map(|(_, op)| op)
    }

    #[test]
    fn control_requests_parse() {
        let (token, list) = parse(r#"{"token":"t0","op":"list"}"#).unwrap();
        assert_eq!((token.as_str(), list), ("t0", Op::List));
        let dir = if cfg!(windows) { r"C:\x" } else { "/x" };
        let mut cwd = String::new();
        escape_json(dir, &mut cwd);
        let line = format!(
            r#"{{"token":"t","op":"open","place":"down","cwd":"{cwd}","command":"npm test","focus":true}}"#
        );
        assert_eq!(
            op(&line),
            Ok(Op::Open {
                place: Place::Beside(Dir::Down),
                cwd: Some(dir.into()),
                command: Some("npm test".into()),
                focus: true,
            })
        );
        assert_eq!(
            op(r#"{"token":"t","op":"open"}"#),
            Ok(Op::Open {
                place: Place::Beside(Dir::Right),
                cwd: None,
                command: None,
                focus: false,
            })
        );
        assert_eq!(
            op(r#"{"token":"t","op":"send","pane":7,"keys":["enter"]}"#),
            Ok(Op::Send {
                pane: PaneId(7),
                text: String::new(),
                keys: vec!["enter".into()],
            })
        );
        assert_eq!(
            op(r#"{"token":"t","op":"read","pane":7,"lines":99999}"#),
            Ok(Op::Read {
                pane: PaneId(7),
                lines: MAX_LINES
            })
        );
        for read in [
            r#"{"token":"t","op":"read","pane":7}"#,
            r#"{"token":"t","op":"read","pane":7,"lines":null}"#,
        ] {
            assert_eq!(
                op(read),
                Ok(Op::Read {
                    pane: PaneId(7),
                    lines: 100
                })
            );
        }
        assert_eq!(
            op(r#"{"token":"t","op":"close","pane":3}"#),
            Ok(Op::Close { pane: PaneId(3) })
        );
    }

    /// A refused request is told why in blitz's words, never with any of
    /// what it sent, which may be a secret on a command line.
    #[test]
    fn control_refusals_never_echo_the_request() {
        let long = "secret".repeat(MAX_TEXT / 6 + 1);
        let keys = vec!["\"a\""; MAX_KEYS + 1].join(",");
        for bad in [
            "secret".to_string(),
            r#"{"op":"list"}"#.into(),
            r#"{"token":"","op":"list"}"#.into(),
            r#"{"token":"t","op":"secret"}"#.into(),
            r#"{"token":"t","op":"open","place":"secret"}"#.into(),
            r#"{"token":"t","op":"open","cwd":"secret"}"#.into(),
            r#"{"token":"t","op":"open","command":["secret"]}"#.into(),
            r#"{"token":"t","op":"send","pane":1}"#.into(),
            r#"{"token":"t","op":"send","text":"secret"}"#.into(),
            r#"{"token":"t","op":"send","pane":-1,"text":"secret"}"#.into(),
            r#"{"token":"t","op":"send","pane":1.5,"text":"secret"}"#.into(),
            r#"{"token":"t","op":"send","pane":1,"keys":"secret"}"#.into(),
            format!(r#"{{"token":"t","op":"send","pane":1,"text":"{long}"}}"#),
            format!(r#"{{"token":"t","op":"send","pane":1,"keys":[{keys}]}}"#),
            r#"{"token":"t","op":"read","pane":1,"lines":"secret"}"#.into(),
            r#"{"token":"t","op":"focus","pane":1}"#.into(),
        ] {
            let why = parse(&bad).map(|_| ()).expect_err(&bad);
            assert!(!why.contains("secret"), "{bad}: {why}");
            assert!(Json::parse(&error(why)).is_some());
        }
        let reply = Json::parse(&error("pane \"7\"")).unwrap();
        assert_eq!(reply.get("ok"), Some(&Json::Bool(false)));
        assert_eq!(
            reply.get("error").and_then(Json::as_str),
            Some("pane \"7\"")
        );
    }

    /// Looking in a folder on another host signs in to it, on the window's
    /// thread.
    #[test]
    fn control_open_takes_only_folders_on_this_machine() {
        let open = |cwd: &str| {
            let mut j = String::new();
            escape_json(cwd, &mut j);
            op(&format!(r#"{{"token":"t","op":"open","cwd":"{j}"}}"#)).map(|_| ())
        };
        let refused = Err("cwd is not a folder on this machine");
        for bad in [
            r"\\host\share",
            r"\\?\UNC\h\s",
            "//host/x",
            "relative",
            "C:x",
        ] {
            assert_eq!(open(bad), refused, "{bad}");
        }
        // Elsewhere a path from one `/` is local to OSC 7 and hand-offs
        // too, automounted `/net` included.
        if cfg!(windows) {
            assert_eq!(open("/net/host/x"), refused);
            assert_eq!(open(r"C:\x"), Ok(()));
        } else {
            assert_eq!(open("/home/x"), Ok(()));
        }
    }

    #[test]
    fn control_tokens_compare_in_full() {
        assert!(same("0123abcd", "0123abcd"));
        assert!(!same("0123abcd", "0123abce"));
        assert!(!same("0123abcd", "0123abc"));
        assert!(!same("", "a"));
    }

    /// Text goes as a paste, so no escape in it reaches the program; keys
    /// go as the keyboard sends them.
    #[test]
    fn control_send_drops_control_characters() {
        let plain = vt::InputModes::default();
        let bracketed = vt::InputModes {
            bracketed: true,
            ..Default::default()
        };
        let keys = |k: &[&str]| k.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            send_bytes("ls\x1b[201~\x07\u{9b}\n", &keys(&["enter"]), &plain),
            Ok(b"ls[201~\r\r".to_vec())
        );
        assert_eq!(
            send_bytes("a\nb", &[], &bracketed),
            Ok(b"\x1b[200~a\rb\x1b[201~".to_vec())
        );
        assert_eq!(
            send_bytes("", &keys(&["ctrl+c"]), &plain),
            Ok(b"\x03".to_vec())
        );
        assert!(send_bytes("", &keys(&["hyper+x"]), &plain).is_err());
        assert_eq!(typed_line("npm\x1b]0;x\x07 test\n"), "npm]0;x test\r");
    }

    #[test]
    fn control_cli_arguments() {
        let req = |a: &[&str]| request(&args(a));
        assert_eq!(req(&["list"]).as_deref(), Some(r#""op":"list""#));
        assert_eq!(req(&["list", "x"]), None);
        assert_eq!(
            req(&["split", "--down", "--focus", "npm", "test", "--watch"]).as_deref(),
            Some(r#""op":"open","place":"down","command":"npm test --watch","focus":true"#)
        );
        assert_eq!(
            req(&["tab", "--", "--x"]).as_deref(),
            Some(r#""op":"open","place":"tab","command":"--x""#)
        );
        let cwd = req(&["split", "--cwd", "sub"]).unwrap();
        let abs = std::env::current_dir().unwrap().join("sub");
        let mut want = String::new();
        escape_json(&abs.to_string_lossy(), &mut want);
        assert!(cwd.contains(&want), "{cwd}");
        assert_eq!(req(&["split", "--cwd"]), None);
        assert_eq!(req(&["tab", "--down"]), None);
        assert_eq!(req(&["split", "--frob"]), None);
        assert_eq!(
            req(&[
                "send",
                "7",
                "--key",
                "ctrl+c",
                "say \"hi\"",
                "--key",
                "enter"
            ])
            .as_deref(),
            Some(r#""op":"send","pane":7,"text":"say \"hi\"","keys":["ctrl+c","enter"]"#)
        );
        assert_eq!(
            req(&["send", "7", "hi"]).as_deref(),
            Some(r#""op":"send","pane":7,"text":"hi","keys":[]"#)
        );
        assert_eq!(req(&["send", "7"]), None, "nothing to send");
        assert_eq!(req(&["send", "7", "a", "b"]), None);
        assert_eq!(req(&["send", "x", "a"]), None);
        assert_eq!(
            req(&["read", "7", "--lines", "5"]).as_deref(),
            Some(r#""op":"read","pane":7,"lines":5"#)
        );
        assert_eq!(
            req(&["read", "7"]).as_deref(),
            Some(r#""op":"read","pane":7"#)
        );
        assert_eq!(req(&["read", "7", "--lines"]), None);
        assert_eq!(
            req(&["close", "7"]).as_deref(),
            Some(r#""op":"close","pane":7"#)
        );
        assert_eq!(req(&["close"]), None);
        assert_eq!(req(&["focus", "7"]), None);
        assert_eq!(req(&[]), None);
        // What the CLI sends, blitz reads.
        for a in [
            &["list"][..],
            &["split", "x"],
            &["send", "1", "--key", "up"],
            &["read", "2"],
        ] {
            let line = format!("{{\"token\":\"t\",{}}}", req(a).unwrap());
            assert!(parse(&line).is_ok(), "{line}");
        }
    }
}
