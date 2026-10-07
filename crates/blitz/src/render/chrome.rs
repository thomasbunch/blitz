//! Window chrome: the session sidebar or its collapsed rail, pane headers,
//! dividers, attention marks, the banner strip and the IME preedit, laid out
//! as plain rectangles, rounded shapes and text runs.
//!
//! Nothing here touches the GPU, so layouts are testable on any platform.

use std::time::{Duration, Instant};

use crate::attention::{Attn, PaneAttn};
use crate::layout::{PaneId, Rect, Tab, Window};
use crate::theme::{Theme, Ui};

/// One session as the chrome shows it.
#[derive(Clone, Debug)]
pub struct Session {
    pub id: PaneId,
    pub name: String,
    /// Set when another session has the same name: this one's number,
    /// drawn dimmer after it to tell them apart.
    pub num: Option<u32>,
    pub cwd: String,
    pub branch: Option<String>,
    pub state: Attn,
    /// When `state` last changed.
    pub since: Instant,
    /// When the turn under way began, and how long the last one took.
    pub turn: Option<Instant>,
    pub took: Option<Duration>,
    /// The user has looked at `state`; a question they saw is drawn
    /// outlined until they answer it.
    pub seen: bool,
    /// Latest one-line message: the hook message, else the title.
    pub msg: String,
    /// What the program last reported of its progress.
    pub progress: Option<Progress>,
    pub exit_code: Option<u32>,
}

/// A program's progress, as OSC 9;4 reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    /// 1 normal, 2 error, 3 indeterminate, 4 paused or warning.
    pub state: u8,
    /// Percent done, when known.
    pub pct: Option<u8>,
}

impl Progress {
    /// The progress after a report of `state` and `pct`, which was `prev`.
    /// State 0 removes it. As in ConEmu, an error or a pause without a
    /// value keeps the last one, and an indeterminate state has none.
    pub fn next(prev: Option<Progress>, state: u8, pct: Option<u8>) -> Option<Progress> {
        let pct = match state {
            0 => return None,
            1 => Some(pct.unwrap_or(0)),
            3 => None,
            _ => pct.or(prev.and_then(|p| p.pct)),
        };
        Some(Progress { state, pct })
    }
}

pub struct ChromeModel<'a> {
    /// Tabs, the active tab and whether the sidebar is expanded.
    pub win: &'a Window,
    pub sessions: &'a [Session],
    pub ui: Ui,
    /// Window client size in pixels.
    pub size: (i32, i32),
    /// DPI scale, 1.0 at 96 DPI.
    pub scale: f32,
    /// Cell size of the sidebar font.
    pub text_cell: (u32, u32),
    /// Cell size of the terminal font.
    pub term_cell: (u32, u32),
    pub now: Instant,
    /// One line of text in a strip under the panes, such as an available
    /// update.
    pub banner: Option<&'a str>,
    /// IME composition in the focused pane: column, row and text.
    pub preedit: Option<(u16, u16, &'a str)>,
    pub picker: Option<Picker<'a>>,
    pub settings: Option<Settings<'a>>,
    /// The spark at the foot of the sidebar, when it is on: seconds into
    /// its animation.
    pub spark: Option<f64>,
    /// blitz run, while it is open.
    pub game: Option<&'a crate::arcade::run::Run>,
    pub commands: Option<Commands<'a>>,
    pub find: Option<FindBar<'a>>,
}

/// The find bar, drawn at the top right of the focused pane.
pub struct FindBar<'a> {
    /// What was typed.
    pub query: &'a str,
    /// The current match, counting from 1, and how many there are; `None`
    /// when nothing matches.
    pub count: Option<(usize, usize)>,
}

/// The theme picker, drawn over everything.
pub struct Picker<'a> {
    /// What was typed to narrow the list.
    pub filter: &'a str,
    /// The themes that match it.
    pub items: Vec<&'a Theme>,
    /// The highlighted item, which is the theme being shown.
    pub sel: usize,
}

/// The command palette, drawn over everything.
pub struct Commands<'a> {
    /// What was typed to narrow the list.
    pub filter: &'a str,
    /// The actions that match it, each with the keys that run it, if any.
    pub items: Vec<(&'a str, String)>,
    /// The highlighted item.
    pub sel: usize,
    /// What the typed line renames, such as `Rename session`, instead of
    /// narrowing the list.
    pub rename: Option<&'a str>,
}

/// Rows the theme picker and the command palette show at once.
pub const PICKER_ROWS: usize = 12;

/// The settings panel, drawn over everything but the theme picker.
pub struct Settings<'a> {
    /// What was typed to narrow the list.
    pub filter: &'a str,
    /// The settings that match it.
    pub rows: Vec<SettingRow>,
    /// The highlighted row.
    pub sel: usize,
    /// The first list line shown in the last frame.
    pub top: usize,
    /// Why the last change was not saved.
    pub error: Option<&'a str>,
}

/// One setting as the panel shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct SettingRow {
    pub group: &'static str,
    pub label: &'static str,
    pub help: &'static str,
    /// When a change takes effect.
    pub applies: &'static str,
    /// A switch's state; `None` shows `value` between arrows instead.
    pub on: Option<bool>,
    pub value: String,
    /// Whether a step left or right would change the value.
    pub less: bool,
    pub more: bool,
    /// The default value, as shown.
    pub default: String,
    /// The value is not the default.
    pub changed: bool,
}

/// Where the settings panel went, for clicks.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SettingsHits {
    pub panel: Rect,
    /// Each row shown: its index, the whole row, and its switch or value.
    pub rows: Vec<(usize, Rect, Rect)>,
    /// The first list line shown.
    pub top: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Prim {
    Rect(Rect, u32),
    /// A rounded rectangle, filled when `stroke` is 0. A square whose
    /// radius is half its side is a dot, or with a stroke a ring.
    Shape {
        r: Rect,
        radius: f32,
        stroke: f32,
        color: u32,
    },
    /// One line of text with its top-left corner at (`x`, `y`), in the
    /// sidebar font, or the terminal font when `term` is set.
    Text {
        x: i32,
        y: i32,
        text: String,
        color: u32,
        bold: bool,
        term: bool,
    },
    /// A git branch icon filling the square `r`.
    Branch(Rect, u32),
}

#[derive(Debug, Default)]
pub struct Chrome {
    /// Drawn in order, after the terminal grids.
    pub prims: Vec<Prim>,
    /// Where each visible pane's terminal grid goes.
    pub panes: Vec<(PaneId, Rect)>,
    /// Each session's row in the sidebar or rail, for clicks.
    pub rows: Vec<(PaneId, Rect)>,
    /// The banner strip, for clicks.
    pub banner: Option<Rect>,
    pub settings: Option<SettingsHits>,
    /// The command palette and each row it shows, by index, for clicks.
    pub commands: Option<(Rect, Vec<(usize, Rect)>)>,
}

/// Least width of the expanded sidebar, and width of the collapsed rail,
/// at 96 DPI, each including its 1 px border.
pub const SIDEBAR_W: f32 = 240.0;
pub const RAIL_W: f32 = 15.0;
/// Characters of the sidebar font that fit across the expanded sidebar.
const SIDEBAR_CELLS: i32 = 34;
/// Height of the banner strip at 96 DPI.
pub const BANNER_H: f32 = 22.0;

/// Height of a pane's header strip at 96 DPI.
const HEADER_H: f32 = 22.0;

/// Width of the expanded sidebar in a window `width` px wide, its border
/// included: room for `SIDEBAR_CELLS` characters `tw` px wide, and at
/// least `SIDEBAR_W`, but never over 40% of the window.
pub fn sidebar_w(width: i32, scale: f32, tw: i32) -> i32 {
    let least = (SIDEBAR_W * scale).round() as i32;
    (SIDEBAR_CELLS * tw).max(least).min(width * 2 / 5)
}

/// The part of a `size` window that the active tab's panes share: all of
/// it but the sidebar or rail, shown once there are two sessions, and the
/// banner strip. `tw` is the width of a sidebar character.
pub fn area(win: &Window, size: (i32, i32), scale: f32, banner: bool, tw: i32) -> Rect {
    let s = |v: f32| (v * scale).round() as i32;
    let fleet = win.has_sidebar();
    let side = match (fleet, win.sidebar_expanded) {
        (false, _) => 0,
        (true, true) => sidebar_w(size.0, scale, tw),
        (true, false) => s(RAIL_W),
    };
    let bh = if banner { s(BANNER_H) } else { 0 };
    Rect {
        x: side,
        y: 0,
        w: (size.0 - side).max(0),
        h: (size.1 - bh).max(0),
    }
}

/// What a pane's tile holds besides its terminal grid: the padding left
/// and right, and the header strip and padding above the grid. A tab of
/// one pane has no header.
pub fn pane_frame(scale: f32, expanded: bool, multi: bool) -> (i32, i32) {
    let s = |v: f32| (v * scale).round() as i32;
    let header = if multi && expanded { s(HEADER_H) } else { 0 };
    let (px, py) = if expanded { (14.0, 8.0) } else { (16.0, 12.0) };
    (2 * s(px), header + s(py))
}

