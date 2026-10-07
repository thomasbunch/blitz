//! The layout kept between runs: tabs, splits, each pane's folder and
//! where the window was, in `%LOCALAPPDATA%\blitz\session.json`.

use std::fmt::Write;
use std::io::{self, Write as _};
use std::path::PathBuf;
use std::time::Duration;

use crate::hook::{Json, escape_json};
use crate::layout::{self, Axis, Node, PaneId, Rect, Split, Tab};

/// Any other version is ignored and blitz starts fresh.
const VERSION: u32 = 1;
/// A window showing less than this much of itself on every monitor, in
/// pixels each way, counts as lost.
const VISIBLE: i32 = 64;

/// Everything a start needs to put the window back.
#[derive(Clone, Debug, PartialEq)]
pub struct State {
    pub window: Geometry,
    pub sidebar_expanded: bool,
    pub active: usize,
    pub tabs: Vec<TabState>,
}

/// The window's outer position and inner size, in physical pixels. A
/// maximized window keeps the place it goes back to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Geometry {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    pub maximized: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TabState {
    pub name: String,
    /// Leaf indexes in tree order, `a` before `b`, since PaneIds are new
    /// every run.
    pub focus: usize,
    pub zoom: Option<usize>,
    pub root: NodeState,
}

#[derive(Clone, Debug, PartialEq)]
pub enum NodeState {
    Pane(PaneMeta),
    Split {
        axis: Axis,
        ratio: f32,
        a: Box<NodeState>,
        b: Box<NodeState>,
    },
}

/// What a pane needs to start again.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PaneMeta {
    pub cwd: String,
    /// The Claude Code session running in the pane.
    pub claude: Option<String>,
    /// Names the pane's saved output (see [`is_key`]); empty for none.
    pub key: String,
}

impl NodeState {
    fn leaves(&self) -> usize {
        match self {
            NodeState::Pane(_) => 1,
            NodeState::Split { a, b, .. } => a.leaves() + b.leaves(),
        }
    }
}

impl State {
    /// `win` as it is now, with `meta` describing each pane.
    pub fn capture(
        win: &layout::Window,
        window: Geometry,
        meta: impl Fn(PaneId) -> PaneMeta,
    ) -> State {
        let tabs = (win.tabs.iter())
            .map(|t| {
                let panes = t.panes();
                let index = |p| panes.iter().position(|&q| q == p);
                TabState {
                    name: t.name.clone(),
                    focus: index(t.focus).unwrap_or(0),
                    zoom: t.zoom.and_then(index),
                    root: capture(&t.root, &meta),
                }
            })
            .collect();
        State {
            window,
            sidebar_expanded: win.sidebar_expanded,
            active: win.active,
            tabs,
        }
    }

    /// The saved layout with PaneIds counted up from `first` in tree order,
    /// and each of those panes with what it needs to start.
    pub fn layout(&self, first: u32) -> (layout::Window, Vec<(PaneId, &PaneMeta)>) {
        let mut panes = Vec::new();
        let tabs = (self.tabs.iter())
            .map(|t| {
                let start = panes.len();
                let root = build(&t.root, first, &mut panes);
                let ids: Vec<PaneId> = panes[start..].iter().map(|p| p.0).collect();
                let leaf = |i: usize| ids.get(i).copied();
                let focus = leaf(t.focus).unwrap_or(ids[0]);
                Tab {
                    name: t.name.clone(),
                    root,
                    focus,
                    zoom: t.zoom.and_then(leaf),
                    mru: vec![focus],
                }
            })
            .collect::<Vec<_>>();
        let win = layout::Window {
            active: self.active.min(tabs.len().saturating_sub(1)),
            tabs,
            sidebar_expanded: self.sidebar_expanded,
        };
        (win, panes)
    }
}

