//! Terminal modes and the subset the input encoders need.

use std::time::{Duration, Instant};

/// Which mouse events the application asked for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MouseMode {
    #[default]
    Off,
    /// Mode 1000: presses and releases.
    Click,
    /// Mode 1002: also motion while a button is held.
    Drag,
    /// Mode 1003: all motion.
    Any,
}

/// Modes that change how keys, mouse, paste and focus are encoded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InputModes {
    /// DECCKM: cursor keys send SS3.
    pub decckm: bool,
    /// DECKPAM: application keypad.
    pub deckpam: bool,
    /// Mode 9001: win32-input-mode.
    pub w32im: bool,
    /// Kitty keyboard flags at the top of the active screen's stack.
    pub kitty: u8,
    /// Mode 2004.
    pub bracketed: bool,
    /// Mode 1004.
    pub focus: bool,
    pub mouse: MouseMode,
    /// Mode 1006.
    pub mouse_sgr: bool,
    pub alt_screen: bool,
}

/// How long a synchronized update (mode 2026) may hold back the screen.
/// A frame takes a few milliseconds; a program that never ends its update
/// should cost a short stall, not a frozen pane.
pub const SYNC_TIMEOUT: Duration = Duration::from_millis(150);

/// Entries kept per kitty keyboard stack; a push beyond this drops the
/// oldest, as kitty does.
const KITTY_DEPTH: usize = 8;

/// The five kitty keyboard enhancement flags.
const KITTY_FLAGS: u8 = 0b1_1111;

/// One screen's kitty keyboard flags stack.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KittyStack(Vec<u8>);

impl KittyStack {
    /// The flags in effect: the top entry, or 0 when empty.
    pub fn flags(&self) -> u8 {
        self.0.last().copied().unwrap_or(0)
    }

    /// Entries from the bottom of the stack up.
    pub fn entries(&self) -> &[u8] {
        &self.0
    }

    /// `CSI > flags u`.
    pub fn push(&mut self, flags: u8) {
        if self.0.len() == KITTY_DEPTH {
            self.0.remove(0);
        }
        self.0.push(flags & KITTY_FLAGS);
    }

    /// `CSI < n u`. Popping more entries than there are empties the stack.
    pub fn pop(&mut self, n: usize) {
        self.0.truncate(self.0.len().saturating_sub(n));
    }

    /// `CSI = flags ; how u`: 1 (or absent) replaces the top entry's flags,
    /// 2 sets the given bits, 3 clears them. An empty stack gets an entry.
    pub fn set(&mut self, flags: u8, how: u16) {
        let (cur, f) = (self.flags(), flags & KITTY_FLAGS);
        let new = match how {
            0 | 1 => f,
            2 => cur | f,
            3 => cur & !f,
            _ => return,
        };
        match self.0.last_mut() {
            Some(top) => *top = new,
            None => self.0.push(new),
        }
    }
}

/// Mode state that does not depend on which screen is showing.
#[derive(Clone, Debug, Default)]
pub struct Modes {
    /// Input modes. `kitty` and `alt_screen` are not kept here; the
    /// terminal fills them in from the active screen.
    pub input: InputModes,
    /// Kitty keyboard flags stacks for the main and alternate screens.
    pub kitty: [KittyStack; 2],
    /// xterm modifyOtherKeys level, from `CSI > 4 ; n m`. Keys are not
    /// encoded with it, but a crashed program's level is still cleared.
    pub mok: u8,
    /// When the open synchronized update began.
    pub sync: Option<Instant>,
    /// The user confirmed a multi-line paste since bracketed paste was last
    /// set. Any program's output can set it, so until then it does not show
    /// that a program reading pastes safely is listening.
    pub paste_confirmed: bool,
    /// Mode 2031: report dark/light changes unasked, as `CSI ? 997 ; n n`.
    pub theme_reports: bool,
}

