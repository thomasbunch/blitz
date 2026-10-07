//! A resolved, render-ready copy of the visible screen.

use crate::style::Color;

/// Colours as `0xRRGGBB`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub fg: u32,
    pub bg: u32,
    pub cursor: u32,
    pub selection_bg: u32,
    pub ansi: [u32; 16],
}

impl Palette {
    /// `c` as `0xRRGGBB`, with `default` for [`Color::Default`]. Indexes
    /// past 15 use xterm's 6x6x6 cube and grey ramp.
    pub fn resolve(&self, c: Color, default: u32) -> u32 {
        let rgb = |r: u32, g: u32, b: u32| r << 16 | g << 8 | b;
        match c {
            Color::Default => default,
            Color::Idx(i @ 0..16) => self.ansi[i as usize],
            Color::Idx(i @ 16..232) => {
                let level = |v: u8| if v == 0 { 0 } else { 55 + 40 * u32::from(v) };
                let i = i - 16;
                rgb(level(i / 36), level(i / 6 % 6), level(i % 6))
            }
            Color::Idx(i) => {
                let v = 8 + 10 * u32::from(i - 232);
                rgb(v, v, v)
            }
            Color::Rgb(r, g, b) => rgb(r.into(), g.into(), b.into()),
        }
    }
}

/// [`RenderCell::attrs`] bits. Inverse and invisible are already applied
/// to the cell's colours, and a cell whose text would not show has none;
/// `INVERSE` is only informational.
pub mod attr {
    pub const BOLD: u16 = 1 << 0;
    pub const ITALIC: u16 = 1 << 1;
    pub const UNDERLINE: u16 = 1 << 2;
    pub const INVERSE: u16 = 1 << 3;
    pub const DIM: u16 = 1 << 4;
    pub const STRIKE: u16 = 1 << 5;
    pub const OVERLINE: u16 = 1 << 6;
}

/// Longest grapheme cluster a cell holds, in bytes: its first code point
/// and up to 28 bytes after it. Enough for a family or a subdivision flag.
pub const CLUSTER_BYTES: usize = 32;

/// One cell with its colours already resolved through the palette.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RenderCell {
    /// UTF-8 of the grapheme cluster; `len` bytes are valid.
    pub text: [u8; CLUSTER_BYTES],
    pub len: u8,
    /// 0 for the right half of a wide character, else 1 or 2.
    pub width: u8,
    pub fg: u32,
    pub bg: u32,
    pub attrs: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorShape {
    Block,
    Bar,
    Underline,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub cols: u16,
    pub rows: u16,
    /// `rows * cols` cells, row-major, viewport only.
    pub cells: Vec<RenderCell>,
    /// Per row: its text continues on the next row.
    pub wrapped: Vec<bool>,
    /// Column, row and shape; `None` when hidden or scrolled out of view.
    pub cursor: Option<(u16, u16, CursorShape)>,
    pub alt_screen: bool,
    /// Start and end (column, row), inclusive.
    pub selection: Option<((u16, u16), (u16, u16))>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Options, Terminal};

    #[test]
    fn line_attributes_reach_the_snapshot() {
        let pal = Palette {
            fg: 0xffffff,
            bg: 0,
            cursor: 0xff0000,
            selection_bg: 0x00ff00,
            ansi: [0x808080; 16],
        };
        let mut t = Terminal::new(Options {
            cols: 6,
            rows: 1,
            ..Options::default()
        });
        t.feed(b"\x1b[9ma\x1b[53mb\x1b[29mc\x1b[55;4md\x1b[0;9;53;4me\x1b[0mf");
        let mut s = Snapshot::default();
        t.snapshot(&mut s, &pal);
        let attrs: Vec<u16> = s.cells.iter().map(|c| c.attrs).collect();
        let (st, ov, ul) = (attr::STRIKE, attr::OVERLINE, attr::UNDERLINE);
        assert_eq!(attrs, [st, st | ov, ov, ul, st | ov | ul, 0]);
        // Each bit is its own.
        let bits = [
            attr::BOLD,
            attr::ITALIC,
            attr::UNDERLINE,
            attr::INVERSE,
            attr::DIM,
            attr::STRIKE,
            attr::OVERLINE,
        ];
        let all = bits.iter().fold(0, |a, b| a | b);
        assert_eq!(all.count_ones() as usize, bits.len());
    }
}