fn capture(n: &Node, meta: &impl Fn(PaneId) -> PaneMeta) -> NodeState {
    match n {
        Node::Leaf(p) => NodeState::Pane(meta(*p)),
        Node::Split(s) => NodeState::Split {
            axis: s.axis,
            ratio: s.ratio,
            a: Box::new(capture(&s.a, meta)),
            b: Box::new(capture(&s.b, meta)),
        },
    }
}

fn build<'a>(n: &'a NodeState, first: u32, panes: &mut Vec<(PaneId, &'a PaneMeta)>) -> Node {
    match n {
        NodeState::Pane(m) => {
            let id = PaneId(first + panes.len() as u32);
            panes.push((id, m));
            Node::Leaf(id)
        }
        NodeState::Split { axis, ratio, a, b } => Node::Split(Box::new(Split {
            axis: *axis,
            ratio: *ratio,
            a: build(a, first, panes),
            b: build(b, first, panes),
        })),
    }
}

/// `%LOCALAPPDATA%\blitz`. It is created on the first save. Debug builds
/// use their own, so `cargo run` never restores, resumes or deletes the
/// installed blitz's sessions.
pub fn dir() -> Option<PathBuf> {
    let name = if cfg!(debug_assertions) {
        "blitz.dev"
    } else {
        "blitz"
    };
    std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join(name))
}

fn file() -> Option<PathBuf> {
    dir().map(|d| d.join("session.json"))
}

/// The saved session. A missing or broken file reads as none. A byte order
/// mark, which an editor may add, is skipped.
pub fn load() -> Option<State> {
    let text = std::fs::read_to_string(file()?).ok()?;
    from_json(text.strip_prefix('\u{feff}').unwrap_or(&text))
}

/// Writes the session next to the old one, then swaps it in, so a crash
/// mid-write never leaves a torn file. The temporary file is this
/// process's own, so two windows saving at once cannot swap in each
/// other's half-written one.
pub fn save(s: &State) -> io::Result<()> {
    let dir = dir().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no LOCALAPPDATA"))?;
    std::fs::create_dir_all(&dir)?;
    // A crash between the write and the swap leaves a temporary file named
    // for a process that is gone; one this old is no live window's.
    for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
        let name = e.file_name();
        let name = name.to_string_lossy();
        let old = (e.metadata().and_then(|m| m.modified()).ok())
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > Duration::from_secs(60));
        if old && name.starts_with("session.json.") && name.ends_with(".tmp") {
            let _ = std::fs::remove_file(e.path());
        }
    }
    let tmp = dir.join(format!("session.json.{}.tmp", std::process::id()));
    let written = std::fs::File::create(&tmp).and_then(|mut f| {
        f.write_all(to_json(s).as_bytes())?;
        f.sync_all()
    });
    let swapped = written.and_then(|()| std::fs::rename(&tmp, dir.join("session.json")));
    if swapped.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    swapped
}

/// Forgets the session and its saved output, so the next start is fresh.
pub fn clear() {
    if let Some(f) = file() {
        let _ = std::fs::remove_file(f);
    }
    let _ = save_output(&[]);
}

/// Where each pane's saved output goes: `<key>.txt`, by the key the pane
/// has in the session. A key stays with its pane, so however the layout
/// changes, and whatever was saved when, output only ever comes back into
/// the pane it came from.
fn output_dir() -> Option<PathBuf> {
    dir().map(|d| d.join("scrollback"))
}

