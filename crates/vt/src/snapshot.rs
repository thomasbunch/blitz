//! A resolved, render-ready copy of the visible screen.

/// Colours as `0xRRGGBB`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub fg: u32,
    pub bg: u32,
    pub cursor: u32,
    pub selection_bg: u32,
    pub ansi: [u32; 16],
}

/// [`RenderCell::attrs`] bits.
pub mod attr {
    pub const BOLD: u16 = 1 << 0;
    pub const ITALIC: u16 = 1 << 1;
    pub const UNDERLINE: u16 = 1 << 2;
    pub const INVERSE: u16 = 1 << 3;
    pub const DIM: u16 = 1 << 4;
}

/// One cell with its colours already resolved through the palette.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RenderCell {
    /// UTF-8 of the grapheme cluster; `len` bytes are valid.
    pub text: [u8; 16],
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
    /// Column, row and shape; `None` when hidden or scrolled out of view.
    pub cursor: Option<(u16, u16, CursorShape)>,
    pub alt_screen: bool,
    /// Start and end (column, row), inclusive.
    pub selection: Option<((u16, u16), (u16, u16))>,
}
