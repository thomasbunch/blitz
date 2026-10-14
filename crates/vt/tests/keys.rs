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

/// A key-down with the fields every encoding looks at. Like the keymap,
/// it gives keypad keys no US base character.
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
            Key::Char(c) if c.is_ascii() && !(0x60..=0x6f).contains(&vk) => Some(c),
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
    alt_scroll: false,
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
fn legacy_and_kitty_every_named_key() {
    for (key, legacy, kkp) in [
        (Key::Up, "\x1b[A", "\x1b[A"),
        (Key::Down, "\x1b[B", "\x1b[B"),
        (Key::Right, "\x1b[C", "\x1b[C"),
        (Key::Left, "\x1b[D", "\x1b[D"),
        (Key::Home, "\x1b[H", "\x1b[H"),
        (Key::End, "\x1b[F", "\x1b[F"),
        (Key::Insert, "\x1b[2~", "\x1b[2~"),
        (Key::Delete, "\x1b[3~", "\x1b[3~"),
        (Key::PageUp, "\x1b[5~", "\x1b[5~"),
        (Key::PageDown, "\x1b[6~", "\x1b[6~"),
        (Key::F(1), "\x1bOP", "\x1b[P"),
        (Key::F(2), "\x1bOQ", "\x1b[Q"),
        (Key::F(3), "\x1bOR", "\x1b[13~"),
        (Key::F(4), "\x1bOS", "\x1b[S"),
        (Key::F(5), "\x1b[15~", "\x1b[15~"),
        (Key::F(6), "\x1b[17~", "\x1b[17~"),
        (Key::F(7), "\x1b[18~", "\x1b[18~"),
        (Key::F(8), "\x1b[19~", "\x1b[19~"),
        (Key::F(9), "\x1b[20~", "\x1b[20~"),
        (Key::F(10), "\x1b[21~", "\x1b[21~"),
        (Key::F(11), "\x1b[23~", "\x1b[23~"),
        (Key::F(12), "\x1b[24~", "\x1b[24~"),
        (Key::F(13), "", "\x1b[57376u"),
        (Key::F(24), "", "\x1b[57387u"),
        (Key::F(35), "", "\x1b[57398u"),
        (Key::F(36), "", ""),
    ] {
        assert_eq!(enc(&k("", key, ""), &LEGACY), legacy, "legacy {key:?}");
        assert_eq!(enc(&k("", key, ""), &kitty(1)), kkp, "kitty {key:?}");
    }
    for (m, key, want) in [
        ("csa", Key::Down, "\x1b[1;8B"),
        ("s", Key::Right, "\x1b[1;2C"),
        ("c", Key::Insert, "\x1b[2;5~"),
        ("a", Key::PageDown, "\x1b[6;3~"),
        ("a", Key::F(2), "\x1b[1;3Q"),
        ("c", Key::F(4), "\x1b[1;5S"),
        ("s", Key::F(6), "\x1b[17;2~"),
        ("cs", Key::F(11), "\x1b[23;6~"),
    ] {
        assert_eq!(enc(&k(m, key, ""), &LEGACY), want, "{m} {key:?}");
        assert_eq!(enc(&k(m, key, ""), &kitty(1)), want, "kitty {m} {key:?}");
    }
}

/// Ctrl with a character follows Xlib, which xterm uses: `@` to `~` drop
/// to C0, and the digits 2 to 8 stand in for the ones that need Shift.
#[test]
fn legacy_keys_ctrl_digits_and_punctuation() {
    for (m, c, text, want) in [
        ("c", '2', "2", "\0"),
        ("c", '3', "3", "\x1b"),
        ("c", '4', "4", "\x1c"),
        ("c", '5', "5", "\x1d"),
        ("c", '6', "6", "\x1e"),
        ("c", '7', "7", "\x1f"),
        ("c", '8', "8", "\x7f"),
        ("c", '9', "9", "9"),
        ("c", '0', "0", "0"),
        ("c", '\\', "\\", "\x1c"),
        ("c", ']', "]", "\x1d"),
        ("c", '`', "`", "\0"),
        ("cs", '`', "~", "\x1e"),
        ("cs", '6', "^", "\x1e"),
        ("cs", '2', "@", "\0"),
        ("cs", '[', "{", "\x1b"),
        ("cs", '\\', "|", "\x1c"),
        ("cs", ']', "}", "\x1d"),
        ("c", '=', "=", "="),
        ("c", ';', ";", ";"),
        ("c", '\'', "'", "'"),
        ("c", ',', ",", ","),
        ("c", '.', ".", "."),
        ("ca", '\\', "\\", "\x1b\x1c"),
    ] {
        assert_eq!(enc(&k(m, Key::Char(c), text), &LEGACY), want, "{m}+{c}");
    }
}

#[test]
fn legacy_keys_modifier_combinations() {
    for (m, key, text, want) in [
        ("sa", Key::Char('a'), "A", "\x1bA"),
        ("a", Key::Char('é'), "é", "\x1bé"),
        ("csa", Key::Char('a'), "A", "\x1b\x01"),
        ("w", Key::Char('a'), "a", "a"),
        ("s", Key::Backspace, "", "\x7f"),
        ("ca", Key::Backspace, "", "\x1b\x08"),
        ("c", Key::Tab, "", "\t"),
        ("a", Key::Tab, "", "\x1b\t"),
        ("sa", Key::Tab, "", "\x1b\x1b[Z"),
        ("s", Key::Escape, "", "\x1b"),
        ("w", Key::Up, "", "\x1b[1;9A"),
        ("cw", Key::Home, "", "\x1b[1;13H"),
    ] {
        assert_eq!(enc(&k(m, key, text), &LEGACY), want, "{m} {key:?}");
    }
}

