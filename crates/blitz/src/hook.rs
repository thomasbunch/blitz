//! `blitz-hook claude` reports Claude Code's state to the pane it runs in;
//! `blitz setup claude` prints the hook settings to install.

use std::fmt::Write as _;
use std::io::{Read, Write};

/// Most payloads are a few KB, but a Write or Edit request carries the
/// whole file, so the cap only stops a runaway stream. A payload cut at the
/// cap does not parse and reports nothing.
const MAX_INPUT: u64 = 64 << 20;
/// Longest message carried in a notification, in chars.
const MAX_MSG: usize = 120;
/// Notification types that mean Claude Code is waiting on the user.
/// `idle_prompt` is left out: it fires a minute after every finished turn.
const NOTIFY_TYPES: &str =
    "permission_prompt|elicitation_dialog|elicitation_url_dialog|agent_needs_input";

/// Entry point of `blitz-hook`. Always returns 0, so a hook can never block
/// Claude Code.
///
/// `blitz-hook claude` reads a hook payload on stdin and prints a
/// `terminalSequence` for Claude Code to write to its own terminal, so the
/// state lands in the right pane without any IPC. The sequence carries the
/// pane's `BLITZ_PANE_TOKEN`, which program output cannot know. Outside a
/// blitz pane (no token) it prints nothing.
pub fn run() -> i32 {
    let claude = std::env::args().nth(1).as_deref() == Some("claude");
    let token = std::env::var("BLITZ_PANE_TOKEN").unwrap_or_default();
    // It goes into the sequence as is, so nothing in it may end the title.
    let in_pane = !token.is_empty() && token.bytes().all(|b| b.is_ascii_alphanumeric());
    if claude {
        // Read even outside a pane: Claude Code writes the whole payload,
        // and exiting first would break the pipe under it.
        let mut input = Vec::new();
        let _ = std::io::stdin().take(MAX_INPUT).read_to_end(&mut input);
        if !in_pane {
            return 0;
        }
        if let Some(out) = claude_output(&token, &String::from_utf8_lossy(&input)) {
            let mut stdout = std::io::stdout().lock();
            let _ = stdout
                .write_all(out.as_bytes())
                .and_then(|()| stdout.flush());
        }
    }
    0
}

/// The hook's stdout for one Claude Code payload, or `None` when the event
/// is not one blitz reports. A byte-order mark in front is skipped.
pub fn claude_output(token: &str, payload: &str) -> Option<String> {
    let ev = Json::parse(payload.strip_prefix('\u{feff}').unwrap_or(payload))?;
    let (state, msg) = claude_state(&ev)?;
    let session = ev
        .get("session_id")
        .and_then(Json::as_str)
        .filter(|id| is_session_id(id));
    Some(notify_json(token, state, session, &msg))
}

/// Whether `id` looks like a Claude Code session id: 36 hex digits and
/// dashes. blitz later types the id into a shell, so anything else is
/// dropped.
pub fn is_session_id(id: &str) -> bool {
    id.len() == 36 && id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
}

/// Maps a hook payload to a state (`working`, `needs-you`, `done`, `error`,
/// `idle`) and a one-line message.
pub fn claude_state(ev: &Json) -> Option<(&'static str, String)> {
    fn field<'a>(v: &'a Json, k: &str) -> &'a str {
        v.get(k).and_then(Json::as_str).unwrap_or("")
    }
    let tool = field(ev, "tool_name");
    let input = ev.get("tool_input").unwrap_or(&Json::Null);
    Some(match field(ev, "hook_event_name") {
        "UserPromptSubmit" => ("working", String::new()),
        "PermissionRequest" => {
            let detail = match field(input, "command") {
                "" => field(input, "file_path"),
                c => c,
            };
            let msg = match (tool, detail) {
                (t, "") | ("", t) => t.to_owned(),
                (t, d) => format!("{t}: {d}"),
            };
            ("needs-you", msg)
        }
        "PreToolUse" if tool == "AskUserQuestion" => {
            let question = match input.get("questions") {
                Some(Json::Arr(qs)) => qs.first().map_or("", |q| field(q, "question")),
                _ => "",
            };
            let msg = if question.is_empty() {
                "Question"
            } else {
                question
            };
            ("needs-you", msg.to_owned())
        }
        "PreToolUse" if tool == "ExitPlanMode" => ("needs-you", "Plan ready".to_owned()),
        "Notification" => {
            let kind = field(ev, "notification_type");
            if !kind.is_empty() && !NOTIFY_TYPES.split('|').any(|t| t == kind) {
                return None;
            }
            ("needs-you", field(ev, "message").to_owned())
        }
        "Stop" => {
            // A Stop hook made Claude continue; the turn is not over.
            if ev.get("stop_hook_active") == Some(&Json::Bool(true)) {
                return None;
            }
            match ev.get("background_tasks") {
                Some(Json::Arr(tasks)) if !tasks.is_empty() => ("working", String::new()),
                _ => {
                    let reply = field(ev, "last_assistant_message");
                    let first = reply.lines().map(str::trim).find(|l| !l.is_empty());
                    ("done", first.unwrap_or("").to_owned())
                }
            }
        }
        "StopFailure" => ("error", field(ev, "error").to_owned()),
        "SessionEnd" => ("idle", String::new()),
        _ => return None,
    })
}

