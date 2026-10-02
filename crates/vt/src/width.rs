//! Display width of grapheme clusters.

/// Columns a grapheme cluster occupies: 0 for an empty string, else 1 or 2.
pub fn cluster_width(s: &str) -> u8 {
    let Some(c) = s.chars().next() else {
        return 0;
    };
    // ponytail: rough East Asian Wide and emoji blocks; exact tables
    // generated from the UCD replace this.
    match c as u32 {
        0x1100..=0x115F
        | 0x2E80..=0x303E
        | 0x3041..=0xA4CF
        | 0xAC00..=0xD7A3
        | 0xF900..=0xFAFF
        | 0xFE30..=0xFE4F
        | 0xFF00..=0xFF60
        | 0xFFE0..=0xFFE6
        | 0x1F300..=0x1F64F
        | 0x1F900..=0x1F9FF
        | 0x20000..=0x3FFFD => 2,
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::cluster_width;

    #[test]
    fn narrow_wide_and_empty() {
        assert_eq!(cluster_width(""), 0);
        assert_eq!(cluster_width("a"), 1);
        assert_eq!(cluster_width("中"), 2);
        assert_eq!(cluster_width("한"), 2);
        assert_eq!(cluster_width("😀"), 2);
        // Symbols that sit in Claude Code's status line stay narrow.
        for s in ["\u{25C9}", "\u{2605}", "\u{23F5}"] {
            assert_eq!(cluster_width(s), 1, "{s}");
        }
    }
}