/// On a layout whose key has no C0 byte of its own, Ctrl falls back to
/// the key in the same place on a US layout, as it does for Cyrillic.
#[test]
fn legacy_keys_ctrl_on_latin_layouts_uses_the_us_position() {
    // AZERTY: the US 3 key types `"`, the US 2 key types `é`.
    let mut quote = k("c", Key::Char('"'), "\"");
    quote.us_base = Some('3');
    assert_eq!(enc(&quote, &LEGACY), "\x1b");
    let mut e = k("c", Key::Char('é'), "é");
    e.us_base = Some('2');
    assert_eq!(enc(&e, &LEGACY), "\0");
    // A key with no US counterpart sends its text.
    e.us_base = None;
    assert_eq!(enc(&e, &LEGACY), "é");
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
    // Windows sends the operator keys whatever Num Lock says.
    for (vk, scan, c, f) in [
        (0x6a, 0x37, '*', 'j'),
        (0x6b, 0x4e, '+', 'k'),
        (0x6c, 0x53, ',', 'l'),
        (0x6d, 0x4a, '-', 'm'),
        (0x6f, 0x35, '/', 'o'),
    ] {
        let s = c.to_string();
        let mut input = key(vk, scan, c as u16, "", Key::Char(c), &s);
        input.extended = vk == 0x6f;
        assert_eq!(enc(&input, &LEGACY), s);
        assert_eq!(enc(&input, &kpam), format!("\x1bO{f}"), "{c}");
        input.locks.num = true;
        assert_eq!(enc(&input, &kpam), s, "{c} with Num Lock on");
    }
    let mut kp_enter = key(0x0d, 0x1c, 13, "", Key::Enter, "");
    kp_enter.extended = true;
    assert_eq!(enc(&kp_enter, &LEGACY), "\r");
    assert_eq!(enc(&kp_enter, &kpam), "\x1bOM");
    // The main Enter key is not on the keypad.
    let enter = key(0x0d, 0x1c, 13, "", Key::Enter, "");
    assert_eq!(enc(&enter, &kpam), "\r");

    // With Num Lock on the digit keys type their digits.
    let mut five = key(0x65, 0x4c, 0x35, "", Key::Char('5'), "5");
    five.locks.num = true;
    let mut dot = key(0x6e, 0x53, 0x2e, "", Key::Char('.'), ".");
    dot.locks.num = true;
    kp_enter.locks.num = true;
    for (input, want) in [(five, "5"), (dot, "."), (kp_enter, "\r")] {
        assert_eq!(enc(&input, &kpam), want, "{input:?}");
    }
    // With it off they arrive as the navigation keys printed under the
    // digits, not extended, and keypad 5 as VK_CLEAR: Begin.
    let home = key(0x24, 0x47, 0, "", Key::Home, "");
    let del = key(0x2e, 0x53, 0, "", Key::Delete, "");
    let begin = key(0x0c, 0x4c, 0, "", Key::Other, "");
    for m in [LEGACY, kpam] {
        assert_eq!(enc(&home, &m), "\x1b[H");
        assert_eq!(enc(&del, &m), "\x1b[3~");
        assert_eq!(enc(&begin, &m), "\x1b[E");
    }
    assert_eq!(enc(&begin, &ckm), "\x1bOE");
    let mut ctrl_begin = begin;
    ctrl_begin.mods = mods("c");
    assert_eq!(enc(&ctrl_begin, &LEGACY), "\x1b[1;5E");
}

#[test]
fn ime_owned_key_sends_nothing_in_any_mode() {
    let input = key(0xe5, 0x1e, 0, "", Key::Char('a'), "a");
    for m in [
        LEGACY,
        W32IM,
        kitty(1),
        kitty(31),
        InputModes { kitty: 31, ..W32IM },
    ] {
        assert_eq!(enc(&input, &m), "", "{m:?}");
        assert_eq!(enc(&up(input), &m), "", "release {m:?}");
    }
}

fn kitty(flags: u8) -> InputModes {
    InputModes {
        kitty: flags,
        ..LEGACY
    }
}

fn up(mut input: KeyInput) -> KeyInput {
    input.down = false;
    input
}

#[test]
fn kitty_keys_disambiguate() {
    let m = kitty(1);
    let cases = [
        (k("", Key::Char('a'), "a"), "a"),
        (k("s", Key::Char('a'), "A"), "A"),
        (k("s", Key::Char('1'), "!"), "!"),
        (k("c", Key::Char('c'), "c"), "\x1b[99;5u"),
        (k("c", Key::Char('j'), "j"), "\x1b[106;5u"),
        (k("a", Key::Char('v'), "v"), "\x1b[118;3u"),
        (k("cs", Key::Char('-'), "_"), "\x1b[45;6u"),
        (k("w", Key::Char('a'), "a"), "\x1b[97;9u"),
        (k("", Key::Enter, ""), "\r"),
        (k("s", Key::Enter, ""), "\x1b[13;2u"),
        (k("c", Key::Enter, ""), "\x1b[13;5u"),
        (k("a", Key::Enter, ""), "\x1b[13;3u"),
        (k("", Key::Tab, ""), "\t"),
        (k("s", Key::Tab, ""), "\x1b[9;2u"),
        (k("", Key::Backspace, ""), "\x7f"),
        (k("c", Key::Backspace, ""), "\x1b[127;5u"),
        (k("", Key::Escape, ""), "\x1b[27u"),
        (k("s", Key::Escape, ""), "\x1b[27;2u"),
        (k("", Key::Up, ""), "\x1b[A"),
        (k("c", Key::Up, ""), "\x1b[1;5A"),
        (k("", Key::Delete, ""), "\x1b[3~"),
        (k("", Key::F(1), ""), "\x1b[P"),
        (k("", Key::F(3), ""), "\x1b[13~"),
        (k("c", Key::F(3), ""), "\x1b[13;5~"),
        (k("", Key::F(5), ""), "\x1b[15~"),
        (k("", Key::F(13), ""), "\x1b[57376u"),
        (k("s", Key::Shift, ""), ""),
        (k("", Key::Char('q'), ""), ""),
    ];
    for (input, want) in cases {
        assert_eq!(enc(&input, &m), want, "{input:?}");
    }
    // Cursor keys keep the CSI form under DECCKM, as kitty does.
    let ckm = InputModes { decckm: true, ..m };
    assert_eq!(enc(&k("", Key::Up, ""), &ckm), "\x1b[A");
}

