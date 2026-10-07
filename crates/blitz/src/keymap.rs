//! Shortcut chords, the default key map, and translating raw key messages
//! into key input for the terminal.

use vt::{Key, KeyInput, Locks, Mods};

use crate::layout::Dir;

/// What a shortcut does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Copy the selection. Without one the key goes to the program.
    Copy,
    /// Paste clipboard text. Without text the key goes to the program.
    Paste,
    /// Scroll the main screen by a page; positive is up.
    ScrollPage(i8),
    NewTab,
    ClosePane,
    /// Next (1) or previous (-1) tab.
    CycleTab(i8),
    /// Tab 1 to 9, 0-based.
    GoToTab(u8),
    SplitRight,
    SplitDown,
    Focus(Dir),
    /// Move the nearest divider of the focused pane toward `Dir`.
    Resize(Dir),
    /// Trade places with the pane in `Dir`.
    Swap(Dir),
    JumpToAttention,
    ToggleSidebar,
    /// Install the newer release, or without one look for it now. Without
    /// a focused pane the key goes to the program.
    Update,
    /// Open the theme picker, or close it unchanged.
    ThemePicker,
    /// Open or close the settings panel.
    Settings,
}

const CTRL: u8 = 1;
const SHIFT: u8 = 2;
const ALT: u8 = 4;

/// Default shortcuts as (modifiers, virtual key, action). Keys are matched
/// by virtual-key code: letter codes follow the key labels on Latin
/// layouts and the US positions on others, and digits and arrows are
/// physical keys everywhere.
const DEFAULT_KEYS: &[(u8, u16, Action)] = &[
    (CTRL, b'C' as u16, Action::Copy),
    (CTRL | SHIFT, b'C' as u16, Action::Copy),
    (CTRL, 0x2d, Action::Copy),
    (CTRL, b'V' as u16, Action::Paste),
    (CTRL | SHIFT, b'V' as u16, Action::Paste),
    (SHIFT, 0x2d, Action::Paste),
    (SHIFT, 0x21, Action::ScrollPage(1)),
    (SHIFT, 0x22, Action::ScrollPage(-1)),
    (CTRL | SHIFT, b'T' as u16, Action::NewTab),
    (CTRL | SHIFT, b'W' as u16, Action::ClosePane),
    (CTRL, 0x09, Action::CycleTab(1)),
    (CTRL | SHIFT, 0x09, Action::CycleTab(-1)),
    (CTRL | SHIFT, b'R' as u16, Action::SplitRight),
    (CTRL | SHIFT, b'D' as u16, Action::SplitDown),
    (CTRL | ALT, 0x25, Action::Focus(Dir::Left)),
    (CTRL | ALT, 0x26, Action::Focus(Dir::Up)),
    (CTRL | ALT, 0x27, Action::Focus(Dir::Right)),
    (CTRL | ALT, 0x28, Action::Focus(Dir::Down)),
    (ALT | SHIFT, 0x25, Action::Resize(Dir::Left)),
    (ALT | SHIFT, 0x26, Action::Resize(Dir::Up)),
    (ALT | SHIFT, 0x27, Action::Resize(Dir::Right)),
    (ALT | SHIFT, 0x28, Action::Resize(Dir::Down)),
    (CTRL | ALT | SHIFT, 0x25, Action::Swap(Dir::Left)),
    (CTRL | ALT | SHIFT, 0x26, Action::Swap(Dir::Up)),
    (CTRL | ALT | SHIFT, 0x27, Action::Swap(Dir::Right)),
    (CTRL | ALT | SHIFT, 0x28, Action::Swap(Dir::Down)),
    (CTRL | SHIFT, b'J' as u16, Action::JumpToAttention),
    (CTRL | SHIFT, b'B' as u16, Action::ToggleSidebar),
    (CTRL | SHIFT, b'U' as u16, Action::Update),
    (CTRL | SHIFT, b'K' as u16, Action::ThemePicker),
    // VK_OEM_COMMA: the comma key on every layout.
    (CTRL, 0xbc, Action::Settings),
];

