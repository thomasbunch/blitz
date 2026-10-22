//! The Files view as terminal cells: a header with the folder, the list
//! and the preview side by side, and a line of keys at the foot.

use vt::snapshot::{CLUSTER_BYTES, attr};
use vt::{CursorShape, Palette, RenderCell, Snapshot};

use super::{FileView, Kind, Preview, Side};

/// Below this many columns the list and the preview take turns.
const SIDE_BY_SIDE: u16 = 50;
/// Columns a tab moves to the next multiple of.
const TAB: u16 = 4;

/// The modifier the keys line names.
const MOD: &str = if cfg!(target_os = "macos") {
    "Cmd"
} else {
    "Ctrl"
};

/// Where the parts of the view go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct At {
    pub cols: u16,
    pub rows: u16,
    /// Rows between the header and the keys line.
    pub body: usize,
    /// The first column and the width of the list and of the preview,
    /// when they show.
    pub list: Option<(u16, u16)>,
    pub preview: Option<(u16, u16)>,
}

pub fn layout(cols: u16, rows: u16, side: Side) -> At {
    let body = usize::from(rows.saturating_sub(2));
    let (list, preview) = if cols >= SIDE_BY_SIDE {
        let w = (cols * 2 / 5).clamp(20, 48);
        (Some((0, w)), Some((w + 1, cols - w - 1)))
    } else if side == Side::List {
        (Some((0, cols)), None)
    } else {
        (None, Some((0, cols)))
    };
    At {
        cols,
        rows,
        body,
        list,
        preview,
    }
}

/// `a` moved `t` 256ths of the way to `b`, both `0xRRGGBB`.
fn blend(a: u32, b: u32, t: u32) -> u32 {
    let ch = |s: u32| {
        let (x, y) = ((a >> s & 0xff) as i32, (b >> s & 0xff) as i32);
        ((x + (y - x) * t as i32 / 256) as u32 & 0xff) << s
    };
    ch(16) | ch(8) | ch(0)
}

/// The colours the view draws with, from the theme's.
struct Colors {
    fg: u32,
    bg: u32,
    quiet: u32,
    bar: u32,
    dir: u32,
    link: u32,
    error: u32,
    sel_fg: u32,
    sel_bg: u32,
    /// The highlight while keys go to the preview.
    sel_idle: u32,
    mark: u32,
}

impl Colors {
    fn new(p: &Palette) -> Colors {
        Colors {
            fg: p.fg,
            bg: p.bg,
            quiet: blend(p.fg, p.bg, 112),
            bar: blend(p.bg, p.fg, 24),
            dir: p.ansi[12],
            link: p.ansi[14],
            error: p.ansi[9],
            sel_fg: p.selection_fg,
            sel_bg: p.selection_bg,
            sel_idle: blend(p.bg, p.fg, 40),
            mark: blend(p.bg, p.selection_bg, 128),
        }
    }
}

/// The grapheme clusters of `s`, as the terminal splits them.
fn clusters(s: &str) -> impl Iterator<Item = &str> {
    let mut rest = s;
    std::iter::from_fn(move || {
        let mut it = rest.char_indices();
        let (_, first) = it.next()?;
        let (mut last, mut len, mut end) = (first, 1, rest.len());
        for (i, c) in it {
            if !vt::width::joins(first, last, len, c) {
                end = i;
                break;
            }
            (last, len) = (c, len + 1);
        }
        let (cluster, tail) = rest.split_at(end);
        rest = tail;
        Some(cluster)
    })
}

/// Columns `s` takes, as [`Canvas::text`] draws it from column 0.
fn width(s: &str) -> u16 {
    clusters(s).fold(0u16, |w, c| match c {
        "\t" => (w / TAB + 1) * TAB,
        _ => w.saturating_add(u16::from(vt::cluster_width(c))),
    })
}

struct Canvas<'a> {
    cells: &'a mut [RenderCell],
    cols: u16,
    rows: u16,
}