/// Whether `k` can name saved output: 32 hex digits, as made by
/// [`crate::pty::pane_token`]. A key read from the session file names no
/// other file.
pub fn is_key(k: &str) -> bool {
    k.len() == 32 && k.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The saved output of the pane with key `key`.
pub fn load_output(key: &str) -> Option<String> {
    let dir = output_dir().filter(|_| is_key(key))?;
    std::fs::read_to_string(dir.join(format!("{key}.txt"))).ok()
}

/// Replaces every pane's saved output with `panes`, as (key, text). None
/// deletes it all.
pub fn save_output(panes: &[(String, String)]) -> io::Result<()> {
    let dir =
        output_dir().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no LOCALAPPDATA"))?;
    // Panes that closed since the last save must not come back.
    match std::fs::remove_dir_all(&dir) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    if panes.is_empty() {
        return Ok(());
    }
    std::fs::create_dir_all(&dir)?;
    for (key, text) in panes.iter().filter(|p| is_key(&p.0)) {
        std::fs::write(dir.join(format!("{key}.txt")), text)?;
    }
    Ok(())
}

pub fn to_json(s: &State) -> String {
    let g = s.window;
    let mut out = String::new();
    let _ = write!(
        out,
        "{{\"v\":{VERSION},\"window\":{{\"x\":{},\"y\":{},\"w\":{},\"h\":{},\"maximized\":{}}},\
         \"sidebar_expanded\":{},\"active\":{},\"tabs\":[",
        g.x, g.y, g.w, g.h, g.maximized, s.sidebar_expanded, s.active
    );
    for (i, t) in s.tabs.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str("{\"name\":\"");
        escape_json(&t.name, &mut out);
        let _ = write!(out, "\",\"focus\":{},\"zoom\":", t.focus);
        match t.zoom {
            Some(z) => {
                let _ = write!(out, "{z}");
            }
            None => out.push_str("null"),
        }
        out.push_str(",\"root\":");
        node_json(&t.root, &mut out);
        out.push('}');
    }
    out.push_str("]}\n");
    out
}

fn node_json(n: &NodeState, out: &mut String) {
    match n {
        NodeState::Pane(m) => {
            out.push_str("{\"pane\":{\"cwd\":\"");
            escape_json(&m.cwd, out);
            out.push_str("\",\"claude\":");
            match &m.claude {
                Some(c) => {
                    out.push('"');
                    escape_json(c, out);
                    out.push('"');
                }
                None => out.push_str("null"),
            }
            out.push_str(",\"key\":\"");
            escape_json(&m.key, out);
            out.push_str("\"}}");
        }
        NodeState::Split { axis, ratio, a, b } => {
            let axis = match axis {
                Axis::Row => "row",
                Axis::Column => "column",
            };
            // `{}` on an f32 prints the shortest text that reads back the same.
            // NaN or infinity would print as text JSON has no word for, and the
            // whole session would fail to read back.
            let ratio = if ratio.is_finite() {
                ratio.clamp(0.0, 1.0)
            } else {
                0.5
            };
            let _ = write!(out, "{{\"split\":\"{axis}\",\"ratio\":{ratio},\"a\":");
            node_json(a, out);
            out.push_str(",\"b\":");
            node_json(b, out);
            out.push('}');
        }
    }
}

/// Reads a session written by [`to_json`]. Anything malformed, out of
/// range or from another version is `None`.
pub fn from_json(s: &str) -> Option<State> {
    let j = Json::parse(s)?;
    if int::<u32>(j.get("v"))? != VERSION {
        return None;
    }
    let w = j.get("window")?;
    let window = Geometry {
        x: int(w.get("x"))?,
        y: int(w.get("y"))?,
        w: int(w.get("w"))?,
        h: int(w.get("h"))?,
        maximized: flag(w.get("maximized"))?,
    };
    let Json::Arr(list) = j.get("tabs")? else {
        return None;
    };
    let mut tabs = Vec::new();
    for t in list {
        let root = node(t.get("root")?)?;
        let n = root.leaves();
        let focus = int(t.get("focus"))?;
        let zoom = match t.get("zoom")? {
            Json::Null => None,
            z => Some(int(Some(z))?),
        };
        if focus >= n || zoom.is_some_and(|z| z >= n) {
            return None;
        }
        tabs.push(TabState {
            name: t.get("name")?.as_str()?.into(),
            focus,
            zoom,
            root,
        });
    }
    let active = int(j.get("active"))?;
    if active >= tabs.len() {
        return None;
    }
    Some(State {
        window,
        sidebar_expanded: flag(j.get("sidebar_expanded"))?,
        active,
        tabs,
    })
}

