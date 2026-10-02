//! Cell storage: rows, the scrollback ring and the recycled-row pool.

use std::collections::VecDeque;
use std::ops::Range;

/// One screen cell. Eight bytes, so 10,000 rows of 120 columns fit in
/// about 10 MB.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct Cell {
    /// Code point, or 0 for a blank (never written or erased) cell.
    pub cp: u32,
    /// Interned style id; erased cells keep the background they were erased
    /// with.
    pub style: u16,
    /// [`cf`] bits.
    pub flags: u16,
}

const _: () = assert!(size_of::<Cell>() == 8);

impl Cell {
    pub const fn blank(style: u16) -> Self {
        Self {
            cp: 0,
            style,
            flags: 0,
        }
    }

    fn has(&self, flag: u16) -> bool {
        self.flags & flag != 0
    }
}

/// [`Cell::flags`] bits.
pub mod cf {
    /// Left half of a two-column character.
    pub const WIDE: u16 = 1 << 0;
    /// Right half of a two-column character. Holds no text.
    pub const SPACER_TAIL: u16 = 1 << 1;
    /// Blank last column left behind when a wide character wrapped early.
    pub const SPACER_HEAD: u16 = 1 << 2;
    /// The cell's cluster continues in [`RowExtra::graphemes`](super::RowExtra).
    pub const GRAPHEME: u16 = 1 << 3;
}

/// [`Row::flags`] bits.
pub mod rf {
    /// The row's text continues on the next row (soft wrap).
    pub const WRAPPED: u8 = 1 << 0;
}

/// Longest grapheme tail kept per cell, in bytes. Anything past it is a
/// stream of combining marks nobody can render anyway.
const MAX_GRAPHEME_TAIL: usize = 28;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Row {
    pub cells: Vec<Cell>,
    pub extra: Option<Box<RowExtra>>,
    /// [`rf`] bits.
    pub flags: u8,
}

/// Rarely needed per-row data, boxed so ordinary rows stay small.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RowExtra {
    /// Code points after the first, for cells flagged [`cf::GRAPHEME`],
    /// keyed by column.
    pub graphemes: Vec<(u16, String)>,
}

impl Row {
    fn new(cols: u16) -> Self {
        Self {
            cells: vec![Cell::default(); cols as usize],
            extra: None,
            flags: 0,
        }
    }

    /// Blanks the whole row and sets its width, keeping its allocation.
    pub fn reset(&mut self, cols: u16, blank: Cell) {
        self.cells.clear();
        self.cells.resize(cols as usize, blank);
        self.extra = None;
        self.flags = 0;
    }

    /// The code points after the first in the cluster at `col`.
    pub fn grapheme(&self, col: u16) -> Option<&str> {
        let ex = self.extra.as_ref()?;
        ex.graphemes
            .iter()
            .find(|g| g.0 == col)
            .map(|g| g.1.as_str())
    }

    /// Appends `c` to the cluster in cell `col`.
    pub fn push_grapheme(&mut self, col: u16, c: char) {
        let Some(cell) = self.cells.get_mut(col as usize) else {
            return;
        };
        cell.flags |= cf::GRAPHEME;
        let ex = self.extra.get_or_insert_with(Default::default);
        match ex.graphemes.iter_mut().find(|g| g.0 == col) {
            Some(g) if g.1.len() < MAX_GRAPHEME_TAIL => g.1.push(c),
            Some(_) => {}
            None => ex.graphemes.push((col, c.to_string())),
        }
    }

    fn drop_graphemes(&mut self, cols: Range<usize>) {
        if let Some(ex) = &mut self.extra {
            ex.graphemes.retain(|g| !cols.contains(&(g.0 as usize)));
        }
    }

    /// Moves grapheme tails at or right of `from` by `delta` columns,
    /// dropping any that leave the row.
    fn shift_graphemes(&mut self, from: usize, delta: isize) {
        let len = self.cells.len();
        if let Some(ex) = &mut self.extra {
            ex.graphemes.retain_mut(|g| {
                let col = g.0 as usize;
                if col < from {
                    return true;
                }
                match col.checked_add_signed(delta).filter(|&c| c < len) {
                    Some(c) => {
                        g.0 = c as u16;
                        true
                    }
                    None => false,
                }
            });
        }
    }

