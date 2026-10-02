//! Headless tools: a script runner over a real pseudoconsole, and the GUI
//! self-test.
//!
//! `blitz debug run` reads a script with one command per line. Blank lines
//! and lines starting with `#` are skipped. TEXT arguments take the escapes
//! `\e \r \n \t \s \\ \xNN`.
//!
//! - `send TEXT`: write TEXT to the child.
//! - `type TEXT`: the same, one character every 40 ms.
//! - `key CHORD`: press and release a chord such as `enter`,
//!   `shift+enter` or `ctrl+c`, encoded for the input modes the program
//!   has set.
//! - `paste TEXT` or `paste @FILE`: paste, bracketed if the program asked
//!   for it.
//! - `mouse wheelup|wheeldown|click COL ROW`: a mouse event at a 0-based
//!   cell, sent only if the program asked for mouse reports.
//! - `waitfor MS TEXT`: wait until TEXT shows up in the output after the
//!   previous match. Case, whitespace and escape sequences are ignored.
//! - `waitany MS A|B|...`: the same for any of several texts.
//! - `idle QUIET MAX`: wait until there has been no output for QUIET ms,
//!   but at most MAX ms.
//! - `expect [MS] REGEX`: wait until a screen row matches REGEX, checking
//!   after every chunk of output (3000 ms by default). See [`Regex`] for
//!   the syntax. No-break spaces in a row match as spaces, since
//!   Claude Code draws one after its `>` prompt.
//! - `save NAME`: store the cursor row; later lines expand `${NAME}`.
//! - `snap LABEL`: print the screen.
//! - `resize COLS ROWS`: resize the pseudoconsole and the screen.
//! - `latency N TEXT`: send TEXT N times and time how long each takes to
//!   come back.
//! - `modes`: print the input modes the program has set.
//! - `waitexit MS`: wait for the child to exit.
//! - `sleep MS`, `note TEXT`.
//!
//! A failed wait prints the screen and ends the run with exit code 1.

use std::fs::File;
use std::io::{self, Write};
use std::path::Path;
use std::str::FromStr;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, Once, PoisonError, mpsc};
use std::time::{Duration, Instant};

use vt::{Key, KeyInput, Locks, Mods, MouseEv, MouseKind, Terminal};
use windows::Win32::UI::Input::KeyboardAndMouse::{MAPVK_VK_TO_VSC, MapVirtualKeyW};

use crate::pty::{Pty, PtyEvent, SpawnOpts, Writer};

const USAGE: &str = "usage: blitz debug run --script FILE [--cmd CMD] [--cwd DIR] \
[--cols N --rows N] [--trace FILE] [--setenv K=V]... [--timeout MS]";

/// What the console host writes before any output of the child.
const PRELUDE: [&[u8]; 4] = [b"\x1b[1t", b"\x1b[c", b"\x1b[?1004h", b"\x1b[?9001h"];

/// `blitz debug run`: drives a session from a script. Returns the process
/// exit code: 0 when the script ran to the end, 1 when a step failed or the
/// run timed out, 2 for bad arguments.
pub fn run(args: &[String]) -> i32 {
    let input = Opts::parse(args).and_then(|o| {
        let script =
            std::fs::read_to_string(&o.script).map_err(|e| format!("{}: {e}", o.script))?;
        Ok((o, script))
    });
    let (opts, script) = match input {
        Ok(v) => v,
        Err(e) => {
            eprintln!("blitz debug run: {e}\n{USAGE}");
            return 2;
        }
    };
    // A run that fails or times out must not leave console hosts behind.
    static JOB: Once = Once::new();
    JOB.call_once(|| {
        if let Err(e) = crate::pty::kill_children_on_exit() {
            eprintln!("blitz debug run: no job object: {e}");
        }
    });
    let mut runner = match Runner::start(&opts) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("blitz debug run: {e}");
            return 1;
        }
    };

    let (done, finished) = mpsc::channel::<()>();
    let (shared, timeout) = (runner.s.clone(), opts.timeout);
    std::thread::spawn(move || {
        let waited = finished.recv_timeout(Duration::from_millis(timeout));
        if waited == Err(mpsc::RecvTimeoutError::Timeout) {
            shared.fail(&format!("timed out after {timeout} ms"));
            std::process::exit(1);
        }
    });
    let code = runner.script(&script);
    drop(done);
    runner.finish();
    code
}

struct Opts {
    script: String,
    cmd: Option<String>,
    cwd: Option<String>,
    cols: u16,
    rows: u16,
    trace: Option<String>,
    setenv: Vec<(String, String)>,
    timeout: u64,
}

impl Opts {
    fn parse(args: &[String]) -> Result<Opts, String> {
        let mut o = Opts {
            script: String::new(),
            cmd: None,
            cwd: None,
            cols: 120,
            rows: 40,
            trace: None,
            setenv: Vec::new(),
            timeout: 120_000,
        };
        let mut it = args.iter();
        while let Some(flag) = it.next() {
            let v = it
                .next()
                .ok_or_else(|| format!("{flag} needs a value"))?
                .clone();
            match flag.as_str() {
                "--script" => o.script = v,
                "--cmd" => o.cmd = Some(v),
                "--cwd" => o.cwd = Some(v),
                "--cols" => o.cols = num(&v)?,
                "--rows" => o.rows = num(&v)?,
                "--trace" => o.trace = Some(v),
                "--setenv" => {
                    let (k, val) = v.split_once('=').ok_or("--setenv takes K=V")?;
                    o.setenv.push((k.to_owned(), val.to_owned()));
                }
                "--timeout" => o.timeout = num(&v)?,
                _ => return Err(format!("unknown option {flag}")),
            }
        }
        if o.script.is_empty() {
            return Err("--script is required".into());
        }
        Ok(o)
    }
}

