//! A `tmux` for Claude Code's agent teams, so that its teammates open as
//! blitz panes. With `--teammate-mode tmux` and no tmux around it, Claude
//! Code 2.1.312 runs `tmux -V`, then makes a session of placeholder panes,
//! styles them, and starts each teammate in one with `respawn-pane`. blitz
//! puts a copy of blitz-hook named `tmux` at the end of every pane's PATH,
//! which sends its arguments here, so a real tmux earlier on PATH still
//! comes first.
//!
//! A team is the pane whose program asks, its lead, under the server that
//! program names, as each Claude Code names its own. Placeholders live only
//! here: the `cat` they would run never does. A teammate that starts opens
//! in the team's tab, right after the lead's, which blitz lays out as a
//! grid whenever one starts or closes. Anything else fails, loudly, so a
//! change in what Claude Code asks shows.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use crate::hook::{Json, escape_json};
use crate::layout::{PaneId, Window};

/// What tmux says when a pane does not fit, which Claude Code explains.
pub const NO_SPACE: &str = "no space for new pane";
/// Why blitz could not start a teammate, said without its command line.
pub const NOT_STARTED: &str = "blitz: the teammate's program did not start";
/// The one session Claude Code makes.
const SESSION: &str = "claude-swarm";
/// The most panes a team has, teammates and placeholders, as any program
/// in the lead's pane may ask for more.
const MAX_PANES: usize = 64;
/// The longest name kept: a teammate's title, its team's, a window's.
const MAX_NAME: usize = 64;
const PANE_ID: &str = "#{pane_id}";
const WINDOW_NAME: &str = "#{window_name}";
/// The options Claude Code sets, which change nothing here.
const OPTIONS: [&str; 6] = [
    "window-style",
    "pane-border-style",
    "pane-active-border-style",
    "pane-border-format",
    "pane-border-status",
    "remain-on-exit",
];
const ONLY: &str = "blitz: this tmux only opens Claude Code teammates as blitz panes; install a real tmux (psmux, MSYS2) and it comes first on PATH";
const ATTACH: &str = "blitz: teammates are in the tab beside the lead";
const FORMAT: &str = "blitz: this tmux prints only #{pane_id} and #{window_name}";
const NO_SESSION: &str = "can't find session: claude-swarm";
const NO_PANE: &str = "can't find pane";
const SHAPE: &str = "blitz: a teammate's command is cd DIR && env NAME=VALUE... PROGRAM ARGS...";
/// What Claude Code on Linux puts before a teammate's command, after `sh
/// -c`, to move it into its cgroup.
const CGROUP: &str = r#"{ echo $PPID > "$1"/cgroup.procs; } 2>/dev/null"#;

/// What a tmux command prints and exits with.
#[derive(Debug, Default, PartialEq)]
pub struct Reply {
    pub code: i32,
    pub out: String,
    pub err: String,
}

impl Reply {
    fn fail(why: &str) -> Reply {
        Reply {
            code: 1,
            out: String::new(),
            err: format!("{why}\n"),
        }
    }

    /// The reply line to the stand-in.
    pub fn json(&self) -> String {
        let mut out = format!("{{\"ok\":true,\"code\":{},\"out\":\"", self.code);
        escape_json(&self.out, &mut out);
        out.push_str("\",\"err\":\"");
        escape_json(&self.err, &mut out);
        out + "\"}"
    }
}

/// What blitz does for a command, besides answering it.
#[derive(Debug, PartialEq)]
pub enum Effect {
    None,
    /// Open a teammate's pane; [`Teams::failed`] when that fails.
    Start(Start),
    /// Close a teammate's pane.
    Close(PaneId),
}

/// A teammate to open, in pane `id` that its placeholder holds.
#[derive(Debug, PartialEq)]
pub struct Start {
    pub lead: PaneId,
    pub id: PaneId,
    /// The lead's teammates that run, in the tab the new one joins.
    pub mates: Vec<PaneId>,
    pub cwd: PathBuf,
    pub cmdline: String,
    /// Its program's name, as outside Windows `cmdline` starts `exec`.
    pub name: String,
    pub env: Vec<(String, String)>,
    pub title: String,
    /// The name of the team, for its tab.
    pub team: String,
}

#[derive(Debug)]
struct Member {
    lead: PaneId,
    /// What `-L` named when it was made, if anything.
    server: Option<String>,
    id: PaneId,
    window: String,
    title: Option<String>,
    /// A teammate runs in it; else it is a placeholder.
    started: bool,
}

/// Every lead's teammates and placeholders, oldest first.
#[derive(Debug, Default)]
pub struct Teams {
    members: Vec<Member>,
}

impl Teams {
    /// Answers `argv` from the program in pane `lead`, whose variables are
    /// `env`, once members whose pane, or lead's, is not in `live` are
    /// gone. A placeholder takes the pane id `next`.
    pub fn run(
        &mut self,
        lead: PaneId,
        argv: &[String],
        env: &[(String, String)],
        live: &[PaneId],
        next: &mut u32,
    ) -> (Reply, Effect) {
        (self.members).retain(|m| live.contains(&m.lead) && (!m.started || live.contains(&m.id)));
        match self.call(lead, argv, env, next) {
            Ok((out, effect)) => (
                Reply {
                    out,
                    ..Reply::default()
                },
                effect,
            ),
            Err(why) => (Reply::fail(why), Effect::None),
        }
    }

    /// The reply for teammate `id`, which did not start, saying `why`; its
    /// placeholder goes, as tmux's would.
    pub fn failed(&mut self, id: PaneId, why: &str) -> Reply {
        self.members.retain(|m| m.id != id);
        Reply::fail(why)
    }

    /// The running teammates of the team pane `id` is in, but `id`; `None`
    /// when it is in none.
    pub fn mates(&self, id: PaneId) -> Option<Vec<PaneId>> {
        let lead = self.members.iter().find(|m| m.id == id)?.lead;
        let mates = (self.team(lead))
            .filter(|m| m.started && m.id != id)
            .map(|m| m.id);
        Some(mates.collect())
    }

    fn team(&self, lead: PaneId) -> impl Iterator<Item = &Member> {
        self.members.iter().filter(move |m| m.lead == lead)
    }

