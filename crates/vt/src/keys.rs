//! Encoding keys, mouse, paste and focus for the child process.

use std::io::Write as _;

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
    /// A text key. The char is what the key produces on the active layout
    /// with no modifiers held (`'a'` for the A key, `'-'` for minus).
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
    /// Text the layout produces for this key with Shift, Caps Lock and AltGr
    /// applied but plain Ctrl and Alt ignored: `"A"` for Shift+A, `"_"` for
    /// Ctrl+Shift+-, `"@"` for AltGr+Q on a German layout. Empty for dead
    /// keys and keys without text. Never contains control characters.
    pub text: &'a str,
    /// UTF-16 unit from `ToUnicodeEx` with the full keyboard state, 0 if
    /// none. A printable value while Ctrl and Alt are both down marks AltGr.
    pub uc: u16,
    /// win32-input-mode control-key state. Bits implied by `mods`, `locks`
    /// and `extended` are added by the encoder, so 0 is fine.
    pub cs: u32,
    pub key: Key,
    /// The key's unshifted character on a US layout, for shortcuts on other
    /// layouts.
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

const ESC: u8 = 0x1b;

/// Windows' "the IME owns this key" virtual key.
const VK_PROCESSKEY: u16 = 0xe5;

/// Appends the bytes for `k` under the current modes.
pub fn encode_key(k: &KeyInput, m: &InputModes, out: &mut Vec<u8>) {
    if k.vk == VK_PROCESSKEY {
        return;
    }
    legacy(k, m, out);
}

/// xterm modifier bits: shift 1, alt 2, ctrl 4, super 8. Kitty uses the same
/// four.
fn mod_bits(m: &Mods) -> u32 {
    u32::from(m.lshift || m.rshift)
        | u32::from(m.lalt || m.ralt) << 1
        | u32::from(m.lctrl || m.rctrl) << 2
        | u32::from(m.lsuper || m.rsuper) << 3
}

/// Writes `CSI num[:shifted[:base]] [;mods[:event]] [;text] fin`. The
/// number is left out when it is 1 and nothing follows it, as in `CSI A`.
#[allow(clippy::too_many_arguments)]
fn csi(
    out: &mut Vec<u8>,
    num: u32,
    shifted: Option<char>,
    base: Option<char>,
    m1: u32,
    event: u8,
    text: &str,
    fin: u8,
) {
    out.extend_from_slice(b"\x1b[");
    let mods = m1 != 1 || event != 1 || !text.is_empty();
    if num != 1 || mods || shifted.is_some() || base.is_some() {
        let _ = write!(out, "{num}");
    }
    if let Some(s) = shifted {
        let _ = write!(out, ":{}", s as u32);
    }
    if let Some(b) = base {
        if shifted.is_none() {
            out.push(b':');
        }
        let _ = write!(out, ":{}", b as u32);
    }
    if mods {
        let _ = write!(out, ";{m1}");
        if event != 1 {
            let _ = write!(out, ":{event}");
        }
    }
    if !text.is_empty() {
        let mut sep = b';';
        for c in text.chars() {
            out.push(sep);
            let _ = write!(out, "{}", c as u32);
            sep = b':';
        }
    }
    out.push(fin);
}

/// The C0 byte xterm sends for Ctrl plus this character, if there is one.
fn ctrl_byte(c: char) -> Option<u8> {
    Some(match c {
        'a'..='z' | 'A'..='Z' => c as u8 & 0x1f,
        ' ' | '2' | '@' => 0,
        '[' | '3' => 0x1b,
        '\\' | '4' => 0x1c,
        ']' | '5' => 0x1d,
        '^' | '6' => 0x1e,
        '_' | '7' | '-' | '/' => 0x1f,
        '8' | '?' => 0x7f,
        _ => return None,
    })
}

/// SS3 final byte for a keypad key in application keypad mode (DECKPAM).
fn keypad_app(k: &KeyInput) -> Option<u8> {
    Some(match k.vk {
        0x60..=0x69 => b'p' + (k.vk - 0x60) as u8,
        0x6a => b'j', // multiply
        0x6b => b'k', // add
        0x6c => b'l', // separator
        0x6d => b'm', // subtract
        0x6e => b'n', // decimal
        0x6f => b'o', // divide
        0x0d if k.extended => b'M',
        _ => return None,
    })
}

