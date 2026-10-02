//! The terminal: screens, cursor, modes, replies and events.

use std::time::Instant;

use crate::grid::{Cell, Grid, Row, cf, rf};
use crate::modes::InputModes;
use crate::parser::{Handler, Params, Parser};
use crate::snapshot::{CursorShape, Palette, RenderCell, Snapshot, attr};
use crate::style::{Style, Styles};
use crate::width::cluster_width;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    pub cols: u16,
    pub rows: u16,
    pub scrollback_lines: usize,
    /// Treat East Asian Ambiguous characters as wide.
    pub ambiguous_wide: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            cols: 80,
            rows: 24,
            scrollback_lines: 10_000,
            ambiguous_wide: false,
        }
    }
}

/// OSC 133 shell-integration marks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptMark {
    /// Prompt start. `blitz` is set when the mark carries `blitz=1`.
    A { blitz: bool },
    /// Command input start.
    B,
    /// Command output start.
    C,
    /// Command finished, with its exit code when given.
    D(Option<i32>),
}

/// Things the host should know about, queued in stream order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Title(String),
    Bell,
    Cwd(String),
    Notify { title: String, body: String },
    Progress { state: u8, pct: Option<u8> },
    Prompt(PromptMark),
    Hyperlink,
}

/// [`Style::attrs`] bits, as set by SGR.
pub mod sgr {
    pub const BOLD: u16 = 1 << 0;
    pub const FAINT: u16 = 1 << 1;
    pub const ITALIC: u16 = 1 << 2;
    /// Underline kind, 3 bits: 0 none, 1 single, 2 double, 3 curly,
    /// 4 dotted, 5 dashed.
    pub const UNDERLINE: u16 = 0b111 << UNDERLINE_SHIFT;
    pub const UNDERLINE_SHIFT: u16 = 3;
    pub const BLINK: u16 = 1 << 6;
    pub const INVERSE: u16 = 1 << 7;
    pub const INVISIBLE: u16 = 1 << 8;
    pub const STRIKE: u16 = 1 << 9;
    pub const OVERLINE: u16 = 1 << 10;
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Charset {
    #[default]
    Ascii,
    /// DEC Special Graphics: line drawing in place of `` ` `` to `~`.
    DecGraphics,
}

/// The cursor and everything DECSC saves with it.
#[derive(Clone, Copy, Debug, Default)]
struct Cursor {
    x: u16,
    y: u16,
    /// The last column was just written; the next character wraps first.
    pending_wrap: bool,
    style: Style,
    /// `style`, interned.
    sid: u16,
    /// Style for erased cells: the background of `style` and nothing else.
    blank: u16,
    /// DECOM: rows count from the top margin.
    origin: bool,
    charsets: [Charset; 2],
    /// Which of `charsets` SO/SI selected.
    gl: usize,
}

struct Screen {
    grid: Grid,
    /// DECSC slot. Each screen has its own.
    saved: Option<Cursor>,
}

pub struct Terminal {
    opts: Options,
    parser: Parser,
    styles: Styles,
    /// The screen being drawn on.
    screen: Screen,
    /// The other one: the alternate screen while `alt` is false, the main
    /// screen while it is true.
    other: Screen,
    alt: bool,
    cur: Cursor,
    /// Scroll region, inclusive.
    top: u16,
    bottom: u16,
    tabs: Vec<bool>,
    autowrap: bool,
    insert: bool,
    /// LNM: LF also returns the carriage.
    newline: bool,
    cursor_visible: bool,
    cursor_shape: CursorShape,
    /// Last printed character, for REP.
    rep: Option<char>,
    /// Cell the last character went to, while combining marks may still
    /// join it.
    last_cell: Option<(u16, u16)>,
    /// The previous code point was a zero-width joiner.
    joiner: bool,
    /// Rows the view is scrolled back into scrollback.
    viewport: usize,
    changed: bool,
    replies: Vec<u8>,
    events: Vec<Event>,
}

impl Terminal {
    pub fn new(o: Options) -> Self {
        let (cols, rows) = (o.cols.max(1), o.rows.max(1));
        Self {
            opts: Options { cols, rows, ..o },
            parser: Parser::new(),
            styles: Styles::new(),
            screen: Screen {
                grid: Grid::new(cols, rows, o.scrollback_lines),
                saved: None,
            },
            other: Screen {
                grid: Grid::new(cols, rows, 0),
                saved: None,
            },
            alt: false,
            cur: Cursor::default(),
            top: 0,
            bottom: rows - 1,
            tabs: default_tabs(cols),
            autowrap: true,
            insert: false,
            newline: false,
            cursor_visible: true,
            cursor_shape: CursorShape::Block,
            rep: None,
            last_cell: None,
            joiner: false,
            viewport: 0,
            changed: true,
            replies: Vec::new(),
            events: Vec::new(),
        }
    }

