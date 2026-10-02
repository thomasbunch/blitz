//! Encoding keys, mouse, paste and focus for the child process.

use crate::modes::InputModes;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub lshift: bool,
    pub rshift: bool,
    pub lctrl: bool,
    pub rctrl: bool,
    pub lalt: bool,
    pub ralt: bool,
    pub lsuper: bool,
    pub rsuper: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Locks {
    pub caps: bool,
    pub num: bool,
    pub scroll: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    Tab,
    Backspace,
    Escape,
    Insert,
    Delete,
    Home,
    End,
    PageUp,
    PageDown,
    Up,
    Down,
    Left,
    Right,
    F(u8),
    Shift,
    Control,
    Alt,
    Super,
    Other,
}

/// One key transition, with everything any of the encodings needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyInput<'a> {
    /// Windows virtual-key code.
    pub vk: u16,
    pub scan: u16,
    pub extended: bool,
    pub down: bool,
    pub repeat: u16,
    pub mods: Mods,
    pub locks: Locks,
    /// Text the layout produces for this key, if any.
    pub text: &'a str,
    /// UTF-16 unit from `ToUnicodeEx`, 0 if none.
    pub uc: u16,
    /// win32-input-mode control-key state.
    pub cs: u32,
    pub key: Key,
    /// The key's character on a US layout, for shortcuts on other layouts.
    pub us_base: Option<char>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseKind {
    Press,
    Release,
    Move,
    WheelUp,
    WheelDown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MouseEv {
    pub kind: MouseKind,
    pub button: u8,
    pub col: u16,
    pub row: u16,
    pub mods: Mods,
}

/// Appends the bytes for `k` under the current modes.
pub fn encode_key(_k: &KeyInput, _m: &InputModes, _out: &mut Vec<u8>) {}

/// Appends a mouse report. Returns false when the modes don't ask for this
/// event and nothing was written.
pub fn encode_mouse(_ev: MouseEv, _m: &InputModes, _out: &mut Vec<u8>) -> bool {
    false
}

/// Appends pasted text, filtered and bracketed when requested.
pub fn encode_paste(_text: &str, _bracketed: bool, _out: &mut Vec<u8>) {}

/// Appends a focus report when mode 1004 is set.
pub fn encode_focus(_focused: bool, _m: &InputModes, _out: &mut Vec<u8>) {}
