//! A session: one child process on its own pseudoconsole, and its screen.

use std::io::{self, Read};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Component, Path, Prefix};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use crate::attention::{Command, PaneAttn};
use crate::layout::PaneId;
use crate::pty::{Pty, PtyEvent, SpawnOpts};

// A panic in vt must end one pane, not every session in the window, so the
// reader catches it below. Under panic = "abort" nothing could.
#[cfg(not(panic = "unwind"))]
compile_error!("blitz must be built with panic = \"unwind\"");

/// The most output parsed per hold of the terminal lock, so the UI thread
/// never waits long for a snapshot.
const FEED_BYTES: usize = 64 * 1024;

pub struct Pane {
    pub id: PaneId,
    pub term: Arc<Mutex<vt::Terminal>>,
    pub pty: Pty,
    /// Attention state and when it last changed.
    pub attn: PaneAttn,
    /// The command its shell is running, from blitz's prompt marks.
    pub cmd: Command,
    /// Shown in the sidebar and the pane header.
    pub name: String,
    /// The name the user gave the session, which wins over any other.
    pub named: Option<String>,
    /// Latest title set by the program (OSC 0/2).
    pub title: String,
    /// What Claude Code's mark in `title` says: true while it works, false
    /// once it stopped, `None` with no such mark. See
    /// [`crate::attention::claude_title`].
    pub claude_title: Option<bool>,
    /// Latest directory reported by the shell, else the spawn directory.
    pub cwd: String,
    /// Git branch of `cwd`, read from `.git/HEAD`.
    pub branch: Option<String>,
    /// Latest one-line message from a hook notification.
    pub msg: String,
    /// The pane's `BLITZ_PANE_TOKEN`. Only hook notifications and prompt
    /// marks that carry it are believed.
    pub token: String,
    /// The Claude Code session running in the pane, from its hook
    /// notifications. Kept until Claude ends the session, so a window closed
    /// mid-session can resume it.
    pub claude: Option<String>,
    /// Claude Code's hooks report for the pane, from the first one with
    /// its token until the session ends. Bells and other notifications
    /// would only say the same again.
    pub hooked: bool,
    /// Set once the child has exited.
    pub exit_code: Option<u32>,
    /// Set by the reader thread when there is new output to draw.
    pub dirty: Arc<AtomicBool>,
}

/// What a pane's reader thread tells the UI thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Note {
    /// New output was parsed. Sent once until the UI clears `dirty`.
    Dirty,
    /// The child exited with this code.
    Exit(u32),
    /// Parsing panicked; the pane no longer updates.
    Dead,
}

/// Locks a mutex even if a thread panicked while holding it: a half
/// updated screen is better than taking down every session.
pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What a new pane runs.
pub struct Spawn<'a> {
    pub cmdline: &'a str,
    pub env: &'a [(String, String)],
    pub cwd: Option<&'a Path>,
    pub cols: u16,
    pub rows: u16,
    pub scrollback: usize,
    pub dark: bool,
    pub pal: vt::Palette,
    /// The window, which becomes the owner of the console's hidden window.
    pub parent: Option<isize>,
    /// From [`crate::pty::pane_token`]; exported as `BLITZ_PANE_TOKEN`.
    pub token: &'a str,
    /// Fed to the screen before the child starts: [`restored`] output.
    pub restored: &'a [u8],
}