    /// Parses `bytes`, queueing replies and events.
    pub fn feed(&mut self, bytes: &[u8]) {
        // The parser calls back into `self`, so it is moved out meanwhile.
        let mut p = std::mem::take(&mut self.parser);
        p.advance(self, bytes);
        self.parser = p;
    }

    /// Moves queued replies (DA1, CPR, OSC answers, ...) to `out`, in order.
    pub fn take_replies(&mut self, out: &mut Vec<u8>) {
        out.append(&mut self.replies);
    }

    pub fn take_events(&mut self, out: &mut Vec<Event>) {
        out.append(&mut self.events);
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        let (cols, rows) = (cols.max(1), rows.max(1));
        if (cols, rows) == (self.cols(), self.rows()) {
            return;
        }
        self.cur.y = self.screen.grid.resize(cols, rows, self.cur.y);
        let other_y = self.other.saved.map_or(0, |c| c.y);
        let other_y = self.other.grid.resize(cols, rows, other_y);
        if let Some(c) = &mut self.other.saved {
            c.y = other_y;
        }
        if cols != self.cols() {
            self.cur.pending_wrap = false;
            self.cur.x = self.cur.x.min(cols - 1);
            self.tabs = default_tabs(cols);
        }
        self.opts.cols = cols;
        self.opts.rows = rows;
        self.top = 0;
        self.bottom = rows - 1;
        self.viewport = self.viewport.min(self.screen.grid.scrollback_len());
        self.changed = true;
    }

    /// Scrolls the view of the main screen; positive is up into scrollback.
    pub fn scroll_viewport(&mut self, delta: isize) {
        let max = self.screen.grid.scrollback_len();
        self.viewport = self.viewport.saturating_add_signed(delta).min(max);
        self.changed = true;
    }

    pub fn input_modes(&self) -> InputModes {
        InputModes {
            alt_screen: self.alt,
            ..InputModes::default()
        }
    }

    /// Column, row and visibility.
    pub fn cursor(&self) -> (u16, u16, bool) {
        (self.cur.x, self.cur.y, self.cursor_visible)
    }

    /// True while a synchronized update (mode 2026) is open and recent.
    pub fn sync_pending(&self, _now: Instant) -> bool {
        false
    }

    /// Fills `out` with the visible screen. Returns whether anything changed
    /// since the last call; when nothing did, `out` is left as it was.
    pub fn snapshot(&mut self, out: &mut Snapshot, pal: &Palette) -> bool {
        let (cols, rows) = (self.cols(), self.rows());
        let n = cols as usize * rows as usize;
        if !self.changed && (out.cols, out.rows, out.cells.len()) == (cols, rows, n) {
            return false;
        }
        self.changed = false;
        out.cols = cols;
        out.rows = rows;
        out.alt_screen = self.alt;
        out.cells.clear();
        out.cells.reserve(n);
        let g = &self.screen.grid;
        let first = g.scrollback_len().saturating_sub(self.viewport);
        let empty = Row::default();
        for i in first..first + rows as usize {
            let row = g.line(i).unwrap_or(&empty);
            for x in 0..cols {
                let cell = row.cells.get(x as usize).copied().unwrap_or_default();
                out.cells
                    .push(render_cell(cell, row, x, self.styles.get(cell.style), pal));
            }
        }
        let y = self.cur.y as usize + self.viewport;
        out.cursor = (self.cursor_visible && y < rows as usize).then_some((
            self.cur.x,
            y as u16,
            self.cursor_shape,
        ));
        true
    }

