use vt::{InputModes, Key, KeyInput, Locks, Mods, encode_key};

/// Modifiers from letters: s shift, c ctrl, a alt, g right alt (AltGr's
/// half), w super.
fn mods(s: &str) -> Mods {
    let mut m = Mods::default();
    for ch in s.chars() {
        match ch {
            's' => m.lshift = true,
            'c' => m.lctrl = true,
            'a' => m.lalt = true,
            'g' => m.ralt = true,
            'w' => m.lsuper = true,
            _ => panic!("unknown modifier {ch}"),
        }
    }
    m
}

/// A key-down with the fields every encoding looks at.
fn key<'a>(vk: u16, scan: u16, uc: u16, m: &str, key: Key, text: &'a str) -> KeyInput<'a> {
    KeyInput {
        vk,
        scan,
        extended: false,
        down: true,
        repeat: 1,
        mods: mods(m),
        locks: Locks::default(),
        text,
        uc,
        cs: 0,
        key,
        us_base: match key {
            Key::Char(c) if c.is_ascii() => Some(c),
            _ => None,
        },
    }
}

/// A key-down where only the key, modifiers and text matter.
fn k<'a>(m: &str, key: Key, text: &'a str) -> KeyInput<'a> {
    self::key(0, 0, 0, m, key, text)
}

fn enc(k: &KeyInput, m: &InputModes) -> String {
    let mut out = Vec::new();
    encode_key(k, m, &mut out);
    String::from_utf8(out).unwrap()
}

const LEGACY: InputModes = InputModes {
    decckm: false,
    deckpam: false,
    w32im: false,
    kitty: 0,
    bracketed: false,
    focus: false,
    mouse: vt::MouseMode::Off,
    mouse_sgr: false,
    alt_screen: false,
};

#[test]
fn legacy_keys() {
    let cases = [
        (k("", Key::Char('a'), "a"), "a"),
        (k("s", Key::Char('a'), "A"), "A"),
        (k("c", Key::Char('a'), "a"), "\x01"),
        (k("cs", Key::Char('a'), "A"), "\x01"),
        (k("a", Key::Char('a'), "a"), "\x1ba"),
        (k("ca", Key::Char('a'), "a"), "\x1b\x01"),
        (k("c", Key::Char('j'), "j"), "\n"),
        (k("c", Key::Char(' '), " "), "\0"),
        (k("c", Key::Char('['), "["), "\x1b"),
        (k("cs", Key::Char('-'), "_"), "\x1f"),
        (k("cs", Key::Char('/'), "?"), "\x7f"),
        (k("c", Key::Char('1'), "1"), "1"),
        (k("", Key::Char('é'), "é"), "é"),
        (k("", Key::Enter, ""), "\r"),
        (k("s", Key::Enter, ""), "\r"),
        (k("c", Key::Enter, ""), "\r"),
        (k("a", Key::Enter, ""), "\x1b\r"),
        (k("", Key::Tab, ""), "\t"),
        (k("s", Key::Tab, ""), "\x1b[Z"),
        (k("", Key::Backspace, ""), "\x7f"),
        (k("c", Key::Backspace, ""), "\x08"),
        (k("a", Key::Backspace, ""), "\x1b\x7f"),
        (k("", Key::Escape, ""), "\x1b"),
        (k("a", Key::Escape, ""), "\x1b\x1b"),
        (k("", Key::Up, ""), "\x1b[A"),
        (k("c", Key::Left, ""), "\x1b[1;5D"),
        (k("sa", Key::End, ""), "\x1b[1;4F"),
        (k("", Key::Delete, ""), "\x1b[3~"),
        (k("c", Key::PageUp, ""), "\x1b[5;5~"),
        (k("", Key::F(1), ""), "\x1bOP"),
        (k("s", Key::F(3), ""), "\x1b[1;2R"),
        (k("", Key::F(5), ""), "\x1b[15~"),
        (k("c", Key::F(12), ""), "\x1b[24;5~"),
        (k("", Key::F(13), ""), ""),
        (k("s", Key::Shift, ""), ""),
        (k("", Key::Char('q'), ""), ""),
    ];
    for (input, want) in cases {
        assert_eq!(enc(&input, &LEGACY), want, "{input:?}");
    }
}

#[test]
fn legacy_keys_ignore_key_up() {
    let mut up = k("", Key::Char('a'), "a");
    up.down = false;
    assert_eq!(enc(&up, &LEGACY), "");
}

#[test]
fn legacy_keys_non_us_layout_use_us_base_for_ctrl() {
    // Ctrl+С on a Russian layout is Ctrl+C.
    let mut input = k("c", Key::Char('с'), "с");
    input.us_base = Some('c');
    assert_eq!(enc(&input, &LEGACY), "\x03");
}

#[test]
fn legacy_keys_decckm_and_deckpam() {
    let ckm = InputModes {
        decckm: true,
        ..LEGACY
    };
    assert_eq!(enc(&k("", Key::Up, ""), &ckm), "\x1bOA");
    assert_eq!(enc(&k("", Key::Home, ""), &ckm), "\x1bOH");
    assert_eq!(enc(&k("c", Key::Up, ""), &ckm), "\x1b[1;5A");

    let kpam = InputModes {
        deckpam: true,
        ..LEGACY
    };
    let five = key(0x65, 0x4c, 0x35, "", Key::Char('5'), "5");
    assert_eq!(enc(&five, &LEGACY), "5");
    assert_eq!(enc(&five, &kpam), "\x1bOu");
    let plus = key(0x6b, 0x4e, 0x2b, "", Key::Char('+'), "+");
    assert_eq!(enc(&plus, &kpam), "\x1bOk");
    let mut kp_enter = key(0x0d, 0x1c, 13, "", Key::Enter, "");
    kp_enter.extended = true;
    assert_eq!(enc(&kp_enter, &LEGACY), "\r");
    assert_eq!(enc(&kp_enter, &kpam), "\x1bOM");
    // The main Enter key is not on the keypad.
    let enter = key(0x0d, 0x1c, 13, "", Key::Enter, "");
    assert_eq!(enc(&enter, &kpam), "\r");
}

#[test]
fn legacy_keys_ime_owned_key_sends_nothing() {
    let input = key(0xe5, 0x1e, 0, "", Key::Char('a'), "a");
    assert_eq!(enc(&input, &LEGACY), "");
}