/// Recursion is bounded by `Json`'s depth cap.
fn node(j: &Json) -> Option<NodeState> {
    if let Some(p) = j.get("pane") {
        let claude = match p.get("claude") {
            None | Some(Json::Null) => None,
            Some(c) => Some(c.as_str()?.into()),
        };
        let cwd = p.get("cwd")?.as_str()?.into();
        // Sessions saved before keys have none, and a bad one names nothing.
        let key = (p.get("key").and_then(Json::as_str))
            .filter(|k| is_key(k))
            .unwrap_or_default()
            .into();
        return Some(NodeState::Pane(PaneMeta { cwd, claude, key }));
    }
    let axis = match j.get("split")?.as_str()? {
        "row" => Axis::Row,
        "column" => Axis::Column,
        _ => return None,
    };
    let Some(&Json::Num(ratio)) = j.get("ratio") else {
        return None;
    };
    if !(0.0..=1.0).contains(&ratio) {
        return None;
    }
    Some(NodeState::Split {
        axis,
        ratio: ratio as f32,
        a: Box::new(node(j.get("a")?)?),
        b: Box::new(node(j.get("b")?)?),
    })
}

/// A whole number that fits `T`.
fn int<T: TryFrom<i64>>(v: Option<&Json>) -> Option<T> {
    let Some(&Json::Num(n)) = v else {
        return None;
    };
    // Also rejects infinity, whose fraction is NaN.
    if n.fract() != 0.0 {
        return None;
    }
    T::try_from(n as i64).ok()
}

fn flag(v: Option<&Json>) -> Option<bool> {
    match v? {
        Json::Bool(b) => Some(*b),
        _ => None,
    }
}