    /// If column `at` is the right half of a wide character, blanks both
    /// halves. Called on both edges of every write so a pair is never left
    /// half overwritten.
    fn split_pair(&mut self, at: usize) {
        if at > 0 && self.cells.get(at).is_some_and(|c| c.has(cf::SPACER_TAIL)) {
            for c in &mut self.cells[at - 1..=at] {
                *c = Cell::blank(c.style);
            }
            self.drop_graphemes(at - 1..at);
        }
    }

    /// Writes `cell` at `x`; a wide cell also takes `x + 1` as its spacer.
    /// The caller guarantees the cell fits.
    pub fn put(&mut self, x: usize, cell: Cell) {
        let w = if cell.has(cf::WIDE) { 2 } else { 1 };
        self.split_pair(x);
        self.split_pair(x + w);
        if self.extra.is_some() {
            self.drop_graphemes(x..x + w);
        }
        self.cells[x] = cell;
        if w == 2 {
            self.cells[x + 1] = Cell {
                cp: 0,
                style: cell.style,
                flags: cf::SPACER_TAIL,
            };
        }
    }

    /// Turns the narrow cell at `x` into a wide one, taking `x + 1` as its
    /// spacer. Does nothing at the last column.
    pub fn widen(&mut self, x: usize) {
        if x + 1 >= self.cells.len() {
            return;
        }
        self.split_pair(x + 2);
        self.drop_graphemes(x + 1..x + 2);
        let style = self.cells[x].style;
        self.cells[x].flags |= cf::WIDE;
        self.cells[x + 1] = Cell {
            cp: 0,
            style,
            flags: cf::SPACER_TAIL,
        };
    }

    /// Writes a run of printable ASCII starting at `x`. The caller guarantees
    /// it fits.
    pub fn put_ascii(&mut self, x: usize, text: &[u8], style: u16) {
        let end = x + text.len();
        self.split_pair(x);
        self.split_pair(end);
        if self.extra.is_some() {
            self.drop_graphemes(x..end);
        }
        for (c, &b) in self.cells[x..end].iter_mut().zip(text) {
            *c = Cell {
                cp: u32::from(b),
                style,
                flags: 0,
            };
        }
    }

    /// Overwrites `cols` (clamped to the row) with `cell`.
    pub fn fill(&mut self, cols: Range<usize>, cell: Cell) {
        let end = cols.end.min(self.cells.len());
        let start = cols.start.min(end);
        if start == end {
            return;
        }
        self.split_pair(start);
        self.split_pair(end);
        self.cells[start..end].fill(cell);
        self.drop_graphemes(start..end);
    }

    /// Inserts `n` blank cells at `x`, pushing the rest right; cells pushed
    /// past the end are lost.
    pub fn insert(&mut self, x: usize, n: usize, blank: Cell) {
        let len = self.cells.len();
        if x >= len {
            return;
        }
        let n = n.min(len - x);
        self.split_pair(x);
        self.split_pair(len - n);
        self.drop_graphemes(len - n..len);
        self.shift_graphemes(x, n as isize);
        self.cells[x..].rotate_right(n);
        self.cells[x..x + n].fill(blank);
    }

    /// Deletes `n` cells at `x`, pulling the rest left and filling the end
    /// with blanks.
    pub fn delete(&mut self, x: usize, n: usize, blank: Cell) {
        let len = self.cells.len();
        if x >= len {
            return;
        }
        let n = n.min(len - x);
        self.split_pair(x);
        self.split_pair(x + n);
        self.drop_graphemes(x..x + n);
        self.shift_graphemes(x + n, -(n as isize));
        self.cells[x..].rotate_left(n);
        self.cells[len - n..].fill(blank);
    }

    fn set_width(&mut self, cols: u16) {
        let cols = cols as usize;
        if cols < self.cells.len() {
            self.split_pair(cols);
            self.cells.truncate(cols);
            self.drop_graphemes(cols..usize::MAX);
        } else {
            if let Some(last) = self.cells.last_mut() {
                last.flags &= !cf::SPACER_HEAD;
            }
            self.cells.resize(cols, Cell::default());
        }
    }

    /// Appends the row's text: blanks as spaces, wide characters once.
    pub fn push_text(&self, out: &mut String) {
        for (x, c) in self.cells.iter().enumerate() {
            if c.has(cf::SPACER_TAIL | cf::SPACER_HEAD) {
                continue;
            }
            out.push(match c.cp {
                0 => ' ',
                cp => char::from_u32(cp).unwrap_or(char::REPLACEMENT_CHARACTER),
            });
            if c.has(cf::GRAPHEME) {
                out.push_str(self.grapheme(x as u16).unwrap_or_default());
            }
        }
    }
}

