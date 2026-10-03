//! Cell styles, interned so a cell stores a 16-bit id, and the OSC 8
//! hyperlinks they point to.

use std::collections::HashMap;
use std::hash::Hasher;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Color {
    #[default]
    Default,
    Idx(u8),
    Rgb(u8, u8, u8),
}

/// [`Style::attrs`] bits.
pub mod attr {
    pub const BOLD: u16 = 1 << 0;
    pub const DIM: u16 = 1 << 1;
    pub const ITALIC: u16 = 1 << 2;
    /// Underline kind as in SGR `4:x`: 0 none, 1 single, 2 double,
    /// 3 curly, 4 dotted, 5 dashed.
    pub const UNDERLINE: u16 = 0b111 << UNDERLINE_SHIFT;
    pub const UNDERLINE_SHIFT: u16 = 3;
    pub const BLINK: u16 = 1 << 6;
    pub const INVERSE: u16 = 1 << 7;
    pub const INVISIBLE: u16 = 1 << 8;
    pub const STRIKE: u16 = 1 << 9;
    pub const OVERLINE: u16 = 1 << 10;
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Style {
    pub fg: Color,
    pub bg: Color,
    /// Underline colour.
    pub ul: Color,
    /// [`attr`] bits.
    pub attrs: u16,
    /// Hyperlink id from [`Styles::intern_link`], 0 for none.
    pub link: u32,
}

/// An OSC 8 hyperlink target.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Link {
    /// The `id=` parameter; empty when the application gave none.
    pub id: String,
    pub uri: String,
}

/// Style and hyperlink interner. Style id 0 is always the default style.
#[derive(Debug)]
pub struct Styles {
    list: Vec<Style>,
    map: HashMap<Style, u16, FxBuild>,
    /// Link `n` is `links[n - 1]`.
    links: Vec<Link>,
    link_map: HashMap<Link, u32>,
    /// Bytes of id and URI held in `links`.
    link_bytes: usize,
    /// Styles and links asked for since the last compaction that were not
    /// in the table yet.
    misses: usize,
}

impl Default for Styles {
    fn default() -> Self {
        Self::new()
    }
}

impl Styles {
    pub fn new() -> Self {
        let d = Style::default();
        Self {
            list: vec![d],
            map: HashMap::from_iter([(d, 0)]),
            links: Vec::new(),
            link_map: HashMap::new(),
            link_bytes: 0,
            misses: 0,
        }
    }

    /// Returns the id for `s`, adding it if needed. When the table is full
    /// a new style falls back to id 0; check [`Styles::wants_compact`] and
    /// [`Styles::compact`] first.
    pub fn intern(&mut self, s: Style) -> u16 {
        if let Some(&id) = self.map.get(&s) {
            return id;
        }
        self.misses += 1;
        let Ok(id) = u16::try_from(self.list.len()) else {
            return 0;
        };
        self.list.push(s);
        self.map.insert(s, id);
        id
    }

    /// The style for `id`; unknown ids read as the default style.
    pub fn get(&self, id: u16) -> &Style {
        self.list.get(usize::from(id)).unwrap_or(&self.list[0])
    }

    /// Interns an OSC 8 link and returns its id for [`Style::link`].
    /// The same `id` and `uri` always give the same link id. When the
    /// link table is full, or the id or URI is too long, the text is left
    /// unlinked (0).
    pub fn intern_link(&mut self, id: &str, uri: &str) -> u32 {
        if uri.len() > MAX_URI || id.len() > MAX_LINK_ID {
            return 0;
        }
        let link = Link {
            id: id.to_owned(),
            uri: uri.to_owned(),
        };
        if let Some(&n) = self.link_map.get(&link) {
            return n;
        }
        self.misses += 1;
        if self.links.len() >= MAX_LINKS || self.link_bytes >= MAX_LINK_BYTES {
            return 0;
        }
        self.link_bytes += id.len() + uri.len();
        self.links.push(link.clone());
        let n = self.links.len() as u32;
        self.link_map.insert(link, n);
        n
    }

    /// The link for a [`Style::link`] id; `None` for 0 or an unknown id.
    pub fn link(&self, n: u32) -> Option<&Link> {
        self.links.get((n as usize).checked_sub(1)?)
    }