/// Lays out the chrome for one frame.
pub fn build(m: &ChromeModel) -> Chrome {
    let c = &m.ui;
    let s = |v: f32| (v * m.scale).round() as i32;
    let (tw, th) = (m.text_cell.0 as i32, m.text_cell.1 as i32);
    let h = m.size.1;
    let mut out = Chrome::default();
    let Some(tab) = m.win.tabs.get(m.win.active) else {
        return out;
    };
    let fleet = m.win.has_sidebar();
    let expanded = m.win.sidebar_expanded;
    let area = area(m.win, m.size, m.scale, m.banner.is_some(), tw);
    let (side, bh) = (area.x, m.banner.map_or(0, |_| s(BANNER_H)));
    let session = |id: PaneId| m.sessions.iter().find(|x| x.id == id);
    // A tab's sessions in reading order, as its panes sit.
    let members =
        |t: &Tab| -> Vec<&Session> { t.panes().into_iter().filter_map(session).collect() };
    let p = &mut out.prims;
    let text = |p: &mut Vec<Prim>, x, y, t: &str, color, bold| {
        if t.is_empty() {
            return;
        }
        p.push(Prim::Text {
            x,
            y,
            text: t.to_string(),
            color,
            bold,
            term: false,
        });
    };
    // A dot or ring of diameter `d` centred on (cx, cy).
    let mark = |p: &mut Vec<Prim>, cx: i32, cy: i32, d: f32, stroke: f32, color| {
        let d = s(d);
        p.push(Prim::Shape {
            r: Rect {
                x: cx - d / 2,
                y: cy - d / 2,
                w: d,
                h: d,
            },
            radius: d as f32 / 2.0,
            stroke: stroke * m.scale,
            color,
        });
    };
    // A 2 px line: the track, then the part done filled, or all of it
    // when the program has not said. An error or a pause colours it, and
    // a program that cannot tell how far it is gets a dimmer full line:
    // nothing here moves.
    let progress = |p: &mut Vec<Prim>, r: Rect, pr: Option<Progress>, track: Option<u32>, fill| {
        if let Some(track) = track {
            p.push(Prim::Rect(r, track));
        }
        let (pct, fill) = match pr {
            Some(Progress { state: 2, pct }) => (pct, c.error),
            Some(Progress { state: 3, .. }) => (None, super::mix(track.unwrap_or(c.track), fill)),
            Some(Progress { state: 4, pct }) => (pct, c.accent),
            Some(Progress { pct, .. }) => (pct, fill),
            None => (None, fill),
        };
        let pct = i32::from(pct.unwrap_or(100).min(100));
        p.push(Prim::Rect(
            Rect {
                w: r.w * pct / 100,
                ..r
            },
            fill,
        ));
    };

    for d in tab.dividers(area) {
        p.push(Prim::Rect(d, c.border));
    }
    let rects = tab.rects(area);
    let multi = rects.len() >= 2;
    for &(id, r) in &rects {
        let sess = session(id);
        let state = sess.map_or(Attn::Idle, |x| x.state);
        let reported = sess.and_then(|x| x.progress);
        let focused = id == tab.focus;
        if multi && expanded {
            let hh = s(HEADER_H);
            if focused {
                p.push(Prim::Rect(Rect { h: hh, ..r }, c.hdr_bg));
            }
            p.push(Prim::Rect(
                Rect {
                    y: r.y + hh - 1,
                    h: 1,
                    ..r
                },
                c.hdr_line,
            ));
            let ty = r.y + (hh - 1 - th) / 2;
            let x = r.x + s(14.0);
            let right = r.right() - s(14.0);
            if let Some(x2) = sess {
                let (name, num) = name_parts(x2, right - x, tw);
                let (nc, cc) = if focused {
                    (c.name, c.dim)
                } else {
                    (c.hdr_name, c.hdr_cwd)
                };
                text(p, x, ty, &name, nc, focused);
                let nx = x + text_w(&name, tw);
                text(p, nx, ty, &num, cc, false);
                let cx = nx + text_w(&num, tw) + s(8.0);
                let room = right - s(15.0) - cx;
                text(p, cx, ty, &fit_left(&x2.cwd, room, tw), cc, false);
            }
            let cy = r.y + (hh - 1) / 2;
            match state {
                Attn::NeedsYou => mark(p, right - s(4.0), cy, 7.0, ring(sess), c.accent),
                Attn::DoneUnseen => mark(p, right - s(4.0), cy, 7.0, 1.5, c.name),
                Attn::Error => mark(p, right - s(4.0), cy, 7.0, 0.0, c.error),
                Attn::Working | Attn::Idle => {}
            }
            if state == Attn::Working || reported.is_some() {
                let line = Rect {
                    y: r.y + hh - 2,
                    h: s(2.0),
                    ..r
                };
                progress(p, line, reported, None, c.fill);
            }
        }
        if fleet && !expanded && (state == Attn::Working || reported.is_some()) {
            let line = Rect { h: s(2.0), ..r };
            progress(p, line, reported, Some(c.top_track), c.rail_work);
        }
        if multi && !expanded {
            if let Some(x) = sess {
                let (label, num) = name_parts(x, r.w / 2, tw);
                let color = if focused { c.label_focus } else { c.label };
                let nx = r.right() - s(16.0) - text_w(&num, tw);
                let (lx, ly) = (nx - text_w(&label, tw), r.bottom() - s(10.0) - th);
                text(p, lx, ly, &label, color, false);
                text(p, nx, ly, &num, c.dim, false);
            }
            if state == Attn::NeedsYou {
                let b = s(2.0);
                for e in [
                    Rect { h: b, ..r },
                    Rect {
                        y: r.bottom() - b,
                        h: b,
                        ..r
                    },
                    Rect {
                        y: r.y + b,
                        w: b,
                        h: r.h - 2 * b,
                        ..r
                    },
                    Rect {
                        x: r.right() - b,
                        y: r.y + b,
                        w: b,
                        h: r.h - 2 * b,
                    },
                ] {
                    p.push(Prim::Rect(e, c.accent));
                }
            }
        }
        // The grid stays inside its tile, however small the tile is.
        let (fw, fh) = pane_frame(m.scale, expanded, multi);
        let (x, y) = ((r.x + fw / 2).min(r.right()), (r.y + fh).min(r.bottom()));
        let content = Rect {
            x,
            y,
            w: (r.w - fw).clamp(0, r.right() - x),
            h: (r.h - fh).clamp(0, r.bottom() - y),
        };
        out.panes.push((id, content));
    }

    if fleet && expanded {
        let bar = Rect {
            x: 0,
            y: 0,
            w: side,
            h,
        };
        p.push(Prim::Rect(bar, c.side_bg));
        p.push(Prim::Rect(
            Rect {
                x: side - 1,
                w: 1,
                ..bar
            },
            c.border,
        ));
        let mut y = s(10.0);
        for (ti, t) in m.win.tabs.iter().enumerate() {
            let list = members(t);
            if ti > 0 {
                y += s(14.0);
            }
            // Group heading: name, a hairline, the session count.
            let (gx, gr) = (s(32.0), side - s(19.0));
            let gh = s(26.0);
            let gy = y + (gh - th) / 2;
            let count = list.len().to_string();
            let cx = gr - text_w(&count, tw);
            // A tab nobody named is called after where its focused pane is.
            let named = match t.name.as_str() {
                "" => folder_name(session(t.focus).map_or("", |x| &x.cwd)),
                n => n.to_owned(),
            };
            let name = fit(&named, cx - s(16.0) - gx, tw);
            let nc = if ti == m.win.active { c.msg } else { c.dim };
            text(p, gx, gy, &name, nc, false);
            text(p, cx, gy, &count, c.dim, false);
            let rx = gx + text_w(&name, tw) + s(8.0);
            let rule = Rect {
                x: rx,
                y: y + gh / 2,
                w: (cx - s(8.0) - rx).max(0),
                h: 1,
            };
            p.push(Prim::Rect(rule, c.rule));
            y += gh;

            for x in list {
                if y >= h {
                    break;
                }
                let focused = ti == m.win.active && x.id == t.focus;
                let (l1, l2, l3, gap) = (s(18.0).max(th), s(16.0).max(th), s(17.0).max(th), s(2.0));
                let bar = x.state == Attn::Working || x.progress.is_some();
                // Every row has room for a message and a progress bar, so
                // output that changes a title or state cannot move the
                // rows below it under the pointer.
                let rh =
                    s(7.0) + l1 + gap + l2 + gap + l3 + gap + s(5.0) + s(2.0) + s(1.0) + s(8.0);
                let row = Rect {
                    x: s(8.0),
                    y,
                    w: (side - s(18.0)).max(0),
                    h: rh,
                };
                out.rows.push((x.id, row));
                if focused {
                    p.push(Prim::Shape {
                        r: row,
                        radius: 6.0 * m.scale,
                        stroke: 0.0,
                        color: c.row_focus,
                    });
                }
                let (mx, my) = (row.x + s(12.0), y + s(7.0) + s(5.0) + s(4.0));
                match x.state {
                    Attn::NeedsYou => mark(p, mx, my, 8.0, ring(Some(x)), c.accent),
                    Attn::DoneUnseen => mark(p, mx, my, 8.0, 1.5, c.name),
                    Attn::Error => mark(p, mx, my, 8.0, 0.0, c.error),
                    Attn::Working | Attn::Idle => {}
                }

                let (left, right) = (row.x + s(24.0), row.right() - s(10.0));
                let mut ly = y + s(7.0);
                // Line 1: name, and the state on the right.
                let word = state_word(x, m.now);
                let ty = ly + (l1 - th) / 2;
                let state_x = if x.state == Attn::NeedsYou {
                    let chip_w = text_w(&word, tw) + s(12.0);
                    let chip_h = s(16.0).max(th);
                    let chip = Rect {
                        x: right - chip_w,
                        y: ly + (l1 - chip_h) / 2,
                        w: chip_w,
                        h: chip_h,
                    };
                    p.push(Prim::Shape {
                        r: chip,
                        radius: 4.0 * m.scale,
                        stroke: ring(Some(x)) * m.scale,
                        color: c.accent,
                    });
                    let cy = chip.y + (chip_h - th) / 2;
                    // Off the fill, the accent is too faint for text on
                    // the light themes.
                    let fg = if x.seen { c.name } else { c.chip_fg };
                    text(p, chip.x + s(6.0), cy, &word, fg, true);
                    chip.x
                } else {
                    let wx = right - text_w(&word, tw);
                    text(p, wx, ty, &word, c.dim, false);
                    wx
                };
                let (name, num) = name_parts(x, state_x - s(8.0) - left, tw);
                text(p, left, ty, &name, c.name, true);
                text(p, left + text_w(&name, tw), ty, &num, c.dim, false);
                ly += l1 + gap;
                // Line 2: directory and branch.
                let ty = ly + (l2 - th) / 2;
                let (gap6, icon) = (s(6.0), s(10.0));
                // A long branch name leaves the folder room to show.
                let branch = (x.branch.as_deref()).map(|b| fit(b, (right - left) * 45 / 100, tw));
                let bw = (branch.as_deref()).map_or(0, |b| 2 * gap6 + icon + text_w(b, tw));
                let cwd = fit_left(&x.cwd, right - left - bw, tw);
                text(p, left, ty, &cwd, c.dim, false);
                if let Some(b) = branch.as_deref() {
                    let ix = left + text_w(&cwd, tw) + gap6;
                    let r = Rect {
                        x: ix,
                        y: ly + (l2 - icon) / 2,
                        w: icon,
                        h: icon,
                    };
                    p.push(Prim::Branch(r, c.dim));
                    let bx = ix + icon + gap6;
                    text(p, bx, ty, &fit(b, right - bx, tw), c.dim, false);
                }
                ly += l2;
                // Line 3: the last message.
                ly += gap;
                if !x.msg.is_empty() {
                    let msg = fit(&x.msg, right - left, tw);
                    text(p, left, ly + (l3 - th) / 2, &msg, c.msg, false);
                }
                ly += l3;
                if bar {
                    let line = Rect {
                        x: left,
                        y: ly + gap + s(5.0),
                        w: right - left,
                        h: s(2.0),
                    };
                    progress(p, line, x.progress, Some(c.track), c.fill);
                }
                y += rh + s(2.0);
            }
        }
        if let Some(t) = m.spark {
            let busiest = (m.sessions.iter()).map(|x| x.state).max();
            let free = Rect {
                x: 0,
                y,
                w: side - 1,
                h: h - y,
            };
            let state = busiest.unwrap_or_default();
            crate::arcade::mascot::draw(p, free, state, t, m.scale, c, (tw, th));
        }
    } else if fleet {
        let rail = Rect {
            x: 0,
            y: 0,
            w: side,
            h,
        };
        p.push(Prim::Rect(
            Rect {
                x: side - 1,
                w: 1,
                ..rail
            },
            c.border,
        ));
        let mut y = s(12.0);
        for (ti, t) in m.win.tabs.iter().enumerate() {
            if ti > 0 {
                y += s(12.0);
            }
            for x in members(t) {
                let row = Rect {
                    x: 0,
                    y,
                    w: side - 1,
                    h: s(20.0),
                };
                out.rows.push((x.id, row));
                if ti == m.win.active && x.id == t.focus {
                    p.push(Prim::Rect(row, c.rail_focus));
                }
                let (cx, cy) = (row.w / 2, y + row.h / 2);
                match x.state {
                    Attn::NeedsYou => mark(p, cx, cy, 7.0, ring(Some(x)), c.accent),
                    Attn::DoneUnseen => mark(p, cx, cy, 8.0, 1.5, c.name),
                    Attn::Error => mark(p, cx, cy, 7.0, 0.0, c.error),
                    Attn::Idle => mark(p, cx, cy, 3.0, 0.0, c.idle),
                    Attn::Working => {
                        let (bw, bh) = (s(8.0), s(2.0));
                        let r = Rect {
                            x: cx - bw / 2,
                            y: cy - bh / 2,
                            w: bw,
                            h: bh,
                        };
                        p.push(Prim::Rect(r, c.rail_work));
                    }
                }
                y += row.h;
            }
        }
    }

    let (cw, ch) = (m.term_cell.0 as i32, m.term_cell.1.max(1) as i32);
    let pane = |id: PaneId| out.panes.iter().find(|x| x.0 == id).map(|x| x.1);
    let mut extra = Vec::new();
    if let Some(msg) = m.banner {
        let strip = Rect {
            y: area.bottom(),
            h: bh,
            ..area
        };
        out.banner = Some(strip);
        extra.push(Prim::Rect(strip, c.hdr_bg));
        extra.push(Prim::Rect(Rect { h: 1, ..strip }, c.hdr_line));
        let x = strip.x + s(14.0);
        let msg = fit(msg, strip.right() - s(14.0) - x, tw);
        extra.push(Prim::Text {
            x,
            y: strip.y + (bh + 1 - th) / 2,
            text: msg,
            color: c.dim,
            bold: false,
            term: false,
        });
    }
    if let (Some((col, row, t)), Some(r)) = (m.preedit, pane(tab.focus)) {
        let (x, y) = (r.x + i32::from(col) * cw, r.y + i32::from(row) * ch);
        // Cut at the pane's edge, so a long composition cannot draw over
        // the next pane or the sidebar.
        let t = fit(t, r.right() - x, cw);
        let pw = text_w(&t, cw);
        extra.push(Prim::Rect(Rect { x, y, w: pw, h: ch }, c.term_bg));
        extra.push(Prim::Text {
            x,
            y,
            text: t,
            color: c.term_fg,
            bold: false,
            term: true,
        });
        let u = s(1.0).max(1);
        let line = Rect {
            x,
            y: y + ch - 2 * u,
            w: pw,
            h: u,
        };
        extra.push(Prim::Rect(line, c.term_fg));
    }
    if let (Some(f), Some(r)) = (&m.find, pane(tab.focus)) {
        find_bar(&mut extra, f, c, r, s, (tw, th));
    }
    if let Some(st) = &m.settings {
        out.settings = Some(settings(&mut extra, st, c, m.size, s, (tw, th)));
    }
    if let Some(pk) = &m.picker {
        picker(&mut extra, pk, c, m.size, s, (tw, th));
    }
    if let Some(g) = m.game {
        g.draw(&mut extra, area, m.scale, c, (tw, th));
    }
    if let Some(cm) = &m.commands {
        out.commands = Some(commands(&mut extra, cm, c, m.size, s, (tw, th)));
    }
    out.prims.extend(extra);
    out
}