    /// Resets keyboard, mouse and paste modes a crashed program may have left
    /// behind.
    pub fn on_child_exit(&mut self) {}

    pub fn set_theme(&mut self, _dark: bool) {}

    /// Cell size in pixels, for size reports.
    pub fn set_cell_px(&mut self, _w: u16, _h: u16) {}

    /// The screen as text: rows joined by `\n`, trailing spaces trimmed.
    pub fn screen_text(&self) -> String {
        let g = &self.screen.grid;
        rows_text((0..g.lines()).map(|y| g.row(y)))
    }

    /// The main screen's scrollback as text, oldest row first.
    pub fn scrollback_text(&self) -> String {
        let g = &self.main().grid;
        rows_text((0..g.scrollback_len()).filter_map(|i| g.line(i)))
    }

    fn cols(&self) -> u16 {
        self.opts.cols
    }

    fn rows(&self) -> u16 {
        self.opts.rows
    }

    fn main(&self) -> &Screen {
        if self.alt { &self.other } else { &self.screen }
    }

    fn blank(&self) -> Cell {
        Cell::blank(self.cur.blank)
    }

    fn row(&mut self) -> &mut Row {
        self.screen.grid.row_mut(self.cur.y)
    }

    // ---- printing ----

    fn print_ascii(&mut self, mut text: &[u8]) {
        while !text.is_empty() {
            if self.cur.pending_wrap {
                self.wrap();
            }
            let x = self.cur.x;
            let n = text.len().min((self.cols() - x) as usize);
            let (blank, sid, insert) = (self.blank(), self.cur.sid, self.insert);
            let row = self.row();
            if insert {
                row.insert(x as usize, n, blank);
            }
            row.put_ascii(x as usize, &text[..n], sid);
            self.last_cell = Some((x + n as u16 - 1, self.cur.y));
            self.rep = Some(text[n - 1] as char);
            self.advance(n as u16);
            text = &text[n..];
        }
    }

    fn print_char(&mut self, c: char) {
        if (self.joiner || is_extend(c))
            && let Some((x, y)) = self.last_cell
        {
            self.screen.grid.row_mut(y).push_grapheme(x, c);
            self.joiner = c == '\u{200D}';
            return;
        }
        self.joiner = false;
        let c = match self.cur.charsets[self.cur.gl] {
            Charset::DecGraphics => dec_graphics(c),
            Charset::Ascii => c,
        };
        self.put(c, is_wide(c));
    }

    /// Writes one character at the cursor and moves past it.
    fn put(&mut self, c: char, wide: bool) {
        if self.cur.pending_wrap {
            self.wrap();
        }
        if wide && self.cur.x + 1 >= self.cols() {
            if !self.autowrap || self.cols() < 2 {
                return;
            }
            // Wide characters never straddle rows: leave a spacer and wrap.
            let x = self.cur.x as usize;
            let head = Cell {
                cp: 0,
                style: self.cur.blank,
                flags: cf::SPACER_HEAD,
            };
            self.row().put(x, head);
            self.wrap();
        }
        let x = self.cur.x;
        let w = 1 + u16::from(wide);
        let blank = self.blank();
        if self.insert {
            self.row().insert(x as usize, w as usize, blank);
        }
        let cell = Cell {
            cp: c as u32,
            style: self.cur.sid,
            flags: if wide { cf::WIDE } else { 0 },
        };
        self.row().put(x as usize, cell);
        self.last_cell = Some((x, self.cur.y));
        self.rep = Some(c);
        self.advance(w);
    }

    fn advance(&mut self, w: u16) {
        let x = self.cur.x + w;
        if x >= self.cols() {
            self.cur.x = self.cols() - 1;
            self.cur.pending_wrap = self.autowrap;
        } else {
            self.cur.x = x;
        }
    }

    fn wrap(&mut self) {
        self.row().flags |= rf::WRAPPED;
        self.cur.x = 0;
        self.index();
    }

    // ---- cursor movement ----