    /// The member of `lead`'s team that target `t` names.
    fn member(&self, lead: PaneId, t: Option<&str>) -> Result<usize, &'static str> {
        let id = (t.and_then(|t| t.strip_prefix('%')))
            .filter(|n| (1..=10).contains(&n.len()) && n.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|n| n.parse().ok())
            .ok_or(NO_PANE)?;
        (self.members.iter())
            .position(|m| m.lead == lead && m.id == PaneId(id))
            .ok_or(NO_PANE)
    }

    fn call(
        &mut self,
        lead: PaneId,
        argv: &[String],
        env: &[(String, String)],
        next: &mut u32,
    ) -> Result<(String, Effect), &'static str> {
        let (server, sub, rest) = sub(argv)?;
        // Another Claude Code in the lead's pane names a server of its own,
        // which, like a fresh tmux, holds nothing the last one left.
        if let Some(s) = server {
            (self.members).retain(|m| m.lead != lead || m.server.as_deref().is_none_or(|t| t == s));
        }
        let f = flags(rest).ok_or(ONLY)?;
        let done = Ok((String::new(), Effect::None));
        match sub {
            "has-session" => {
                session(f.get(b't'))?;
                match self.team(lead).next() {
                    Some(_) => done,
                    None => Err(NO_SESSION),
                }
            }
            "new-session" => {
                if f.get(b's') != Some(SESSION) {
                    return Err(ONLY);
                }
                if self.team(lead).next().is_some() {
                    return Err("duplicate session: claude-swarm");
                }
                match f.cmd {
                    [] => done,
                    _ => self.placeholder(lead, server, f.get(b'n').unwrap_or("0"), &f, next),
                }
            }
            "new-window" => {
                session(f.get(b't'))?;
                self.placeholder(lead, server, f.get(b'n').unwrap_or("0"), &f, next)
            }
            // Where and which way does not matter: the tab is a grid.
            "split-window" => {
                let window = self.members[self.member(lead, f.get(b't'))?].window.clone();
                self.placeholder(lead, server, &window, &f, next)
            }
            "list-windows" => {
                session(f.get(b't'))?;
                format(&f, WINDOW_NAME)?;
                let mut names: Vec<&str> = Vec::new();
                for m in self.team(lead) {
                    if !names.contains(&m.window.as_str()) {
                        names.push(&m.window);
                    }
                }
                lines(names.into_iter().map(str::to_owned), NO_SESSION)
            }
            "list-panes" => {
                let window = session(f.get(b't'))?;
                format(&f, PANE_ID)?;
                let ids = (self.team(lead))
                    .filter(|m| window.is_none_or(|w| m.window == w))
                    .map(|m| format!("%{}", m.id.0));
                lines(ids, "can't find window")
            }
            "select-pane" => {
                let at = self.member(lead, f.get(b't'))?;
                // A model chose it, so it is shown as an OSC title would be.
                let title = vt::osc::clean(f.get(b'T').ok_or(ONLY)?, MAX_NAME);
                self.members[at].title = Some(title);
                done
            }
            "set-option" => match f.words[..] {
                [o, _] if OPTIONS.contains(&o) => done,
                _ => Err("blitz: this tmux takes only the options Claude Code sets"),
            },
            "select-layout" => done,
            "respawn-pane" => {
                let at = self.member(lead, f.get(b't'))?;
                let m = &self.members[at];
                if m.started {
                    return Err("blitz: a teammate's pane starts only once");
                }
                let [line] = f.cmd else {
                    return Err(SHAPE);
                };
                let cmd = command(line)?;
                let clean = |s: &str| vt::osc::clean(s, MAX_NAME);
                let title = (m.title.clone())
                    .or_else(|| flag(&cmd.argv, "--agent-name").map(clean))
                    .unwrap_or_else(|| "teammate".into());
                let team = flag(&cmd.argv, "--team-name").unwrap_or("agent team");
                let name = Path::new(&cmd.argv[0]).file_stem();
                let id = m.id;
                self.members[at].started = true;
                let start = Start {
                    lead,
                    id,
                    mates: self.mates(id).unwrap_or_default(),
                    cmdline: cmdline(&cmd.argv),
                    name: name.map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
                    team: clean(team),
                    cwd: cmd.cwd,
                    env: teammate_env(env, cmd.env),
                    title,
                };
                Ok((String::new(), Effect::Start(start)))
            }
            "kill-pane" => {
                let at = self.member(lead, f.get(b't'))?;
                if self.members[at].started {
                    return Ok((String::new(), Effect::Close(self.members[at].id)));
                }
                self.members.remove(at);
                done
            }
            "attach" | "a" | "attach-session" => Err(ATTACH),
            _ => Err(ONLY),
        }
    }

    /// A placeholder pane for `lead` in `window`, printed as `-P` asks.
    fn placeholder(
        &mut self,
        lead: PaneId,
        server: Option<&str>,
        window: &str,
        f: &Flags,
        next: &mut u32,
    ) -> Result<(String, Effect), &'static str> {
        if f.has(b'P') {
            format(f, PANE_ID)?;
        }
        if window.len() > MAX_NAME {
            return Err("blitz: a window's name is too long");
        }
        if self.team(lead).count() >= MAX_PANES {
            return Err(NO_SPACE);
        }
        let id = PaneId(*next);
        *next = next.checked_add(1).ok_or(NO_SPACE)?;
        self.members.push(Member {
            lead,
            server: server.map(str::to_owned),
            id,
            window: window.to_owned(),
            title: None,
            started: false,
        });
        let out = match f.has(b'P') {
            true => format!("%{}\n", id.0),
            false => String::new(),
        };
        Ok((out, Effect::None))
    }
}

/// `items` a line each; refused with `none` when there are none, as an
/// empty list would read as a session with nothing in it.
fn lines(
    items: impl Iterator<Item = String>,
    none: &'static str,
) -> Result<(String, Effect), &'static str> {
    let out: String = items.map(|i| i + "\n").collect();
    match out.is_empty() {
        true => Err(none),
        false => Ok((out, Effect::None)),
    }
}

/// The window target `t` names in the session, if it names one.
fn session(t: Option<&str>) -> Result<Option<&str>, &'static str> {
    match t.and_then(|t| t.strip_prefix(SESSION)) {
        Some("") => Ok(None),
        Some(w) => (w.strip_prefix(':'))
            .filter(|w| !w.is_empty())
            .map(Some)
            .ok_or(NO_SESSION),
        None => Err(NO_SESSION),
    }
}

fn format(f: &Flags, want: &str) -> Result<(), &'static str> {
    match f.get(b'F') == Some(want) {
        true => Ok(()),
        false => Err(FORMAT),
    }
}