#[test]
fn kitty_keys_alternates() {
    let m = kitty(5);
    assert_eq!(enc(&k("s", Key::Char('a'), "A"), &m), "A");
    assert_eq!(enc(&k("cs", Key::Char('-'), "_"), &m), "\x1b[45:95;6u");
    assert_eq!(enc(&k("cs", Key::Char('a'), "A"), &m), "\x1b[97:65;6u");
    // Ctrl+С and Ctrl+Shift+С on a Russian layout carry the US key.
    let mut ru = k("c", Key::Char('с'), "с");
    ru.us_base = Some('c');
    assert_eq!(enc(&ru, &m), "\x1b[1089::99;5u");
    ru.mods = mods("cs");
    ru.text = "С";
    assert_eq!(enc(&ru, &m), "\x1b[1089:1057:99;6u");
}

#[test]
fn kitty_keys_all_as_escapes() {
    let m = kitty(8);
    assert_eq!(enc(&k("", Key::Char('a'), "a"), &m), "\x1b[97u");
    assert_eq!(enc(&k("s", Key::Char('a'), "A"), &m), "\x1b[97;2u");
    assert_eq!(enc(&k("", Key::Enter, ""), &m), "\x1b[13u");
    assert_eq!(enc(&k("", Key::Backspace, ""), &m), "\x1b[127u");
    assert_eq!(
        enc(&key(0x10, 0x2a, 0, "s", Key::Shift, ""), &m),
        "\x1b[57441;2u"
    );
    assert_eq!(
        enc(&key(0x10, 0x36, 0, "s", Key::Shift, ""), &m),
        "\x1b[57447;2u"
    );
    let mut rctrl = key(0x11, 0x1d, 0, "c", Key::Control, "");
    rctrl.extended = true;
    assert_eq!(enc(&rctrl, &m), "\x1b[57448;5u");
    assert_eq!(
        enc(&key(0x14, 0x3a, 0, "", Key::Other, ""), &m),
        "\x1b[57358u"
    );
    let five = key(0x65, 0x4c, 0x35, "", Key::Char('5'), "5");
    assert_eq!(enc(&five, &m), "\x1b[57404u");
    assert_eq!(enc(&five, &kitty(1)), "5");
    let mut kp_enter = key(0x0d, 0x1c, 13, "", Key::Enter, "");
    kp_enter.extended = true;
    assert_eq!(enc(&kp_enter, &m), "\x1b[57414u");
    assert_eq!(enc(&kp_enter, &kitty(1)), "\r");

    // Lock states are reported only here.
    let mut caps = k("", Key::Char('a'), "A");
    caps.locks.caps = true;
    assert_eq!(enc(&caps, &m), "\x1b[97;65u");
    let mut num = k("c", Key::Char('a'), "a");
    num.locks.num = true;
    assert_eq!(enc(&num, &m), "\x1b[97;133u");
    assert_eq!(enc(&num, &kitty(5)), "\x1b[97;5u");

    assert_eq!(
        enc(&k("s", Key::Char('a'), "A"), &kitty(12)),
        "\x1b[97:65;2u"
    );
}

#[test]
fn kitty_keys_associated_text() {
    let m = kitty(8 | 16);
    assert_eq!(enc(&k("", Key::Char('a'), "a"), &m), "\x1b[97;1;97u");
    assert_eq!(enc(&k("s", Key::Char('a'), "A"), &m), "\x1b[97;2;65u");
    // Text needs all keys as escape codes, and is never on a release.
    assert_eq!(
        enc(&k("c", Key::Char('a'), "a"), &kitty(1 | 16)),
        "\x1b[97;5u"
    );
    assert_eq!(
        enc(&up(k("", Key::Char('a'), "a")), &kitty(8 | 16 | 2)),
        "\x1b[97;1:3u"
    );
    // Only keys that type something carry it: Ctrl, Alt and Super chords
    // type nothing, AltGr types its character.
    assert_eq!(enc(&k("c", Key::Char('a'), "a"), &m), "\x1b[97;5u");
    assert_eq!(enc(&k("a", Key::Char('a'), "a"), &m), "\x1b[97;3u");
    assert_eq!(enc(&k("w", Key::Char('a'), "a"), &m), "\x1b[97;9u");
    assert_eq!(enc(&k("cs", Key::Char('a'), "A"), &m), "\x1b[97;6u");
    let at = key(0x51, 16, 0x40, "cg", Key::Char('q'), "@");
    assert_eq!(enc(&at, &m), "\x1b[113;1;64u");
    // Every code point of the text, with the alternates when asked.
    assert_eq!(
        enc(&k("", Key::Char('e'), "e\u{301}"), &m),
        "\x1b[101;1;101:769u"
    );
    assert_eq!(
        enc(&k("s", Key::Char('a'), "A"), &kitty(4 | 8 | 16)),
        "\x1b[97:65;2;65u"
    );
    let mut ru = k("s", Key::Char('с'), "С");
    ru.us_base = Some('c');
    assert_eq!(enc(&ru, &kitty(4 | 8 | 16)), "\x1b[1089:1057:99;2;1057u");
}

#[test]
fn kitty_keys_event_types() {
    let m = kitty(1 | 2);
    assert_eq!(enc(&up(k("", Key::Escape, "")), &m), "\x1b[27;1:3u");
    assert_eq!(enc(&up(k("c", Key::Char('a'), "a")), &m), "\x1b[97;5:3u");
    assert_eq!(enc(&up(k("", Key::Up, "")), &m), "\x1b[1;1:3A");
    assert_eq!(enc(&up(k("", Key::Char('a'), "a")), &m), "");
    assert_eq!(enc(&up(k("", Key::Enter, "")), &m), "");
    assert_eq!(enc(&up(k("s", Key::Enter, "")), &m), "");
    assert_eq!(enc(&up(k("", Key::Escape, "")), &kitty(1)), "");
    let all = kitty(1 | 2 | 8);
    assert_eq!(enc(&up(k("", Key::Enter, "")), &all), "\x1b[13;1:3u");
    assert_eq!(enc(&up(k("", Key::Char('a'), "a")), &all), "\x1b[97;1:3u");
}

