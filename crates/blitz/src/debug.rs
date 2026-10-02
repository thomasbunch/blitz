//! Headless tools: a script runner over a real pseudoconsole, and the GUI
//! self-test.
//!
//! `blitz debug run` reads a script with one command per line. Blank lines
//! and lines starting with `#` are skipped. TEXT arguments take the escapes
//! `\e \r \n \t \s \\ \xNN`.
//!
//! - `send TEXT`: write TEXT to the child.
//! - `type TEXT`: the same, one character every 40 ms.
//! - `waitfor MS TEXT`: wait until TEXT shows up in the output after the
//!   previous match. Case, whitespace and escape sequences are ignored.
//! - `waitany MS A|B|...`: the same for any of several texts.
//! - `idle QUIET MAX`: wait until there has been no output for QUIET ms,
//!   but at most MAX ms.
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

use vt::Terminal;

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
        Ok(Runner { s, pty, seen: 0 })
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
            if let Err(e) = self.step(line) {
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
