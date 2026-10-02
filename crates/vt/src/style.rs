//! Cell styles, interned so a cell stores a 16-bit id.

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
    /// Hyperlink id, 0 for none.
    pub link: u32,
}

/// Style interner. Id 0 is always the default style.
#[derive(Debug)]
pub struct Styles {
    list: Vec<Style>,
    map: HashMap<Style, u16>,
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
        }
    }

    pub fn intern(&mut self, s: Style) -> u16 {
        if let Some(&id) = self.map.get(&s) {
            return id;
        }
        // ponytail: a full table falls back to the default style; compact
        // unused entries instead if long sessions ever hit 65536 styles.
        let Ok(id) = u16::try_from(self.list.len()) else {
            return 0;
        };
        self.list.push(s);
        self.map.insert(s, id);
        id
    }

    /// The style for `id`; unknown ids read as the default style.
    pub fn get(&self, id: u16) -> &Style {
        self.list.get(id as usize).unwrap_or(&self.list[0])
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
}
