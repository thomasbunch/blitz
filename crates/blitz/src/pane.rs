//! A session: one child process on its own pseudoconsole, and its screen.

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use crate::attention::PaneAttn;
use crate::layout::PaneId;
use crate::pty::Pty;

pub struct Pane {
    pub id: PaneId,
    pub term: Arc<Mutex<vt::Terminal>>,
    pub pty: Pty,
    /// Attention state and when it last changed.
    pub attn: PaneAttn,
    /// Shown in the sidebar and the pane header.
    pub name: String,
    /// Latest title set by the program (OSC 0/2).
    pub title: String,
    /// Latest directory reported by the shell, else the spawn directory.
    pub cwd: String,
    /// Git branch of `cwd`, read from `.git/HEAD`.
    pub branch: Option<String>,
    /// Latest one-line message from a hook notification.
    pub msg: String,
    /// Set once the child has exited.
    pub exit_code: Option<u32>,
    /// Set by the reader thread when there is new output to draw.
    pub dirty: Arc<AtomicBool>,
}