    /// LF / IND: down one row, scrolling at the bottom margin.
    fn index(&mut self) {
        self.cur.pending_wrap = false;
        if self.cur.y == self.bottom {
            // Rows leaving a region that starts at the top of the main
            // screen go to scrollback. Full-screen programs draw their
            // transcript this way above a fixed status area.
            let keep = self.top == 0 && !self.alt;
            let before = self.screen.grid.scrollback_len();
            let blank = self.blank();
            self.screen
                .grid
                .scroll_up(self.top, self.bottom, 1, blank, keep);
            if self.viewport > 0 {
                // Keep a scrolled-back view on the same text.
                let added = self.screen.grid.scrollback_len().saturating_sub(before);
                self.viewport = (self.viewport + added).min(self.screen.grid.scrollback_len());
            }
        } else if self.cur.y + 1 < self.rows() {
            self.cur.y += 1;
        }
    }

    /// RI: up one row, scrolling down at the top margin.
    fn reverse_index(&mut self) {
        self.cur.pending_wrap = false;
        if self.cur.y == self.top {
            let blank = self.blank();
            self.screen
                .grid
                .scroll_down(self.top, self.bottom, 1, blank);
        } else if self.cur.y > 0 {
            self.cur.y -= 1;
        }
    }

    /// CUP. `row` is relative to the top margin in origin mode.
    fn goto(&mut self, row: u16, col: u16) {
        let (top, max) = if self.cur.origin {
            (self.top, self.bottom)
        } else {
            (0, self.rows() - 1)
        };
        self.cur.y = top.saturating_add(row).min(max);
        self.cur.x = col.min(self.cols() - 1);
        self.cur.pending_wrap = false;
    }

    fn set_x(&mut self, x: u16) {
        self.cur.x = x.min(self.cols() - 1);
        self.cur.pending_wrap = false;
    }

    fn up(&mut self, n: u16) {
        let lim = if self.cur.y >= self.top { self.top } else { 0 };
        self.cur.y = self.cur.y.saturating_sub(n).max(lim);
        self.cur.pending_wrap = false;
    }

    fn down(&mut self, n: u16) {
        let lim = if self.cur.y <= self.bottom {
            self.bottom
        } else {
            self.rows() - 1
        };
        self.cur.y = self.cur.y.saturating_add(n).min(lim);
        self.cur.pending_wrap = false;
    }

    fn tab(&mut self, n: u16) {
        for _ in 0..n {
            let from = self.cur.x as usize + 1;
            self.cur.x = (from..self.tabs.len())
                .find(|&i| self.tabs[i])
                .unwrap_or(self.tabs.len() - 1) as u16;
        }
        self.cur.pending_wrap = false;
    }

    fn back_tab(&mut self, n: u16) {
        for _ in 0..n {
            let x = self.cur.x as usize;
            self.cur.x = (0..x).rev().find(|&i| self.tabs[i]).unwrap_or(0) as u16;
        }
        self.cur.pending_wrap = false;
    }

    fn save_cursor(&mut self) {
        self.screen.saved = Some(self.cur);
    }

    fn restore_cursor(&mut self) {
        let mut c = self.screen.saved.unwrap_or_default();
        if c.x >= self.cols() {
            c.x = self.cols() - 1;
            c.pending_wrap = false;
        }
        c.y = c.y.min(self.rows() - 1);
        self.cur = c;
    }

    // ---- erasing and editing ----

    fn erase_display(&mut self, mode: u16) {
        let (cols, blank) = (self.cols(), self.blank());
        let rows = match mode {
            0 => {
                self.erase_line(0);
                self.cur.y + 1..self.rows()
            }
            1 => {
                self.erase_line(1);
                0..self.cur.y
            }
            2 => 0..self.rows(),
            3 => {
                let main = if self.alt {
                    &mut self.other
                } else {
                    &mut self.screen
                };
                main.grid.clear_scrollback();
                self.viewport = 0;
                return;
            }
            _ => return,
        };
        for y in rows {
            self.screen.grid.row_mut(y).reset(cols, blank);
        }
        self.cur.pending_wrap = false;
    }

