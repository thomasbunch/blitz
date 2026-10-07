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

/// Longest grapheme tail kept per cell, in bytes, so that with its first
/// code point a cluster fits a [`RenderCell`](crate::RenderCell). Anything
/// past it is a stream of combining marks nobody can render anyway.
const MAX_GRAPHEME_TAIL: usize = crate::snapshot::CLUSTER_BYTES - 4;

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
            Some(g) if g.1.len() + c.len_utf8() <= MAX_GRAPHEME_TAIL => g.1.push(c),
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

    /// Changes the size without rewrapping. When rows shrink, blank rows
    /// below the cursor go first, then rows from the top move into
    /// scrollback; only once the cursor is on top do rows below it go.
    /// Returns the cursor's new row.
    pub fn resize(&mut self, cols: u16, lines: u16, cursor_y: u16) -> u16 {
        let mut y = cursor_y.min(self.lines - 1);
        if lines < self.lines {
            let need = self.lines - lines;
            let blank = (y + 1..self.lines)
                .rev()
                .take_while(|&r| self.row(r).cells.iter().all(|c| c.cp == 0 && c.flags == 0))
                .count() as u16;
            let up = (need - need.min(blank)).min(y);
            for _ in 0..need - up {
                if let Some(r) = self.rows.pop_back() {
                    self.pool.push(r);
                }
            }
            y -= up;
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

    /// Rewraps every row at a new width, keeping the line count: rows
    /// joined by soft wraps split again at `cols`. Blank space past a
    /// line's text is kept only up to the end of its last new row. `cur`
    /// is the cursor's screen column, row and pending wrap; returns where
    /// it lands. Blank rows below the cursor go before rows above it move
    /// into scrollback. Needs `cols >= 2` so a wide character fits a row.
    // ponytail: walks all scrollback on each width change; rows whose line
    // already fits are moved, not copied.
    pub fn reflow(&mut self, cols: u16, cur: (u16, u16, bool)) -> (u16, u16, bool) {
        let new = usize::from(cols);
        let cy = self.scrollback_len() + usize::from(cur.1);
        let old = std::mem::take(&mut self.rows);
        let last = old.len() - 1;
        self.cols = cols;
        let text_len = |cells: &[Cell]| {
            (cells.iter())
                .rposition(|c| c.cp != 0 || c.flags != 0)
                .map_or(0, |t| t + 1)
        };
        let mut out = VecDeque::with_capacity(old.len());
        let mut at = (0, 0);
        let mut line = Vec::new();
        let mut graphemes = Vec::new();
        let mut cursor = None;
        for (i, mut row) in old.into_iter().enumerate() {
            let wrapped = row.flags & rf::WRAPPED != 0 && i < last;
            if line.is_empty() && !wrapped && i != cy && text_len(&row.cells) <= new {
                row.flags = 0;
                row.set_width(cols);
                out.push_back(row);
                continue;
            }
            // Tails in column order, taken as their cells are, so a line
            // of many clusters costs no more than one of plain text.
            let mut tails = row.extra.take().map(|e| e.graphemes).unwrap_or_default();
            tails.sort_unstable_by_key(|g| g.0);
            let mut tails = tails.into_iter().peekable();
            for (x, c) in row.cells.iter().enumerate() {
                if i == cy && x == usize::from(cur.0) {
                    cursor = Some(line.len());
                }
                // Left where a wide character did not fit; placed anew below.
                if c.has(cf::SPACER_HEAD) {
                    continue;
                }
                while tails.next_if(|g| usize::from(g.0) < x).is_some() {}
                if let Some((_, g)) = tails.next_if(|g| usize::from(g.0) == x)
                    && c.has(cf::GRAPHEME)
                {
                    graphemes.push((line.len(), g));
                }
                line.push(*c);
            }
            self.pool.push(row);
            if wrapped {
                continue;
            }
            let len = text_len(&line);
            let keep = line.len().min(len.div_ceil(new).max(1) * new).max(len);
            line.resize(keep.max(cursor.map_or(0, |c| c + 1)), Cell::default());
            let mut row = self.fresh(Cell::default());
            let mut x = 0;
            let mut tails = graphemes.drain(..).peekable();
            for (k, &c) in line.iter().enumerate() {
                if x == new || (c.has(cf::WIDE) && x + 1 == new) {
                    if x < new {
                        row.cells[x].flags = cf::SPACER_HEAD;
                    }
                    row.flags |= rf::WRAPPED;
                    let next = self.fresh(Cell::default());
                    out.push_back(std::mem::replace(&mut row, next));
                    x = 0;
                }
                row.cells[x] = c;
                if let Some((_, g)) = tails.next_if(|g| g.0 == k) {
                    let ex = row.extra.get_or_insert_with(Default::default);
                    ex.graphemes.push((x as u16, g));
                }
                if cursor == Some(k) {
                    at = (out.len(), x);
                }
                x += 1;
            }
            out.push_back(row);
            line.clear();
            cursor = None;
        }

        let (mut x, mut pending) = (at.1, cur.2);
        if pending && x + 1 < new {
            (x, pending) = (x + 1, false);
        }
        let lines = usize::from(self.lines);
        let blank = |r: &Row| r.cells.iter().all(|c| c.cp == 0 && c.flags == 0);
        while out.len() > lines && out.len() - 1 > at.0 && out.back().is_some_and(blank) {
            self.pool.extend(out.pop_back());
        }
        while out.len() < lines {
            out.push_back(self.fresh(Cell::default()));
        }
        let top = (out.len() - lines).min(at.0);
        while out.len() > top + lines {
            self.pool.extend(out.pop_back());
        }
        self.rows = out;
        self.trim();
        // Narrowing built every rewrapped row before the excess went; keep
        // only as many spare rows as the grid can hold.
        self.pool.truncate(self.max_scrollback + lines);
        self.pool.shrink_to_fit();
        self.rows.shrink_to_fit();
        (x as u16, (at.0 - top) as u16, pending)
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
    fn shrinking_moves_the_top_to_scrollback_before_dropping_text() {
        // Text below the cursor stays; the rows above it make room.
        let mut g = grid_with(&["a", "b", "c", "d"]);
        assert_eq!(g.resize(4, 3, 1), 0);
        assert_eq!(text(&g), ["a", "b", "c", "d"]);
        assert_eq!(g.scrollback_len(), 1);
        // Only when the cursor is already on top does text below it go,
        // so the cursor stays on screen.
        let mut g = grid_with(&["a", "b", "c", "d"]);
        assert_eq!(g.resize(4, 2, 0), 0);
        assert_eq!(text(&g), ["a", "b"]);
        // Blank rows at the bottom go first.
        let mut g = grid_with(&["a", "b", "c", ""]);
        assert_eq!(g.resize(4, 2, 1), 0);
        assert_eq!(text(&g), ["a", "b", "c"]);
        assert_eq!(g.scrollback_len(), 1);
    }

    /// A long line full of clusters rewraps in time linear in its length,
    /// each mark staying on its own character.
    #[test]
    fn reflow_of_a_long_clustered_line_is_linear() {
        let mark = |k: usize| {
            if k.is_multiple_of(3) {
                "\u{300}"
            } else {
                "\u{301}"
            }
        };
        let mut g = Grid::new(100, 4000, 0);
        for y in 0..4000u16 {
            let r = g.row_mut(y);
            r.put_ascii(0, &[b'e'; 100], 0);
            for x in 0..100u16 {
                let k = usize::from(y) * 100 + usize::from(x);
                r.push_grapheme(x, mark(k).chars().next().unwrap());
            }
            r.flags |= rf::WRAPPED;
        }
        let t0 = std::time::Instant::now();
        g.reflow(99, (0, 0, false));
        assert!(t0.elapsed().as_secs() < 5, "{:?}", t0.elapsed());
        for y in 0..4000u16 {
            for x in 0..99u16 {
                let k = usize::from(y) * 99 + usize::from(x);
                assert_eq!(g.row(y).grapheme(x), Some(mark(k)), "{x},{y}");
            }
        }
    }

    /// Narrowing builds every rewrapped row before the excess is dropped;
    /// none of that may stay allocated afterwards.
    #[test]
    fn narrow_then_wide_reflow_frees_the_temporary_rows() {
        let mut g = Grid::new(200, 10, 1000);
        for _ in 0..1010 {
            g.row_mut(9).put_ascii(0, &[b'x'; 200], 0);
            g.scroll_up(0, 9, 1, Cell::default(), true);
        }
        let before = g.bytes_used();
        g.reflow(2, (0, 9, false));
        assert!(
            g.bytes_used() < 2 * before,
            "{} vs {before}",
            g.bytes_used()
        );
        g.reflow(200, (0, 9, false));
        assert!(
            g.bytes_used() < 2 * before,
            "{} vs {before}",
            g.bytes_used()
        );
    }

    /// A tail never passes 28 bytes, so with the first code point a cluster
    /// fits the snapshot's 32.
    #[test]
    fn grapheme_tails_stop_at_the_byte_cap() {
        let mut r = Row::new(2);
        r.put_ascii(0, b"ab", 0);
        for _ in 0..20 {
            r.push_grapheme(0, '\u{20D0}');
            r.push_grapheme(1, '\u{301}');
        }
        // Nine three-byte marks; a tenth, or a two-byte one, would pass 28.
        assert_eq!(r.grapheme(0), Some("\u{20D0}".repeat(9).as_str()));
        r.push_grapheme(0, '\u{301}');
        assert_eq!(r.grapheme(0).map(str::len), Some(27));
        assert_eq!(r.grapheme(1), Some("\u{301}".repeat(14).as_str()));
        // Past the row's end nothing happens.
        r.push_grapheme(9, '\u{301}');
        assert_eq!(r.extra.as_ref().map(|e| e.graphemes.len()), Some(2));
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
