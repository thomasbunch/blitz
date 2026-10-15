//! A child process on its own terminal: ConPTY on Windows, a pseudo
//! terminal elsewhere. Both give a pane the same [`Pty`].

use std::io::{self, Write};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, mpsc};

#[cfg(windows)]
mod conpty;
#[cfg(unix)]
mod unix;

#[cfg(windows)]
pub use conpty::*;
#[cfg(unix)]
pub use unix::*;

/// Bytes queued for the child's input beyond which replies are dropped.
const MAX_PENDING: usize = 1 << 20;

/// What the reader thread reports.
pub enum PtyEvent<'a> {
    /// Output from the child, in order. Replies to queries go through the
    /// [`Writer`] passed alongside, so they stay in stream order.
    Data(&'a [u8]),
    /// The terminal and every process attached to it are gone. Carries the
    /// shell's exit code (`STILL_ACTIVE`, 259, if it outlived a forced
    /// close).
    Exit(u32),
}

/// Sends bytes to the child's input. Cheap to clone; writes happen on the
/// pane's writer thread, in the order they are sent.
#[derive(Clone)]
pub struct Writer {
    tx: mpsc::Sender<Vec<u8>>,
    /// Bytes sent but not yet written.
    pending: Arc<AtomicUsize>,
}

impl Writer {
    /// Starts the writer thread on the child's input.
    fn start(mut input: impl Write + Send + 'static) -> io::Result<Writer> {
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        let writer = Writer {
            tx,
            pending: Arc::default(),
        };
        let pending = writer.pending.clone();
        std::thread::Builder::new()
            .name("pty-write".into())
            .spawn(move || {
                for bytes in rx {
                    if input.write_all(&bytes).is_err() {
                        break;
                    }
                    pending.fetch_sub(bytes.len(), Ordering::Relaxed);
                }
            })?;
        Ok(writer)
    }

    pub fn send(&self, bytes: impl Into<Vec<u8>>) {
        let bytes = bytes.into();
        self.pending.fetch_add(bytes.len(), Ordering::Relaxed);
        // An error only means the child has gone.
        let _ = self.tx.send(bytes);
    }

    /// [`Writer::send`] for answers to the child's queries. They are dropped
    /// while its input is backed up, so a program that floods queries
    /// cannot make blitz queue replies without limit.
    pub fn reply(&self, bytes: impl Into<Vec<u8>>) {
        if self.pending.load(Ordering::Relaxed) < MAX_PENDING {
            self.send(bytes);
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct SpawnOpts<'a> {
    /// The full command line, e.g. from [`crate::shell::launch`]. Outside
    /// Windows an empty one starts the user's login shell.
    pub cmdline: &'a str,
    pub cwd: Option<&'a Path>,
    /// Added to the child's environment after blitz's own variables.
    pub env: &'a [(String, String)],
    pub cols: u16,
    pub rows: u16,
    /// Exported to the child as `BLITZ_PANE_ID`.
    pub pane_id: u32,
    /// Window that owns the pseudoconsole's hidden console window. Windows
    /// only.
    pub parent: Option<isize>,
}

/// Variables that describe the terminal the parent runs in. A child that
/// sees them would believe it runs in that terminal instead of blitz.
const STRIP_PREFIXES: &[&str] = &[
    "TERM_PROGRAM",
    "KITTY_",
    "GHOSTTY_",
    "WEZTERM_",
    "ITERM_",
    "LC_TERMINAL",
    "CONEMU",
    "VSCODE_",
    "ALACRITTY_",
    // blitz's own settings, such as BLITZ_CONPTY_DIR and BLITZ_TRACE, are
    // for the blitz they were set for, not for programs its panes start.
    "BLITZ_",
];

const STRIP: &[&str] = &[
    // TERM changes how Git for Windows tools draw; Windows terminals leave
    // it unset. Elsewhere a pane sets its own.
    "TERM",
    "WT_SESSION",
    "WT_PROFILE_ID",
    "TERMINAL_EMULATOR",
    "VTE_VERSION",
    "ZED_TERM",
    "CURSOR_TRACE_ID",
    "TILIX_ID",
    "KONSOLE_VERSION",
    "GNOME_TERMINAL_SERVICE",
    "TERMINATOR_UUID",
    "XTERM_VERSION",
    "TMUX",
    "TMUX_PANE",
    "ZELLIJ",
    "STY",
    "COLORTERM",
    // Set when blitz itself was started from a Claude Code session. Each
    // pane starts a fresh session, and the messaging token is a secret.
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_REMOTE_SESSION_ID",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_PID",
    "CLAUDE_EFFORT",
    "CLAUDE_PLUGIN_DATA",
    "CLAUDE_PROJECT_DIR",
    "CLAUDE_ENV_FILE",
];

/// Whether a variable named `upper`, in capitals, is another terminal's or
/// blitz's own, and so stays out of a pane's environment.
fn foreign(upper: &str) -> bool {
    STRIP.contains(&upper) || STRIP_PREFIXES.iter().any(|p| upper.starts_with(p))
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}