#[test]
fn kitty_keys_without_disambiguate_stay_legacy() {
    assert_eq!(enc(&k("c", Key::Char('c'), "c"), &kitty(4)), "\x03");
    assert_eq!(enc(&k("", Key::Escape, ""), &kitty(2)), "\x1b");
    // And with win32-input-mode on they stay console records.
    let a = key(0x41, 30, 97, "", Key::Char('a'), "a");
    let both = InputModes {
        kitty: 2 | 4,
        ..W32IM
    };
    assert_eq!(enc(&a, &both), "\x1b[65;30;97;1;0;1_");
}

#[test]
fn kitty_keys_releases_and_repeats() {
    let all = kitty(1 | 2 | 8);
    assert_eq!(enc(&up(k("s", Key::Enter, "")), &all), "\x1b[13;2:3u");
    assert_eq!(enc(&up(k("c", Key::Backspace, "")), &all), "\x1b[127;5:3u");
    assert_eq!(enc(&up(k("", Key::F(5), "")), &all), "\x1b[15;1:3~");
    // KeyInput carries no repeat flag, so a repeat goes out as a press.
    let mut held = k("c", Key::Char('a'), "a");
    held.repeat = 3;
    assert_eq!(enc(&held, &kitty(1 | 2)), "\x1b[97;5u");
}

#[test]
fn kitty_keys_modifier_and_lock_keys() {
    let m = kitty(8);
    let mut ralt = key(0x12, 0x38, 0, "g", Key::Alt, "");
    ralt.extended = true;
    for (input, want) in [
        (key(0x11, 0x1d, 0, "c", Key::Control, ""), "\x1b[57442;5u"),
        (key(0xa2, 0x1d, 0, "c", Key::Control, ""), "\x1b[57442;5u"),
        (key(0x12, 0x38, 0, "a", Key::Alt, ""), "\x1b[57443;3u"),
        (ralt, "\x1b[57449;3u"),
        (key(0x5b, 0x5b, 0, "w", Key::Super, ""), "\x1b[57444;9u"),
        (key(0x5c, 0x5c, 0, "w", Key::Super, ""), "\x1b[57450;9u"),
        (key(0xa1, 0x36, 0, "s", Key::Shift, ""), "\x1b[57447;2u"),
        (key(0x91, 0x46, 0, "", Key::Other, ""), "\x1b[57359u"),
        (key(0x90, 0x45, 0, "", Key::Other, ""), "\x1b[57360u"),
        (key(0x5d, 0x5d, 0, "", Key::Other, ""), ""),
    ] {
        assert_eq!(enc(&input, &m), want, "{input:?}");
        // Without all keys as escape codes they send nothing.
        assert_eq!(enc(&input, &kitty(1)), "", "{input:?}");
    }
}

#[test]
fn kitty_keys_keypad_codes() {
    let m = kitty(8);
    for (vk, code) in [
        (0x60, 57399),
        (0x69, 57408),
        (0x6e, 57409),
        (0x6f, 57410),
        (0x6a, 57411),
        (0x6d, 57412),
        (0x6b, 57413),
        (0x6c, 57416),
    ] {
        let input = key(vk, 0, 0, "", Key::Char('x'), "x");
        assert_eq!(enc(&input, &m), format!("\x1b[{code}u"), "{vk:#x}");
    }
    // Num Lock off: the keypad's navigation keys are not extended, the
    // main ones are. Keypad 5 is Begin, which keeps its letter form.
    for (vk, scan, k, code) in [
        (0x21, 0x49, Key::PageUp, 57421),
        (0x22, 0x51, Key::PageDown, 57422),
        (0x23, 0x4f, Key::End, 57424),
        (0x24, 0x47, Key::Home, 57423),
        (0x25, 0x4b, Key::Left, 57417),
        (0x26, 0x48, Key::Up, 57419),
        (0x27, 0x4d, Key::Right, 57418),
        (0x28, 0x50, Key::Down, 57420),
        (0x2d, 0x52, Key::Insert, 57425),
        (0x2e, 0x53, Key::Delete, 57426),
    ] {
        let mut input = key(vk, scan, 0, "", k, "");
        assert_eq!(enc(&input, &m), format!("\x1b[{code}u"), "{k:?}");
        let plain = enc(&input, &kitty(1));
        input.extended = true;
        assert_eq!(enc(&input, &kitty(1)), plain, "{k:?} like the main key");
        assert_ne!(enc(&input, &m), format!("\x1b[{code}u"), "main {k:?}");
    }
    let begin = key(0x0c, 0x4c, 0, "", Key::Other, "");
    assert_eq!(enc(&begin, &m), "\x1b[E");
    assert_eq!(enc(&begin, &kitty(1)), "\x1b[E");
    let mut ctrl_begin = begin;
    ctrl_begin.mods = mods("c");
    assert_eq!(enc(&ctrl_begin, &kitty(1)), "\x1b[1;5E");
    // Keypad keys carry no US base key for the alternates.
    let five = key(0x65, 0x4c, 0x35, "", Key::Char('5'), "5");
    assert_eq!(enc(&five, &kitty(4 | 8)), "\x1b[57404u");
}

const W32IM: InputModes = InputModes {
    w32im: true,
    ..LEGACY
};