/// The server a `-L` names, which must be one of Claude Code's, then the
/// subcommand of `argv` and what follows it.
pub fn sub(argv: &[String]) -> Result<(Option<&str>, &str, &[String]), &'static str> {
    let (server, rest) = match argv {
        [l, name, rest @ ..] if l == "-L" => {
            let pid = name.strip_prefix("claude-swarm-").unwrap_or_default();
            if pid.is_empty() || !pid.bytes().all(|b| b.is_ascii_digit()) {
                return Err(ONLY);
            }
            (Some(name.as_str()), rest)
        }
        _ => (None, argv),
    };
    let (sub, rest) = rest.split_first().ok_or(ONLY)?;
    Ok((server, sub.as_str(), rest))
}

/// The flags of one command: those Claude Code gives, and no others.
#[derive(Default)]
struct Flags<'a> {
    /// Those without a value, by letter.
    on: Vec<u8>,
    vals: Vec<(u8, &'a str)>,
    /// What is neither, before `--`.
    words: Vec<&'a str>,
    /// What follows `--`.
    cmd: &'a [String],
}

impl<'a> Flags<'a> {
    fn has(&self, c: u8) -> bool {
        self.on.contains(&c)
    }

    fn get(&self, c: u8) -> Option<&'a str> {
        self.vals.iter().find(|v| v.0 == c).map(|v| v.1)
    }
}

fn flags(args: &[String]) -> Option<Flags<'_>> {
    let mut f = Flags::default();
    let mut it = args.iter().enumerate();
    while let Some((i, a)) = it.next() {
        match a.as_bytes() {
            b"--" => {
                f.cmd = &args[i + 1..];
                break;
            }
            [b'-', c @ (b'd' | b'P' | b'h' | b'v' | b'k' | b'p' | b'w')] => f.on.push(*c),
            [b'-', c @ (b'F' | b'n' | b's' | b't' | b'l' | b'c' | b'T')] => {
                f.vals.push((*c, it.next()?.1));
            }
            [b'-', ..] => return None,
            _ => f.words.push(a),
        }
    }
    Some(f)
}

/// The value of `--name` in `argv`, as `--name V` or `--name=V`.
fn flag<'a>(argv: &'a [String], name: &str) -> Option<&'a str> {
    (argv.iter().enumerate()).find_map(|(i, a)| match a.strip_prefix(name)? {
        "" => argv.get(i + 1).map(String::as_str),
        v => v.strip_prefix('='),
    })
}

#[derive(Debug, PartialEq)]
enum Tok {
    Word(String),
    And,
    Semi,
}

/// `line` split into words as a POSIX shell splits it, taking only what
/// Claude Code writes: words of `[A-Za-z0-9_./:=@+,-]`, single quotes,
/// double quotes with nothing a shell would expand or escape, `&&` and `;`.
/// Pieces with nothing between them make one word.
fn tokens(line: &str) -> Result<Vec<Tok>, &'static str> {
    const BAD: &str = "blitz: a teammate's command holds what this tmux does not run";
    let mut out = Vec::new();
    let mut word: Option<String> = None;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' | '&' | ';' => {
                out.extend(word.take().map(Tok::Word));
                match c {
                    '&' if chars.next() == Some('&') => out.push(Tok::And),
                    '&' => return Err(BAD),
                    ';' => out.push(Tok::Semi),
                    _ => {}
                }
            }
            '\'' | '"' => {
                let w = word.get_or_insert_default();
                loop {
                    match chars.next() {
                        Some(q) if q == c => break,
                        Some(x)
                            if x.is_control()
                                || (c == '"' && matches!(x, '$' | '`' | '\\' | '!')) =>
                        {
                            return Err(BAD);
                        }
                        Some(x) => w.push(x),
                        None => return Err(BAD),
                    }
                }
            }
            c if c.is_ascii_alphanumeric() || "_./:=@+,-".contains(c) => {
                word.get_or_insert_default().push(c);
            }
            _ => return Err(BAD),
        }
    }
    out.extend(word.map(Tok::Word));
    Ok(out)
}

/// A teammate's start: its folder, the variables its command sets, and
/// its program and arguments.
#[derive(Debug, PartialEq)]
struct Cmd {
    cwd: PathBuf,
    env: Vec<(String, String)>,
    argv: Vec<String>,
}

/// The teammate `line` starts, which must be `cd DIR && env NAME=VALUE...
/// PROGRAM ARGS...` with DIR and PROGRAM in full and there. blitz's own
/// variables are the new pane's, so the ones the line sets are left out.
fn command(line: &str) -> Result<Cmd, &'static str> {
    use Tok::{And, Semi, Word};
    let toks = tokens(line)?;
    let mut rest = &toks[..];
    if let [
        Word(sh),
        Word(c),
        Word(script),
        Word(arg0),
        Word(_),
        Semi,
        after @ ..,
    ] = rest
        && (sh.as_str(), c.as_str(), script.as_str(), arg0.as_str()) == ("sh", "-c", CGROUP, "sh")
    {
        rest = after;
    }
    let [Word(cd), Word(dir), And, Word(env), words @ ..] = rest else {
        return Err(SHAPE);
    };
    if cd != "cd" || env != "env" {
        return Err(SHAPE);
    }
    let words = (words.iter())
        .map(|t| match t {
            Word(w) => Ok(w.as_str()),
            _ => Err(SHAPE),
        })
        .collect::<Result<Vec<_>, _>>()?;
    let at = words.iter().position(|w| !w.contains('=')).ok_or(SHAPE)?;
    let mut vars = Vec::new();
    for w in &words[..at] {
        let (k, v) = w.split_once('=').unwrap_or_default();
        let name = k.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
            && k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
        if !name {
            return Err(SHAPE);
        }
        // `NAME=` sets it empty, over what the lead has.
        if !k.to_ascii_uppercase().starts_with("BLITZ_") {
            vars.push((k.to_owned(), v.to_owned()));
        }
    }
    // UNC and device paths go before either is looked at: looking at
    // `\\host\share` makes Windows sign in to that host.
    let local = |p: &str| vt::osc::local_dir(p) && Path::new(p).is_absolute();
    let cwd = PathBuf::from(dir);
    if !local(dir) || !cwd.is_dir() {
        return Err("blitz: a teammate's folder is not a folder named in full");
    }
    let prog = Path::new(words[at]);
    if !local(words[at]) || !prog.is_file() {
        return Err("blitz: a teammate's program is not a file named in full");
    }
    // Windows runs those through cmd, which reads the line its own way, and
    // finds one with dots and spaces after its name too.
    let batch =
        |e: &std::ffi::OsStr| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat");
    let named = Path::new(words[at].trim_end_matches(['.', ' ']));
    if cfg!(windows) && named.extension().is_some_and(batch) {
        return Err("blitz: a teammate's program is not a batch file");
    }
    let argv = words[at..].iter().map(|w| w.to_string()).collect();
    Ok(Cmd {
        cwd,
        env: vars,
        argv,
    })
}