impl Modes {
    /// Sets one of the DEC private modes kept here. Returns false for any
    /// other mode.
    pub fn set_dec(&mut self, m: u16, on: bool) -> bool {
        let i = &mut self.input;
        match m {
            1 => i.decckm = on,
            66 => i.deckpam = on,
            // One tracking mode at a time; turning any of them off stops
            // tracking, as in xterm.
            1000 | 1002 | 1003 => {
                i.mouse = match (on, m) {
                    (false, _) => MouseMode::Off,
                    (true, 1000) => MouseMode::Click,
                    (true, 1002) => MouseMode::Drag,
                    (true, _) => MouseMode::Any,
                }
            }
            1004 => i.focus = on,
            1006 => i.mouse_sgr = on,
            2004 => {
                i.bracketed = on;
                self.paste_confirmed = false;
            }
            // A repeated begin keeps the first start time, so a program
            // that never ends its update is still shown every timeout.
            2026 if on => {
                self.sync.get_or_insert_with(Instant::now);
            }
            2026 => self.sync = None,
            2031 => self.theme_reports = on,
            9001 => i.w32im = on,
            _ => return false,
        }
        true
    }

    /// Whether a mode [`Modes::set_dec`] knows is set, or `None` for the rest.
    pub fn dec(&self, m: u16) -> Option<bool> {
        let i = &self.input;
        Some(match m {
            1 => i.decckm,
            66 => i.deckpam,
            1000 => i.mouse == MouseMode::Click,
            1002 => i.mouse == MouseMode::Drag,
            1003 => i.mouse == MouseMode::Any,
            1004 => i.focus,
            1006 => i.mouse_sgr,
            2004 => i.bracketed,
            2026 => self.sync.is_some(),
            2031 => self.theme_reports,
            9001 => i.w32im,
            _ => return None,
        })
    }

    /// Clears the input modes a program can leave behind when it dies
    /// without restoring them: both kitty stacks, modifyOtherKeys, mouse
    /// tracking, bracketed paste, synchronized output and theme reports.
    /// Otherwise the next program gets CSI-u keys or reports it never asked
    /// for.
    pub fn reset_input(&mut self) {
        self.kitty = Default::default();
        self.mok = 0;
        self.input.mouse = MouseMode::Off;
        self.input.mouse_sgr = false;
        self.input.bracketed = false;
        self.paste_confirmed = false;
        self.sync = None;
        self.theme_reports = false;
    }

    /// True while a synchronized update is open and younger than
    /// [`SYNC_TIMEOUT`].
    pub fn sync_pending(&self, now: Instant) -> bool {
        self.sync
            .is_some_and(|t| now.saturating_duration_since(t) < SYNC_TIMEOUT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kitty_stack_push_pop_set() {
        let mut s = KittyStack::default();
        assert_eq!(s.flags(), 0);
        s.set(5, 0);
        assert_eq!(s.entries(), [5]);
        s.push(0xFF);
        assert_eq!(s.flags(), 31);
        s.set(2, 3);
        assert_eq!(s.flags(), 29);
        s.set(2, 2);
        assert_eq!(s.flags(), 31);
        s.set(1, 9);
        assert_eq!(s.flags(), 31);
        s.pop(1);
        assert_eq!(s.flags(), 5);
        s.pop(10);
        assert_eq!(s.entries(), []);
        for f in 1..=10 {
            s.push(f);
        }
        assert_eq!(s.entries(), [3, 4, 5, 6, 7, 8, 9, 10]);
    }

    #[test]
    fn mouse_modes_replace_each_other() {
        let mut m = Modes::default();
        for n in [1000, 1002, 1003] {
            m.set_dec(n, true);
        }
        assert_eq!(m.input.mouse, MouseMode::Any);
        assert_eq!((m.dec(1000), m.dec(1003)), (Some(false), Some(true)));
        m.set_dec(1000, false);
        assert_eq!(m.input.mouse, MouseMode::Off);
        assert_eq!(m.dec(1016), None);
    }

    #[test]
    fn reset_input_clears_modify_other_keys() {
        let mut m = Modes {
            mok: 2,
            ..Modes::default()
        };
        m.kitty[1].push(1);
        m.reset_input();
        assert_eq!((m.mok, m.kitty[1].flags()), (0, 0));
    }

    #[test]
    fn sync_times_out() {
        let mut m = Modes::default();
        m.set_dec(2026, true);
        let t = m.sync.unwrap();
        assert!(m.sync_pending(t));
        assert!(m.sync_pending(t + SYNC_TIMEOUT - Duration::from_millis(1)));
        assert!(!m.sync_pending(t + SYNC_TIMEOUT));
        m.set_dec(2026, true);
        assert_eq!(m.sync, Some(t), "a repeated begin keeps the start");
        m.set_dec(2026, false);
        assert!(!m.sync_pending(t));
    }
}
