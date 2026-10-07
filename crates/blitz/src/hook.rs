//! `blitz-hook claude` reports Claude Code's state to the pane it runs in.
//! blitz has Claude Code load the hooks as a plugin; `blitz setup claude`
//! prints them for a Claude Code that does not.

use std::fmt::Write as _;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

/// Most payloads are a few KB, but a Write or Edit request carries the
/// whole file, so the cap only stops a runaway stream. A payload cut at the
/// cap does not parse and reports nothing.
const MAX_INPUT: u64 = 64 << 20;
/// Longest message carried in a notification, in chars.
const MAX_MSG: usize = 120;
/// What blitz shows for a claude.ai usage limit. The turn is over until
/// the limit resets, which is not a failure.
const USAGE_LIMIT: &str = "usage limit";
/// The version of the titles blitz-hook writes, stamped at their end as
/// `:v2`; titles without one are version 1. blitz can then tell a hook
/// older than itself, which settings pasted long ago may still run.
pub const PROTOCOL: u32 = 2;

/// Entry point of `blitz-hook`. Always returns 0, so a hook can never block
/// the agent that runs it.
///
/// `blitz-hook claude` reads a hook payload on stdin and prints a
/// `terminalSequence` for Claude Code to write to its own terminal, so the
/// state lands in the right pane without any IPC. The sequence carries the
/// pane's `BLITZ_PANE_TOKEN`, which program output cannot know. Outside a
/// blitz pane (no token) it prints nothing.
///
/// `blitz-hook notify <state> [message]` is for other agents' hooks, which
/// cannot hand their terminal a sequence: it writes one to the console
/// the pane gave it.
pub fn run() -> i32 {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let token = std::env::var("BLITZ_PANE_TOKEN").unwrap_or_default();
    // It goes into the sequence as is, so nothing in it may end the title.
    let in_pane = !token.is_empty() && token.bytes().all(|b| b.is_ascii_alphanumeric());
    if args.first().is_some_and(|a| a == "notify") {
        match notify_output(&token, &args[1..]) {
            Some(seq) if in_pane => {
                let _ = write_console(&seq);
            }
            Some(_) => {}
            None => eprintln!("usage: blitz-hook notify {} [message]", STATES.join("|")),
        }
    }
    if args.first().is_some_and(|a| a == "claude") {
        // Read all of it, even outside a pane or past the cap: Claude Code
        // writes the whole payload, and exiting first would break the pipe
        // under it.
        let mut input = Vec::new();
        let _ = std::io::stdin().take(MAX_INPUT).read_to_end(&mut input);
        let _ = std::io::copy(&mut std::io::stdin(), &mut std::io::sink());
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

/// The states `blitz-hook notify` takes.
const STATES: [&str; 5] = ["working", "needs-you", "done", "error", "idle"];

/// What `blitz-hook notify <state> [message]` writes to its pane, or `None`
/// for a state blitz does not know. A message that is a JSON object is the
/// notification Codex adds after the arguments it was given, and its last
/// reply shows.
pub fn notify_output(token: &str, args: &[String]) -> Option<String> {
    let state = args.first().filter(|s| STATES.contains(&s.as_str()))?;
    let msg = args.get(1).map_or("", String::as_str);
    let msg = match Json::parse(msg) {
        Some(j @ Json::Obj(_)) => {
            let reply = j.get("last-assistant-message").and_then(Json::as_str);
            reply_line(reply.unwrap_or("")).to_owned()
        }
        _ => msg.to_owned(),
    };
    Some(notify_seq(token, state, None, &msg))
}

/// Writes `seq` to the console this process shares with its pane, past
/// the pipe an agent reads its hooks' output from.
#[cfg(windows)]
fn write_console(seq: &str) -> io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Console::{
        CONSOLE_MODE, ENABLE_VIRTUAL_TERMINAL_PROCESSING, GetConsoleMode, SetConsoleMode,
        WriteConsoleW,
    };
    let con = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("CONOUT$")?;
    let h = HANDLE(con.as_raw_handle());
    let wide: Vec<u16> = seq.encode_utf16().collect();
    let mut mode = CONSOLE_MODE::default();
    // SAFETY: a console handle that `con` keeps open. The console passes
    // the sequence on only while it reads escapes; the agent's own mode is
    // put back after.
    unsafe {
        GetConsoleMode(h, &mut mode)?;
        SetConsoleMode(h, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING)?;
        let wrote = WriteConsoleW(h, &wide, None, None);
        let _ = SetConsoleMode(h, mode);
        wrote?;
    }
    Ok(())
}

#[cfg(not(windows))]
fn write_console(seq: &str) -> io::Result<()> {
    let mut tty = std::fs::OpenOptions::new().write(true).open("/dev/tty")?;
    tty.write_all(seq.as_bytes())
}

/// The hook's stdout for one Claude Code payload, or `None` when the event
/// is not one blitz reports. A byte-order mark in front is skipped.
pub fn claude_output(token: &str, payload: &str) -> Option<String> {
    let ev = Json::parse(payload.strip_prefix('\u{feff}').unwrap_or(payload))?;
    let (state, msg) = claude_state(&ev)?;
    // A new or cleared session has nothing to resume before its first
    // prompt.
    let resumable = state != "ready"
        || matches!(
            ev.get("source").and_then(Json::as_str),
            Some("resume" | "fork")
        );
    let session = ev
        .get("session_id")
        .and_then(Json::as_str)
        .filter(|id| resumable && is_session_id(id));
    Some(notify_json(token, state, session, &msg))
}

/// Whether `id` looks like a Claude Code session id: 36 hex digits and
/// dashes. blitz later types the id into a shell, so anything else is
/// dropped.
pub fn is_session_id(id: &str) -> bool {
    id.len() == 36 && id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
}

/// Maps a hook payload to a state (`working`, `needs-you`, `done`, `error`,
/// `idle`, or `ready`, which changes none) and a one-line message.
pub fn claude_state(ev: &Json) -> Option<(&'static str, String)> {
    fn field<'a>(v: &'a Json, k: &str) -> &'a str {
        v.get(k).and_then(Json::as_str).unwrap_or("")
    }
    let tool = field(ev, "tool_name");
    let input = ev.get("tool_input").unwrap_or(&Json::Null);
    Some(match field(ev, "hook_event_name") {
        // Claude Code is up and its hooks report.
        "SessionStart" => ("ready", String::new()),
        // What the user asked, so the sidebar says what the turn is about.
        "UserPromptSubmit" => ("working", field(ev, "prompt").to_owned()),
        "PermissionRequest" => {
            let detail = match field(input, "file_path") {
                "" => (["command", "url", "pattern", "query"].into_iter())
                    .map(|k| field(input, k))
                    .find(|d| !d.is_empty())
                    .unwrap_or(""),
                path => relative(path, field(ev, "cwd")),
            };
            let tool = tool_label(tool);
            let msg = match (tool.as_str(), detail) {
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
        // The hook runs for every type, so a type Claude Code adds later is
        // sorted here, in what ships with blitz, not in pasted settings.
        // `idle_prompt` is left out: it fires a minute after every turn.
        "Notification" => match field(ev, "notification_type") {
            "permission_prompt"
            | "elicitation_dialog"
            | "elicitation_url_dialog"
            | "agent_needs_input"
            // A usage limit reset while the computer slept; Claude waits
            // for Enter.
            | "quota_auto_resume_stale" => ("needs-you", field(ev, "message").to_owned()),
            "quota_auto_resume_fired" => ("working", String::new()),
            "quota_auto_resume_disabled" => ("done", USAGE_LIMIT.to_owned()),
            _ => return None,
        },
        "Stop" => {
            // A Stop hook made Claude continue; the turn is not over.
            if ev.get("stop_hook_active") == Some(&Json::Bool(true)) {
                return None;
            }
            let tasks = match ev.get("background_tasks") {
                Some(Json::Arr(tasks)) => &tasks[..],
                _ => &[],
            };
            // Agents, here or in the cloud, report back and wake Claude
            // up; a shell, a server or a monitor started in the background
            // may run for hours.
            let agents = (tasks.iter())
                .filter(|t| {
                    matches!(
                        field(t, "type"),
                        "subagent" | "workflow" | "teammate" | "cloud session"
                    )
                })
                .count();
            if agents > 0 {
                let s = if agents == 1 { "" } else { "s" };
                return Some(("working", format!("waiting on {agents} agent{s}")));
            }
            let line = reply_line(field(ev, "last_assistant_message"));
            let msg = match (tasks.len(), line) {
                (0, line) => line.to_owned(),
                (n, "") => format!("{n} background"),
                (n, line) => {
                    let more = format!(" \u{b7} {n} background");
                    one_line_max(line, MAX_MSG - more.chars().count()) + &more
                }
            };
            ("done", msg)
        }
        // Error codes in words: `server_error` reads "server error".
        "StopFailure" => match field(ev, "error") {
            "rate_limit" => ("done", USAGE_LIMIT.to_owned()),
            "max_output_tokens" => ("error", "reply too long".to_owned()),
            "unknown" => ("error", "unknown error".to_owned()),
            e => ("error", e.replace('_', " ")),
        },
        "SessionEnd" => ("idle", String::new()),
        _ => return None,
    })
}

/// The line of a reply to show: its first, or its last when the reply ends
/// by asking something.
fn reply_line(reply: &str) -> &str {
    let mut lines = reply.lines().map(str::trim).filter(|l| !l.is_empty());
    let first = lines.next().unwrap_or("");
    match lines.next_back() {
        Some(last) if last.ends_with('?') => last,
        _ => first,
    }
}

/// A tool as people read it: `mcp__github__create_issue` is
/// `github: create_issue`.
fn tool_label(tool: &str) -> String {
    match tool.strip_prefix("mcp__").and_then(|t| t.split_once("__")) {
        Some((server, t)) => format!("{server}: {t}"),
        None => tool.to_owned(),
    }
}

/// `path` relative to `cwd` when it is inside it, so the end of a long
/// path is what shows. Case is ignored, as Windows does.
fn relative<'a>(path: &'a str, cwd: &str) -> &'a str {
    let cwd = cwd.trim_end_matches(['\\', '/']);
    let inside = (path.get(..cwd.len()))
        .filter(|head| !cwd.is_empty() && head.eq_ignore_ascii_case(cwd))
        .and_then(|_| path[cwd.len()..].strip_prefix(['\\', '/']));
    inside.filter(|rest| !rest.is_empty()).unwrap_or(path)
}