/// The find bar: one line at the top right of pane `r` with the query and
/// which match is current, or that nothing matches.
fn find_bar(
    p: &mut Vec<Prim>,
    f: &FindBar,
    c: &Ui,
    r: Rect,
    s: impl Fn(f32) -> i32,
    (tw, th): (i32, i32),
) {
    let text = |p: &mut Vec<Prim>, x, y, t: String, color, bold| {
        p.push(Prim::Text {
            x,
            y,
            text: t,
            color,
            bold,
            term: false,
        });
    };
    let (pad, one) = (s(10.0), s(1.0).max(1));
    let (w, h) = (s(300.0).min(r.w).max(0), th + s(10.0));
    let panel = Rect {
        x: r.right() - w,
        y: r.y,
        w,
        h,
    };
    p.push(Prim::Rect(panel, c.border));
    let inner = Rect {
        x: panel.x + one,
        y: panel.y + one,
        w: (w - 2 * one).max(0),
        h: (h - 2 * one).max(0),
    };
    p.push(Prim::Rect(inner, c.side_bg));
    let (left, right, ty) = (inner.x + pad, inner.right() - pad, panel.y + (h - th) / 2);
    text(p, left, ty, "Find".into(), c.name, true);
    let (count, color) = match f.count {
        Some((at, of)) => (format!("{at}/{of}"), c.dim),
        None if f.query.is_empty() => (String::new(), c.dim),
        None => ("no matches".into(), c.error),
    };
    let cx = right - text_w(&count, tw);
    let qx = left + 6 * tw;
    let (query, qc) = if f.query.is_empty() {
        ("type to find", c.dim)
    } else {
        (f.query, c.msg)
    };
    // The end of a long query is the part being typed.
    text(p, qx, ty, fit_left(query, cx - s(8.0) - qx, tw), qc, false);
    if !count.is_empty() {
        text(p, cx, ty, count, color, false);
    }
}

/// What the theme picker and the command palette have in common.
struct List<'a> {
    title: &'a str,
    /// What was typed to narrow the list.
    filter: &'a str,
    /// The names that match it, and the highlighted one.
    names: Vec<&'a str>,
    sel: usize,
    /// Shown when nothing matches.
    empty: &'a str,
    hint: &'a str,
    /// Shown in place of an empty filter.
    prompt: &'a str,
    /// Panel width at 96 DPI.
    width: f32,
}

/// A list panel near the top: the title and the filter, a window of rows
/// that follows the highlight, and a key hint. `side(p, i, row, right)`
/// draws the right end of row `i` up to `right` and returns where the name
/// must end. Returns the panel and each row shown, by index.
fn list(
    p: &mut Vec<Prim>,
    l: &List,
    c: &Ui,
    (w, h): (i32, i32),
    s: impl Fn(f32) -> i32,
    (tw, th): (i32, i32),
    mut side: impl FnMut(&mut Vec<Prim>, usize, Rect, i32) -> i32,
) -> (Rect, Vec<(usize, Rect)>) {
    let text = |p: &mut Vec<Prim>, x, y, t: String, color, bold| {
        p.push(Prim::Text {
            x,
            y,
            text: t,
            color,
            bold,
            term: false,
        });
    };
    let (pad, row_h, one) = (s(12.0), th + s(10.0), s(1.0).max(1));
    let shown = l.names.len().clamp(1, PICKER_ROWS) as i32;
    let pw = s(l.width).min(w - s(32.0)).max(0);
    let ph = 2 * row_h + shown * row_h + s(12.0);
    let panel = Rect {
        x: (w - pw) / 2,
        y: s(56.0).min((h - ph) / 2).max(0),
        w: pw,
        h: ph,
    };
    p.push(Prim::Rect(panel, c.border));
    let inner = inset(panel, one);
    p.push(Prim::Rect(inner, c.side_bg));
    let (left, right) = (inner.x + pad, inner.right() - pad);
    let ty = |row_y: i32| row_y + (row_h - th) / 2;

    let mut y = inner.y + s(4.0);
    text(p, left, ty(y), l.title.into(), c.name, true);
    let fx = left + text_w(l.title, tw) + 2 * tw;
    let (filter, color) = if l.filter.is_empty() {
        (l.prompt, c.dim)
    } else {
        (l.filter, c.msg)
    };
    text(p, fx, ty(y), fit(filter, right - fx, tw), color, false);
    y += row_h;
    let rule = Rect {
        x: inner.x,
        y,
        w: inner.w,
        h: one,
    };
    p.push(Prim::Rect(rule, c.rule));
    y += s(4.0);

    if l.names.is_empty() {
        text(p, left, ty(y), l.empty.into(), c.dim, false);
    }
    let mut rows = Vec::new();
    let first = (l.sel + 1).saturating_sub(PICKER_ROWS);
    for (i, name) in l.names.iter().enumerate().skip(first).take(PICKER_ROWS) {
        let row = Rect {
            x: inner.x,
            y,
            w: inner.w,
            h: row_h,
        };
        let sel = i == l.sel;
        if sel {
            p.push(Prim::Rect(row, c.row_focus));
            p.push(Prim::Rect(Rect { w: s(2.0), ..row }, c.accent));
        }
        let end = side(p, i, row, right);
        let color = if sel { c.name } else { c.msg };
        text(
            p,
            left,
            ty(y),
            fit(name, end - s(8.0) - left, tw),
            color,
            sel,
        );
        rows.push((i, row));
        y += row_h;
    }

    let hy = panel.bottom() - row_h - s(2.0);
    text(p, left, ty(hy), fit(l.hint, right - left, tw), c.dim, false);
    (panel, rows)
}

