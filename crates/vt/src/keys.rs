//! Encoding keys, mouse, paste and focus for the child process.

use std::io::Write as _;

use crate::modes::{InputModes, MouseMode};

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
    /// 0 left, 1 middle, 2 right. On `Move` it is the held button, or 3
    /// when none is.
    pub button: u8,
    /// 0-based cell.
    pub col: u16,
    pub row: u16,
    pub mods: Mods,
}

const ESC: u8 = 0x1b;

/// Windows' "the IME owns this key" virtual key.
const VK_PROCESSKEY: u16 = 0xe5;

const VK_CANCEL: u16 = 0x03;
/// Keypad 5 with Num Lock off: Begin.
const VK_CLEAR: u16 = 0x0c;
const VK_C: u16 = 0x43;

// Kitty keyboard protocol flags.
const DISAMBIGUATE: u8 = 1;
const EVENT_TYPES: u8 = 2;
const ALTERNATE_KEYS: u8 = 4;
const ALL_KEYS: u8 = 8;
const ASSOCIATED_TEXT: u8 = 16;

/// Appends the bytes for `k` under the current modes.
pub fn encode_key(k: &KeyInput, m: &InputModes, out: &mut Vec<u8>) {
    if k.vk == VK_PROCESSKEY {
        return;
    }
    // Ctrl+C and Ctrl+Break stay console key records under ConPTY even
    // when kitty flags are pushed: only then does conhost interrupt a
    // program that reads keys, and any program can print a push.
    if m.w32im && is_interrupt(k) {
        win32(k, out);
    } else if m.kitty & (DISAMBIGUATE | ALL_KEYS) != 0 {
        // Without disambiguate or all-keys, kitty flags leave presses legacy.
        kitty(k, m.kitty, out);
    } else if m.w32im {
        win32(k, out);
    } else {
        legacy(k, m, out);
    }
}

/// Whether `k` is the interrupt chord: C or Break with Ctrl, and neither
/// Shift nor Alt. Ctrl+Shift+C is copy in most terminals.
pub fn is_interrupt(k: &KeyInput) -> bool {
    matches!(k.vk, VK_C | VK_CANCEL) && mod_bits(k) & 7 == 4
}

/// Whether `k` reaches the program as Ctrl+C, which interrupts it: the
/// interrupt chord always, and C with Ctrl and Shift too unless kitty
/// flags send that as a key of its own.
pub fn interrupts(k: &KeyInput, m: &InputModes) -> bool {
    let bits = mod_bits(k);
    let kitty = m.kitty & (DISAMBIGUATE | ALL_KEYS) != 0;
    is_interrupt(k) || k.vk == VK_C && bits & 6 == 4 && !kitty
}

/// win32-input-mode: one console key record per transition, releases and
/// bare modifier keys included. Console apps such as PSReadLine need it to
/// see Shift on Enter.
fn win32(k: &KeyInput, out: &mut Vec<u8>) {
    let m = &k.mods;
    let cs = k.cs
        | u32::from(m.ralt)
        | u32::from(m.lalt) << 1
        | u32::from(m.rctrl) << 2
        | u32::from(m.lctrl) << 3
        | u32::from(m.lshift || m.rshift) << 4
        | u32::from(k.locks.num) << 5
        | u32::from(k.locks.scroll) << 6
        | u32::from(k.locks.caps) << 7
        | u32::from(k.extended) << 8;
    let _ = write!(
        out,
        "\x1b[{};{};{};{};{};{}_",
        k.vk,
        k.scan,
        k.uc,
        u8::from(k.down),
        cs,
        k.repeat.max(1)
    );
}

