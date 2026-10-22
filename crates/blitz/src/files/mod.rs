//! The Files view: the folder a pane's shell is in, listed beside a
//! preview of the highlighted file, drawn over the pane in place of its
//! terminal. The terminal goes on underneath, and Esc brings it back.
//!
//! Folders and files are read on other threads: [`FileView::needs`] says
//! what to read, and [`FileView::accept`] takes what was read.

mod draw;
pub mod listing;
pub mod load;

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use vt::{Key, KeyInput, Snapshot};

pub use listing::{Entry, Kind, Listing};
pub use load::Preview;

/// Which column keys go to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    List,
    Preview,
}

/// What was read on another thread.
#[derive(Debug)]
pub enum Loaded {
    Listing(PathBuf, Listing),
    Preview(PathBuf, Preview),
}

/// What to read for the view as it is now.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Needs {
    pub list: Option<PathBuf>,
    pub preview: Option<PathBuf>,
}

/// What a key or a click asks of the app.
#[derive(Debug, PartialEq, Eq)]
pub enum Act {
    /// Nothing changed.
    None,
    /// Something changed: draw again, and read what [`FileView::needs`].
    Redraw,
    /// Close the view.
    Close,
    /// Open the file at this line in the user's editor or its program.
    Open(PathBuf, u32),
}

pub struct FileView {
    /// The folder shown.
    pub dir: PathBuf,
    /// Its entries once read.
    listing: Option<Listing>,
    /// The folder last asked for, so it is read once.
    asked_dir: Option<PathBuf>,
    /// Typed text: entries whose names hold it, ignoring case, show.
    pub filter: String,
    /// The highlighted entry, among those that show.
    sel: usize,
    /// The first entry the list shows.
    top: usize,
    /// The file the preview shows, and what was read of it.
    preview: Option<(PathBuf, Preview)>,
    /// The file last asked for.
    asked_file: Option<PathBuf>,
    /// The first line the preview shows.
    scroll: usize,
    /// A line to mark, counting from 0, as when a link named it.
    mark: Option<usize>,
    /// Where keys go.
    pub side: Side,
    /// Hidden files and folders show.
    pub hidden: bool,
    /// The entry to highlight once the listing comes, such as the folder
    /// just left.
    pick: Option<OsString>,
    /// Rows of entries or lines in the last frame, for paging.
    body: usize,
    /// Columns the list took in the last frame, 0 when it did not show.
    list_cols: u16,
}

impl FileView {
    /// A view of folder `dir`.
    pub fn new(dir: PathBuf) -> FileView {
        FileView {
            dir,
            listing: None,
            asked_dir: None,
            filter: String::new(),
            sel: 0,
            top: 0,
            preview: None,
            asked_file: None,
            scroll: 0,
            mark: None,
            side: Side::List,
            hidden: false,
            pick: None,
            body: 10,
            list_cols: 0,
        }
    }

    /// A view of the folder of `file`, with `file` highlighted and shown
    /// at `line`, counting from 1.
    pub fn at(file: &Path, line: Option<u32>) -> FileView {
        let dir = file
            .parent()
            .map_or_else(|| file.to_owned(), Path::to_owned);
        let mut f = FileView::new(dir);
        f.pick = file.file_name().map(OsString::from);
        f.mark = line.map(|l| l.saturating_sub(1) as usize);
        f.side = Side::Preview;
        // Hidden files show when one is asked for.
        f.hidden = f
            .pick
            .as_ref()
            .is_some_and(|n| n.to_string_lossy().starts_with('.'));
        f
    }

    /// The indexes of the entries that show, in order.
    fn shown(&self) -> Vec<usize> {
        let Some(l) = &self.listing else {
            return Vec::new();
        };
        let filter = self.filter.to_lowercase();
        (l.entries.iter().enumerate())
            .filter(|(_, e)| self.hidden || !e.hidden)
            .filter(|(_, e)| filter.is_empty() || e.label.to_lowercase().contains(&filter))
            .map(|(i, _)| i)
            .collect()
    }

    /// The highlighted entry.
    pub fn selected(&self) -> Option<&Entry> {
        let i = *self.shown().get(self.sel)?;
        self.listing.as_ref()?.entries.get(i)
    }

    /// What to read now; each folder and file is asked for once.
    pub fn needs(&mut self) -> Needs {
        let mut n = Needs::default();
        if self.asked_dir.as_ref() != Some(&self.dir) {
            self.asked_dir = Some(self.dir.clone());
            n.list = Some(self.dir.clone());
        }
        let file = (self.selected())
            .filter(|e| e.kind == Kind::File)
            .map(|e| self.dir.join(&e.name));
        if let Some(p) = file
            && self.asked_file.as_ref() != Some(&p)
        {
            self.asked_file = Some(p.clone());
            n.preview = Some(p);
        }
        n
    }