/// `argv` as a command line that starts its program with no shell
/// between: as `CreateProcessW` splits it on Windows, and run with `exec`
/// by `sh` elsewhere.
fn cmdline(argv: &[String]) -> String {
    if cfg!(windows) {
        let words: Vec<_> = argv.iter().map(|a| crate::shell::quote(a)).collect();
        return words.join(" ");
    }
    let words: Vec<_> = (argv.iter())
        .map(|a| format!("'{}'", a.replace('\'', r"'\''")))
        .collect();
    format!("exec {}", words.join(" "))
}

/// The variables a teammate starts with: its lead's, but another
/// terminal's and blitz's own, under those its command `set`. A cmd
/// prompt of blitz's holds the lead's token, so it goes too.
fn teammate_env(lead: &[(String, String)], set: Vec<(String, String)>) -> Vec<(String, String)> {
    (lead.iter())
        .filter(|(k, v)| {
            let k = k.to_ascii_uppercase();
            !crate::pty::foreign(&k)
                && !(k == "PROMPT" && crate::shell::is_blitz_prompt(v.as_ref()))
        })
        .cloned()
        .chain(set)
        .collect()
}

/// Lays out again, as a grid with `new` added, the tab that holds any of
/// a team's `mates`; the index of that tab.
pub fn regrid(win: &mut Window, mates: &[PaneId], new: Option<PaneId>) -> Option<usize> {
    let i = (win.tabs.iter()).position(|t| mates.iter().any(|&m| t.root.contains(m)))?;
    let mut panes = win.tabs[i].panes();
    panes.extend(new);
    // Panes in the order they were made.
    panes.sort();
    win.tabs[i].tile(&panes);
    Some(i)
}

/// Puts teammate `s` in its team's tab, or in a new one after its lead's,
/// named for the team; the index of that tab.
pub fn place(win: &mut Window, s: &Start) -> Option<usize> {
    if let Some(i) = regrid(win, &s.mates, Some(s.id)) {
        return Some(i);
    }
    if !win.tab_beside(s.lead, s.id) {
        return None;
    }
    let i = win.tabs.iter().position(|t| t.root.contains(s.id))?;
    win.tabs[i].name.clone_from(&s.team);
    Some(i)
}

/// `argv` for a log: each value after a `=`, and the command after `--`,
/// left out, as they may hold secrets.
pub fn redacted(argv: &[String]) -> String {
    let mut out = Vec::new();
    for a in argv {
        if a == "--" {
            out.push("-- ...".to_owned());
            break;
        }
        out.push(match a.split_once('=') {
            Some((k, _)) => format!("{k}=..."),
            None => a.clone(),
        });
    }
    out.join(" ")
}