impl Pane {
    /// Starts the child. `notify` runs on the reader thread.
    pub fn spawn(
        id: PaneId,
        s: &Spawn,
        notify: impl Fn(PaneId, Note) + Send + 'static,
    ) -> io::Result<Pane> {
        let mut term = vt::Terminal::new(vt::Options {
            cols: s.cols,
            rows: s.rows,
            scrollback_lines: s.scrollback,
            ..Default::default()
        });
        term.set_theme(s.dark, &s.pal);
        term.set_prompt_token(s.token);
        term.feed(s.restored);
        let term = Arc::new(Mutex::new(term));
        let dirty = Arc::new(AtomicBool::new(false));
        let (t, d) = (term.clone(), dirty.clone());
        let mut dead = false;
        let mut replies = Vec::new();
        let on_event = move |ev: PtyEvent<'_>, w: &crate::pty::Writer| {
            if dead {
                return;
            }
            let r = catch_unwind(AssertUnwindSafe(|| match ev {
                PtyEvent::Data(bytes) => {
                    for chunk in bytes.chunks(FEED_BYTES) {
                        let mut term = lock(&t);
                        term.feed(chunk);
                        term.take_replies(&mut replies);
                        drop(term);
                        // Replies go out in the order the queries came in.
                        if !replies.is_empty() {
                            w.reply(std::mem::take(&mut replies));
                        }
                    }
                    if !d.swap(true, Ordering::AcqRel) {
                        notify(id, Note::Dirty);
                    }
                }
                PtyEvent::Exit(code) => {
                    lock(&t).on_child_exit();
                    notify(id, Note::Exit(code));
                }
            }));
            if r.is_err() {
                dead = true;
                // The panic may have left the screen half updated, and the UI
                // thread reads it every frame. A blank one takes its place,
                // 1x1 until the next resize so it never draws past the pane.
                *lock(&t) = vt::Terminal::new(vt::Options {
                    cols: 1,
                    rows: 1,
                    ..Default::default()
                });
                notify(id, Note::Dead);
            }
        };
        let mut env = s.env.to_vec();
        env.push(("BLITZ_PANE_TOKEN".into(), s.token.into()));
        let pty = Pty::spawn(
            &SpawnOpts {
                cmdline: s.cmdline,
                cwd: s.cwd,
                env: &env,
                cols: s.cols,
                rows: s.rows,
                pane_id: id.0,
                parent: s.parent,
            },
            on_event,
        )?;
        Ok(Pane {
            id,
            term,
            pty,
            attn: PaneAttn::new(Instant::now()),
            cmd: Command::default(),
            name: String::new(),
            named: None,
            title: String::new(),
            claude_title: None,
            cwd: s.cwd.map(|p| p.display().to_string()).unwrap_or_default(),
            branch: None,
            msg: String::new(),
            token: s.token.into(),
            claude: None,
            hooked: false,
            exit_code: None,
            dirty,
        })
    }

    /// Resizes the screen, then the pseudoconsole, so a cursor report the
    /// program asks for right after the resize already uses the new size.
    /// `marks` move with their text; see [`vt::Terminal::resize_keeping`].
    pub fn resize(&self, cols: u16, rows: u16, marks: &mut [vt::terminal::LineCol]) -> bool {
        if cols == 0 || rows == 0 {
            return false;
        }
        let kept = lock(&self.term).resize_keeping(cols, rows, marks);
        self.pty.resize(cols, rows);
        kept
    }

    pub fn send(&self, bytes: impl Into<Vec<u8>>) {
        let bytes = bytes.into();
        if !bytes.is_empty() {
            self.pty.writer().send(bytes);
        }
    }
}

/// What a new screen is fed before its shell starts, to show `text`, a
/// pane's saved output, above a dim `restored · <stamp>` line. All of it
/// goes into scrollback, leaving a blank screen with the cursor at the
/// top: the console host in Windows clears the screen when it starts, and
/// the bundled one writes over it as if it were blank. Control characters
/// are dropped, since a query in the text would be answered to the new
/// shell.
pub fn restored(text: &str, stamp: &str, rows: u16) -> Vec<u8> {
    let mut s = String::with_capacity(text.len() + 64 + usize::from(rows));
    for line in text.lines() {
        s.extend(line.chars().filter(|c| !c.is_control()));
        s.push_str("\r\n");
    }
    s.push_str(&format!("\x1b[2m── restored · {stamp} ──\x1b[m\r\n"));
    s.push_str(&"\n".repeat(rows.into()));
    s.push_str("\x1b[H");
    s.into_bytes()
}

