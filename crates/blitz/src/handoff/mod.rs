//! Launching blitz while it runs: a second `blitz` brings the main window
//! to the front and exits, and a second `blitz --cwd X` also sends X, and
//! that window opens a tab there.

use std::path::PathBuf;

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod win;

#[cfg(unix)]
pub use unix::*;
#[cfg(windows)]
pub use win::*;

/// What another launch asks of the main window.
#[derive(Debug, PartialEq, Eq)]
pub enum Ask {
    /// Come to the front.
    Show,
    /// Come to the front and open a tab in this folder.
    Open(PathBuf),
}