impl Canvas<'_> {
    fn cell(&mut self, col: u16, row: u16) -> Option<&mut RenderCell> {
        if col >= self.cols || row >= self.rows {
            return None;
        }
        self.cells
            .get_mut(usize::from(row) * usize::from(self.cols) + usize::from(col))
    }

    /// Paints columns `from` to `to`, not including `to`, of `row`.
    fn fill(&mut self, row: u16, from: u16, to: u16, bg: u32) {
        for col in from..to {
            if let Some(c) = self.cell(col, row) {
                *c = RenderCell {
                    fg: bg,
                    bg,
                    ul: bg,
                    width: 1,
                    ..RenderCell::default()
                };
            }
        }
    }

    /// Writes `s` on `row` from column `col`, stopping before column
    /// `end`; a text cut short ends in an ellipsis. Tabs go to the next
    /// multiple of [`TAB`] from `col`, and control characters show as
    /// replacement characters. Returns the column after the text.
    fn text(&mut self, row: u16, col: u16, end: u16, s: &str, fg: u32, bold: bool) -> u16 {
        let end = end.min(self.cols);
        let mut c = col;
        for cluster in clusters(s) {
            if cluster == "\t" {
                c = col + ((c - col) / TAB + 1) * TAB;
                continue;
            }
            let shown = if cluster.starts_with(char::is_control) {
                "\u{fffd}"
            } else {
                cluster
            };
            let w = u16::from(vt::cluster_width(shown));
            if w == 0 {
                continue;
            }
            if c + w > end {
                if end > col {
                    self.glyph(row, end - 1, "\u{2026}", 1, fg, bold);
                }
                return end;
            }
            self.glyph(row, c, shown, w as u8, fg, bold);
            c += w;
        }
        c.min(end)
    }

    fn glyph(&mut self, row: u16, col: u16, s: &str, w: u8, fg: u32, bold: bool) {
        // A cluster too long for a cell keeps its first character.
        let s = if s.len() > CLUSTER_BYTES {
            let n = s.chars().next().map_or(0, char::len_utf8);
            &s[..n]
        } else {
            s
        };
        if let Some(cell) = self.cell(col, row) {
            let mut text = [0; CLUSTER_BYTES];
            text[..s.len()].copy_from_slice(s.as_bytes());
            cell.text = text;
            cell.len = s.len() as u8;
            cell.width = w;
            cell.fg = fg;
            cell.ul = fg;
            cell.attrs = if bold { attr::BOLD } else { 0 };
        }
        if w == 2
            && let Some(right) = self.cell(col + 1, row)
        {
            right.width = 0;
            right.len = 0;
        }
    }
}

/// `n` bytes for people: `912 B`, `4.2 KB`, `18 MB`.
fn size(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u + 1 < UNITS.len() {
        v /= 1024.0;
        u += 1;
    }
    match u {
        0 => format!("{n} B"),
        _ if v < 10.0 => format!("{v:.1} {}", UNITS[u]),
        _ => format!("{v:.0} {}", UNITS[u]),
    }
}

/// `s` cut from the start to fit `cols`, as a long path keeps its end.
fn tail(s: &str, cols: u16) -> String {
    if width(s) <= cols {
        return s.to_owned();
    }
    let cs: Vec<&str> = clusters(s).collect();
    let mut w = 1;
    let mut start = cs.len();
    while start > 0 {
        let cw = width(cs[start - 1]);
        if w + cw > cols {
            break;
        }
        w += cw;
        start -= 1;
    }
    format!("\u{2026}{}", cs[start..].concat())
}

