//! The terminal: screens, cursor, modes, replies and events.

use std::collections::VecDeque;
use std::io::Write;
use std::time::Instant;

use crate::grid::{Cell, Grid, Row, cf, rf};
use crate::modes::{InputModes, KittyStack, Modes};
use crate::osc::{self, Osc9};
use crate::parser::{Handler, Params, Parser};
use crate::snapshot::{self, CursorShape, Palette, RenderCell, Snapshot};
use crate::style::{Color, Style, Styles, attr};
use crate::width::{chars_width, joins};

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
    /// Prompt start. `blitz` is set when the mark carries `blitz=<token>`
    /// with the token the host gave [`Terminal::set_prompt_token`].
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

/// Where the last printed grapheme cluster went and what it holds so far.
#[derive(Clone, Copy, Debug)]
struct Cluster {
    x: u16,
    y: u16,
    first: char,
    last: char,
    len: usize,
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
    /// The cluster just printed, while more code points may still join it.
    cluster: Option<Cluster>,
    /// Rows the view is scrolled back into scrollback.
    viewport: usize,
    changed: bool,
    modes: Modes,
    /// Dark or light system theme, for `CSI ? 996 n`.
    dark: bool,
    /// Cell width and height in pixels, for size reports.
    cell_px: (u16, u16),
    /// Foreground, background and cursor colours the host draws with, for
    /// OSC 10, 11 and 12 queries.
    pal: [u32; 3],
    /// The same colours as set by the program with OSC 10, 11 and 12.
    colors: [Option<u32>; 3],
    replies: Vec<u8>,
    /// Output bytes not yet spent on replies; see [`MAX_REPLIES`].
    reply_credit: usize,
    events: VecDeque<Event>,
    /// The secret that marks blitz's own shell-integration prompts.
    prompt_token: String,
}

/// Foreground, background and cursor colours of the default dark and
/// light themes, used for colour queries until a snapshot gives the
/// host's own palette.
const DARK: [u32; 3] = [0xD6D7D9, 0x131417, 0xECECEA];
const LIGHT: [u32; 3] = [0x2F3135, 0xFCFCFB, 0x141518];

/// Longest window title and notification texts kept, in characters.
const MAX_TITLE: usize = 256;
const MAX_NOTIFY_TITLE: usize = 64;
const MAX_NOTIFY_BODY: usize = 256;

/// Replies may use a sixteenth of the output, plus a reserve of this
/// many bytes for the questions a program asks at startup; queries past
/// that go unanswered. Output full of queries would otherwise type an
/// unbounded stream of answers into the program's input.
const MAX_REPLIES: usize = 4096;
const REPLY_CREDIT: usize = 16 * MAX_REPLIES;