/// The keys Claude Code relies on, on a US layout: legacy, kitty flags 5
/// (what Claude Code pushes) and the win32-input-mode key-down record.
#[test]
fn keys_claude_code_relies_on() {
    let mut arrow = key(0x26, 72, 0, "", Key::Up, "");
    arrow.extended = true;
    let cases = [
        (
            key(0x0d, 28, 13, "s", Key::Enter, ""),
            "\r",
            "\x1b[13;2u",
            "\x1b[13;28;13;1;16;1_",
        ),
        (
            key(0x0d, 28, 10, "c", Key::Enter, ""),
            "\r",
            "\x1b[13;5u",
            "\x1b[13;28;10;1;8;1_",
        ),
        (
            key(0x0d, 28, 13, "a", Key::Enter, ""),
            "\x1b\r",
            "\x1b[13;3u",
            "\x1b[13;28;13;1;2;1_",
        ),
        (
            key(0x1b, 1, 27, "", Key::Escape, ""),
            "\x1b",
            "\x1b[27u",
            "\x1b[27;1;27;1;0;1_",
        ),
        (
            key(0x56, 47, 118, "a", Key::Char('v'), "v"),
            "\x1bv",
            "\x1b[118;3u",
            "\x1b[86;47;118;1;2;1_",
        ),
        (
            key(0x50, 25, 112, "a", Key::Char('p'), "p"),
            "\x1bp",
            "\x1b[112;3u",
            "\x1b[80;25;112;1;2;1_",
        ),
        (
            key(0x54, 20, 116, "a", Key::Char('t'), "t"),
            "\x1bt",
            "\x1b[116;3u",
            "\x1b[84;20;116;1;2;1_",
        ),
        (
            key(0x4f, 24, 111, "a", Key::Char('o'), "o"),
            "\x1bo",
            "\x1b[111;3u",
            "\x1b[79;24;111;1;2;1_",
        ),
        (
            key(0x43, 46, 3, "c", Key::Char('c'), "c"),
            "\x03",
            "\x1b[99;5u",
            "\x1b[67;46;3;1;8;1_",
        ),
        (
            key(0xbd, 12, 31, "cs", Key::Char('-'), "_"),
            "\x1f",
            "\x1b[45:95;6u",
            "\x1b[189;12;31;1;24;1_",
        ),
        (
            key(0x09, 15, 9, "s", Key::Tab, ""),
            "\x1b[Z",
            "\x1b[9;2u",
            "\x1b[9;15;9;1;16;1_",
        ),
        (
            key(0x08, 14, 127, "c", Key::Backspace, ""),
            "\x08",
            "\x1b[127;5u",
            "\x1b[8;14;127;1;8;1_",
        ),
        (
            key(0x08, 14, 8, "", Key::Backspace, ""),
            "\x7f",
            "\x7f",
            "\x1b[8;14;8;1;0;1_",
        ),
        (arrow, "\x1b[A", "\x1b[A", "\x1b[38;72;0;1;256;1_"),
        // Ctrl+J is one vector set: no special case to a bare newline.
        (
            key(0x4a, 36, 10, "c", Key::Char('j'), "j"),
            "\n",
            "\x1b[106;5u",
            "\x1b[74;36;10;1;8;1_",
        ),
    ];
    for (input, legacy, kkp, w32) in cases {
        assert_eq!(enc(&input, &LEGACY), legacy, "legacy {input:?}");
        assert_eq!(enc(&input, &kitty(5)), kkp, "kitty {input:?}");
        assert_eq!(enc(&input, &W32IM), w32, "w32im {input:?}");
        // Pushed kitty flags win over win32-input-mode, except on Ctrl+C.
        let both = InputModes { kitty: 5, ..W32IM };
        let want = if input.vk == 0x43 { w32 } else { kkp };
        assert_eq!(enc(&input, &both), want, "kitty over w32im {input:?}");
    }
}

/// Any output can push kitty flags. Under ConPTY, Ctrl+C and Ctrl+Break
/// must still reach conhost as keys, or a console program can no longer
/// be interrupted.
#[test]
fn interrupt_keys_stay_console_records() {
    let ctrl_c = key(0x43, 46, 3, "c", Key::Char('c'), "c");
    let mut ctrl_break = key(0x03, 70, 3, "c", Key::Other, "");
    ctrl_break.extended = true;
    for flags in [1, 5, 1 | 2 | 8] {
        let m = InputModes {
            kitty: flags,
            ..W32IM
        };
        assert_eq!(enc(&ctrl_c, &m), "\x1b[67;46;3;1;8;1_", "{flags}");
        assert_eq!(enc(&up(ctrl_c), &m), "\x1b[67;46;3;0;8;1_", "{flags}");
        assert_eq!(enc(&ctrl_break, &m), "\x1b[3;70;3;1;264;1_", "{flags}");
    }
    // Other chords and plain C keep the kitty encoding.
    let both = InputModes { kitty: 1, ..W32IM };
    assert_eq!(enc(&key(0x43, 46, 99, "", Key::Char('c'), "c"), &both), "c");
    assert_eq!(
        enc(&key(0x43, 46, 3, "ca", Key::Char('c'), ""), &both),
        "\x1b[99;7u"
    );
    // Super on top still interrupts, as conhost sees Ctrl+C. Ctrl+Shift+C
    // is copy, so with nothing to copy a program that pushed kitty flags
    // gets it as its own key.
    let cw_c = key(0x43, 46, 3, "cw", Key::Char('c'), "c");
    assert_eq!(enc(&cw_c, &both), "\x1b[67;46;3;1;8;1_");
    let cs_c = key(0x43, 46, 3, "cs", Key::Char('c'), "C");
    assert_eq!(enc(&cs_c, &both), "\x1b[99;6u");
    // Ctrl+Shift+Break is no copy key, so it still interrupts.
    let mut cs_break = key(0x03, 70, 3, "cs", Key::Other, "");
    cs_break.extended = true;
    assert_eq!(enc(&cs_break, &both), "\x1b[3;70;3;1;280;1_");
    // Without win32-input-mode there is no console record to keep.
    assert_eq!(enc(&ctrl_c, &kitty(1)), "\x1b[99;5u");
}