/// xterm modifier bits: shift 1, alt 2, ctrl 4, super 8. Kitty uses the same
/// four.
///
/// Windows reports AltGr as Ctrl+Alt. When Ctrl+Alt produced printable text
/// the key was AltGr on a layout that has it (`@` on a German keyboard, `ą`
/// on a Polish one), so Ctrl and Alt are dropped and the text goes out as
/// typed. On a US layout Ctrl+Alt+A produces nothing and stays a chord.
fn mod_bits(k: &KeyInput) -> u32 {
    let m = &k.mods;
    let bits = u32::from(m.lshift || m.rshift)
        | u32::from(m.lalt || m.ralt) << 1
        | u32::from(m.lctrl || m.rctrl) << 2
        | u32::from(m.lsuper || m.rsuper) << 3;
    let printable = k.uc >= 0x20 && !(0x7f..0xa0).contains(&k.uc);
    if bits & 6 == 6 && printable && !k.text.is_empty() {
        bits & !6
    } else {
        bits
    }
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

/// The C0 byte xterm sends for Ctrl plus this character, if there is one:
/// Xlib's rule, which takes `@` to `~` to C0 and lets the digits 2 to 8
/// stand for the characters above them, plus Windows' Ctrl+- and Ctrl+?.
fn ctrl_byte(c: char) -> Option<u8> {
    Some(match c {
        '@'..='~' => c as u8 & 0x1f,
        ' ' | '2' => 0,
        '3'..='7' => c as u8 - b'3' + 0x1b,
        '8' | '?' => 0x7f,
        '-' | '/' => 0x1f,
        _ => return None,
    })
}

/// SS3 final byte for a keypad key in application keypad mode (DECKPAM).
/// Only Num Lock off gets here, when Windows reports the digit keys as
/// the navigation keys printed under them, so only the operators and
/// Enter are left.
fn keypad_app(k: &KeyInput) -> Option<u8> {
    Some(match k.vk {
        0x6a => b'j', // multiply
        0x6b => b'k', // add
        0x6c => b'l', // separator
        0x6d => b'm', // subtract
        0x6f => b'o', // divide
        0x0d if k.extended => b'M',
        _ => return None,
    })
}

/// Whether this is a cursor key: the arrows, Home and End, and Begin.
fn is_cursor(k: &KeyInput) -> bool {
    matches!(
        k.key,
        Key::Up | Key::Down | Key::Right | Key::Left | Key::Home | Key::End
    ) || k.key == Key::Other && k.vk == VK_CLEAR
}

/// Plain xterm encoding: what a terminal sends when the application asked
/// for nothing better.
fn legacy(k: &KeyInput, m: &InputModes, out: &mut Vec<u8>) {
    if !k.down {
        return;
    }
    let bits = mod_bits(k);
    let (shift, alt, ctrl) = (bits & 1 != 0, bits & 2 != 0, bits & 4 != 0);
    let m1 = bits + 1;
    let esc_if_alt = |out: &mut Vec<u8>| {
        if alt {
            out.push(ESC);
        }
    };
    // With Num Lock on the keypad types what is printed on it, as in xterm.
    if m.deckpam
        && bits == 0
        && !k.locks.num
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
        _ if is_cursor(k) => {
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
        Key::End => b'F',
        _ => b'E',
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

/// The kitty keyboard protocol, limited to the flags the application
/// pushed: no event types, alternates or text unless asked for.
fn kitty(k: &KeyInput, flags: u8, out: &mut Vec<u8>) {
    let all = flags & ALL_KEYS != 0;
    // KeyInput has no auto-repeat bit, so repeats go out as presses. Add
    // one if an app that pushes event types turns out to care.
    let event = if k.down { 1 } else { 3 };
    if !k.down && flags & EVENT_TYPES == 0 {
        return;
    }
    let mut bits = mod_bits(k);
    // The text is what the key types, and a Ctrl, Alt or Super chord types
    // nothing; KeyInput's text leaves those modifiers out.
    let typed = bits & !1 == 0;
    // Lock keys only show up with all keys as escape codes. Apps that push
    // less tend to compare the modifier field exactly, and Num Lock is on
    // for most Windows users.
    if all {
        bits |= u32::from(k.locks.caps) << 6 | u32::from(k.locks.num) << 7;
    }
    let m1 = bits + 1;
    let text = if all && flags & ASSOCIATED_TEXT != 0 && k.down && typed {
        k.text
    } else {
        ""
    };
    // Keypad keys get their own codes only with all keys as escape codes.
    // The spec also wants them for non-text keypad keys under disambiguate,
    // but apps that push just that may not map them, and keypad Enter has
    // to keep submitting.
    if all && let Some(code) = keypad_code(k) {
        csi(out, code, None, None, m1, event, text, b'u');
        return;
    }
    let key = |out: &mut Vec<u8>, num: u32, fin: u8| csi(out, num, None, None, m1, event, "", fin);
    match k.key {
        Key::Char(c) => {
            if !all && bits & !1 == 0 {
                // Text keys alone or with Shift stay plain text.
                if k.down {
                    out.extend_from_slice(k.text.as_bytes());
                }
                return;
            }
            let code = c.to_lowercase().next().unwrap_or(c) as u32;
            let alternates = flags & ALTERNATE_KEYS != 0;
            let shifted =
                single(k.text).filter(|s| alternates && bits & 1 != 0 && *s as u32 != code);
            let base = k.us_base.filter(|b| alternates && *b as u32 != code);
            csi(out, code, shifted, base, m1, event, text, b'u');
        }
        Key::Enter | Key::Tab | Key::Backspace => {
            let (code, plain) = match k.key {
                Key::Enter => (13, b'\r'),
                Key::Tab => (9, b'\t'),
                _ => (127, 0x7f),
            };
            // Unmodified they keep their legacy bytes, so `reset` can still
            // be typed after a crashed app leaves flags pushed, and they
            // report releases only with all keys as escape codes.
            if !all && (bits == 0 || !k.down) {
                if k.down {
                    out.push(plain);
                }
                return;
            }
            key(out, code, b'u');
        }
        Key::Escape => key(out, 27, b'u'),
        _ if is_cursor(k) => key(out, 1, cursor_final(k.key)),
        Key::Insert | Key::Delete | Key::PageUp | Key::PageDown => {
            key(out, tilde_number(k.key), b'~');
        }
        // CSI R would read as a cursor position report.
        Key::F(3) => key(out, 13, b'~'),
        Key::F(n @ (1 | 2 | 4)) => key(out, 1, b'P' + n - 1),
        Key::F(n @ 5..=12) => key(out, f_tilde(n), b'~'),
        Key::F(n @ 13..=35) => key(out, 57376 + u32::from(n - 13), b'u'),
        _ => {
            if all && let Some(code) = modifier_code(k) {
                key(out, code, b'u');
            }
        }
    }
}

/// The string's only character, if it has exactly one.
fn single(s: &str) -> Option<char> {
    let mut it = s.chars();
    it.next().filter(|_| it.next().is_none())
}

/// Kitty's private-use code for a keypad key. With Num Lock off the digit
/// keys arrive as navigation keys that, unlike the main ones, are not
/// extended. Begin keeps its `CSI E` form, as the spec allows.
fn keypad_code(k: &KeyInput) -> Option<u32> {
    Some(match k.vk {
        0x60..=0x69 => 57399 + u32::from(k.vk - 0x60),
        0x6e => 57409, // decimal
        0x6f => 57410, // divide
        0x6a => 57411, // multiply
        0x6d => 57412, // subtract
        0x6b => 57413, // add
        0x0d if k.extended => 57414,
        0x6c => 57416, // separator
        _ if k.extended => return None,
        0x25 => 57417, // left
        0x27 => 57418, // right
        0x26 => 57419, // up
        0x28 => 57420, // down
        0x21 => 57421, // page up
        0x22 => 57422, // page down
        0x24 => 57423, // home
        0x23 => 57424, // end
        0x2d => 57425, // insert
        0x2e => 57426, // delete
        _ => return None,
    })
}

/// Kitty's code for a modifier or lock key. Windows reports the generic
/// VK_SHIFT, VK_CONTROL and VK_MENU; the right-hand ones are told apart by
/// scan code or the extended bit.
fn modifier_code(k: &KeyInput) -> Option<u32> {
    Some(match k.vk {
        0x10 | 0xa0 | 0xa1 if k.vk == 0xa1 || k.scan == 0x36 => 57447,
        0x10 | 0xa0 | 0xa1 => 57441,
        0x11 | 0xa2 | 0xa3 if k.vk == 0xa3 || k.extended => 57448,
        0x11 | 0xa2 | 0xa3 => 57442,
        0x12 | 0xa4 | 0xa5 if k.vk == 0xa5 || k.extended => 57449,
        0x12 | 0xa4 | 0xa5 => 57443,
        0x5b => 57444,
        0x5c => 57450,
        0x14 => 57358, // caps lock
        0x91 => 57359, // scroll lock
        0x90 => 57360, // num lock
        _ => return None,
    })
}

/// Appends an SGR (mode 1006) mouse report. Returns false when the modes
/// don't ask for this event and nothing was written.
pub fn encode_mouse(ev: MouseEv, m: &InputModes, out: &mut Vec<u8>) -> bool {
    // SGR only. ConPTY asks for 1006 itself; add the X10 byte form if an
    // app ever enables 1000 without 1006 and expects it.
    if m.mouse == MouseMode::Off || !m.mouse_sgr {
        return false;
    }
    let cb = match ev.kind {
        MouseKind::Press | MouseKind::Release if ev.button < 3 => u32::from(ev.button),
        MouseKind::Move => match (m.mouse, ev.button) {
            (MouseMode::Any, 0..=3) | (MouseMode::Drag, 0..=2) => 32 + u32::from(ev.button),
            _ => return false,
        },
        MouseKind::WheelUp => 64,
        MouseKind::WheelDown => 65,
        _ => return false,
    };
    let mo = &ev.mods;
    let cb = cb
        | u32::from(mo.lshift || mo.rshift) << 2
        | u32::from(mo.lalt || mo.ralt) << 3
        | u32::from(mo.lctrl || mo.rctrl) << 4;
    let fin = if ev.kind == MouseKind::Release {
        'm'
    } else {
        'M'
    };
    let (x, y) = (u32::from(ev.col) + 1, u32::from(ev.row) + 1);
    let _ = write!(out, "\x1b[<{cb};{x};{y}{fin}");
    true
}

/// Drops pointer motion that stays inside one cell, so the app gets one
/// report per cell crossed instead of one per pixel.
#[derive(Clone, Copy, Debug, Default)]
pub struct MouseTracker {
    last: Option<(u16, u16)>,
}

impl MouseTracker {
    /// [`encode_mouse`], except that a move within the cell of the previous
    /// event writes nothing and returns false.
    pub fn encode(&mut self, ev: MouseEv, m: &InputModes, out: &mut Vec<u8>) -> bool {
        let cell = Some((ev.col, ev.row));
        if ev.kind == MouseKind::Move && self.last == cell {
            return false;
        }
        let sent = encode_mouse(ev, m, out);
        if sent {
            self.last = cell;
        }
        sent
    }
}

/// Appends pasted text, filtered and bracketed when requested.
///
/// The filter is Windows Terminal's: C0 controls other than tab, LF and CR
/// are dropped, as are DEL and C1, so pasted text can't carry escape
/// sequences of its own, an early `ESC[201~` included. Line breaks become
/// CR, which is what Enter sends.
pub fn encode_paste(text: &str, bracketed: bool, out: &mut Vec<u8>) {
    if bracketed {
        out.extend_from_slice(b"\x1b[200~");
    }
    // Dropped characters are left out before line breaks are paired, so CR,
    // ESC, LF is one break.
    let mut prev = '\0';
    for c in text.chars() {
        match c {
            '\n' if prev == '\r' => {}
            '\n' | '\r' => out.push(b'\r'),
            '\t' => out.push(b'\t'),
            '\0'..='\x1f' | '\x7f'..='\u{9f}' => continue,
            _ => out.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes()),
        }
        prev = c;
    }
    if bracketed {
        out.extend_from_slice(b"\x1b[201~");
    }
}

/// Whether a paste should be confirmed first. Without bracketed paste a
/// shell runs every pasted line the moment it arrives; Windows PowerShell
/// 5.1's PSReadLine never turns bracketed paste on. Nor does cmd, but
/// `type` on a file can turn the mode on while cmd or another program
/// that knows nothing of it reads the keys, and conhost then runs every
/// line. So `trusted` is [`crate::Terminal::paste_trusted`]: bracketed
/// paste on, and a paste under it already confirmed by the user. A paste
/// of more than [`LARGE_PASTE`] bytes without bracketed paste is confirmed
/// too, even on one line, as in Windows Terminal: it is typed in key by key.
pub fn needs_paste_confirm(text: &str, bracketed: bool, trusted: bool) -> bool {
    !trusted && (text.contains(['\r', '\n']) || !bracketed && text.len() > LARGE_PASTE)
}

/// Bytes of text above which a paste without bracketed paste is confirmed.
pub const LARGE_PASTE: usize = 5 * 1024;

/// Appends a focus report when mode 1004 is set.
pub fn encode_focus(focused: bool, m: &InputModes, out: &mut Vec<u8>) {
    if m.focus {
        out.extend_from_slice(if focused { b"\x1b[I" } else { b"\x1b[O" });
    }
}