/// `{"terminalSequence":"ESC]777;notify;blitz:<token>:<state>;<msg>BEL"}`
/// and a newline, with `:<session>` after the state when there is one. The
/// message is made safe to embed first; the session must already pass
/// `is_session_id`.
pub fn notify_json(token: &str, state: &str, session: Option<&str>, msg: &str) -> String {
    let session = session.map(|s| format!(":{s}")).unwrap_or_default();
    let seq = format!(
        "\x1b]777;notify;blitz:{token}:{state}{session};{}\x07",
        one_line(msg)
    );
    let mut out = String::from("{\"terminalSequence\":\"");
    escape_json(&seq, &mut out);
    out.push_str("\"}\n");
    out
}

/// Drops control characters (which could end the OSC early), folds runs of
/// whitespace into one space, and caps the length at `MAX_MSG` chars.
pub fn one_line(s: &str) -> String {
    let words: Vec<String> = s
        .split_whitespace()
        .map(|w| w.replace(char::is_control, ""))
        .filter(|w| !w.is_empty())
        .collect();
    let line = words.join(" ");
    if line.chars().count() <= MAX_MSG {
        return line;
    }
    let mut cut: String = line.chars().take(MAX_MSG - 1).collect();
    cut.push('…');
    cut
}

/// Claude Code events blitz hooks, with the matcher each one needs.
const CLAUDE_HOOKS: [(&str, &str); 7] = [
    ("UserPromptSubmit", ""),
    ("PermissionRequest", ""),
    ("PreToolUse", "^(AskUserQuestion|ExitPlanMode)$"),
    ("Notification", NOTIFY_TYPES),
    ("Stop", ""),
    ("StopFailure", ""),
    ("SessionEnd", ""),
];

/// `blitz setup <app>`. Returns the process exit code.
///
/// `blitz setup claude` prints the hooks to add to Claude Code's settings,
/// pointing at the `blitz-hook` next to this exe. It never edits the
/// settings file itself; that file belongs to the user.
pub fn setup(args: &[String]) -> i32 {
    if args != ["claude"] {
        eprintln!("usage: blitz setup claude");
        return 2;
    }
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => {
            eprintln!("blitz setup: cannot find blitz itself: {e}");
            return 1;
        }
    };
    let hook = exe.with_file_name(format!("blitz-hook{}", std::env::consts::EXE_SUFFIX));
    if !hook.is_file() {
        eprintln!(
            "warning: {} is missing; keep it next to blitz",
            hook.display()
        );
    }
    #[cfg(windows)]
    if [hook.parent(), Some(hook.as_path())]
        .into_iter()
        .flatten()
        .any(others_can_write)
    {
        eprintln!(
            "warning: other users can replace {}, and Claude Code would run their \
             program in every session. Keep blitz in a folder only you can change, \
             such as where the installer puts it.",
            hook.display()
        );
    }
    let settings = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::home_dir().map(|h| h.join(".claude")))
        .unwrap_or_default()
        .join("settings.json");
    eprintln!(
        "Merge the \"hooks\" below into {}.\n\
         Claude Code picks the change up without a restart.\n",
        settings.display()
    );
    print!("{}", claude_settings(&hook.to_string_lossy()));
    0
}