    fn erase_line(&mut self, mode: u16) {
        let (x, blank) = (self.cur.x as usize, self.blank());
        let row = self.row();
        match mode {
            0 => {
                row.fill(x..usize::MAX, blank);
                row.flags &= !rf::WRAPPED;
            }
            1 => row.fill(0..x + 1, blank),
            2 => {
                row.fill(0..usize::MAX, blank);
                row.flags &= !rf::WRAPPED;
            }
            _ => {}
        }
        self.cur.pending_wrap = false;
    }

    /// IL (`down`) or DL: shifts the rows from the cursor to the bottom
    /// margin. Does nothing outside the scroll region.
    fn shift_lines(&mut self, n: u16, down: bool) {
        let y = self.cur.y;
        if y < self.top || y > self.bottom {
            return;
        }
        let blank = self.blank();
        if down {
            self.screen.grid.scroll_down(y, self.bottom, n, blank);
        } else {
            self.screen.grid.scroll_up(y, self.bottom, n, blank, false);
        }
        self.cur.x = 0;
        self.cur.pending_wrap = false;
    }

    fn soft_reset(&mut self) {
        self.cursor_visible = true;
        self.insert = false;
        self.autowrap = true;
        self.top = 0;
        self.bottom = self.rows() - 1;
        self.cur = Cursor {
            x: self.cur.x,
            y: self.cur.y,
            ..Cursor::default()
        };
        self.screen.saved = None;
    }

    fn full_reset(&mut self) {
        let replies = std::mem::take(&mut self.replies);
        let events = std::mem::take(&mut self.events);
        *self = Self::new(self.opts);
        self.replies = replies;
        self.events = events;
    }

    // ---- modes ----

    fn set_ansi_mode(&mut self, m: u16, on: bool) {
        match m {
            4 => self.insert = on,
            20 => self.newline = on,
            _ => {}
        }
    }

    fn set_dec_mode(&mut self, m: u16, on: bool) {
        match m {
            6 => {
                self.cur.origin = on;
                self.goto(0, 0);
            }
            7 => {
                self.autowrap = on;
                self.cur.pending_wrap &= on;
            }
            25 => self.cursor_visible = on,
            47 => self.switch_screen(on),
            1047 => {
                if !on && self.alt {
                    self.erase_display(2);
                }
                self.switch_screen(on);
            }
            1048 if on => self.save_cursor(),
            1048 => self.restore_cursor(),
            1049 if on && !self.alt => {
                self.save_cursor();
                self.switch_screen(true);
                self.erase_display(2);
            }
            1049 if !on && self.alt => {
                self.switch_screen(false);
                self.restore_cursor();
            }
            _ => {}
        }
    }

    fn switch_screen(&mut self, alt: bool) {
        if alt != self.alt {
            std::mem::swap(&mut self.screen, &mut self.other);
            self.alt = alt;
            self.viewport = 0;
        }
    }
}

impl Handler for Terminal {
    fn print(&mut self, s: &str) {
        self.changed = true;
        if s.is_ascii() && self.cur.charsets[self.cur.gl] == Charset::Ascii {
            self.joiner = false;
            self.print_ascii(s.as_bytes());
        } else {
            s.chars().for_each(|c| self.print_char(c));
        }
    }

    fn execute(&mut self, c0: u8) {
        self.changed = true;
        self.last_cell = None;
        self.joiner = false;
        match c0 {
            0x07 => self.events.push(Event::Bell),
            0x08 => {
                self.cur.x = self.cur.x.saturating_sub(1);
                self.cur.pending_wrap = false;
            }
            0x09 => self.tab(1),
            0x0A..=0x0C => {
                self.index();
                if self.newline {
                    self.cur.x = 0;
                }
            }
            0x0D => {
                self.cur.x = 0;
                self.cur.pending_wrap = false;
            }
            0x0E => self.cur.gl = 1,
            0x0F => self.cur.gl = 0,
            _ => {}
        }
    }

