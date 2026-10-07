//! Window chrome: the session sidebar or its collapsed rail, pane headers,
//! dividers, attention marks, the banner strip and the IME preedit, laid out
//! as plain rectangles, rounded shapes and text runs.
//!
//! Nothing here touches the GPU, so layouts are testable on any platform.

use std::time::{Duration, Instant};

use crate::attention::Attn;
use crate::layout::{PaneId, Rect, Tab, Window};
use crate::theme::{Theme, Ui};

/// One session as the chrome shows it.
#[derive(Clone, Debug)]
pub struct Session {
    pub id: PaneId,
    pub name: String,
    pub cwd: String,
    pub branch: Option<String>,
    pub state: Attn,
    /// When `state` last changed.
    pub since: Instant,
    /// Latest one-line message: the hook message, else the title.
    pub msg: String,
    /// Percent done, when the program reports it.
    pub progress: Option<u8>,
    pub exit_code: Option<u32>,
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

/// Rows the picker shows at once.
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
}

/// Width of the expanded sidebar and of the collapsed rail at 96 DPI,
/// each including its 1 px border.
pub const SIDEBAR_W: f32 = 240.0;
pub const RAIL_W: f32 = 15.0;
/// Height of the banner strip at 96 DPI.
pub const BANNER_H: f32 = 22.0;

/// Height of a pane's header strip at 96 DPI.
const HEADER_H: f32 = 22.0;