fn num<T: FromStr>(s: &str) -> Result<T, String> {
    s.trim().parse().map_err(|_| format!("not a number: {s:?}"))
}

fn two<T: FromStr>(s: &str) -> Result<(T, T), String> {
    let (a, b) = s
        .trim()
        .split_once(' ')
        .ok_or_else(|| format!("expected two numbers: {s:?}"))?;
    Ok((num(a)?, num(b)?))
}

/// Timings and counts for performance checks, written as JSON to the file
/// named by `BLITZ_TRACE` when a run ends.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Counters {
    /// From spawning the child to the first byte of output.
    pub first_pty_byte_ms: Option<f64>,
    /// From spawning the child to the first output that is more than the
    /// console host's prelude.
    pub first_post_prelude_ms: Option<f64>,
    /// From start-up to the first frame on screen.
    pub first_present_ms: Option<f64>,
    pub frames: u64,
    /// Times the UI thread woke up.
    pub wakeups: u64,
    /// Running on the system's ConPTY rather than the bundled one.
    pub inbox: bool,
}

impl Counters {
    pub fn to_json(&self) -> String {
        let ms = |v: Option<f64>| v.map_or_else(|| "null".to_owned(), |v| format!("{v:.1}"));
        format!(
            "{{\"first_pty_byte_ms\":{},\"first_post_prelude_ms\":{},\"first_present_ms\":{},\
             \"frames\":{},\"wakeups\":{},\"inbox\":{}}}",
            ms(self.first_pty_byte_ms),
            ms(self.first_post_prelude_ms),
            ms(self.first_present_ms),
            self.frames,
            self.wakeups,
            self.inbox,
        )
    }

    /// Writes the counters to the file named by `BLITZ_TRACE`, if it is set.
    pub fn write_trace(&self) -> io::Result<()> {
        match std::env::var_os("BLITZ_TRACE") {
            Some(path) => std::fs::write(path, self.to_json() + "\n"),
            None => Ok(()),
        }
    }
}

/// What the reader thread and the script share.
struct Shared {
    state: Mutex<State>,
    /// Signalled on every chunk of output and at exit.
    changed: Condvar,
    started: Instant,
}

struct State {
    term: Terminal,
    out: Vec<u8>,
    chunks: u64,
    /// When the last output arrived, in ms since start.
    last_ms: f64,
    counters: Counters,
    exit: Option<u32>,
    /// The `--trace` log: one `ms<TAB>kind<TAB>text` line per event.
    log: Option<File>,
}

impl State {
    fn log(&mut self, ms: f64, kind: &str, text: &str) {
        if let Some(f) = &mut self.log {
            let _ = f.write_all(format!("{ms:.1}\t{kind}\t{text}\n").as_bytes());
        }
    }
}

