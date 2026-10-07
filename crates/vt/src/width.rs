//! Display width of grapheme clusters and the rules that join code points
//! into one cluster.
//!
//! Widths follow what Claude Code measures with (`string-width` with
//! ambiguous characters narrow): East Asian Wide and Fullwidth take two
//! columns, as does anything with Emoji_Presentation; VS16 widens an emoji
//! base and VS15 never narrows one.
//!
//! A few code points on their own follow terminals' wcwidth instead, where
//! the two disagree: the soft hyphen takes a column, as terminals draw it;
//! the Hangul fillers U+3164 and U+FFA0 and unassigned default-ignorable
//! code points keep their East Asian width; and a Hangul medial vowel or
//! final consonant with no syllable to join takes none. string-width
//! measures the first three 0 and the last 1.

#[rustfmt::skip]
#[path = "width_tables.rs"]
mod tables;

// Property byte layout, shared with examples/gen_unicode.rs.
const WIDTH_MASK: u8 = 0b11;
const W_ZERO: u8 = 0;
const W_WIDE: u8 = 2;
const W_AMBIGUOUS: u8 = 3;
const GCB_SHIFT: u8 = 2;
const EXT_PICT: u8 = 1 << 6;
const EMOJI: u8 = 1 << 7;

// Grapheme cluster break classes.
const EXTEND: u8 = 1;
const ZWJ: u8 = 2;
const SPACING_MARK: u8 = 3;
const PREPEND: u8 = 4;
const RI: u8 = 5;
const L: u8 = 6;
const V: u8 = 7;
const T: u8 = 8;
const LV: u8 = 9;
const LVT: u8 = 10;
const CONTROL: u8 = 11;

const VS16: char = '\u{FE0F}';

fn props(c: char) -> u8 {
    let cp = c as usize;
    let block = usize::from(tables::STAGE1[cp >> tables::SHIFT]);
    tables::STAGE2[(block << tables::SHIFT) | (cp & ((1 << tables::SHIFT) - 1))]
}

fn gcb(c: char) -> u8 {
    (props(c) >> GCB_SHIFT) & 0xF
}

fn is_emoji_modifier(c: char) -> bool {
    ('\u{1F3FB}'..='\u{1F3FF}').contains(&c)
}

/// Columns a single code point takes on its own: 0 for marks, joiners and
/// controls, 2 for wide and emoji-presentation characters, else 1.
/// Ambiguous-width characters count as narrow.
pub fn char_width(c: char) -> u8 {
    if (' '..='~').contains(&c) {
        return 1;
    }
    match props(c) & WIDTH_MASK {
        W_ZERO => 0,
        W_WIDE => 2,
        _ => 1,
    }
}

/// Whether `c` has the Emoji property.
pub fn is_emoji(c: char) -> bool {
    props(c) & EMOJI != 0
}

/// Default_Ignorable_Code_Point: joiners, variation selectors, bidi and
/// other format controls, fillers and tags, which draw nothing of their
/// own.
pub fn is_ignorable(c: char) -> bool {
    matches!(c,
        '\u{AD}' | '\u{34F}' | '\u{61C}' | '\u{115F}'..='\u{1160}' | '\u{17B4}'..='\u{17B5}'
        | '\u{180B}'..='\u{180F}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}'
        | '\u{2060}'..='\u{206F}' | '\u{3164}' | '\u{FE00}'..='\u{FE0F}' | '\u{FEFF}'
        | '\u{FFA0}' | '\u{FFF0}'..='\u{FFF8}' | '\u{1BCA0}'..='\u{1BCA3}'
        | '\u{1D173}'..='\u{1D17A}' | '\u{E0000}'..='\u{E0FFF}')
}

/// Columns a grapheme cluster occupies.
///
/// This is the width of its first code point that takes a column (format
/// characters such as U+0600 can be prepended), raised to 2 when an emoji
/// base is followed by VS16 or a skin-tone modifier, or starts a ZWJ
/// sequence with another pictograph. Returns 0 for an empty cluster or one
/// of zero-width code points only (a lone combining mark, for example).
pub fn cluster_width(s: &str) -> u8 {
    chars_width(s.chars(), false)
}