pub fn paint(f: &FileView, snap: &mut Snapshot, at: &At, pal: &Palette) {
    let c = Colors::new(pal);
    let (cols, rows) = (at.cols, at.rows);
    snap.cols = cols;
    snap.rows = rows;
    snap.cells.clear();
    let blank = RenderCell {
        fg: c.fg,
        bg: c.bg,
        ul: c.fg,
        width: 1,
        ..RenderCell::default()
    };
    snap.cells
        .resize(usize::from(cols) * usize::from(rows), blank);
    snap.wrapped.clear();
    snap.wrapped.resize(usize::from(rows), false);
    snap.cursor = None;
    snap.selection = None;
    snap.highlights.clear();
    snap.hover = None;
    snap.top = 0;
    let mut cv = Canvas {
        cells: &mut snap.cells,
        cols,
        rows,
    };
    if rows == 0 || cols == 0 {
        return;
    }

    // The header: the folder, and how many entries show.
    cv.fill(0, 0, cols, c.bar);
    let shown = f.shown();
    let count = match &f.listing {
        None => "Reading\u{2026}".to_owned(),
        Some(l) if l.error.is_some() => String::new(),
        Some(l) => {
            let total = l.entries.len() + l.more;
            let n = if f.filter.is_empty() && f.hidden {
                total
            } else {
                shown.len()
            };
            match (n, f.filter.is_empty()) {
                (1, true) => "1 item".into(),
                (n, true) => format!("{n} items"),
                (n, false) => format!("{n} of {total}"),
            }
        }
    };
    let count_w = width(&count);
    let right = cols.saturating_sub(count_w + 1);
    let x = cv.text(0, 1, right, "Files", c.fg, true);
    let room = right.saturating_sub(x + 3);
    let dir = f.dir.display().to_string();
    cv.text(
        0,
        x + 2,
        right.saturating_sub(1),
        &tail(&dir, room),
        c.fg,
        false,
    );
    if count_w + 2 <= cols {
        cv.text(0, right, cols, &count, c.quiet, false);
    }
    if rows < 2 {
        return;
    }

    // The keys line, or what was typed to narrow the list.
    let foot = rows - 1;
    cv.fill(foot, 0, cols, c.bar);
    if f.filter.is_empty() {
        let keys = match f.side {
            Side::List => format!(
                "Enter open \u{b7} Backspace up \u{b7} type to filter \u{b7} Ctrl+H hidden \
                 \u{b7} {MOD}+Enter editor \u{b7} Esc close"
            ),
            Side::Preview => format!(
                "\u{2191}\u{2193} scroll \u{b7} Tab list \u{b7} {MOD}+Enter editor \u{b7} Esc back"
            ),
        };
        cv.text(foot, 1, cols, &keys, c.quiet, false);
    } else {
        let x = cv.text(foot, 1, cols, "Filter: ", c.quiet, false);
        let x = cv.text(foot, x, cols.saturating_sub(1), &f.filter, c.fg, false);
        snap.cursor = Some((x, foot, CursorShape::Bar));
    }
    let mut cv = Canvas {
        cells: &mut snap.cells,
        cols,
        rows,
    };

    let body = at.body as u16;
    if let (Some((lx, lw)), Some((px, _))) = (at.list, at.preview) {
        // The divider, in the column between.
        for row in 1..=body {
            cv.text(row, lx + lw, px, "\u{2502}", c.bar, false);
        }
    }
    if let Some((lx, lw)) = at.list {
        list(f, &mut cv, &c, &shown, lx, lw, body);
    }
    if let Some((px, pw)) = at.preview {
        preview(f, &mut cv, &c, px, pw, body);
    }
}

fn list(f: &FileView, cv: &mut Canvas, c: &Colors, shown: &[usize], x: u16, w: u16, body: u16) {
    let end = x + w;
    let Some(l) = &f.listing else {
        return;
    };
    if let Some(e) = &l.error {
        cv.text(1, x + 1, end, e, c.error, false);
        return;
    }
    if shown.is_empty() {
        let why = if l.entries.is_empty() {
            "Empty folder"
        } else {
            "Nothing matches"
        };
        cv.text(1, x + 1, end, why, c.quiet, false);
    }
    let rows = shown.iter().enumerate().skip(f.top).take(usize::from(body));
    for (row, (i, &e)) in (1..).zip(rows) {
        let e = &l.entries[e];
        let sel = i == f.sel;
        let (fg, bg) = match (sel, f.side) {
            (true, Side::List) => (c.sel_fg, c.sel_bg),
            (true, Side::Preview) => (c.fg, c.sel_idle),
            _ => (c.fg, c.bg),
        };
        if bg != c.bg {
            cv.fill(row, x, end, bg);
        }
        let name_fg = match e.kind {
            _ if sel && f.side == Side::List => fg,
            Kind::Dir => c.dir,
            _ if e.link => c.link,
            Kind::Other => c.quiet,
            Kind::File => fg,
        };
        let size = (e.kind == Kind::File).then(|| size(e.size));
        let size_w = size.as_deref().map_or(0, width);
        // The size shows when it leaves the name some room.
        let size_at = end.saturating_sub(size_w + 1);
        let fits = size.is_some() && size_at > x + 12;
        let name_end = if fits { size_at - 1 } else { end - 1 };
        let mut name = e.label.clone();
        if e.kind == Kind::Dir {
            name.push(std::path::MAIN_SEPARATOR);
        }
        cv.text(row, x + 1, name_end, &name, name_fg, e.kind == Kind::Dir);
        if let Some(s) = size.filter(|_| fits) {
            let quiet = if sel && f.side == Side::List {
                fg
            } else {
                c.quiet
            };
            cv.text(row, size_at, end, &s, quiet, false);
        }
    }
    let past = shown.len().saturating_sub(f.top);
    if l.more > 0 && past < usize::from(body) {
        let row = past as u16 + 1;
        let more = format!("{} more not shown", l.more);
        cv.text(row, x + 1, end, &more, c.quiet, false);
    }
}