/// Moves a window that no monitor shows enough of (its monitor was
/// unplugged, say) to the middle of `primary`, shrunk to fit it.
pub fn on_screen(g: Geometry, monitors: &[Rect], primary: Rect) -> Geometry {
    let (w, h) = (
        g.w.min(i32::MAX as u32) as i32,
        g.h.min(i32::MAX as u32) as i32,
    );
    // In i64: the position comes from a file, and near i32::MIN the overlap
    // of a window far off one side would not fit an i32.
    let seen = monitors.iter().any(|m| {
        let span = |at: i32, len: i32, m_at: i32, m_len: i32| {
            let (at, m_at) = (i64::from(at), i64::from(m_at));
            (at + i64::from(len)).min(m_at + i64::from(m_len)) - at.max(m_at)
        };
        span(g.x, w, m.x, m.w) >= i64::from(VISIBLE.min(w))
            && span(g.y, h, m.y, m.h) >= i64::from(VISIBLE.min(h))
    });
    if seen {
        return g;
    }
    let (w, h) = (w.min(primary.w), h.min(primary.h));
    Geometry {
        x: primary.x + (primary.w - w) / 2,
        y: primary.y + (primary.h - h) / 2,
        w: w.max(0) as u32,
        h: h.max(0) as u32,
        maximized: g.maximized,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Dir;

    const AREA: Rect = Rect {
        x: 0,
        y: 0,
        w: 1001,
        h: 601,
    };

    fn pane(cwd: &str, claude: Option<&str>) -> NodeState {
        NodeState::Pane(PaneMeta {
            cwd: cwd.into(),
            claude: claude.map(Into::into),
            key: String::new(),
        })
    }

    fn split(axis: Axis, ratio: f32, a: NodeState, b: NodeState) -> NodeState {
        NodeState::Split {
            axis,
            ratio,
            a: Box::new(a),
            b: Box::new(b),
        }
    }

    /// Two tabs: a lone pane, then 1 | (2 over (3 | 4)) with focus and
    /// zoom on the third leaf.
    fn sample() -> State {
        State {
            window: Geometry {
                x: -1800,
                y: 40,
                w: 980,
                h: 620,
                maximized: true,
            },
            sidebar_expanded: false,
            active: 1,
            tabs: vec![
                TabState {
                    name: "C:\\".into(),
                    focus: 0,
                    zoom: None,
                    root: pane("C:\\", None),
                },
                TabState {
                    name: "shop \"two\"".into(),
                    focus: 2,
                    zoom: Some(2),
                    root: split(
                        Axis::Row,
                        0.3333,
                        pane(r"C:\dev\shop", None),
                        split(
                            Axis::Column,
                            0.25,
                            pane(
                                r"C:\dev\shop\crates",
                                Some("3f2a0c1e-0000-4000-8000-00000000abcd"),
                            ),
                            split(
                                Axis::Row,
                                0.5,
                                pane(r"C:\Users\me\Café", None),
                                pane("", None),
                            ),
                        ),
                    ),
                },
            ],
        }
    }

    #[test]
    fn round_trips() {
        let s = sample();
        let json = to_json(&s);
        assert_eq!(from_json(&json), Some(s));
        assert!(json.starts_with("{\"v\":1,"), "{json}");
    }

    #[test]
    fn rejects_malformed_input() {
        let good = to_json(&sample());
        let cases = [
            String::new(),
            "garbage".into(),
            "null".into(),
            "[]".into(),
            good[..good.len() / 2].into(),
            good.replace("\"v\":1", "\"v\":2"),
            good.replace("\"v\":1,", ""),
            good.replace("\"active\":1", "\"active\":2"),
            good.replace("\"active\":1", "\"active\":-1"),
            good.replace("\"active\":1", "\"active\":0.5"),
            good.replace("\"focus\":2", "\"focus\":4"),
            good.replace("\"zoom\":2", "\"zoom\":9"),
            good.replace("\"zoom\":2", "\"zoom\":\"2\""),
            good.replace("\"ratio\":0.25", "\"ratio\":1.5"),
            good.replace("\"split\":\"row\"", "\"split\":\"diagonal\""),
            good.replace("\"w\":980", "\"w\":-980"),
            good.replace("\"x\":-1800", "\"x\":1e999"),
            good.replace("\"maximized\":true", "\"maximized\":1"),
            good.replace("\"cwd\":\"\"", "\"cwd\":null"),
            good.replace("\"claude\":null", "\"claude\":7"),
            good.replace("\"zoom\":null,", ""),
            format!("{}\"tabs\":7}}", &good[..good.find("\"tabs\"").unwrap()]),
            format!("{}\"tabs\":{{}}}}", &good[..good.find("\"tabs\"").unwrap()]),
        ];
        for (i, c) in cases.iter().enumerate() {
            assert_eq!(from_json(c), None, "case {i}: {c}");
        }
        // The cases for `tabs` are JSON, so what turns them down is the check
        // on `tabs`, not the parser.
        for c in &cases[cases.len() - 2..] {
            assert!(Json::parse(c).is_some(), "{c}");
        }
        // With no tabs there is no active one.
        let empty = State {
            active: 0,
            tabs: Vec::new(),
            ..sample()
        };
        assert_eq!(from_json(&to_json(&empty)), None);
    }

    #[test]
    fn keys_name_no_other_file() {
        assert!(is_key("0123456789abcdefABCDEF0123456789"));
        let long = "0".repeat(33);
        for k in [
            "",
            "0123",
            r"..\session.json",
            "0123456789abcdef0123456789abcdeg",
            &long,
        ] {
            assert!(!is_key(k), "{k:?}");
        }
        // One read from a file edited by hand is dropped, not the session.
        let bad = to_json(&sample()).replacen("\"key\":\"\"", r#""key":"..\\x""#, 1);
        let s = from_json(&bad).expect("still a session");
        assert!(s.layout(1).1.iter().all(|(_, p)| p.key.is_empty()));
    }

    #[test]
    fn fields_it_does_not_know_are_ignored() {
        let good = to_json(&sample());
        let more = good
            .replacen("{\"v\":1,", "{\"v\":1,\"later\":[1,{\"x\":null}],", 1)
            .replace("\"key\":\"\"}", "\"key\":\"\",\"shell\":\"pwsh\"}");
        assert_ne!(more, good);
        assert_eq!(from_json(&more), Some(sample()));
    }

    #[test]
    fn a_ratio_that_is_not_a_number_still_saves() {
        for (ratio, back) in [
            (f32::NAN, 0.5),
            (f32::INFINITY, 0.5),
            (-0.25, 0.0),
            (1.5, 1.0),
        ] {
            let mut s = sample();
            s.tabs[1].root = split(Axis::Row, ratio, pane("a", None), pane("b", None));
            s.tabs[1].focus = 0;
            s.tabs[1].zoom = None;
            let read = from_json(&to_json(&s)).expect("reads back");
            let NodeState::Split { ratio, .. } = read.tabs[1].root else {
                panic!("{:?}", read.tabs[1].root);
            };
            assert_eq!(ratio, back);
        }
    }

    /// `cargo run` must never restore, resume or delete the sessions of the
    /// installed blitz.
    #[test]
    #[cfg(debug_assertions)]
    fn debug_builds_keep_their_own_state() {
        if let Some(d) = dir() {
            assert_eq!(d.file_name().and_then(|n| n.to_str()), Some("blitz.dev"));
        }
    }

    #[test]
    fn rejects_a_deep_tree_without_overflowing() {
        let leaf = "{\"pane\":{\"cwd\":\"x\",\"claude\":null}}";
        let mut root = leaf.to_string();
        for _ in 0..200 {
            root = format!("{{\"split\":\"row\",\"ratio\":0.5,\"a\":{root},\"b\":{leaf}}}");
        }
        let json = format!(
            "{{\"v\":1,\"window\":{{\"x\":0,\"y\":0,\"w\":1,\"h\":1,\"maximized\":false}},\
             \"sidebar_expanded\":true,\"active\":0,\"tabs\":[{{\"name\":\"t\",\"focus\":0,\
             \"zoom\":null,\"root\":{root}}}]}}"
        );
        assert_eq!(from_json(&json), None);
    }

    #[test]
    fn layout_maps_leaf_indexes_to_fresh_ids() {
        let s = sample();
        let (win, panes) = s.layout(7);
        let ids: Vec<u32> = panes.iter().map(|p| p.0.0).collect();
        assert_eq!(ids, [7, 8, 9, 10, 11]);
        assert_eq!(panes[2].1.cwd, r"C:\dev\shop\crates");
        assert_eq!((win.active, win.sidebar_expanded), (1, false));
        let t = &win.tabs[1];
        assert_eq!(t.panes(), [8, 9, 10, 11].map(PaneId));
        assert_eq!((t.focus, t.zoom), (PaneId(10), Some(PaneId(10))));
        assert_eq!(win.tabs[0].focus, PaneId(7));
    }

    #[test]
    fn capture_then_layout_keeps_the_tree() {
        let mut t = Tab::new("t".into(), PaneId(1));
        t.split(Dir::Right, PaneId(2), AREA, (80, 48));
        t.split(Dir::Down, PaneId(3), AREA, (80, 48));
        t.split(Dir::Left, PaneId(4), AREA, (80, 48));
        t.focus(PaneId(2));
        t.toggle_zoom();
        let win = layout::Window {
            tabs: vec![t],
            active: 0,
            sidebar_expanded: true,
        };
        let s = State::capture(&win, Geometry::default(), |p| PaneMeta {
            cwd: format!("d{}", p.0),
            claude: None,
            key: format!("{:032x}", p.0),
        });
        let s = from_json(&to_json(&s)).expect("reads back");
        // Leaf order is 1, 2, 4, 3: the left split put 4 before 3.
        assert_eq!((s.tabs[0].focus, s.tabs[0].zoom), (1, Some(1)));
        let (back, panes) = s.layout(1);
        let cwds: Vec<&str> = panes.iter().map(|p| p.1.cwd.as_str()).collect();
        assert_eq!(cwds, ["d1", "d2", "d4", "d3"]);
        // Each key stays with its pane, whatever the new numbering.
        for (_, p) in &panes {
            assert_eq!(
                p.key,
                format!("{:032x}", p.cwd[1..].parse::<u32>().unwrap())
            );
        }
        // Same shape and ratios, renumbered in tree order.
        let t = &back.tabs[0];
        assert_eq!(t.rects(AREA), vec![(PaneId(2), AREA)]);
        let mut renamed = win.tabs[0].clone();
        renamed.root = t.root.clone();
        assert_eq!(renamed.dividers(AREA), win.tabs[0].dividers(AREA));
    }

    #[test]
    fn lost_windows_move_to_the_primary_monitor() {
        let primary = Rect {
            x: 0,
            y: 0,
            w: 1920,
            h: 1080,
        };
        let left = Rect {
            x: -2560,
            y: -200,
            w: 2560,
            h: 1440,
        };
        let g = |x, y, w, h| Geometry {
            x,
            y,
            w,
            h,
            maximized: false,
        };
        let both = [primary, left];
        // On either monitor, or across the seam: kept.
        assert_eq!(
            on_screen(g(100, 100, 800, 600), &both, primary),
            g(100, 100, 800, 600)
        );
        assert_eq!(
            on_screen(g(-1800, -100, 800, 600), &both, primary),
            g(-1800, -100, 800, 600)
        );
        assert_eq!(
            on_screen(g(-400, 50, 800, 600), &both, primary),
            g(-400, 50, 800, 600)
        );
        // The left monitor is gone.
        assert_eq!(
            on_screen(g(-1800, -100, 800, 600), &[primary], primary),
            g(560, 240, 800, 600)
        );
        // Only a sliver shows: lost. Too big for the primary: shrunk.
        assert_eq!(
            on_screen(g(1900, 100, 800, 600), &[primary], primary),
            g(560, 240, 800, 600)
        );
        assert_eq!(
            on_screen(g(5000, 0, 3000, 2000), &[primary], primary),
            g(0, 0, 1920, 1080)
        );
        // Minimized windows sit at -32000.
        let min = on_screen(g(-32000, -32000, 160, 28), &both, primary);
        assert_eq!((min.x, min.y), (880, 526));
    }

    /// The place comes from a file, so it can be anything an i32 holds.
    #[test]
    fn far_off_windows_come_back_without_overflowing() {
        let primary = Rect {
            x: 0,
            y: 0,
            w: 1920,
            h: 1080,
        };
        let right = Rect {
            x: 1920,
            y: 0,
            w: 1920,
            h: 1080,
        };
        let g = |x, y, w, h| Geometry {
            x,
            y,
            w,
            h,
            maximized: false,
        };
        for (x, y, w, h) in [
            (-2_147_483_000, 0, 800, 600),
            (0, -2_147_483_000, 800, 600),
            (i32::MIN, i32::MIN, 800, 600),
            (i32::MAX, i32::MAX, 800, 600),
            (i32::MAX - 10, 0, u32::MAX, u32::MAX),
            (i32::MIN, 0, u32::MAX, 600),
        ] {
            let r = on_screen(g(x, y, w, h), &[primary, right], primary);
            assert!(
                r.x >= 0 && r.y >= 0 && r.x + r.w as i32 <= 1920 && r.y + r.h as i32 <= 1080,
                "{x},{y} {w}x{h} -> {r:?}"
            );
        }
        // Covering both monitors from far off the left still shows on them.
        let wide = g(-1_000_000, 0, 2_000_000, 600);
        assert_eq!(on_screen(wide, &[primary, right], primary), wide);
    }
}