/// Most events queued between [`Terminal::take_events`] calls.
const MAX_EVENTS: usize = 1024;

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
            cluster: None,
            viewport: 0,
            changed: true,
            modes: Modes::default(),
            dark: true,
            cell_px: (0, 0),
            pal: DARK,
            colors: [None; 3],
            replies: Vec::new(),
            reply_credit: REPLY_CREDIT,
            events: VecDeque::new(),
            prompt_token: "1".into(),
        }
    }

    /// Parses `bytes`, queueing replies and events.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.reply_credit = (self.reply_credit + bytes.len()).min(REPLY_CREDIT);
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
        out.extend(self.events.drain(..));
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
        // The last cluster's cell may be gone; a mark arriving now must
        // not reach for it.
        self.cluster = None;
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
            kitty: self.kitty().flags(),
            alt_screen: self.alt,
            ..self.modes.input
        }
    }

    /// A screen's kitty keyboard flags stack, bottom entry first.
    pub fn kitty_stack(&self, alt: bool) -> &[u8] {
        self.modes.kitty[usize::from(alt)].entries()
    }

    /// Column, row and visibility.
    pub fn cursor(&self) -> (u16, u16, bool) {
        (self.cur.x, self.cur.y, self.cursor_visible)
    }

    /// True while a synchronized update (mode 2026) is open and younger
    /// than [`crate::modes::SYNC_TIMEOUT`]. Snapshots wait meanwhile, so
    /// the host should look again once it times out.
    pub fn sync_pending(&self, now: Instant) -> bool {
        self.modes.sync_pending(now)
    }

    /// Fills `out` with the visible screen. Returns whether anything changed
    /// since the last call; when nothing did, `out` is left as it was.
    /// During a synchronized update the screen is half drawn, so nothing
    /// is taken until it ends or times out.
    pub fn snapshot(&mut self, out: &mut Snapshot, pal: &Palette) -> bool {
        if self.modes.sync.is_some() && self.sync_pending(Instant::now()) {
            return false;
        }
        let (cols, rows) = (self.cols(), self.rows());
        let n = cols as usize * rows as usize;
        if !self.changed && (out.cols, out.rows, out.cells.len()) == (cols, rows, n) {
            return false;
        }
        self.changed = false;
        self.pal = [pal.fg, pal.bg, pal.cursor];
        let [fg, bg, cursor] = self.colors;
        let pal = &Palette {
            fg: fg.unwrap_or(pal.fg),
            bg: bg.unwrap_or(pal.bg),
            cursor: cursor.unwrap_or(pal.cursor),
            ..*pal
        };
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
    pub fn on_child_exit(&mut self) {
        self.modes.reset_input();
        self.changed = true;
    }

    pub fn set_theme(&mut self, dark: bool) {
        self.dark = dark;
        self.pal = if dark { DARK } else { LIGHT };
    }

    /// The secret blitz's shell integration puts in its prompt marks, as
    /// `133;A;blitz=<token>`. Only a mark with it resets input modes, so
    /// program output cannot. Until this is called the token is `1`.
    pub fn set_prompt_token(&mut self, token: &str) {
        token.clone_into(&mut self.prompt_token);
    }

    /// Cell size in pixels, for size reports.
    pub fn set_cell_px(&mut self, w: u16, h: u16) {
        self.cell_px = (w, h);
    }

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

    /// The kitty keyboard stack of the screen being shown.
    fn kitty(&self) -> &KittyStack {
        &self.modes.kitty[usize::from(self.alt)]
    }

    fn kitty_mut(&mut self) -> &mut KittyStack {
        &mut self.modes.kitty[usize::from(self.alt)]
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
            let c = text[n - 1] as char;
            self.cluster = Some(Cluster {
                x: x + n as u16 - 1,
                y: self.cur.y,
                first: c,
                last: c,
                len: 1,
            });
            self.rep = Some(c);
            self.advance(n as u16);
            text = &text[n..];
        }
    }

    fn print_char(&mut self, c: char) {
        let c = match self.cur.charsets[self.cur.gl] {
            Charset::DecGraphics => dec_graphics(c),
            Charset::Ascii => c,
        };
        if let Some(cl) = &mut self.cluster
            && joins(cl.first, cl.last, cl.len, c)
        {
            cl.last = c;
            cl.len += 1;
            let Cluster { x, y, first, .. } = *cl;
            self.join(x, y, first, c);
            return;
        }
        match self.width(c) {
            // A mark or joiner with nothing to attach to.
            0 => {}
            w => self.put(c, w == 2),
        }
    }

    /// Adds `c` to the cluster in cell (`x`, `y`). VS16 or a skin tone can
    /// make a narrow emoji wide; it then takes the next column when the
    /// cursor is still right after it.
    fn join(&mut self, x: u16, y: u16, first: char, c: char) {
        let amb = self.opts.ambiguous_wide;
        let at_cursor = self.cur.y == y && self.cur.x == x + 1 && !self.cur.pending_wrap;
        let row = self.screen.grid.row_mut(y);
        row.push_grapheme(x, c);
        let narrow = row.cells[x as usize].flags & cf::WIDE == 0;
        let tail = row.grapheme(x).unwrap_or_default().chars();
        if narrow && at_cursor && chars_width(std::iter::once(first).chain(tail), amb) == 2 {
            row.widen(x as usize);
            self.advance(1);
        }
    }

    fn width(&self, c: char) -> u8 {
        chars_width([c], self.opts.ambiguous_wide)
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
        self.cluster = Some(Cluster {
            x,
            y: self.cur.y,
            first: c,
            last: c,
            len: 1,
        });
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
        // Past the width every step lands on the last column.
        for _ in 0..n.min(self.cols()) {
            let from = self.cur.x as usize + 1;
            self.cur.x = (from..self.tabs.len())
                .find(|&i| self.tabs[i])
                .unwrap_or(self.tabs.len() - 1) as u16;
        }
        self.cur.pending_wrap = false;
    }

    fn back_tab(&mut self, n: u16) {
        for _ in 0..n.min(self.cols()) {
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
        self.modes.input.decckm = false;
        self.modes.input.deckpam = false;
        self.top = 0;
        self.bottom = self.rows() - 1;
        self.cur = Cursor {
            x: self.cur.x,
            y: self.cur.y,
            ..Cursor::default()
        };
        // Both screens forget their saved cursor, as in conhost.
        self.screen.saved = None;
        self.other.saved = None;
    }

    fn full_reset(&mut self) {
        let mut t = Self::new(self.opts);
        // Queued output and what the host told us survive.
        std::mem::swap(&mut t.replies, &mut self.replies);
        t.reply_credit = self.reply_credit;
        std::mem::swap(&mut t.events, &mut self.events);
        std::mem::swap(&mut t.prompt_token, &mut self.prompt_token);
        t.dark = self.dark;
        t.cell_px = self.cell_px;
        t.pal = self.pal;
        // ConPTY turns these on for itself at startup and is not told that
        // a program reset the terminal, so they stay.
        t.modes.input.w32im = self.modes.input.w32im;
        t.modes.input.focus = self.modes.input.focus;
        *self = t;
    }

    /// At the shell's own prompt, clears what a program can leave behind
    /// that would bend or hide everything printed after it: input modes,
    /// margins, origin, insert and wrap modes, charsets, tab stops and
    /// colours set with OSC 10, 11 and 12. The cursor stays where the
    /// prompt is about to be drawn. The screen is left alone: the prompt
    /// leaves the alternate screen with its own sequences, which conhost
    /// acts on too.
    fn prompt_reset(&mut self) {
        self.modes.reset_input();
        self.top = 0;
        self.bottom = self.rows() - 1;
        self.cur.origin = false;
        self.cur.charsets = Default::default();
        self.cur.gl = 0;
        self.insert = false;
        self.autowrap = true;
        self.tabs = default_tabs(self.cols());
        self.colors = [None; 3];
        self.changed = true;
    }

    // ---- OSC ----

    fn set_link(&mut self, body: &str) {
        let Some((params, uri)) = body.split_once(';') else {
            return;
        };
        if self.styles.wants_compact() {
            self.compact_styles();
        }
        let id = params
            .split(':')
            .find_map(|kv| kv.strip_prefix("id="))
            .unwrap_or("");
        self.cur.style.link = match uri {
            "" => 0,
            uri => self.styles.intern_link(id, uri),
        };
        self.cur.sid = self.styles.intern(self.cur.style);
    }

    /// OSC 10, 11 and 12, starting at colour `first` (0 foreground,
    /// 1 background, 2 cursor). Each `;` item moves to the next colour, as
    /// in `OSC 10;?;?`. An item is a query (`?`) or a colour to use.
    fn dynamic_colors(&mut self, first: usize, body: &str, bel: bool) {
        for (n, item) in (first..3).zip(body.split(';')) {
            if item == "?" {
                let rgb = self.colors[n].unwrap_or(self.pal[n]);
                let start = self.replies.len();
                osc::color_reply(10 + n, rgb, bel, &mut self.replies);
                self.charge_reply(start);
            } else if let Some(rgb) = osc::parse_color(item) {
                self.colors[n] = Some(rgb);
                self.changed = true;
            }
        }
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
            _ => {
                self.modes.set_dec(m, on);
            }
        }
    }

    fn switch_screen(&mut self, alt: bool) {
        if alt != self.alt {
            std::mem::swap(&mut self.screen, &mut self.other);
            self.alt = alt;
            self.viewport = 0;
        }
    }

    // ---- replies ----

    fn reply(&mut self, args: std::fmt::Arguments) {
        let start = self.replies.len();
        // Writing to a Vec cannot fail.
        let _ = self.replies.write_fmt(args);
        self.charge_reply(start);
    }

    /// Pays for the reply queued from `start`, or drops it when the output
    /// so far has not earned it.
    fn charge_reply(&mut self, start: usize) {
        let cost = 16 * (self.replies.len() - start);
        match self.reply_credit.checked_sub(cost) {
            Some(left) => self.reply_credit = left,
            None => self.replies.truncate(start),
        }
    }

    /// Queues `ev` for the host. Only the newest title and directory
    /// matter and pending bells ring once, so each replaces the one
    /// already queued and a flood of them costs the host a single update.
    /// Past [`MAX_EVENTS`] the oldest event is dropped.
    fn event(&mut self, ev: Event) {
        if matches!(ev, Event::Title(_) | Event::Cwd(_) | Event::Bell) {
            // At most one of each is queued, and in a flood it was the
            // last one pushed, so searching from the back is quick.
            let kind = std::mem::discriminant(&ev);
            if let Some(i) = self
                .events
                .iter()
                .rposition(|e| std::mem::discriminant(e) == kind)
            {
                self.events.remove(i);
            }
        }
        if self.events.len() >= MAX_EVENTS {
            self.events.pop_front();
        }
        self.events.push_back(ev);
    }

    /// CPR and DECXCPR. Rows count from the top margin in origin mode.
    fn report_cursor(&mut self, private: &str) {
        let top = if self.cur.origin { self.top } else { 0 };
        let (row, col) = (self.cur.y.saturating_sub(top) + 1, self.cur.x + 1);
        self.reply(format_args!("\x1b[{private}{row};{col}R"));
    }

    /// DECRPM status of a DEC private mode: 0 not recognized, 1 set,
    /// 2 reset, 3 permanently set.
    fn dec_mode_status(&self, m: u16) -> u8 {
        let on = match m {
            6 => self.cur.origin,
            7 => self.autowrap,
            25 => self.cursor_visible,
            47 | 1047 | 1049 => self.alt,
            1048 => false,
            // Grapheme clusters are always kept together.
            2027 => return 3,
            // Everything else, including 1016 (SGR pixel mouse), is
            // unknown unless the mode table has it.
            _ => match self.modes.dec(m) {
                Some(on) => on,
                None => return 0,
            },
        };
        if on { 1 } else { 2 }
    }

    fn ansi_mode_status(&self, m: u16) -> u8 {
        match m {
            4 => 2 - u8::from(self.insert),
            20 => 2 - u8::from(self.newline),
            _ => 0,
        }
    }

    /// XTWINOPS reports. Window moves and resizes are ignored.
    fn window_report(&mut self, op: u16) {
        let (w, h) = self.cell_px;
        let (cols, rows) = (self.cols(), self.rows());
        match op {
            14 => {
                let (pw, ph) = (
                    u32::from(cols) * u32::from(w),
                    u32::from(rows) * u32::from(h),
                );
                self.reply(format_args!("\x1b[4;{ph};{pw}t"));
            }
            16 => self.reply(format_args!("\x1b[6;{h};{w}t")),
            18 => self.reply(format_args!("\x1b[8;{rows};{cols}t")),
            _ => {}
        }
    }

    // ---- SGR ----

    /// Shrinks the style table to the styles still stored somewhere and
    /// renumbers every reference to them.
    fn compact_styles(&mut self) {
        let mut live = vec![self.cur.sid, self.cur.blank];
        for c in [self.screen.saved, self.other.saved].into_iter().flatten() {
            live.extend([c.sid, c.blank]);
        }
        let cells = self.screen.grid.cells().chain(self.other.grid.cells());
        let map = self
            .styles
            .compact(live.into_iter().chain(cells.map(|c| c.style)));
        let remap = |id: &mut u16| *id = map.get(usize::from(*id)).copied().unwrap_or(0);
        for g in [&mut self.screen.grid, &mut self.other.grid] {
            g.cells_mut().for_each(|c| remap(&mut c.style));
        }
        let cursors = [
            Some(&mut self.cur),
            self.screen.saved.as_mut(),
            self.other.saved.as_mut(),
        ];
        for c in cursors.into_iter().flatten() {
            remap(&mut c.sid);
            remap(&mut c.blank);
            // Link ids were renumbered too.
            c.style = *self.styles.get(c.sid);
        }
    }

    fn sgr(&mut self, p: &Params) {
        if self.styles.wants_compact() {
            self.compact_styles();
        }
        let v = p.as_slice();
        let s = &mut self.cur.style;
        if v.is_empty() {
            *s = Style {
                link: s.link,
                ..Style::default()
            };
        }
        let mut i = 0;
        while i < v.len() {
            let mut subs = 0;
            while p.is_sub(i + 1 + subs) {
                subs += 1;
            }
            let mut used = 1 + subs;
            let a = &mut s.attrs;
            match v[i] {
                // A hyperlink is not a rendition; SGR 0 leaves it alone.
                0 => {
                    *s = Style {
                        link: s.link,
                        ..Style::default()
                    }
                }
                1 => *a |= attr::BOLD,
                2 => *a |= attr::DIM,
                3 => *a |= attr::ITALIC,
                4 => {
                    let kind = if subs > 0 { v[i + 1].min(5) } else { 1 };
                    *a = (*a & !attr::UNDERLINE) | kind << attr::UNDERLINE_SHIFT;
                }
                5 | 6 => *a |= attr::BLINK,
                7 => *a |= attr::INVERSE,
                8 => *a |= attr::INVISIBLE,
                9 => *a |= attr::STRIKE,
                21 => *a = (*a & !attr::UNDERLINE) | 2 << attr::UNDERLINE_SHIFT,
                22 => *a &= !(attr::BOLD | attr::DIM),
                23 => *a &= !attr::ITALIC,
                24 => *a &= !attr::UNDERLINE,
                25 => *a &= !attr::BLINK,
                27 => *a &= !attr::INVERSE,
                28 => *a &= !attr::INVISIBLE,
                29 => *a &= !attr::STRIKE,
                53 => *a |= attr::OVERLINE,
                55 => *a &= !attr::OVERLINE,
                n @ 30..=37 => s.fg = Color::Idx((n - 30) as u8),
                n @ 40..=47 => s.bg = Color::Idx((n - 40) as u8),
                n @ 90..=97 => s.fg = Color::Idx((n - 90 + 8) as u8),
                n @ 100..=107 => s.bg = Color::Idx((n - 100 + 8) as u8),
                39 => s.fg = Color::Default,
                49 => s.bg = Color::Default,
                59 => s.ul = Color::Default,
                n @ (38 | 48 | 58) => {
                    let (c, n_used) = ext_color(v, i, subs);
                    used = n_used;
                    if let Some(c) = c {
                        match n {
                            38 => s.fg = c,
                            48 => s.bg = c,
                            _ => s.ul = c,
                        }
                    }
                }
                _ => {}
            }
            i += used;
        }
        let bg = s.bg;
        self.cur.sid = self.styles.intern(self.cur.style);
        self.cur.blank = match bg {
            Color::Default => 0,
            bg => self.styles.intern(Style {
                bg,
                ..Style::default()
            }),
        };
    }
}