/// Plain xterm encoding: what a terminal sends when the application asked
/// for nothing better.
fn legacy(k: &KeyInput, m: &InputModes, out: &mut Vec<u8>) {
    if !k.down {
        return;
    }
    let bits = mod_bits(&k.mods);
    let (shift, alt, ctrl) = (bits & 1 != 0, bits & 2 != 0, bits & 4 != 0);
    let m1 = bits + 1;
    let esc_if_alt = |out: &mut Vec<u8>| {
        if alt {
            out.push(ESC);
        }
    };
    if m.deckpam
        && bits == 0
        && let Some(f) = keypad_app(k)
    {
        out.extend_from_slice(&[ESC, b'O', f]);
        return;
    }
    match k.key {
        Key::Char(c) => {
            let cb = if ctrl {
                k.text
                    .chars()
                    .next()
                    .and_then(ctrl_byte)
                    .or_else(|| ctrl_byte(c))
                    .or_else(|| k.us_base.and_then(ctrl_byte))
            } else {
                None
            };
            if cb.is_none() && k.text.is_empty() {
                return;
            }
            esc_if_alt(out);
            match cb {
                Some(b) => out.push(b),
                None => out.extend_from_slice(k.text.as_bytes()),
            }
        }
        Key::Enter => {
            esc_if_alt(out);
            out.push(b'\r');
        }
        Key::Tab => {
            esc_if_alt(out);
            out.extend_from_slice(if shift { b"\x1b[Z" } else { b"\t" });
        }
        Key::Backspace => {
            esc_if_alt(out);
            out.push(if ctrl { 0x08 } else { 0x7f });
        }
        Key::Escape => {
            esc_if_alt(out);
            out.push(ESC);
        }
        Key::Up | Key::Down | Key::Right | Key::Left | Key::Home | Key::End => {
            let fin = cursor_final(k.key);
            if m1 == 1 && m.decckm {
                out.extend_from_slice(&[ESC, b'O', fin]);
            } else {
                csi(out, 1, None, None, m1, 1, "", fin);
            }
        }
        Key::Insert | Key::Delete | Key::PageUp | Key::PageDown => {
            csi(out, tilde_number(k.key), None, None, m1, 1, "", b'~');
        }
        Key::F(n @ 1..=4) => {
            let fin = b'P' + n - 1;
            if m1 == 1 {
                out.extend_from_slice(&[ESC, b'O', fin]);
            } else {
                csi(out, 1, None, None, m1, 1, "", fin);
            }
        }
        Key::F(n @ 5..=12) => csi(out, f_tilde(n), None, None, m1, 1, "", b'~'),
        _ => {}
    }
}

fn cursor_final(key: Key) -> u8 {
    match key {
        Key::Up => b'A',
        Key::Down => b'B',
        Key::Right => b'C',
        Key::Left => b'D',
        Key::Home => b'H',
        _ => b'F',
    }
}

fn tilde_number(key: Key) -> u32 {
    match key {
        Key::Insert => 2,
        Key::Delete => 3,
        Key::PageUp => 5,
        _ => 6,
    }
}

/// `CSI n ~` number for F5 to F12.
fn f_tilde(n: u8) -> u32 {
    [15, 17, 18, 19, 20, 21, 23, 24][usize::from(n - 5)]
}

/// Appends a mouse report. Returns false when the modes don't ask for this
/// event and nothing was written.
pub fn encode_mouse(_ev: MouseEv, _m: &InputModes, _out: &mut Vec<u8>) -> bool {
    false
}

/// Appends pasted text, filtered and bracketed when requested.
pub fn encode_paste(_text: &str, _bracketed: bool, _out: &mut Vec<u8>) {}

/// Appends a focus report when mode 1004 is set.
pub fn encode_focus(_focused: bool, _m: &InputModes, _out: &mut Vec<u8>) {}