/// Which keys arrive as Ctrl+C, so a copy chord with nothing to copy can
/// be kept from interrupting the program.
#[test]
fn ctrl_shift_c_interrupts_only_without_kitty_keys() {
    use vt::keys::{interrupts, is_interrupt};
    let ctrl_c = key(0x43, 46, 3, "c", Key::Char('c'), "c");
    let cs_c = key(0x43, 46, 3, "cs", Key::Char('c'), "C");
    assert!(is_interrupt(&ctrl_c) && !is_interrupt(&cs_c));
    for m in [
        LEGACY,
        W32IM,
        kitty(1),
        kitty(8),
        InputModes { kitty: 5, ..W32IM },
    ] {
        assert!(interrupts(&ctrl_c, &m), "{m:?}");
    }
    // Without kitty keys Ctrl+Shift+C is sent as Ctrl+C.
    assert_eq!(enc(&cs_c, &LEGACY), "\x03");
    for m in [LEGACY, W32IM, kitty(2)] {
        assert!(interrupts(&cs_c, &m), "{m:?}");
    }
    for m in [kitty(1), kitty(8), InputModes { kitty: 1, ..W32IM }] {
        assert!(!interrupts(&cs_c, &m), "{m:?}");
    }
    // Other chords never interrupt.
    let ctrl_insert = key(0x2d, 82, 0, "c", Key::Insert, "");
    let ctrl_alt_c = key(0x43, 46, 3, "ca", Key::Char('c'), "c");
    let c = key(0x43, 46, 99, "", Key::Char('c'), "c");
    for k in [ctrl_insert, ctrl_alt_c, c] {
        assert!(!interrupts(&k, &LEGACY), "{k:?}");
    }
}

#[test]
fn win32_input_mode_keys_send_every_transition() {
    // Shift down, Enter down, Enter up, Shift up.
    let seq = [
        key(0x10, 42, 0, "s", Key::Shift, ""),
        key(0x0d, 28, 13, "s", Key::Enter, ""),
        up(key(0x0d, 28, 13, "s", Key::Enter, "")),
        up(key(0x10, 42, 0, "", Key::Shift, "")),
    ];
    let got: String = seq.iter().map(|input| enc(input, &W32IM)).collect();
    assert_eq!(
        got,
        "\x1b[16;42;0;1;16;1_\x1b[13;28;13;1;16;1_\x1b[13;28;13;0;16;1_\x1b[16;42;0;0;0;1_"
    );

    // Lock states and a caller-supplied control state are carried over.
    let mut caps = key(0x41, 30, 65, "", Key::Char('a'), "A");
    caps.locks = Locks {
        caps: true,
        num: true,
        scroll: false,
    };
    assert_eq!(enc(&caps, &W32IM), "\x1b[65;30;65;1;160;1_");
    let mut given = key(0x41, 30, 97, "", Key::Char('a'), "a");
    given.cs = 0x20;
    given.repeat = 3;
    assert_eq!(enc(&given, &W32IM), "\x1b[65;30;97;1;32;3_");

    // Each control-key-state bit: RIGHT_ALT 1, LEFT_ALT 2, RIGHT_CTRL 4,
    // LEFT_CTRL 8, SHIFT 16, NUMLOCK 32, SCROLLLOCK 64, CAPSLOCK 128,
    // ENHANCED_KEY 256. A repeat count of 0 is sent as 1.
    let mut right = key(0x41, 30, 97, "", Key::Char('a'), "a");
    right.mods = Mods {
        rctrl: true,
        rshift: true,
        ..Mods::default()
    };
    right.locks.scroll = true;
    right.repeat = 0;
    assert_eq!(enc(&right, &W32IM), "\x1b[65;30;97;1;84;1_");
    right.mods = Mods {
        ralt: true,
        lalt: true,
        ..Mods::default()
    };
    right.locks = Locks::default();
    right.extended = true;
    assert_eq!(enc(&right, &W32IM), "\x1b[65;30;97;1;259;1_");
}

#[test]
fn altgr_keys_send_their_text() {
    // AltGr arrives as Left Ctrl + Right Alt.
    let at = key(0x51, 16, 0x40, "cg", Key::Char('q'), "@");
    assert_eq!(enc(&at, &LEGACY), "@");
    assert_eq!(enc(&at, &kitty(5)), "@");
    assert_eq!(enc(&at, &W32IM), "\x1b[81;16;64;1;9;1_");
    let ogonek = key(0x41, 30, 0x105, "cg", Key::Char('a'), "ą");
    assert_eq!(enc(&ogonek, &LEGACY), "ą");
    assert_eq!(enc(&ogonek, &kitty(5)), "ą");
    assert_eq!(enc(&ogonek, &W32IM), "\x1b[65;30;261;1;9;1_");

    // Ctrl+Alt+A on a US layout produces no text and stays a chord.
    for uc in [0, 1] {
        let chord = key(0x41, 30, uc, "ca", Key::Char('a'), "a");
        assert_eq!(enc(&chord, &LEGACY), "\x1b\x01");
        assert_eq!(enc(&chord, &kitty(5)), "\x1b[97;7u");
    }
}

fn mouse(kind: vt::MouseKind, button: u8, col: u16, row: u16, m: &str) -> vt::MouseEv {
    vt::MouseEv {
        kind,
        button,
        col,
        row,
        mods: mods(m),
    }
}

fn tracking(mode: vt::MouseMode) -> InputModes {
    InputModes {
        mouse: mode,
        mouse_sgr: true,
        ..LEGACY
    }
}

fn enc_mouse(ev: vt::MouseEv, m: &InputModes) -> Option<String> {
    let mut out = Vec::new();
    let sent = vt::encode_mouse(ev, m, &mut out);
    assert_eq!(sent, !out.is_empty(), "{ev:?}");
    sent.then(|| String::from_utf8(out).unwrap())
}