    /// True once a new style or link might not fit and enough new ones
    /// were asked for since the last compaction to make another worth its
    /// cost. The terminal should then call [`Styles::compact`] before
    /// interning more. When every stored style is still in use a
    /// compaction frees nothing, and repeating it for each new style would
    /// rescan the whole scrollback every time.
    pub fn wants_compact(&self) -> bool {
        let full = self.list.len() > usize::from(u16::MAX)
            || self.links.len() >= MAX_LINKS
            || self.link_bytes >= MAX_LINK_BYTES;
        full && self.misses >= COMPACT_MISSES
    }

    /// Drops every style not in `live` and every link no kept style uses,
    /// then renumbers both.
    ///
    /// `live` must yield every style id still stored anywhere: cells on
    /// both screens and in scrollback, the cursor and saved cursors.
    /// Returns a table from old to new style id (dropped ids map to 0);
    /// the caller rewrites every stored id through it. A [`Style`] value
    /// copied out of the table has a stale `link` afterwards, so re-read it
    /// with [`Styles::get`].
    pub fn compact(&mut self, live: impl IntoIterator<Item = u16>) -> Vec<u16> {
        let mut keep = vec![false; self.list.len()];
        for id in live {
            if let Some(k) = keep.get_mut(usize::from(id)) {
                *k = true;
            }
        }
        let old = std::mem::take(&mut self.list);
        let old_links = std::mem::take(&mut self.links);
        *self = Self::new();
        let map = old
            .into_iter()
            .zip(keep)
            .map(|(mut s, keep)| {
                if !keep {
                    return 0;
                }
                s.link = match (s.link as usize)
                    .checked_sub(1)
                    .and_then(|i| old_links.get(i))
                {
                    Some(l) => self.intern_link(&l.id, &l.uri),
                    None => 0,
                };
                self.intern(s)
            })
            .collect();
        self.misses = 0;
        map
    }
}

/// rustc's Fx hash: a rotate and multiply per word. For hot tables keyed
/// by small values, where SipHash costs more than the work around it: the
/// style table, looked up on every SGR, and the renderer's glyph atlas,
/// looked up for every cell drawn. Both are bounded, so colliding keys can
/// cost a program no more than a full table does.
#[derive(Default)]
pub struct FxHasher(u64);

/// Builds [`FxHasher`]s, for `HashMap::with_hasher`.
pub type FxBuild = std::hash::BuildHasherDefault<FxHasher>;

impl Hasher for FxHasher {
    fn write(&mut self, bytes: &[u8]) {
        let (words, tail) = bytes.as_chunks::<8>();
        for w in words {
            self.write_u64(u64::from_le_bytes(*w));
        }
        for &b in tail {
            self.write_u64(u64::from(b));
        }
    }

    fn write_u8(&mut self, n: u8) {
        self.write_u64(u64::from(n));
    }

    fn write_u16(&mut self, n: u16) {
        self.write_u64(u64::from(n));
    }

    fn write_u32(&mut self, n: u32) {
        self.write_u64(u64::from(n));
    }

    fn write_u64(&mut self, n: u64) {
        self.0 = (self.0.rotate_left(5) ^ n).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }

    fn write_usize(&mut self, n: usize) {
        self.write_u64(n as u64);
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

/// Distinct hyperlinks kept between compactions. A program that emits a
/// fresh URI for every line would otherwise grow the table forever.
const MAX_LINKS: usize = 1 << 16;

/// Total id and URI bytes kept between compactions. Each link is stored
/// twice, in the list and as its map key.
const MAX_LINK_BYTES: usize = 4 << 20;

/// Longest URI and id linked, as in VTE. Longer ones are shown unlinked.
const MAX_URI: usize = 2083;
const MAX_LINK_ID: usize = 250;

/// New styles and links that must be asked for between two compactions of
/// a full table; until then they fall back to the default style or no
/// link.
const COMPACT_MISSES: usize = 4096;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intern_dedups_and_default_is_zero() {
        let mut s = Styles::new();
        let red = Style {
            fg: Color::Idx(1),
            ..Style::default()
        };
        assert_eq!(s.intern(Style::default()), 0);
        let id = s.intern(red);
        assert_eq!(id, 1);
        assert_eq!(s.intern(red), id);
        assert_eq!(*s.get(id), red);
        assert_eq!(*s.get(999), Style::default());
    }

    #[test]
    fn links_dedup_by_id_and_uri() {
        let mut s = Styles::new();
        let a = s.intern_link("", "https://example.com/a");
        assert_eq!(a, 1);
        assert_eq!(s.intern_link("", "https://example.com/a"), a);
        let b = s.intern_link("x", "https://example.com/a");
        assert_ne!(b, a);
        assert_eq!(s.link(b).map(|l| l.id.as_str()), Some("x"));
        assert_eq!(
            s.link(a).map(|l| l.uri.as_str()),
            Some("https://example.com/a")
        );
        assert_eq!(s.link(0), None);
        assert_eq!(s.link(99), None);
    }

    fn fg(n: u8) -> Style {
        Style {
            fg: Color::Idx(n),
            ..Style::default()
        }
    }

    #[test]
    fn compact_keeps_live_styles_and_their_links() {
        let mut s = Styles::new();
        let dead_link = s.intern_link("", "https://example.com/dead");
        let live_link = s.intern_link("", "https://example.com/live");
        let a = s.intern(fg(1));
        let b = s.intern(Style {
            link: dead_link,
            ..fg(2)
        });
        let c = s.intern(Style {
            link: live_link,
            ..fg(3)
        });

        let map = s.compact([0, c, c]);
        assert_eq!(map.len(), 4);
        assert_eq!((map[0], map[a as usize], map[b as usize]), (0, 0, 0));
        let c2 = map[c as usize];
        assert_eq!(c2, 1);
        let kept = *s.get(c2);
        assert_eq!(kept.fg, Color::Idx(3));
        assert_eq!(kept.link, 1);
        assert_eq!(
            s.link(1).map(|l| l.uri.as_str()),
            Some("https://example.com/live")
        );
        assert_eq!(s.link(2), None, "unused link dropped");
        // The table works as before after compaction.
        assert_eq!(s.intern(kept), c2);
        assert_eq!(s.intern(fg(4)), 2);
    }

    #[test]
    fn full_table_falls_back_to_default_until_compacted() {
        let mut s = Styles::new();
        for i in 0..u16::MAX {
            let [hi, lo] = i.to_be_bytes();
            s.intern(Style {
                fg: Color::Rgb(hi, lo, 0),
                ..Style::default()
            });
        }
        assert!(s.wants_compact());
        assert_eq!(s.intern(fg(1)), 0);

        let keep = s.intern(Style {
            fg: Color::Rgb(0, 7, 0),
            ..Style::default()
        });
        let map = s.compact([keep]);
        assert!(!s.wants_compact());
        assert_eq!(map[keep as usize], 1);
        assert_eq!(s.intern(fg(1)), 2);
    }

    #[test]
    fn compaction_that_frees_nothing_is_not_repeated_at_once() {
        let mut s = Styles::new();
        let rgb = |i: u16, b| {
            let [hi, lo] = i.to_be_bytes();
            Style {
                fg: Color::Rgb(hi, lo, b),
                ..Style::default()
            }
        };
        let live: Vec<u16> = (0..u16::MAX).map(|i| s.intern(rgb(i, 0))).collect();
        assert!(s.wants_compact());
        s.compact(live);
        // Still full, but scanning again right away would free nothing.
        assert!(!s.wants_compact());
        assert_eq!(s.intern(Style::default()), 0);
        assert_eq!(s.intern(rgb(5, 0)), 6);
        for i in 1..COMPACT_MISSES as u16 {
            assert_eq!(s.intern(rgb(i, 1)), 0);
            assert!(!s.wants_compact());
        }
        s.intern(rgb(0, 1));
        assert!(s.wants_compact());
    }

    #[test]
    fn link_text_is_bounded() {
        let mut s = Styles::new();
        let long = "a".repeat(MAX_URI + 1);
        assert_eq!(s.intern_link("", &long), 0);
        assert_eq!(s.intern_link(&long[..MAX_LINK_ID + 1], "u"), 0);
        assert_eq!(s.intern_link(&long[..MAX_LINK_ID], &long[..MAX_URI]), 1);

        // Unused links fill the byte budget long before the count cap.
        let mut n = 1;
        while s.intern_link("", &format!("{n:08}{}", &long[8..MAX_URI])) != 0 {
            n += 1;
        }
        assert!(n * MAX_URI < MAX_LINK_BYTES + 2 * MAX_URI, "{n}");
        for _ in 0..COMPACT_MISSES {
            assert_eq!(s.intern_link("", "https://example.com/"), 0);
        }
        assert!(s.wants_compact());
        s.compact([]);
        assert_eq!(s.intern_link("", "https://example.com/"), 1);
    }
}