    /// Takes what was read, unless the view has moved on since it was
    /// asked for.
    pub fn accept(&mut self, loaded: Loaded) {
        match loaded {
            Loaded::Listing(dir, l) if dir == self.dir => {
                self.listing = Some(l);
                self.sel = 0;
                if let Some(name) = self.pick.take() {
                    let entries = &self.listing.as_ref().map_or(&[][..], |l| &l.entries);
                    let at = self.shown().iter().position(|&i| entries[i].name == name);
                    self.sel = at.unwrap_or(0);
                }
                self.top = self.sel.saturating_sub(self.body / 2);
                if self.selected().is_none_or(|e| e.kind != Kind::File) {
                    self.side = Side::List;
                }
            }
            Loaded::Preview(path, p) if self.asked_file.as_ref() == Some(&path) => {
                let lines = match &p {
                    Preview::Text { lines, .. } => lines.len(),
                    _ => 0,
                };
                self.mark = self.mark.filter(|&m| m < lines);
                self.scroll = self.mark.map_or(0, |m| m.saturating_sub(self.body / 3));
                self.preview = Some((path, p));
            }
            _ => {}
        }
    }

    /// The preview of the highlighted file, once read.
    fn shown_preview(&self) -> Option<&Preview> {
        let (path, p) = self.preview.as_ref()?;
        (Some(path) == self.asked_file.as_ref()).then_some(p)
    }

    fn preview_lines(&self) -> usize {
        match self.shown_preview() {
            Some(Preview::Text { lines, .. }) => lines.len(),
            _ => 0,
        }
    }

    /// Highlights entry `sel` of those that show, keeping it in view, and
    /// starts its preview from the top.
    fn select(&mut self, sel: usize) {
        let last = self.shown().len().saturating_sub(1);
        let sel = sel.min(last);
        if sel != self.sel {
            self.scroll = 0;
            self.mark = None;
        }
        self.sel = sel;
        let body = self.body.max(1);
        if self.sel < self.top {
            self.top = self.sel;
        } else if self.sel >= self.top + body {
            self.top = self.sel + 1 - body;
        }
    }

    fn select_by(&mut self, by: isize) {
        self.select(self.sel.saturating_add_signed(by));
    }

    fn scroll_by(&mut self, by: isize) {
        let last = self.preview_lines().saturating_sub(self.body.max(1));
        self.scroll = self.scroll.saturating_add_signed(by).min(last);
    }

    /// Goes into folder `dir`, highlighting `pick` once it is read.
    fn go(&mut self, dir: PathBuf, pick: Option<OsString>) {
        *self = FileView {
            hidden: self.hidden,
            body: self.body,
            pick,
            ..FileView::new(dir)
        };
    }

    fn up(&mut self) -> Act {
        let Some(parent) = self.dir.parent().map(Path::to_owned) else {
            return Act::None;
        };
        let pick = self.dir.file_name().map(OsString::from);
        self.go(parent, pick);
        Act::Redraw
    }

    /// Goes into the highlighted folder, or moves to the preview of the
    /// highlighted file.
    fn enter(&mut self) -> Act {
        let Some(e) = self.selected() else {
            return Act::None;
        };
        match e.kind {
            Kind::Dir => {
                let dir = self.dir.join(&e.name);
                self.go(dir, None);
                Act::Redraw
            }
            Kind::File => {
                self.side = Side::Preview;
                Act::Redraw
            }
            Kind::Other => Act::None,
        }
    }

    fn open(&self) -> Act {
        match self.selected() {
            Some(e) if e.kind == Kind::File => {
                let line = self.mark.unwrap_or(self.scroll) + 1;
                Act::Open(self.dir.join(&e.name), line as u32)
            }
            _ => Act::None,
        }
    }

    /// Hidden entries show, or no longer do, keeping the highlight on the
    /// same entry where it still shows.
    fn toggle_hidden(&mut self) {
        let name = self.selected().map(|e| e.name.clone());
        self.hidden = !self.hidden;
        let shown = self.shown();
        let entries = self.listing.as_ref().map_or(&[][..], |l| &l.entries);
        let at = name.and_then(|n| shown.iter().position(|&i| entries[i].name == n));
        self.top = 0;
        self.select(at.unwrap_or(0));
    }