    fn esc(&mut self, inter: &[u8], fin: u8) {
        self.changed = true;
        self.last_cell = None;
        self.joiner = false;
        match (inter, fin) {
            ([], b'7') => self.save_cursor(),
            ([], b'8') => self.restore_cursor(),
            ([], b'D') => self.index(),
            ([], b'E') => {
                self.index();
                self.cur.x = 0;
            }
            ([], b'H') => self.tabs[self.cur.x as usize] = true,
            ([], b'M') => self.reverse_index(),
            ([], b'c') => self.full_reset(),
            ([g @ (b'(' | b')')], f) => {
                self.cur.charsets[usize::from(*g == b')')] = match f {
                    b'0' => Charset::DecGraphics,
                    _ => Charset::Ascii,
                };
            }
            _ => {}
        }
    }

    fn csi(&mut self, p: &Params, inter: &[u8], fin: u8) {
        self.changed = true;
        self.last_cell = None;
        self.joiner = false;
        // Count parameters: absent or 0 means 1.
        let n = |i: usize| p.get(i).max(1);
        let blank = self.blank();
        let x = self.cur.x as usize;
        match (inter, fin) {
            ([], b'@') => {
                self.row().insert(x, n(0) as usize, blank);
                self.cur.pending_wrap = false;
            }
            ([], b'A') => self.up(n(0)),
            ([], b'B' | b'e') => self.down(n(0)),
            ([], b'C' | b'a') => self.set_x(self.cur.x.saturating_add(n(0))),
            ([], b'D') => self.set_x(self.cur.x.saturating_sub(n(0))),
            ([], b'E') => {
                self.down(n(0));
                self.cur.x = 0;
            }
            ([], b'F') => {
                self.up(n(0));
                self.cur.x = 0;
            }
            ([], b'G' | b'`') => self.set_x(n(0) - 1),
            ([], b'H' | b'f') => self.goto(n(0) - 1, n(1) - 1),
            ([], b'I') => self.tab(n(0)),
            ([] | [b'?'], b'J') => self.erase_display(p.get(0)),
            ([] | [b'?'], b'K') => self.erase_line(p.get(0)),
            ([], b'L') => self.shift_lines(n(0), true),
            ([], b'M') => self.shift_lines(n(0), false),
            ([], b'P') => {
                self.row().delete(x, n(0) as usize, blank);
                self.cur.pending_wrap = false;
            }
            ([], b'S') => self
                .screen
                .grid
                .scroll_up(self.top, self.bottom, n(0), blank, false),
            // With more parameters this is xterm's mouse highlight tracking.
            ([], b'T') if p.len() <= 1 => {
                self.screen
                    .grid
                    .scroll_down(self.top, self.bottom, n(0), blank);
            }
            ([], b'X') => {
                self.row().fill(x..x + n(0) as usize, blank);
                self.cur.pending_wrap = false;
            }
            ([], b'Z') => self.back_tab(n(0)),
            ([], b'b') => {
                if let Some(c) = self.rep {
                    let wide = is_wide(c);
                    let max = self.cols() as usize * self.rows() as usize;
                    for _ in 0..(n(0) as usize).min(max) {
                        self.put(c, wide);
                    }
                }
            }
            ([], b'd') => self.goto(n(0) - 1, self.cur.x),
            ([], b'g') => match p.get(0) {
                0 => self.tabs[x] = false,
                3 => self.tabs.fill(false),
                _ => {}
            },
            ([], b'h' | b'l') => {
                for &m in p.as_slice() {
                    self.set_ansi_mode(m, fin == b'h');
                }
            }
            ([b'?'], b'h' | b'l') => {
                for &m in p.as_slice() {
                    self.set_dec_mode(m, fin == b'h');
                }
            }
            ([], b'r') => {
                let top = n(0) - 1;
                let bottom = match p.get(1) {
                    0 => self.rows(),
                    b => b.min(self.rows()),
                } - 1;
                if top < bottom {
                    self.top = top;
                    self.bottom = bottom;
                    self.goto(0, 0);
                }
            }
            ([], b's') => self.save_cursor(),
            ([], b'u') => self.restore_cursor(),
            ([b'!'], b'p') => self.soft_reset(),
            ([b' '], b'q') => {
                self.cursor_shape = match p.get(0) {
                    3 | 4 => CursorShape::Underline,
                    5 | 6 => CursorShape::Bar,
                    _ => CursorShape::Block,
                };
            }
            _ => {}
        }
    }

