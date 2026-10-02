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

    /// Replaces the split that holds leaf `p` with `p`'s sibling and returns
    /// the sibling's panes. The splits above keep their ratios.
    fn remove(&mut self, p: PaneId) -> Option<Vec<PaneId>> {
        let Node::Split(s) = self else {
            return None;
        };
        let sibling = if s.a == Node::Leaf(p) {
            &mut s.b
        } else if s.b == Node::Leaf(p) {
            &mut s.a
        } else {
            return s.a.remove(p).or_else(|| s.b.remove(p));
        };
        let sibling = std::mem::replace(sibling, Node::Leaf(p));
        let mut panes = Vec::new();
        sibling.leaves(&mut panes);
        *self = sibling;
        Some(panes)
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

    /// Focuses the nearest pane in `dir`: one entirely on that side of the
    /// focused pane that overlaps it on the other axis. Ties go to the
    /// larger overlap, then to the most recently used. There is no wrap at
    /// the edges, since jumping to the far side by accident is worse than
    /// not moving. Returns whether focus moved.
    pub fn focus_dir(&mut self, dir: Dir, area: Rect) -> bool {
        let tiles = self.tiles(area);
        let Some(&(_, f)) = tiles.iter().find(|t| t.0 == self.focus) else {
            return false;
        };
        let best = tiles
            .iter()
            .filter(|t| t.0 != self.focus)
            .filter_map(|&(p, r)| {
                let gap = match dir {
                    Dir::Left => f.x - r.right(),
                    Dir::Right => r.x - f.right(),
                    Dir::Up => f.y - r.bottom(),
                    Dir::Down => r.y - f.bottom(),
                };
                let overlap = match dir.axis() {
                    Axis::Row => f.bottom().min(r.bottom()) - f.y.max(r.y),
                    Axis::Column => f.right().min(r.right()) - f.x.max(r.x),
                };
                let rank = self.mru.iter().position(|&q| q == p);
                (gap >= 0 && overlap > 0).then_some((gap, -overlap, rank.unwrap_or(usize::MAX), p))
            })
            .min();
        let Some((.., p)) = best else {
            return false;
        };
        self.focus(p);
        true
    }

    /// Removes `p` and gives its space to its sibling. If `p` had focus, the
    /// most recently focused pane on the sibling's side gets it. Returns
    /// false if `p` is not in this tab or is its only pane; closing the
    /// last pane closes the tab, which is up to the window.
    pub fn close(&mut self, p: PaneId) -> bool {
        let Some(sibling) = self.root.remove(p) else {
            return false;
        };
        self.mru.retain(|&q| q != p);
        if self.zoom == Some(p) {
            self.zoom = None;
        }
        if self.focus == p {
            let mut recent = self.mru.iter().copied();
            let next = recent.find(|q| sibling.contains(q)).unwrap_or(sibling[0]);
            self.focus(next);
        }
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

impl Window {
    /// Closes pane `p` wherever it is. Closing a tab's last pane removes
    /// the tab; the window should close once `tabs` is empty. Returns false
    /// if no tab has `p`.
    pub fn close_pane(&mut self, p: PaneId) -> bool {
        let Some(i) = self.tabs.iter().position(|t| t.root.contains(p)) else {
            return false;
        };
        if !self.tabs[i].close(p) {
            self.tabs.remove(i);
            if i < self.active || self.active >= self.tabs.len() {
                self.active = self.active.saturating_sub(1);
            }
        }
        true
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

    #[test]
    fn focus_moves_to_the_nearest_neighbour() {
        let mut t = four();
        let mut go = |d| {
            t.focus_dir(d, AREA);
            t.focus.0
        };
        assert_eq!(go(Dir::Left), 3);
        assert_eq!(go(Dir::Up), 2);
        assert_eq!(go(Dir::Down), 3, "3 overlaps 2 by 250 px, 4 by 249");
        assert_eq!(go(Dir::Right), 4);
        assert_eq!(go(Dir::Left), 3);
        assert_eq!(go(Dir::Left), 1);
    }

    #[test]
    fn focus_does_not_wrap() {
        let mut t = four();
        assert!(!t.focus_dir(Dir::Right, AREA));
        assert!(!t.focus_dir(Dir::Down, AREA));
        t.focus(PaneId(1));
        assert!(!t.focus_dir(Dir::Left, AREA));
        assert!(!t.focus_dir(Dir::Up, AREA));
        assert_eq!(t.focus, PaneId(1));
        let mut lone = Tab::new("t".into(), PaneId(1));
        assert!(!lone.focus_dir(Dir::Right, AREA));
    }

    #[test]
    fn focus_breaks_ties_by_mru() {
        // From 1, panes 2 and 3 are both 1 px away and overlap it by 300
        // px; the one used last wins.
        let mut t = four();
        t.focus(PaneId(1));
        assert!(t.focus_dir(Dir::Right, AREA));
        assert_eq!(t.focus, PaneId(3));
        t.focus(PaneId(2));
        t.focus(PaneId(1));
        assert!(t.focus_dir(Dir::Right, AREA));
        assert_eq!(t.focus, PaneId(2));
    }

    #[test]
    fn close_promotes_the_sibling() {
        // Pane 2's sibling (3 | 4) takes the whole right column; the root
        // keeps its ratio and focus stays on 4.
        let mut t = four();
        assert!(t.close(PaneId(2)));
        assert_eq!(t.panes(), ids(&[1, 3, 4]));
        assert_eq!(t.focus, PaneId(4));
        assert_eq!(t.mru, ids(&[4, 3, 1]));
        assert_eq!(
            t.rects(AREA),
            vec![
                (PaneId(1), r(0, 0, 500, 601)),
                (PaneId(3), r(501, 0, 250, 601)),
                (PaneId(4), r(752, 0, 249, 601)),
            ]
        );
    }

    #[test]
    fn close_moves_focus_by_mru_within_the_sibling() {
        let mut t = four();
        assert!(t.close(PaneId(4)));
        assert_eq!(t.focus, PaneId(3));

        // Closing 1 hands focus to the most recent of 2, 3, 4, not to the
        // first pane in the sibling.
        let mut t = four();
        t.focus(PaneId(3));
        t.focus(PaneId(1));
        assert!(t.close(PaneId(1)));
        assert_eq!(t.focus, PaneId(3));
        assert_eq!(t.panes(), ids(&[2, 3, 4]));
        assert_eq!(t.rects(AREA)[0], (PaneId(2), r(0, 0, 1001, 300)));
    }

    #[test]
    fn close_refuses_the_only_pane_and_strangers() {
        let mut t = Tab::new("t".into(), PaneId(1));
        assert!(!t.close(PaneId(1)));
        assert!(!t.close(PaneId(9)));
        let mut t = four();
        assert!(!t.close(PaneId(9)));
        assert_eq!(t, four());
    }

    #[test]
    fn closing_the_last_pane_closes_the_tab() {
        let mut w = Window::default();
        for n in 1..=3 {
            w.tabs.push(Tab::new(format!("t{n}"), PaneId(n)));
        }
        assert!(w.tabs[1].split(Dir::Right, PaneId(4), AREA, MIN));
        w.active = 2;

        assert!(w.close_pane(PaneId(4)));
        assert_eq!(w.tabs.len(), 3);
        // A tab before the active one goes away; the same tab stays active.
        assert!(w.close_pane(PaneId(1)));
        assert_eq!((w.tabs.len(), w.active), (2, 1));
        assert_eq!(w.tabs[w.active].name, "t3");
        // The active tab was last, so the one before it becomes active.
        assert!(w.close_pane(PaneId(3)));
        assert_eq!((w.tabs.len(), w.active), (1, 0));
        assert!(!w.close_pane(PaneId(3)));
        assert!(w.close_pane(PaneId(2)));
        assert!(w.tabs.is_empty());
        assert_eq!(w.active, 0);
    }
}