/// The rows of one screen: scrollback first, then the `lines` screen rows.
#[derive(Debug)]
pub struct Grid {
    rows: VecDeque<Row>,
    /// Rows dropped from the screen or scrollback, reused before allocating.
    pool: Vec<Row>,
    cols: u16,
    lines: u16,
    max_scrollback: usize,
}

impl Grid {
    pub fn new(cols: u16, lines: u16, max_scrollback: usize) -> Self {
        Self {
            rows: (0..lines).map(|_| Row::new(cols)).collect(),
            pool: Vec::new(),
            cols,
            lines,
            max_scrollback,
        }
    }

    pub fn cols(&self) -> u16 {
        self.cols
    }

    pub fn lines(&self) -> u16 {
        self.lines
    }

    pub fn scrollback_len(&self) -> usize {
        self.rows.len() - self.lines as usize
    }

    /// Screen row `y`.
    pub fn row(&self, y: u16) -> &Row {
        &self.rows[self.scrollback_len() + y as usize]
    }

    pub fn row_mut(&mut self, y: u16) -> &mut Row {
        let i = self.scrollback_len() + y as usize;
        &mut self.rows[i]
    }

    /// Row `i` counting from the oldest scrollback row.
    pub fn line(&self, i: usize) -> Option<&Row> {
        self.rows.get(i)
    }

    /// Every cell, scrollback included.
    pub fn cells(&self) -> impl Iterator<Item = &Cell> {
        self.rows.iter().flat_map(|r| &r.cells)
    }

    pub fn cells_mut(&mut self) -> impl Iterator<Item = &mut Cell> {
        self.rows.iter_mut().flat_map(|r| &mut r.cells)
    }

    fn fresh(&mut self, blank: Cell) -> Row {
        let mut r = self.pool.pop().unwrap_or_default();
        r.reset(self.cols, blank);
        r
    }

    /// Scrolls screen rows `top..=bottom` up by `n`; blank rows enter at the
    /// bottom. With `keep`, the rows leaving at the top go to scrollback
    /// instead of being dropped, which is only meaningful when `top` is 0.
    pub fn scroll_up(&mut self, top: u16, bottom: u16, n: u16, blank: Cell, keep: bool) {
        let n = n.min(bottom + 1 - top);
        for _ in 0..n {
            let base = self.scrollback_len();
            if keep && self.max_scrollback > 0 {
                // Inserting below the region shifts only the rows under it;
                // the screen window then starts one row later, so the old
                // top row becomes the newest scrollback row.
                let row = self.fresh(blank);
                self.rows.insert(base + bottom as usize + 1, row);
            } else if let Some(mut row) = self.rows.remove(base + top as usize) {
                row.reset(self.cols, blank);
                self.rows.insert(base + bottom as usize, row);
            }
        }
        self.trim();
    }

    /// Scrolls screen rows `top..=bottom` down by `n`; blank rows enter at
    /// the top.
    pub fn scroll_down(&mut self, top: u16, bottom: u16, n: u16, blank: Cell) {
        let n = n.min(bottom + 1 - top);
        let base = self.scrollback_len();
        for _ in 0..n {
            if let Some(mut row) = self.rows.remove(base + bottom as usize) {
                row.reset(self.cols, blank);
                self.rows.insert(base + top as usize, row);
            }
        }
    }

    /// Drops all scrollback, keeping the rows for reuse.
    pub fn clear_scrollback(&mut self) {
        let n = self.scrollback_len();
        self.pool.extend(self.rows.drain(..n));
    }

    fn trim(&mut self) {
        let excess = self.scrollback_len().saturating_sub(self.max_scrollback);
        self.pool.extend(self.rows.drain(..excess));
    }

    /// Changes the size without rewrapping. When rows shrink, blank space
    /// below the cursor goes first, then rows from the top move into
    /// scrollback. Returns the cursor's new row.
    pub fn resize(&mut self, cols: u16, lines: u16, cursor_y: u16) -> u16 {
        let mut y = cursor_y.min(self.lines - 1);
        if lines < self.lines {
            let drop = (self.lines - lines).min(self.lines - 1 - y);
            for _ in 0..drop {
                if let Some(r) = self.rows.pop_back() {
                    self.pool.push(r);
                }
            }
            y -= self.lines - drop - lines;
        } else {
            for _ in self.lines..lines {
                let row = self.fresh(Cell::default());
                self.rows.push_back(row);
            }
        }
        self.lines = lines;
        self.cols = cols;
        let base = self.scrollback_len();
        for r in self.rows.range_mut(base..) {
            r.set_width(cols);
        }
        self.trim();
        y
    }

