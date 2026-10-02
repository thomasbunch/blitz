//! Window chrome: the session sidebar or its collapsed rail, pane headers,
//! dividers, attention marks, the banner line and the IME preedit, laid out
//! as plain rectangles, rounded shapes and text runs.
//!
//! Nothing here touches the GPU, so layouts are testable on any platform.

use std::time::{Duration, Instant};

use crate::attention::Attn;
use crate::layout::{PaneId, Rect, Tab, Window};

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
    pub light: bool,
    pub accent: u32,
    /// Window client size in pixels.
    pub size: (i32, i32),
    /// DPI scale, 1.0 at 96 DPI.
    pub scale: f32,
    /// Cell size of the sidebar font.
    pub text_cell: (u32, u32),
    /// Cell size of the terminal font.
    pub term_cell: (u32, u32),
    pub now: Instant,
    /// A one-line notice shown at the bottom of the first pane.
    pub banner: Option<&'a str>,
    /// IME composition in the focused pane: column, row and text.
    pub preedit: Option<(u16, u16, &'a str)>,
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
}

/// Width of the expanded sidebar and of the collapsed rail at 96 DPI,
/// each including its 1 px border.
pub const SIDEBAR_W: f32 = 240.0;
pub const RAIL_W: f32 = 15.0;

struct Colors {
    term_bg: u32,
    term_fg: u32,
    side_bg: u32,
    border: u32,
    rule: u32,
    row_focus: u32,
    name: u32,
    dim: u32,
    msg: u32,
    chip_fg: u32,
    track: u32,
    fill: u32,
    error: u32,
    hdr_bg: u32,
    hdr_line: u32,
    hdr_name: u32,
    hdr_cwd: u32,
    rail_focus: u32,
    rail_work: u32,
    idle: u32,
    label: u32,
    label_focus: u32,
    top_track: u32,
}

fn colors(light: bool) -> Colors {
    let pal = if light {
        crate::theme::light()
    } else {
        crate::theme::dark()
    };
    if light {
        Colors {
            term_bg: pal.bg,
            term_fg: pal.fg,
            side_bg: 0xf3f3f1,
            border: 0xdcdcd8,
            rule: 0xdfdfdb,
            row_focus: 0xe6e6e3,
            name: 0x141518,
            dim: 0x5c5f65,
            msg: 0x2f3135,
            chip_fg: 0x17140d,
            track: 0xd4d4d0,
            fill: 0x6f7278,
            error: 0xc8382f,
            hdr_bg: 0xefefec,
            hdr_line: 0xe8e8e4,
            hdr_name: 0x45484d,
            hdr_cwd: 0x686b71,
            rail_focus: 0xe0e0dc,
            rail_work: 0x6b6f76,
            idle: 0xc3c5c8,
            label: 0x63676e,
            label_focus: 0x5c6067,
            top_track: 0xebebe8,
        }
    } else {
        Colors {
            term_bg: pal.bg,
            term_fg: pal.fg,
            side_bg: 0x0f1013,
            border: 0x222429,
            rule: 0x26282d,
            row_focus: 0x1c1d21,
            name: 0xececea,
            dim: 0x8f9298,
            msg: 0xc8c9cc,
            chip_fg: 0x17140d,
            track: 0x2c2e33,
            fill: 0x9a9da3,
            error: 0xe5534b,
            hdr_bg: 0x17181c,
            hdr_line: 0x1b1d21,
            hdr_name: 0xa9abb0,
            hdr_cwd: 0x7d8087,
            rail_focus: 0x1c1d21,
            rail_work: 0x8d9199,
            idle: 0x3a3d43,
            label: 0x7e828a,
            label_focus: 0x8f939a,
            top_track: 0x1e2024,
        }
    }
}