/// The part of a `size` window that the active tab's panes share: all of
/// it but the sidebar or rail, shown once there are two sessions, and the
/// banner strip.
pub fn area(win: &Window, size: (i32, i32), scale: f32, banner: bool) -> Rect {
    let s = |v: f32| (v * scale).round() as i32;
    let fleet = win.tabs.iter().map(|t| t.panes().len()).sum::<usize>() >= 2;
    let side = match (fleet, win.sidebar_expanded) {
        (false, _) => 0,
        (true, true) => s(SIDEBAR_W),
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
    let fleet = m.win.tabs.iter().map(|t| t.panes().len()).sum::<usize>() >= 2;
    let expanded = m.win.sidebar_expanded;
    let area = area(m.win, m.size, m.scale, m.banner.is_some());
    let (side, bh) = (area.x, m.banner.map_or(0, |_| s(BANNER_H)));
    let session = |id: PaneId| m.sessions.iter().find(|x| x.id == id);
    // A tab's sessions, in the order the caller lists them.
    let members = |t: &Tab| -> Vec<&Session> {
        let ids = t.panes();
        m.sessions.iter().filter(|x| ids.contains(&x.id)).collect()
    };
    let p = &mut out.prims;
    let text = |p: &mut Vec<Prim>, x, y, t: &str, color, bold| {
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
    // A 2 px line: the track, then `pct` of it filled.
    let progress = |p: &mut Vec<Prim>, r: Rect, pct: Option<u8>, track: Option<u32>, fill| {
        if let Some(track) = track {
            p.push(Prim::Rect(r, track));
        }
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
                let name = fit(&x2.name, right - x, tw);
                let (nc, cc) = if focused {
                    (c.name, c.dim)
                } else {
                    (c.hdr_name, c.hdr_cwd)
                };
                text(p, x, ty, &name, nc, focused);
                let cx = x + text_w(&name, tw) + s(8.0);
                let room = right - s(15.0) - cx;
                text(p, cx, ty, &fit_left(&x2.cwd, room, tw), cc, false);
            }
            let cy = r.y + (hh - 1) / 2;
            match state {
                Attn::NeedsYou => mark(p, right - s(4.0), cy, 7.0, 0.0, c.accent),
                Attn::DoneUnseen => mark(p, right - s(4.0), cy, 7.0, 1.5, c.name),
                Attn::Error => mark(p, right - s(4.0), cy, 7.0, 0.0, c.error),
                Attn::Working => {
                    let line = Rect {
                        y: r.y + hh - 2,
                        h: s(2.0),
                        ..r
                    };
                    progress(p, line, sess.and_then(|x| x.progress), None, c.fill);
                }
                Attn::Idle => {}
            }
        }
        if fleet && !expanded && state == Attn::Working {
            let line = Rect { h: s(2.0), ..r };
            let pct = sess.and_then(|x| x.progress);
            progress(p, line, pct, Some(c.top_track), c.rail_work);
        }
        if multi && !expanded {
            if let Some(x) = sess {
                let label = fit(&x.name, r.w / 2, tw);
                let color = if focused { c.label_focus } else { c.label };
                let lx = r.right() - s(16.0) - text_w(&label, tw);
                text(p, lx, r.bottom() - s(10.0) - th, &label, color, false);
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
            let name = fit(&t.name, cx - s(16.0) - gx, tw);
            let nc = if ti == m.win.active { c.msg } else { c.dim };
            text(p, gx, gy, &name, nc, false);
            text(p, cx, gy, &count, c.dim, false);
            let rx = gx + text_w(&name, tw) + s(8.0);
            let rule = Rect {
                x: rx,
                y: y + gh / 2,
                w: cx - s(8.0) - rx,
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
                let working = x.state == Attn::Working;
                // Every row has room for a message and a progress bar, so
                // output that changes a title or state cannot move the
                // rows below it under the pointer.
                let rh =
                    s(7.0) + l1 + gap + l2 + gap + l3 + gap + s(5.0) + s(2.0) + s(1.0) + s(8.0);
                let row = Rect {
                    x: s(8.0),
                    y,
                    w: s(222.0),
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
                    Attn::NeedsYou => mark(p, mx, my, 8.0, 0.0, c.accent),
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
                        stroke: 0.0,
                        color: c.accent,
                    });
                    let cy = chip.y + (chip_h - th) / 2;
                    text(p, chip.x + s(6.0), cy, &word, c.chip_fg, true);
                    chip.x
                } else {
                    let wx = right - text_w(&word, tw);
                    text(p, wx, ty, &word, c.dim, false);
                    wx
                };
                text(
                    p,
                    left,
                    ty,
                    &fit(&x.name, state_x - s(8.0) - left, tw),
                    c.name,
                    true,
                );
                ly += l1 + gap;
                // Line 2: directory and branch.
                let ty = ly + (l2 - th) / 2;
                let (gap6, icon) = (s(6.0), s(10.0));
                let bw = x
                    .branch
                    .as_deref()
                    .map_or(0, |b| 2 * gap6 + icon + text_w(b, tw));
                let cwd = fit_left(&x.cwd, right - left - bw, tw);
                text(p, left, ty, &cwd, c.dim, false);
                if let Some(b) = x.branch.as_deref() {
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
                if working {
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
                    Attn::NeedsYou => mark(p, cx, cy, 7.0, 0.0, c.accent),
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
    if let Some(st) = &m.settings {
        out.settings = Some(settings(&mut extra, st, c, m.size, s, (tw, th)));
    }
    if let Some(pk) = &m.picker {
        picker(&mut extra, pk, c, m.size, s, (tw, th));
    }
    out.prims.extend(extra);
    out
}

/// The theme picker: a panel near the top with the filter, a window of
/// matching themes, each with a strip of its colours, and a key hint.
fn picker(
    p: &mut Vec<Prim>,
    pk: &Picker,
    c: &Ui,
    (w, h): (i32, i32),
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
    let (pad, row_h, one) = (s(12.0), th + s(10.0), s(1.0).max(1));
    let shown = pk.items.len().clamp(1, PICKER_ROWS) as i32;
    let pw = s(380.0).min(w - s(32.0)).max(0);
    let ph = 2 * row_h + shown * row_h + s(12.0);
    let panel = Rect {
        x: (w - pw) / 2,
        y: s(56.0).min((h - ph) / 2).max(0),
        w: pw,
        h: ph,
    };
    let inset = |r: Rect| Rect {
        x: r.x + one,
        y: r.y + one,
        w: (r.w - 2 * one).max(0),
        h: (r.h - 2 * one).max(0),
    };
    p.push(Prim::Rect(panel, c.border));
    let inner = inset(panel);
    p.push(Prim::Rect(inner, c.side_bg));
    let (left, right) = (inner.x + pad, inner.right() - pad);
    let ty = |row_y: i32| row_y + (row_h - th) / 2;

    let mut y = inner.y + s(4.0);
    text(p, left, ty(y), "Theme".into(), c.name, true);
    let fx = left + 7 * tw;
    let (filter, color) = if pk.filter.is_empty() {
        ("type to filter", c.dim)
    } else {
        (pk.filter, c.msg)
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

    if pk.items.is_empty() {
        text(p, left, ty(y), "no theme matches".into(), c.dim, false);
    }
    // Six of the theme's colours on its own background.
    let (sq, gap) = (s(8.0), s(4.0));
    let strip_w = 6 * sq + 7 * gap;
    let first = (pk.sel + 1).saturating_sub(PICKER_ROWS);
    for (i, t) in pk.items.iter().enumerate().skip(first).take(PICKER_ROWS) {
        let row = Rect {
            x: inner.x,
            y,
            w: inner.w,
            h: row_h,
        };
        let sel = i == pk.sel;
        if sel {
            p.push(Prim::Rect(row, c.row_focus));
            p.push(Prim::Rect(Rect { w: s(2.0), ..row }, c.accent));
        }
        let strip = Rect {
            x: right - strip_w,
            y: y + (row_h - sq - 2 * gap) / 2,
            w: strip_w,
            h: sq + 2 * gap,
        };
        let color = if sel { c.name } else { c.msg };
        let name = fit(&t.name, strip.x - s(8.0) - left, tw);
        text(p, left, ty(y), name, color, sel);
        p.push(Prim::Rect(strip, c.border));
        p.push(Prim::Rect(inset(strip), t.pal.bg));
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
        y += row_h;
    }

    let hint = "\u{2191}\u{2193} preview  \u{b7}  Enter keep  \u{b7}  Esc cancel";
    let hy = panel.bottom() - row_h - s(2.0);
    text(p, left, ty(hy), fit(hint, right - left, tw), c.dim, false);
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

/// The state shown on the right of a sidebar row.
fn state_word(x: &Session, now: Instant) -> String {
    match (x.state, x.exit_code) {
        (Attn::NeedsYou, _) => "needs you".into(),
        (Attn::Working, _) => format!("working \u{b7} {}", elapsed(now - x.since)),
        (Attn::DoneUnseen, _) => "done".into(),
        (_, Some(n)) => format!("exited {}", n as i32),
        (Attn::Error, None) => "error".into(),
        (Attn::Idle, None) => "idle".into(),
    }
}

/// `45s`, `1m 12s`, `2h 5m`.
fn elapsed(d: Duration) -> String {
    let t = d.as_secs();
    match t {
        0..60 => format!("{t}s"),
        60..3600 => format!("{}m {}s", t / 60, t % 60),
        _ => format!("{}h {}m", t / 3600, t / 60 % 60),
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
            cwd: format!(r"C:\dev\{name}"),
            branch: Some("main".into()),
            state,
            since: now - Duration::from_secs(72),
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
            "needs you",
            "working \u{b7} 1m 12s",
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

    #[test]
    fn sidebar_rows_keep_their_height() {
        let (win, mut sessions, now) = fleet(true);
        sessions[2].msg = "title".into();
        let c = build(&model(&win, &sessions, now));
        // Needs you, working with no message, idle with one.
        assert!(c.rows.windows(2).all(|w| w[0].1.h == w[1].1.h));
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
        assert_eq!(elapsed(Duration::from_secs(72)), "1m 12s");
        assert_eq!(elapsed(Duration::from_secs(7500)), "2h 5m");
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
            let a = area(&win, m.size, scale, true);
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
            let tiles = win.tabs[0].rects(area(&win, size, 1.0, false));
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