/// The theme picker: the matching themes, each with a strip of its colours.
fn picker(
    p: &mut Vec<Prim>,
    pk: &Picker,
    c: &Ui,
    size: (i32, i32),
    s: impl Fn(f32) -> i32,
    cells: (i32, i32),
) {
    let l = List {
        title: "Theme",
        filter: pk.filter,
        names: pk.items.iter().map(|t| t.name.as_str()).collect(),
        sel: pk.sel,
        empty: "no theme matches",
        hint: "\u{2191}\u{2193} preview  \u{b7}  Enter keep  \u{b7}  Esc cancel",
        prompt: "type to filter",
        width: 380.0,
    };
    // Six of the theme's colours on its own background.
    let (sq, gap, one) = (s(8.0), s(4.0), s(1.0).max(1));
    let strip_w = 6 * sq + 7 * gap;
    list(p, &l, c, size, &s, cells, |p, i, row, right| {
        let t = pk.items[i];
        let strip = Rect {
            x: right - strip_w,
            y: row.y + (row.h - sq - 2 * gap) / 2,
            w: strip_w,
            h: sq + 2 * gap,
        };
        p.push(Prim::Rect(strip, c.border));
        p.push(Prim::Rect(inset(strip, one), t.pal.bg));
        for (k, &col) in t.pal.ansi[1..7].iter().enumerate() {
            let x = strip.x + gap + k as i32 * (sq + gap);
            let r = Rect {
                x,
                y: strip.y + gap,
                w: sq,
                h: sq,
            };
            p.push(Prim::Rect(r, col));
        }
        strip.x
    });
}

/// The command palette: the matching actions, each with its keys.
fn commands(
    p: &mut Vec<Prim>,
    cm: &Commands,
    c: &Ui,
    size: (i32, i32),
    s: impl Fn(f32) -> i32,
    (tw, th): (i32, i32),
) -> (Rect, Vec<(usize, Rect)>) {
    let l = match cm.rename {
        Some(title) => List {
            title,
            filter: cm.filter,
            names: Vec::new(),
            sel: 0,
            empty: "With no name, blitz picks one again",
            hint: "Enter rename  \u{b7}  Esc cancel",
            prompt: "type a name",
            width: 460.0,
        },
        None => List {
            title: "Commands",
            filter: cm.filter,
            names: cm.items.iter().map(|i| i.0).collect(),
            sel: cm.sel,
            empty: "no command matches",
            hint: "\u{2191}\u{2193} choose  \u{b7}  Enter run  \u{b7}  Esc close",
            prompt: "type to filter",
            width: 460.0,
        },
    };
    list(p, &l, c, size, s, (tw, th), |p, i, row, right| {
        let keys = &cm.items[i].1;
        let x = right - text_w(keys, tw);
        p.push(Prim::Text {
            x,
            y: row.y + (row.h - th) / 2,
            text: keys.clone(),
            color: c.dim,
            bold: false,
            term: false,
        });
        x
    })
}

/// `r` shrunk by `by` on every side.
fn inset(r: Rect, by: i32) -> Rect {
    Rect {
        x: r.x + by,
        y: r.y + by,
        w: (r.w - 2 * by).max(0),
        h: (r.h - 2 * by).max(0),
    }
}