/// `ESC]777;notify;blitz:<token>:<state>:v<PROTOCOL>;<msg>BEL`, with
/// `:<session>` after the state when there is one. The message is made
/// safe to embed first; the session must already pass `is_session_id`.
pub fn notify_seq(token: &str, state: &str, session: Option<&str>, msg: &str) -> String {
    let session = session.map(|s| format!(":{s}")).unwrap_or_default();
    format!(
        "\x1b]777;notify;blitz:{token}:{state}{session}:v{PROTOCOL};{}\x07",
        one_line(msg)
    )
}

/// [`notify_seq`] as hook output, `{"terminalSequence":"..."}` and a
/// newline, for Claude Code to write to its terminal.
pub fn notify_json(token: &str, state: &str, session: Option<&str>, msg: &str) -> String {
    let mut out = String::from("{\"terminalSequence\":\"");
    escape_json(&notify_seq(token, state, session, msg), &mut out);
    out.push_str("\"}\n");
    out
}

/// Drops control characters (which could end the OSC early), folds runs of
/// whitespace into one space, and caps the length at `MAX_MSG` chars.
pub fn one_line(s: &str) -> String {
    one_line_max(s, MAX_MSG)
}

/// [`one_line`], capped at `max` chars.
fn one_line_max(s: &str, max: usize) -> String {
    let words: Vec<String> = s
        .split_whitespace()
        .map(|w| w.replace(char::is_control, ""))
        .filter(|w| !w.is_empty())
        .collect();
    let line = words.join(" ");
    if line.chars().count() <= max {
        return line;
    }
    let mut cut: String = line.chars().take(max.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

/// Claude Code events blitz hooks, with the matcher each one needs.
const CLAUDE_HOOKS: [(&str, &str); 8] = [
    ("SessionStart", ""),
    ("UserPromptSubmit", ""),
    ("PermissionRequest", ""),
    ("PreToolUse", "^(AskUserQuestion|ExitPlanMode)$"),
    ("Notification", ""),
    ("Stop", ""),
    ("StopFailure", ""),
    ("SessionEnd", ""),
];

/// The `blitz-hook` next to this exe.
pub fn hook_exe() -> io::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    Ok(exe.with_file_name(format!("blitz-hook{}", std::env::consts::EXE_SUFFIX)))
}

const PLUGIN_JSON: &str = concat!(
    "{\n  \"name\": \"blitz\",\n  \"version\": \"",
    env!("CARGO_PKG_VERSION"),
    "\",\n  \"description\": \"Tells blitz what each Claude Code session is doing\",\n  \
     \"author\": { \"name\": \"blitz contributors\" }\n}\n"
);

/// Writes blitz's Claude Code plugin to `dir`: a manifest, and in
/// `hooks/hooks.json` the hooks of [`claude_settings`], running `hook_exe`.
/// Claude Code 2.1.280 and later load it in every pane through
/// `CLAUDE_CODE_PLUGIN_DIRS`, so nothing needs pasting into its settings.
/// A file that already holds what it should is left alone, so most starts
/// write nothing, and one that changes is swapped in whole, so a Claude
/// Code starting meanwhile never reads half of it.
pub fn write_plugin(dir: &Path, hook_exe: &str) -> io::Result<()> {
    for (name, text) in [
        (".claude-plugin/plugin.json", PLUGIN_JSON.to_owned()),
        ("hooks/hooks.json", claude_settings(hook_exe)),
    ] {
        let path = dir.join(name);
        if std::fs::read(&path).is_ok_and(|old| old == text.as_bytes()) {
            continue;
        }
        std::fs::create_dir_all(path.parent().unwrap_or(dir))?;
        let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
        let swapped = std::fs::write(&tmp, text).and_then(|()| std::fs::rename(&tmp, &path));
        if swapped.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        swapped?;
    }
    Ok(())
}

/// Writes the plugin where this blitz keeps its state, for the blitz-hook
/// next to it, and returns its folder; `None` when that cannot be done.
pub fn install_plugin() -> Option<PathBuf> {
    plugin_for(&hook_exe().ok()?, &crate::session::dir()?)
}

/// [`install_plugin`] for the blitz-hook at `hook`, under `state`. Each
/// blitz-hook has a folder of its own, so a second copy of blitz, such as
/// a portable one or a release build, never points the panes of the first
/// at its own. None for a blitz-hook other users can replace, which Claude
/// Code would run in every session.
fn plugin_for(hook: &Path, state: &Path) -> Option<PathBuf> {
    if !hook.is_file() || exposed(hook) {
        return None;
    }
    let hook = hook.to_string_lossy();
    let dir = plugin_dir(state, &hook);
    match write_plugin(&dir, &hook) {
        Ok(()) => Some(dir),
        Err(e) => {
            eprintln!("blitz: writing the Claude Code plugin: {e}");
            None
        }
    }
}

/// The plugin folder under `state` for the blitz-hook at `hook`.
fn plugin_dir(state: &Path, hook: &str) -> PathBuf {
    // FNV-1a, which stays the same from one Rust release to the next.
    let id = (hook.to_lowercase().bytes()).fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    });
    state.join(format!("claude-plugin-{id:016x}"))
}