/// The shortcut a key press triggers, if any. Modifiers must match
/// exactly, so AltGr (Ctrl+Alt) and Win never trigger Ctrl shortcuts.
pub fn action(k: &KeyInput) -> Option<Action> {
    if !k.down {
        return None;
    }
    let m = &k.mods;
    if m.lsuper || m.rsuper {
        return None;
    }
    let held = [
        (m.lctrl || m.rctrl, CTRL),
        (m.lshift || m.rshift, SHIFT),
        (m.lalt || m.ralt, ALT),
    ];
    let mods = held.iter().filter(|h| h.0).fold(0, |a, h| a | h.1);
    if mods == CTRL && (0x31..=0x39).contains(&k.vk) {
        return Some(Action::GoToTab((k.vk - 0x31) as u8));
    }
    DEFAULT_KEYS
        .iter()
        .find(|&&(m, vk, _)| m == mods && vk == k.vk)
        .map(|&(_, _, a)| a)
}

/// Whether a key message is an auto-repeat: lParam bit 30 says the key was
/// already down.
pub fn held_before(lparam: isize) -> bool {
    lparam as u32 & 1 << 30 != 0
}

/// Whether the auto-repeat of a held key does again what its press did,
/// when blitz took the press (`panel` when the theme picker or settings
/// panel did). Moving, scrolling and typing do. Anything that confirms,
/// toggles, opens or closes does not: a held key would answer its own
/// "press again", flicker, close pane after pane, or reach the program
/// once its press closed what took it.
pub fn repeats(k: &KeyInput, panel: bool) -> bool {
    match action(k) {
        Some(a) => matches!(
            a,
            Action::ScrollPage(_)
                | Action::CycleTab(_)
                | Action::Focus(_)
                | Action::Resize(_)
                | Action::Swap(_)
        ),
        None => panel && !matches!(k.key, Key::Enter | Key::Escape | Key::Delete),
    }
}

