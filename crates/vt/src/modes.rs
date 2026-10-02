//! Terminal modes and the subset the input encoders need.

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