/// Whether others may replace the blitz-hook at `hook`, or add one in its
/// folder.
#[cfg(windows)]
fn exposed(hook: &Path) -> bool {
    [hook.parent(), Some(hook)]
        .into_iter()
        .flatten()
        .any(others_can_write)
}

#[cfg(not(windows))]
fn exposed(_hook: &Path) -> bool {
    false
}

/// `CLAUDE_CODE_PLUGIN_DIRS` for a pane: the folders blitz itself was
/// given, if any, and blitz's plugin once.
pub fn plugin_dirs(inherited: Option<&str>, ours: &str) -> String {
    let mut dirs: Vec<&str> = (inherited.unwrap_or("").split(';'))
        .filter(|d| !d.is_empty())
        .collect();
    if !dirs.iter().any(|d| d.eq_ignore_ascii_case(ours)) {
        dirs.push(ours);
    }
    dirs.join(";")
}

/// `blitz setup <app>`. Returns the process exit code.
///
/// `blitz setup claude` says that blitz loads its hooks itself, and prints
/// them, pointing at the `blitz-hook` next to this exe, for a Claude Code
/// that does not load them. It never edits the settings file itself; that
/// file belongs to the user.
pub fn setup(args: &[String]) -> i32 {
    let wsl = match args {
        [app] if app == "claude" => false,
        [app, flag] if app == "claude" && flag == "--wsl" => true,
        _ => {
            eprintln!("usage: blitz setup claude [--wsl]");
            return 2;
        }
    };
    let hook = match hook_exe() {
        Ok(hook) => hook,
        Err(e) => {
            eprintln!("blitz setup: cannot find blitz itself: {e}");
            return 1;
        }
    };
    if !hook.is_file() {
        eprintln!(
            "warning: {} is missing; keep it next to blitz",
            hook.display()
        );
    }
    if exposed(&hook) {
        eprintln!(
            "warning: other users can replace {}, and Claude Code would run their \
             program in every session. Keep blitz in a folder only you can change, \
             such as where the installer puts it.",
            hook.display()
        );
    }
    // Claude Code in WSL runs the Windows exe; blitz passes the pane's
    // token into WSL and back.
    if wsl {
        let Some(path) = wsl_path(&hook.to_string_lossy()) else {
            eprintln!("blitz setup: WSL cannot reach {}", hook.display());
            return 1;
        };
        eprintln!(
            "Merge the \"hooks\" below into ~/.claude/settings.json inside WSL.\n\
             Claude Code picks the change up without a restart.\n"
        );
        print!("{}", claude_settings(&path));
        return 0;
    }
    let settings = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::home_dir().map(|h| h.join(".claude")))
        .unwrap_or_default()
        .join("settings.json");
    eprintln!(
        "blitz loads these hooks itself in its panes, with Claude Code 2.1.280 or\n\
         later: there is nothing to set up. An older Claude Code, or settings that\n\
         turn off plugin folders, need them merged into {}.\n\
         Claude Code picks the change up without a restart.\n",
        settings.display()
    );
    if runs_blitz_hook(&settings) {
        eprintln!(
            "note: {} runs blitz-hook already. With Claude Code 2.1.280 or later\n\
             blitz's own hooks report each event too; remove those to report once.\n",
            settings.display()
        );
    }
    print!("{}", claude_settings(&hook.to_string_lossy()));
    0
}