/// The settings panel: a search line, the settings under group headings,
/// each with a switch or a value between arrows, help for the highlighted
/// one and a key hint. The list scrolls only as far as it must to show the
/// highlighted row and its heading.
fn settings(
    p: &mut Vec<Prim>,
    st: &Settings,
    c: &Ui,
    (w, h): (i32, i32),
    s: impl Fn(f32) -> i32,
    (tw, th): (i32, i32),
) -> SettingsHits {
    enum Line {
        Head(&'static str),
        Row(usize),
    }
    let text = |p: &mut Vec<Prim>, x, y, t: String, color, bold| {
        p.push(Prim::Text {
            x,
            y,
            text: t,
            color,
            bold,
            term: false,
        });
    };
    let dot = |p: &mut Vec<Prim>, r: Rect, color| {
        p.push(Prim::Shape {
            r,
            radius: r.h as f32 / 2.0,
            stroke: 0.0,
            color,
        });
    };
    let mut lines = Vec::new();
    for (i, r) in st.rows.iter().enumerate() {
        if i == 0 || st.rows[i - 1].group != r.group {
            lines.push(Line::Head(r.group));
        }
        lines.push(Line::Row(i));
    }

    let (pad, line_h, one, gap) = (s(16.0), th + s(12.0), s(1.0).max(1), s(4.0));
    let head_h = line_h + gap;
    let help_line = th + s(4.0);
    let help_h = s(10.0) + 3 * help_line + gap;
    // Everything but the list: border, search line, rules, help and hint.
    let fixed = 2 * one + head_h + one + 2 * gap + one + help_h + line_h;
    let room = (h - s(48.0) - fixed) / line_h.max(1);
    let shown = room.clamp(1, lines.len().max(1) as i32) as usize;
    // Scroll as little as shows the highlight, and its group's heading
    // when there is room.
    let at = (lines.iter())
        .position(|l| matches!(l, Line::Row(i) if *i == st.sel))
        .unwrap_or(0);
    let head = if at > 0 && matches!(lines[at - 1], Line::Head(_)) && shown > 1 {
        at - 1
    } else {
        at
    };
    let first = (st.top.min(lines.len().saturating_sub(shown)).min(head))
        .max((at + 1).saturating_sub(shown));

    let pw = s(600.0).min(w - s(32.0)).max(0);
    let ph = fixed + shown as i32 * line_h;
    let panel = Rect {
        x: (w - pw) / 2,
        y: s(48.0).min((h - ph) / 2).max(0),
        w: pw,
        h: ph,
    };
    let mut hits = SettingsHits {
        panel,
        rows: Vec::new(),
        top: first,
    };
    p.push(Prim::Rect(panel, c.border));
    let inner = Rect {
        x: panel.x + one,
        y: panel.y + one,
        w: (panel.w - 2 * one).max(0),
        h: (panel.h - 2 * one).max(0),
    };
    p.push(Prim::Rect(inner, c.side_bg));
    let (left, right) = (inner.x + pad, inner.right() - pad);
    let mid = |y: i32, hh: i32| y + (hh - th) / 2;
    let rule = |p: &mut Vec<Prim>, y| {
        let r = Rect {
            x: inner.x,
            y,
            w: inner.w,
            h: one,
        };
        p.push(Prim::Rect(r, c.rule));
    };

    let mut y = inner.y;
    text(p, left, mid(y, head_h), "Settings".into(), c.name, true);
    let fx = left + 10 * tw;
    let (filter, color) = if st.filter.is_empty() {
        ("type to search", c.dim)
    } else {
        (st.filter, c.msg)
    };
    text(
        p,
        fx,
        mid(y, head_h),
        fit(filter, right - fx, tw),
        color,
        false,
    );
    y += head_h;
    rule(p, y);
    y += one + gap;
    let list_y = y;

    if lines.is_empty() {
        text(
            p,
            left,
            mid(y, line_h),
            "No setting matches".into(),
            c.dim,
            false,
        );
    }
    for line in lines.iter().skip(first).take(shown) {
        match *line {
            Line::Head(g) => {
                let ty = y + line_h - th - s(3.0);
                text(p, left, ty, g.to_uppercase(), c.dim, false);
            }
            Line::Row(i) => {
                let r = &st.rows[i];
                let row = Rect {
                    x: inner.x,
                    y,
                    w: inner.w,
                    h: line_h,
                };
                let sel = i == st.sel;
                if sel {
                    p.push(Prim::Rect(row, c.row_focus));
                    p.push(Prim::Rect(Rect { w: s(2.0), ..row }, c.accent));
                }
                // Changed from the default.
                if r.changed {
                    let d = s(5.0);
                    let mark = Rect {
                        x: inner.x + (pad - d) / 2,
                        y: y + (line_h - d) / 2,
                        w: d,
                        h: d,
                    };
                    dot(p, mark, c.accent);
                }
                let (lc, ty) = (if sel { c.name } else { c.msg }, mid(y, line_h));
                text(p, left, ty, fit(r.label, pw / 2 - pad, tw), lc, sel);
                let control = match r.on {
                    Some(on) => {
                        let (sw, sh, knob) = (s(30.0), s(16.0), s(12.0));
                        let track = Rect {
                            x: right - sw,
                            y: y + (line_h - sh) / 2,
                            w: sw,
                            h: sh,
                        };
                        dot(p, track, if on { c.accent } else { c.track });
                        let inset = (sh - knob) / 2;
                        let kx = if on {
                            track.right() - inset - knob
                        } else {
                            track.x + inset
                        };
                        let k = Rect {
                            x: kx,
                            y: track.y + inset,
                            w: knob,
                            h: knob,
                        };
                        dot(p, k, if on { c.chip_fg } else { c.dim });
                        track
                    }
                    None => {
                        let value = fit(&r.value, pw / 2 - pad, tw);
                        let sp = s(8.0);
                        let cw = tw + sp + text_w(&value, tw) + sp + tw;
                        let cx = right - cw;
                        let arrow = |ok| if ok { c.dim } else { c.rule };
                        text(p, cx, ty, "\u{2039}".into(), arrow(r.less), false);
                        text(p, cx + tw + sp, ty, value, lc, false);
                        text(p, right - tw, ty, "\u{203a}".into(), arrow(r.more), false);
                        Rect {
                            x: cx,
                            y,
                            w: cw,
                            h: line_h,
                        }
                    }
                };
                hits.rows.push((i, row, control));
            }
        }
        y += line_h;
    }

    y = list_y + shown as i32 * line_h + gap;
    rule(p, y);
    y += one + s(10.0);
    let meta_y = y + 2 * help_line;
    if let Some(r) = st.rows.get(st.sel) {
        for (k, l) in wrap(r.help, right - left, tw, 2).into_iter().enumerate() {
            text(p, left, y + k as i32 * help_line, l, c.msg, false);
        }
        if st.error.is_none() {
            let meta = format!("{} \u{b7} default {}", r.applies, r.default);
            text(p, left, meta_y, fit(&meta, right - left, tw), c.dim, false);
        }
    }
    if let Some(e) = st.error {
        text(p, left, meta_y, fit(e, right - left, tw), c.error, false);
    }
    let hint = "\u{2191}\u{2193} choose  \u{b7}  \u{2190}\u{2192} change  \u{b7}  Del default  \u{b7}  Esc close";
    let hy = inner.bottom() - line_h;
    text(
        p,
        left,
        mid(hy, line_h),
        fit(hint, right - left, tw),
        c.dim,
        false,
    );
    hits
}

/// `t` broken at spaces into at most `n` lines of `max` pixels; the last
/// ends in an ellipsis when the text goes on.
fn wrap(t: &str, max: i32, cw: i32, n: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for word in t.split_whitespace() {
        match lines.last_mut() {
            Some(l) if text_w(l, cw) + cw + text_w(word, cw) <= max => {
                l.push(' ');
                l.push_str(word);
            }
            _ => lines.push(word.to_string()),
        }
    }
    if lines.len() > n {
        let rest = lines.split_off(n.saturating_sub(1)).join(" ");
        lines.push(fit(&rest, max, cw));
    }
    lines.into_iter().map(|l| fit(&l, max, cw)).collect()
}

/// A session's name cut to fit `max` pixels, and after it its number when
/// another session has the same name, or else nothing.
fn name_parts(x: &Session, max: i32, cw: i32) -> (String, String) {
    let num = x.num.map(|n| format!(" {n}")).unwrap_or_default();
    (fit(&x.name, max - text_w(&num, cw), cw), num)
}

/// Numbers the sessions that share a name with another, so the sidebar
/// can tell them apart: by the number each keeps across restarts, or else
/// its id. The rest go without.
pub fn number_twins(sessions: &mut [Session]) {
    for i in 0..sessions.len() {
        let twin = (sessions.iter().enumerate()).any(|(j, x)| j != i && x.name == sessions[i].name);
        let x = &mut sessions[i];
        x.num = twin.then(|| x.num.unwrap_or(x.id.0));
    }
}

/// What a tab nobody named is called: the last folder of `cwd`, a drive
/// root as it is, or `shell` with none. Either slash separates folders.
pub fn folder_name(cwd: &str) -> String {
    let leaf = (cwd.trim_end_matches(['\\', '/']).rsplit(['\\', '/']).next()).unwrap_or("");
    match leaf {
        "" => "shell".into(),
        l if l.ends_with(':') => cwd.into(),
        l => l.into(),
    }
}

/// The stroke of a needs-you mark: filled until the user has seen the
/// question, then a ring until they answer it.
fn ring(x: Option<&Session>) -> f32 {
    if x.is_some_and(|x| x.seen) { 1.5 } else { 0.0 }
}

/// The state shown on the right of a sidebar row, with how long the turn
/// has run, how long the last one took, or after a minute how long a
/// question has waited.
fn state_word(x: &Session, now: Instant) -> String {
    let waited = now.saturating_duration_since(x.since);
    match (x.state, x.exit_code) {
        (Attn::NeedsYou, _) if waited.as_secs() >= 60 => {
            format!("needs you \u{b7} {}", elapsed(waited))
        }
        (Attn::NeedsYou, _) => "needs you".into(),
        (Attn::Working, _) => {
            let start = x.turn.unwrap_or(x.since);
            format!(
                "working \u{b7} {}",
                elapsed(now.saturating_duration_since(start))
            )
        }
        (Attn::DoneUnseen, _) => match x.took {
            Some(d) => format!("done \u{b7} {}", elapsed(d)),
            None => "done".into(),
        },
        (_, Some(n)) => crate::attention::exit_text(n),
        (Attn::Error, None) => "error".into(),
        (Attn::Idle, None) => "idle".into(),
    }
}

/// `45s`, `12m`, `2h 5m`: seconds only in the first minute, so a long
/// turn wakes blitz once a minute, not every second.
pub fn elapsed(d: Duration) -> String {
    let t = d.as_secs();
    match t {
        0..60 => format!("{t}s"),
        60..3600 => format!("{}m", t / 60),
        _ => format!("{}h {}m", t / 3600, t / 60 % 60),
    }
}

/// When the time [`elapsed`] shows for something that began at `start`
/// next changes, as of `now`; with `seconds` false, as if the first minute
/// showed no time at all.
pub fn next_tick(start: Instant, now: Instant, seconds: bool) -> Instant {
    let t = now.saturating_duration_since(start).as_secs();
    let next = if t < 60 && seconds {
        t + 1
    } else {
        (t / 60 + 1) * 60
    };
    start + Duration::from_secs(next)
}

/// When the time the sidebar shows for a session in `a` next changes, if
/// it shows one.
pub fn row_tick(a: &PaneAttn, now: Instant) -> Option<Instant> {
    match a.state {
        Attn::Working => Some(next_tick(a.turn.unwrap_or(a.since), now, true)),
        Attn::NeedsYou => Some(next_tick(a.since, now, false)),
        _ => None,
    }
}

/// Width of `t` in pixels in a font whose cells are `cw` wide.
// Counted per character, so a combining mark costs a cell; fine for
// names and paths.
fn text_w(t: &str, cw: i32) -> i32 {
    t.chars().map(char_cells).sum::<i32>() * cw
}

fn char_cells(c: char) -> i32 {
    let mut buf = [0u8; 4];
    i32::from(vt::cluster_width(c.encode_utf8(&mut buf)).max(1))
}

/// `t` cut to `max` pixels, ending in an ellipsis when cut.
fn fit(t: &str, max: i32, cw: i32) -> String {
    if text_w(t, cw) <= max {
        return t.to_string();
    }
    let mut out = String::new();
    let mut used = cw;
    for c in t.chars() {
        used += char_cells(c) * cw;
        if used > max {
            break;
        }
        out.push(c);
    }
    if max >= cw {
        out.push('\u{2026}');
    }
    out
}

/// Like `fit` but keeps the end, which is the useful part of a path.
fn fit_left(t: &str, max: i32, cw: i32) -> String {
    if text_w(t, cw) <= max {
        return t.to_string();
    }
    let mut tail: Vec<char> = Vec::new();
    let mut used = cw;
    for c in t.chars().rev() {
        used += char_cells(c) * cw;
        if used > max {
            break;
        }
        tail.push(c);
    }
    let head = if max >= cw { "\u{2026}" } else { "" };
    head.chars().chain(tail.into_iter().rev()).collect()
}

/// Antialiased coverage of a git branch icon in an `n` x `n` square: two
/// commits on a line, and a third joining it from the right.
pub fn branch_mask(n: u32) -> Vec<u8> {
    let k = n as f32 / 10.0;
    let p = |x: f32, y: f32| (x * k, y * k);
    // The curve to the right commit: a cubic on the 10 x 10 grid, as a
    // polyline.
    let curve: Vec<(f32, f32)> = (0..=8)
        .map(|i| {
            let t = i as f32 / 8.0;
            let u = 1.0 - t;
            let b = |a: f32, b: f32, c: f32, d: f32| {
                u * u * u * a + 3.0 * u * u * t * b + 3.0 * u * t * t * c + t * t * t * d
            };
            p(b(7.5, 7.5, 2.5, 2.5), b(4.25, 6.25, 5.75, 6.75))
        })
        .collect();
    let seg = |q: (f32, f32), a: (f32, f32), b: (f32, f32)| {
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let len = (dx * dx + dy * dy).max(1e-6);
        let t = (((q.0 - a.0) * dx + (q.1 - a.1) * dy) / len).clamp(0.0, 1.0);
        (q.0 - a.0 - t * dx).hypot(q.1 - a.1 - t * dy)
    };
    let mut out = Vec::with_capacity((n * n) as usize);
    for y in 0..n {
        for x in 0..n {
            let q = (x as f32 + 0.5, y as f32 + 0.5);
            let ring = |cx: f32, cy: f32| {
                let c = p(cx, cy);
                ((q.0 - c.0).hypot(q.1 - c.1) - 1.25 * k).abs()
            };
            let mut d = ring(2.5, 2.0).min(ring(2.5, 8.0)).min(ring(7.5, 3.0));
            d = d.min(seg(q, p(2.5, 3.25), p(2.5, 6.75)));
            for w in curve.windows(2) {
                d = d.min(seg(q, w[0], w[1]));
            }
            let cov = (k / 2.0 + 0.5 - d).clamp(0.0, 1.0);
            out.push((cov * 255.0).round() as u8);
        }
    }
    out
}

/// Antialiased coverage of a `w` x `h` rounded rectangle with corner
/// `radius`, filled, or a `stroke`-wide outline when `stroke` > 0.
pub fn shape_mask(w: u32, h: u32, radius: f32, stroke: f32) -> Vec<u8> {
    let (hw, hh) = (w as f32 / 2.0, h as f32 / 2.0);
    let r = radius.min(hw).min(hh);
    let mut out = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        for x in 0..w {
            // Signed distance to the edge, negative inside.
            let qx = (x as f32 + 0.5 - hw).abs() - (hw - r);
            let qy = (y as f32 + 0.5 - hh).abs() - (hh - r);
            let d = qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r;
            let mut cov = (0.5 - d).clamp(0.0, 1.0);
            if stroke > 0.0 {
                cov -= (0.5 - d - stroke).clamp(0.0, 1.0);
            }
            out.push((cov * 255.0).round() as u8);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Dir;

    const AREA: Rect = Rect {
        x: 0,
        y: 0,
        w: 1440,
        h: 868,
    };

    fn session(id: u32, name: &str, state: Attn, now: Instant) -> Session {
        Session {
            id: PaneId(id),
            name: name.into(),
            num: None,
            cwd: format!(r"C:\dev\{name}"),
            branch: Some("main".into()),
            state,
            since: now - Duration::from_secs(72),
            turn: None,
            took: None,
            seen: false,
            msg: String::new(),
            progress: None,
            exit_code: None,
        }
    }

    /// Two panes side by side in one tab, and a second tab of one pane.
    fn fleet(expanded: bool) -> (Window, Vec<Session>, Instant) {
        let now = Instant::now();
        let mut a = Tab::new("shop".into(), PaneId(1));
        assert!(a.split(Dir::Right, PaneId(2), AREA, (80, 48)));
        let win = Window {
            tabs: vec![a, Tab::new("db".into(), PaneId(3))],
            active: 0,
            sidebar_expanded: expanded,
        };
        let sessions = vec![
            session(1, "api", Attn::NeedsYou, now),
            session(2, "web", Attn::Working, now),
            session(3, "db", Attn::Idle, now),
        ];
        (win, sessions, now)
    }

    fn model<'a>(win: &'a Window, sessions: &'a [Session], now: Instant) -> ChromeModel<'a> {
        ChromeModel {
            win,
            sessions,
            ui: crate::theme::blitz(false).ui,
            size: (AREA.w, AREA.h),
            scale: 1.0,
            text_cell: (7, 15),
            term_cell: (9, 19),
            now,
            banner: None,
            preedit: None,
            picker: None,
            settings: None,
            spark: None,
            game: None,
            commands: None,
            find: None,
        }
    }

    fn texts(c: &Chrome) -> Vec<&str> {
        c.prims
            .iter()
            .filter_map(|p| match p {
                Prim::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn one_session_has_no_chrome() {
        let now = Instant::now();
        let win = Window {
            tabs: vec![Tab::new("t".into(), PaneId(1))],
            ..Window::default()
        };
        let sessions = [session(1, "a", Attn::NeedsYou, now)];
        let c = build(&model(&win, &sessions, now));
        assert!(c.prims.is_empty() && c.rows.is_empty());
        assert_eq!(c.panes.len(), 1);
        assert_eq!(c.panes[0].1.x, 14);
    }

    #[test]
    fn expanded_sidebar_lists_sessions_with_headers() {
        let (win, sessions, now) = fleet(true);
        let c = build(&model(&win, &sessions, now));
        assert!(c.panes.iter().all(|(_, r)| r.x >= 240));
        // One clickable row per session, in the sidebar, top to bottom.
        let rows: Vec<_> = c.rows.iter().map(|(id, _)| id.0).collect();
        assert_eq!(rows, [1, 2, 3]);
        assert!(c.rows.windows(2).all(|w| w[0].1.bottom() <= w[1].1.y));
        assert!(c.rows.iter().all(|(_, r)| r.right() <= 240));
        // Panes start below their 22 px header strips.
        assert_eq!(c.panes[0].1.y, 22 + 8);
        let t = texts(&c);
        for want in [
            "shop",
            "db",
            "api",
            "web",
            "needs you \u{b7} 1m",
            "working \u{b7} 1m",
        ] {
            assert!(t.contains(&want), "missing {want:?} in {t:?}");
        }
        assert!(t.contains(&"idle"));
        assert!(t.contains(&r"C:\dev\api") && t.contains(&"main"));
        assert!(
            c.prims
                .iter()
                .any(|p| matches!(p, Prim::Branch(r, _) if r.w == 10))
        );
    }

    /// A question the user saw but did not answer stays, outlined.
    #[test]
    fn a_seen_question_is_outlined() {
        let (win, mut sessions, now) = fleet(true);
        let accent = crate::theme::blitz(false).ui.accent;
        let strokes = |sessions: &[Session]| -> Vec<f32> {
            let c = build(&model(&win, sessions, now));
            (c.prims.iter())
                .filter_map(|p| match p {
                    Prim::Shape { stroke, color, .. } if *color == accent => Some(*stroke),
                    _ => None,
                })
                .collect()
        };
        // The pane header's dot, the row's dot and the chip.
        assert_eq!(strokes(&sessions), [0.0; 3]);
        sessions[0].seen = true;
        assert_eq!(strokes(&sessions), [1.5; 3]);
        // Its word takes the name's colour, which reads on any sidebar.
        let name = crate::theme::blitz(false).ui.name;
        let c = build(&model(&win, &sessions, now));
        let word = (c.prims.iter()).find_map(|p| match p {
            Prim::Text { text, color, .. } if text == "needs you \u{b7} 1m" => Some(*color),
            _ => None,
        });
        assert_eq!(word, Some(name));
    }

    /// Sessions that share a name are told apart by a dim number; the
    /// rest show their name alone.
    #[test]
    fn twins_get_their_numbers() {
        let now = Instant::now();
        let mut s = vec![
            session(1, "Claude Code", Attn::Idle, now),
            session(2, "pwsh", Attn::Idle, now),
            session(3, "Claude Code", Attn::Idle, now),
        ];
        number_twins(&mut s);
        let nums: Vec<_> = s.iter().map(|x| x.num).collect();
        assert_eq!(nums, [Some(1), None, Some(3)]);
        s[2].name = "Fix the login".into();
        number_twins(&mut s);
        assert!(s.iter().all(|x| x.num.is_none()));
        // A number kept across restarts is the one shown.
        s[2].name = "Claude Code".into();
        s[2].num = Some(7);
        number_twins(&mut s);
        let nums: Vec<_> = s.iter().map(|x| x.num).collect();
        assert_eq!(nums, [Some(1), None, Some(7)]);

        let (win, mut sessions, now) = fleet(true);
        sessions[1].name = "api".into();
        number_twins(&mut sessions);
        let c = build(&model(&win, &sessions, now));
        let dim = crate::theme::blitz(false).ui.dim;
        let nums: Vec<_> = (c.prims.iter())
            .filter_map(|p| match p {
                Prim::Text { text, color, .. } if text.starts_with(' ') => {
                    Some((text.as_str(), *color))
                }
                _ => None,
            })
            .collect();
        // The focused pane's header and the two sidebar rows.
        assert!(
            nums.contains(&(" 1", dim)) && nums.contains(&(" 2", dim)),
            "{nums:?}"
        );
        assert!(texts(&c).iter().filter(|t| **t == "api").count() >= 4);
    }

    /// A tab nobody named follows the folder of its focused pane.
    #[test]
    fn unnamed_tabs_follow_the_focused_folder() {
        let (mut win, mut sessions, now) = fleet(true);
        win.tabs[0].name = String::new();
        sessions[0].cwd = r"C:\dev\shop\api".into();
        let heads = |win: &Window, sessions: &[Session]| {
            let c = build(&model(win, sessions, now));
            texts(&c).iter().map(|t| t.to_string()).collect::<Vec<_>>()
        };
        assert!(heads(&win, &sessions).contains(&"api".to_owned()));
        win.tabs[0].focus = PaneId(2);
        sessions[1].cwd = r"C:\dev\shop\web\src".into();
        assert!(heads(&win, &sessions).contains(&"src".to_owned()));
        // A name the user gave stays.
        win.tabs[0].name = "shop".into();
        assert!(heads(&win, &sessions).contains(&"shop".to_owned()));
        assert_eq!(folder_name(r"C:\dev\shop"), "shop");
        assert_eq!(folder_name(r"C:\dev\shop\"), "shop");
        assert_eq!(folder_name("/home/me/shop"), "shop");
        assert_eq!(folder_name(r"C:\"), r"C:\");
        assert_eq!(folder_name(""), "shell");
    }

    #[test]
    fn sidebar_says_how_a_session_exited() {
        let (win, mut sessions, now) = fleet(true);
        sessions[1].state = Attn::Error;
        sessions[1].exit_code = Some(0xC000_0005);
        sessions[2].exit_code = Some(u32::MAX);
        let c = build(&model(&win, &sessions, now));
        let t = texts(&c);
        assert!(
            t.contains(&"access violation") && t.contains(&"exit -1"),
            "{t:?}"
        );
    }

    #[test]
    fn sidebar_lists_sessions_as_their_panes_sit() {
        let (mut win, sessions, now) = fleet(true);
        // The focused pane, 2, moves left of pane 1.
        assert!(win.tabs[0].swap(Dir::Left, AREA));
        for expanded in [true, false] {
            win.sidebar_expanded = expanded;
            let c = build(&model(&win, &sessions, now));
            let rows: Vec<_> = c.rows.iter().map(|(id, _)| id.0).collect();
            assert_eq!(rows, [2, 1, 3]);
        }
    }

    #[test]
    fn a_long_branch_leaves_room_for_the_folder() {
        let (win, mut sessions, now) = fleet(true);
        sessions[0].branch = Some("feature/paging-for-every-list-endpoint".into());
        let c = build(&model(&win, &sessions, now));
        let t = texts(&c);
        // The row's text runs from 32 to 220 px, in 7 px cells.
        let branch = t
            .iter()
            .find(|s| s.starts_with("feature/"))
            .expect("branch");
        assert!(branch.ends_with('\u{2026}') && text_w(branch, 7) <= 188 * 45 / 100);
        assert!(t.contains(&r"C:\dev\api"), "{t:?}");
    }

    #[test]
    fn sidebar_width_follows_its_font() {
        assert_eq!(sidebar_w(1440, 1.0, 7), 240);
        assert_eq!(sidebar_w(1440, 1.5, 7), 360);
        // A bigger font: room for 34 characters.
        assert_eq!(sidebar_w(1440, 1.0, 10), 340);
        // Never over 40% of the window.
        assert_eq!(sidebar_w(500, 1.0, 10), 200);

        let (win, sessions, now) = fleet(true);
        let mut m = model(&win, &sessions, now);
        m.text_cell = (10, 18);
        let c = build(&m);
        assert!(c.panes.iter().all(|(_, r)| r.x >= 340));
        assert!(c.rows.iter().all(|(_, r)| r.right() <= 340 && r.w > 300));
    }

    #[test]
    fn sidebar_rows_keep_their_height() {
        let (win, mut sessions, now) = fleet(true);
        sessions[2].msg = "title".into();
        let c = build(&model(&win, &sessions, now));
        // Needs you, working with no message, idle with one.
        assert!(c.rows.windows(2).all(|w| w[0].1.h == w[1].1.h));
    }

    #[test]
    fn progress_reports_keep_or_drop_their_value() {
        let p = |state, pct| Some(Progress { state, pct });
        let steps = [
            (1, Some(40), p(1, Some(40))),
            // An error without a value keeps the last one.
            (2, None, p(2, Some(40))),
            (3, Some(9), p(3, None)),
            (4, None, p(4, None)),
            (1, None, p(1, Some(0))),
            (0, None, None),
        ];
        let mut now = None;
        for (state, pct, want) in steps {
            now = Progress::next(now, state, pct);
            assert_eq!(now, want, "{state} {pct:?}");
        }
    }

    #[test]
    fn reported_progress_shows_on_any_session_and_stands_still() {
        let (win, mut sessions, now) = fleet(true);
        let ui = crate::theme::blitz(false).ui;
        sessions[0].progress = Some(Progress {
            state: 2,
            pct: Some(50),
        });
        // Idle, in the other tab.
        sessions[2].progress = Some(Progress {
            state: 3,
            pct: None,
        });
        let c = build(&model(&win, &sessions, now));
        let bars = |color| -> Vec<i32> {
            (c.prims.iter())
                .filter_map(|p| match p {
                    Prim::Rect(r, rgb) if *rgb == color && r.h == 2 => Some(r.w),
                    _ => None,
                })
                .collect()
        };
        // Half of the header line and of the sidebar row's track.
        let half = bars(ui.error);
        assert!(half.len() == 2 && half.contains(&94), "{half:?}");
        // No telling how far: the whole track, dimmer than progress.
        let dim = crate::render::mix(ui.track, ui.fill);
        assert_eq!(bars(dim), [188]);
    }

    #[test]
    fn collapsed_rail_is_dots_and_corner_labels() {
        let (win, sessions, now) = fleet(false);
        let c = build(&model(&win, &sessions, now));
        assert_eq!(c.panes[0].1.x, 15 + 16);
        assert_eq!(c.panes[0].1.y, 12, "no header strip");
        assert!(c.rows.iter().all(|(_, r)| r.right() <= 15) && c.rows.len() == 3);
        let t = texts(&c);
        assert_eq!(t, ["api", "web"], "only the pane labels");
        // The needs-you pane gets a 2 px accent ring.
        let accent = crate::theme::blitz(false).ui.accent;
        let ring = c
            .prims
            .iter()
            .filter(|p| matches!(p, Prim::Rect(r, a) if *a == accent && (r.w == 2 || r.h == 2)))
            .count();
        assert_eq!(ring, 4);
        // Three rail marks: needs-you dot, working bar, idle dot.
        let dots = c
            .prims
            .iter()
            .filter(|p| matches!(p, Prim::Shape { r, .. } if r.right() <= 15))
            .count();
        assert_eq!(dots, 2);
    }

    #[test]
    fn banner_is_a_strip_under_the_panes() {
        let now = Instant::now();
        let win = Window {
            tabs: vec![Tab::new("t".into(), PaneId(1))],
            ..Window::default()
        };
        let sessions = [session(1, "a", Attn::Idle, now)];
        let mut m = model(&win, &sessions, now);
        m.banner = Some("blitz 0.0.2 is available");
        let c = build(&m);
        let strip = Rect {
            x: 0,
            y: AREA.h - 22,
            w: AREA.w,
            h: 22,
        };
        assert_eq!(c.banner, Some(strip), "even without a sidebar");
        assert_eq!(c.panes[0].1.bottom(), strip.y, "panes end above it");
        assert!(texts(&c).contains(&"blitz 0.0.2 is available"));

        let (win, sessions, now) = fleet(true);
        let mut m = model(&win, &sessions, now);
        m.banner = Some("x");
        let c = build(&m);
        assert_eq!(c.banner.map(|r| (r.x, r.right())), Some((240, AREA.w)));
        assert!(c.panes.iter().all(|(_, r)| r.bottom() <= AREA.h - 22));
    }

    #[test]
    fn preedit_goes_in_the_focused_pane() {
        let (win, sessions, now) = fleet(true);
        let mut m = model(&win, &sessions, now);
        m.preedit = Some((2, 1, "\u{4e2d}a"));
        let c = build(&m);
        assert!(texts(&c).contains(&"\u{4e2d}a"));
        let focus = c.panes.iter().find(|p| p.0 == PaneId(2)).map(|p| p.1);
        let focus = focus.expect("focused pane");
        // The underline spans the wide character's two cells plus one.
        let line = Rect {
            x: focus.x + 18,
            y: focus.y + 19 + 17,
            w: 27,
            h: 1,
        };
        assert!(
            c.prims
                .contains(&Prim::Rect(line, crate::theme::blitz(false).ui.term_fg))
        );
    }

    #[test]
    fn find_bar_sits_at_the_top_right_of_the_focused_pane() {
        let (win, sessions, now) = fleet(true);
        let mut m = model(&win, &sessions, now);
        m.find = Some(FindBar {
            query: "needle",
            count: Some((3, 17)),
        });
        let c = build(&m);
        let t = texts(&c);
        assert!(t.contains(&"Find") && t.contains(&"needle") && t.contains(&"3/17"));
        let focus = c.panes.iter().find(|p| p.0 == PaneId(2)).map(|p| p.1);
        let focus = focus.expect("focused pane");
        let border = m.ui.border;
        let corner = |p: &Prim| {
            matches!(p, Prim::Rect(r, c)
                if *c == border && r.right() == focus.right() && r.y == focus.y && r.w < focus.w)
        };
        assert!(c.prims.iter().any(corner));

        m.find = Some(FindBar {
            query: "zzz",
            count: None,
        });
        let c = build(&m);
        let error = m.ui.error;
        let none = |p: &Prim| matches!(p, Prim::Text { text, color, .. } if text == "no matches" && *color == error);
        assert!(c.prims.iter().any(none));
    }

    fn setting_rows() -> Vec<SettingRow> {
        let row = |group, label, on: Option<bool>| SettingRow {
            group,
            label,
            help: "Help for this setting.",
            applies: "Applies now",
            on,
            value: if on.is_none() {
                "11 pt".into()
            } else {
                String::new()
            },
            less: true,
            more: true,
            default: "on".into(),
            changed: false,
        };
        vec![
            row("Appearance", "Font", None),
            row("Appearance", "Font size", None),
            row("Sessions", "Reopen tabs", Some(true)),
            row("Sessions", "Restore output", Some(false)),
            row("Updates", "Check for updates", Some(true)),
        ]
    }

    #[test]
    fn settings_panel_groups_rows_and_marks_where_to_click() {
        let (win, sessions, now) = fleet(true);
        let mut m = model(&win, &sessions, now);
        m.settings = Some(Settings {
            filter: "",
            rows: setting_rows(),
            sel: 1,
            top: 0,
            error: None,
        });
        let c = build(&m);
        let hits = c.settings.clone().expect("settings hits");
        let t = texts(&c);
        for want in ["Settings", "APPEARANCE", "SESSIONS", "Font size", "11 pt"] {
            assert!(t.contains(&want), "missing {want:?} in {t:?}");
        }
        assert!(t.contains(&"Help for this setting."));
        // Every row is shown, inside the panel, with its control on it.
        let inside = |a: Rect, b: Rect| {
            a.x >= b.x && a.y >= b.y && a.right() <= b.right() && a.bottom() <= b.bottom()
        };
        let shown: Vec<usize> = hits.rows.iter().map(|r| r.0).collect();
        assert_eq!(shown, [0, 1, 2, 3, 4]);
        assert!(
            hits.rows
                .iter()
                .all(|&(_, row, ctl)| { inside(row, hits.panel) && inside(ctl, row) && ctl.w > 0 })
        );
        assert_eq!(hits.top, 0);
    }

    #[test]
    fn settings_list_scrolls_only_as_far_as_the_highlight() {
        let (win, sessions, now) = fleet(true);
        let mut m = model(&win, &sessions, now);
        // Room for a few lines only.
        m.size = (900, 330);
        let mut show = |sel, top| {
            m.settings = Some(Settings {
                filter: "",
                rows: setting_rows(),
                sel,
                top,
                error: Some("Cannot save to config.toml: denied"),
            });
            build(&m).settings.expect("settings hits")
        };
        let first = show(0, 0);
        let n = first.rows.len();
        assert!(n < 5, "the list must not fit: {n} rows");
        // The last row scrolls in at the bottom.
        let last = show(4, first.top);
        assert_eq!(last.rows.last().map(|r| r.0), Some(4));
        // Moving up by one keeps the list where it is while the row shows.
        let up = show(3, last.top);
        assert_eq!(up.top, last.top);
        // Going back to the top shows the first heading again.
        assert_eq!(show(0, up.top).top, 0);
    }

    #[test]
    fn command_palette_follows_the_highlight() {
        let (win, sessions, now) = fleet(true);
        let mut m = model(&win, &sessions, now);
        let items = (0..20)
            .map(|i| ("Split right", format!("Ctrl+{i}")))
            .collect();
        m.commands = Some(Commands {
            filter: "",
            items,
            sel: 15,
            rename: None,
        });
        let c = build(&m);
        let (panel, rows) = c.commands.clone().expect("palette hits");
        let shown: Vec<usize> = rows.iter().map(|r| r.0).collect();
        assert_eq!(shown, (4..16).collect::<Vec<_>>());
        let inside = |r: Rect| r.y >= panel.y && r.bottom() <= panel.bottom();
        assert!(rows.iter().all(|&(_, r)| inside(r)));
        let t = texts(&c);
        assert!(t.contains(&"Commands") && t.contains(&"Ctrl+15"));
        assert!(!t.contains(&"Ctrl+3"), "scrolled out");
    }

    /// Renaming takes the palette's line for the name.
    #[test]
    fn command_palette_takes_a_name() {
        let (win, sessions, now) = fleet(true);
        let mut m = model(&win, &sessions, now);
        m.commands = Some(Commands {
            filter: "",
            items: Vec::new(),
            sel: 0,
            rename: Some("Rename tab"),
        });
        let t: Vec<String> = texts(&build(&m)).iter().map(|t| t.to_string()).collect();
        assert!(t.contains(&"Rename tab".into()) && t.contains(&"type a name".into()));
        m.commands = Some(Commands {
            filter: "shop api",
            items: Vec::new(),
            sel: 0,
            rename: Some("Rename tab"),
        });
        let c = build(&m);
        assert!(texts(&c).contains(&"shop api"));
        assert!(c.commands.expect("palette").1.is_empty(), "no rows to pick");
    }

    #[test]
    fn help_wraps_at_spaces() {
        assert_eq!(wrap("aa bb cc", 35, 7, 2), ["aa bb", "cc"]);
        assert_eq!(wrap("aa bb cc dd ee", 35, 7, 2), ["aa bb", "cc d\u{2026}"]);
        assert!(wrap("", 35, 7, 2).is_empty());
    }

    #[test]
    fn text_is_cut_with_an_ellipsis() {
        assert_eq!(fit("abcdef", 42, 7), "abcdef");
        assert_eq!(fit("abcdefg", 42, 7), "abcde\u{2026}");
        assert_eq!(fit_left(r"C:\dev\shop", 35, 7), "\u{2026}shop");
        assert_eq!(elapsed(Duration::from_secs(72)), "1m");
        assert_eq!(elapsed(Duration::from_secs(7500)), "2h 5m");
    }

    /// Seconds tick for the first minute, then minutes do.
    #[test]
    fn times_tick_each_second_then_each_minute() {
        let t0 = Instant::now();
        let at = |s| t0 + Duration::from_secs(s);
        let ms = |m| t0 + Duration::from_millis(m);
        assert_eq!(next_tick(t0, t0, true), at(1));
        assert_eq!(next_tick(t0, ms(59_500), true), at(60));
        assert_eq!(next_tick(t0, at(60), true), at(120));
        assert_eq!(next_tick(t0, ms(119_999), true), at(120));
        assert_eq!(next_tick(t0, at(3601), true), at(3660));
        assert_eq!(next_tick(t0, t0, false), at(60));
        assert_eq!(next_tick(t0, at(61), false), at(120));
        // Each tick is where the text changes.
        for s in [0, 59, 60, 61, 119, 3599, 3600] {
            let d = |t: Instant| elapsed(t - t0);
            let next = next_tick(t0, at(s), true);
            assert_ne!(d(next), d(next - Duration::from_millis(1)), "{s}");
        }
    }

    /// A turn's time runs across its questions; a question shows how long
    /// it has waited once that is a minute; a result shows how long its
    /// turn took.
    #[test]
    fn rows_show_turn_and_waiting_times() {
        let now = Instant::now();
        let ago = |s| now - Duration::from_secs(s);
        let mut x = session(1, "a", Attn::Working, now);
        x.since = ago(5);
        x.turn = Some(ago(600));
        assert_eq!(state_word(&x, now), "working \u{b7} 10m");
        x.state = Attn::NeedsYou;
        assert_eq!(state_word(&x, now), "needs you");
        x.since = ago(185);
        assert_eq!(state_word(&x, now), "needs you \u{b7} 3m");
        x.state = Attn::DoneUnseen;
        assert_eq!(state_word(&x, now), "done");
        x.took = Some(Duration::from_secs(750));
        assert_eq!(state_word(&x, now), "done \u{b7} 12m");

        // The sidebar wakes when those change: each second of a turn's
        // first minute, then each minute, and a question's each minute.
        let mut a = PaneAttn::new(ago(5));
        a.state = Attn::Working;
        a.turn = Some(ago(600));
        assert_eq!(row_tick(&a, now), Some(ago(600) + Duration::from_secs(660)));
        a.turn = None;
        assert_eq!(row_tick(&a, now), Some(ago(5) + Duration::from_secs(6)));
        a.state = Attn::NeedsYou;
        assert_eq!(row_tick(&a, now), Some(ago(5) + Duration::from_secs(60)));
        a.state = Attn::DoneUnseen;
        assert_eq!(row_tick(&a, now), None);
    }

    #[test]
    fn shapes_are_antialiased() {
        let dot = shape_mask(8, 8, 4.0, 0.0);
        assert_eq!(dot[3 * 8 + 3], 255, "centre is solid");
        assert_eq!(dot[0], 0, "corner is empty");
        let ring = shape_mask(8, 8, 4.0, 1.5);
        assert_eq!(ring[3 * 8 + 3], 0, "ring has a hole");
        assert!(ring[3 * 8] > 128, "ring has an edge");
        let chip = shape_mask(20, 10, 2.0, 0.0);
        assert_eq!(chip[5 * 20 + 1], 255);
        let icon = branch_mask(10);
        assert!(icon[5 * 10 + 2] > 128, "the line between the left commits");
        assert_eq!(icon[9 * 10 + 9], 0, "nothing bottom right");
    }

    #[test]
    fn layout_holds_at_fractional_scales() {
        let (win, sessions, now) = fleet(true);
        for scale in [1.0f32, 1.25, 1.5, 1.75, 2.0] {
            let mut m = model(&win, &sessions, now);
            m.scale = scale;
            m.banner = Some("u");
            let c = build(&m);
            let side = (SIDEBAR_W * scale).round() as i32;
            let strip = c.banner.expect("banner");
            let a = area(&win, m.size, scale, true, 7);
            assert_eq!(
                (a.x, a.right(), a.bottom()),
                (side, AREA.w, strip.y),
                "{scale}"
            );
            // Each grid is its tile less the frame.
            let (fw, fh) = pane_frame(scale, true, true);
            let tiles = win.tabs[0].rects(a);
            assert_eq!(c.panes.len(), tiles.len());
            for ((id, p), (tid, t)) in c.panes.iter().zip(&tiles) {
                assert_eq!(id, tid);
                assert_eq!(
                    (p.x, p.y, p.w, p.h),
                    (t.x + fw / 2, t.y + fh, t.w - fw, t.h - fh)
                );
            }
            assert!(c.rows.iter().all(|(_, r)| r.right() <= side), "{scale}");
            assert!(c.rows.windows(2).all(|w| w[0].1.bottom() <= w[1].1.y));
        }
    }

    #[test]
    fn tiny_tiles_keep_their_grid_inside() {
        let (win, sessions, now) = fleet(true);
        for size in [(260, 30), (241, 1), (250, 0), (0, 0)] {
            let mut m = model(&win, &sessions, now);
            m.size = size;
            let c = build(&m);
            let tiles = win.tabs[0].rects(area(&win, size, 1.0, false, 7));
            for ((_, p), (_, t)) in c.panes.iter().zip(&tiles) {
                assert!(p.w >= 0 && p.h >= 0, "{size:?} {p:?}");
                assert!(
                    p.x >= t.x && p.right() <= t.right().max(t.x),
                    "{size:?} {p:?} {t:?}"
                );
                assert!(
                    p.y >= t.y && p.bottom() <= t.bottom().max(t.y),
                    "{size:?} {p:?} {t:?}"
                );
            }
        }
        // Without a header, a tab of one pane loses only the padding.
        assert_eq!(pane_frame(1.0, true, false), (28, 8));
        assert_eq!(pane_frame(1.0, true, true), (28, 30));
        assert_eq!(pane_frame(1.0, false, true), (32, 12));
        assert_eq!(pane_frame(1.5, true, true), (42, 45));
    }

    #[test]
    fn preedit_is_cut_at_the_pane_edge() {
        let (win, sessions, now) = fleet(true);
        let mut m = model(&win, &sessions, now);
        let long = "\u{65e5}".repeat(200);
        m.preedit = Some((0, 0, &long));
        let c = build(&m);
        let pane = c.panes.iter().find(|p| p.0 == PaneId(2)).expect("pane").1;
        let (text, x) = (c.prims.iter())
            .find_map(|p| match p {
                Prim::Text {
                    text,
                    x,
                    term: true,
                    ..
                } => Some((text.clone(), *x)),
                _ => None,
            })
            .expect("preedit text");
        assert!(
            x + text_w(&text, 9) <= pane.right(),
            "{} cells",
            text.chars().count()
        );
        assert!(text.ends_with('\u{2026}'));
        // Its background and underline stay inside too.
        let ui = crate::theme::blitz(false).ui;
        for p in &c.prims {
            if let Prim::Rect(r, color) = p
                && (*color == ui.term_bg || *color == ui.term_fg)
            {
                assert!(r.right() <= pane.right(), "{r:?}");
            }
        }
    }

    #[test]
    fn picker_keeps_the_highlight_in_view() {
        let (win, sessions, now) = fleet(true);
        let themes: Vec<Theme> = (0..20)
            .map(|i| crate::theme::parse(&format!("t{i:02}"), ""))
            .collect();
        let shown = |sel: usize, size: (i32, i32)| {
            let mut m = model(&win, &sessions, now);
            m.size = size;
            m.picker = Some(Picker {
                filter: "",
                items: themes.iter().collect(),
                sel,
            });
            let c = build(&m);
            let names: Vec<String> = (texts(&c).into_iter())
                .filter(|t| t.starts_with('t') && t.len() == 3)
                .map(str::to_string)
                .collect();
            names
        };
        let top = shown(0, (AREA.w, AREA.h));
        assert_eq!(top.len(), PICKER_ROWS);
        assert_eq!(top.first().map(String::as_str), Some("t00"));
        // The last theme scrolls the window so it is the last row shown.
        let end = shown(19, (AREA.w, AREA.h));
        assert_eq!(end.len(), PICKER_ROWS);
        assert_eq!(end.last().map(String::as_str), Some("t19"));
        assert_eq!(end.first().map(String::as_str), Some("t08"));
        // A tiny window still lays out, with no negative sizes.
        let mut m = model(&win, &sessions, now);
        m.size = (40, 30);
        m.picker = Some(Picker {
            filter: "",
            items: themes.iter().collect(),
            sel: 5,
        });
        let c = build(&m);
        assert!(c.prims.iter().all(|p| match p {
            Prim::Rect(r, _) | Prim::Shape { r, .. } => r.w >= 0 && r.h >= 0,
            _ => true,
        }));

        let mut m = model(&win, &sessions, now);
        m.picker = Some(Picker {
            filter: "zzz",
            items: Vec::new(),
            sel: 0,
        });
        let c = build(&m);
        assert!(texts(&c).contains(&"no theme matches"));
        assert!(texts(&c).contains(&"zzz"));
    }

    #[test]
    fn fit_handles_wide_and_negative_widths() {
        assert_eq!(fit("ab\u{4e2d}c", 28, 7), "ab\u{2026}");
        assert_eq!(fit("ab\u{4e2d}c", 35, 7), "ab\u{4e2d}c");
        assert_eq!(fit("abc", 6, 7), "");
        assert_eq!(fit("abc", 0, 7), "");
        assert_eq!(fit("abc", -5, 7), "");
        assert_eq!(fit("", -5, 7), "");
        assert_eq!(fit_left("\u{4e2d}\u{6587}x", 21, 7), "\u{2026}x");
        assert_eq!(fit_left("abc", -1, 7), "");
        assert_eq!(text_w("a\u{4e2d}", 7), 21);
        assert_eq!(elapsed(Duration::from_secs(0)), "0s");
        assert_eq!(elapsed(Duration::from_secs(3600)), "1h 0m");
    }
}