/// Parses the colour after SGR 38/48/58 at `v[i]`, in either the colon
/// form (`38:5:n`, `38:2::r:g:b`, `38:2:r:g:b`) or the semicolon form
/// (`38;5;n`, `38;2;r;g;b`). Returns the colour and how many parameters it
/// took, counting `v[i]`.
fn ext_color(v: &[u16], i: usize, subs: usize) -> (Option<Color>, usize) {
    let byte = |x: u16| x.min(255) as u8;
    let rgb = |s: &[u16]| Color::Rgb(byte(s[0]), byte(s[1]), byte(s[2]));
    if subs > 0 {
        let s = &v[i + 1..=i + subs];
        let c = match (s[0], s.len()) {
            (5, 2..) => Some(Color::Idx(byte(s[1]))),
            (2, 5..) => Some(rgb(&s[2..])),
            (2, 4) => Some(rgb(&s[1..])),
            _ => None,
        };
        return (c, 1 + subs);
    }
    let rest = &v[i + 1..];
    match rest {
        [5, n, ..] => (Some(Color::Idx(byte(*n))), 3),
        [2, r, g, b, ..] => (Some(rgb(&[*r, *g, *b])), 5),
        // Malformed: skip everything, as xterm does.
        _ => (None, v.len() - i),
    }
}

