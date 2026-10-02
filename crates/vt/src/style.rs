//! Cell styles, interned so a cell stores a 16-bit id, and the OSC 8
//! hyperlinks they point to.

use std::collections::HashMap;

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
    map: HashMap<Style, u16>,
    /// Link `n` is `links[n - 1]`.
    links: Vec<Link>,
    link_map: HashMap<Link, u32>,
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
            map: HashMap::from([(d, 0)]),
            links: Vec::new(),
            link_map: HashMap::new(),
        }
    }

    pub fn intern(&mut self, s: Style) -> u16 {
        if let Some(&id) = self.map.get(&s) {
            return id;
        }
        // A full table falls back to the default style.
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
    /// The same `id` and `uri` always give the same link id.
    pub fn intern_link(&mut self, id: &str, uri: &str) -> u32 {
        let link = Link {
            id: id.to_owned(),
            uri: uri.to_owned(),
        };
        if let Some(&n) = self.link_map.get(&link) {
            return n;
        }
        self.links.push(link.clone());
        let n = self.links.len() as u32;
        self.link_map.insert(link, n);
        n
    }

    /// The link for a [`Style::link`] id; `None` for 0 or an unknown id.
    pub fn link(&self, n: u32) -> Option<&Link> {
        self.links.get((n as usize).checked_sub(1)?)
    }
}

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
}