/// `C:\x\y` where WSL mounts it, `/mnt/c/x/y`; `None` for a path not on a
/// drive letter.
fn wsl_path(path: &str) -> Option<String> {
    let path = path.strip_prefix(r"\\?\").unwrap_or(path);
    let drive = path.chars().next().filter(char::is_ascii_alphabetic)?;
    let rest = path.get(1..)?.strip_prefix(r":\")?;
    let drive = drive.to_ascii_lowercase();
    Some(format!("/mnt/{drive}/{}", rest.replace('\\', "/")))
}

/// Whether the Claude Code settings file at `path` runs blitz-hook.
fn runs_blitz_hook(path: &Path) -> bool {
    std::fs::read_to_string(path).is_ok_and(|s| s.to_ascii_lowercase().contains("blitz-hook"))
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
        // A timeout on SessionEnd would raise the time every Claude Code
        // session may take to exit, from 1.5 s to that.
        let timeout = match *event {
            "SessionEnd" => "",
            _ => ", \"timeout\": 5",
        };
        let comma = if i + 1 < CLAUDE_HOOKS.len() { "," } else { "" };
        let _ = writeln!(
            out,
            "    \"{event}\": [{{ {matcher}\"hooks\": [{{ \"type\": \"command\", \
             \"command\": \"{cmd}\", \"args\": [\"claude\"]{timeout} }}] }}]{comma}"
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
    use crate::attention::{Ev, notify_protocol};

    fn state(payload: &str) -> Option<(&'static str, String)> {
        claude_state(&Json::parse(payload).expect("valid test JSON"))
    }

    #[test]
    fn claude_events() {
        let cases: &[(&str, Option<(&str, &str)>)] = &[
            (
                r#"{"hook_event_name":"SessionStart","source":"startup"}"#,
                Some(("ready", "")),
            ),
            (
                r#"{"hook_event_name":"UserPromptSubmit","prompt":"hi"}"#,
                Some(("working", "hi")),
            ),
            (
                r#"{"hook_event_name":"UserPromptSubmit"}"#,
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
            // Inside the session's folder the path is relative to it.
            (
                r#"{"hook_event_name":"PermissionRequest","cwd":"C:\\Work\\app","tool_name":"Edit","tool_input":{"file_path":"c:\\work\\APP\\src\\main.rs"}}"#,
                Some(("needs-you", "Edit: src\\main.rs")),
            ),
            (
                r#"{"hook_event_name":"PermissionRequest","cwd":"C:\\work\\app","tool_name":"Edit","tool_input":{"file_path":"C:\\work\\apple\\a.rs"}}"#,
                Some(("needs-you", "Edit: C:\\work\\apple\\a.rs")),
            ),
            (
                r#"{"hook_event_name":"PermissionRequest","tool_name":"mcp__x__y","tool_input":{"q":1}}"#,
                Some(("needs-you", "x: y")),
            ),
            (
                r#"{"hook_event_name":"PermissionRequest","tool_name":"mcp__github__create_issue","tool_input":{"title":"t"}}"#,
                Some(("needs-you", "github: create_issue")),
            ),
            (
                r#"{"hook_event_name":"PermissionRequest","tool_name":"WebFetch","tool_input":{"url":"https://example.com/a","prompt":"p"}}"#,
                Some(("needs-you", "WebFetch: https://example.com/a")),
            ),
            (
                r#"{"hook_event_name":"PermissionRequest","tool_name":"Grep","tool_input":{"pattern":"fn main","path":"src"}}"#,
                Some(("needs-you", "Grep: fn main")),
            ),
            (
                r#"{"hook_event_name":"PermissionRequest","tool_name":"WebSearch","tool_input":{"query":"conpty osc"}}"#,
                Some(("needs-you", "WebSearch: conpty osc")),
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
            // The hook runs for every notification, so one it cannot place
            // says nothing.
            (r#"{"hook_event_name":"Notification","message":"m"}"#, None),
            (
                r#"{"hook_event_name":"Notification","notification_type":"auth_success","message":"m"}"#,
                None,
            ),
            (
                r#"{"hook_event_name":"Notification","notification_type":"elicitation_complete","message":"m"}"#,
                None,
            ),
            (
                r#"{"hook_event_name":"Notification","notification_type":"agent_completed","message":"m"}"#,
                None,
            ),
            // Waiting out a usage limit: over when Claude continues, done
            // when it gives up, and Enter after a long sleep.
            (
                r#"{"hook_event_name":"Notification","notification_type":"quota_auto_resume_fired","message":"Continuing"}"#,
                Some(("working", "")),
            ),
            (
                r#"{"hook_event_name":"Notification","notification_type":"quota_auto_resume_disabled","message":"m"}"#,
                Some(("done", "usage limit")),
            ),
            (
                r#"{"hook_event_name":"Notification","notification_type":"quota_auto_resume_stale","message":"Press Enter to continue"}"#,
                Some(("needs-you", "Press Enter to continue")),
            ),
            (
                r#"{"hook_event_name":"Stop","last_assistant_message":"\n  Done: tests pass.\nMore detail."}"#,
                Some(("done", "Done: tests pass.")),
            ),
            // A reply that ends by asking shows the question.
            (
                r#"{"hook_event_name":"Stop","last_assistant_message":"Tests pass.\n\nShould I push?\n"}"#,
                Some(("done", "Should I push?")),
            ),
            (
                r#"{"hook_event_name":"Stop","last_assistant_message":"Why?\nBecause."}"#,
                Some(("done", "Why?")),
            ),
            (r#"{"hook_event_name":"Stop"}"#, Some(("done", ""))),
            (
                r#"{"hook_event_name":"Stop","stop_hook_active":true}"#,
                None,
            ),
            // Only agents keep the turn going; a shell or server started in
            // the background, or a task of no known type, does not.
            (
                r#"{"hook_event_name":"Stop","stop_hook_active":false,"background_tasks":[{"type":"subagent","agent_id":"a1"}]}"#,
                Some(("working", "waiting on 1 agent")),
            ),
            (
                r#"{"hook_event_name":"Stop","background_tasks":[{"type":"workflow"},{"type":"shell"},{"type":"teammate"}]}"#,
                Some(("working", "waiting on 2 agents")),
            ),
            (
                r#"{"hook_event_name":"Stop","background_tasks":[{"type":"cloud session"},{"type":"monitor"},{"type":"MCP task"}]}"#,
                Some(("working", "waiting on 1 agent")),
            ),
            (
                r#"{"hook_event_name":"Stop","background_tasks":[{"type":"shell","command":"npm run dev"}],"last_assistant_message":"Server is up."}"#,
                Some(("done", "Server is up. \u{b7} 1 background")),
            ),
            (
                r#"{"hook_event_name":"Stop","background_tasks":[{"id":"1"},{"type":7}]}"#,
                Some(("done", "2 background")),
            ),
            (
                r#"{"hook_event_name":"Stop","background_tasks":[],"last_assistant_message":"ok"}"#,
                Some(("done", "ok")),
            ),
            // A usage limit ends the turn until it resets; nothing failed.
            (
                r#"{"hook_event_name":"StopFailure","error":"rate_limit","last_assistant_message":"API Error: Rate limit reached"}"#,
                Some(("done", "usage limit")),
            ),
            (
                r#"{"hook_event_name":"StopFailure","error":"overloaded"}"#,
                Some(("error", "overloaded")),
            ),
            (
                r#"{"hook_event_name":"StopFailure","error":"authentication_failed"}"#,
                Some(("error", "authentication failed")),
            ),
            (
                r#"{"hook_event_name":"StopFailure","error":"max_output_tokens"}"#,
                Some(("error", "reply too long")),
            ),
            (
                r#"{"hook_event_name":"StopFailure","error":"unknown"}"#,
                Some(("error", "unknown error")),
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
    fn paths_relative_to_the_session() {
        for (path, cwd, want) in [
            (r"C:\a\b.rs", r"C:\", r"a\b.rs"),
            (r"C:\a\b.rs", r"C:\a\", "b.rs"),
            ("/home/me/x/y.rs", "/home/me", "x/y.rs"),
            (r"C:\a", r"C:\a", r"C:\a"),
            (r"C:\ab\c", r"C:\a", r"C:\ab\c"),
            (r"D:\a\b", r"C:\a", r"D:\a\b"),
            (r"C:\a\b", "", r"C:\a\b"),
            ("é", "ab", "é"),
        ] {
            assert_eq!(relative(path, cwd), want, "{path} in {cwd}");
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
            let timeout = (event != "SessionEnd").then_some(Json::Num(5.0));
            assert_eq!(cmd.get("timeout"), timeout.as_ref(), "{event}");
        }
        // Every notification reaches the hook, which sorts them.
        let Some(Json::Arr(groups)) = hooks.get("Notification") else {
            panic!("Notification missing");
        };
        assert_eq!(groups[0].get("matcher"), None);
    }

    #[test]
    fn plugin_is_written_once() {
        let dir = std::env::temp_dir().join(format!("blitz-plugin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let exe = r"C:\Program Files\blitz\blitz-hook.exe";
        write_plugin(&dir, exe).expect("first write");
        let manifest = std::fs::read_to_string(dir.join(".claude-plugin/plugin.json")).unwrap();
        let manifest = Json::parse(&manifest).expect("manifest is JSON");
        assert_eq!(manifest.get("name").and_then(Json::as_str), Some("blitz"));
        let hooks = dir.join("hooks/hooks.json");
        assert_eq!(
            std::fs::read_to_string(&hooks).unwrap(),
            claude_settings(exe)
        );

        // Unchanged, it is not written again: a read-only file would fail.
        // Elsewhere a rename replaces a read-only file all the same.
        #[cfg(windows)]
        {
            let readonly = |on: bool| {
                for f in [&hooks, &dir.join(".claude-plugin/plugin.json")] {
                    let mut p = std::fs::metadata(f).unwrap().permissions();
                    p.set_readonly(on);
                    std::fs::set_permissions(f, p).unwrap();
                }
            };
            readonly(true);
            let again = write_plugin(&dir, exe);
            readonly(false);
            again.expect("nothing to write");
        }

        // blitz moved: the hooks follow, and no temporary file stays.
        let moved = r"D:\tools\blitz\blitz-hook.exe";
        write_plugin(&dir, moved).expect("rewrite");
        assert_eq!(
            std::fs::read_to_string(&hooks).unwrap(),
            claude_settings(moved)
        );
        let left: Vec<_> = std::fs::read_dir(dir.join("hooks")).unwrap().collect();
        assert_eq!(left.len(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn each_blitz_hook_has_its_own_plugin() {
        let state = Path::new(r"C:\Users\me\AppData\Local\blitz");
        let installed = plugin_dir(state, r"C:\Program Files\blitz\blitz-hook.exe");
        let built = plugin_dir(state, r"C:\dev\blitz\target\release\blitz-hook.exe");
        assert_ne!(installed, built);
        assert_eq!(installed.parent(), Some(state));
        assert_eq!(
            plugin_dir(state, r"C:\PROGRAM FILES\blitz\blitz-hook.exe"),
            installed,
            "Windows paths ignore case"
        );
        assert_eq!(
            installed.file_name().and_then(|n| n.to_str()),
            Some("claude-plugin-bc19ae5af3c1512f"),
            "the same in every release"
        );
    }

    #[test]
    fn plugin_dirs_keep_the_users_own() {
        let ours = r"C:\Users\me\AppData\Local\blitz\claude-plugin";
        assert_eq!(plugin_dirs(None, ours), ours);
        assert_eq!(plugin_dirs(Some(""), ours), ours);
        assert_eq!(
            plugin_dirs(Some(r"C:\mine;D:\more;"), ours),
            format!(r"C:\mine;D:\more;{ours}")
        );
        // A blitz started in a blitz pane already has it.
        let both = format!(r"C:\mine;{}", ours.to_uppercase());
        assert_eq!(plugin_dirs(Some(&both), ours), both);
    }

    #[test]
    fn setup_needs_an_app() {
        assert_eq!(setup(&[]), 2);
        assert_eq!(setup(&["vim".into()]), 2);
        assert_eq!(setup(&["claude".into(), "--wls".into()]), 2);
    }

    #[test]
    fn paths_as_wsl_sees_them() {
        for (path, want) in [
            (
                r"C:\Program Files\blitz\blitz-hook.exe",
                Some("/mnt/c/Program Files/blitz/blitz-hook.exe"),
            ),
            (r"\\?\D:\b\blitz-hook.exe", Some("/mnt/d/b/blitz-hook.exe")),
            (r"\\server\share\blitz-hook.exe", None),
            ("relative", None),
            (r"é:\x", None),
            ("C:", None),
        ] {
            assert_eq!(wsl_path(path).as_deref(), want, "{path}");
        }
    }

    /// As long as a real pane token: 128 bits in hex.
    const TOKEN: &str = "4b1d0123456789abcdef0123456789ab";
    const SESSION: &str = "0b8f6a3e-1c2d-4e5f-9a7b-3c4d5e6f7a8b";

    #[test]
    fn every_state_is_an_attention_event() {
        for s in ["working", "needs-you", "done", "error", "idle", "ready"] {
            let title = format!("blitz:{TOKEN}:{s}");
            assert!(Ev::from_notify(&title, TOKEN).is_some(), "{s}");
            for title in [
                format!("blitz:{TOKEN}:{s}:{SESSION}"),
                format!("blitz:{TOKEN}:{s}:{SESSION}:v{PROTOCOL}"),
            ] {
                assert_eq!(
                    Ev::from_notify(&title, TOKEN).and_then(|(_, id)| id),
                    Some(SESSION),
                    "{title}"
                );
            }
            let out = notify_seq(TOKEN, s, None, "m");
            let title = out.strip_prefix("\x1b]777;notify;").unwrap();
            let title = title.split_once(';').unwrap().0;
            assert!(Ev::from_notify(title, TOKEN).is_some(), "{title}");
            assert_eq!(notify_protocol(title).1, PROTOCOL);
        }
    }

    /// SessionStart says Claude Code's hooks report, and passes on the id
    /// only of a session that has a conversation to resume.
    #[test]
    fn session_start_is_ready() {
        let seq = |source: &str| {
            let payload = format!(
                r#"{{"hook_event_name":"SessionStart","source":"{source}","session_id":"{SESSION}"}}"#
            );
            let out = claude_output(TOKEN, &payload).unwrap();
            let v = Json::parse(&out).unwrap();
            (v.get("terminalSequence").and_then(Json::as_str))
                .unwrap()
                .to_owned()
        };
        for source in ["resume", "fork"] {
            assert_eq!(
                seq(source),
                format!("\x1b]777;notify;blitz:{TOKEN}:ready:{SESSION}:v2;\x07")
            );
        }
        for source in ["startup", "clear", "compact", ""] {
            assert_eq!(
                seq(source),
                format!("\x1b]777;notify;blitz:{TOKEN}:ready:v2;\x07")
            );
        }
    }

    #[test]
    fn output_is_one_json_line() {
        assert_eq!(
            notify_json(TOKEN, "done", None, "All \"good\" \\ ok"),
            format!(
                "{{\"terminalSequence\":\"\\u001b]777;notify;blitz:{TOKEN}:done:v2;All \\\"good\\\" \\\\ ok\\u0007\"}}\n"
            )
        );
        let out = claude_output(TOKEN, r#"{"hook_event_name":"SessionEnd"}"#).unwrap();
        let v = Json::parse(&out).unwrap();
        assert_eq!(
            v.get("terminalSequence").and_then(Json::as_str),
            Some(format!("\x1b]777;notify;blitz:{TOKEN}:idle:v2;\x07").as_str())
        );
        assert_eq!(claude_output(TOKEN, "not json"), None);
    }

    /// Payloads as other writers may send them: pretty-printed with CRLF
    /// line ends, or behind a byte-order mark.
    #[test]
    fn output_for_pretty_and_bom_payloads() {
        let want = Some(format!(
            "{{\"terminalSequence\":\"\\u001b]777;notify;blitz:{TOKEN}:done:v2;ok\\u0007\"}}\n"
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
            format!("\x1b]777;notify;blitz:{TOKEN}:done:{SESSION}:v2;ok\x07")
        );
        // A bad id is dropped; the state still gets through.
        assert_eq!(
            seq(r#"{"hook_event_name":"SessionEnd","session_id":"x;rm -rf ~"}"#),
            format!("\x1b]777;notify;blitz:{TOKEN}:idle:v2;\x07")
        );
        assert_eq!(
            seq(r#"{"hook_event_name":"SessionEnd","session_id":7}"#),
            format!("\x1b]777;notify;blitz:{TOKEN}:idle:v2;\x07")
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

    /// Other agents report through `blitz-hook notify`, with the same
    /// sequence a Claude Code hook gets written.
    #[test]
    fn notify_for_other_agents() {
        let notify = |args: &[&str]| {
            let args: Vec<String> = args.iter().map(|&a| a.into()).collect();
            notify_output(TOKEN, &args)
        };
        let seq = |state: &str, msg: &str| Some(notify_seq(TOKEN, state, None, msg));
        for state in STATES {
            assert_eq!(notify(&[state]), seq(state, ""), "{state}");
            let title = format!("blitz:{TOKEN}:{state}:v{PROTOCOL}");
            assert!(Ev::from_notify(&title, TOKEN).is_some(), "{state}");
        }
        assert_eq!(
            notify(&["needs-you", "Approve\nthe \x1b]0;x\x07plan"]),
            seq("needs-you", "Approve the ]0;xplan")
        );
        // Codex adds its notification as JSON after the arguments.
        let codex = r#"{"type":"agent-turn-complete","last-assistant-message":"Done.\nShip it?","input-messages":["fix"]}"#;
        assert_eq!(notify(&["done", codex]), seq("done", "Ship it?"));
        assert_eq!(notify(&["done", "Built", codex]), seq("done", "Built"));
        assert_eq!(notify(&["done", "[1]"]), seq("done", "[1]"));
        for bad in [&[][..], &["ready"], &["Done"], &["needs you"], &["", "x"]] {
            assert_eq!(notify(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn message_cannot_break_the_sequence() {
        let evil = "a\x1b]0;pwned\x07b\u{9c}c\r\nd\te\x00f";
        assert_eq!(one_line(evil), "a]0;pwnedbc d ef");
        assert_eq!(one_line("  lots   of\n\n space  "), "lots of space");
        assert_eq!(one_line(" \x1b \x07 "), "");
    }

    /// The count of background tasks survives a long reply.
    #[test]
    fn done_keeps_the_background_count() {
        let payload = format!(
            r#"{{"hook_event_name":"Stop","background_tasks":[{{"type":"shell"}}],"last_assistant_message":"{}"}}"#,
            "word ".repeat(100)
        );
        let (_, msg) = state(&payload).unwrap();
        assert_eq!(msg.chars().count(), MAX_MSG);
        assert!(msg.ends_with(" word\u{2026} \u{b7} 1 background"), "{msg}");
        assert_eq!(one_line(&msg), msg, "nothing more is cut on the way out");
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
        // A blitz-hook in there gets no plugin.
        let hook = dir.join("blitz-hook.exe");
        let made = std::fs::write(&hook, "");
        let state = dir.with_extension("state");
        let plugin = plugin_for(&hook, &state);
        let _ = std::fs::remove_file(&hook);
        let _ = std::fs::remove_dir(&dir);
        made.expect("blitz-hook");
        assert_eq!(plugin, None);
        assert!(!state.exists());
        for (grant, got, want) in seen {
            assert_eq!(got, want, "after {grant}");
        }
    }
}