/// Whether anyone but this user, SYSTEM, Administrators or TrustedInstaller
/// may change `path`, or add and remove files in it if it is a folder. Deny
/// entries are not weighed against the grants, so it errs towards yes.
#[cfg(windows)]
fn others_can_write(path: &std::path::Path) -> bool {
    use windows::Win32::Foundation::{
        CloseHandle, GENERIC_ALL, GENERIC_WRITE, HANDLE, HLOCAL, LocalFree,
    };
    use windows::Win32::Security::Authorization::{
        ConvertSidToStringSidW, GetNamedSecurityInfoW, SE_FILE_OBJECT,
    };
    use windows::Win32::Security::{
        ACCESS_ALLOWED_ACE, ACL, DACL_SECURITY_INFORMATION, GetAce, GetTokenInformation,
        INHERIT_ONLY_ACE, PSECURITY_DESCRIPTOR, PSID, TOKEN_QUERY, TOKEN_USER, TokenUser,
    };
    use windows::Win32::Storage::FileSystem::{
        DELETE, FILE_DELETE_CHILD, FILE_WRITE_DATA, WRITE_DAC, WRITE_OWNER,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    use windows::core::{HSTRING, PWSTR};

    // For a folder, FILE_WRITE_DATA is the right to add files.
    const WRITE: u32 = FILE_WRITE_DATA.0
        | FILE_DELETE_CHILD.0
        | DELETE.0
        | WRITE_DAC.0
        | WRITE_OWNER.0
        | GENERIC_WRITE.0
        | GENERIC_ALL.0;
    // SYSTEM, Administrators and TrustedInstaller.
    const TRUSTED: [&str; 3] = [
        "S-1-5-18",
        "S-1-5-32-544",
        "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464",
    ];
    let sid_string = |sid: PSID| {
        let mut s = PWSTR::null();
        // SAFETY: a valid SID; the string is copied, then freed.
        unsafe {
            ConvertSidToStringSidW(sid, &mut s).ok()?;
            let out = s.to_string().ok();
            LocalFree(Some(HLOCAL(s.0.cast())));
            out
        }
    };
    // SAFETY: the buffer is as large as the call is told; the token is
    // closed after use.
    let me = unsafe {
        let mut token = HANDLE::default();
        let mut user = [0u64; 64];
        let mut len = 0;
        let ok = OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_ok()
            && GetTokenInformation(
                token,
                TokenUser,
                Some(user.as_mut_ptr().cast()),
                size_of_val(&user) as u32,
                &mut len,
            )
            .is_ok();
        let _ = CloseHandle(token);
        ok.then(|| sid_string((*user.as_ptr().cast::<TOKEN_USER>()).User.Sid))
            .flatten()
    };
    let mut dacl: *mut ACL = std::ptr::null_mut();
    let mut sd = PSECURITY_DESCRIPTOR::default();
    // SAFETY: valid out pointers. The DACL and its entries live inside `sd`,
    // which is freed after the last use of them.
    unsafe {
        let err = GetNamedSecurityInfoW(
            &HSTRING::from(path),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            None,
            None,
            Some(&mut dacl),
            None,
            &mut sd,
        );
        if err.is_err() {
            return false;
        }
        // Without a DACL everyone may do anything.
        let mut open = dacl.is_null();
        for i in 0..if open { 0 } else { (*dacl).AceCount } {
            let mut ace = std::ptr::null_mut();
            if GetAce(dacl, i.into(), &mut ace).is_err() {
                continue;
            }
            let ace = &*ace.cast::<ACCESS_ALLOWED_ACE>();
            // Type 0 grants access; an inherit-only entry is for children.
            if ace.Header.AceType != 0
                || u32::from(ace.Header.AceFlags) & INHERIT_ONLY_ACE.0 != 0
                || ace.Mask & WRITE == 0
            {
                continue;
            }
            let who = sid_string(PSID((&raw const ace.SidStart).cast_mut().cast()));
            if !who.is_some_and(|w| TRUSTED.contains(&w.as_str()) || Some(&w) == me.as_ref()) {
                open = true;
                break;
            }
        }
        LocalFree(Some(HLOCAL(sd.0)));
        open
    }
}

/// The Claude Code settings fragment that runs `hook_exe claude` on every
/// event in `CLAUDE_HOOKS`.
pub fn claude_settings(hook_exe: &str) -> String {
    let mut cmd = String::new();
    escape_json(hook_exe, &mut cmd);
    let mut out = String::from("{\n  \"hooks\": {\n");
    for (i, (event, matcher)) in CLAUDE_HOOKS.iter().enumerate() {
        let matcher = match *matcher {
            "" => String::new(),
            m => format!("\"matcher\": \"{m}\", "),
        };
        let comma = if i + 1 < CLAUDE_HOOKS.len() { "," } else { "" };
        let _ = writeln!(
            out,
            "    \"{event}\": [{{ {matcher}\"hooks\": [{{ \"type\": \"command\", \
             \"command\": \"{cmd}\", \"args\": [\"claude\"], \"timeout\": 5 }}] }}]{comma}"
        );
    }
    out.push_str("  }\n}\n");
    out
}

/// A parsed JSON value. Objects keep their keys in order.
#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

/// Most arrays and objects nested in one another; deeper input is rejected
/// rather than risking the stack.
const MAX_DEPTH: usize = 128;

impl Json {
    /// Parses one JSON document. Returns `None` on malformed input.
    pub fn parse(s: &str) -> Option<Json> {
        let mut p = Parser { s, i: 0 };
        let v = p.value(0)?;
        p.ws();
        (p.i == s.len()).then_some(v)
    }

    /// The value under `key` when this is an object. With duplicate keys the
    /// last one wins, as in JavaScript.
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(m) => m.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }
}