    /// A key press.
    pub fn key(&mut self, k: &KeyInput) -> Act {
        let m = &k.mods;
        let (ctrl, alt) = (m.lctrl || m.rctrl || m.lsuper || m.rsuper, m.lalt || m.ralt);
        let page = self.body.saturating_sub(1).max(1) as isize;
        let preview = self.side == Side::Preview;
        match k.key {
            Key::Enter if ctrl => self.open(),
            Key::Char(c)
                if ctrl && !alt && (c.eq_ignore_ascii_case(&'h') || k.us_base == Some('h')) =>
            {
                self.toggle_hidden();
                Act::Redraw
            }
            Key::Escape if preview => {
                self.side = Side::List;
                Act::Redraw
            }
            Key::Escape if !self.filter.is_empty() => {
                self.filter.clear();
                self.select(0);
                Act::Redraw
            }
            Key::Escape => Act::Close,
            Key::Tab | Key::Left if preview => {
                self.side = Side::List;
                Act::Redraw
            }
            Key::Tab | Key::Right if self.selected().is_some_and(|e| e.kind == Kind::File) => {
                self.side = Side::Preview;
                Act::Redraw
            }
            Key::Up if preview => self.moved(|f| f.scroll_by(-1)),
            Key::Down if preview => self.moved(|f| f.scroll_by(1)),
            Key::PageUp if preview => self.moved(|f| f.scroll_by(-page)),
            Key::PageDown if preview => self.moved(|f| f.scroll_by(page)),
            Key::Home if preview => self.moved(|f| f.scroll = 0),
            Key::End if preview => self.moved(|f| f.scroll_by(isize::MAX)),
            Key::Up => self.moved(|f| f.select_by(-1)),
            Key::Down => self.moved(|f| f.select_by(1)),
            Key::PageUp => self.moved(|f| f.select_by(-page)),
            Key::PageDown => self.moved(|f| f.select_by(page)),
            Key::Home => self.moved(|f| f.select(0)),
            Key::End => self.moved(|f| f.select(usize::MAX)),
            Key::Enter | Key::Right => self.enter(),
            Key::Left => self.up(),
            Key::Backspace if ctrl && !alt && !self.filter.is_empty() => {
                self.filter.clear();
                self.select(0);
                Act::Redraw
            }
            Key::Backspace if self.filter.is_empty() => self.up(),
            Key::Backspace => {
                self.filter.pop();
                self.side = Side::List;
                self.select(0);
                Act::Redraw
            }
            // Ctrl and Alt together are AltGr when the layout gives text.
            _ if (ctrl != alt || ctrl && k.uc == 0) || k.text.is_empty() => Act::None,
            _ => {
                self.type_text(k.text);
                Act::Redraw
            }
        }
    }

    fn moved(&mut self, f: impl FnOnce(&mut Self)) -> Act {
        f(self);
        Act::Redraw
    }

    /// Typed or pasted text narrows the list.
    pub fn type_text(&mut self, t: &str) {
        self.filter.extend(t.chars().filter(|c| !c.is_control()));
        self.side = Side::List;
        self.top = 0;
        self.select(0);
    }

    /// A left click on cell (`col`, `row`); `double` when it is the second
    /// of a double click.
    pub fn click(&mut self, col: u16, row: u16, double: bool) -> Act {
        let line = usize::from(row);
        if line == 0 || line > self.body {
            return Act::None;
        }
        if col < self.list_cols {
            let i = self.top + line - 1;
            if i >= self.shown().len() {
                return Act::None;
            }
            self.side = Side::List;
            self.select(i);
            if double {
                return self.enter();
            }
            Act::Redraw
        } else if self.preview_lines() > 0 {
            self.side = Side::Preview;
            Act::Redraw
        } else {
            Act::None
        }
    }

    /// The wheel turned `lines` over column `col`: up is positive.
    pub fn wheel(&mut self, col: u16, lines: isize) -> Act {
        if col < self.list_cols {
            let last = self.shown().len().saturating_sub(self.body.max(1));
            self.top = self.top.saturating_add_signed(-lines).min(last);
        } else {
            self.scroll_by(-lines);
        }
        Act::Redraw
    }

    /// Draws the view into `snap`, `cols` by `rows` cells.
    pub fn draw(&mut self, snap: &mut Snapshot, cols: u16, rows: u16, pal: &vt::Palette) {
        let at = draw::layout(cols, rows, self.side);
        self.body = at.body;
        self.list_cols = at.list.map_or(0, |(_, w)| w);
        // The pane may have shrunk since the highlight last moved.
        let body = self.body.max(1);
        if self.sel >= self.top + body {
            self.top = self.sel + 1 - body;
        }
        let last = self.preview_lines().saturating_sub(body);
        self.scroll = self.scroll.min(last);
        draw::paint(self, snap, &at, pal);
    }
}

#[cfg(test)]
mod tests;
