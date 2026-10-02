//! Shortcut chords, the default key map, and translating raw key messages
//! into key input for the terminal.

use vt::{Key, KeyInput, Locks, Mods};

/// Unshifted characters of a US layout by set-1 scan code, NUL where the
/// key types nothing.
const US_BY_SCAN: &[u8] =
    b"\0\x001234567890-=\0\0qwertyuiop[]\0\0asdfghjkl;'`\0\\zxcvbnm,./\0\0\0 ";

/// Turns a `WM_KEYDOWN`, `WM_KEYUP`, `WM_SYSKEYDOWN` or `WM_SYSKEYUP` into
/// key input for [`vt::encode_key`].
///
/// `keystate` is `GetKeyboardState` read while handling the message, so it
/// already includes this key's own transition. `layout(vk, scan, state)`
/// returns what the keyboard layout types for the key under `state`, as
/// `ToUnicodeEx` does without touching the dead key state, and nothing for
/// a dead key. The text of the result is stored in `text`.
pub fn msg_to_key<'a>(
    vk: u16,
    lparam: isize,
    keystate: &[u8; 256],
    layout: impl Fn(u16, u16, &[u8; 256]) -> String,
    text: &'a mut String,
) -> KeyInput<'a> {
    // Only the low 32 bits carry anything; a 64-bit LPARAM may be sign
    // extended for key-ups.
    let lp = lparam as u32;
    let scan = (lp >> 16 & 0xff) as u16;
    let extended = lp & 1 << 24 != 0;
    let held = |vk: usize| keystate[vk] & 0x80 != 0;
    let on = |vk: usize| keystate[vk] & 1 != 0;
    let mods = Mods {
        lshift: held(0xa0),
        rshift: held(0xa1),
        lctrl: held(0xa2),
        rctrl: held(0xa3),
        lalt: held(0xa4),
        ralt: held(0xa5),
        lsuper: held(0x5b),
        rsuper: held(0x5c),
    };
    let ctrl = mods.lctrl || mods.rctrl;
    let alt = mods.lalt || mods.ralt;

    let typed = layout(vk, scan, keystate);
    let uc = typed.encode_utf16().next().unwrap_or(0);
    // Windows reports AltGr as Ctrl+Alt. When Ctrl+Alt types something
    // printable that is the key's text; otherwise the text is what the key
    // types with Ctrl and Alt let go.
    *text = if !(ctrl || alt) || ctrl && alt && printable(&typed) {
        typed
    } else {
        let mut plain = *keystate;
        for vk in [0x11, 0x12, 0xa2, 0xa3, 0xa4, 0xa5] {
            plain[vk] &= !0x80;
        }
        layout(vk, scan, &plain)
    };
    if !printable(text) {
        text.clear();
    }

    let key = match vk {
        0x08 => Key::Backspace,
        0x09 => Key::Tab,
        0x0d => Key::Enter,
        0x1b => Key::Escape,
        0x21 => Key::PageUp,
        0x22 => Key::PageDown,
        0x23 => Key::End,
        0x24 => Key::Home,
        0x25 => Key::Left,
        0x26 => Key::Up,
        0x27 => Key::Right,
        0x28 => Key::Down,
        0x2d => Key::Insert,
        0x2e => Key::Delete,
        0x70..=0x87 => Key::F((vk - 0x6f) as u8),
        0x10 | 0xa0 | 0xa1 => Key::Shift,
        0x11 | 0xa2 | 0xa3 => Key::Control,
        0x12 | 0xa4 | 0xa5 => Key::Alt,
        0x5b | 0x5c => Key::Super,
        _ => {
            let base = layout(vk, scan, &[0; 256]);
            let mut chars = base.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) if !c.is_control() => Key::Char(c),
                // A key whose plain press is dead or empty still types
                // something with the modifiers held.
                _ => text.chars().next().map_or(Key::Other, Key::Char),
            }
        }
    };
    let us_base = US_BY_SCAN
        .get(usize::from(scan))
        .filter(|&&b| b != 0 && !extended)
        .map(|&b| char::from(b));

    KeyInput {
        vk,
        scan,
        extended,
        down: lp & 1 << 31 == 0,
        repeat: lp as u16,
        mods,
        locks: Locks {
            caps: on(0x14),
            num: on(0x90),
            scroll: on(0x91),
        },
        text,
        uc,
        cs: 0,
        key,
        us_base,
    }
}

fn printable(s: &str) -> bool {
    !s.is_empty() && !s.chars().any(char::is_control)
}

#[cfg(test)]
mod msg_to_key_tests {
    use super::*;

    const HELD: u8 = 0x80;

    /// Keyboard state with these left/right modifier keys held (the generic
    /// VK_SHIFT, VK_CONTROL and VK_MENU are set too, as Windows does) and
    /// these lock keys toggled on.
    fn state(held: &[usize], locks: &[usize]) -> [u8; 256] {
        let mut s = [0; 256];
        for &vk in held {
            s[vk] |= HELD;
            match vk {
                0xa0 | 0xa1 => s[0x10] |= HELD,
                0xa2 | 0xa3 => s[0x11] |= HELD,
                0xa4 | 0xa5 => s[0x12] |= HELD,
                _ => {}
            }
        }
        for &vk in locks {
            s[vk] |= 1;
        }
        s
    }

    /// lParam of a key message. Key-ups are sign extended, as they can be
    /// in a 64-bit LPARAM.
    fn lp(scan: u16, extended: bool, down: bool, repeat: u16) -> isize {
        let mut v = u32::from(repeat) | u32::from(scan) << 16 | u32::from(extended) << 24;
        if !down {
            v |= 3 << 30;
        }
        v as i32 as isize
    }