impl Handler for Terminal {
    fn print(&mut self, s: &str) {
        self.changed = true;
        if s.is_ascii() && self.cur.charsets[self.cur.gl] == Charset::Ascii {
            self.print_ascii(s.as_bytes());
        } else {
            s.chars().for_each(|c| self.print_char(c));
        }
    }

    fn execute(&mut self, c0: u8) {
        self.changed = true;
        self.cluster = None;
        match c0 {
            0x07 => self.event(Event::Bell),
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
        self.cluster = None;
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
            ([], b'=') => self.modes.input.deckpam = true,
            ([], b'>') => self.modes.input.deckpam = false,
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
        self.cluster = None;
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
            // DA1. Never claim 28 (rectangular editing): ConPTY would
            // start sending DECCRA and DECFRA.
            ([], b'c') if p.get(0) == 0 => self.reply(format_args!("\x1b[?62;22c")),
            ([b'>'], b'c') if p.get(0) == 0 => self.reply(format_args!("\x1b[>1;0;0c")),
            ([b'>'], b'q') if p.get(0) == 0 => {
                let v = env!("CARGO_PKG_VERSION");
                self.reply(format_args!("\x1bP>|blitz {v}\x1b\\"));
            }
            ([], b'n') => match p.get(0) {
                5 => self.reply(format_args!("\x1b[0n")),
                6 => self.report_cursor(""),
                _ => {}
            },
            ([b'?'], b'n') => match p.get(0) {
                // Two parameters only; no page number.
                6 => self.report_cursor("?"),
                996 => {
                    let theme = if self.dark { 1 } else { 2 };
                    self.reply(format_args!("\x1b[?997;{theme}n"));
                }
                _ => {}
            },
            ([b'?', b'$'], b'p') => {
                let m = p.get(0);
                let s = self.dec_mode_status(m);
                self.reply(format_args!("\x1b[?{m};{s}$y"));
            }
            ([b'$'], b'p') => {
                let m = p.get(0);
                let s = self.ansi_mode_status(m);
                self.reply(format_args!("\x1b[{m};{s}$y"));
            }
            ([], b't') => self.window_report(p.get(0)),
            ([], b'b') => {
                if let Some(c) = self.rep {
                    let wide = self.width(c) == 2;
                    // Programs repeat within a line. A larger count lets a
                    // few bytes keep the parser busy, more so in insert
                    // mode, where each character shifts the rest of the row.
                    for _ in 0..n(0).min(self.cols()) {
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
            // A `:` has no place in a mode list. ConPTY ignores such a
            // sequence whole, so acting on any of it would leave the two
            // out of step.
            ([] | [b'?'], b'h' | b'l') if (1..p.len()).any(|i| p.is_sub(i)) => {}
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
            ([], b'm') => self.sgr(p),
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
            // Kitty keyboard protocol. A bare `CSI u` is SCORC, above.
            ([b'?'], b'u') => {
                let f = self.kitty().flags();
                self.reply(format_args!("\x1b[?{f}u"));
            }
            ([b'>'], b'u') => self.kitty_mut().push(p.get(0) as u8),
            ([b'<'], b'u') => self.kitty_mut().pop(n(0) as usize),
            ([b'='], b'u') => self.kitty_mut().set(p.get(0) as u8, p.get(1)),
            // XTMODKEYS: `CSI > 4 ; n m` sets modifyOtherKeys. Not SGR.
            ([b'>'], b'm') if p.is_empty() => self.modes.mok = 0,
            ([b'>'], b'm') if p.get(0) == 4 => self.modes.mok = p.get(1).min(2) as u8,
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

    fn osc(&mut self, data: &[u8], bel: bool) {
        let s = String::from_utf8_lossy(data);
        let (cmd, body) = s.split_once(';').unwrap_or((&s, ""));
        let ev = match cmd {
            "0" | "2" => Event::Title(osc::clean(body, MAX_TITLE)),
            "7" => match osc::file_url_path(body) {
                Some(p) if osc::local_dir(&p) => Event::Cwd(p),
                _ => return,
            },
            "8" => return self.set_link(body),
            "9" => match osc::classify_osc9(body) {
                Osc9::Notify(text) => Event::Notify {
                    title: String::new(),
                    body: osc::clean(text, MAX_NOTIFY_BODY),
                },
                Osc9::Progress { state, pct } => Event::Progress { state, pct },
                Osc9::Cwd(p) if osc::local_dir(p) => Event::Cwd(p.to_owned()),
                Osc9::Cwd(_) => return,
                Osc9::PromptStart => Event::Prompt(PromptMark::A { blitz: false }),
                Osc9::Ignore => return,
            },
            "10" | "11" | "12" => {
                let first = usize::from(cmd.as_bytes()[1] - b'0');
                return self.dynamic_colors(first, body, bel);
            }
            "110" | "111" | "112" => {
                self.colors[usize::from(cmd.as_bytes()[2] - b'0')] = None;
                self.changed = true;
                return;
            }
            // Resets palette entries set with OSC 4, which is not
            // supported, so there is nothing to reset.
            "104" => return,
            "133" => match osc::prompt_mark(body, &self.prompt_token) {
                // blitz's own shell integration marks its prompts, so a
                // program that died in this shell left its modes behind.
                // Other prompt starts are left alone: in screen-reader mode
                // Claude Code sends 133;A;redraw=0 every turn while it
                // still wants its kitty keys.
                Some(m @ PromptMark::A { blitz: true }) => {
                    self.prompt_reset();
                    Event::Prompt(m)
                }
                Some(m) => Event::Prompt(m),
                None => return,
            },
            // `notify;title;body`, where the body runs to the end and may
            // hold `;`.
            "777" => match body.split_once(';') {
                Some(("notify", rest)) => {
                    let (title, text) = rest.split_once(';').unwrap_or((rest, ""));
                    Event::Notify {
                        title: osc::clean(title, MAX_NOTIFY_TITLE),
                        body: osc::clean(text, MAX_NOTIFY_BODY),
                    }
                }
                _ => return,
            },
            _ => return,
        };
        self.event(ev);
    }

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
    if a & attr::INVERSE != 0 {
        std::mem::swap(&mut fg, &mut bg);
    }
    if a & attr::INVISIBLE != 0 {
        fg = bg;
    }
    let mut attrs = 0;
    for (from, to) in [
        (attr::BOLD, snapshot::attr::BOLD),
        (attr::ITALIC, snapshot::attr::ITALIC),
        (attr::UNDERLINE, snapshot::attr::UNDERLINE),
        (attr::INVERSE, snapshot::attr::INVERSE),
        (attr::DIM, snapshot::attr::DIM),
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