struct Parser<'a> {
    s: &'a str,
    /// Byte offset; always on a char boundary.
    i: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.s.as_bytes().get(self.i).copied()
    }

    fn ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.i += 1;
        }
    }

    /// Skips whitespace, then consumes `b` if it comes next.
    fn eat(&mut self, b: u8) -> bool {
        self.ws();
        let hit = self.peek() == Some(b);
        if hit {
            self.i += 1;
        }
        hit
    }

    /// `depth` is the number of arrays and objects around the value.
    fn value(&mut self, depth: usize) -> Option<Json> {
        self.ws();
        let open = self.peek()?;
        if matches!(open, b'{' | b'[') && depth >= MAX_DEPTH {
            return None;
        }
        match open {
            b'{' => {
                self.i += 1;
                let mut m = Vec::new();
                if !self.eat(b'}') {
                    loop {
                        self.ws();
                        if self.peek() != Some(b'"') {
                            return None;
                        }
                        let k = self.string()?;
                        if !self.eat(b':') {
                            return None;
                        }
                        m.push((k, self.value(depth + 1)?));
                        if self.eat(b'}') {
                            break;
                        }
                        if !self.eat(b',') {
                            return None;
                        }
                    }
                }
                Some(Json::Obj(m))
            }
            b'[' => {
                self.i += 1;
                let mut a = Vec::new();
                if !self.eat(b']') {
                    loop {
                        a.push(self.value(depth + 1)?);
                        if self.eat(b']') {
                            break;
                        }
                        if !self.eat(b',') {
                            return None;
                        }
                    }
                }
                Some(Json::Arr(a))
            }
            b'"' => self.string().map(Json::Str),
            b't' => self.word("true", Json::Bool(true)),
            b'f' => self.word("false", Json::Bool(false)),
            b'n' => self.word("null", Json::Null),
            _ => self.number(),
        }
    }

    fn word(&mut self, w: &str, v: Json) -> Option<Json> {
        self.s[self.i..].starts_with(w).then(|| {
            self.i += w.len();
            v
        })
    }

    /// `-? (0 | [1-9][0-9]*) (. [0-9]+)? ([eE] [+-]? [0-9]+)?`, as JSON has
    /// it; Rust's float parser alone also takes `+1`, `01` and `.5`.
    fn number(&mut self) -> Option<Json> {
        let start = self.i;
        if self.peek() == Some(b'-') {
            self.i += 1;
        }
        if self.peek() == Some(b'0') {
            self.i += 1;
        } else if !self.digits() {
            return None;
        }
        if self.peek() == Some(b'.') {
            self.i += 1;
            if !self.digits() {
                return None;
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.i += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.i += 1;
            }
            if !self.digits() {
                return None;
            }
        }
        self.s[start..self.i].parse().ok().map(Json::Num)
    }

    /// Skips ASCII digits; true if there was at least one.
    fn digits(&mut self) -> bool {
        let start = self.i;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.i += 1;
        }
        self.i > start
    }

    /// Called on the opening quote.
    fn string(&mut self) -> Option<String> {
        self.i += 1;
        let mut out = String::new();
        loop {
            let rest = &self.s[self.i..];
            let n = rest.find(['"', '\\'])?;
            out.push_str(&rest[..n]);
            self.i += n + 1;
            if rest.as_bytes()[n] == b'"' {
                return Some(out);
            }
            let esc = self.peek()?;
            self.i += 1;
            out.push(match esc {
                b'"' => '"',
                b'\\' => '\\',
                b'/' => '/',
                b'b' => '\u{8}',
                b'f' => '\u{c}',
                b'n' => '\n',
                b'r' => '\r',
                b't' => '\t',
                b'u' => self.unicode_escape()?,
                // Also covers a non-ASCII byte, so `i` never stops inside
                // a char.
                _ => return None,
            });
        }
    }

    /// The `XXXX` of `\uXXXX`, joining a surrogate pair when the low half
    /// follows. A lone surrogate becomes U+FFFD.
    fn unicode_escape(&mut self) -> Option<char> {
        let hi = self.hex4()?;
        if (0xD800..0xDC00).contains(&hi) && self.s[self.i..].starts_with("\\u") {
            let save = self.i;
            self.i += 2;
            let lo = self.hex4()?;
            if (0xDC00..0xE000).contains(&lo) {
                return char::from_u32(0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00));
            }
            self.i = save;
        }
        Some(char::from_u32(hi).unwrap_or('\u{FFFD}'))
    }

    fn hex4(&mut self) -> Option<u32> {
        let h = self.s.get(self.i..self.i + 4)?;
        if !h.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        self.i += 4;
        u32::from_str_radix(h, 16).ok()
    }
}