/// The program a command line runs, without directory or extension:
/// `pwsh` for `"C:\Program Files\PowerShell\7\pwsh.exe" -NoLogo`.
pub fn program_name(cmdline: &str) -> String {
    Path::new(crate::shell::split_program(cmdline).0)
        .file_stem()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
}

/// The branch checked out in the repository that holds `dir`, read from
/// `.git/HEAD` without running git. A detached HEAD gives the short hash.
/// Blocks on the drive, so the app calls it off the UI thread.
pub fn git_branch(dir: &Path) -> Option<String> {
    for d in dir.ancestors() {
        let git = d.join(".git");
        let head = if git.is_dir() {
            git.join("HEAD")
        } else if git.is_file() {
            // A worktree or submodule: the file names the real git dir.
            // A folder from a download can name a share there, and just
            // reading it makes Windows sign in to that host.
            let link = read_start(&git)?;
            let gitdir = d.join(link.strip_prefix("gitdir:")?.trim());
            match gitdir.components().next() {
                Some(Component::Prefix(p)) if matches!(p.kind(), Prefix::Disk(_)) => {}
                _ => return None,
            }
            gitdir.join("HEAD")
        } else {
            continue;
        };
        let head = read_start(&head)?;
        let head = head.trim();
        return match head.strip_prefix("ref: refs/heads/") {
            Some(branch) => Some(branch.to_string()),
            None => head.get(..7).map(str::to_string),
        };
    }
    None
}

