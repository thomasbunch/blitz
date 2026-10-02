//! Tabs, and the binary split tree inside each tab.
//!
//! Everything here is in pixels. The caller passes the area the tab is
//! drawn in and the smallest pane it accepts, which is `MIN_COLS` by
//! `MIN_ROWS` cells plus any per-pane header.

/// Width of the line between two panes, in pixels.
pub const DIVIDER: i32 = 1;
/// A split is refused if either half would be narrower than this.
pub const MIN_COLS: i32 = 10;
/// A split is refused if either half would be shorter than this.
pub const MIN_ROWS: i32 = 3;

/// Never reused within a process. Exported to the child as `BLITZ_PANE_ID`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PaneId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    /// Side by side, with a vertical divider.
    Row,
    /// Stacked, with a horizontal divider.
    Column,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

impl Dir {
    fn axis(self) -> Axis {
        match self {
            Dir::Left | Dir::Right => Axis::Row,
            Dir::Up | Dir::Down => Axis::Column,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub fn right(self) -> i32 {
        self.x + self.w
    }

    pub fn bottom(self) -> i32 {
        self.y + self.h
    }

    /// Splits off a divider along `axis`. `a` gets `ratio` of what is left
    /// after the divider, rounded, and `b` gets the rest.
    /// Returns `(a, divider, b)`.
    fn cut(self, axis: Axis, ratio: f32) -> (Rect, Rect, Rect) {
        let Rect { x, y, w, h } = self;
        match axis {
            Axis::Row => {
                let free = (w - DIVIDER).max(0);
                let aw = ((free as f32 * ratio).round() as i32).clamp(0, free);
                (
                    Rect { x, y, w: aw, h },
                    Rect {
                        x: x + aw,
                        y,
                        w: DIVIDER,
                        h,
                    },
                    Rect {
                        x: x + aw + DIVIDER,
                        y,
                        w: free - aw,
                        h,
                    },
                )
            }
            Axis::Column => {
                let free = (h - DIVIDER).max(0);
                let ah = ((free as f32 * ratio).round() as i32).clamp(0, free);
                (
                    Rect { x, y, w, h: ah },
                    Rect {
                        x,
                        y: y + ah,
                        w,
                        h: DIVIDER,
                    },
                    Rect {
                        x,
                        y: y + ah + DIVIDER,
                        w,
                        h: free - ah,
                    },
                )
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Leaf(PaneId),
    Split(Box<Split>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Split {
    pub axis: Axis,
    /// Share of the space given to `a`.
    pub ratio: f32,
    /// Left or top.
    pub a: Node,
    pub b: Node,
}

impl Node {
    pub fn contains(&self, p: PaneId) -> bool {
        match self {
            Node::Leaf(q) => *q == p,
            Node::Split(s) => s.a.contains(p) || s.b.contains(p),
        }
    }

    fn leaves(&self, out: &mut Vec<PaneId>) {
        match self {
            Node::Leaf(p) => out.push(*p),
            Node::Split(s) => {
                s.a.leaves(out);
                s.b.leaves(out);
            }
        }
    }

    fn leaf_mut(&mut self, p: PaneId) -> Option<&mut Node> {
        if *self == Node::Leaf(p) {
            return Some(self);
        }
        match self {
            Node::Leaf(_) => None,
            Node::Split(s) => {
                let side = if s.a.contains(p) { &mut s.a } else { &mut s.b };
                side.leaf_mut(p)
            }
        }
    }

    fn walk(&self, area: Rect, panes: &mut Vec<(PaneId, Rect)>, dividers: &mut Vec<Rect>) {
        match self {
            Node::Leaf(p) => panes.push((*p, area)),
            Node::Split(s) => {
                let (a, d, b) = area.cut(s.axis, s.ratio);
                s.a.walk(a, panes, dividers);
                dividers.push(d);
                s.b.walk(b, panes, dividers);
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Tab {
    /// Heading for this tab's group in the sidebar.
    pub name: String,
    pub root: Node,
    pub focus: PaneId,
    pub zoom: Option<PaneId>,
    /// Most recently focused first.
    pub mru: Vec<PaneId>,
}

impl Tab {
    pub fn new(name: String, pane: PaneId) -> Self {
        Self {
            name,
            root: Node::Leaf(pane),
            focus: pane,
            zoom: None,
            mru: vec![pane],
        }
    }

    /// Panes in reading order: left before right, top before bottom.
    pub fn panes(&self) -> Vec<PaneId> {
        let mut out = Vec::new();
        self.root.leaves(&mut out);
        out
    }

    /// Focusing a pane that the zoom hides ends the zoom.
    pub fn focus(&mut self, p: PaneId) {
        self.focus = p;
        self.mru.retain(|&q| q != p);
        self.mru.insert(0, p);
        if self.zoom.is_some_and(|z| z != p) {
            self.zoom = None;
        }
    }

    /// Where each visible pane goes inside `area`. A zoomed pane fills it.
    pub fn rects(&self, area: Rect) -> Vec<(PaneId, Rect)> {
        match self.zoom {
            Some(z) => vec![(z, area)],
            None => self.tiles(area),
        }
    }

    /// The divider lines between visible panes.
    pub fn dividers(&self, area: Rect) -> Vec<Rect> {
        let mut dividers = Vec::new();
        if self.zoom.is_none() {
            self.root.walk(area, &mut Vec::new(), &mut dividers);
        }
        dividers
    }

    /// Rects for every pane, ignoring the zoom.
    fn tiles(&self, area: Rect) -> Vec<(PaneId, Rect)> {
        let mut panes = Vec::new();
        self.root.walk(area, &mut panes, &mut Vec::new());
        panes
    }

    /// Splits the focused pane in half and focuses `new`, which goes on the
    /// `dir` side. Refused when either half would be smaller than `min`
    /// (width, height).
    pub fn split(&mut self, dir: Dir, new: PaneId, area: Rect, min: (i32, i32)) -> bool {
        let focus = self.focus;
        let Some(&(_, r)) = self.tiles(area).iter().find(|t| t.0 == focus) else {
            return false;
        };
        let axis = dir.axis();
        let (a, _, b) = r.cut(axis, 0.5);
        if a.w.min(b.w) < min.0 || a.h.min(b.h) < min.1 {
            return false;
        }
        let Some(leaf) = self.root.leaf_mut(focus) else {
            return false;
        };
        let (old, new_leaf) = (Node::Leaf(focus), Node::Leaf(new));
        let (a, b) = match dir {
            Dir::Right | Dir::Down => (old, new_leaf),
            Dir::Left | Dir::Up => (new_leaf, old),
        };
        *leaf = Node::Split(Box::new(Split {
            axis,
            ratio: 0.5,
            a,
            b,
        }));
        self.focus(new);
        true
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Window {
    pub tabs: Vec<Tab>,
    pub active: usize,
    /// The session list when true, the narrow dot rail when false. Neither
    /// is shown while there is only one session.
    pub sidebar_expanded: bool,
}

impl Default for Window {
    fn default() -> Self {
        Self {
            tabs: Vec::new(),
            active: 0,
            sidebar_expanded: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect {
        x: 0,
        y: 0,
        w: 1001,
        h: 601,
    };
    /// 10 x 3 cells of 8 x 16 px.
    const MIN: (i32, i32) = (80, 48);

    fn r(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect { x, y, w, h }
    }

    /// 1 | (2 over (3 | 4)): split right, then down, then right again in
    /// the right column.
    fn four() -> Tab {
        let mut t = Tab::new("t".into(), PaneId(1));
        assert!(t.split(Dir::Right, PaneId(2), AREA, MIN));
        assert!(t.split(Dir::Down, PaneId(3), AREA, MIN));
        assert!(t.split(Dir::Right, PaneId(4), AREA, MIN));
        t
    }

    fn ids(v: &[u32]) -> Vec<PaneId> {
        v.iter().map(|&n| PaneId(n)).collect()
    }

    #[test]
    fn four_pane_rects() {
        let t = four();
        assert_eq!(t.panes(), ids(&[1, 2, 3, 4]));
        assert_eq!(t.focus, PaneId(4));
        assert_eq!(t.mru, ids(&[4, 3, 2, 1]));
        assert_eq!(
            t.rects(AREA),
            vec![
                (PaneId(1), r(0, 0, 500, 601)),
                (PaneId(2), r(501, 0, 500, 300)),
                (PaneId(3), r(501, 301, 250, 300)),
                (PaneId(4), r(752, 301, 249, 300)),
            ]
        );
        assert_eq!(
            t.dividers(AREA),
            vec![r(500, 0, 1, 601), r(501, 300, 500, 1), r(751, 301, 1, 300)]
        );
    }

    #[test]
    fn split_left_and_up_put_the_new_pane_first() {
        let mut t = Tab::new("t".into(), PaneId(1));
        assert!(t.split(Dir::Left, PaneId(2), AREA, MIN));
        assert!(t.split(Dir::Up, PaneId(3), AREA, MIN));
        assert_eq!(t.panes(), ids(&[3, 2, 1]));
        assert_eq!(t.focus, PaneId(3));
    }

    #[test]
    fn split_refused_below_minimum() {
        let small = r(0, 0, 150, 100);
        let mut t = Tab::new("t".into(), PaneId(1));
        // One half would be 74 px wide.
        assert!(!t.split(Dir::Right, PaneId(2), small, MIN));
        assert_eq!(t, Tab::new("t".into(), PaneId(1)));
        // 49 px tall halves are still allowed.
        assert!(t.split(Dir::Down, PaneId(2), small, MIN));
        assert!(!t.split(Dir::Down, PaneId(3), small, MIN));

        // Pane 4 is 249 px wide: one more right split fits, two do not.
        let mut t = four();
        assert!(t.split(Dir::Right, PaneId(5), AREA, MIN));
        let before = t.clone();
        assert!(!t.split(Dir::Right, PaneId(6), AREA, MIN));
        assert_eq!(t, before);
    }
}
