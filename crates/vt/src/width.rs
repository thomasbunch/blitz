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

/// Columns a grapheme cluster occupies.
///
/// This is the width of its first code point, raised to 2 when an emoji
/// base is followed by VS16 or a skin-tone modifier, or starts a ZWJ
/// sequence. Returns 0 for an empty cluster or one that starts with a
/// zero-width code point (a lone combining mark, for example).
pub fn cluster_width(s: &str) -> u8 {
    chars_width(s.chars(), false)
}

/// [`cluster_width`] over a cluster held as code points, with East Asian
/// ambiguous characters counted as wide when `ambiguous_wide` is set.
pub fn chars_width(cluster: impl IntoIterator<Item = char>, ambiguous_wide: bool) -> u8 {
    let mut cps = cluster.into_iter();
    let Some(first) = cps.next() else {
        return 0;
    };
    let p = props(first);
    let w = match p & WIDTH_MASK {
        W_ZERO => return 0,
        W_WIDE => return 2,
        W_AMBIGUOUS if ambiguous_wide => return 2,
        _ => 1,
    };
    if p & (EMOJI | EXT_PICT) == 0 {
        return w;
    }
    let widens = |c: char| {
        (p & EMOJI != 0 && (c == VS16 || is_emoji_modifier(c)))
            || (p & EXT_PICT != 0 && c == '\u{200D}')
    };
    if cps.any(widens) { 2 } else { w }
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