/// The first 4 KiB of a file that should hold one short line.
fn read_start(path: &Path) -> Option<String> {
    let mut s = String::new();
    let file = std::fs::File::open(path).ok()?;
    file.take(4096).read_to_string(&mut s).ok()?;
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn pane_program_names() {
        let pwsh = r#""C:\Program Files\PowerShell\7\pwsh.exe" -NoLogo -NoExit"#;
        assert_eq!(program_name(pwsh), "pwsh");
        assert_eq!(program_name(r"C:\Windows\system32\cmd.exe /d"), "cmd");
        assert_eq!(program_name("claude"), "claude");
        assert_eq!(program_name(""), "");
    }

    #[test]
    fn pane_restored_text_goes_into_scrollback() {
        for (text, last) in [("one\ntwo\n", "two"), (&"line\n".repeat(50), "line")] {
            let mut term = vt::Terminal::new(vt::Options {
                cols: 40,
                rows: 5,
                scrollback_lines: 100,
                ..Default::default()
            });
            term.feed(&restored(text, "14:32", 5));
            let sb = term.scrollback_text();
            assert!(sb.starts_with(text.lines().next().unwrap()), "{sb:?}");
            assert!(
                sb.contains(&format!("{last}\n── restored · 14:32 ──")),
                "{sb:?}"
            );
            assert_eq!(term.screen_text().trim(), "");
            assert_eq!(term.cursor(), (0, 0, true));
        }
    }

    #[test]
    fn pane_restored_text_drops_control_characters() {
        let mut term = vt::Terminal::new(vt::Options::default());
        term.feed(&restored("a\x1b[cb\x07\x1b]0;t\x07\r\n", "14:32", 24));
        let mut replies = Vec::new();
        term.take_replies(&mut replies);
        assert!(replies.is_empty(), "{replies:?}");
        assert!(term.scrollback_text().starts_with("a[cb]0;t\n"));
    }

    #[test]
    fn pane_git_branch_from_head() {
        let root = std::env::temp_dir().join(format!("blitz-branch-{}", std::process::id()));
        let sub = root.join("repo").join("src").join("deep");
        std::fs::create_dir_all(&sub).expect("dirs");
        std::fs::create_dir_all(root.join("repo").join(".git")).expect("git dir");
        let head = root.join("repo").join(".git").join("HEAD");
        std::fs::write(&head, "ref: refs/heads/feature/x\n").expect("head");
        assert_eq!(git_branch(&sub).as_deref(), Some("feature/x"));
        std::fs::write(&head, "0123456789abcdef\n").expect("head");
        assert_eq!(git_branch(&sub).as_deref(), Some("0123456"));

        // A worktree's `.git` is a file pointing at its git dir.
        let wt = root.join("wt");
        std::fs::create_dir_all(root.join("meta")).expect("meta");
        std::fs::create_dir_all(&wt).expect("wt");
        std::fs::write(
            wt.join(".git"),
            format!("gitdir: {}\n", root.join("meta").display()),
        )
        .expect("link");
        std::fs::write(root.join("meta").join("HEAD"), "ref: refs/heads/side\n").expect("head");
        assert_eq!(git_branch(&wt).as_deref(), Some("side"));

        // Only a git dir on a drive is read, not a share or device path.
        std::fs::write(
            wt.join(".git"),
            format!("gitdir: \\\\?\\{}\n", root.join("meta").display()),
        )
        .expect("link");
        assert_eq!(git_branch(&wt), None);

        // A huge HEAD is not read whole.
        std::fs::write(&head, format!("ref: refs/heads/{}", "x".repeat(10_000))).expect("head");
        assert!(git_branch(&sub).is_some_and(|b| b.len() < 4096));

        // Gone with its repository. A folder above the temp folder may be in
        // a repository of its own, so that is what is found now.
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(git_branch(&sub), git_branch(&std::env::temp_dir()));
        assert_ne!(git_branch(&sub).as_deref(), Some("feature/x"));
    }

    /// A pane parses its child's output on the reader thread, answers
    /// queries, and reports the exit.
    #[test]
    fn pane_runs_a_command_and_reports_exit() {
        let (tx, rx) = mpsc::channel();
        let pane = Pane::spawn(
            PaneId(7),
            &Spawn {
                cmdline: "cmd.exe /d /c echo pane-%BLITZ_PANE_ID%",
                env: &[],
                cwd: None,
                cols: 40,
                rows: 5,
                scrollback: 100,
                dark: true,
                pal: crate::theme::dark(),
                parent: None,
                token: "t",
                restored: &[],
            },
            move |id, n| {
                let _ = tx.send((id, n));
            },
        )
        .expect("spawn");
        let mut exit = None;
        while let Ok((id, n)) = rx.recv_timeout(Duration::from_secs(20)) {
            assert_eq!(id, PaneId(7));
            if n == Note::Dirty {
                pane.dirty.store(false, Ordering::Release);
            }
            if let Note::Exit(code) = n {
                exit = Some(code);
                break;
            }
        }
        assert_eq!(exit, Some(0));
        let text = lock(&pane.term).screen_text();
        assert!(text.contains("pane-7"), "screen: {text:?}");
    }

    /// After a panic on the reader thread the UI never sees the screen it
    /// was updating.
    #[test]
    fn pane_drops_its_screen_after_a_panic() {
        let (tx, rx) = mpsc::channel();
        let pane = Pane::spawn(
            PaneId(8),
            &Spawn {
                cmdline: "cmd.exe /d /c echo pane-output",
                env: &[],
                cwd: None,
                cols: 40,
                rows: 5,
                scrollback: 100,
                dark: true,
                pal: crate::theme::dark(),
                parent: None,
                token: "t",
                restored: &[],
            },
            move |_, n| {
                // Stands in for a parser panic, once all output is on screen.
                assert!(!matches!(n, Note::Exit(_)), "test panic at exit");
                let _ = tx.send(n);
            },
        )
        .expect("spawn");
        let dead = std::iter::from_fn(|| rx.recv_timeout(Duration::from_secs(20)).ok())
            .any(|n| n == Note::Dead);
        assert!(dead);
        let text = lock(&pane.term).screen_text();
        assert!(!text.contains("pane-output"), "screen: {text:?}");
    }
}
