//! The terminal: screens, cursor, modes, replies and events.

use std::time::Instant;

use crate::modes::InputModes;
use crate::snapshot::{Palette, Snapshot};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    pub cols: u16,
    pub rows: u16,
    pub scrollback_lines: usize,
    /// Treat East Asian Ambiguous characters as wide.
    pub ambiguous_wide: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            cols: 80,
            rows: 24,
            scrollback_lines: 10_000,
            ambiguous_wide: false,
        }
    }
}

/// OSC 133 shell-integration marks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptMark {
    /// Prompt start. `blitz` is set when the mark carries `blitz=1`.
    A { blitz: bool },
    /// Command input start.
    B,
    /// Command output start.
    C,
    /// Command finished, with its exit code when given.
    D(Option<i32>),
}

/// Things the host should know about, queued in stream order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Title(String),
    Bell,
    Cwd(String),
    Notify { title: String, body: String },
    Progress { state: u8, pct: Option<u8> },
    Prompt(PromptMark),
    Hyperlink,
}

pub struct Terminal {
    opts: Options,
}

impl Terminal {
    pub fn new(o: Options) -> Self {
        Self { opts: o }
    }

    /// Parses `bytes`, queueing replies and events.
    pub fn feed(&mut self, _bytes: &[u8]) {}

    /// Moves queued replies (DA1, CPR, OSC answers, ...) to `out`, in order.
    pub fn take_replies(&mut self, _out: &mut Vec<u8>) {}

    pub fn take_events(&mut self, _out: &mut Vec<Event>) {}

    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.opts.cols = cols;
        self.opts.rows = rows;
    }

    /// Scrolls the view of the main screen; positive is up into scrollback.
    pub fn scroll_viewport(&mut self, _delta: isize) {}

    pub fn input_modes(&self) -> InputModes {
        InputModes::default()
    }

    /// Column, row and visibility.
    pub fn cursor(&self) -> (u16, u16, bool) {
        (0, 0, true)
    }

    /// True while a synchronized update (mode 2026) is open and recent.
    pub fn sync_pending(&self, _now: Instant) -> bool {
        false
    }

    /// Fills `out` with the visible screen. Returns whether anything changed
    /// since the last call.
    pub fn snapshot(&mut self, out: &mut Snapshot, _pal: &Palette) -> bool {
        out.cols = self.opts.cols;
        out.rows = self.opts.rows;
        false
    }

    /// Resets keyboard, mouse and paste modes a crashed program may have left
    /// behind.
    pub fn on_child_exit(&mut self) {}

    pub fn set_theme(&mut self, _dark: bool) {}

    /// Cell size in pixels, for size reports.
    pub fn set_cell_px(&mut self, _w: u16, _h: u16) {}

    /// The screen as text: rows joined by `\n`, trailing spaces trimmed.
    pub fn screen_text(&self) -> String {
        String::new()
    }

    pub fn scrollback_text(&self) -> String {
        String::new()
    }
}