    /// Heap bytes held by rows, scrollback and the pool.
    pub fn bytes_used(&self) -> usize {
        let row = |r: &Row| {
            r.cells.capacity() * size_of::<Cell>()
                + r.extra.as_ref().map_or(0, |e| {
                    size_of::<RowExtra>()
                        + e.graphemes.capacity() * size_of::<(u16, String)>()
                        + e.graphemes.iter().map(|g| g.1.capacity()).sum::<usize>()
                })
        };
        (self.rows.capacity() + self.pool.capacity()) * size_of::<Row>()
            + self.rows.iter().chain(&self.pool).map(row).sum::<usize>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(g: &Grid) -> Vec<String> {
        (0..g.rows.len())
            .map(|i| {
                let mut s = String::new();
                g.line(i).unwrap().push_text(&mut s);
                s.trim_end().to_string()
            })
            .collect()
    }

    fn grid_with(rows: &[&str]) -> Grid {
        let mut g = Grid::new(4, rows.len() as u16, 10);
        for (y, s) in rows.iter().enumerate() {
            g.row_mut(y as u16).put_ascii(0, s.as_bytes(), 0);
        }
        g
    }

    #[test]
    fn scroll_up_keeps_rows_below_the_region() {
        let mut g = grid_with(&["a", "b", "c", "S"]);
        g.scroll_up(0, 2, 1, Cell::default(), true);
        assert_eq!(text(&g), ["a", "b", "c", "", "S"]);
        assert_eq!(g.scrollback_len(), 1);

        let mut g = grid_with(&["a", "b", "c", "S"]);
        g.scroll_up(1, 2, 1, Cell::default(), false);
        assert_eq!(text(&g), ["a", "c", "", "S"]);
        g.scroll_down(0, 2, 1, Cell::default());
        assert_eq!(text(&g), ["", "a", "c", "S"]);
    }

    #[test]
    fn scrollback_is_capped_and_cleared_into_the_pool() {
        let mut g = grid_with(&["a", "b"]);
        for _ in 0..25 {
            g.scroll_up(0, 1, 1, Cell::default(), true);
        }
        assert_eq!(g.scrollback_len(), 10);
        g.clear_scrollback();
        assert_eq!(g.scrollback_len(), 0);
        assert!(g.pool.len() >= 10);
    }

    #[test]
    fn writes_never_leave_half_a_wide_pair() {
        let mut r = Row::new(4);
        r.put(
            1,
            Cell {
                cp: '中' as u32,
                style: 0,
                flags: cf::WIDE,
            },
        );
        r.put_ascii(2, b"x", 0);
        let mut s = String::new();
        r.push_text(&mut s);
        assert_eq!(s, "  x ");
        assert!(r.cells.iter().all(|c| c.flags == 0));
    }

    #[test]
    fn insert_and_delete_move_grapheme_tails() {
        let mut r = Row::new(4);
        r.put_ascii(0, b"ab", 0);
        r.push_grapheme(1, '\u{301}');
        r.insert(0, 1, Cell::default());
        assert_eq!(r.grapheme(2), Some("\u{301}"));
        r.delete(0, 2, Cell::default());
        assert_eq!(r.grapheme(0), Some("\u{301}"));
        r.insert(0, 4, Cell::default());
        assert!(r.extra.as_ref().unwrap().graphemes.is_empty());
    }

    #[test]
    fn shrinking_drops_blank_rows_under_the_cursor_first() {
        let mut g = grid_with(&["a", "b", "", ""]);
        assert_eq!(g.resize(4, 3, 1), 1);
        assert_eq!(text(&g), ["a", "b", ""]);
        assert_eq!(g.resize(2, 2, 2), 1);
        assert_eq!(text(&g), ["a", "b", ""]);
        assert_eq!(g.scrollback_len(), 1);
    }

    #[test]
    fn ten_thousand_rows_of_120_columns_fit_in_10_mib() {
        let mut g = Grid::new(120, 50, 10_000);
        for _ in 0..12_000 {
            g.scroll_up(0, 49, 1, Cell::default(), true);
        }
        assert_eq!(g.scrollback_len(), 10_000);
        let used = g.bytes_used();
        assert!(used <= 10 << 20, "{used} bytes");
    }
}