#[test]
fn mouse_sgr_reports() {
    use vt::MouseKind::*;
    use vt::MouseMode::*;
    let click = tracking(Click);
    let s = |v: &str| Some(v.to_string());
    assert_eq!(
        enc_mouse(mouse(Press, 0, 0, 0, ""), &click),
        s("\x1b[<0;1;1M")
    );
    assert_eq!(
        enc_mouse(mouse(Release, 0, 0, 0, ""), &click),
        s("\x1b[<0;1;1m")
    );
    assert_eq!(
        enc_mouse(mouse(Press, 2, 9, 4, ""), &click),
        s("\x1b[<2;10;5M")
    );
    assert_eq!(
        enc_mouse(mouse(Press, 1, 299, 99, ""), &click),
        s("\x1b[<1;300;100M")
    );
    assert_eq!(
        enc_mouse(mouse(WheelUp, 0, 4, 2, ""), &click),
        s("\x1b[<64;5;3M")
    );
    assert_eq!(
        enc_mouse(mouse(WheelDown, 0, 4, 2, "c"), &click),
        s("\x1b[<81;5;3M")
    );
    assert_eq!(
        enc_mouse(mouse(Press, 0, 0, 0, "sa"), &click),
        s("\x1b[<12;1;1M")
    );
    assert_eq!(enc_mouse(mouse(Move, 0, 1, 1, ""), &click), None);
    assert_eq!(enc_mouse(mouse(Press, 4, 0, 0, ""), &click), None);

    let drag = tracking(Drag);
    assert_eq!(
        enc_mouse(mouse(Move, 0, 1, 1, ""), &drag),
        s("\x1b[<32;2;2M")
    );
    assert_eq!(enc_mouse(mouse(Move, 3, 1, 1, ""), &drag), None);

    let any = tracking(Any);
    assert_eq!(
        enc_mouse(mouse(Move, 3, 1, 1, ""), &any),
        s("\x1b[<35;2;2M")
    );
    assert_eq!(
        enc_mouse(mouse(Move, 2, 1, 1, ""), &any),
        s("\x1b[<34;2;2M")
    );

    // Nothing without tracking, or without SGR encoding: the X10 byte form
    // is not implemented.
    assert_eq!(enc_mouse(mouse(Press, 0, 0, 0, ""), &tracking(Off)), None);
    let no_sgr = InputModes {
        mouse_sgr: false,
        ..click
    };
    assert_eq!(enc_mouse(mouse(Press, 0, 0, 0, ""), &no_sgr), None);
}

#[test]
fn mouse_sgr_modifiers_buttons_and_edges() {
    use vt::MouseKind::*;
    use vt::MouseMode::*;
    let s = |v: &str| Some(v.to_string());
    let (click, drag, any) = (tracking(Click), tracking(Drag), tracking(Any));
    // Shift 4, Alt 8, Ctrl 16, on every kind of event.
    assert_eq!(
        enc_mouse(mouse(Release, 2, 0, 0, "c"), &click),
        s("\x1b[<18;1;1m")
    );
    assert_eq!(
        enc_mouse(mouse(Move, 1, 0, 0, "s"), &drag),
        s("\x1b[<37;1;1M")
    );
    assert_eq!(
        enc_mouse(mouse(Press, 0, 0, 0, "g"), &click),
        s("\x1b[<8;1;1M")
    );
    assert_eq!(
        enc_mouse(mouse(WheelUp, 0, 0, 0, "s"), &click),
        s("\x1b[<68;1;1M")
    );
    let mut right = mouse(Press, 0, 0, 0, "");
    right.mods = Mods {
        rshift: true,
        rctrl: true,
        ..Mods::default()
    };
    assert_eq!(enc_mouse(right, &click), s("\x1b[<20;1;1M"));
    // Super is not reported.
    assert_eq!(
        enc_mouse(mouse(Press, 0, 0, 0, "w"), &click),
        s("\x1b[<0;1;1M")
    );
    // Drags with any button; motion without one only in mode 1003.
    assert_eq!(
        enc_mouse(mouse(Move, 1, 2, 3, ""), &drag),
        s("\x1b[<33;3;4M")
    );
    assert_eq!(
        enc_mouse(mouse(Move, 2, 2, 3, ""), &drag),
        s("\x1b[<34;3;4M")
    );
    assert_eq!(enc_mouse(mouse(Move, 4, 0, 0, ""), &any), None);
    assert_eq!(enc_mouse(mouse(Release, 3, 0, 0, ""), &click), None);
    // SGR has no coordinate limit.
    assert_eq!(
        enc_mouse(mouse(Press, 0, u16::MAX, u16::MAX, ""), &click),
        s("\x1b[<0;65536;65536M")
    );
}

/// A move the modes did not ask for is not a report, so it does not stop
/// the next move to the same cell once they do.
#[test]
fn mouse_motion_not_sent_does_not_count() {
    use vt::MouseKind::*;
    let mut tracker = vt::keys::MouseTracker::default();
    let mut out = Vec::new();
    assert!(!tracker.encode(
        mouse(Move, 3, 5, 5, ""),
        &tracking(vt::MouseMode::Drag),
        &mut out
    ));
    assert!(tracker.encode(
        mouse(Move, 3, 5, 5, ""),
        &tracking(vt::MouseMode::Any),
        &mut out
    ));
    assert!(!tracker.encode(mouse(Press, 0, 6, 6, ""), &LEGACY, &mut out));
    assert!(tracker.encode(
        mouse(Move, 3, 6, 6, ""),
        &tracking(vt::MouseMode::Any),
        &mut out
    ));
    assert_eq!(
        String::from_utf8(out).unwrap(),
        "\x1b[<35;6;6M\x1b[<35;7;7M"
    );
}

#[test]
fn mouse_motion_only_on_cell_change() {
    use vt::MouseKind::*;
    let m = tracking(vt::MouseMode::Any);
    let mut tracker = vt::keys::MouseTracker::default();
    let mut out = Vec::new();
    let mut sends = |ev| tracker.encode(ev, &m, &mut out);
    assert!(sends(mouse(Move, 3, 5, 5, "")));
    assert!(!sends(mouse(Move, 3, 5, 5, "")));
    assert!(sends(mouse(Press, 0, 5, 5, "")));
    assert!(!sends(mouse(Move, 0, 5, 5, "")));
    assert!(sends(mouse(Move, 0, 6, 5, "")));
    assert!(sends(mouse(Release, 0, 6, 5, "")));
    assert!(sends(mouse(Release, 0, 6, 5, "")));
    assert_eq!(
        String::from_utf8(out).unwrap(),
        "\x1b[<35;6;6M\x1b[<0;6;6M\x1b[<32;7;6M\x1b[<0;7;6m\x1b[<0;7;6m"
    );
}

#[test]
fn focus_reports_follow_mode_1004() {
    let on = InputModes {
        focus: true,
        ..LEGACY
    };
    let mut out = Vec::new();
    vt::encode_focus(true, &on, &mut out);
    vt::encode_focus(false, &on, &mut out);
    vt::encode_focus(true, &LEGACY, &mut out);
    assert_eq!(out, b"\x1b[I\x1b[O");
}