/// The `tmux` panes find: answers `-V` itself, which is how Claude Code
/// looks for tmux, and sends anything else to the blitz the pane is in.
/// Returns the exit code.
pub fn client(args: &[String]) -> i32 {
    if args == ["-V"] {
        return match crate::control::reach() {
            Ok(_) => {
                let _ = std::io::stdout().write_all(b"tmux 3.4\n");
                0
            }
            Err(e) => {
                eprintln!("blitz: {e}");
                1
            }
        };
    }
    let mut fields = String::from("\"op\":\"tmux\",\"argv\":[");
    for (i, a) in args.iter().enumerate() {
        fields.push_str(if i == 0 { "\"" } else { ",\"" });
        escape_json(a, &mut fields);
        fields.push('"');
    }
    fields.push(']');
    // A teammate starts with what its lead has; nothing else needs it.
    if matches!(sub(args), Ok((_, "respawn-pane", _))) {
        fields.push_str(",\"env\":{");
        let vars = std::env::vars_os()
            .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)));
        for (i, (k, v)) in vars.enumerate() {
            fields.push_str(if i == 0 { "\"" } else { ",\"" });
            escape_json(&k, &mut fields);
            fields.push_str("\":\"");
            escape_json(&v, &mut fields);
            fields.push('"');
        }
        fields.push('}');
    }
    match crate::control::call(&fields) {
        Ok(r) => {
            let text = |k| r.get(k).and_then(Json::as_str).unwrap_or("");
            let _ = std::io::stdout().write_all(text("out").as_bytes());
            let _ = std::io::stderr().write_all(text("err").as_bytes());
            match r.get("code") {
                Some(&Json::Num(n)) => n as i32,
                _ => 1,
            }
        }
        Err(e) => {
            eprintln!("blitz: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Tab;

    const LEAD: PaneId = PaneId(1);
    const W: &str = "claude-swarm:swarm-view";
    const AREA: crate::layout::Rect = crate::layout::Rect {
        x: 0,
        y: 0,
        w: 1001,
        h: 601,
    };

    /// How Claude Code quotes a word: bare when it can be, else in single
    /// quotes, each one in it as `'"'"'`.
    fn q(w: &str) -> String {
        let safe = |c: char| c.is_ascii_alphanumeric() || "_./:=@+,-".contains(c);
        match !w.is_empty() && w.chars().all(safe) {
            true => w.to_owned(),
            false => format!("'{}'", w.replace('\'', r#"'"'"'"#)),
        }
    }

    fn dir() -> String {
        env!("CARGO_MANIFEST_DIR").to_owned()
    }

    fn exe() -> String {
        std::env::current_exe()
            .unwrap()
            .to_string_lossy()
            .into_owned()
    }

    /// A teammate's command shaped as 2.1.312 writes it, with made-up
    /// values.
    fn line(name: &str) -> String {
        format!(
            "cd {} && env CLAUDECODE=1 CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1 BLITZ_PANE_TOKEN=x \
             CLAUDE_CODE_ATTRIBUTION_PROJECT='' CLAUDE_CODE_EVEN_MORE_DANGEROUSLY_SKIP_PERMISSIONS= \
             {} --agent-id {name}@demo --agent-name {name} --team-name demo --agent-color blue \
             --parent-session-id 00000000-0000-4000-8000-000000000000 --permission-mode acceptEdits",
            q(&dir()),
            q(&exe())
        )
    }

    fn words(w: &[&str]) -> Vec<String> {
        w.iter().map(|s| s.to_string()).collect()
    }

    /// A lead and what blitz does with its tmux, as the window would.
    struct Lead {
        teams: Teams,
        next: u32,
        live: Vec<PaneId>,
        win: Window,
        env: Vec<(String, String)>,
        started: Vec<Start>,
        /// `-L` and its server, or nothing.
        server: Vec<String>,
    }

    impl Lead {
        fn new(server: &[&str]) -> Lead {
            let mut win = Window::default();
            win.tabs.push(Tab::new(String::new(), LEAD));
            Lead {
                teams: Teams::default(),
                next: 2,
                live: vec![LEAD],
                win,
                env: Vec::new(),
                started: Vec::new(),
                server: words(server),
            }
        }

        fn tmux(&mut self, argv: &[&str]) -> Reply {
            let argv = [self.server.clone(), words(argv)].concat();
            let (reply, effect) =
                (self.teams).run(LEAD, &argv, &self.env, &self.live, &mut self.next);
            match effect {
                Effect::Start(s) => {
                    place(&mut self.win, &s).expect("placed");
                    self.live.push(s.id);
                    self.started.push(s);
                }
                Effect::Close(id) => self.close(id),
                Effect::None => {}
            }
            reply
        }

        fn ok(&mut self, argv: &[&str]) -> String {
            let r = self.tmux(argv);
            assert_eq!((r.code, r.err.as_str()), (0, ""), "{argv:?}");
            r.out
        }

        fn fails(&mut self, argv: &[&str]) -> String {
            let r = self.tmux(argv);
            assert_eq!((r.code, r.out.as_str()), (1, ""), "{argv:?}");
            r.err
        }

        /// The teammate's pane closes, by `kill-pane` or its own exit.
        fn close(&mut self, id: PaneId) {
            let mates = self.teams.mates(id);
            self.win.close_pane(id);
            if let Some(m) = mates {
                regrid(&mut self.win, &m, None);
            }
            self.live.retain(|&p| p != id);
        }

        fn team_tab(&self) -> Vec<u32> {
            self.win.tabs[1].panes().iter().map(|p| p.0).collect()
        }

        /// What Claude Code runs for a teammate when the session is there.
        fn styled(&mut self, pane: &str, name: &str) {
            for (o, v) in [
                ("window-style", "bg=default,fg=blue"),
                ("pane-border-style", "fg=blue"),
                ("pane-active-border-style", "fg=blue"),
            ] {
                self.ok(&["set-option", "-p", "-t", pane, o, v]);
            }
            self.ok(&["select-pane", "-t", pane, "-T", name]);
            let fmt = "#[fg=blue,bold] #{pane_title} #[default]";
            self.ok(&["set-option", "-p", "-t", pane, "pane-border-format", fmt]);
        }

        fn respawn(&mut self, pane: &str, name: &str) {
            self.ok(&["set-option", "-p", "-t", pane, "remain-on-exit", "failed"]);
            self.ok(&["respawn-pane", "-k", "-t", pane, "--", &line(name)]);
        }

        /// A teammate after the first, as 2.1.312 adds it outside tmux.
        fn later(&mut self, name: &str, want: &str) {
            self.ok(&["has-session", "-t", "claude-swarm"]);
            let windows = ["list-windows", "-t", "claude-swarm", "-F", "#{window_name}"];
            assert_eq!(self.ok(&windows), "swarm-view\n");
            let panes = self.ok(&["list-panes", "-t", W, "-F", "#{pane_id}"]);
            for _ in 0..2 {
                assert_eq!(self.ok(&["list-panes", "-t", W, "-F", "#{pane_id}"]), panes);
            }
            let ids: Vec<&str> = panes.lines().collect();
            let (n, way) = (ids.len(), if ids.len() % 2 == 1 { "-v" } else { "-h" });
            let split = [
                "split-window",
                "-d",
                "-t",
                ids[(n - 1) / 2],
                way,
                "-P",
                "-F",
                "#{pane_id}",
            ];
            let id = self.ok(&[&split[..], &["--", "cat"]].concat());
            assert_eq!(id, format!("{want}\n"));
            self.styled(want, name);
            self.ok(&["list-panes", "-t", W, "-F", "#{pane_id}"]);
            self.ok(&["select-layout", "-t", W, "tiled"]);
            self.respawn(want, name);
        }
    }

    #[test]
    fn tmux_words_as_a_shell_splits_them() {
        let w = |s: &str| Tok::Word(s.into());
        assert_eq!(
            tokens(r#"a 'b c'  "d e" 'it'"'"'s' X='' Y= f;g&&h"#),
            Ok(vec![
                w("a"),
                w("b c"),
                w("d e"),
                w("it's"),
                w("X="),
                w("Y="),
                w("f"),
                Tok::Semi,
                w("g"),
                Tok::And,
                w("h"),
            ])
        );
        assert_eq!(tokens("''"), Ok(vec![w("")]));
        assert_eq!(
            tokens(r"'C:\Program Files\x'"),
            Ok(vec![w(r"C:\Program Files\x")])
        );
        for bad in [
            r#""$HOME""#,
            r#""a\b""#,
            "\"`id`\"",
            r#""hi!""#,
            "$x",
            "a|b",
            "a&b",
            "a>b",
            "*",
            "~/x",
            "(x)",
            "'open",
            "\"open",
            "a\nb",
            "'a\x1bb'",
            r"a\ b",
        ] {
            assert!(tokens(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn tmux_reads_a_teammate_command() {
        let cmd = command(&line("a")).unwrap();
        assert_eq!(cmd.cwd, PathBuf::from(dir()));
        assert_eq!(
            cmd.env,
            [
                ("CLAUDECODE", "1"),
                ("CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS", "1"),
                ("CLAUDE_CODE_ATTRIBUTION_PROJECT", ""),
                ("CLAUDE_CODE_EVEN_MORE_DANGEROUSLY_SKIP_PERMISSIONS", ""),
            ]
            .map(|(k, v)| (k.to_string(), v.to_string()))
        );
        assert_eq!(cmd.argv[..3], [exe(), "--agent-id".into(), "a@demo".into()]);
        assert_eq!(cmd.argv.last().map(String::as_str), Some("acceptEdits"));
        assert_eq!(flag(&cmd.argv, "--agent-name"), Some("a"));
        assert_eq!(flag(&words(&["--team-name=t"]), "--team-name"), Some("t"));

        // On Linux it may first move into Claude Code's cgroup.
        let cgroup = format!("sh -c {} sh /sys/fs/cgroup/x; {}", q(CGROUP), line("a"));
        assert_eq!(command(&cgroup), Ok(cmd));

        // Installed with npm, the program is node and the script follows.
        let npm = format!(
            "cd {} && env CLAUDECODE=1 {} {} --agent-name n",
            q(&dir()),
            q(&exe()),
            q("/x/cli.js")
        );
        assert_eq!(
            command(&npm).unwrap().argv[..2],
            [exe(), "/x/cli.js".into()]
        );

        let missing = std::path::Path::new(&dir()).join("missing");
        let missing = missing.to_string_lossy();
        let shell = |s: &str| format!("cd {} && env A=1 {} {s}", q(&dir()), q(&exe()));
        for bad in [
            format!("cd x && env A=1 {}", q(&exe())),
            format!("cd {} && env A=1 {}", q(&missing), q(&exe())),
            format!("cd {} && env A=1 claude", q(&dir())),
            format!("cd {} && env A=1 {}", q(&dir()), q(&missing)),
            format!("cd {} && env A=1", q(&dir())),
            format!("cd {} && {} x", q(&dir()), q(&exe())),
            format!("cd {} && env 1A=secret {}", q(&dir()), q(&exe())),
            format!("cd {} ; env A=1 {}", q(&dir()), q(&exe())),
            format!("sh -c 'secret' sh x; {}", line("a")),
            shell("; rm -rf x"),
            shell("&& secret"),
            shell("$secret"),
            // UNC, device and verbatim paths, even to what is there.
            format!(
                "cd {} && env A=1 {}",
                q(&format!(r"\\?\{}", dir())),
                q(&exe())
            ),
            format!("cd {} && env A=1 {}", q(&format!("/{}", dir())), q(&exe())),
            format!(
                "cd {} && env A=1 {}",
                q(&dir()),
                q(&format!(r"\\?\{}", exe()))
            ),
        ] {
            let why = command(&bad).expect_err(&bad);
            assert!(
                !why.contains("secret") && !why.contains("rm"),
                "{bad}: {why}"
            );
        }
        #[cfg(windows)]
        {
            let bat = std::env::temp_dir().join(format!("blitz-tmux-{}.cmd", std::process::id()));
            std::fs::write(&bat, "").unwrap();
            // Windows finds it with dots and spaces after its name too.
            for end in ["", ".", " ", ". ."] {
                let bat = format!("{}{end}", bat.to_string_lossy());
                let run = format!("cd {} && env A=1 {}", q(&dir()), q(&bat));
                assert!(command(&run).is_err(), "{bat:?}");
            }
            std::fs::remove_file(&bat).unwrap();
        }
    }

    /// A teammate starts with its lead's variables, but blitz's and another
    /// terminal's, and `NAME=` in its command empties one the lead has.
    #[test]
    fn tmux_a_teammate_command_empties_what_it_sets_empty() {
        let mut lead = Lead::new(&[]);
        let prompt = crate::shell::cmd_prompt("lead", "$P$G");
        lead.env = [
            ("CLAUDE_CODE_EVEN_MORE_DANGEROUSLY_SKIP_PERMISSIONS", "1"),
            ("BLITZ_PANE_TOKEN", "lead"),
            ("TERM_PROGRAM", "other"),
            ("HOME", "/home/me"),
            ("Prompt", prompt.as_str()),
        ]
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .to_vec();
        lead.ok(&[
            "new-session",
            "-d",
            "-s",
            "claude-swarm",
            "-n",
            "swarm-view",
            "--",
            "cat",
        ]);
        lead.respawn("%2", "a");
        let env = &lead.started[0].env;
        let last = |k: &str| env.iter().rev().find(|e| e.0 == k).map(|e| e.1.as_str());
        assert_eq!(
            last("CLAUDE_CODE_EVEN_MORE_DANGEROUSLY_SKIP_PERMISSIONS"),
            Some("")
        );
        assert_eq!(last("HOME"), Some("/home/me"));
        assert_eq!(last("CLAUDECODE"), Some("1"));
        assert_eq!(
            (last("BLITZ_PANE_TOKEN"), last("TERM_PROGRAM")),
            (None, None)
        );
        // cmd's prompt in a blitz pane holds the lead's token.
        assert!(!env.iter().any(|e| e.1.contains("lead")), "{env:?}");
    }

    /// The calls 2.1.312 makes for three teammates outside tmux, then as
    /// they go.
    #[test]
    fn tmux_replays_three_teammates() {
        let mut lead = Lead::new(&["-L", "claude-swarm-4242"]);
        assert_eq!(
            lead.fails(&["has-session", "-t", "claude-swarm"]),
            format!("{NO_SESSION}\n")
        );
        let new = [
            "new-session",
            "-d",
            "-s",
            "claude-swarm",
            "-n",
            "swarm-view",
            "-P",
            "-F",
            "#{pane_id}",
            "--",
            "cat",
        ];
        assert_eq!(lead.ok(&new), "%2\n");
        assert_eq!(
            lead.ok(&["list-panes", "-t", W, "-F", "#{pane_id}"]),
            "%2\n"
        );
        lead.ok(&["set-option", "-w", "-t", W, "pane-border-status", "top"]);
        lead.styled("%2", "researcher");
        lead.ok(&["list-panes", "-t", W, "-F", "#{pane_id}"]);
        assert_eq!(lead.win.tabs.len(), 1, "a placeholder is no pane");
        lead.respawn("%2", "a");
        lead.later("b", "%3");
        lead.later("c", "%4");

        let s = &lead.started;
        let ids: Vec<_> = s.iter().map(|s| (s.id.0, s.mates.len())).collect();
        assert_eq!(ids, [(2, 0), (3, 1), (4, 2)]);
        assert_eq!(s[0].title, "researcher", "named by select-pane -T");
        assert_eq!((s[1].title.as_str(), s[1].team.as_str()), ("b", "demo"));
        assert_eq!(s[2].cwd, PathBuf::from(dir()));
        assert!(s[2].cmdline.contains("c@demo"), "{}", s[2].cmdline);
        let name = std::ffi::OsStr::new(&s[2].name);
        assert_eq!(Path::new(&exe()).file_stem(), Some(name), "not exec");
        // One tab after the lead's, a grid, and the lead keeps focus.
        assert_eq!((lead.win.tabs.len(), lead.win.active), (2, 0));
        assert_eq!(
            (lead.team_tab(), lead.win.tabs[1].name.as_str()),
            (vec![2, 3, 4], "demo")
        );
        let rows: Vec<i32> = lead.win.tabs[1].rects(AREA).iter().map(|r| r.1.y).collect();
        assert_eq!(rows, [0, 0, 301]);
        assert!(
            lead.fails(&["respawn-pane", "-k", "-t", "%3", "--", &line("b")])
                .contains("once")
        );

        // Shut down: kill-pane, an exit, and the session is gone.
        lead.ok(&["kill-pane", "-t", "%3"]);
        assert_eq!(lead.team_tab(), [2, 4]);
        let rows: Vec<i32> = lead.win.tabs[1].rects(AREA).iter().map(|r| r.1.y).collect();
        assert_eq!(rows, [0, 0], "laid out again");
        lead.close(PaneId(4));
        assert_eq!(
            lead.ok(&["list-panes", "-t", W, "-F", "#{pane_id}"]),
            "%2\n"
        );
        assert_eq!(
            lead.fails(&["kill-pane", "-t", "%4"]),
            format!("{NO_PANE}\n")
        );
        lead.ok(&["kill-pane", "-t", "%2"]);
        assert_eq!(lead.win.tabs.len(), 1);
        lead.fails(&["has-session", "-t", "claude-swarm"]);
        lead.fails(&["list-panes", "-t", W, "-F", "#{pane_id}"]);
        lead.fails(&["list-windows", "-t", "claude-swarm", "-F", "#{window_name}"]);
        assert_eq!(lead.ok(&new), "%5\n", "a new session");
    }

    /// With `use_splitpane` off a teammate gets a window of its own, with
    /// no `-L`, and joins the same tab.
    #[test]
    fn tmux_replays_teammates_in_windows() {
        let mut lead = Lead::new(&[]);
        for (name, want) in [("x", "%2\n"), ("y", "%3\n")] {
            if lead.tmux(&["has-session", "-t", "claude-swarm"]).code != 0 {
                assert_eq!(lead.ok(&["new-session", "-d", "-s", "claude-swarm"]), "");
            }
            let window = format!("teammate-{name}");
            let new = [
                "new-window",
                "-t",
                "claude-swarm",
                "-n",
                &window,
                "-P",
                "-F",
                "#{pane_id}",
                "--",
                "cat",
            ];
            let id = lead.ok(&new);
            assert_eq!(id, want);
            lead.respawn(id.trim_end(), name);
        }
        assert_eq!(
            lead.ok(&["list-windows", "-t", "claude-swarm", "-F", "#{window_name}"]),
            "teammate-x\nteammate-y\n"
        );
        assert_eq!((lead.win.tabs.len(), lead.team_tab()), (2, vec![2, 3]));
        assert_eq!(
            lead.ok(&[
                "list-panes",
                "-t",
                "claude-swarm:teammate-y",
                "-F",
                "#{pane_id}"
            ]),
            "%3\n"
        );
        // Either way of asking reaches the same team.
        lead.server = words(&["-L", "claude-swarm-7"]);
        lead.ok(&["kill-pane", "-t", "%2"]);
        assert_eq!(lead.team_tab(), [3]);
    }

    /// Teammates started out of order still share one tab.
    #[test]
    fn tmux_parallel_spawns_share_a_tab() {
        let mut lead = Lead::new(&["-L", "claude-swarm-9"]);
        let new = [
            "new-session",
            "-d",
            "-s",
            "claude-swarm",
            "-n",
            "swarm-view",
            "-P",
            "-F",
            "#{pane_id}",
            "--",
            "cat",
        ];
        assert_eq!(lead.ok(&new), "%2\n");
        let split = [
            "split-window",
            "-d",
            "-t",
            "%2",
            "-h",
            "-P",
            "-F",
            "#{pane_id}",
            "--",
            "cat",
        ];
        assert_eq!(lead.ok(&split), "%3\n");
        lead.respawn("%3", "b");
        lead.respawn("%2", "a");
        assert_eq!((lead.win.tabs.len(), lead.team_tab()), (2, vec![2, 3]));
    }

    /// A second Claude Code in the lead's pane names another server, and
    /// finds nothing the first left: not its teammates, nor a placeholder
    /// it stopped before using.
    #[test]
    fn tmux_a_second_claude_code_makes_a_team_of_its_own() {
        let mut lead = Lead::new(&["-L", "claude-swarm-100"]);
        let new = [
            "new-session",
            "-d",
            "-s",
            "claude-swarm",
            "-n",
            "swarm-view",
            "-P",
            "-F",
            "#{pane_id}",
            "--",
            "cat",
        ];
        assert_eq!(lead.ok(&new), "%2\n");
        lead.respawn("%2", "a");
        let split = [
            "split-window",
            "-d",
            "-t",
            "%2",
            "-h",
            "-P",
            "-F",
            "#{pane_id}",
            "--",
            "cat",
        ];
        assert_eq!(lead.ok(&split), "%3\n");
        lead.server = words(&["-L", "claude-swarm-200"]);
        lead.fails(&["has-session", "-t", "claude-swarm"]);
        assert_eq!(lead.ok(&new), "%4\n");
        assert_eq!(
            lead.ok(&["list-panes", "-t", W, "-F", "#{pane_id}"]),
            "%4\n"
        );
        lead.respawn("%4", "b");
        assert!(lead.started[1].mates.is_empty());
        // A tab of its own, beside the lead's.
        assert_eq!((lead.win.tabs.len(), lead.team_tab()), (3, vec![4]));
    }

    /// The names a model chose are shown as an OSC title would be: with
    /// no controls, nothing that turns text around, and cut short.
    #[test]
    fn tmux_cleans_the_names_it_shows() {
        let mut lead = Lead::new(&[]);
        lead.ok(&[
            "new-session",
            "-d",
            "-s",
            "claude-swarm",
            "-n",
            "swarm-view",
            "--",
            "cat",
        ]);
        let long = "x".repeat(100);
        lead.ok(&[
            "select-pane",
            "-t",
            "%2",
            "-T",
            &format!("\u{202E}a\x1bb{long}"),
        ]);
        let team = format!("--team-name '\u{202E}team{long}'");
        let run = line("a").replace("--team-name demo", &team);
        lead.ok(&["respawn-pane", "-k", "-t", "%2", "--", &run]);
        let s = &lead.started[0];
        assert_eq!(s.title, format!("ab{}", &long[..62]));
        assert_eq!(s.team, format!("team{}", &long[..60]));
        assert_eq!(lead.win.tabs[1].name, s.team);
    }

    /// Anything in the lead's pane may ask for placeholders, which stay
    /// until they are killed: a team holds so many, named so long.
    #[test]
    fn tmux_limits_placeholders() {
        let mut lead = Lead::new(&[]);
        let new = ["new-window", "-t", "claude-swarm", "--", "cat"];
        let long = "x".repeat(65);
        let named = ["new-window", "-t", "claude-swarm", "-n", &long, "--", "cat"];
        assert!(lead.fails(&named).contains("too long"));
        lead.ok(&[
            "new-window",
            "-t",
            "claude-swarm",
            "-n",
            &long[1..],
            "--",
            "cat",
        ]);
        for _ in 1..MAX_PANES {
            lead.ok(&new);
        }
        assert_eq!(lead.fails(&new), format!("{NO_SPACE}\n"));
        // Nor does a pane id come round again.
        lead.ok(&["kill-pane", "-t", "%2"]);
        lead.next = u32::MAX;
        assert_eq!(lead.fails(&new), format!("{NO_SPACE}\n"));
    }

    /// A teammate blitz could not start leaves no placeholder behind, and
    /// "no space" is said as tmux says it.
    #[test]
    fn tmux_a_failed_start_drops_its_placeholder() {
        let mut t = Teams::default();
        let (live, mut next) = (vec![LEAD], 2);
        let mut run =
            |t: &mut Teams, argv: &[&str]| t.run(LEAD, &words(argv), &[], &live, &mut next);
        run(
            &mut t,
            &[
                "new-session",
                "-d",
                "-s",
                "claude-swarm",
                "-n",
                "swarm-view",
                "--",
                "cat",
            ],
        );
        let (reply, effect) = run(
            &mut t,
            &["respawn-pane", "-k", "-t", "%2", "--", &line("a")],
        );
        assert_eq!(reply, Reply::default());
        let Effect::Start(s) = effect else {
            panic!("{effect:?}");
        };
        assert_eq!(t.failed(s.id, NO_SPACE), Reply::fail(NO_SPACE));
        assert_eq!(
            run(&mut t, &["has-session", "-t", "claude-swarm"]).0.code,
            1
        );
        assert_eq!(
            run(&mut t, &["list-panes", "-t", W, "-F", "#{pane_id}"])
                .0
                .code,
            1
        );
    }

    /// What Claude Code does not ask fails with why, never with exit 0.
    #[test]
    fn tmux_refuses_what_claude_code_does_not_ask() {
        let mut lead = Lead::new(&[]);
        lead.ok(&[
            "new-session",
            "-d",
            "-s",
            "claude-swarm",
            "-n",
            "swarm-view",
            "--",
            "cat",
        ]);
        for argv in [
            &["-L", "default", "has-session", "-t", "claude-swarm"][..],
            &["-L", "claude-swarm-", "has-session", "-t", "claude-swarm"],
            &["-L", "claude-swarm-x", "has-session", "-t", "claude-swarm"],
            &["list-panes", "-t", W, "-F", "#{pane_index}"],
            &["list-panes", "-t", W],
            &["list-windows", "-t", "claude-swarm", "-F", "#{pane_id}"],
            &["list-panes", "-t", "other", "-F", "#{pane_id}"],
            &[
                "split-window",
                "-d",
                "-t",
                "%2",
                "-P",
                "-F",
                "#{session_name}",
                "--",
                "cat",
            ],
            &["split-window", "-t", "%9", "--", "cat"],
            &["split-window", "-Z", "-t", "%2", "--", "cat"],
            &["set-option", "-g", "mouse", "on"],
            &["set-option", "-p", "-t", "%2", "mouse", "on"],
            &["send-keys", "-t", "%2", "secret", "Enter"],
            &["capture-pane", "-p", "-t", "%2"],
            &["display-message", "-p", "#{pane_id}"],
            &["new-session", "-d", "-s", "claude-swarm"],
            &["new-session", "-d", "-s", "other"],
            &["respawn-pane", "-k", "-t", "%2", "--", "cat"],
            &[
                "respawn-pane",
                "-k",
                "-t",
                "%2",
                "--",
                "cd",
                "/",
                "&&",
                "env",
            ],
            &["frob"],
            &[],
        ] {
            let err = lead.fails(argv);
            assert!(
                !err.contains("secret") && !err.trim().is_empty(),
                "{argv:?}: {err}"
            );
        }
        for attach in ["attach", "a", "attach-session"] {
            assert!(lead.fails(&[attach]).contains("tab beside the lead"));
        }
        assert!(lead.fails(&["send-keys"]).contains("real tmux"));
        // Still there, for what Claude Code asks next.
        lead.ok(&["has-session", "-t", "claude-swarm"]);
        assert!(lead.started.is_empty());
    }

    #[test]
    fn tmux_logs_leave_out_values() {
        let argv = words(&[
            "-L",
            "claude-swarm-1",
            "set-option",
            "-p",
            "-t",
            "%2",
            "window-style",
            "bg=default",
            "--",
            "secret",
        ]);
        assert_eq!(
            redacted(&argv),
            "-L claude-swarm-1 set-option -p -t %2 window-style bg=... -- ..."
        );
        let json = Json::parse(&Reply::fail("a \"b\"").json()).unwrap();
        assert_eq!(json.get("err").and_then(Json::as_str), Some("a \"b\"\n"));
    }

    #[test]
    fn tmux_starts_a_teammate_with_no_shell() {
        let argv = words(&[r"C:\a b\claude.exe", "--agent-name", "it's"]);
        let want = match cfg!(windows) {
            true => r#""C:\a b\claude.exe" --agent-name it's"#,
            false => r#"exec 'C:\a b\claude.exe' '--agent-name' 'it'\''s'"#,
        };
        assert_eq!(cmdline(&argv), want);
    }
}