/// [`cluster_width`] over a cluster held as code points, with East Asian
/// ambiguous characters counted as wide when `ambiguous_wide` is set.
pub fn chars_width(cluster: impl IntoIterator<Item = char>, ambiguous_wide: bool) -> u8 {
    let mut cps = (cluster.into_iter()).skip_while(|&c| props(c) & WIDTH_MASK == W_ZERO);
    let Some(first) = cps.next() else {
        return 0;
    };
    let p = props(first);
    let w = match p & WIDTH_MASK {
        W_WIDE => return 2,
        W_AMBIGUOUS if ambiguous_wide => return 2,
        _ => 1,
    };
    if p & (EMOJI | EXT_PICT) == 0 {
        return w;
    }
    let mut joined = false;
    for c in cps {
        if p & EMOJI != 0 && (c == VS16 || is_emoji_modifier(c))
            || joined && p & EXT_PICT != 0 && props(c) & EXT_PICT != 0
        {
            return 2;
        }
        joined = c == '\u{200D}';
    }
    w
}

/// Whether `c` continues the grapheme cluster that starts with `first`,
/// ends with `last` and holds `len` code points.
///
/// These are the UAX #29 pair rules plus regional-indicator pairs, which
/// covers combining marks, emoji sequences, flags and Hangul jamo. It is
/// not full UAX #29: the emoji ZWJ rule only checks the first code point,
/// and Indic conjuncts (GB9c) split. Move to the full state machine if a
/// script needs those.
pub fn joins(first: char, last: char, len: usize, c: char) -> bool {
    match (gcb(last), gcb(c)) {
        (CONTROL, _) | (_, CONTROL) => false,
        (_, EXTEND | ZWJ | SPACING_MARK) | (PREPEND, _) => true,
        (L, L | V | LV | LVT) | (LV | V, V | T) | (LVT | T, T) => true,
        (ZWJ, _) => props(first) & EXT_PICT != 0 && props(c) & EXT_PICT != 0,
        (RI, RI) => len == 1,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Splits `s` into clusters with [`joins`], as UAX #29 has them. The
    /// terminal then drops a cluster that starts with a zero-width code
    /// point, which these keep.
    fn clusters(s: &str) -> Vec<&str> {
        let mut out = Vec::new();
        let mut start = 0;
        let (mut first, mut last, mut len) = ('\0', '\0', 0);
        for (i, c) in s.char_indices() {
            if len > 0 && joins(first, last, len, c) {
                last = c;
                len += 1;
                continue;
            }
            if len > 0 {
                out.push(&s[start..i]);
            }
            (start, first, last, len) = (i, c, c, 1);
        }
        if len > 0 {
            out.push(&s[start..]);
        }
        out
    }

    #[test]
    fn width_basics() {
        assert_eq!(cluster_width(""), 0);
        assert_eq!(cluster_width("a"), 1);
        assert_eq!(cluster_width("中"), 2);
        assert_eq!(cluster_width("한"), 2);
        assert_eq!(cluster_width("ｱ"), 1, "halfwidth katakana");
        assert_eq!(cluster_width("Ａ"), 2, "fullwidth latin");
        assert_eq!(cluster_width("😀"), 2);
        assert_eq!(cluster_width("\u{0301}"), 0, "lone combining mark");
        assert_eq!(cluster_width("\u{200B}"), 0, "zero width space");
        assert_eq!(cluster_width("\u{AD}"), 1, "soft hyphen");
        assert_eq!(cluster_width("\u{1160}"), 0, "hangul medial vowel");
    }

    #[test]
    fn width_claude_code_glyphs_are_narrow() {
        for s in [
            "▐", "▛", "█", "▜", "▌", "⎿", "✻", "✳", "●", "◐", "◑", "❯", "◉", "★", "⏵", "─", "│",
            "╭", "╮", "╰", "╯",
        ] {
            assert_eq!(
                cluster_width(s),
                1,
                "{s} U+{:04X}",
                s.chars().next().unwrap() as u32
            );
        }
    }

    #[test]
    fn width_emoji_sequences() {
        assert_eq!(cluster_width("❤\u{FE0F}"), 2, "VS16 widens");
        assert_eq!(cluster_width("✳\u{FE0F}"), 2);
        assert_eq!(cluster_width("1\u{FE0F}\u{20E3}"), 2, "keycap");
        assert_eq!(cluster_width("a\u{FE0F}"), 1, "VS16 on a non-emoji");
        assert_eq!(cluster_width("⌚\u{FE0E}"), 2, "VS15 never narrows");
        assert_eq!(cluster_width("☝\u{1F3FD}"), 2, "skin tone");
        assert_eq!(cluster_width("👨\u{200D}👩\u{200D}👧"), 2);
        assert_eq!(cluster_width("🇺🇸"), 2);
        assert_eq!(cluster_width("e\u{0301}"), 1);
    }

    #[test]
    fn width_ambiguous_option() {
        assert_eq!(chars_width(['─'], false), 1);
        assert_eq!(chars_width(['─'], true), 2);
        assert_eq!(chars_width(['a'], true), 1);
        assert_eq!(chars_width("中".chars(), false), 2);
    }

    #[test]
    fn width_char_width() {
        assert_eq!(char_width('a'), 1);
        assert_eq!(char_width('\u{7F}'), 0);
        assert_eq!(char_width('\u{1B}'), 0);
        assert_eq!(char_width('\u{0301}'), 0);
        assert_eq!(char_width('\u{200D}'), 0);
        assert_eq!(char_width('\u{FE0F}'), 0);
        assert_eq!(char_width('中'), 2);
        assert_eq!(char_width('★'), 1);
        assert_eq!(char_width('\u{1F3FB}'), 2, "modifier on its own");
    }

    #[test]
    fn width_prepended_format_characters_take_no_column() {
        assert_eq!(cluster_width("\u{0600}1"), 1, "number sign before a digit");
        assert_eq!(cluster_width("\u{0600}\u{0600}\u{0661}"), 1);
        assert_eq!(cluster_width("\u{0600}中"), 2);
        assert_eq!(cluster_width("\u{0600}"), 0, "on its own");
        assert_eq!(cluster_width("\u{0301}\u{0302}"), 0, "marks only");
        // A spacing prepend is a letter and keeps its column.
        assert_eq!(cluster_width("\u{0D4E}\u{0D15}"), 1);
    }

    #[test]
    fn width_joiner_widens_only_a_pictograph_sequence() {
        // The base is narrow on its own, so this goes past the early
        // return for wide bases.
        assert_eq!(cluster_width("\u{2764}\u{200D}\u{1F525}"), 2);
        assert_eq!(cluster_width("\u{A9}\u{200D}\u{2764}"), 2);
        assert_eq!(
            cluster_width("\u{A9}\u{200D}"),
            1,
            "nothing after the joiner"
        );
        assert_eq!(cluster_width("\u{A9}\u{200D}\u{0301}"), 1);
        assert_eq!(cluster_width("#\u{200D}\u{2764}"), 1, "not a pictograph");
    }

    #[test]
    fn width_ambiguous_option_leaves_zero_width_and_narrow_alone() {
        assert_eq!(chars_width(['\u{300}'], true), 0, "ambiguous but a mark");
        assert_eq!(
            chars_width(['\u{FE0F}'], true),
            0,
            "ambiguous but a selector"
        );
        assert_eq!(chars_width(['\u{AD}'], true), 1, "soft hyphen stays narrow");
        assert_eq!(chars_width(['\u{E000}'], false), 1, "private use");
        assert_eq!(chars_width(['\u{E000}'], true), 2);
        assert_eq!(chars_width(['\u{FFFD}'], true), 2);
        assert_eq!(chars_width(['\u{AE}'], true), 2);
        assert_eq!(chars_width(['\u{AE}', '\u{FE0F}'], false), 2);
    }

    #[test]
    fn width_kirat_rai_vowel_signs_are_spacing_letters() {
        // Grapheme break class V like Hangul medial vowels, but letters
        // that take a column of their own.
        for c in ['\u{16D63}', '\u{16D67}', '\u{16D6A}'] {
            assert_eq!(char_width(c), 1, "U+{:X}", c as u32);
        }
        assert_eq!(char_width('\u{1161}'), 0, "hangul medial vowel");
        assert_eq!(char_width('\u{11A8}'), 0, "hangul final consonant");
        assert_eq!(char_width('\u{D7B0}'), 0, "hangul jamo extended-b vowel");
        assert_eq!(char_width('\u{D7FB}'), 0, "hangul jamo extended-b final");
    }

    #[test]
    fn width_joins_clusters() {
        assert_eq!(clusters("abc"), ["a", "b", "c"]);
        assert_eq!(clusters("e\u{0301}x"), ["e\u{0301}", "x"]);
        assert_eq!(clusters("❤\u{FE0F}!"), ["❤\u{FE0F}", "!"]);
        assert_eq!(
            clusters("👨\u{200D}👩\u{200D}👧👍\u{1F3FD}"),
            ["👨\u{200D}👩\u{200D}👧", "👍\u{1F3FD}"]
        );
        assert_eq!(clusters("🇺🇸🇬🇧🇫"), ["🇺🇸", "🇬🇧", "🇫"]);
        // A joiner after plain text does not glue the next emoji on.
        assert_eq!(clusters("a\u{200D}😀"), ["a\u{200D}", "😀"]);
        // Conjoining jamo: L V T builds one syllable.
        assert_eq!(
            clusters("\u{1100}\u{1161}\u{11A8}가"),
            ["\u{1100}\u{1161}\u{11A8}", "가"]
        );
        assert_eq!(clusters("a\u{1B}\u{0301}"), ["a", "\u{1B}", "\u{0301}"]);
        assert_eq!(clusters("\u{0600}1"), ["\u{0600}1"], "prepend");
        assert_eq!(cluster_width("\u{0600}1"), 1);
        assert_eq!(clusters("\u{0D4E}\u{0D15}x"), ["\u{0D4E}\u{0D15}", "x"]);
        assert_eq!(clusters("क\u{093E}"), ["क\u{093E}"], "spacing mark");
    }

    #[test]
    fn width_joins_every_hangul_pair() {
        let (l, v, t) = ('\u{1100}', '\u{1161}', '\u{11A8}');
        let (lv, lvt) = ('\u{AC00}', '\u{AC01}');
        for (a, b, joined) in [
            (l, l, true),
            (l, v, true),
            (l, lv, true),
            (l, lvt, true),
            (l, t, false),
            (v, v, true),
            (v, t, true),
            (v, l, false),
            (lv, v, true),
            (lv, t, true),
            (lv, l, false),
            (lvt, t, true),
            (lvt, v, false),
            (t, t, true),
            (t, v, false),
            (t, lv, false),
        ] {
            assert_eq!(
                joins(a, a, 1, b),
                joined,
                "U+{:X} U+{:X}",
                a as u32,
                b as u32
            );
        }
        assert_eq!(cluster_width("가\u{11A8}"), 2);
        assert_eq!(cluster_width("\u{1100}가"), 2);
    }

    #[test]
    fn width_regional_indicators_pair_only_when_adjacent() {
        assert_eq!(clusters("🇺\u{0301}🇸"), ["🇺\u{0301}", "🇸"]);
        assert_eq!(clusters("🇺🇸🇬🇧"), ["🇺🇸", "🇬🇧"]);
        assert_eq!(clusters("a🇺🇸"), ["a", "🇺🇸"]);
    }

    #[test]
    fn width_emoji_modifier_range_edges() {
        for (m, w) in [
            ('\u{1F3FA}', 1),
            ('\u{1F3FB}', 2),
            ('\u{1F3FF}', 2),
            ('\u{1F400}', 1),
        ] {
            assert_eq!(chars_width(['\u{261D}', m], false), w, "U+{:X}", m as u32);
        }
        assert_eq!(chars_width(['a', '\u{1F3FB}'], false), 1, "not an emoji");
    }

    #[test]
    fn width_ignorable_matches_default_ignorable_code_point() {
        // Edges of the UCD 17.0 Default_Ignorable_Code_Point ranges.
        for (c, ignorable) in [
            ('\u{AC}', false),
            ('\u{AD}', true),
            ('\u{AE}', false),
            ('\u{34E}', false),
            ('\u{34F}', true),
            ('\u{350}', false),
            ('\u{61B}', false),
            ('\u{61C}', true),
            ('\u{115E}', false),
            ('\u{115F}', true),
            ('\u{1160}', true),
            ('\u{1161}', false),
            ('\u{17B3}', false),
            ('\u{17B4}', true),
            ('\u{17B5}', true),
            ('\u{17B6}', false),
            ('\u{180A}', false),
            ('\u{180B}', true),
            ('\u{180F}', true),
            ('\u{1810}', false),
            ('\u{200A}', false),
            ('\u{200B}', true),
            ('\u{200F}', true),
            ('\u{2010}', false),
            ('\u{2029}', false),
            ('\u{202A}', true),
            ('\u{202E}', true),
            ('\u{202F}', false),
            ('\u{205F}', false),
            ('\u{2060}', true),
            ('\u{2065}', true),
            ('\u{206F}', true),
            ('\u{2070}', false),
            ('\u{3163}', false),
            ('\u{3164}', true),
            ('\u{3165}', false),
            ('\u{FDFF}', false),
            ('\u{FE00}', true),
            ('\u{FE0F}', true),
            ('\u{FE10}', false),
            ('\u{FEFE}', false),
            ('\u{FEFF}', true),
            ('\u{FF00}', false),
            ('\u{FF9F}', false),
            ('\u{FFA0}', true),
            ('\u{FFA1}', false),
            ('\u{FFEF}', false),
            ('\u{FFF0}', true),
            ('\u{FFF8}', true),
            ('\u{FFF9}', false),
            ('\u{1BC9F}', false),
            ('\u{1BCA0}', true),
            ('\u{1BCA3}', true),
            ('\u{1BCA4}', false),
            ('\u{1D172}', false),
            ('\u{1D173}', true),
            ('\u{1D17A}', true),
            ('\u{1D17B}', false),
            ('\u{DFFFF}', false),
            ('\u{E0000}', true),
            ('\u{E0FFF}', true),
            ('\u{E1000}', false),
        ] {
            assert_eq!(is_ignorable(c), ignorable, "U+{:X}", c as u32);
        }
    }

    #[test]
    fn width_lone_fillers_and_jamo_follow_wcwidth() {
        // See the module docs: string-width differs on these.
        assert_eq!(cluster_width("\u{3164}"), 2, "hangul filler");
        assert_eq!(cluster_width("\u{FFA0}"), 1, "halfwidth hangul filler");
        assert_eq!(
            cluster_width("\u{E0080}"),
            1,
            "unassigned default ignorable"
        );
        assert_eq!(
            cluster_width("\u{1161}"),
            0,
            "medial vowel without a syllable"
        );
        assert_eq!(cluster_width("\u{115F}\u{1161}"), 2, "filler-led syllable");
    }

    #[test]
    fn width_table_covers_every_code_point() {
        assert_eq!(tables::STAGE1.len() << tables::SHIFT, 0x11_0000);
        assert_eq!(tables::STAGE2.len() % (1 << tables::SHIFT), 0);
        let blocks = tables::STAGE2.len() >> tables::SHIFT;
        assert!(tables::STAGE1.iter().all(|&b| usize::from(b) < blocks));
        for c in (0..=0x10_FFFF).filter_map(char::from_u32) {
            assert!(gcb(c) <= CONTROL, "U+{:X}", c as u32);
            assert!(chars_width([c], true) <= 2, "U+{:X}", c as u32);
        }
        for c in '\u{AC00}'..='\u{D7A3}' {
            let lv = (c as u32 - 0xAC00).is_multiple_of(28);
            assert_eq!(gcb(c), if lv { LV } else { LVT }, "U+{:X}", c as u32);
            assert_eq!(char_width(c), 2, "U+{:X}", c as u32);
        }
        for c in '\u{1F1E6}'..='\u{1F1FF}' {
            assert_eq!((gcb(c), char_width(c)), (RI, 2), "U+{:X}", c as u32);
        }
        assert_eq!(char_width('\u{10FFFF}'), 1);
        assert_eq!(char_width('\u{D7FF}'), 1, "unassigned past the jamo");
    }

    #[test]
    fn width_tables_are_unicode_17() {
        // Emoji_Presentation since emoji 17.0; unassigned and narrow before.
        assert_eq!(cluster_width("\u{1F6D8}"), 2, "landslide");
        assert_eq!(cluster_width("\u{1FAEA}"), 2, "distorted face");
    }
}
