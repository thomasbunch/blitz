//! A resolved, render-ready copy of the visible screen.

use crate::grid::Found;
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

/// [`RenderCell::attrs`] bits, the same as the style's. Inverse and
/// invisible are already applied to the cell's colours, and a cell whose
/// text would not show has none; `INVERSE` is only informational. Blink
/// is left out: nothing blitz draws moves.
pub mod attr {
    use crate::style::attr as s;

    pub const BOLD: u16 = s::BOLD;
    pub const DIM: u16 = s::DIM;
    pub const ITALIC: u16 = s::ITALIC;
    /// Underline kind as in SGR `4:x`: 0 none, 1 single, 2 double,
    /// 3 curly, 4 dotted, 5 dashed.
    pub const UNDERLINE: u16 = s::UNDERLINE;
    pub const UNDERLINE_SHIFT: u16 = s::UNDERLINE_SHIFT;
    pub const INVERSE: u16 = s::INVERSE;
    pub const STRIKE: u16 = s::STRIKE;
    pub const OVERLINE: u16 = s::OVERLINE;
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
    /// Underline colour; `fg` unless the program chose one.
    pub ul: u32,
    pub attrs: u16,
}

// Snapshots are rebuilt every frame something changes; keep cells small.
const _: () = assert!(size_of::<RenderCell>() == 48);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorShape {
    Block,
    Bar,
    Underline,
}

/// A search match to mark on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Highlight {
    /// First and last (column, row), inclusive.
    pub start: (u16, u16),
    pub end: (u16, u16),
    /// The current match, marked more strongly than the rest.
    pub current: bool,
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
    /// The cursor colour the program set with OSC 12, if any.
    pub cursor_color: Option<u32>,
    pub alt_screen: bool,
    /// Start and end (column, row), inclusive.
    pub selection: Option<((u16, u16), (u16, u16))>,
    /// The first row's line, numbered as [`crate::grid::Grid::dropped`]
    /// numbers them.
    pub top: usize,
    /// Search matches to mark, in reading order.
    pub highlights: Vec<Highlight>,
    /// The selection is a block: start and end are its corners, and it
    /// takes the same columns of every row between them.
    pub block: bool,
    /// A link under the pointer, drawn underlined: start and end (column,
    /// row), inclusive.
    pub hover: Option<((u16, u16), (u16, u16))>,
}

impl Snapshot {
    /// Sets [`Self::highlights`] to the matches of `found` that show, with
    /// match `cur` as the current one. `found` is in order, as
    /// [`crate::grid::Grid::find`] gives it.
    pub fn highlight(&mut self, found: &[Found], cur: Option<usize>) {
        self.highlights.clear();
        let (top, bottom) = (self.top, self.top + usize::from(self.rows));
        let last = (self.cols.saturating_sub(1), self.rows.saturating_sub(1));
        let row = |line: usize| (line - top) as u16;
        let first = found.partition_point(|f| f.end.0 < top);
        for (i, f) in found.iter().enumerate().skip(first) {
            if f.start.0 >= bottom {
                break;
            }
            // A match can run off the top or the bottom of the view.
            let start = match f.start {
                (line, x) if line >= top => (x, row(line)),
                _ => (0, 0),
            };
            let end = match f.end {
                (line, x) if line < bottom => (x, row(line)),
                _ => last,
            };
            self.highlights.push(Highlight {
                start,
                end,
                current: Some(i) == cur,
            });
        }
    }
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
        // A plain SGR 4 is a single underline.
        let ul = 1 << attr::UNDERLINE_SHIFT;
        let (st, ov) = (attr::STRIKE, attr::OVERLINE);
        assert_eq!(attrs, [st, st | ov, ov, ul, st | ov | ul, 0]);
        // Each field is its own.
        let fields = [
            attr::BOLD,
            attr::ITALIC,
            attr::UNDERLINE,
            attr::INVERSE,
            attr::DIM,
            attr::STRIKE,
            attr::OVERLINE,
        ];
        for (i, a) in fields.iter().enumerate() {
            for b in &fields[i + 1..] {
                assert_eq!(a & b, 0, "{a:#x} and {b:#x}");
            }
        }
    }
}