    fn osc(&mut self, _data: &[u8], _bel_terminated: bool) {}

    fn dcs_hook(&mut self, _p: &Params, _inter: &[u8], _fin: u8) {}

    fn dcs_put(&mut self, _chunk: &[u8]) {}

    fn dcs_unhook(&mut self) {}
}

fn default_tabs(cols: u16) -> Vec<bool> {
    (0..cols).map(|i| i > 0 && i % 8 == 0).collect()
}

fn rows_text<'a>(rows: impl Iterator<Item = &'a Row>) -> String {
    let mut s = String::new();
    for (i, row) in rows.enumerate() {
        if i > 0 {
            s.push('\n');
        }
        row.push_text(&mut s);
        s.truncate(s.trim_end_matches(' ').len());
    }
    s
}

fn is_wide(c: char) -> bool {
    cluster_width(c.encode_utf8(&mut [0; 4])) == 2
}

/// Code points that join the previous cell instead of taking their own.
// ponytail: common combining blocks, joiners, variation selectors, skin
// tones and tags; the generated Unicode tables should own this list.
fn is_extend(c: char) -> bool {
    matches!(c as u32,
        0x0300..=0x036F | 0x0483..=0x0489 | 0x0591..=0x05BD | 0x0610..=0x061A
        | 0x064B..=0x065F | 0x0E31 | 0x0E34..=0x0E3A | 0x0E47..=0x0E4E
        | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x200C..=0x200D | 0x20D0..=0x20FF
        | 0xFE00..=0xFE0F | 0xFE20..=0xFE2F | 0x1F3FB..=0x1F3FF
        | 0xE0020..=0xE007F | 0xE0100..=0xE01EF)
}

/// DEC Special Graphics for `_` and `` ` `` through `~`.
fn dec_graphics(c: char) -> char {
    const MAP: [char; 31] = [
        '◆', '▒', '␉', '␌', '␍', '␊', '°', '±', '␤', '␋', '┘', '┐', '┌', '└', '┼', '⎺', '⎻', '─',
        '⎼', '⎽', '├', '┤', '┴', '┬', '│', '≤', '≥', 'π', '≠', '£', '·',
    ];
    match c {
        '`'..='~' => MAP[c as usize - 0x60],
        '_' => ' ',
        _ => c,
    }
}

fn render_cell(cell: Cell, row: &Row, x: u16, style: &Style, pal: &Palette) -> RenderCell {
    let a = style.attrs;
    let mut fg = pal.resolve(style.fg, pal.fg);
    let mut bg = pal.resolve(style.bg, pal.bg);
    if a & sgr::INVERSE != 0 {
        std::mem::swap(&mut fg, &mut bg);
    }
    if a & sgr::INVISIBLE != 0 {
        fg = bg;
    }
    let mut attrs = 0;
    for (from, to) in [
        (sgr::BOLD, attr::BOLD),
        (sgr::ITALIC, attr::ITALIC),
        (sgr::UNDERLINE, attr::UNDERLINE),
        (sgr::INVERSE, attr::INVERSE),
        (sgr::FAINT, attr::DIM),
    ] {
        if a & from != 0 {
            attrs |= to;
        }
    }
    let mut rc = RenderCell {
        width: match cell.flags {
            f if f & cf::SPACER_TAIL != 0 => 0,
            f if f & cf::WIDE != 0 => 2,
            _ => 1,
        },
        fg,
        bg,
        attrs,
        ..RenderCell::default()
    };
    if let Some(c) = char::from_u32(cell.cp).filter(|&c| c != '\0') {
        let mut len = c.encode_utf8(&mut rc.text).len();
        if cell.flags & cf::GRAPHEME != 0 {
            for c in row.grapheme(x).unwrap_or_default().chars() {
                if len + c.len_utf8() > rc.text.len() {
                    break;
                }
                len += c.encode_utf8(&mut rc.text[len..]).len();
            }
        }
        rc.len = len as u8;
    }
    rc
}