/// Whether blitz drops the auto-repeat of a held key: the press was blitz's
/// (`taken`), and [`repeats`] says no. A shortcut that asks to be pressed
/// again counts as taken even when blitz lost track of its press, as when
/// focus left and came back while it was held, so a held key never answers
/// its own "press again". Other shortcuts that passed their press on, as
/// Ctrl+C with nothing selected, pass their repeats on too.
pub fn drops_repeat(k: &KeyInput, taken: bool, panel: bool) -> bool {
    let confirms = matches!(
        action(k),
        Some(Action::Paste | Action::ClosePane | Action::Update)
    );
    (taken || confirms) && !repeats(k, panel)
}

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
/// `ToUnicodeEx` does without touching the dead key state, and `None` for a
/// dead key. A dead key sends nothing: what it composes comes as WM_CHAR
/// with the next key. The text of the result is stored in `text`.
pub fn msg_to_key<'a>(
    vk: u16,
    lparam: isize,
    keystate: &[u8; 256],
    layout: impl Fn(u16, u16, &[u8; 256]) -> Option<String>,
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
    // An AltGr dead key types nothing either, even though the same key
    // without Ctrl and Alt does.
    let dead = typed.is_none();
    let typed = typed.unwrap_or_default();
    let uc = typed.encode_utf16().next().unwrap_or(0);
    // Windows reports AltGr as Ctrl+Alt. When Ctrl+Alt types something
    // printable that is the key's text; otherwise the text is what the key
    // types with Ctrl and Alt let go.
    *text = if dead || !(ctrl || alt) || ctrl && alt && printable(&typed) {
        typed
    } else {
        let mut plain = *keystate;
        for vk in [0x11, 0x12, 0xa2, 0xa3, 0xa4, 0xa5] {
            plain[vk] &= !0x80;
        }
        layout(vk, scan, &plain).unwrap_or_default()
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
        _ if dead => Key::Other,
        _ => {
            let base = layout(vk, scan, &[0; 256]).unwrap_or_default();
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

/// The calling thread's keyboard layout, as the `layout` of [`msg_to_key`].
#[cfg(windows)]
pub fn system_layout(vk: u16, scan: u16, keystate: &[u8; 256]) -> Option<String> {
    use windows::Win32::UI::Input::KeyboardAndMouse::ToUnicode;
    let mut buf = [0u16; 8];
    // Flag 4 leaves the dead key state alone, so the WM_CHAR that
    // TranslateMessage makes for this key still composes. A dead key
    // returns a negative count.
    // SAFETY: `keystate` and `buf` are valid for the whole call.
    let n = unsafe { ToUnicode(vk.into(), scan.into(), Some(keystate), &mut buf, 4) };
    let n = usize::try_from(n).ok()?.min(buf.len());
    Some(String::from_utf16_lossy(&buf[..n]))
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

    /// Marks a dead key in a test layout.
    const DEAD: &str = "<dead>";

    /// A test layout from `(vk, plain, shifted, altgr)` rows. Ctrl alone
    /// turns letters and Enter into control codes, as Windows layouts do.
    fn layout(
        rows: &'static [(u16, &'static str, &'static str, &'static str)],
    ) -> impl Fn(u16, u16, &[u8; 256]) -> Option<String> {
        move |vk, _, s| {
            let held = |vk: usize| s[vk] & HELD != 0;
            let Some(&(_, plain, shifted, altgr)) = rows.iter().find(|r| r.0 == vk) else {
                return Some(String::new());
            };
            let caps = s[0x14] & 1 != 0 && plain.chars().all(char::is_alphabetic);
            let typed = match (held(0x11), held(0x12)) {
                (true, true) => altgr,
                (true, false) if (0x41..=0x5a).contains(&vk) => {
                    return Some(char::from(vk as u8 - 0x40).to_string());
                }
                (true, false) if vk == 0x0d => "\n",
                _ if held(0x10) != caps => shifted,
                _ => plain,
            };
            (typed != DEAD).then(|| typed.into())
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

    /// German: AltGr+Q is `@`, Z and Y swap places, the keypad has a comma.
    const DE: &[(u16, &str, &str, &str)] = &[
        (0x51, "q", "Q", "@"),
        (0x5a, "z", "Z", ""),
        (0x6e, ",", ",", ""),
    ];

    /// Polish (214): AltGr+Q is a backslash.
    const PL: &[(u16, &str, &str, &str)] = &[(0x51, "q", "Q", "\\")];

    /// Czech: the 1 key types `+`, and AltGr+1 is a dead `~`. Shift+´ is a
    /// dead caron without AltGr.
    const CZ: &[(u16, &str, &str, &str)] = &[
        (0x31, "+", "1", DEAD),
        (0xbb, DEAD, DEAD, ""),
        (0x45, "e", "E", "\u{20ac}"),
    ];

    fn modes(kitty: u8, w32im: bool) -> vt::InputModes {
        vt::InputModes {
            kitty,
            w32im,
            ..Default::default()
        }
    }

    /// What the terminal sends for one key message.
    fn enc(
        vk: u16,
        lparam: isize,
        s: &[u8; 256],
        rows: &'static [(u16, &'static str, &'static str, &'static str)],
        m: &vt::InputModes,
    ) -> String {
        let mut t = String::new();
        let k = msg_to_key(vk, lparam, s, layout(rows), &mut t);
        let mut out = Vec::new();
        vt::encode_key(&k, m, &mut out);
        String::from_utf8(out).unwrap()
    }

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

    #[test]
    fn keymap_shift_enter_with_kitty_flags() {
        // Num Lock stays out of the report unless all keys are escapes.
        let s = state(&[0xa0], &[0x90]);
        let enter = lp(0x1c, false, true, 1);
        assert_eq!(enc(0x0d, enter, &s, US, &modes(5, true)), "\x1b[13;2u");
    }

    #[test]
    fn keymap_shift_enter_in_win32_input_mode() {
        let m = modes(0, true);
        let shift = state(&[0xa0], &[]);
        let up = state(&[], &[]);
        let seq = [
            enc(0x10, lp(0x2a, false, true, 1), &shift, US, &m),
            enc(0x0d, lp(0x1c, false, true, 1), &shift, US, &m),
            enc(0x0d, lp(0x1c, false, false, 1), &shift, US, &m),
            enc(0x10, lp(0x2a, false, false, 1), &up, US, &m),
        ]
        .concat();
        assert_eq!(
            seq,
            "\x1b[16;42;0;1;16;1_\x1b[13;28;13;1;16;1_\x1b[13;28;13;0;16;1_\x1b[16;42;0;0;0;1_"
        );
    }

    #[test]
    fn keymap_altgr_sends_text_without_modifiers() {
        // AltGr is Left Ctrl plus Right Alt.
        let altgr = state(&[0xa2, 0xa5], &[]);
        let q = lp(0x10, false, true, 1);
        for m in [modes(0, false), modes(1, false), modes(5, false)] {
            assert_eq!(enc(0x51, q, &altgr, DE, &m), "@");
            assert_eq!(enc(0x51, q, &altgr, PL, &m), "\\");
        }
        // The console record keeps the real control state.
        assert_eq!(
            enc(0x51, q, &altgr, DE, &modes(0, true)),
            "\x1b[81;16;64;1;9;1_"
        );
        // A US layout has no AltGr, so Ctrl+Alt+Q stays a chord.
        assert_eq!(enc(0x51, q, &altgr, US, &modes(0, false)), "\x1b\x11");
        assert_eq!(enc(0x51, q, &altgr, US, &modes(1, false)), "\x1b[113;7u");
    }

    #[test]
    fn keymap_dead_keys_send_nothing() {
        let altgr = state(&[0xa2, 0xa5], &[]);
        let shift = state(&[0xa0], &[]);
        let one = lp(0x02, false, true, 1);
        for kitty in [0, 1, 5, 31] {
            let m = modes(kitty, false);
            assert_eq!(
                enc(0x31, one, &altgr, CZ, &m),
                "",
                "AltGr dead key, kitty {kitty}"
            );
            let up = lp(0x02, false, false, 1);
            assert_eq!(
                enc(0x31, up, &altgr, CZ, &m),
                "",
                "its release, kitty {kitty}"
            );
            let caron = lp(0x0d, false, true, 1);
            assert_eq!(
                enc(0xbb, caron, &shift, CZ, &m),
                "",
                "dead key, kitty {kitty}"
            );
        }
        let mut t = String::new();
        let k = msg_to_key(0x31, one, &altgr, layout(CZ), &mut t);
        assert_eq!((k.key, k.text, k.uc), (Key::Other, "", 0));
        // The console record still goes out, without a character.
        assert_eq!(
            enc(0x31, one, &altgr, CZ, &modes(0, true)),
            "\x1b[49;2;0;1;9;1_"
        );
        // The same key types as usual without AltGr, and so does a key
        // whose AltGr level is not dead.
        assert_eq!(enc(0x31, one, &[0; 256], CZ, &modes(0, false)), "+");
        let e = lp(0x12, false, true, 1);
        assert_eq!(enc(0x45, e, &altgr, CZ, &modes(0, false)), "\u{20ac}");
    }

    #[test]
    fn keymap_named_keys() {
        let mut t = String::new();
        let none = [0; 256];
        for (vk, scan, extended, want) in [
            (0x08, 0x0e, false, Key::Backspace),
            (0x09, 0x0f, false, Key::Tab),
            (0x1b, 0x01, false, Key::Escape),
            (0x21, 0x49, true, Key::PageUp),
            (0x22, 0x51, true, Key::PageDown),
            (0x23, 0x4f, true, Key::End),
            (0x25, 0x4b, true, Key::Left),
            (0x27, 0x4d, true, Key::Right),
            (0x28, 0x50, true, Key::Down),
            (0x2d, 0x52, true, Key::Insert),
            (0x2e, 0x53, true, Key::Delete),
            (0x70, 0x3b, false, Key::F(1)),
            (0x87, 0x76, false, Key::F(24)),
            (0x10, 0x2a, false, Key::Shift),
            (0xa1, 0x36, false, Key::Shift),
            (0xa2, 0x1d, false, Key::Control),
            (0x12, 0x38, false, Key::Alt),
            (0xa5, 0x38, true, Key::Alt),
            (0x5b, 0x5b, true, Key::Super),
            (0x5c, 0x5c, true, Key::Super),
        ] {
            let k = msg_to_key(vk, lp(scan, extended, true, 1), &none, layout(US), &mut t);
            assert_eq!((k.key, k.text, k.extended), (want, "", extended), "{vk:#x}");
        }
        // Ctrl+Enter types a line feed on Windows layouts; the text stays
        // what the key types without Ctrl, and uc is the control code.
        let k = msg_to_key(
            0x0d,
            lp(0x1c, false, true, 1),
            &state(&[0xa2], &[]),
            layout(US),
            &mut t,
        );
        assert_eq!((k.key, k.text, k.uc), (Key::Enter, "", 10));
    }

    #[test]
    fn keymap_numpad() {
        let mut t = String::new();
        // Num Lock on: the keypad types digits, and a comma on German.
        let num = state(&[], &[0x90]);
        let k = msg_to_key(0x67, lp(0x47, false, true, 1), &num, layout(US), &mut t);
        assert_eq!(
            (k.key, k.text, k.extended, k.us_base),
            (Key::Char('7'), "7", false, None)
        );
        assert!(k.locks.num);
        let k = msg_to_key(0x6e, lp(0x53, false, true, 1), &num, layout(DE), &mut t);
        assert_eq!((k.key, k.text), (Key::Char(','), ","));
        // Num Lock off: keypad 7 is Home without the extended bit; the Home
        // key has it.
        let k = msg_to_key(
            0x24,
            lp(0x47, false, true, 1),
            &[0; 256],
            layout(US),
            &mut t,
        );
        assert_eq!((k.key, k.extended), (Key::Home, false));
        let k = msg_to_key(0x24, lp(0x47, true, true, 1), &[0; 256], layout(US), &mut t);
        assert_eq!((k.key, k.extended), (Key::Home, true));
    }

    #[test]
    fn keymap_enhanced_bit() {
        let none = [0; 256];
        let kp_enter = lp(0x1c, true, true, 1);
        assert_eq!(
            enc(0x0d, kp_enter, &none, US, &modes(0, true)),
            "\x1b[13;28;13;1;256;1_"
        );
        // Keypad Enter has its own kitty code only when all keys are escapes.
        assert_eq!(
            enc(0x0d, kp_enter, &none, US, &modes(9, false)),
            "\x1b[57414u"
        );
        assert_eq!(enc(0x0d, kp_enter, &none, US, &modes(1, false)), "\r");
        // The arrow keys outside the keypad carry it too.
        assert_eq!(
            enc(0x26, lp(0x48, true, true, 1), &none, US, &modes(0, true)),
            "\x1b[38;72;0;1;256;1_"
        );
    }

    fn press(vk: u16, held: &[usize]) -> Option<Action> {
        let mut t = String::new();
        let k = msg_to_key(
            vk,
            lp(0, false, true, 1),
            &state(held, &[]),
            layout(US),
            &mut t,
        );
        action(&k)
    }

    #[test]
    fn keymap_default_shortcuts() {
        const LCTRL: usize = 0xa2;
        const LSHIFT: usize = 0xa0;
        const LALT: usize = 0xa4;
        assert_eq!(press(0x43, &[LCTRL]), Some(Action::Copy));
        assert_eq!(press(0x56, &[LCTRL, LSHIFT]), Some(Action::Paste));
        assert_eq!(press(0x2d, &[0xa1]), Some(Action::Paste));
        assert_eq!(press(0x21, &[LSHIFT]), Some(Action::ScrollPage(1)));
        assert_eq!(press(0x52, &[LCTRL, LSHIFT]), Some(Action::SplitRight));
        assert_eq!(press(0x44, &[0xa3, LSHIFT]), Some(Action::SplitDown));
        assert_eq!(press(0x42, &[LCTRL, LSHIFT]), Some(Action::ToggleSidebar));
        assert_eq!(press(0x09, &[LCTRL, LSHIFT]), Some(Action::CycleTab(-1)));
        assert_eq!(press(0x33, &[LCTRL]), Some(Action::GoToTab(2)));
        assert_eq!(press(0x25, &[LCTRL, LALT]), Some(Action::Focus(Dir::Left)));
        assert_eq!(press(0x26, &[LALT, LSHIFT]), Some(Action::Resize(Dir::Up)));
        assert_eq!(
            press(0x27, &[LCTRL, LALT, LSHIFT]),
            Some(Action::Swap(Dir::Right))
        );
        // Modifiers match exactly.
        assert_eq!(press(0x52, &[LCTRL]), None);
        assert_eq!(press(0x43, &[LCTRL, LALT]), None, "AltGr is not Ctrl");
        assert_eq!(press(0x43, &[LCTRL, 0x5b]), None);
        assert_eq!(
            press(0x0d, &[LSHIFT]),
            None,
            "Shift+Enter goes to the program"
        );
        assert_eq!(press(0x22, &[]), None);
        // Releases never trigger.
        let mut t = String::new();
        let up = lp(0, false, false, 1);
        let k = msg_to_key(0x43, up, &state(&[LCTRL], &[]), layout(US), &mut t);
        assert_eq!(action(&k), None);
    }

    /// The left-hand keys for a set of shortcut modifiers.
    fn held_for(m: u8) -> Vec<usize> {
        [(CTRL, 0xa2), (SHIFT, 0xa0), (ALT, 0xa4)]
            .into_iter()
            .filter(|&(bit, _)| m & bit != 0)
            .map(|(_, vk)| vk)
            .collect()
    }

    #[test]
    fn keymap_every_default_shortcut_fires() {
        for &(m, vk, a) in DEFAULT_KEYS {
            assert_eq!(press(vk, &held_for(m)), Some(a), "{m} {vk:#x}");
            // The right-hand modifier keys count the same.
            let right: Vec<usize> = held_for(m).into_iter().map(|k| k + 1).collect();
            assert_eq!(press(vk, &right), Some(a), "right-hand {m} {vk:#x}");
            // One modifier more is another chord, and Win is never one.
            for (bit, extra) in [(CTRL, 0xa2), (SHIFT, 0xa0), (ALT, 0xa4), (0, 0x5b)] {
                let mut more = held_for(m);
                if !more.contains(&extra) {
                    more.push(extra);
                    let other = (DEFAULT_KEYS.iter())
                        .find(|b| bit != 0 && b.0 == m | bit && b.1 == vk)
                        .map(|b| b.2);
                    assert_eq!(press(vk, &more), other, "{m} {vk:#x} + {extra:#x}");
                }
            }
        }
        for i in 0..9 {
            assert_eq!(press(0x31 + i, &[0xa2]), Some(Action::GoToTab(i as u8)));
        }
        assert_eq!(press(0x30, &[0xa2]), None, "Ctrl+0");
        assert_eq!(press(0x31, &[0xa2, 0xa0]), None, "Ctrl+Shift+1");
        assert_eq!(press(0x31, &[0xa2, 0xa4]), None, "AltGr+1");
        assert_eq!(press(0x61, &[0xa2]), None, "Ctrl+keypad 1 is not tab 1");
    }

    #[test]
    fn keymap_default_shortcuts_are_unique_and_unshadowed() {
        for (i, a) in DEFAULT_KEYS.iter().enumerate() {
            for b in &DEFAULT_KEYS[i + 1..] {
                assert!((a.0, a.1) != (b.0, b.1), "{a:?} and {b:?}");
            }
            let tab = a.0 == CTRL && (0x31..=0x39).contains(&a.1);
            assert!(!tab, "{a:?} is a Go to tab key");
        }
    }

    #[test]
    fn readme_lists_every_default_shortcut() {
        let readme = include_str!("../../../README.md");
        for &(m, vk, _) in DEFAULT_KEYS {
            let key = match vk {
                0x09 => "Tab".to_string(),
                0x21 => "PgUp".into(),
                0x22 => "PgDn".into(),
                0x2d => "Insert".into(),
                0x25..=0x28 => "Arrows".into(),
                0xbc => ",".into(),
                _ => char::from(vk as u8).to_string(),
            };
            let mut chord = String::new();
            for (bit, name) in [(CTRL, "Ctrl+"), (ALT, "Alt+"), (SHIFT, "Shift+")] {
                if m & bit != 0 {
                    chord.push_str(name);
                }
            }
            chord.push_str(&key);
            // Followed by something other than more of a key name, so
            // Ctrl+Shift+T does not count as Ctrl+Shift+Tab.
            let listed = readme.match_indices(&chord).any(|(i, _)| {
                (readme[i + chord.len()..].chars().next())
                    .is_none_or(|c| !c.is_ascii_alphanumeric())
            });
            assert!(listed, "README.md does not list {chord}");
        }
    }

    #[test]
    fn keymap_auto_repeat() {
        assert!(!held_before(lp(0x16, false, true, 1)), "a first press");
        assert!(held_before(lp(0x16, false, true, 1) | 1 << 30), "a repeat");
        let repeats_with = |vk: u16, held: &[usize], panel: bool| {
            let mut t = String::new();
            let again = lp(0, false, true, 1) | 1 << 30;
            let k = msg_to_key(vk, again, &state(held, &[]), layout(US), &mut t);
            repeats(&k, panel)
        };
        const CS: &[usize] = &[0xa2, 0xa0];
        // Holding a key must not answer its own "press again", flicker a
        // toggle, or open and close things over and over.
        for (vk, held) in [
            (0x55, CS),      // Update
            (0x57, CS),      // ClosePane
            (0x56, &[0xa2]), // Paste
            (0x43, &[0xa2]), // Copy
            (0x4b, CS),      // ThemePicker
            (0xbc, &[0xa2]), // Settings
            (0x42, CS),      // ToggleSidebar
            (0x54, CS),      // NewTab
            (0x52, CS),      // SplitRight
            (0x4a, CS),      // JumpToAttention
            (0x33, &[0xa2]), // GoToTab
        ] {
            for panel in [false, true] {
                assert!(!repeats_with(vk, held, panel), "{vk:#x} panel {panel}");
            }
        }
        // Moving and scrolling go on while the key is held.
        for (vk, held) in [
            (0x21, &[0xa0][..]),         // ScrollPage
            (0x09, &[0xa2]),             // CycleTab
            (0x25, &[0xa2, 0xa4]),       // Focus
            (0x26, &[0xa4, 0xa0]),       // Resize
            (0x27, &[0xa2, 0xa4, 0xa0]), // Swap
        ] {
            assert!(repeats_with(vk, held, false), "{vk:#x}");
        }
        // In a panel the arrows, Backspace and typing repeat; Enter, Esc
        // and Delete do not.
        for vk in [0x26, 0x28, 0x25, 0x27, 0x21, 0x08, 0x41] {
            assert!(repeats_with(vk, &[], true), "{vk:#x}");
        }
        for vk in [0x0d, 0x1b, 0x2e] {
            assert!(!repeats_with(vk, &[], true), "{vk:#x}");
        }
        // Outside a panel, a key blitz took that is no shortcut, as Enter
        // closing an exited pane, does not go on to the next pane.
        assert!(!repeats_with(0x0d, &[], false));

        let drops = |vk: u16, held: &[usize], taken: bool| {
            let mut t = String::new();
            let again = lp(0, false, true, 1) | 1 << 30;
            let k = msg_to_key(vk, again, &state(held, &[]), layout(US), &mut t);
            drops_repeat(&k, taken, false)
        };
        // A held shortcut that asks to be pressed again is dropped even once
        // blitz lost track of its press, as when focus left and came back
        // while it was held.
        for (vk, held) in [(0x56, &[0xa2][..]), (0x57, CS), (0x55, CS), (0x2d, &[0xa1])] {
            assert!(drops(vk, held, false), "{vk:#x}");
        }
        // Copy with nothing selected passes its press on, so the program
        // gets every ^C of a held Ctrl+C; taken, it copies once.
        for (vk, held) in [(0x43, &[0xa2][..]), (0x2d, &[0xa2])] {
            assert!(!drops(vk, held, false), "{vk:#x}");
            assert!(drops(vk, held, true), "{vk:#x} taken");
        }
        assert!(drops(0x0d, &[], true), "Enter that closed an exited pane");
        // Typing, and moving that repeats, go on.
        assert!(!drops(0x41, &[], false));
        assert!(!drops(0x0d, &[], false));
        assert!(!drops(0x09, &[0xa2], true), "CycleTab");
    }

    /// Enter and Space type the same on every layout.
    #[cfg(windows)]
    #[test]
    fn keymap_system_layout() {
        let mut t = String::new();
        let shift = state(&[0xa0], &[]);
        let k = msg_to_key(
            0x0d,
            lp(0x1c, false, true, 1),
            &shift,
            system_layout,
            &mut t,
        );
        assert_eq!((k.key, k.uc, k.text), (Key::Enter, 13, ""));
        let k = msg_to_key(
            0x20,
            lp(0x39, false, true, 1),
            &shift,
            system_layout,
            &mut t,
        );
        assert_eq!((k.key, k.uc, k.text), (Key::Char(' '), 32, " "));
    }
}