fn paste(text: &str, bracketed: bool) -> String {
    let mut out = Vec::new();
    vt::encode_paste(text, bracketed, &mut out);
    String::from_utf8(out).unwrap()
}

#[test]
fn paste_filters_controls_and_line_breaks() {
    assert_eq!(paste("a\r\nb\nc\rd\r\n", false), "a\rb\rc\rd\r");
    assert_eq!(paste("x\x1b[31my\x03\x7f\u{9b}z\tq", false), "x[31myz\tq");
    assert_eq!(paste("héllo ✳ 日本 🦀", false), "héllo ✳ 日本 🦀");
    assert_eq!(paste("", false), "");
}

#[test]
fn paste_drops_controls_before_joining_line_breaks() {
    // CR LF with a dropped control between them is still one line break.
    assert_eq!(paste("a\r\x1b\nb", false), "a\rb");
    assert_eq!(paste("a\r\u{9b}\nb", false), "a\rb");
    // Breaks in other orders stay apart.
    assert_eq!(paste("a\n\rb", false), "a\r\rb");
    assert_eq!(paste("a\r\r\nb", false), "a\r\rb");
    assert_eq!(paste("\r\n\r\n", false), "\r\r");
    // NEL is C1 and dropped like the rest.
    assert_eq!(paste("a\u{85}b", false), "ab");
    assert_eq!(paste("a\tb", true), "\x1b[200~a\tb\x1b[201~");
}

#[test]
fn paste_bracketed() {
    assert_eq!(paste("hi\n", true), "\x1b[200~hi\r\x1b[201~");
    assert_eq!(paste("", true), "\x1b[200~\x1b[201~");
}

#[test]
fn paste_cannot_break_out_of_the_bracket() {
    let got = paste("\x1b[201~\r\nrm -rf ~\r\n\u{9b}201~", true);
    assert_eq!(got, "\x1b[200~[201~\rrm -rf ~\r201~\x1b[201~");
    assert_eq!(got.matches("\x1b[201~").count(), 1);
    assert!(got.ends_with("\x1b[201~"));
}

#[test]
fn paste_confirm_only_for_untrusted_line_breaks() {
    use vt::keys::needs_paste_confirm;
    for bracketed in [false, true] {
        assert!(!needs_paste_confirm("ls -la", bracketed, false));
        assert!(needs_paste_confirm("echo 1\necho 2", bracketed, false));
        assert!(needs_paste_confirm("echo 1\r", bracketed, false));
        assert!(!needs_paste_confirm("", bracketed, false));
    }
    assert!(!needs_paste_confirm("echo 1\necho 2", true, true));
}

/// A long line without bracketed paste is typed in key by key, so it is
/// confirmed first, as Windows Terminal's large paste warning does.
#[test]
fn paste_confirm_for_large_unbracketed_text() {
    use vt::keys::{LARGE_PASTE, needs_paste_confirm};
    let line = "x".repeat(LARGE_PASTE);
    assert!(!needs_paste_confirm(&line, false, false), "at the limit");
    let big = "x".repeat(LARGE_PASTE + 1);
    assert!(needs_paste_confirm(&big, false, false));
    assert!(!needs_paste_confirm(&big, true, false), "bracketed");
    assert!(!needs_paste_confirm(&big, true, true));
}

/// Any output can turn bracketed paste on, also for a program that does
/// not read it, so a paste under it is trusted only once the user has
/// confirmed one.
#[test]
fn bracketed_paste_is_trusted_once_confirmed() {
    let mut t = vt::Terminal::new(vt::Options::default());
    t.confirm_paste();
    t.feed(b"\x1b]133;A;blitz=1\x07\x1b]133;C\x07\x1b[?2004h");
    assert!(!t.paste_trusted(), "set by output after a command started");
    t.confirm_paste();
    assert!(t.paste_trusted());
    // Claude Code sets it again on every redraw; only turning it on anew
    // asks again.
    t.feed(b"\x1b[?2004h\x1b[?2004h");
    assert!(t.paste_trusted(), "set again while on");
    for reset in ["\x1b]133;A;blitz=1\x07", "\x1bc", "\x1b[?2004l"] {
        t.confirm_paste();
        t.feed(reset.as_bytes());
        t.feed(b"\x1b[?2004h");
        assert!(!t.paste_trusted(), "{reset:?}");
    }
}

/// A program that says it reads pastes as text before it turns bracketed
/// paste on is trusted when it does, and only that once.
#[test]
fn a_vouched_paste_holds_for_the_next_turn_on_only() {
    let mut t = vt::Terminal::new(vt::Options::default());
    t.vouch_paste(true);
    assert!(!t.paste_trusted(), "off");
    t.feed(b"\x1b[?2004h");
    assert!(t.paste_trusted());
    t.feed(b"\x1b[?2004l\x1b[?2004h");
    assert!(!t.paste_trusted(), "the next one asks");
    // A shell's prompt, a reset or a no in between forget it.
    for reset in ["\x1b]133;A;blitz=1\x07", "\x1bc"] {
        t.feed(b"\x1b[?2004l");
        t.vouch_paste(true);
        t.feed(reset.as_bytes());
        t.feed(b"\x1b[?2004h");
        assert!(!t.paste_trusted(), "{reset:?}");
    }
    t.feed(b"\x1b[?2004l");
    t.vouch_paste(true);
    t.vouch_paste(false);
    t.feed(b"\x1b[?2004h");
    assert!(!t.paste_trusted(), "no");
    // While on, a yes confirms at once, as the user would.
    t.vouch_paste(true);
    assert!(t.paste_trusted());
    // A no after the turn-on took the yes forgets it too, so a prompt
    // that is not blitz's does not keep it.
    t.feed(b"[?2004l");
    t.vouch_paste(true);
    t.feed(b"[?2004h");
    t.vouch_paste(false);
    assert!(!t.paste_trusted(), "no after the turn-on");
    t.feed(b"]133;A");
    assert!(!t.paste_trusted(), "another prompt");
}