fn preview(f: &FileView, cv: &mut Canvas, c: &Colors, x: u16, w: u16, body: u16) {
    let end = x + w;
    let note = |cv: &mut Canvas, text: &str, fg: u32| {
        cv.text(1, x + 1, end, text, fg, false);
    };
    let Some(e) = f.selected() else {
        return;
    };
    match e.kind {
        Kind::Dir => return note(cv, "Folder \u{b7} Enter opens it", c.quiet),
        Kind::Other => return note(cv, "Nothing to show", c.quiet),
        Kind::File => {}
    }
    let lines = match f.shown_preview() {
        None => return note(cv, "Reading\u{2026}", c.quiet),
        Some(Preview::Error(e)) => return note(cv, e, c.error),
        Some(Preview::Binary(n)) => {
            let text = format!("Binary file \u{b7} {}", size(*n));
            return note(cv, &text, c.quiet);
        }
        Some(Preview::Text { lines, .. }) if lines.is_empty() => {
            return note(cv, "Empty file", c.quiet);
        }
        Some(Preview::Text { lines, cut }) => (lines, *cut),
    };
    let (lines, cut) = lines;
    let digits = lines.len().to_string().len() as u16;
    let text_x = x + 1 + digits + 2;
    let shown = lines
        .iter()
        .enumerate()
        .skip(f.scroll)
        .take(usize::from(body));
    for (row, (i, line)) in (1..).zip(shown) {
        if f.mark == Some(i) {
            cv.fill(row, x, end, c.mark);
        }
        let n = format!("{:>w$}", i + 1, w = usize::from(digits));
        cv.text(row, x + 1, text_x, &n, c.quiet, false);
        cv.text(row, text_x, end, line, c.fg, false);
    }
    let past = lines.len().saturating_sub(f.scroll);
    if cut && past < usize::from(body) {
        let text = format!("Only the first {} are shown", size(super::load::MAX_BYTES));
        cv.text(past as u16 + 1, text_x, end, &text, c.quiet, false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_as_people_say_them() {
        assert_eq!(size(0), "0 B");
        assert_eq!(size(1023), "1023 B");
        assert_eq!(size(1024), "1.0 KB");
        assert_eq!(size(4300), "4.2 KB");
        assert_eq!(size(18 << 20), "18 MB");
    }

    #[test]
    fn clusters_keep_marks_and_emoji_together() {
        let got: Vec<&str> = clusters("e\u{301}x\u{1f44d}\u{1f3fd}!").collect();
        assert_eq!(got, ["e\u{301}", "x", "\u{1f44d}\u{1f3fd}", "!"]);
    }

    #[test]
    fn long_paths_keep_their_end() {
        assert_eq!(tail("/a/b", 10), "/a/b");
        assert_eq!(tail("/home/me/dev/shop", 8), "\u{2026}ev/shop");
    }

    #[test]
    fn narrow_panes_show_one_side_at_a_time() {
        let wide = layout(100, 30, Side::List);
        assert_eq!((wide.list, wide.preview), (Some((0, 40)), Some((41, 59))));
        assert_eq!(wide.body, 28);
        let narrow = layout(40, 10, Side::Preview);
        assert_eq!((narrow.list, narrow.preview), (None, Some((0, 40))));
        assert_eq!(layout(40, 10, Side::List).list, Some((0, 40)));
    }

    #[test]
    fn widths_count_tabs_and_wide_characters() {
        assert_eq!(width("a\tb"), 5);
        assert_eq!(width("\u{4e2d}x"), 3);
    }
}