    /// A test layout from `(vk, plain, shifted, altgr)` rows. Ctrl alone
    /// turns letters and Enter into control codes, as Windows layouts do.
    fn layout(
        rows: &'static [(u16, &'static str, &'static str, &'static str)],
    ) -> impl Fn(u16, u16, &[u8; 256]) -> String {
        move |vk, _, s| {
            let held = |vk: usize| s[vk] & HELD != 0;
            let Some(&(_, plain, shifted, altgr)) = rows.iter().find(|r| r.0 == vk) else {
                return String::new();
            };
            let caps = s[0x14] & 1 != 0 && plain.chars().all(char::is_alphabetic);
            match (held(0x11), held(0x12)) {
                (true, true) => altgr.into(),
                (true, false) if (0x41..=0x5a).contains(&vk) => {
                    char::from(vk as u8 - 0x40).to_string()
                }
                (true, false) if vk == 0x0d => "\n".into(),
                _ if held(0x10) != caps => shifted.into(),
                _ => plain.into(),
            }
        }
    }

    const US: &[(u16, &str, &str, &str)] = &[
        (0x0d, "\r", "\r", ""),
        (0x20, " ", " ", ""),
        (0x41, "a", "A", ""),
        (0x4a, "j", "J", ""),
        (0x51, "q", "Q", ""),
        (0x67, "7", "7", ""),
        (0x6e, ".", ".", ""),
    ];

    #[test]
    fn keymap_shift_enter_fields() {
        let mut t = String::new();
        let k = msg_to_key(
            0x0d,
            lp(0x1c, false, true, 1),
            &state(&[0xa0], &[]),
            layout(US),
            &mut t,
        );
        let want = KeyInput {
            vk: 0x0d,
            scan: 0x1c,
            extended: false,
            down: true,
            repeat: 1,
            mods: Mods {
                lshift: true,
                ..Mods::default()
            },
            locks: Locks::default(),
            text: "",
            uc: 13,
            cs: 0,
            key: Key::Enter,
            us_base: None,
        };
        assert_eq!(k, want);
    }

    #[test]
    fn keymap_letters_text_and_control_codes() {
        let mut t = String::new();
        let k = msg_to_key(
            0x41,
            lp(0x1e, false, true, 1),
            &state(&[0xa1], &[]),
            layout(US),
            &mut t,
        );
        assert_eq!((k.text, k.uc, k.key), ("A", 'A' as u16, Key::Char('a')));
        assert!(k.mods.rshift && !k.mods.lshift);
        assert_eq!(k.us_base, Some('a'));

        // Caps Lock with Shift types lowercase again.
        let k = msg_to_key(
            0x41,
            lp(0x1e, false, true, 1),
            &state(&[0xa0], &[0x14]),
            layout(US),
            &mut t,
        );
        assert_eq!(k.text, "a");
        assert!(k.locks.caps);

        // Ctrl+J: the text ignores Ctrl, uc is the control code.
        let k = msg_to_key(
            0x4a,
            lp(0x24, false, true, 1),
            &state(&[0xa2], &[]),
            layout(US),
            &mut t,
        );
        assert_eq!((k.text, k.uc, k.key), ("j", 10, Key::Char('j')));
        assert!(k.mods.lctrl);
    }

    #[test]
    fn keymap_lparam_repeat_and_release() {
        let mut t = String::new();
        let k = msg_to_key(
            0x20,
            lp(0x39, false, true, 3),
            &[0; 256],
            layout(US),
            &mut t,
        );
        assert_eq!((k.down, k.repeat, k.text), (true, 3, " "));
        let k = msg_to_key(
            0x20,
            lp(0x39, false, false, 1),
            &[0; 256],
            layout(US),
            &mut t,
        );
        assert_eq!((k.down, k.repeat, k.scan), (false, 1, 0x39));
    }

    #[test]
    fn keymap_modifier_keys_and_locks() {
        let mut t = String::new();
        // Right Ctrl is VK_CONTROL with the extended bit.
        let k = msg_to_key(
            0x11,
            lp(0x1d, true, true, 1),
            &state(&[0xa3], &[0x90, 0x91]),
            layout(US),
            &mut t,
        );
        assert_eq!(
            (k.key, k.extended, k.uc, k.text),
            (Key::Control, true, 0, "")
        );
        assert!(k.mods.rctrl && !k.mods.lctrl);
        assert_eq!(
            (k.locks.num, k.locks.scroll, k.locks.caps),
            (true, true, false)
        );
        assert_eq!(k.us_base, None);
    }

    #[test]
    fn keymap_us_base_follows_the_key_position() {
        // German Z sits where US has Y.
        const DE: &[(u16, &str, &str, &str)] = &[(0x5a, "z", "Z", "")];
        let mut t = String::new();
        let k = msg_to_key(
            0x5a,
            lp(0x15, false, true, 1),
            &[0; 256],
            layout(DE),
            &mut t,
        );
        assert_eq!((k.key, k.us_base), (Key::Char('z'), Some('y')));
    }

    #[test]
    fn keymap_keys_without_text() {
        let mut t = String::new();
        let k = msg_to_key(0x5d, lp(0x5d, true, true, 1), &[0; 256], layout(US), &mut t);
        assert_eq!((k.key, k.text, k.uc), (Key::Other, "", 0));
        let k = msg_to_key(
            0x7b,
            lp(0x58, false, true, 1),
            &[0; 256],
            layout(US),
            &mut t,
        );
        assert_eq!(k.key, Key::F(12));
    }
}