impl Shared {
    fn ms(&self) -> f64 {
        self.started.elapsed().as_secs_f64() * 1000.0
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn note(&self, text: &str) {
        let ms = self.ms();
        eprintln!("[{ms:.0}] {text}");
        self.lock().log(ms, "note", text);
    }

    /// Waits until `done` holds, the child exits or `ms` pass, checking
    /// again after every chunk of output.
    fn wait(&self, ms: u64, mut done: impl FnMut(&State) -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_millis(ms);
        let mut st = self.lock();
        loop {
            if done(&st) {
                return true;
            }
            let now = Instant::now();
            if now >= deadline || st.exit.is_some() {
                return false;
            }
            st = self
                .changed
                .wait_timeout(st, deadline - now)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// Reports a failed step with the screen and the last raw output.
    fn fail(&self, why: &str) {
        self.note(&format!("FAILED: {why}"));
        let st = self.lock();
        let tail = &st.out[st.out.len().saturating_sub(512)..];
        println!(
            "--- screen ---\n{}\n--- last output ---\n{}",
            st.term.screen_text(),
            esc(tail)
        );
    }

    fn on_event(&self, ev: PtyEvent<'_>, w: &Writer) {
        let ms = self.ms();
        let mut guard = self.lock();
        let st = &mut *guard;
        match ev {
            PtyEvent::Data(d) => {
                let c = &mut st.counters;
                c.first_pty_byte_ms.get_or_insert(ms);
                if c.first_post_prelude_ms.is_none() && past_prelude(d) {
                    c.first_post_prelude_ms = Some(ms);
                }
                st.log(ms, "out", &esc(d));
                st.term.feed(d);
                let mut reply = Vec::new();
                st.term.take_replies(&mut reply);
                if reply.is_empty() {
                    let (col, row, _) = st.term.cursor();
                    answer_queries(d, col, row, &mut reply);
                }
                if !reply.is_empty() {
                    st.log(ms, "ans", &esc(&reply));
                    w.send(reply);
                }
                let mut events = Vec::new();
                st.term.take_events(&mut events);
                for e in events {
                    let e = format!("{e:?}");
                    eprintln!("[{ms:.0}] event {e}");
                    st.log(ms, "event", &e);
                }
                st.out.extend_from_slice(d);
                st.chunks += 1;
                st.last_ms = ms;
            }
            PtyEvent::Exit(code) => {
                st.exit = Some(code);
                st.log(ms, "note", &format!("exit {code}"));
            }
        }
        drop(guard);
        self.changed.notify_all();
    }
}

/// Answers the queries programs send at start-up when the terminal left
/// them unanswered, so a run never stalls waiting for a reply.
fn answer_queries(chunk: &[u8], col: u16, row: u16, out: &mut Vec<u8>) {
    const DA1: &[u8] = b"\x1b[?62;22c";
    let cpr = format!("\x1b[{};{}R", row + 1, col + 1);
    let version = format!("\x1bP>|blitz {}\x1b\\", env!("CARGO_PKG_VERSION"));
    let table: [(&[u8], &[u8]); 11] = [
        (b"\x1b[c", DA1),
        (b"\x1b[0c", DA1),
        (b"\x1b[>c", b"\x1b[>1;0;0c"),
        (b"\x1b[6n", cpr.as_bytes()),
        (b"\x1b[?u", b"\x1b[?0u"),
        (b"\x1b[>q", version.as_bytes()),
        (b"\x1b[>0q", version.as_bytes()),
        (b"\x1b]10;?", b"\x1b]10;rgb:cccc/cccc/cccc\x1b\\"),
        (b"\x1b]11;?", b"\x1b]11;rgb:1e1e/1e1e/1e1e\x1b\\"),
        (b"\x1b[?2026$p", b"\x1b[?2026;2$y"),
        (b"\x1b[?1016$p", b"\x1b[?1016;0$y"),
    ];
    for (query, answer) in table {
        if find(chunk, query).is_some() {
            out.extend_from_slice(answer);
        }
    }
}

struct Runner {
    s: Arc<Shared>,
    pty: Pty,
    /// `waitfor` looks for text after this offset in the output.
    seen: usize,
    /// Values stored by `save`, expanded as `${name}`.
    vars: Vec<(String, String)>,
}

impl Runner {
    fn start(o: &Opts) -> Result<Runner, String> {
        let (cmdline, mut env) = match &o.cmd {
            Some(c) => (c.clone(), Vec::new()),
            None => {
                let l = crate::shell::launch("", &[], true);
                (l.cmdline, l.env)
            }
        };
        env.extend(o.setenv.iter().cloned());
        let log = match &o.trace {
            Some(path) => Some(File::create(path).map_err(|e| format!("{path}: {e}"))?),
            None => None,
        };
        let term = Terminal::new(vt::Options {
            cols: o.cols,
            rows: o.rows,
            ..Default::default()
        });
        let s = Arc::new(Shared {
            state: Mutex::new(State {
                term,
                out: Vec::new(),
                chunks: 0,
                last_ms: 0.0,
                counters: Counters::default(),
                exit: None,
                log,
            }),
            changed: Condvar::new(),
            started: Instant::now(),
        });
        let header = format!("cmd={cmdline} size={}x{}", o.cols, o.rows);
        s.lock().log(0.0, "note", &header);
        let opts = SpawnOpts {
            cmdline: &cmdline,
            cwd: o.cwd.as_deref().map(Path::new),
            env: &env,
            cols: o.cols,
            rows: o.rows,
            pane_id: 1,
            parent: None,
        };
        let reader = s.clone();
        let pty = Pty::spawn(&opts, move |ev, w| reader.on_event(ev, w))
            .map_err(|e| format!("cannot start {cmdline}: {e}"))?;
        Ok(Runner {
            s,
            pty,
            seen: 0,
            vars: Vec::new(),
        })
    }

    fn send(&self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let ms = self.s.ms();
        self.s.lock().log(ms, "in", &esc(bytes));
        self.pty.writer().send(bytes);
    }

    fn script(&mut self, script: &str) -> i32 {
        for (n, line) in script.lines().enumerate() {
            let line = line.trim_end();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut line = line.to_owned();
            for (name, value) in &self.vars {
                line = line.replace(&format!("${{{name}}}"), value);
            }
            if let Err(e) = self.step(&line) {
                self.s.fail(&format!("line {}: {e}", n + 1));
                return 1;
            }
        }
        0
    }

    fn step(&mut self, line: &str) -> Result<(), String> {
        let (cmd, rest) = line.split_once(' ').unwrap_or((line, ""));
        match cmd {
            "send" => self.send(&unesc(rest)),
            "type" => {
                for c in String::from_utf8_lossy(&unesc(rest)).chars() {
                    self.send(c.encode_utf8(&mut [0; 4]).as_bytes());
                    std::thread::sleep(Duration::from_millis(40));
                }
            }
            "key" => {
                let mut text = String::new();
                let keys = chord(rest, &mut text)?;
                let modes = self.s.lock().term.input_modes();
                let mut out = Vec::new();
                for k in &keys {
                    vt::encode_key(k, &modes, &mut out);
                }
                self.send(&out);
                self.s.note(&format!("key {rest}: {}", esc(&out)));
            }
            "paste" => {
                let text = match rest.strip_prefix('@') {
                    Some(path) => {
                        std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?
                    }
                    None => String::from_utf8_lossy(&unesc(rest)).into_owned(),
                };
                let bracketed = self.s.lock().term.input_modes().bracketed;
                let mut out = Vec::new();
                vt::encode_paste(&text, bracketed, &mut out);
                self.send(&out);
            }
            "mouse" => {
                let (kind, cell) = rest
                    .split_once(' ')
                    .ok_or("mouse wheelup|wheeldown|click COL ROW")?;
                let (col, row) = two(cell)?;
                let kinds: &[MouseKind] = match kind {
                    "wheelup" => &[MouseKind::WheelUp],
                    "wheeldown" => &[MouseKind::WheelDown],
                    "click" => &[MouseKind::Press, MouseKind::Release],
                    _ => return Err(format!("unknown mouse event {kind:?}")),
                };
                let modes = self.s.lock().term.input_modes();
                let mut out = Vec::new();
                for &kind in kinds {
                    let ev = MouseEv {
                        kind,
                        button: 0,
                        col,
                        row,
                        mods: Mods::default(),
                    };
                    vt::encode_mouse(ev, &modes, &mut out);
                }
                self.send(&out);
                let sent = match out.is_empty() {
                    true => "nothing, mouse reporting is off".to_owned(),
                    false => esc(&out),
                };
                self.s.note(&format!("mouse {rest}: {sent}"));
            }
            "waitfor" | "waitany" => {
                let (ms, text) = rest
                    .split_once(' ')
                    .ok_or_else(|| format!("{cmd} MS TEXT"))?;
                let wanted: Vec<&str> = match cmd {
                    "waitany" => text.split('|').collect(),
                    _ => vec![text],
                };
                let squashed: Vec<String> = wanted.iter().map(|t| squash(t)).collect();
                let seen = self.seen;
                let mut from = seen;
                let mut hit = None;
                self.s.wait(num(ms)?, |st| {
                    let hay = plain(&st.out[from..]);
                    // Rescan a little of the old output in case a match
                    // straddles two chunks.
                    from = st.out.len().saturating_sub(4096).max(seen);
                    hit = squashed
                        .iter()
                        .position(|w| hay.contains(w.as_str()))
                        .map(|i| (i, st.out.len()));
                    hit.is_some()
                });
                let (i, end) = hit.ok_or_else(|| format!("{text:?} did not show up in {ms} ms"))?;
                self.seen = end;
                self.s.note(&format!("{cmd} ok: {}", wanted[i]));
            }
            "idle" => {
                let (quiet, max): (f64, f64) = two(rest)?;
                let start = self.s.ms();
                loop {
                    std::thread::sleep(Duration::from_millis(20));
                    let st = self.s.lock();
                    let now = self.s.ms();
                    if now - st.last_ms.max(start) >= quiet
                        || now - start >= max
                        || st.exit.is_some()
                    {
                        break;
                    }
                }
                let waited = self.s.ms() - start;
                self.s.note(&format!("idle after {waited:.0} ms"));
            }
            "snap" => {
                let screen = self.s.lock().term.screen_text();
                println!("--- snap {rest} ---\n{screen}");
                self.s.note(&format!("snap {rest}"));
            }
            "resize" => {
                let (cols, rows) = two(rest)?;
                self.pty.resize(cols, rows);
                self.s.lock().term.resize(cols, rows);
                self.s.note(&format!("resize {cols}x{rows}"));
            }
            "latency" => {
                let (n, text) = rest.split_once(' ').ok_or("latency N TEXT")?;
                let echo = unesc(text);
                let mut times = Vec::new();
                for _ in 0..num::<u32>(n)? {
                    let from = self.s.lock().out.len();
                    let t = Instant::now();
                    self.pty.writer().send(echo.as_slice());
                    self.s
                        .wait(2000, |st| find(&st.out[from..], &echo).is_some());
                    times.push(t.elapsed().as_secs_f64() * 1000.0);
                    std::thread::sleep(Duration::from_millis(30));
                }
                times.sort_by(f64::total_cmp);
                let at = |q: usize| times.get(times.len() * q / 100).copied().unwrap_or(0.0);
                self.s.note(&format!(
                    "latency n={} min={:.2} p50={:.2} p90={:.2} max={:.2} ms",
                    times.len(),
                    at(0),
                    at(50),
                    at(90),
                    times.last().copied().unwrap_or(0.0),
                ));
            }
            "expect" => {
                let (ms, pattern) = match rest.split_once(' ') {
                    Some((ms, p)) if ms.bytes().all(|b| b.is_ascii_digit()) => (num(ms)?, p),
                    _ => (3000, rest),
                };
                let re = Regex::new(pattern)?;
                if !self
                    .s
                    .wait(ms, |st| re.matches_a_row(&st.term.screen_text()))
                {
                    return Err(format!("no row matched {pattern:?} within {ms} ms"));
                }
                self.s.note(&format!("expect ok: {pattern}"));
            }
            "save" => {
                let name = rest.trim();
                let (_, row, _) = self.s.lock().term.cursor();
                self.vars.retain(|(n, _)| n != name);
                self.vars.push((name.to_owned(), row.to_string()));
                self.s.note(&format!("save {name} = {row}"));
            }
            "modes" => {
                let modes = self.s.lock().term.input_modes();
                self.s.note(&format!("modes {modes:?}"));
            }
            "waitexit" => {
                let ms = num(rest)?;
                if !self.s.wait(ms, |st| st.exit.is_some()) {
                    return Err(format!("still running after {ms} ms"));
                }
                let code = self.s.lock().exit.unwrap_or_default();
                self.s.note(&format!("exited with code {code}"));
            }
            "sleep" => std::thread::sleep(Duration::from_millis(num(rest)?)),
            "note" => self.s.note(rest),
            _ => return Err(format!("unknown command {cmd:?}")),
        }
        Ok(())
    }

    /// Prints a summary and writes the counters to `BLITZ_TRACE`.
    fn finish(&self) {
        let summary = {
            let mut st = self.s.lock();
            st.counters.inbox = crate::pty::inbox_notice().is_some();
            let c = &st.counters;
            if let Err(e) = c.write_trace() {
                eprintln!("blitz debug run: BLITZ_TRACE: {e}");
            }
            let ms = |v: Option<f64>| v.map_or_else(|| "none".to_owned(), |v| format!("{v:.1}"));
            format!(
                "exit={:?} out_bytes={} chunks={} first_pty_byte_ms={} first_post_prelude_ms={} conpty={}",
                st.exit,
                st.out.len(),
                st.chunks,
                ms(c.first_pty_byte_ms),
                ms(c.first_post_prelude_ms),
                if c.inbox { "inbox" } else { "bundled" },
            )
        };
        self.s.note(&summary);
    }
}

/// The key transitions for a chord such as `shift+enter`, as a keyboard
/// would produce them: each modifier goes down, the key goes down and up,
/// then the modifiers go up in reverse order. Names are case-insensitive;
/// the key is a name (`enter`, `esc`, `up`, `f5`, ...) or one character.
/// The inputs borrow their text from `text`.
fn chord<'a>(spec: &str, text: &'a mut String) -> Result<Vec<KeyInput<'a>>, String> {
    let spec = spec.trim().to_ascii_lowercase();
    // A trailing "+" is the plus key: "+", "ctrl++".
    let (modifiers, name) = match spec.strip_suffix('+') {
        Some(rest) => (rest.strip_suffix('+').unwrap_or(rest), "+"),
        None => spec.rsplit_once('+').unwrap_or(("", &spec)),
    };
    let mut held = Vec::new();
    for m in modifiers.split('+').filter(|m| !m.is_empty()) {
        held.push(match m {
            "shift" => (Key::Shift, 0x10),
            "ctrl" => (Key::Control, 0x11),
            "alt" => (Key::Alt, 0x12),
            "win" | "super" => (Key::Super, 0x5b),
            _ => return Err(format!("unknown modifier {m:?}")),
        });
    }
    // Virtual-key code, and the character ToUnicodeEx gives with no
    // modifiers held.
    let (key, vk, uc) = match name {
        "enter" => (Key::Enter, 0x0d, 13),
        "tab" => (Key::Tab, 0x09, 9),
        "backspace" | "bs" => (Key::Backspace, 0x08, 8),
        "esc" | "escape" => (Key::Escape, 0x1b, 27),
        "space" => (Key::Char(' '), 0x20, 32),
        "pgup" | "pageup" => (Key::PageUp, 0x21, 0),
        "pgdn" | "pagedown" => (Key::PageDown, 0x22, 0),
        "end" => (Key::End, 0x23, 0),
        "home" => (Key::Home, 0x24, 0),
        "left" => (Key::Left, 0x25, 0),
        "up" => (Key::Up, 0x26, 0),
        "right" => (Key::Right, 0x27, 0),
        "down" => (Key::Down, 0x28, 0),
        "insert" | "ins" => (Key::Insert, 0x2d, 0),
        "delete" | "del" => (Key::Delete, 0x2e, 0),
        f if f.len() > 1 && f.starts_with('f') && f[1..].parse::<u8>().is_ok() => {
            let n: u8 = num(&f[1..])?;
            if !(1..=24).contains(&n) {
                return Err(format!("no key {f:?}"));
            }
            (Key::F(n), 0x6f + u16::from(n), 0)
        }
        _ => {
            let mut chars = name.chars();
            let (Some(c), None) = (chars.next(), chars.next()) else {
                return Err(format!("unknown key {name:?}"));
            };
            // Letters and digits have layout-independent codes; other
            // characters go out as text only.
            let vk = match c.is_ascii_alphanumeric() {
                true => c.to_ascii_uppercase() as u16,
                false => 0,
            };
            (Key::Char(c), vk, 0)
        }
    };
    let held_down = |k: Key| held.iter().any(|&(h, _)| h == k);
    let (shift, ctrl) = (held_down(Key::Shift), held_down(Key::Control));
    text.clear();
    if let Key::Char(c) = key {
        text.push(if shift { c.to_ascii_uppercase() } else { c });
    }
    let text: &'a str = text;
    let uc = match key {
        Key::Char(c) if ctrl && c.is_ascii_alphabetic() => c as u16 & 0x1f,
        Key::Char(_) => text.encode_utf16().next().unwrap_or(0),
        _ => uc,
    };
    let input = |key: Key, vk: u16, down: bool, mods: Mods, text: &'a str, uc: u16| KeyInput {
        vk,
        // SAFETY: a table lookup with no pointers involved.
        scan: unsafe { MapVirtualKeyW(u32::from(vk), MAPVK_VK_TO_VSC) } as u16,
        extended: matches!(vk, 0x21..=0x28 | 0x2d | 0x2e | 0x5b),
        down,
        repeat: 1,
        mods,
        locks: Locks::default(),
        text,
        uc,
        cs: 0,
        key,
        us_base: match key {
            Key::Char(c) if c.is_ascii() => Some(c),
            _ => None,
        },
    };
    let press = |mods: &mut Mods, k: Key, down: bool| match k {
        Key::Shift => mods.lshift = down,
        Key::Control => mods.lctrl = down,
        Key::Alt => mods.lalt = down,
        _ => mods.lsuper = down,
    };
    let mut mods = Mods::default();
    let mut out = Vec::new();
    for &(k, vk) in &held {
        press(&mut mods, k, true);
        out.push(input(k, vk, true, mods, "", 0));
    }
    out.push(input(key, vk, true, mods, text, uc));
    out.push(input(key, vk, false, mods, text, uc));
    for &(k, vk) in held.iter().rev() {
        press(&mut mods, k, false);
        out.push(input(k, vk, false, mods, "", 0));
    }
    Ok(out)
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

/// True if `chunk` holds anything besides the console host's prelude.
fn past_prelude(chunk: &[u8]) -> bool {
    let mut rest = chunk.to_vec();
    for p in PRELUDE {
        while let Some(i) = find(&rest, p) {
            rest.drain(i..i + p.len());
        }
    }
    !rest.is_empty()
}

/// Bytes as printable ASCII, with the escapes `unesc` reads.
fn esc(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len());
    for &c in b {
        match c {
            0x1b => s.push_str("\\e"),
            b'\\' => s.push_str("\\\\"),
            b'\r' => s.push_str("\\r"),
            b'\n' => s.push_str("\\n"),
            0x20..=0x7e => s.push(c as char),
            _ => s.push_str(&format!("\\x{c:02x}")),
        }
    }
    s
}

