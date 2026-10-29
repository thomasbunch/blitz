//! A pseudo terminal on macOS and Linux: opening, spawning, the I/O
//! threads, resize and shutdown.

use std::ffi::OsString;
use std::fs::File;
use std::io::{self, Read};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::{PtyEvent, SpawnOpts, Writer, foreign, lock, with_path};

/// The most one read of the terminal takes.
const READ_BYTES: usize = 128 * 1024;
/// The code reported for a child still running after its terminal closed,
/// as Windows reports `STILL_ACTIVE`.
const STILL_ACTIVE: u32 = 259;
/// How long a child asked to exit gets before it is killed.
const CLOSE_GRACE: Duration = Duration::from_secs(6);

/// Only Windows can fall back to a slower terminal.
pub fn inbox_notice() -> Option<&'static str> {
    None
}

/// A child here gets SIGHUP when its pseudoterminal closes, which is when
/// blitz exits; nothing to set up.
pub fn kill_children_on_exit() -> io::Result<()> {
    Ok(())
}

/// A new secret for one pane: 128 bits from the system's random number
/// generator, in hex. See the Windows version for what it is for.
pub fn pane_token() -> io::Result<String> {
    let mut bytes = [0u8; 16];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// A child process in its own session, on its own pseudo terminal.
pub struct Pty {
    /// The terminal's controlling side, for resizes.
    master: OwnedFd,
    child: Arc<Mutex<Child>>,
    writer: Writer,
    closing: AtomicBool,
}

fn winsize(cols: u16, rows: u16) -> libc::winsize {
    libc::winsize {
        ws_row: rows.max(1),
        ws_col: cols.max(1),
        ws_xpixel: 0,
        ws_ypixel: 0,
    }
}

/// Opens a pseudo terminal of `cols` by `rows`, as (master, slave). Neither
/// end leaks into other children: both are opened close-on-exec, at once,
/// so a program another thread starts meanwhile cannot take them along;
/// the slave is handed to this one as its standard streams only.
fn open(cols: u16, rows: u16) -> io::Result<(OwnedFd, OwnedFd)> {
    let flags = libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC;
    let m = open_fd(c"/dev/ptmx", flags)?;
    // SAFETY: an open pseudo terminal master; neither call forks here, as
    // the kernel's devpts or ioctl does the work.
    if unsafe { libc::grantpt(m.as_raw_fd()) } < 0 || unsafe { libc::unlockpt(m.as_raw_fd()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    let s = open_fd(&slave_name(&m)?, flags)?;
    let ws = winsize(cols, rows);
    // SAFETY: an open terminal and a valid size, which the call only reads.
    if unsafe { libc::ioctl(m.as_raw_fd(), libc::TIOCSWINSZ, &ws) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((m, s))
}

fn open_fd(path: &std::ffi::CStr, flags: libc::c_int) -> io::Result<OwnedFd> {
    // SAFETY: a NUL-terminated path; the descriptor is owned right away.
    let fd = unsafe { libc::open(path.as_ptr(), flags) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: open returned a descriptor nothing else owns.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// The path of `master`'s slave, read into a buffer of blitz's own, as
/// `ptsname` shares one between threads.
fn slave_name(master: &OwnedFd) -> io::Result<std::ffi::CString> {
    let mut buf = [0 as libc::c_char; 128];
    #[cfg(target_os = "macos")]
    {
        // SAFETY: an open master and a buffer of the 128 bytes the call
        // fills.
        let r = unsafe {
            libc::ioctl(
                master.as_raw_fd(),
                libc::TIOCPTYGNAME.into(),
                buf.as_mut_ptr(),
            )
        };
        if r < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        // SAFETY: an open master, and the buffer's real length.
        let r = unsafe { libc::ptsname_r(master.as_raw_fd(), buf.as_mut_ptr(), buf.len()) };
        // It returns the error rather than setting errno.
        if r != 0 {
            return Err(io::Error::from_raw_os_error(r));
        }
    }
    // The last byte stays NUL whatever the call wrote.
    buf[buf.len() - 1] = 0;
    // SAFETY: a NUL-terminated name in a buffer that outlives the borrow.
    Ok(unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) }.to_owned())
}

/// The shell to start when a pane names no command: `SHELL` when it is an
/// absolute path, else `/bin/sh`.
fn login_shell(var: Option<OsString>) -> PathBuf {
    var.map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| "/bin/sh".into())
}

/// The shell the user's account names, as Terminal starts it.
fn account_shell() -> Option<OsString> {
    use std::os::unix::ffi::OsStrExt;
    // SAFETY: getpwuid returns this user's entry in static storage or
    // null; its shell is copied out at once, on the one thread that asks.
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if pw.is_null() || (*pw).pw_shell.is_null() {
            return None;
        }
        let shell = std::ffi::CStr::from_ptr((*pw).pw_shell);
        Some(std::ffi::OsStr::from_bytes(shell.to_bytes()).to_owned())
    }
}

/// The shell a pane starts with no command: `SHELL`, or the one the
/// account names for an app started from the Finder or the Dock, which
/// has no SHELL.
pub fn user_shell() -> PathBuf {
    login_shell(std::env::var_os("SHELL").or_else(account_shell))
}

/// `cmdline` as `/bin/sh -c` gets it: a single command replaces the shell,
/// so the program is the pane's own process, gets Ctrl+C and the hangup
/// itself, and its exit code is the pane's. dash keeps itself around
/// otherwise, and dies of a Ctrl+C its program caught. A line that is more
/// than one command, or starts with a word `exec` would not run, stays as
/// it is.
fn sh_line(cmdline: &str) -> String {
    // Shell words, and builtins with no program of their name to run.
    const WORDS: &[&str] = &[
        "!", "{", "}", "[[", "case", "do", "done", "elif", "else", "esac", "fi", "for", "function",
        "if", "in", "select", "then", "time", "until", "while", "exec", ".", "alias", "cd", "eval",
        "exit", "export", "local", "read", "readonly", "return", "set", "shift", "source", "trap",
        "ulimit", "umask", "unset", "wait", ":", "command", "type", "hash", "jobs", "getopts",
        "break", "continue", "times", "fg", "bg", "builtin", "declare", "typeset", "let", "shopt",
        "pushd", "popd", "dirs",
    ];
    let line = cmdline.trim();
    let first = line.split([' ', '\t']).next().unwrap_or_default();
    let several = line.contains([';', '&', '|', '\n', '(', ')', '`']);
    if several || first.contains('=') || WORDS.contains(&first) {
        line.to_owned()
    } else {
        format!("exec {line}")
    }
}

/// `cmdline` run by `/bin/sh`, or with none, the login shell as a login
/// shell, as terminals on macOS start it, so its profile sets `PATH`.
fn command(cmdline: &str) -> Command {
    if cmdline.trim().is_empty() {
        let shell = user_shell();
        let name = shell.file_name().unwrap_or_default().to_string_lossy();
        let mut c = Command::new(&shell);
        c.arg0(format!("-{name}"));
        c
    } else {
        let mut c = Command::new("/bin/sh");
        c.arg("-c").arg(sh_line(cmdline));
        c
    }
}

/// The exit code a pane reports: the child's own, or 128 plus the signal
/// that ended it, as shells report it. Ctrl+C reads as it does on Windows,
/// so the pane closes as it does there.
fn code(status: ExitStatus) -> u32 {
    match (status.code(), status.signal()) {
        (Some(c), _) => c as u32,
        (None, Some(libc::SIGINT)) => crate::attention::CTRL_C_EXIT,
        (None, Some(sig)) => 128 + sig as u32,
        (None, None) => 1,
    }
}

/// Waits up to `within` for the child to exit, and its exit code.
fn wait(child: &Mutex<Child>, within: Duration) -> Option<u32> {
    let end = Instant::now() + within;
    loop {
        match lock(child).try_wait() {
            Ok(Some(status)) => return Some(code(status)),
            Ok(None) => {}
            Err(_) => return None,
        }
        if Instant::now() >= end {
            return None;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Sends `sig` to the child's process group, while the child has not been
/// waited for. The lock keeps it from being reaped meanwhile, so its id
/// cannot name another process yet.
fn signal(child: &Mutex<Child>, sig: libc::c_int) {
    let mut c = lock(child);
    if matches!(c.try_wait(), Ok(None)) {
        let Ok(pid) = libc::pid_t::try_from(c.id()) else {
            return;
        };
        // SAFETY: plain syscall; the child leads its own session, so its
        // process group has its id.
        unsafe { libc::kill(-pid, sig) };
    }
}

/// In the child before exec: a session of its own, with the terminal on
/// its standard input as its controlling one, so job control works and a
/// hangup reaches it.
fn take_terminal() -> io::Result<()> {
    // A signal blitz was started ignoring, as under nohup, is the
    // program's again: otherwise Ctrl+C and closing the pane do nothing.
    for sig in [
        libc::SIGHUP,
        libc::SIGINT,
        libc::SIGQUIT,
        libc::SIGTERM,
        libc::SIGCHLD,
        libc::SIGALRM,
        libc::SIGTSTP,
        libc::SIGTTIN,
        libc::SIGTTOU,
    ] {
        // SAFETY: async-signal-safe, with the default action.
        unsafe { libc::signal(sig, libc::SIG_DFL) };
    }
    // SAFETY: plain syscall.
    if unsafe { libc::setsid() } < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: plain syscall on the child's own standard input.
    if unsafe { libc::ioctl(0, libc::TIOCSCTTY as _, 0) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

impl Pty {
    /// Starts `opts.cmdline` on a new pseudo terminal. `on_event` runs on
    /// the pane's reader thread for every chunk of output and once at exit.
    pub fn spawn(
        opts: &SpawnOpts,
        mut on_event: impl FnMut(PtyEvent<'_>, &Writer) + Send + 'static,
    ) -> io::Result<Pty> {
        let (master, slave) = open(opts.cols, opts.rows)?;
        let mut cmd = command(opts.cmdline);
        let parent = with_path(std::env::vars_os(), crate::shell::pane_var("PATH"));
        cmd.env_clear()
            .envs(child_env(parent, opts.pane_id, opts.env))
            .stdin(Stdio::from(slave.try_clone()?))
            .stdout(Stdio::from(slave.try_clone()?))
            .stderr(Stdio::from(slave));
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let cwd = (opts.cwd.map(Path::to_path_buf).into_iter())
            .chain(home)
            .find(|d| d.is_dir())
            .unwrap_or_else(|| "/".into());
        cmd.current_dir(cwd);
        // SAFETY: `take_terminal` runs in the child between fork and exec,
        // and calls only async-signal-safe functions.
        unsafe { cmd.pre_exec(take_terminal) };
        let child = Arc::new(Mutex::new(cmd.spawn()?));
        // The parent's copies of the slave go with the command, so the
        // reader sees the end once the child's side closes.
        drop(cmd);
        // Without its threads nothing would ever close the pane, so a thread
        // that fails to start takes the child down with it.
        let fail = |e: io::Error| {
            signal(&child, libc::SIGKILL);
            e
        };
        let input = File::from(master.try_clone().map_err(fail)?);
        let writer = Writer::start(input).map_err(fail)?;
        let mut output = File::from(master.try_clone().map_err(fail)?);
        let (reply, child2) = (writer.clone(), child.clone());
        std::thread::Builder::new()
            .name("pty-read".into())
            .spawn(move || {
                let mut buf = vec![0; READ_BYTES];
                loop {
                    match output.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => on_event(PtyEvent::Data(&buf[..n]), &reply),
                        Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                        // Linux reports the closed slave as an error.
                        Err(_) => break,
                    }
                }
                // The end arrives just before the child's exit is visible;
                // give it a moment so the code is real.
                let code = wait(&child2, Duration::from_secs(1)).unwrap_or(STILL_ACTIVE);
                on_event(PtyEvent::Exit(code), &reply);
            })
            .map_err(fail)?;
        Ok(Pty {
            master,
            child,
            writer,
            closing: AtomicBool::new(false),
        })
    }

    pub fn writer(&self) -> &Writer {
        &self.writer
    }

    /// Resizes the terminal; the system tells the program with `SIGWINCH`.
    pub fn resize(&self, cols: u16, rows: u16) {
        if cols == 0 || rows == 0 {
            return;
        }
        let ws = winsize(cols, rows);
        // SAFETY: an open descriptor and a valid size, read only.
        unsafe { libc::ioctl(self.master.as_raw_fd(), libc::TIOCSWINSZ, &ws) };
    }

    /// Only ConPTY keeps a buffer of its own to clear.
    pub fn clear(&self) -> bool {
        false
    }

    /// Hangs up on the child and the terminal's foreground job (SIGHUP to
    /// their process groups) and kills what is still running 6 s later.
    /// Returns at once; output keeps flowing to the reader until the end.
    /// Also called on drop.
    pub fn close(&self) {
        if self.closing.swap(true, Ordering::Relaxed) {
            return;
        }
        let child = self.child.clone();
        let master = self.master.try_clone().ok();
        // A job the shell started runs in a group of its own, which a signal
        // to the child's misses: the terminal's foreground group gets it too.
        let foreground = move || {
            // SAFETY: a plain call on an open terminal.
            let group = master
                .as_ref()
                .map(|m| unsafe { libc::tcgetpgrp(m.as_raw_fd()) });
            group.filter(|&g| g > 0)
        };
        let hang_up = |group: Option<libc::pid_t>, sig| {
            if let Some(g) = group {
                // SAFETY: a signal to a group of this terminal's session.
                unsafe { libc::kill(-g, sig) };
            }
        };
        let _ = std::thread::Builder::new()
            .name("pty-close".into())
            .spawn(move || {
                // The group as it is now: once the shell dies of the hangup
                // the terminal has none, but a job that ignores it lives on
                // and keeps the pane open.
                let group = foreground();
                hang_up(group, libc::SIGHUP);
                signal(&child, libc::SIGHUP);
                let end = Instant::now() + CLOSE_GRACE;
                if wait(&child, CLOSE_GRACE).is_some() {
                    // The shell died of the hangup at once, as one does; its
                    // job gets the rest of the grace to clean up. While the
                    // group has members its id cannot name another's.
                    if let Some(g) = group {
                        // SAFETY: signal 0 only asks whether the group has
                        // members.
                        while unsafe { libc::kill(-g, 0) } == 0 {
                            if Instant::now() >= end {
                                hang_up(group, libc::SIGKILL);
                                break;
                            }
                            std::thread::sleep(Duration::from_millis(20));
                        }
                    }
                } else {
                    // The session still exists, so its group now is current:
                    // an id read seconds ago could name another's by now.
                    hang_up(foreground(), libc::SIGKILL);
                    signal(&child, libc::SIGKILL);
                    // Reaped, so it does not stay a zombie until blitz exits.
                    let _ = wait(&child, CLOSE_GRACE);
                }
            });
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        self.close();
    }
}

/// The environment for a pane's child: `parent` minus other terminals'
/// markers, plus blitz's own, plus `extra` (which wins).
pub fn child_env(
    parent: impl IntoIterator<Item = (OsString, OsString)>,
    pane_id: u32,
    extra: &[(String, String)],
) -> Vec<(OsString, OsString)> {
    let mut env: Vec<(OsString, OsString)> = (parent.into_iter())
        .filter(|(k, _)| !foreign(&k.to_string_lossy().to_ascii_uppercase()))
        .collect();
    // An app started from the Finder gets no locale, and programs then
    // print anything but ASCII as escapes.
    let locale = ["LC_ALL", "LC_CTYPE", "LANG"];
    if cfg!(target_os = "macos") && !env.iter().any(|(k, _)| locale.iter().any(|l| k == l)) {
        env.push(("LANG".into(), "en_US.UTF-8".into()));
    }
    let id = pane_id.to_string();
    let ours = [
        ("TERM", "xterm-256color"),
        ("TERM_PROGRAM", "blitz"),
        ("TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION")),
        ("COLORTERM", "truecolor"),
        ("FORCE_HYPERLINK", "1"),
        // Claude Code draws ~~text~~ struck through only in terminals it
        // knows, and blitz draws strikethrough.
        ("CLAUDE_CODE_FORCE_STRIKETHROUGH", "1"),
        ("BLITZ_PANE_ID", id.as_str()),
    ];
    let sets = ours
        .into_iter()
        .chain(extra.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    for (k, v) in sets {
        env.retain(|(e, _)| e != k);
        env.push((k.into(), v.into()));
    }
    env
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;

    enum Got {
        Data(Vec<u8>),
        Exit(u32),
    }

    fn spawn(opts: &SpawnOpts) -> (Pty, mpsc::Receiver<Got>) {
        let (tx, rx) = mpsc::channel();
        let pty = Pty::spawn(opts, move |ev, _| {
            let _ = tx.send(match ev {
                PtyEvent::Data(b) => Got::Data(b.to_vec()),
                PtyEvent::Exit(c) => Got::Exit(c),
            });
        })
        .expect("spawn");
        (pty, rx)
    }

    /// Everything the child prints, and its exit code.
    fn finish(rx: &mpsc::Receiver<Got>) -> (String, u32) {
        let mut out = Vec::new();
        loop {
            match rx
                .recv_timeout(Duration::from_secs(20))
                .expect("the child exits")
            {
                Got::Data(b) => out.extend(b),
                Got::Exit(c) => return (String::from_utf8_lossy(&out).into_owned(), c),
            }
        }
    }

    fn run(cmdline: &str) -> (String, u32) {
        let (_pty, rx) = spawn(&SpawnOpts {
            cmdline,
            cols: 80,
            rows: 24,
            ..Default::default()
        });
        finish(&rx)
    }

    #[test]
    fn output_and_exit_code_arrive() {
        let (out, code) = run("printf hello; exit 3");
        assert!(out.contains("hello"), "{out:?}");
        assert_eq!(code, 3);
    }

    #[test]
    fn a_signal_reads_as_a_shell_reports_it() {
        assert_eq!(run("kill -TERM $$").1, 128 + libc::SIGTERM as u32);
    }

    #[test]
    fn the_child_has_a_terminal_of_the_pane_size() {
        let (_pty, rx) = spawn(&SpawnOpts {
            cmdline: "test -t 0 && stty size",
            cols: 91,
            rows: 27,
            ..Default::default()
        });
        let (out, code) = finish(&rx);
        assert_eq!((out.trim(), code), ("27 91", 0));
    }

    #[test]
    fn a_resize_reaches_the_child() {
        let (pty, rx) = spawn(&SpawnOpts {
            cmdline: "read x; stty size",
            cols: 80,
            rows: 24,
            ..Default::default()
        });
        pty.resize(100, 40);
        pty.writer().send(b"\r".to_vec());
        let (out, _) = finish(&rx);
        assert!(out.ends_with("40 100\r\n"), "{out:?}");
    }

    #[test]
    fn input_reaches_the_child() {
        let (pty, rx) = spawn(&SpawnOpts {
            cmdline: "read x; printf '<%s>' \"$x\"",
            cols: 80,
            rows: 24,
            ..Default::default()
        });
        pty.writer().send(b"typed\r".to_vec());
        let (out, _) = finish(&rx);
        assert!(out.contains("<typed>"), "{out:?}");
    }

    #[test]
    fn the_child_starts_in_its_folder_with_blitz_variables() {
        let dir = std::env::temp_dir().canonicalize().unwrap();
        let extra = [("BLITZ_PANE_TOKEN".to_string(), "t0k".to_string())];
        let (_pty, rx) = spawn(&SpawnOpts {
            cmdline: "printf '%s|%s|%s|%s' \"$(pwd -P)\" \"$BLITZ_PANE_ID\" \"$TERM\" \"$BLITZ_PANE_TOKEN\"",
            cwd: Some(&dir),
            env: &extra,
            cols: 80,
            rows: 24,
            pane_id: 7,
            ..Default::default()
        });
        let (out, _) = finish(&rx);
        assert_eq!(out, format!("{}|7|xterm-256color|t0k", dir.display()));
    }

    #[test]
    fn closing_hangs_up_on_the_child() {
        let (pty, rx) = spawn(&SpawnOpts {
            cmdline: "exec sleep 100",
            cols: 80,
            rows: 24,
            ..Default::default()
        });
        let started = Instant::now();
        pty.close();
        let (_, code) = finish(&rx);
        assert_eq!(code, 128 + libc::SIGHUP as u32);
        assert!(started.elapsed() < CLOSE_GRACE);
    }

    #[test]
    fn other_terminals_variables_stay_out() {
        let parent = [
            ("HOME", "/home/me"),
            ("TMUX", "/tmp/tmux"),
            ("TERM", "screen"),
            ("BLITZ_TRACE", "/tmp/t.json"),
            ("KITTY_WINDOW_ID", "1"),
            ("EDITOR", "vi"),
        ]
        .map(|(k, v)| (OsString::from(k), OsString::from(v)));
        let extra = [("EDITOR".to_string(), "hx".to_string())];
        let env = child_env(parent, 3, &extra);
        let get = |k: &str| {
            env.iter()
                .find(|(e, _)| e == k)
                .map(|(_, v)| v.to_str().unwrap())
        };
        assert_eq!(get("HOME"), Some("/home/me"));
        assert_eq!(get("TERM"), Some("xterm-256color"));
        assert_eq!(get("BLITZ_PANE_ID"), Some("3"));
        assert_eq!(get("EDITOR"), Some("hx"));
        for gone in ["TMUX", "BLITZ_TRACE", "KITTY_WINDOW_ID"] {
            assert_eq!(get(gone), None, "{gone}");
        }
        assert_eq!(env.iter().filter(|(k, _)| k == "EDITOR").count(), 1);
    }

    #[test]
    fn a_single_command_replaces_sh() {
        assert_eq!(sh_line(" python3 -q "), "exec python3 -q");
        assert_eq!(sh_line("/bin/zsh -l"), "exec /bin/zsh -l");
        for kept in [
            "a; b",
            "a && b",
            "a | b",
            "(a)",
            "FOO=1 a",
            "exit 5",
            "if true; then a; fi",
            "exec sleep 1",
            "read x",
            ":",
            "command -v sh",
        ] {
            assert_eq!(sh_line(kept), kept);
        }
    }

    #[test]
    fn the_program_gets_ctrl_c_and_its_exit_code_is_the_panes() {
        // sh's trap stands in for a program that catches Ctrl+C, as Python
        // does; the line is one command, so it replaces the outer sh.
        let script = "trap 'exit 5' INT; echo ready; while :; do sleep 0.05; done";
        let env = [("S".to_string(), script.to_string())];
        let (pty, rx) = spawn(&SpawnOpts {
            cmdline: "sh -c \"$S\"",
            env: &env,
            cols: 80,
            rows: 24,
            ..Default::default()
        });
        wait_for(&rx, "ready");
        pty.writer().send(b"\x03".to_vec());
        let (_, code) = finish(&rx);
        assert_eq!(code, 5);
    }

    #[test]
    fn closing_gives_a_job_its_grace_after_the_shell_ends() {
        let dir = std::env::temp_dir().join(format!("blitz-grace-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let marker = dir.join("cleaned");
        // The job cleans up for a second when hung up on.
        let script = "sh -c \"trap 'sleep 1; touch \\\"$M\\\"; exit 0' HUP; echo ready; \
                      while :; do sleep 0.1; done\"; :";
        let env = [
            ("S".to_string(), script.to_string()),
            ("M".to_string(), marker.display().to_string()),
        ];
        let (pty, rx) = spawn(&SpawnOpts {
            cmdline: "/bin/sh -i -c \"$S\"",
            env: &env,
            cols: 80,
            rows: 24,
            ..Default::default()
        });
        wait_for(&rx, "ready");
        pty.close();
        let _ = finish(&rx);
        // A Mac ends the pane when the shell ends, as the terminal goes
        // with its session, so the job may still be cleaning up.
        let started = Instant::now();
        while !marker.exists() && started.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(50));
        }
        let cleaned = marker.exists();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(cleaned, "the job was killed before it cleaned up");
    }

    #[test]
    fn closing_ends_a_job_that_outlives_its_shell() {
        // The shell dies of the hangup; its job ignores it.
        let script = "sh -c \"trap '' HUP; echo ready; exec sleep 123\"; :";
        let env = [("S".to_string(), script.to_string())];
        let (pty, rx) = spawn(&SpawnOpts {
            cmdline: "/bin/sh -i -c \"$S\"",
            env: &env,
            cols: 80,
            rows: 24,
            ..Default::default()
        });
        wait_for(&rx, "ready");
        pty.close();
        // Ends at all: the job no longer holds the terminal open.
        let _ = finish(&rx);
    }

    #[test]
    fn closing_ends_a_job_in_a_group_of_its_own() {
        // An interactive shell runs its job in a group of its own, and
        // neither hears the hangup. The job says it is ready itself, so
        // its group exists by then.
        let script = "trap '' HUP; sh -c 'echo ready; exec sleep 123'";
        let env = [("S".to_string(), script.to_string())];
        let (pty, rx) = spawn(&SpawnOpts {
            cmdline: "/bin/sh -i -c \"$S\"",
            env: &env,
            cols: 80,
            rows: 24,
            ..Default::default()
        });
        wait_for(&rx, "ready");
        pty.close();
        let (_, code) = finish(&rx);
        assert_eq!(code, 128 + libc::SIGKILL as u32);
    }

    /// Reads output until it shows `what`.
    fn wait_for(rx: &mpsc::Receiver<Got>, what: &str) {
        let mut out = Vec::new();
        while !String::from_utf8_lossy(&out).contains(what) {
            match rx.recv_timeout(Duration::from_secs(20)).expect("output") {
                Got::Data(b) => out.extend(b),
                Got::Exit(c) => panic!("exited {c}: {}", String::from_utf8_lossy(&out)),
            }
        }
    }

    #[test]
    fn the_login_shell_is_an_absolute_shell_or_sh() {
        assert_eq!(login_shell(Some("/bin/zsh".into())), Path::new("/bin/zsh"));
        assert_eq!(login_shell(Some("zsh".into())), Path::new("/bin/sh"));
        assert_eq!(login_shell(None), Path::new("/bin/sh"));
    }

    #[test]
    fn tokens_are_new_each_time() {
        let (a, b) = (pane_token().unwrap(), pane_token().unwrap());
        assert_eq!(a.len(), 32);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }
}
