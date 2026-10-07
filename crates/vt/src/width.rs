//! Display width of grapheme clusters and the rules that join code points
//! into one cluster.
//!
//! Widths follow what Claude Code measures with (`string-width` with
//! ambiguous characters narrow): East Asian Wide and Fullwidth take two
//! columns, as does anything with Emoji_Presentation; VS16 widens an emoji
//! base and VS15 never narrows one.

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

    /// Splits `s` into clusters with [`joins`].
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
        assert_eq!(clusters("क\u{093E}"), ["क\u{093E}"], "spacing mark");
    }
}