/// Lays out the chrome for one frame.
pub fn build(m: &ChromeModel) -> Chrome {
    let c = colors(m.light);
    let s = |v: f32| (v * m.scale).round() as i32;
    let (tw, th) = (m.text_cell.0 as i32, m.text_cell.1 as i32);
    let (w, h) = m.size;
    let mut out = Chrome::default();
    let Some(tab) = m.win.tabs.get(m.win.active) else {
        return out;
    };
    let fleet = m.win.tabs.iter().map(|t| t.panes().len()).sum::<usize>() >= 2;
    let expanded = m.win.sidebar_expanded;
    let side = match (fleet, expanded) {
        (false, _) => 0,
        (true, true) => s(SIDEBAR_W),
        (true, false) => s(RAIL_W),
    };
    let area = Rect {
        x: side,
        y: 0,
        w: (w - side).max(0),
        h,
    };
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
        let mut content = r;
        if multi && expanded {
            let hh = s(22.0);
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
                Attn::NeedsYou => mark(p, right - s(4.0), cy, 7.0, 0.0, m.accent),
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
            content.y += hh;
            content.h -= hh;
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
                    p.push(Prim::Rect(e, m.accent));
                }
            }
        }
        let (px, py) = if expanded { (14.0, 8.0) } else { (16.0, 12.0) };
        let (px, py) = (s(px), s(py));
        content = Rect {
            x: content.x + px,
            y: content.y + py,
            w: (content.w - 2 * px).max(0),
            h: (content.h - py).max(0),
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
                let mut rh = s(7.0) + l1 + gap + l2 + s(8.0);
                if !x.msg.is_empty() {
                    rh += gap + l3;
                }
                if working {
                    rh += gap + s(5.0) + s(2.0) + s(1.0);
                }
                let row = Rect {
                    x: s(8.0),
                    y,
                    w: s(222.0),
                    h: rh,
                };
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
                    Attn::NeedsYou => mark(p, mx, my, 8.0, 0.0, m.accent),
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
                        color: m.accent,
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
                if !x.msg.is_empty() {
                    ly += gap;
                    let msg = fit(&x.msg, right - left, tw);
                    text(p, left, ly + (l3 - th) / 2, &msg, c.msg, false);
                    ly += l3;
                }
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
                if ti == m.win.active && x.id == t.focus {
                    p.push(Prim::Rect(row, c.rail_focus));
                }
                let (cx, cy) = (row.w / 2, y + row.h / 2);
                match x.state {
                    Attn::NeedsYou => mark(p, cx, cy, 7.0, 0.0, m.accent),
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
    if let (Some(msg), Some(r)) = (m.banner, tab.panes().first().copied().and_then(pane)) {
        let rows = r.h / ch;
        if rows > 0 {
            let y = r.y + (rows - 1) * ch;
            extra.push(Prim::Rect(Rect { y, h: ch, ..r }, c.term_bg));
            extra.push(Prim::Text {
                x: r.x,
                y,
                text: fit(msg, r.w, cw),
                color: c.dim,
                bold: false,
                term: true,
            });
        }
    }
    if let (Some((col, row, t)), Some(r)) = (m.preedit, pane(tab.focus)) {
        let (x, y) = (r.x + i32::from(col) * cw, r.y + i32::from(row) * ch);
        let pw = text_w(t, cw).min(r.right() - x).max(0);
        extra.push(Prim::Rect(Rect { x, y, w: pw, h: ch }, c.term_bg));
        extra.push(Prim::Text {
            x,
            y,
            text: t.to_string(),
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
    out.prims.extend(extra);
    out
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
// ponytail: per character, so a combining mark costs a cell; fine for
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
            light: false,
            accent: 0xf2b84b,
            size: (AREA.w, AREA.h),
            scale: 1.0,
            text_cell: (7, 15),
            term_cell: (9, 19),
            now,
            banner: None,
            preedit: None,
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
        assert!(c.prims.is_empty());
        assert_eq!(c.panes.len(), 1);
        assert_eq!(c.panes[0].1.x, 14);
    }

    #[test]
    fn expanded_sidebar_lists_sessions_with_headers() {
        let (win, sessions, now) = fleet(true);
        let c = build(&model(&win, &sessions, now));
        assert!(c.panes.iter().all(|(_, r)| r.x >= 240));
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
    fn collapsed_rail_is_dots_and_corner_labels() {
        let (win, sessions, now) = fleet(false);
        let c = build(&model(&win, &sessions, now));
        assert_eq!(c.panes[0].1.x, 15 + 16);
        assert_eq!(c.panes[0].1.y, 12, "no header strip");
        let t = texts(&c);
        assert_eq!(t, ["api", "web"], "only the pane labels");
        // The needs-you pane gets a 2 px accent ring.
        let ring = c
            .prims
            .iter()
            .filter(|p| matches!(p, Prim::Rect(r, 0xf2b84b) if r.w == 2 || r.h == 2))
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
    fn banner_and_preedit_go_in_panes() {
        let (win, sessions, now) = fleet(true);
        let mut m = model(&win, &sessions, now);
        m.banner = Some("inbox ConPTY");
        m.preedit = Some((2, 1, "\u{4e2d}a"));
        let c = build(&m);
        let t = texts(&c);
        assert!(t.contains(&"inbox ConPTY") && t.contains(&"\u{4e2d}a"));
        let focus = c.panes.iter().find(|p| p.0 == PaneId(2)).map(|p| p.1);
        let focus = focus.expect("focused pane");
        // The underline spans the wide character's two cells plus one.
        let line = Rect {
            x: focus.x + 18,
            y: focus.y + 19 + 17,
            w: 27,
            h: 1,
        };
        assert!(c.prims.contains(&Prim::Rect(line, colors(false).term_fg)));
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
}