/// Appends `s` to `out` as the inside of a JSON string literal.
pub fn escape_json(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attention::Ev;

    fn state(payload: &str) -> Option<(&'static str, String)> {
        claude_state(&Json::parse(payload).expect("valid test JSON"))
    }

    #[test]
    fn claude_events() {
        let cases: &[(&str, Option<(&str, &str)>)] = &[
            (
                r#"{"hook_event_name":"UserPromptSubmit","prompt":"hi"}"#,
                Some(("working", "")),
            ),
            (
                r#"{"hook_event_name":"PermissionRequest","tool_name":"Bash","tool_input":{"command":"git push"}}"#,
                Some(("needs-you", "Bash: git push")),
            ),
            (
                r#"{"hook_event_name":"PermissionRequest","tool_name":"Write","tool_input":{"file_path":"C:\\x\\a.rs"}}"#,
                Some(("needs-you", "Write: C:\\x\\a.rs")),
            ),
            (
                r#"{"hook_event_name":"PermissionRequest","tool_name":"mcp__x__y","tool_input":{"q":1}}"#,
                Some(("needs-you", "mcp__x__y")),
            ),
            (
                r#"{"hook_event_name":"PermissionRequest","tool_input":{"command":"git push"}}"#,
                Some(("needs-you", "git push")),
            ),
            (
                r#"{"hook_event_name":"PermissionRequest"}"#,
                Some(("needs-you", "")),
            ),
            (
                r#"{"hook_event_name":"PreToolUse","tool_name":"AskUserQuestion","tool_input":{"questions":[{"question":"Which one?"}]}}"#,
                Some(("needs-you", "Which one?")),
            ),
            (
                r#"{"hook_event_name":"PreToolUse","tool_name":"AskUserQuestion","tool_input":{}}"#,
                Some(("needs-you", "Question")),
            ),
            (
                r#"{"hook_event_name":"PreToolUse","tool_name":"ExitPlanMode"}"#,
                Some(("needs-you", "Plan ready")),
            ),
            (
                r#"{"hook_event_name":"PreToolUse","tool_name":"Bash"}"#,
                None,
            ),
            (
                r#"{"hook_event_name":"Notification","notification_type":"permission_prompt","message":"Claude needs your permission"}"#,
                Some(("needs-you", "Claude needs your permission")),
            ),
            (
                r#"{"hook_event_name":"Notification","notification_type":"agent_needs_input","message":"m"}"#,
                Some(("needs-you", "m")),
            ),
            (
                r#"{"hook_event_name":"Notification","notification_type":"idle_prompt","message":"waiting"}"#,
                None,
            ),
            // The settings only run the hook for the types it reports; a
            // notification without a type must have matched one.
            (
                r#"{"hook_event_name":"Notification","message":"m"}"#,
                Some(("needs-you", "m")),
            ),
            (
                r#"{"hook_event_name":"Notification","notification_type":"auth_success","message":"m"}"#,
                None,
            ),
            (
                r#"{"hook_event_name":"Stop","last_assistant_message":"\n  Done: tests pass.\nMore detail."}"#,
                Some(("done", "Done: tests pass.")),
            ),
            (r#"{"hook_event_name":"Stop"}"#, Some(("done", ""))),
            (
                r#"{"hook_event_name":"Stop","stop_hook_active":true}"#,
                None,
            ),
            (
                r#"{"hook_event_name":"Stop","stop_hook_active":false,"background_tasks":[{"id":"1"}]}"#,
                Some(("working", "")),
            ),
            (
                r#"{"hook_event_name":"Stop","background_tasks":[],"last_assistant_message":"ok"}"#,
                Some(("done", "ok")),
            ),
            (
                r#"{"hook_event_name":"StopFailure","error":"rate_limit"}"#,
                Some(("error", "rate_limit")),
            ),
            // Not a string: still an error, without a message.
            (
                r#"{"hook_event_name":"StopFailure","error":{"type":"x"}}"#,
                Some(("error", "")),
            ),
            (
                r#"{"hook_event_name":"SessionEnd","reason":"clear"}"#,
                Some(("idle", "")),
            ),
            (
                r#"{"hook_event_name":"PostToolUse","tool_name":"Bash"}"#,
                None,
            ),
            (r#"{"prompt":"no event name"}"#, None),
            (r#"[1,2]"#, None),
        ];
        for (payload, want) in cases {
            let got = state(payload);
            let got = got.as_ref().map(|(s, m)| (*s, m.as_str()));
            assert_eq!(got, *want, "{payload}");
        }
    }

    #[test]
    fn settings_snippet() {
        let exe = r"C:\Program Files\blitz\blitz-hook.exe";
        let v = Json::parse(&claude_settings(exe)).expect("snippet is valid JSON");
        let hooks = v.get("hooks").unwrap();
        for (event, matcher) in CLAUDE_HOOKS {
            let Some(Json::Arr(groups)) = hooks.get(event) else {
                panic!("{event} missing");
            };
            assert_eq!(groups.len(), 1, "{event}");
            let group = &groups[0];
            let want_matcher = (!matcher.is_empty()).then(|| Json::Str(matcher.into()));
            assert_eq!(group.get("matcher"), want_matcher.as_ref(), "{event}");
            let Some(Json::Arr(cmds)) = group.get("hooks") else {
                panic!("{event} has no hooks");
            };
            let cmd = &cmds[0];
            assert_eq!(cmd.get("type").and_then(Json::as_str), Some("command"));
            assert_eq!(cmd.get("command").and_then(Json::as_str), Some(exe));
            assert_eq!(
                cmd.get("args"),
                Some(&Json::Arr(vec![Json::Str("claude".into())]))
            );
            assert_eq!(cmd.get("timeout"), Some(&Json::Num(5.0)));
        }
    }

    #[test]
    fn setup_needs_an_app() {
        assert_eq!(setup(&[]), 2);
        assert_eq!(setup(&["vim".into()]), 2);
    }

    /// As long as a real pane token: 128 bits in hex.
    const TOKEN: &str = "4b1d0123456789abcdef0123456789ab";
    const SESSION: &str = "0b8f6a3e-1c2d-4e5f-9a7b-3c4d5e6f7a8b";

    #[test]
    fn every_state_is_an_attention_event() {
        for s in ["working", "needs-you", "done", "error", "idle"] {
            let title = format!("blitz:{TOKEN}:{s}");
            assert!(Ev::from_notify(&title, TOKEN).is_some(), "{s}");
            let title = format!("blitz:{TOKEN}:{s}:{SESSION}");
            assert_eq!(
                Ev::from_notify(&title, TOKEN).and_then(|(_, id)| id),
                Some(SESSION),
                "{s}"
            );
        }
    }

    #[test]
    fn output_is_one_json_line() {
        assert_eq!(
            notify_json(TOKEN, "done", None, "All \"good\" \\ ok"),
            format!(
                "{{\"terminalSequence\":\"\\u001b]777;notify;blitz:{TOKEN}:done;All \\\"good\\\" \\\\ ok\\u0007\"}}\n"
            )
        );
        let out = claude_output(TOKEN, r#"{"hook_event_name":"SessionEnd"}"#).unwrap();
        let v = Json::parse(&out).unwrap();
        assert_eq!(
            v.get("terminalSequence").and_then(Json::as_str),
            Some(format!("\x1b]777;notify;blitz:{TOKEN}:idle;\x07").as_str())
        );
        assert_eq!(claude_output(TOKEN, "not json"), None);
    }

    /// Payloads as other writers may send them: pretty-printed with CRLF
    /// line ends, or behind a byte-order mark.
    #[test]
    fn output_for_pretty_and_bom_payloads() {
        let want = Some(format!(
            "{{\"terminalSequence\":\"\\u001b]777;notify;blitz:{TOKEN}:done;ok\\u0007\"}}\n"
        ));
        let pretty = "{\r\n  \"hook_event_name\": \"Stop\",\r\n  \"last_assistant_message\": \"ok\"\r\n}\r\n";
        assert_eq!(claude_output(TOKEN, pretty), want);
        assert_eq!(claude_output(TOKEN, &format!("\u{feff}{pretty}")), want);
        assert_eq!(claude_output(TOKEN, "\u{feff}"), None);
    }

    #[test]
    fn output_carries_the_session_id() {
        let seq = |payload: &str| {
            let out = claude_output(TOKEN, payload).unwrap();
            let v = Json::parse(&out).unwrap();
            v.get("terminalSequence")
                .and_then(Json::as_str)
                .unwrap()
                .to_owned()
        };
        assert_eq!(
            seq(&format!(
                r#"{{"hook_event_name":"Stop","session_id":"{SESSION}","last_assistant_message":"ok"}}"#
            )),
            format!("\x1b]777;notify;blitz:{TOKEN}:done:{SESSION};ok\x07")
        );
        // A bad id is dropped; the state still gets through.
        assert_eq!(
            seq(r#"{"hook_event_name":"SessionEnd","session_id":"x;rm -rf ~"}"#),
            format!("\x1b]777;notify;blitz:{TOKEN}:idle;\x07")
        );
        assert_eq!(
            seq(r#"{"hook_event_name":"SessionEnd","session_id":7}"#),
            format!("\x1b]777;notify;blitz:{TOKEN}:idle;\x07")
        );
    }

    #[test]
    fn session_ids() {
        assert!(is_session_id(SESSION));
        assert!(is_session_id(&SESSION.to_uppercase()));
        for bad in [
            "",
            &SESSION[1..],
            &format!("{SESSION}0"),
            &SESSION.replacen('0', ";", 1),
            &SESSION.replacen('0', " ", 1),
            &SESSION.replacen('0', "\"", 1),
            &SESSION.replacen('0', "'", 1),
            &SESSION.replacen('0', "`", 1),
            &SESSION.replacen('0', "\n", 1),
            &SESSION.replacen('0', "$", 1),
            &SESSION.replacen('0', "g", 1),
            // Same byte length, but not ASCII.
            &SESSION.replacen("0b", "é", 1),
        ] {
            assert!(!is_session_id(bad), "{bad:?}");
        }
    }

    #[test]
    fn message_cannot_break_the_sequence() {
        let evil = "a\x1b]0;pwned\x07b\u{9c}c\r\nd\te\x00f";
        assert_eq!(one_line(evil), "a]0;pwnedbc d ef");
        assert_eq!(one_line("  lots   of\n\n space  "), "lots of space");
        assert_eq!(one_line(" \x1b \x07 "), "");
    }

    #[test]
    fn message_is_capped() {
        let long = "é".repeat(500);
        let m = one_line(&long);
        assert_eq!(m.chars().count(), MAX_MSG);
        assert!(m.ends_with('…'));
        let exact = "x".repeat(MAX_MSG);
        assert_eq!(one_line(&exact), exact);
    }

    #[test]
    fn json_values() {
        let v = Json::parse(
            r#" {"a": [1, -2.5e3, true, false, null], "b": {"c": "d"}, "e": [], "f": {}} "#,
        )
        .unwrap();
        assert_eq!(
            v.get("a"),
            Some(&Json::Arr(vec![
                Json::Num(1.0),
                Json::Num(-2500.0),
                Json::Bool(true),
                Json::Bool(false),
                Json::Null,
            ]))
        );
        assert_eq!(
            v.get("b").and_then(|b| b.get("c")),
            Some(&Json::Str("d".into()))
        );
        assert_eq!(v.get("e"), Some(&Json::Arr(vec![])));
        assert_eq!(v.get("f"), Some(&Json::Obj(vec![])));
        assert_eq!(v.get("zz"), None);
        assert_eq!(Json::parse("\"x\"").unwrap().get("x"), None);
    }

    #[test]
    fn json_string_escapes() {
        let v = Json::parse(r#""q\" s\\ /\/ \b\f\n\r\t \u00e9 \ud83d\ude00 \ud800x é""#).unwrap();
        assert_eq!(
            v.as_str(),
            Some("q\" s\\ // \u{8}\u{c}\n\r\t é 😀 \u{FFFD}x é")
        );
    }

    /// A high surrogate not followed by a low one stands alone, and what
    /// follows is read on its own.
    #[test]
    fn json_lone_surrogates() {
        for (lit, want) in [
            (r#""\ud83dA""#, "\u{FFFD}A"),
            (r#""\ud83d😀""#, "\u{FFFD}\u{1F600}"),
            (r#""\ude00\ud83d""#, "\u{FFFD}\u{FFFD}"),
            (r#""😀""#, "\u{1F600}"),
        ] {
            assert_eq!(Json::parse(lit).unwrap().as_str(), Some(want), "{lit}");
        }
        assert_eq!(Json::parse(r#""\ud83d\u00""#), None);
    }

    #[test]
    fn json_numbers_follow_the_grammar() {
        for (lit, want) in [
            ("0", 0.0),
            ("-0", -0.0),
            ("10", 10.0),
            ("1.5", 1.5),
            ("-2.5e3", -2500.0),
            ("1E2", 100.0),
            ("1e-2", 0.01),
            ("1e+2", 100.0),
            ("0.25", 0.25),
        ] {
            assert_eq!(Json::parse(lit), Some(Json::Num(want)), "{lit}");
        }
        for bad in [
            "+1", "01", "-01", ".5", "1.", "1.e5", "1e", "1e+", "-", "--1", "1.2.3", "1ee2",
            "0x10", "[01]", "[.5]", "Infinity", "NaN",
        ] {
            assert_eq!(Json::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn json_last_duplicate_key_wins() {
        let v = Json::parse(r#"{"k": 1, "k": 2}"#).unwrap();
        assert_eq!(v.get("k"), Some(&Json::Num(2.0)));
    }

    #[test]
    fn json_rejects_malformed() {
        for bad in [
            "",
            " ",
            "{",
            "}",
            "[1,]",
            "[1 2]",
            "{\"a\"}",
            "{\"a\":}",
            "{a:1}",
            "\"open",
            "\"bad \\q\"",
            "\"\\u12\"",
            "\"\\u12G4\"",
            "tru",
            "nul",
            "1 2",
            "-",
            "[\"\\é\"]",
        ] {
            assert_eq!(Json::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn json_depth_is_capped() {
        let arrays = |n: usize| format!("{}1{}", "[".repeat(n), "]".repeat(n));
        let objects = |n: usize| format!("{}1{}", r#"{"a":"#.repeat(n), "}".repeat(n));
        assert!(Json::parse(&arrays(MAX_DEPTH)).is_some());
        assert_eq!(Json::parse(&arrays(MAX_DEPTH + 1)), None);
        assert!(Json::parse(&objects(MAX_DEPTH)).is_some());
        assert_eq!(Json::parse(&objects(MAX_DEPTH + 1)), None);
        let empty = |n: usize| format!("{}{}", "[".repeat(n), "]".repeat(n));
        assert!(Json::parse(&empty(MAX_DEPTH)).is_some());
        assert_eq!(Json::parse(&empty(MAX_DEPTH + 1)), None);
        let deep = "[".repeat(100_000);
        assert_eq!(Json::parse(&deep), None);
    }

    #[test]
    fn json_never_panics_on_prefixes() {
        let doc = r#"{"hook_event_name":"Stop","x":[1,{"y":"\u00e9\ud83d\ude00 é\n"}],"z":null}"#;
        for (i, _) in doc.char_indices() {
            let _ = Json::parse(&doc[..i]);
            let _ = Json::parse(&doc[i..]);
        }
        assert!(Json::parse(doc).is_some());
    }

    #[test]
    fn escape_round_trips() {
        let s = "a\"b\\c\x1b]777;\x07\n\u{9b}é😀";
        let mut lit = String::from("\"");
        escape_json(s, &mut lit);
        lit.push('"');
        assert!(lit.contains("\\u001b") && lit.contains("\\u0007") && lit.contains("\\u009b"));
        assert_eq!(Json::parse(&lit).unwrap().as_str(), Some(s));
    }

    #[cfg(windows)]
    #[test]
    fn setup_notices_folders_others_can_change() {
        let dir = std::env::temp_dir().join(format!("blitz-acl-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let icacls = |args: &[&str]| {
            let out = std::process::Command::new("icacls")
                .arg(&dir)
                .args(args)
                .output()
                .expect("icacls");
            assert!(out.status.success(), "icacls {args:?}");
        };
        // `"DOMAIN\user","S-1-5-..."`
        let whoami = std::process::Command::new("whoami")
            .args(["/user", "/fo", "csv", "/nh"])
            .output()
            .expect("whoami");
        let me = String::from_utf8_lossy(&whoami.stdout)
            .trim()
            .rsplit(',')
            .next()
            .expect("a SID")
            .trim_matches('"')
            .to_owned();
        assert!(me.starts_with("S-1-"), "{me}");
        let mut seen = Vec::new();
        for (grant, others) in [
            // Only SYSTEM; as the owner this process can still read and
            // change the list.
            ("*S-1-5-18:(OI)(CI)F", false),
            // The user installing it, the common per-user install.
            (&*format!("*{me}:(OI)(CI)M"), false),
            ("*S-1-5-32-544:(OI)(CI)F", false),
            ("*S-1-1-0:(OI)(CI)RX", false),
            // For files created inside, not for the folder itself.
            ("*S-1-1-0:(OI)(CI)(IO)M", false),
            ("*S-1-5-32-545:(OI)(CI)M", true),
        ] {
            let args: &[&str] = match seen.is_empty() {
                true => &["/inheritance:r", "/grant:r", grant],
                false => &["/grant", grant],
            };
            icacls(args);
            seen.push((grant.to_owned(), others_can_write(&dir), others));
        }
        let _ = std::fs::remove_dir(&dir);
        for (grant, got, want) in seen {
            assert_eq!(got, want, "after {grant}");
        }
    }
}