fn unesc(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let (c, next) = (b[i], b.get(i + 1).copied());
        i += 1;
        if c != b'\\' || next.is_none() {
            out.push(c);
            continue;
        }
        i += 1;
        match next.unwrap_or_default() {
            b'e' => out.push(0x1b),
            b'r' => out.push(b'\r'),
            b'n' => out.push(b'\n'),
            b't' => out.push(b'\t'),
            b's' => out.push(b' '),
            b'\\' => out.push(b'\\'),
            b'x' => match s.get(i..i + 2).and_then(|h| u8::from_str_radix(h, 16).ok()) {
                Some(v) => {
                    out.push(v);
                    i += 2;
                }
                None => out.extend_from_slice(b"\\x"),
            },
            other => out.extend_from_slice(&[b'\\', other]),
        }
    }
    out
}

/// Output with escape sequences, whitespace and control bytes removed and
/// ASCII lowercased, for "did this text appear" checks.
fn plain(b: &[u8]) -> String {
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == 0x1b && i + 1 < b.len() {
            let kind = b[i + 1];
            i += 2;
            match kind {
                b'[' => {
                    while i < b.len() && !(0x40..=0x7e).contains(&b[i]) {
                        i += 1;
                    }
                    i += 1;
                }
                b']' | b'P' | b'_' | b'^' => {
                    while i < b.len()
                        && b[i] != 7
                        && !(b[i] == 0x1b && b.get(i + 1) == Some(&b'\\'))
                    {
                        i += 1;
                    }
                    i += if b.get(i) == Some(&7) { 1 } else { 2 };
                }
                _ => {}
            }
            continue;
        }
        if b[i] > 0x20 {
            out.push(b[i].to_ascii_lowercase());
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A small regular expression for `expect`, matched against one screen
/// row at a time: literals, `.`, classes such as `[a-z]`, `[^ ]`, `\d`,
/// `\s` and `\w` (and `\D`, `\S`, `\W`), groups with `|`, the quantifiers
/// `*`, `+`, `?`, `{n}`, `{n,}` and `{n,m}`, and the anchors `^` and `$`.
/// Other characters after `\` stand for themselves.
///
/// Matching backtracks, which can be slow on pathological patterns but is
/// fine for screen rows of a few hundred characters.
pub(crate) struct Regex(Vec<Re>);

enum Re {
    Char(char),
    Any,
    /// Inclusive ranges; the flag negates the set.
    Set(Vec<(char, char)>, bool),
    Start,
    End,
    Group(Vec<Vec<Re>>),
    Repeat(Box<Re>, u32, u32),
}

impl Regex {
    pub(crate) fn new(pattern: &str) -> Result<Regex, String> {
        let p: Vec<char> = pattern.chars().collect();
        let mut i = 0;
        let alts = re_alts(&p, &mut i)?;
        if i < p.len() {
            return Err(format!("unmatched ) in {pattern:?}"));
        }
        Ok(Regex(vec![Re::Group(alts)]))
    }

    fn is_match(&self, row: &str) -> bool {
        let s: Vec<char> = row.chars().collect();
        (0..=s.len()).any(|i| re_seq(&self.0, &s, i, &|_| true))
    }

    pub(crate) fn matches_a_row(&self, screen: &str) -> bool {
        screen
            .lines()
            .any(|row| self.is_match(&row.replace('\u{a0}', " ")))
    }
}

fn re_alts(p: &[char], i: &mut usize) -> Result<Vec<Vec<Re>>, String> {
    let mut alts = vec![Vec::new()];
    while let Some(&c) = p.get(*i) {
        *i += 1;
        let atom = match c {
            '|' => {
                alts.push(Vec::new());
                continue;
            }
            ')' => {
                *i -= 1;
                break;
            }
            '(' => {
                let inner = re_alts(p, i)?;
                if p.get(*i) != Some(&')') {
                    return Err("missing )".into());
                }
                *i += 1;
                Re::Group(inner)
            }
            '[' => re_set(p, i)?,
            '\\' => {
                let e = *p.get(*i).ok_or("pattern ends with \\")?;
                *i += 1;
                match re_class(e) {
                    Some((ranges, negated)) => Re::Set(ranges, negated),
                    None => Re::Char(e),
                }
            }
            '.' => Re::Any,
            '^' => Re::Start,
            '$' => Re::End,
            '*' | '+' | '?' | '{' => return Err(format!("nothing to repeat before {c:?}")),
            c => Re::Char(c),
        };
        let atom = re_quantifier(p, i, atom)?;
        if let Some(seq) = alts.last_mut() {
            seq.push(atom);
        }
    }
    Ok(alts)
}

fn re_quantifier(p: &[char], i: &mut usize, atom: Re) -> Result<Re, String> {
    let (min, max) = match p.get(*i) {
        Some('*') => (0, u32::MAX),
        Some('+') => (1, u32::MAX),
        Some('?') => (0, 1),
        Some('{') => {
            let close = p[*i..].iter().position(|&c| c == '}').ok_or("missing }")? + *i;
            let body: String = p[*i + 1..close].iter().collect();
            *i = close;
            match body.split_once(',') {
                None => (num(&body)?, num(&body)?),
                Some((lo, "")) => (num(lo)?, u32::MAX),
                Some((lo, hi)) => (num(lo)?, num(hi)?),
            }
        }
        _ => return Ok(atom),
    };
    *i += 1;
    Ok(Re::Repeat(Box::new(atom), min, max))
}

/// `[...]` after the opening bracket.
fn re_set(p: &[char], i: &mut usize) -> Result<Re, String> {
    let negated = p.get(*i) == Some(&'^');
    if negated {
        *i += 1;
    }
    let mut ranges = Vec::new();
    let mut first = true;
    loop {
        let mut c = *p.get(*i).ok_or("missing ]")?;
        *i += 1;
        if c == ']' && !first {
            return Ok(Re::Set(ranges, negated));
        }
        first = false;
        if c == '\\' {
            c = *p.get(*i).ok_or("missing ]")?;
            *i += 1;
            match re_class(c) {
                Some((r, false)) => {
                    ranges.extend(r);
                    continue;
                }
                Some(_) => return Err(format!("\\{c} inside [...] is not supported")),
                None => {}
            }
        }
        match (p.get(*i), p.get(*i + 1)) {
            (Some('-'), Some(&hi)) if hi != ']' => {
                ranges.push((c, hi));
                *i += 2;
            }
            _ => ranges.push((c, c)),
        }
    }
}

/// `\d`, `\s`, `\w` and their negated capitals.
fn re_class(e: char) -> Option<(Vec<(char, char)>, bool)> {
    let ranges = match e.to_ascii_lowercase() {
        'd' => vec![('0', '9')],
        's' => vec![(' ', ' '), ('\t', '\r')],
        'w' => vec![('a', 'z'), ('A', 'Z'), ('0', '9'), ('_', '_')],
        _ => return None,
    };
    Some((ranges, e.is_ascii_uppercase()))
}

/// Matches `seq` at `s[i..]`, then hands the end position to `k`.
fn re_seq(seq: &[Re], s: &[char], i: usize, k: &dyn Fn(usize) -> bool) -> bool {
    match seq.split_first() {
        None => k(i),
        Some((Re::Repeat(r, min, max), rest)) => re_repeat(r, *min, *max, rest, s, i, k),
        Some((r, rest)) => re_one(r, s, i, &|j| re_seq(rest, s, j, k)),
    }
}

fn re_one(r: &Re, s: &[char], i: usize, k: &dyn Fn(usize) -> bool) -> bool {
    let c = s.get(i).copied();
    match r {
        Re::Char(want) => c == Some(*want) && k(i + 1),
        Re::Any => c.is_some() && k(i + 1),
        Re::Set(ranges, negated) => {
            c.is_some_and(|c| ranges.iter().any(|&(lo, hi)| (lo..=hi).contains(&c)) != *negated)
                && k(i + 1)
        }
        Re::Start => i == 0 && k(i),
        Re::End => i == s.len() && k(i),
        Re::Group(alts) => alts.iter().any(|seq| re_seq(seq, s, i, k)),
        Re::Repeat(r, min, max) => re_repeat(r, *min, *max, &[], s, i, k),
    }
}

/// Greedy: takes as many repeats as it can, then backs off one at a time.
fn re_repeat(
    r: &Re,
    min: u32,
    max: u32,
    rest: &[Re],
    s: &[char],
    i: usize,
    k: &dyn Fn(usize) -> bool,
) -> bool {
    // A repeat that consumes nothing would loop forever.
    let more = max > 0
        && re_one(r, s, i, &|j| {
            j > i && re_repeat(r, min.saturating_sub(1), max - 1, rest, s, j, k)
        });
    more || (min == 0 && re_seq(rest, s, i, k))
}

/// `text` the way [`plain`] leaves it.
fn squash(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_text_helpers() {
        assert_eq!(
            unesc(r"a\e[1m\r\n\x41\\\s\q\x4"),
            b"a\x1b[1m\r\nA\\ \\q\\x4"
        );
        assert_eq!(esc(b"\x1b[1mA\\\r\n\x07"), r"\e[1mA\\\r\n\x07");
        assert_eq!(
            plain(b"\x1b[1mHello \x1b]0;title\x07Wo\x1b]8;;x\x1b\\rld\r\n"),
            "helloworld"
        );
        assert_eq!(squash("Claude Code"), "claudecode");
        assert!(!past_prelude(b"\x1b[1t\x1b[c\x1b[?1004h"));
        assert!(past_prelude(b"\x1b[1thi"));
    }

    #[test]
    fn debug_regex() {
        let rule90 = "─".repeat(90);
        let cases: &[(&str, &str, bool)] = &[
            ("^> line1$", "> line1", true),
            ("^> line1$", ">  line1", false),
            ("^> line1$", "> line1 x", false),
            ("^  line2$", "  line2", true),
            ("^  line2$", " line2", false),
            (r"^>\s*$", ">", true),
            (r"^>\s*$", ">   ", true),
            (r"^>\s*$", "> a", false),
            (
                r"\[Pasted text #1 \+1(19|20) lines\]",
                "> [Pasted text #1 +119 lines] ",
                true,
            ),
            (
                r"\[Pasted text #1 \+1(19|20) lines\]",
                "[Pasted text #1 +120 lines]",
                true,
            ),
            (
                r"\[Pasted text #1 \+1(19|20) lines\]",
                "[Pasted text #1 +121 lines]",
                false,
            ),
            ("^─{90}$", &rule90, true),
            ("^─{90}$", &rule90[3..], false),
            ("^─{90}$", &(rule90.clone() + "─"), false),
            ("^● ok$", "● ok", true),
            ("^1$", "11", false),
            (">>", "PS C:\\> >> ", true),
            ("a.*b", "xxaxxbyy", true),
            ("a.*b", "bxxa", false),
            ("colou?r", "color", true),
            ("colou?r", "colouur", false),
            (r"^\d{2,3}$", "123", true),
            (r"^\d{2,3}$", "1234", false),
            (r"^\d{2,}$", "12345", true),
            ("[^a-c]x", "ax bx dx", true),
            ("[^a-c]x", "ax bx", false),
            (r"[\d_]+z", "a_9z", true),
            ("^(ab)+$", "ababab", true),
            ("^(ab)+$", "ababa", false),
            ("^$", "", true),
        ];
        for &(pattern, row, want) in cases {
            let re = Regex::new(pattern).unwrap();
            assert_eq!(re.is_match(row), want, "{pattern:?} on {row:?}");
        }
        assert!(Regex::new("^> a$").unwrap().matches_a_row("x\n> a\ny"));
        assert!(Regex::new("^> a$").unwrap().matches_a_row(">\u{a0}a"));
        assert!(Regex::new(r"^>\s*$").unwrap().matches_a_row(">\u{a0}"));
        for bad in ["(a", "a)", "*a", "[ab", "a{2", r"a\"] {
            assert!(Regex::new(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn debug_key_chords() {
        let mut text = String::new();
        let keys = chord("Shift+Enter", &mut text).unwrap();
        let got: Vec<_> = keys
            .iter()
            .map(|k| (k.key, k.vk, k.scan, k.down, k.mods.lshift, k.uc))
            .collect();
        assert_eq!(
            got,
            [
                (Key::Shift, 0x10, 0x2a, true, true, 0),
                (Key::Enter, 0x0d, 0x1c, true, true, 13),
                (Key::Enter, 0x0d, 0x1c, false, true, 13),
                (Key::Shift, 0x10, 0x2a, false, false, 0),
            ]
        );

        let keys = chord("ctrl+c", &mut text).unwrap();
        let c = &keys[1];
        assert_eq!((c.key, c.vk, c.text, c.uc), (Key::Char('c'), 0x43, "c", 3));
        assert!(c.mods.lctrl && !keys[3].mods.lctrl);
        assert_eq!(c.us_base, Some('c'));

        let keys = chord("shift+a", &mut text).unwrap();
        assert_eq!((keys[1].text, keys[1].uc), ("A", u16::from(b'A')));

        let keys = chord("up", &mut text).unwrap();
        assert_eq!(keys.len(), 2);
        assert!(keys[0].extended && keys[0].text.is_empty());

        let keys = chord("ctrl++", &mut text).unwrap();
        assert_eq!((keys[1].key, keys[1].mods.lctrl), (Key::Char('+'), true));
        assert_eq!(chord("f5", &mut text).unwrap()[0].vk, 0x74);
        // Through the encoder, Shift+Enter is what Claude Code and
        // PSReadLine expect for a newline.
        let encode = |spec: &str, modes: vt::InputModes| {
            let mut text = String::new();
            let mut out = Vec::new();
            for k in chord(spec, &mut text).unwrap() {
                vt::encode_key(&k, &modes, &mut out);
            }
            String::from_utf8(out).unwrap()
        };
        let kitty = vt::InputModes {
            kitty: 5,
            ..Default::default()
        };
        let w32im = vt::InputModes {
            w32im: true,
            ..Default::default()
        };
        assert_eq!(encode("shift+enter", kitty), "\x1b[13;2u");
        assert_eq!(
            encode("shift+enter", w32im),
            "\x1b[16;42;0;1;16;1_\x1b[13;28;13;1;16;1_\x1b[13;28;13;0;16;1_\x1b[16;42;0;0;0;1_"
        );
        assert_eq!(encode("ctrl+c", Default::default()), "\x03");

        assert!(chord("hyper+a", &mut text).is_err());
        assert!(chord("f25", &mut text).is_err());
        assert!(chord("nosuchkey", &mut text).is_err());
    }

    #[test]
    fn debug_answers_startup_queries() {
        let mut out = Vec::new();
        answer_queries(b"\x1b[1t\x1b[c", 0, 0, &mut out);
        assert_eq!(out, b"\x1b[?62;22c");
        out.clear();
        answer_queries(b"\x1b[?u\x1b[6n", 4, 2, &mut out);
        assert_eq!(out, b"\x1b[3;5R\x1b[?0u");
    }

    #[test]
    fn debug_counters_json() {
        let c = Counters {
            first_pty_byte_ms: Some(13.0),
            first_post_prelude_ms: Some(40.0),
            frames: 3,
            ..Default::default()
        };
        assert_eq!(
            c.to_json(),
            "{\"first_pty_byte_ms\":13.0,\"first_post_prelude_ms\":40.0,\"first_present_ms\":null,\
             \"frames\":3,\"wakeups\":0,\"inbox\":false}"
        );
    }

    #[test]
    fn debug_options() {
        let args: Vec<String> = ["--script", "s.txt", "--cols", "90", "--setenv", "A=b=c"]
            .map(String::from)
            .to_vec();
        let o = Opts::parse(&args).unwrap();
        assert_eq!((o.script.as_str(), o.cols, o.rows), ("s.txt", 90, 40));
        assert_eq!(o.setenv, [("A".to_owned(), "b=c".to_owned())]);
        assert!(Opts::parse(&args[2..]).is_err());
        assert!(Opts::parse(&["--script".to_owned()]).is_err());
    }
}
